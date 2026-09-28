//! End-to-end harness tests for CLI <-> daemon integration.
//!
//! These tests spin up a live daemon API server in-process, then execute the
//! real `hypercolor` binary against it to verify cross-crate behavior.
//!
//! The lifecycle test body runs in a re-executed copy of this binary rooted in
//! a fresh sandbox: HOME, every XDG root, the runtime directory, and the CLI
//! config point inside it, and inherited `HYPERCOLOR_*` variables are gone.
//! The in-process daemon resolves its directories from that environment, so
//! without the sandbox it shares the invoking user's state directory with any
//! daemon they run, and writes its runtime session and audit trail there.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use hypercolor_core::config::{BootConfig, ConfigManager};
use hypercolor_daemon::api;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::startup::{DaemonState, default_config};
use hypercolor_types::config::{RenderAccelerationMode, ServoGpuImportMode};
use tokio::sync::{Mutex, oneshot};

const HEALTH_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const CLI_TIMEOUT: Duration = Duration::from_secs(15);
const SANDBOXED_RUN_TIMEOUT: Duration = Duration::from_mins(2);
/// Name of the test [`sandboxed_run`] re-executes; it must match the fn.
const LIFECYCLE_TEST: &str = "cli_e2e_status_and_effect_lifecycle_round_trip";
/// Names the sandbox root to the re-executed copy of this binary.
const SANDBOX_ENV: &str = "HYPERCOLOR_CLI_E2E_SANDBOX";
static PATH_OVERRIDE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Directory roots of one sandboxed run.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn config_home(&self) -> PathBuf {
        self.root.join("config")
    }

    fn data_home(&self) -> PathBuf {
        self.root.join("data")
    }

    fn state_home(&self) -> PathBuf {
        self.root.join("state")
    }

    fn cache_home(&self) -> PathBuf {
        self.root.join("cache")
    }

    fn runtime_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    fn cli_config(&self) -> PathBuf {
        self.config_home().join("hypercolor").join("cli.toml")
    }

    fn create_dirs(&self) -> Result<()> {
        for dir in [
            self.home(),
            self.config_home(),
            self.data_home(),
            self.state_home(),
            self.cache_home(),
            self.runtime_dir(),
        ] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(self.runtime_dir(), std::fs::Permissions::from_mode(0o700))
                .context("failed to restrict the sandbox runtime directory")?;
        }
        Ok(())
    }
}

/// Strip every inherited variable that could point a daemon or CLI at the
/// invoking user's directories, daemon, or session bus, and keep its
/// loopback requests off any proxy, including one the OS configures.
fn strip_ambient_environment(command: &mut tokio::process::Command) {
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        let upper = key.to_ascii_uppercase();
        if key.starts_with("HYPERCOLOR_")
            || key.starts_with("XDG_")
            || key == "DBUS_SESSION_BUS_ADDRESS"
            || upper.ends_with("_PROXY")
        {
            command.env_remove(key);
        }
    }
    // The wildcard covers names; loopback IP literals need their own entries.
    command
        .env("NO_PROXY", "*,127.0.0.1,::1")
        .env("no_proxy", "*,127.0.0.1,::1");
}

/// Run `test_name` from this binary inside a fresh sandbox and fail with its
/// output if it fails. Returns the sandbox when this process is already that
/// sandboxed run, so the caller runs the test body only there.
async fn sandboxed_run(test_name: &str) -> Result<Option<Sandbox>> {
    if let Some(root) = std::env::var_os(SANDBOX_ENV) {
        return Ok(Some(Sandbox {
            root: PathBuf::from(root),
        }));
    }

    let root = tempfile::tempdir().context("failed to create the sandbox")?;
    let sandbox = Sandbox {
        root: root.path().to_path_buf(),
    };
    sandbox.create_dirs()?;

    let mut command =
        tokio::process::Command::new(std::env::current_exe().context("test executable")?);
    command
        .kill_on_drop(true)
        .args(["--exact", test_name, "--nocapture"]);
    strip_ambient_environment(&mut command);
    command
        .env(SANDBOX_ENV, &sandbox.root)
        .env("HOME", sandbox.home())
        .env("XDG_CONFIG_HOME", sandbox.config_home())
        .env("XDG_DATA_HOME", sandbox.data_home())
        .env("XDG_STATE_HOME", sandbox.state_home())
        .env("XDG_CACHE_HOME", sandbox.cache_home())
        .env("XDG_RUNTIME_DIR", sandbox.runtime_dir())
        .env("HYPERCOLOR_CLI_CONFIG", sandbox.cli_config());

    let output = tokio::time::timeout(SANDBOXED_RUN_TIMEOUT, command.output())
        .await
        .with_context(|| format!("sandboxed {test_name} timed out"))?
        .with_context(|| format!("failed to start sandboxed {test_name}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !stdout.contains("1 passed") {
        bail!(
            "sandboxed {test_name} failed (status={}):\nstdout:\n{stdout}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(None)
}

/// Fail loudly when any directory the daemon or CLI resolves escapes the
/// sandbox.
fn assert_confined(sandbox: &Sandbox) -> Result<()> {
    let pinned = [
        ("config", ConfigManager::config_dir()),
        ("data", ConfigManager::data_dir()),
        ("state", ConfigManager::state_dir()),
    ];
    for (tier, dir) in pinned.into_iter().chain(environment_resolved_dirs()) {
        if !dir.starts_with(&sandbox.root) {
            bail!(
                "{tier} directory {} escapes the sandbox {}",
                dir.display(),
                sandbox.root.display()
            );
        }
    }
    Ok(())
}

/// Directories resolved from HOME and the XDG roots rather than pinned by
/// override. An unresolvable home reads as an empty path, which fails the
/// confinement check.
#[cfg(unix)]
fn environment_resolved_dirs() -> Vec<(&'static str, PathBuf)> {
    use hypercolor_core::config::paths;
    vec![
        ("cache", ConfigManager::cache_dir()),
        ("servo cache", paths::servo_runtime_cache_dir()),
        ("home", paths::home_dir().unwrap_or_default()),
    ]
}

/// Windows resolves these through known-folder APIs, not the environment; the
/// daemon's config, data, and state tiers stay pinned by override there.
#[cfg(not(unix))]
fn environment_resolved_dirs() -> Vec<(&'static str, PathBuf)> {
    Vec::new()
}

struct DaemonHarness {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server_task: Option<tokio::task::JoinHandle<()>>,
    daemon_state: Option<DaemonState>,
    _paths: TestPathsGuard,
}

struct TestPathsGuard {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    config_dir: PathBuf,
}

impl TestPathsGuard {
    async fn new(sandbox: &Sandbox) -> Result<Self> {
        let lock = PATH_OVERRIDE_LOCK.lock().await;
        let config_dir = sandbox.config_home().join("hypercolor");
        ConfigManager::set_config_dir_override(Some(config_dir.clone()));
        ConfigManager::set_data_dir_override(Some(sandbox.data_home().join("hypercolor")));
        ConfigManager::set_state_dir_override(Some(sandbox.state_home().join("hypercolor")));
        assert_confined(sandbox)?;
        Ok(Self {
            _lock: lock,
            config_dir,
        })
    }

    fn config_path(&self) -> PathBuf {
        self.config_dir.join("hypercolor-e2e.toml")
    }
}

impl Drop for TestPathsGuard {
    fn drop(&mut self) {
        ConfigManager::set_state_dir_override(None);
        ConfigManager::set_config_dir_override(None);
        ConfigManager::set_data_dir_override(None);
    }
}

impl DaemonHarness {
    async fn start(sandbox: &Sandbox) -> Result<Self> {
        let paths = TestPathsGuard::new(sandbox).await?;
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
    let Some(sandbox) = sandboxed_run(LIFECYCLE_TEST).await? else {
        return Ok(());
    };
    let harness = Box::pin(DaemonHarness::start(&sandbox)).await?;
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

/// Every file and directory under `root`, with file contents.
fn snapshot(root: &Path) -> Result<BTreeMap<PathBuf, Option<Vec<u8>>>> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let path = entry?.path();
            let relative = path.strip_prefix(root)?.to_path_buf();
            if path.is_dir() {
                pending.push(path);
                entries.insert(relative, None);
            } else {
                let contents =
                    std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
                entries.insert(relative, Some(contents));
            }
        }
    }
    Ok(entries)
}

/// Accepts and counts connections, standing in for a daemon the invoking
/// user already runs.
async fn decoy_daemon() -> Result<(u16, Arc<AtomicUsize>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("failed to bind the decoy daemon")?;
    let port = listener.local_addr()?.port();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while let Ok((_stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    Ok((port, connections))
}

/// Runs the lifecycle test the way a developer does, from a shell whose HOME,
/// XDG roots, CLI profile, and `HYPERCOLOR_*` variables all belong to a
/// workstation that already runs its own daemon, and proves the run leaves
/// that workstation exactly as it found it.
#[tokio::test]
async fn the_lifecycle_test_leaves_the_invoking_users_daemon_alone() -> Result<()> {
    if std::env::var_os(SANDBOX_ENV).is_some() {
        return Ok(());
    }
    let workstation_root = tempfile::tempdir().context("failed to create the user root")?;
    let user = Sandbox {
        root: workstation_root.path().to_path_buf(),
    };
    user.create_dirs()?;
    let (decoy_port, decoy_connections) = decoy_daemon().await?;

    let live_state = user.state_home().join("hypercolor");
    std::fs::create_dir_all(live_state.join("logs"))?;
    std::fs::write(
        live_state.join("runtime-state.json"),
        br#"{"active_scene_id":"live-scene"}"#,
    )?;
    std::fs::write(
        live_state.join("logs").join("api-audit.jsonl"),
        b"{\"path\":\"/api/v1/live-request\"}\n",
    )?;
    std::fs::create_dir_all(user.cli_config().parent().context("cli config parent")?)?;
    std::fs::write(
        user.cli_config(),
        format!("[profiles.local]\nhost = \"127.0.0.1\"\nport = {decoy_port}\n"),
    )?;
    let before = snapshot(&user.root)?;

    let mut command =
        tokio::process::Command::new(std::env::current_exe().context("test executable")?);
    command
        .kill_on_drop(true)
        .args(["--exact", LIFECYCLE_TEST, "--nocapture"]);
    strip_ambient_environment(&mut command);
    command
        .env("HOME", user.home())
        .env("XDG_CONFIG_HOME", user.config_home())
        .env("XDG_DATA_HOME", user.data_home())
        .env("XDG_STATE_HOME", user.state_home())
        .env("XDG_CACHE_HOME", user.cache_home())
        .env("XDG_RUNTIME_DIR", user.runtime_dir())
        .env("HYPERCOLOR_HOST", "127.0.0.1")
        .env("HYPERCOLOR_PORT", decoy_port.to_string());
    let output = tokio::time::timeout(SANDBOXED_RUN_TIMEOUT, command.output())
        .await
        .context("lifecycle test timed out")?
        .context("failed to start the lifecycle test")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !stdout.contains("1 passed") {
        bail!(
            "lifecycle test failed (status={}):\nstdout:\n{stdout}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let after = snapshot(&user.root)?;
    let touched: Vec<_> = after
        .iter()
        .filter(|(path, contents)| before.get(*path) != Some(*contents))
        .map(|(path, _)| path.display().to_string())
        .chain(
            before
                .keys()
                .filter(|path| !after.contains_key(*path))
                .map(|path| format!("{} (removed)", path.display())),
        )
        .collect();
    if !touched.is_empty() {
        let audit = std::fs::read_to_string(live_state.join("logs").join("api-audit.jsonl"))
            .unwrap_or_default();
        bail!(
            "the lifecycle test wrote the invoking user's directories: {touched:?}\n\
             their audit trail now reads:\n{audit}"
        );
    }
    let connections = decoy_connections.load(Ordering::SeqCst);
    if connections != 0 {
        bail!("the lifecycle test reached the user's daemon {connections} time(s)");
    }
    Ok(())
}
