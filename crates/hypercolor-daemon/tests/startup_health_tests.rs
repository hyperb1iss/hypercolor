//! The API listener answers `/health` while the daemon starts, then hands
//! the same listener to the full router once startup completes.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use hypercolor_core::config::ConfigManager;
use hypercolor_daemon::api::startup::ApiHandoff;
use hypercolor_daemon::daemon::{
    DaemonExtensionInstaller, DaemonRunOptions, bind_api_listener, run_with_extensions,
    serve_api_handoff_with_shutdown_timeout,
};
use hypercolor_daemon::extensions::DaemonLifecycleExtension;
use hypercolor_daemon::startup::{DaemonState, StartupProgress};
use hypercolor_types::api::system::{DaemonStartupPhase, HEALTH_STATUS_STARTING, HealthResponse};
use hypercolor_types::config::RenderAccelerationMode;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};

const STARTUP_WAIT: Duration = Duration::from_mins(2);

struct ServedHandoff {
    addr: SocketAddr,
    shutdown_tx: watch::Sender<bool>,
    server: tokio::task::JoinHandle<Result<()>>,
}

impl ServedHandoff {
    fn spawn(handoff: &ApiHandoff) -> Self {
        let listener = bind_api_listener(
            "127.0.0.1:0"
                .parse()
                .expect("ephemeral loopback address should parse"),
        )
        .expect("listener should bind");
        let addr = listener.local_addr().expect("listener address resolves");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = tokio::spawn(serve_api_handoff_with_shutdown_timeout(
            vec![listener],
            handoff.clone(),
            shutdown_rx,
            Duration::from_secs(1),
        ));
        Self {
            addr,
            shutdown_tx,
            server,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn stop(self) {
        self.shutdown_tx.send(true).expect("shutdown signal sends");
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("server stops promptly")
            .expect("server task joins")
            .expect("server shuts down cleanly");
    }
}

fn ready_router() -> Router {
    Router::new().route("/health", get(|| async { (StatusCode::OK, "ready") }))
}

async fn starting_health(client: &reqwest::Client, url: &str) -> HealthResponse {
    let response = client.get(url).send().await.expect("health answers");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONNECTION)
            .and_then(|value| value.to_str().ok()),
        Some("close"),
        "startup answers close the connection so the next one reaches the full router"
    );
    response.json().await.expect("startup health body decodes")
}

#[tokio::test]
async fn health_reports_starting_phases_then_ready_on_the_same_listener() {
    let progress = StartupProgress::default();
    let handoff = ApiHandoff::starting(progress.clone(), "9.9.9-test");
    let served = ServedHandoff::spawn(&handoff);
    let client = reqwest::Client::new();

    let first = starting_health(&client, &served.url("/health")).await;
    assert_eq!(first.status, HEALTH_STATUS_STARTING);
    assert_eq!(first.version, "9.9.9-test");
    assert_eq!(first.checks.render_loop, "starting");
    let first_progress = first.startup.expect("starting body carries progress");
    assert_eq!(first_progress.phase, DaemonStartupPhase::Initializing);

    progress.enter(DaemonStartupPhase::StartingRenderThread);
    let second = starting_health(&client, &served.url("/health"))
        .await
        .startup
        .expect("starting body carries progress");
    assert_eq!(second.phase, DaemonStartupPhase::StartingRenderThread);
    assert!(second.sequence > first_progress.sequence);

    let other = client
        .get(served.url("/api/v1/effects"))
        .send()
        .await
        .expect("every route answers while starting");
    assert_eq!(other.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = other.json().await.expect("error envelope decodes");
    assert_eq!(body["error"]["code"], "service_unavailable");
    assert_eq!(body["error"]["details"]["phase"], "starting_render_thread");
    assert_eq!(body["error"]["details"]["sequence"], second.sequence);

    assert!(!handoff.is_ready());
    assert!(handoff.install(ready_router()));
    assert!(handoff.is_ready());
    assert!(
        !handoff.install(Router::new()),
        "the first installed router stays in place"
    );

    let ready = client
        .get(served.url("/health"))
        .send()
        .await
        .expect("ready health answers");
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(ready.text().await.expect("ready body reads"), "ready");

    served.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_step_is_named_and_advances_the_sequence_only_when_done() {
    const STEP: &str = "SparkleFlinger GPU area horizontal tile scan";
    let progress = StartupProgress::default();
    let handoff = ApiHandoff::starting(progress.clone(), "9.9.9-test");
    let served = ServedHandoff::spawn(&handoff);
    let client = reqwest::Client::new();
    progress.enter(DaemonStartupPhase::StartingRenderThread);
    let entered = progress.snapshot().sequence;

    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let stepper = {
        let progress = progress.clone();
        std::thread::spawn(move || {
            progress.step(STEP, || {
                started_tx.send(()).expect("step start signals");
                release_rx.recv().expect("step release arrives");
            });
        })
    };
    tokio::task::spawn_blocking(move || started_rx.recv())
        .await
        .expect("start wait joins")
        .expect("step starts");

    // In flight: named, but not progress yet.
    let running = starting_health(&client, &served.url("/health"))
        .await
        .startup
        .expect("starting body carries progress");
    assert_eq!(running.phase, DaemonStartupPhase::StartingRenderThread);
    assert_eq!(running.sequence, entered);
    assert_eq!(running.detail.as_deref(), Some(STEP));
    let other: Value = client
        .get(served.url("/api/v1/effects"))
        .send()
        .await
        .expect("every route answers while starting")
        .json()
        .await
        .expect("error envelope decodes");
    assert_eq!(other["error"]["details"]["detail"], STEP);

    release_tx.send(()).expect("step releases");
    stepper.join().expect("step thread joins");

    let done = starting_health(&client, &served.url("/health"))
        .await
        .startup
        .expect("starting body carries progress");
    assert_eq!(done.sequence, entered + 1);
    assert_eq!(done.detail, None);

    served.stop().await;
}

#[tokio::test]
async fn a_connection_accepted_while_starting_is_served_by_the_full_router() {
    let handoff = ApiHandoff::starting(StartupProgress::default(), "9.9.9-test");
    let served = ServedHandoff::spawn(&handoff);

    // Accepted before the handoff, first request sent after it.
    let mut stream = tokio::net::TcpStream::connect(served.addr)
        .await
        .expect("raw connection opens");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(handoff.install(ready_router()));

    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("request writes");
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_string(&mut response))
        .await
        .expect("response arrives")
        .expect("response reads");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected response: {response}"
    );
    assert!(response.ends_with("ready"), "unexpected body: {response}");

    served.stop().await;
}

#[tokio::test]
async fn remote_peers_get_a_bare_starting_answer() {
    let handoff = ApiHandoff::starting(StartupProgress::default(), "9.9.9-test");
    let served = ServedHandoff::spawn(&handoff);
    let client = reqwest::Client::new();

    // A loopback proxy forwarding a LAN client: the security layer treats
    // the forwarded address as the peer, and so does the startup surface.
    let response = client
        .get(served.url("/health"))
        .header("x-forwarded-for", "192.168.1.20")
        .send()
        .await
        .expect("health answers");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.expect("error envelope decodes");
    assert_eq!(body["error"]["code"], "service_unavailable");
    assert!(body["error"].get("details").is_none());
    assert!(body.get("version").is_none());

    served.stop().await;
}

// ── Full daemon ─────────────────────────────────────────────────────────

/// Holds daemon startup inside `StartingServices` until released.
struct StartupGate {
    entered: Notify,
    release: Notify,
}

struct GateExtension(Arc<StartupGate>);

#[async_trait]
impl DaemonLifecycleExtension for GateExtension {
    fn name(&self) -> &'static str {
        "startup-gate"
    }

    async fn start(&self, _daemon: &DaemonState) -> Result<()> {
        self.0.entered.notify_one();
        self.0.release.notified().await;
        Ok(())
    }
}

struct GateInstaller(Arc<StartupGate>);

impl DaemonExtensionInstaller for GateInstaller {
    fn install(&self, daemon: &mut DaemonState) -> Result<()> {
        daemon.register_lifecycle_extension(Arc::new(GateExtension(Arc::clone(&self.0))));
        Ok(())
    }
}

struct IsolatedDirs {
    _root: tempfile::TempDir,
    config_file: PathBuf,
}

impl IsolatedDirs {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary root builds");
        ConfigManager::set_config_dir_override(Some(root.path().join("config")));
        ConfigManager::set_data_dir_override(Some(root.path().join("data")));
        ConfigManager::set_state_dir_override(Some(root.path().join("state")));
        let config_file = root.path().join("hypercolor.toml");
        std::fs::write(
            &config_file,
            "schema_version = 5\n\n\
             [network]\nmdns_publish = false\n\n\
             [discovery]\nbackground_enabled = false\n\n\
             [effect_engine]\nwatch_effects = false\n",
        )
        .expect("config writes");
        Self {
            _root: root,
            config_file,
        }
    }
}

impl Drop for IsolatedDirs {
    fn drop(&mut self) {
        ConfigManager::set_config_dir_override(None);
        ConfigManager::set_data_dir_override(None);
        ConfigManager::set_state_dir_override(None);
    }
}

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("ephemeral port resolves")
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn running_daemon_answers_starting_until_its_router_is_installed() {
    let dirs = IsolatedDirs::new();
    let port = free_loopback_port();
    let base = format!("http://127.0.0.1:{port}");
    let gate = Arc::new(StartupGate {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let installers: &'static [&'static dyn DaemonExtensionInstaller] = Box::leak(Box::new([
        Box::leak(Box::new(GateInstaller(Arc::clone(&gate))))
            as &'static dyn DaemonExtensionInstaller,
    ]));
    let options = DaemonRunOptions {
        config: Some(dirs.config_file.clone()),
        bind: Some(format!("127.0.0.1:{port}")),
        compositor_acceleration_mode: Some(RenderAccelerationMode::Cpu),
        ..DaemonRunOptions::default()
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Drive the daemon on this task, as the process host's `block_on`
    // does, while the probe side runs beside it.
    let daemon = Box::pin(run_with_extensions(options, shutdown_rx, installers));
    let probe = async {
        tokio::time::timeout(STARTUP_WAIT, gate.entered.notified())
            .await
            .expect("startup reaches the gated extension");

        let client = reqwest::Client::new();
        let health = starting_health(&client, &format!("{base}/health")).await;
        assert_eq!(health.status, HEALTH_STATUS_STARTING);
        assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
        let progress = health.startup.expect("starting body carries progress");
        assert_eq!(progress.phase, DaemonStartupPhase::StartingServices);
        // Seven phase entries, then the render thread's own steps: the
        // runtime, input publication, and the compositor canvases and
        // sampling plan (the CPU compositor compiles no pipelines).
        assert!(
            progress.sequence >= 12,
            "every earlier phase and render-thread step advanced the sequence, got {}",
            progress.sequence
        );
        assert_eq!(progress.detail, None);

        let status = client
            .get(format!("{base}/api/v1/system"))
            .send()
            .await
            .expect("system route answers while starting");
        assert_eq!(status.status(), StatusCode::SERVICE_UNAVAILABLE);

        gate.release.notify_one();

        let deadline = tokio::time::Instant::now() + STARTUP_WAIT;
        let ready = loop {
            let response = client
                .get(format!("{base}/health"))
                .send()
                .await
                .expect("health keeps answering through the handoff");
            if response.status() == StatusCode::OK {
                break response;
            }
            let body: HealthResponse = response.json().await.expect("starting body decodes");
            assert_eq!(body.status, HEALTH_STATUS_STARTING);
            assert!(
                tokio::time::Instant::now() < deadline,
                "daemon never became ready"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let ready: HealthResponse = ready.json().await.expect("ready body decodes");
        assert_eq!(ready.status, "healthy");
        assert!(ready.startup.is_none());

        shutdown_tx.send(true).expect("shutdown signal sends");
    };

    let (result, ()) =
        tokio::time::timeout(STARTUP_WAIT * 2, async { tokio::join!(daemon, probe) })
            .await
            .expect("daemon starts, serves, and stops");
    result.expect("daemon exits cleanly");
    drop(dirs);
}
