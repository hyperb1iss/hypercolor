use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use hypercolor_core::config::ConfigManager;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use hypercolor_core::input::screen::ScreenAdmissionCapacity;
use hypercolor_core::input::screen::{PixelExtent, ScreenCaptureDemand};
use hypercolor_core::input::{
    InputData, InputManager, InputSource, ScreenReconfigurationConflict, SourceIssue, SourceKind,
    SourceState, SourceStatus, SourceStatusHandle, SourceStatusReporter,
};
use hypercolor_types::config::InteractionRoutePolicy;

use super::live::{
    CaptureConfigTransactionError, LiveSections, apply_capture_config_transaction,
    apply_input_config_change, canvas_dimensions_differ, capture_statuses_match, live_sections_for,
    validate_prepared_capture_status, write_covers,
};
use super::{ConfigApplyQuery, put_config_key};
use crate::app_state::AppState;

struct TestScreenSource {
    running: bool,
    demand: ScreenCaptureDemand,
    stopped: Arc<AtomicBool>,
}

impl TestScreenSource {
    fn new(stopped: Arc<AtomicBool>) -> Self {
        Self {
            running: false,
            demand: ScreenCaptureDemand::Inactive,
            stopped,
        }
    }
}

impl InputSource for TestScreenSource {
    fn name(&self) -> &'static str {
        "test_screen"
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
        self.stopped.store(true, Ordering::Release);
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn is_screen_source(&self) -> bool {
        true
    }

    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.demand
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        self.demand = demand;
        Ok(())
    }
}

fn test_screen_demand() -> ScreenCaptureDemand {
    ScreenCaptureDemand::active(
        PixelExtent::new(640, 480).expect("test screen extent should be non-empty"),
    )
}

fn screen_status(state: SourceState, resource_count: usize) -> Arc<SourceStatus> {
    screen_status_handle(state, resource_count).snapshot()
}

fn screen_status_handle(state: SourceState, resource_count: usize) -> SourceStatusHandle {
    let mut reporter =
        SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
    reporter.set_source_graph_generation(1);
    if state == SourceState::Stopped {
        return reporter.handle();
    }
    let session = reporter
        .begin_session()
        .expect("test source session begins")
        .expect("manager-bound source creates a session");
    match state {
        SourceState::Starting => {}
        SourceState::Live => {
            session.mark_event_driven_live_without_deadline(resource_count);
        }
        SourceState::Degraded => {
            session.degraded_with_resources(
                SourceIssue::new("test_degraded", "reduced capture", true),
                resource_count,
            );
        }
        SourceState::Unavailable => {
            session.unavailable(SourceIssue::new(
                "test_unavailable",
                "capture unavailable",
                true,
            ));
        }
        SourceState::Failed => {
            session.failed(SourceIssue::new("test_failed", "capture failed", false));
        }
        SourceState::Stopped => unreachable!("stopped status returned before session start"),
    }
    reporter.handle()
}

fn starting_screen_status() -> SourceStatusHandle {
    let mut reporter =
        SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
    reporter.set_source_graph_generation(1);
    reporter
        .begin_session()
        .expect("test source session begins")
        .expect("manager-bound source creates a session");
    reporter.handle()
}

#[test]
fn registry_dispatch_routes_one_section_per_live_key() {
    assert_eq!(
        live_sections_for(Some("audio.device")),
        LiveSections {
            audio: true,
            ..LiveSections::default()
        }
    );
    assert_eq!(
        live_sections_for(Some("capture.enabled")),
        LiveSections {
            capture: true,
            ..LiveSections::default()
        }
    );
    assert_eq!(
        live_sections_for(Some("input.enabled")),
        LiveSections {
            input: true,
            ..LiveSections::default()
        }
    );
    assert_eq!(
        live_sections_for(Some("daemon.target_fps")),
        LiveSections {
            render: true,
            ..LiveSections::default()
        }
    );
}

#[test]
fn registry_dispatch_applies_nothing_for_non_live_policies() {
    // Restart, NextScan, LiveOnRead, and Inert keys all persist
    // without a live subsystem to re-apply.
    for key in [
        "daemon.port",
        "discovery.scan_interval_secs",
        "session.sleep_behavior",
        "tui.theme",
        "drivers.wled.known_ips",
    ] {
        assert!(
            live_sections_for(Some(key)).is_empty(),
            "{key} should not dispatch a live section"
        );
    }
}

#[test]
fn writing_a_section_carries_the_live_keys_nested_under_it() {
    // The exact render overrides live under a Restart-classified
    // section, so a whole-section write still retunes the loop.
    let daemon = live_sections_for(Some("daemon"));
    assert!(daemon.render);
    assert!(!daemon.audio);
    assert!(write_covers(Some("daemon"), "daemon.target_fps"));
    assert!(!write_covers(Some("daemon.port"), "daemon.target_fps"));
    assert!(write_covers(None, "daemon.canvas_width"));
}

#[test]
fn a_whole_config_write_touches_every_live_section() {
    let sections = live_sections_for(None);
    assert!(sections.audio);
    assert!(sections.capture);
    assert!(sections.input);
    // The regression this fixes: the old hand predicate matched
    // three exact keys and ignored the whole-config case, so a full
    // reset persisted a new target FPS without ever retuning.
    assert!(sections.render);
}

#[test]
fn read_surfaces_mask_secret_namespaces_and_leave_plain_keys_alone() {
    let document = serde_json::json!({
        "audio": { "device": "default" },
        "drivers": {
            "wled": { "enabled": true, "known_ips": ["192.168.1.50"] },
        },
        "cloud": { "api_key": "secret" },
    });

    let redacted = super::redact_document(document);

    assert_eq!(redacted["audio"]["device"], serde_json::json!("default"));
    assert_eq!(
        redacted["drivers"]["wled"],
        serde_json::json!({ "redacted": true })
    );
    assert_eq!(redacted["cloud"], serde_json::json!({ "redacted": true }));
}

#[test]
fn a_masked_document_still_parses_as_a_config() {
    // Clients type this response as the config struct, so the mask
    // has to keep the document readable rather than break the read
    // surface it protects.
    let mut config = hypercolor_types::config::HypercolorConfig::default();
    config.drivers.insert(
        "wled".to_owned(),
        hypercolor_types::config::DriverConfigEntry::enabled(
            [("known_ips".to_owned(), serde_json::json!(["192.168.1.50"]))]
                .into_iter()
                .collect(),
        ),
    );
    config
        .extensions
        .insert("cloud".to_owned(), serde_json::json!({ "token": "secret" }));

    let document =
        super::redact_document(serde_json::to_value(&config).expect("config projects to JSON"));
    let parsed: hypercolor_types::config::HypercolorConfig =
        serde_json::from_value(document).expect("a masked config still deserializes");

    assert!(parsed.drivers.contains_key("wled"));
    assert_eq!(
        parsed.drivers["wled"].settings.get("known_ips"),
        None,
        "the masked entry keeps its name and drops its settings"
    );
    assert_eq!(parsed.daemon.port, config.daemon.port);
}

#[test]
fn key_reads_mask_at_every_depth_of_a_secret_namespace() {
    assert_eq!(
        super::redact_key("drivers.wled.known_ips", serde_json::json!(["10.0.0.1"])),
        serde_json::json!({ "redacted": true })
    );
    assert_eq!(
        super::redact_key("drivers.wled", serde_json::json!({ "enabled": true })),
        serde_json::json!({ "redacted": true })
    );
    assert_eq!(
        super::redact_key(
            "drivers",
            serde_json::json!({ "wled": { "enabled": true }, "hue": {} })
        ),
        serde_json::json!({ "wled": { "redacted": true }, "hue": { "redacted": true } })
    );
    assert_eq!(
        super::redact_key("daemon.port", serde_json::json!(9420)),
        serde_json::json!(9420)
    );
}

#[test]
fn canvas_dimensions_differ_only_when_size_changes() {
    assert!(!canvas_dimensions_differ(800, 600, 800, 600));
    assert!(canvas_dimensions_differ(800, 600, 801, 600));
    assert!(canvas_dimensions_differ(800, 600, 800, 601));
}

#[tokio::test]
async fn route_only_input_config_changes_publish_without_rebuilding_sources() {
    let mut state = AppState::new();
    let config_path = std::env::temp_dir().join(format!(
        "hypercolor-route-config-{}-{}.toml",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow Unix epoch")
            .as_nanos()
    ));
    let manager =
        Arc::new(ConfigManager::new(config_path).expect("test config manager should initialize"));
    state.config_manager = Some(Arc::clone(&manager));
    let state = Arc::new(state);
    let graph_generation = state.input_manager.lock().await.source_graph_generation();

    manager.modify(|config| config.input.daemon_route = InteractionRoutePolicy::Merge);
    assert!(apply_input_config_change(&state, Some("input.daemon_route")).await);
    let first = state.interaction_routing.snapshot();
    assert_eq!(first.daemon_policy, InteractionRoutePolicy::Merge);
    assert_eq!(first.config_generation, 2);

    manager.modify(|config| config.input.preview_route = InteractionRoutePolicy::Host);
    assert!(apply_input_config_change(&state, Some("input.preview_route")).await);
    let second = state.interaction_routing.snapshot();
    assert_eq!(second.preview_policy, InteractionRoutePolicy::Host);
    assert_eq!(second.config_generation, 3);
    assert_eq!(
        state.input_manager.lock().await.source_graph_generation(),
        graph_generation
    );
}

#[tokio::test]
async fn demanded_starting_capture_times_out_instead_of_committing() {
    let error = validate_prepared_capture_status(starting_screen_status())
        .await
        .expect_err("starting capture must become usable before commit");

    assert!(error.to_string().contains("did not become usable within"));
}

#[tokio::test]
async fn demanded_degraded_capture_commits_only_with_usable_resources() {
    validate_prepared_capture_status(screen_status_handle(SourceState::Degraded, 1))
        .await
        .expect("degraded capture with resources is usable");

    let error = validate_prepared_capture_status(screen_status_handle(SourceState::Degraded, 0))
        .await
        .expect_err("degraded capture without resources is unusable");
    assert!(error.to_string().contains("reduced capture"));
}

#[test]
fn capture_runtime_health_rejects_missing_stopped_failed_and_extra_sources() {
    let mut capture = hypercolor_types::config::CaptureConfig {
        enabled: true,
        ..hypercolor_types::config::CaptureConfig::default()
    };
    assert!(!capture_statuses_match(&capture, &[]));
    assert!(!capture_statuses_match(
        &capture,
        &[screen_status(SourceState::Stopped, 0)]
    ));
    assert!(!capture_statuses_match(
        &capture,
        &[screen_status(SourceState::Failed, 0)]
    ));
    assert!(capture_statuses_match(
        &capture,
        &[screen_status(SourceState::Degraded, 1)]
    ));

    capture.enabled = false;
    assert!(!capture_statuses_match(
        &capture,
        &[
            screen_status(SourceState::Stopped, 0),
            screen_status(SourceState::Stopped, 0),
        ]
    ));
}

#[test]
fn capture_runtime_fingerprint_rejects_divergent_config() {
    let tempdir = tempfile::tempdir().expect("temporary config directory should build");
    let manager = ConfigManager::new(tempdir.path().join("hypercolor.toml"))
        .expect("test config manager should initialize");
    let applied = manager.get().capture.clone();
    manager.mark_capture_runtime_applied(&applied);
    let mut divergent = applied.clone();
    divergent.capture_fps += 1;

    assert!(manager.capture_runtime_matches(&applied));
    assert!(!manager.capture_runtime_matches(&divergent));
}

#[test]
fn screen_runtime_commit_preserves_demand_and_retires_after_swap() {
    let mut manager = InputManager::new();
    manager
        .set_screen_capture_demand(test_screen_demand())
        .expect("screen demand should cache before a source exists");
    let first_plan = manager.plan_screen_runtime_config(true);
    assert_eq!(first_plan.capture_demand(), test_screen_demand());

    let first_stopped = Arc::new(AtomicBool::new(false));
    let mut first = Box::new(TestScreenSource::new(Arc::clone(&first_stopped)));
    first
        .set_screen_capture_demand(first_plan.capture_demand())
        .expect("prepared source should accept demand");
    first.start().expect("prepared source should start");
    let mut first = Some(first as Box<dyn InputSource>);
    manager
        .commit_screen_runtime_config(&first_plan, &mut first)
        .expect("initial prepared source should commit")
        .retire();
    assert!(first.is_none());

    let replacement_plan = manager.plan_screen_runtime_config(true);
    assert_eq!(replacement_plan.capture_demand(), test_screen_demand());
    let replacement_stopped = Arc::new(AtomicBool::new(false));
    let mut replacement = Box::new(TestScreenSource::new(replacement_stopped));
    replacement
        .set_screen_capture_demand(replacement_plan.capture_demand())
        .expect("replacement should accept demand");
    replacement.start().expect("replacement should start");
    let mut replacement = Some(replacement as Box<dyn InputSource>);
    let retirement = manager
        .commit_screen_runtime_config(&replacement_plan, &mut replacement)
        .expect("replacement should commit");

    assert!(!first_stopped.load(Ordering::Acquire));
    retirement.retire();
    assert!(first_stopped.load(Ordering::Acquire));
}

#[test]
fn screen_runtime_commit_rejects_stale_graph_without_consuming_replacement() {
    let mut manager = InputManager::new();
    let plan = manager.plan_screen_runtime_config(true);
    let stopped = Arc::new(AtomicBool::new(false));
    let mut source = Box::new(TestScreenSource::new(Arc::clone(&stopped)));
    source.start().expect("prepared source should start");
    let mut replacement = Some(source as Box<dyn InputSource>);
    manager.add_source(Box::new(hypercolor_core::input::MediaSource::new()));

    assert!(matches!(
        manager.commit_screen_runtime_config(&plan, &mut replacement),
        Err(ScreenReconfigurationConflict::GraphChanged)
    ));
    assert!(replacement.is_some());
    assert!(!stopped.load(Ordering::Acquire));
    replacement
        .as_mut()
        .expect("failed commit preserves replacement ownership")
        .stop();
    assert!(stopped.load(Ordering::Acquire));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[tokio::test]
async fn capture_transaction_applies_publication_capacity_with_config() {
    let tempdir = tempfile::tempdir().expect("temporary config directory should build");
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("test config manager should initialize"),
    );
    let expected = Arc::clone(&manager.get());
    let mut capture = expected.capture.clone();
    capture.enabled = false;
    capture.publication_memory_bytes = Some(30_000);
    let mut state = AppState::new();
    state.config_manager = Some(Arc::clone(&manager));
    let state = Arc::new(state);
    state
        .input_manager
        .lock()
        .await
        .set_screen_capacity_plan(
            ScreenAdmissionCapacity::new(40_000, 40_000),
            ScreenAdmissionCapacity::new(30_000, 40_000),
            ScreenAdmissionCapacity::new(20_000, 40_000),
        )
        .expect("empty manager should accept test capacity");

    apply_capture_config_transaction(&state, &expected, capture.clone())
        .await
        .expect("valid publication capacity should apply");

    assert_eq!(manager.get().capture, capture);
    assert!(manager.capture_runtime_matches(&capture));
    let capacity = state
        .input_manager
        .lock()
        .await
        .screen_publication_capacity();
    assert_eq!(capacity.byte_budget(), 30_000);
    assert_eq!(capacity.backend_capacity(), 40_000);
    assert_eq!(
        state.input_manager.lock().await.screen_resource_capacity(),
        ScreenAdmissionCapacity::new(40_000, 40_000)
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[tokio::test]
async fn capture_transaction_conflict_preserves_publication_capacity() {
    let tempdir = tempfile::tempdir().expect("temporary config directory should build");
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("test config manager should initialize"),
    );
    let expected = Arc::clone(&manager.get());
    let mut capture = expected.capture.clone();
    capture.enabled = false;
    capture.publication_memory_bytes = Some(30_000);
    manager.modify(|config| config.capture.capture_fps += 1);
    let mut state = AppState::new();
    state.config_manager = Some(Arc::clone(&manager));
    let state = Arc::new(state);
    state
        .input_manager
        .lock()
        .await
        .set_screen_capacity_plan(
            ScreenAdmissionCapacity::new(40_000, 40_000),
            ScreenAdmissionCapacity::new(20_000, 40_000),
            ScreenAdmissionCapacity::new(20_000, 40_000),
        )
        .expect("empty manager should accept test capacity");

    let result = apply_capture_config_transaction(&state, &expected, capture).await;

    assert!(matches!(
        result,
        Err(CaptureConfigTransactionError::Conflict)
    ));
    let capacity = state
        .input_manager
        .lock()
        .await
        .screen_publication_capacity();
    assert_eq!(capacity, ScreenAdmissionCapacity::new(20_000, 40_000));
    assert_eq!(
        manager.get().capture.capture_fps,
        expected.capture.capture_fps + 1
    );
    assert_eq!(manager.get().capture.publication_memory_bytes, None);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn failed_windows_capture_preparation_preserves_old_graph_and_config() {
    let config_path = std::env::temp_dir().join(format!(
        "hypercolor-capture-config-{}-{}.toml",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should follow Unix epoch")
            .as_nanos()
    ));
    let manager = Arc::new(
        ConfigManager::new(config_path.clone()).expect("test config manager should initialize"),
    );
    let mut state = AppState::new();
    state.config_manager = Some(Arc::clone(&manager));
    let state = Arc::new(state);
    {
        let mut input_manager = state.input_manager.lock().await;
        let mut old = Box::new(TestScreenSource::new(Arc::new(AtomicBool::new(false))));
        old.start().expect("old test source should start");
        input_manager.add_source(old);
        input_manager
            .set_screen_capture_demand(test_screen_demand())
            .expect("old source should accept active demand");
    }
    let graph_generation = state.input_manager.lock().await.source_graph_generation();
    let admission_coordinator = state
        .input_manager
        .lock()
        .await
        .screen_admission_coordinator();
    let reserved_before = admission_coordinator.snapshot().reserved_bytes();
    let expected = Arc::clone(&manager.get());
    let mut capture = expected.capture.clone();
    capture.source = "monitor:hypercolor-test-source-that-does-not-exist".to_owned();

    let result = apply_capture_config_transaction(&state, &expected, capture).await;

    assert!(matches!(
        result,
        Err(CaptureConfigTransactionError::Prepare(_))
    ));
    assert_eq!(manager.get().capture.source, "auto");
    let input_manager = state.input_manager.lock().await;
    assert_eq!(input_manager.source_graph_generation(), graph_generation);
    assert!(input_manager.has_screen_source());
    assert!(
        input_manager
            .source_names()
            .iter()
            .any(|name| name == "test_screen")
    );
    assert_eq!(
        admission_coordinator.snapshot().reserved_bytes(),
        reserved_before
    );
    assert!(!config_path.exists());
}

#[tokio::test]
async fn unchanged_disabled_capture_repairs_stale_runtime_source() {
    let tempdir = tempfile::tempdir().expect("temporary config directory should build");
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("test config manager should initialize"),
    );
    manager.modify(|config| config.capture.enabled = false);
    let mut state = AppState::new();
    state.config_manager = Some(Arc::clone(&manager));
    let state = Arc::new(state);
    let stopped = Arc::new(AtomicBool::new(false));
    {
        let mut input_manager = state.input_manager.lock().await;
        let mut source = Box::new(TestScreenSource::new(Arc::clone(&stopped)));
        source.start().expect("stale source should start");
        input_manager.add_source(source);
        let mut extra = Box::new(TestScreenSource::new(Arc::new(AtomicBool::new(false))));
        extra.start().expect("extra stale source should start");
        input_manager.add_source(extra);
    }

    let response = put_config_key(
        axum::extract::State(Arc::clone(&state)),
        axum::extract::Path("capture.enabled".to_owned()),
        axum::extract::Query(ConfigApplyQuery { live: true }),
        axum::Extension(crate::api::security::RequestAuthContext::control()),
        axum::Json(serde_json::json!(false)),
    )
    .await;

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("config response body should be readable");
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(!state.input_manager.lock().await.has_screen_source());
    assert!(stopped.load(Ordering::Acquire));
}

#[tokio::test(flavor = "current_thread")]
async fn unchanged_capture_rejects_a_concurrent_config_generation() {
    let tempdir = tempfile::tempdir().expect("temporary config directory should build");
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("test config manager should initialize"),
    );
    manager.modify(|config| config.capture.enabled = false);
    let initial = Arc::clone(&manager.get());
    manager.mark_capture_runtime_applied(&initial.capture);
    let mut state = AppState::new();
    state.config_manager = Some(Arc::clone(&manager));
    let state = Arc::new(state);
    let input_manager = state.input_manager.lock().await;
    let request_state = Arc::clone(&state);
    let unchanged_fps = initial.capture.capture_fps;
    let request = tokio::spawn(async move {
        put_config_key(
            axum::extract::State(request_state),
            axum::extract::Path("capture.capture_fps".to_owned()),
            axum::extract::Query(ConfigApplyQuery { live: true }),
            axum::Extension(crate::api::security::RequestAuthContext::control()),
            axum::Json(serde_json::json!(unchanged_fps)),
        )
        .await
    });

    tokio::task::yield_now().await;
    let mut competing = (*initial).clone();
    competing.capture.capture_fps += 1;
    let competing_capture = competing.capture.clone();
    manager.update(competing);
    manager.mark_capture_runtime_applied(&competing_capture);
    drop(input_manager);

    let response = request.await.expect("unchanged request should complete");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("config response body should be readable");
    assert_eq!(
        status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(manager.get().capture, competing_capture);
    assert!(manager.capture_runtime_matches(&competing_capture));
}
