//! End-to-end harness tests for CLI <-> daemon integration.
//!
//! These tests spin up a live daemon API server in-process, then execute the
//! real `hyper` binary against it to verify cross-crate behavior.

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_core::config::{BootConfig, ConfigManager};
use hypercolor_core::input::{BrowserInputHandle, InputManager};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::interaction_routing::InteractionRoutingControl;
use hypercolor_daemon::startup::{DaemonState, default_config};
use hypercolor_types::config::{RenderAccelerationMode, ServoGpuImportMode};
use tempfile::TempDir;
use tokio::sync::{Mutex, oneshot};

const HEALTH_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const CLI_TIMEOUT: Duration = Duration::from_secs(15);
static PATH_OVERRIDE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct DaemonHarness {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: Option<tokio::task::JoinHandle<()>>,
    daemon_state: Option<DaemonState>,
    _paths: TestPathsGuard,
}

struct TestPathsGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    temp_dir: TempDir,
}

impl TestPathsGuard {
    async fn new() -> Result<Self> {
        let lock = PATH_OVERRIDE_LOCK.lock().await;
        let temp_dir = tempfile::tempdir().context("failed to create temp dir")?;
        ConfigManager::set_config_dir_override(Some(temp_dir.path().join("config")));
        ConfigManager::set_data_dir_override(Some(temp_dir.path().join("data")));
        Ok(Self {
            _lock: lock,
            temp_dir,
        })
    }

    fn config_path(&self) -> std::path::PathBuf {
        self.temp_dir
            .path()
            .join("config")
            .join("hypercolor-e2e.toml")
    }
}

impl Drop for TestPathsGuard {
    fn drop(&mut self) {
        ConfigManager::set_config_dir_override(None);
        ConfigManager::set_data_dir_override(None);
    }
}

impl DaemonHarness {
    async fn start() -> Result<Self> {
        let paths = TestPathsGuard::new().await?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("failed to bind test listener")?;
        let port = listener
            .local_addr()
            .context("failed to read listener local address")?
            .port();

        let mut config = default_config();
        "127.0.0.1".clone_into(&mut config.daemon.listen_address);
        config.daemon.port = port;
        "none".clone_into(&mut config.daemon.start_scene);
        config.audio.enabled = false;
        config.capture.enabled = false;
        config.input.enabled = false;
        config.session.enabled = false;
        config.effect_engine.compositor_acceleration_mode = RenderAccelerationMode::Cpu;
        config.rendering.servo_gpu_import.mode = ServoGpuImportMode::Off;
        config.effect_engine.watch_effects = false;
        config.discovery.background_enabled = false;
        config.discovery.mdns_enabled = false;
        config.discovery.blocks_scan = false;
        config.network.mdns_publish = false;

        let config_manager = Arc::new(ConfigManager::from_config_unchecked(
            paths.config_path(),
            config.clone(),
        ));
        let mut daemon_state = DaemonState::initialize(
            BootConfig::from_config_unchecked(config.clone()),
            config_manager,
        )
        .context("failed to initialize daemon state")?;
        install_browser_only_input(&mut daemon_state);
        daemon_state
            .start()
            .await
            .context("failed to start daemon state")?;
        if daemon_state.input_publication_demands().is_none() {
            let _ = daemon_state.shutdown().await;
            bail!("daemon started without an input publication pump");
        }

        let app_state = Arc::new(AppState::from_daemon_state(&daemon_state));
        let router = api::build_router(app_state, None);

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

        let harness = Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            server_task: Some(server_task),
            daemon_state: Some(daemon_state),
            _paths: paths,
        };

        if let Err(error) = wait_for_health(port, HEALTH_WAIT_TIMEOUT).await {
            return match Box::pin(harness.shutdown()).await {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(error.context(format!(
                    "daemon health failure cleanup also failed: {cleanup_error:#}"
                ))),
            };
        }

        Ok(harness)
    }

    fn port(&self) -> u16 {
        self.port
    }

    async fn shutdown(mut self) -> Result<()> {
        let mut first_error = None;
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if let Some(mut task) = self.server_task.take() {
            match tokio::time::timeout(SERVER_SHUTDOWN_TIMEOUT, &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => record_first_error(
                    &mut first_error,
                    anyhow!("API server task join failed: {error}"),
                ),
                Err(_) => {
                    record_first_error(
                        &mut first_error,
                        anyhow!("timed out waiting for API server shutdown"),
                    );
                    task.abort();
                    let _ = task.await;
                }
            }
        }

        if let Some(mut state) = self.daemon_state.take()
            && let Err(error) = state.shutdown().await
        {
            record_first_error(
                &mut first_error,
                error.context("failed to shut down daemon state"),
            );
        }

        first_error.map_or(Ok(()), Err)
    }
}

fn install_browser_only_input(daemon_state: &mut DaemonState) {
    let config = daemon_state.config();
    let browser_input = BrowserInputHandle::new();
    let interaction_routing = InteractionRoutingControl::new(
        browser_input.registry(),
        1,
        config.input.daemon_route,
        config.input.preview_route,
    );
    let input_manager = InputManager::new();
    let input_status = input_manager.source_status_registry();

    daemon_state.input_manager = Arc::new(Mutex::new(input_manager));
    daemon_state.input_status = input_status;
    daemon_state.browser_input = browser_input;
    daemon_state.interaction_routing = interaction_routing;
}

fn record_first_error(first_error: &mut Option<anyhow::Error>, error: anyhow::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
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
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_hypercolor"));
    cmd.kill_on_drop(true);
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
    let harness = Box::pin(DaemonHarness::start()).await?;
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
        let applied_effect_layer = activation["zone"]["layers"]
            .as_array()
            .and_then(|layers| layers.last())
            .is_some_and(|layer| layer["source"]["type"] == serde_json::json!("effect"));
        if !applied_effect_layer {
            bail!(
                "expected apply response to carry the new effect layer, got {}",
                activation["zone"]
            );
        }

        let status_after = run_hyper_json(port, &["status"]).await?;
        if status_after["active_effect"] != serde_json::json!("Audio Pulse") {
            bail!(
                "expected status.active_effect to be Audio Pulse, got {}",
                status_after["active_effect"]
            );
        }

        let stop = run_hyper_json(port, &["effects", "stop"]).await?;
        let cleared = stop["zones"].as_array().is_some_and(|zones| {
            zones.iter().all(|zone| {
                zone["role"] == serde_json::json!("display")
                    || zone["layers"]
                        .as_array()
                        .is_some_and(std::vec::Vec::is_empty)
            })
        });
        if !cleared {
            bail!("expected stop to return a cleared scene, got {stop}");
        }

        Ok(())
    }
    .await;

    let shutdown_result = Box::pin(harness.shutdown()).await;
    test_result.and(shutdown_result)
}
