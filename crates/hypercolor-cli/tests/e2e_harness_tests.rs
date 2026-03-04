//! End-to-end harness tests for CLI <-> daemon integration.
//!
//! These tests spin up a live daemon API server in-process, then execute the
//! real `hyper` binary against it to verify cross-crate behavior.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hypercolor_daemon::api::{self, AppState};
use hypercolor_daemon::startup::{DaemonState, default_config};
use tempfile::TempDir;
use tokio::sync::oneshot;

const HEALTH_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const CLI_TIMEOUT: Duration = Duration::from_secs(5);

struct DaemonHarness {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: Option<tokio::task::JoinHandle<()>>,
    daemon_state: Option<DaemonState>,
    #[allow(dead_code)]
    temp_dir: TempDir,
}

impl DaemonHarness {
    async fn start() -> Result<Self> {
        let temp_dir = tempfile::tempdir().context("failed to create temp dir")?;
        let config_path = temp_dir.path().join("hypercolor-e2e.toml");

        let mut config = default_config();
        "127.0.0.1".clone_into(&mut config.daemon.listen_address);
        config.daemon.port = reserve_loopback_port()?;

        let mut daemon_state = DaemonState::initialize(&config, config_path)
            .context("failed to initialize daemon state")?;

        let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
        let router = api::build_router(app_state, None);
        let bind = format!("{}:{}", config.daemon.listen_address, config.daemon.port);
        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .with_context(|| format!("failed to bind test listener at {bind}"))?;
        let port = listener
            .local_addr()
            .context("failed to read listener local address")?
            .port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        });

        if let Err(error) = wait_for_health(port, HEALTH_WAIT_TIMEOUT).await {
            let _ = shutdown_tx.send(());
            let _ = daemon_state.shutdown().await;
            let _ = server_task.await;
            return Err(error);
        }

        Ok(Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            server_task: Some(server_task),
            daemon_state: Some(daemon_state),
            temp_dir,
        })
    }

    fn port(&self) -> u16 {
        self.port
    }

    async fn shutdown(mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if let Some(task) = self.server_task.take() {
            tokio::time::timeout(SERVER_SHUTDOWN_TIMEOUT, task)
                .await
                .context("timed out waiting for API server shutdown")?
                .context("API server task join failed")?;
        }

        if let Some(mut state) = self.daemon_state.take() {
            state
                .shutdown()
                .await
                .context("failed to shut down daemon state")?;
        }

        Ok(())
    }
}

fn reserve_loopback_port() -> Result<u16> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).context("failed to reserve loopback port")?;
    let port = listener
        .local_addr()
        .context("failed to inspect reserved loopback port")?
        .port();
    Ok(port)
}

async fn wait_for_health(port: u16, timeout: Duration) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/health");
    let deadline = Instant::now() + timeout;

    loop {
        if let Ok(response) = client.get(&url).send().await
            && response.status().is_success()
        {
            return Ok(());
        }

        if Instant::now() >= deadline {
            bail!(
                "daemon health endpoint did not become ready at {url} within {}ms",
                timeout.as_millis()
            );
        }

        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

async fn run_hyper_json(port: u16, args: &[&str]) -> Result<serde_json::Value> {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_hyper"));
    cmd.arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--json")
        .args(args);

    let output = tokio::time::timeout(CLI_TIMEOUT, cmd.output())
        .await
        .context("timed out waiting for hyper CLI process")?
        .context("failed to execute hyper CLI")?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "hyper CLI failed (status={}):\nstdout:\n{}\nstderr:\n{}",
            output.status,
            stdout,
            stderr
        );
    }

    serde_json::from_slice(&output.stdout).context("failed to parse CLI JSON output")
}

#[tokio::test]
async fn cli_e2e_status_and_effect_lifecycle_round_trip() -> Result<()> {
    let harness = DaemonHarness::start().await?;
    let port = harness.port();

    let test_result = async {
        let status_before = run_hyper_json(port, &["status"]).await?;
        if status_before["running"] != serde_json::json!(true) {
            bail!("expected running=true, got {}", status_before["running"]);
        }

        let effect_list = run_hyper_json(port, &["effects", "list"]).await?;
        let has_effects = effect_list["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty());
        if !has_effects {
            bail!("expected at least one effect in catalog");
        }

        let activation = run_hyper_json(port, &["effects", "activate", "audio_pulse"]).await?;
        if activation["effect"]["name"] != serde_json::json!("audio_pulse") {
            bail!(
                "expected active effect name audio_pulse, got {}",
                activation["effect"]["name"]
            );
        }

        let status_after = run_hyper_json(port, &["status"]).await?;
        if status_after["active_effect"] != serde_json::json!("audio_pulse") {
            bail!(
                "expected status.active_effect to be audio_pulse, got {}",
                status_after["active_effect"]
            );
        }

        let stop = run_hyper_json(port, &["effects", "stop"]).await?;
        if stop["stopped"] != serde_json::json!(true) {
            bail!("expected stopped=true, got {}", stop["stopped"]);
        }

        Ok(())
    }
    .await;

    let shutdown_result = harness.shutdown().await;
    test_result.and(shutdown_result)
}
