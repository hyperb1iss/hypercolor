#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use hypercolor_core::config::{BootConfig, ConfigManager};
use hypercolor_core::input::screen::{PixelExtent, ScreenAdmissionCapacity, ScreenCaptureDemand};
#[cfg(target_os = "macos")]
use hypercolor_core::input::{SourceKind, SourceStatusHandle, SourceStatusReporter};

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::CaptureConfigPersistenceGate;
#[cfg(target_os = "linux")]
use super::CaptureConfigPersistenceUpdate;
use super::{
    screen_analysis_plan_for_demand, screen_capacity_plan, screen_capacity_plan_for_backend,
    screen_capture_config_from, screen_capture_config_with_capacity_from,
};

#[test]
fn steady_capacity_uses_configured_budget_and_live_backend_memory() {
    let capture = hypercolor_types::config::CaptureConfig {
        publication_memory_bytes: Some(1),
        ..Default::default()
    };

    let plan = screen_capacity_plan(&capture).expect("capacity resolves");

    assert_eq!(plan.total_capacity().byte_budget(), 1);
    assert!(plan.total_capacity().backend_capacity() > 0);
    assert!(plan.total_capacity().backend_capacity() < u64::MAX);
}

#[test]
fn steady_capacity_defaults_to_live_backend_memory() {
    let plan = screen_capacity_plan_for_backend(
        &hypercolor_types::config::CaptureConfig::default(),
        1_000_000,
    )
    .expect("capacity resolves");
    let total = plan.total_capacity();

    assert_eq!(total.byte_budget(), total.backend_capacity());
    assert_eq!(total.byte_budget(), 1_000_000);
}

#[test]
fn analysis_quote_uses_the_actual_requested_extent() {
    let mut capture = hypercolor_types::config::CaptureConfig::default();
    let extent = PixelExtent::new(3840, 2160).expect("4K extent is non-empty");
    let demand = ScreenCaptureDemand::active(extent);
    let unbounded = ScreenAdmissionCapacity::new(u64::MAX, u64::MAX);
    let baseline = screen_analysis_plan_for_demand(&capture, demand, unbounded)
        .expect("4K analysis is representable")
        .expect("active demand has an analysis plan");
    let peak = baseline.peak_bytes();

    capture.publication_memory_bytes = Some(peak);
    let exact = screen_capacity_plan_for_backend(&capture, peak).expect("exact peak is configured");
    let admitted = screen_analysis_plan_for_demand(&capture, demand, exact.total_capacity())
        .expect("exact 4K peak is admitted")
        .expect("active demand has an analysis plan");
    assert_eq!(admitted.peak_bytes(), peak);

    capture.publication_memory_bytes = Some(peak - 1);
    let undersized = screen_capacity_plan_for_backend(&capture, peak)
        .expect("steady policy is valid while capture is inactive");
    assert!(
        screen_analysis_plan_for_demand(&capture, demand, undersized.total_capacity()).is_err()
    );

    capture.publication_memory_bytes = Some(peak + 777);
    let capacity = screen_capacity_plan_for_backend(&capture, peak + 333)
        .expect("independent steady and physical fences resolve");
    assert_eq!(
        capacity.resource_capacity(),
        ScreenAdmissionCapacity::new(peak + 333, peak + 333)
    );
    assert_eq!(
        capacity.total_capacity(),
        ScreenAdmissionCapacity::new(peak + 777, peak + 333)
    );
}

#[cfg(target_os = "macos")]
fn live_screen_status() -> SourceStatusHandle {
    let mut reporter =
        SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
    reporter.set_source_graph_generation(1);
    let session = reporter
        .begin_session()
        .expect("test source session begins")
        .expect("manager-bound source creates a session");
    session.mark_event_driven_live_without_deadline(1);
    reporter.handle()
}

#[cfg(target_os = "macos")]
fn macos_picker_gate(
    manager: &Arc<ConfigManager>,
) -> (
    CaptureConfigPersistenceGate,
    Arc<hypercolor_types::config::HypercolorConfig>,
) {
    let expected = Arc::clone(&manager.get());
    let persistence = CaptureConfigPersistenceGate::for_macos_picker(
        Arc::clone(manager),
        &expected,
        live_screen_status(),
    )
    .expect("picker persistence authority is reserved");
    (persistence, expected)
}

#[cfg(target_os = "macos")]
#[test]
fn macos_display_picker_selection_persists_stable_uuid() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    let (persistence, expected) = macos_picker_gate(&manager);

    persistence.publish_macos_selection(
        expected.capture.source.clone(),
        "display:7a3f4954-3d72-47a6-a914-16ef68d02122".to_owned(),
    );

    assert_eq!(
        manager.get().capture.source,
        "display:7a3f4954-3d72-47a6-a914-16ef68d02122"
    );
    drop(manager);
    let restarted = ConfigManager::new(path).expect("config manager reopens");
    assert_eq!(
        restarted.get().capture.source,
        "display:7a3f4954-3d72-47a6-a914-16ef68d02122"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_window_picker_selection_persists_only_session_scope() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    let (persistence, expected) = macos_picker_gate(&manager);

    persistence
        .publish_macos_selection(expected.capture.source.clone(), "session_scoped".to_owned());

    assert_eq!(manager.get().capture.source, "session_scoped");
    drop(persistence);
    drop(manager);
    let restarted = ConfigManager::new(path).expect("config manager reopens");
    assert_eq!(restarted.get().capture.source, "session_scoped");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_picker_update_cannot_overwrite_newer_config() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let manager = Arc::new(
        ConfigManager::new(directory.path().join("hypercolor.toml")).expect("config manager opens"),
    );
    let (persistence, expected) = macos_picker_gate(&manager);
    manager.modify(|config| config.capture.source = "primary_display".to_owned());

    persistence.publish_macos_selection(
        expected.capture.source.clone(),
        "display:7a3f4954-3d72-47a6-a914-16ef68d02122".to_owned(),
    );

    assert_eq!(manager.get().capture.source, "primary_display");
}

#[cfg(target_os = "macos")]
#[test]
fn revoked_macos_picker_gate_preserves_current_selection() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let manager = Arc::new(
        ConfigManager::new(directory.path().join("hypercolor.toml")).expect("config manager opens"),
    );
    let (persistence, expected) = macos_picker_gate(&manager);
    persistence.revoke();

    persistence
        .publish_macos_selection(expected.capture.source.clone(), "session_scoped".to_owned());

    assert_eq!(manager.get().capture.source, expected.capture.source);
}

#[test]
fn screen_capture_config_conversion_preserves_validated_values_exactly() {
    let capture = hypercolor_types::config::CaptureConfig {
        capture_fps: hypercolor_core::input::screen::MAX_REPRESENTABLE_CAPTURE_FPS,
        grid_cols: 64,
        grid_rows: 1,
        smoothing: 1.0,
        gamma: 5.0,
        target_led_white_x: 0.2,
        target_led_white_y: 0.3,
        target_led_reference_white_nits: 100.0,
        target_led_peak_nits: 1_000.0,
        exposure_ev: -2.0,
        ..hypercolor_types::config::CaptureConfig::default()
    };

    let runtime = screen_capture_config_from(&capture).expect("boundary config should validate");

    assert_eq!(
        runtime.target_fps,
        hypercolor_core::input::screen::MAX_REPRESENTABLE_CAPTURE_FPS
    );
    assert_eq!(runtime.grid_cols, 64);
    assert_eq!(runtime.grid_rows, 1);
    assert_eq!(runtime.analysis_memory_bytes, u64::MAX);
    assert!((runtime.smoothing_alpha - 1.0).abs() < f32::EPSILON);
    assert!((runtime.tuning.gamma - 5.0).abs() < f32::EPSILON);
    assert!((runtime.target_led_white_x - 0.2).abs() < f32::EPSILON);
    assert!((runtime.target_led_white_y - 0.3).abs() < f32::EPSILON);
    assert!((runtime.target_led_reference_white_nits - 100.0).abs() < f32::EPSILON);
    assert!((runtime.target_led_peak_nits - 1_000.0).abs() < f32::EPSILON);
    assert!((runtime.exposure_ev - -2.0).abs() < f32::EPSILON);
}

#[test]
fn platform_capture_config_installs_the_steady_analysis_budget() {
    let capture = hypercolor_types::config::CaptureConfig::default();
    let capacity = ScreenAdmissionCapacity::new(1_000_000, 2_000_000);

    let runtime = screen_capture_config_with_capacity_from(&capture, capacity)
        .expect("platform analysis capacity should be admitted");

    assert_eq!(runtime.analysis_memory_bytes, 1_000_000);
}

#[test]
fn screen_capture_config_conversion_rejects_unrepresentable_scheduler_cadence() {
    let capture = hypercolor_types::config::CaptureConfig {
        capture_fps: hypercolor_core::input::screen::MAX_REPRESENTABLE_CAPTURE_FPS + 1,
        ..hypercolor_types::config::CaptureConfig::default()
    };

    let error = screen_capture_config_from(&capture)
        .expect_err("runtime cadence beyond clock resolution must fail admission");

    assert!(format!("{error:#}").contains("scheduler clock limit"));
}

#[test]
fn daemon_initialization_rejects_invalid_capture_config_before_startup() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let config = hypercolor_types::config::HypercolorConfig {
        capture: hypercolor_types::config::CaptureConfig {
            capture_fps: 0,
            ..hypercolor_types::config::CaptureConfig::default()
        },
        ..hypercolor_types::config::HypercolorConfig::default()
    };

    let manager = Arc::new(ConfigManager::from_config_unchecked(
        directory.path().join("hypercolor.toml"),
        config.clone(),
    ));
    let result = super::DaemonState::initialize(BootConfig::from_config_unchecked(config), manager);
    let Err(error) = result else {
        panic!("invalid capture config must stop daemon initialization");
    };

    assert!(format!("{error:#}").contains("capture.capture_fps"));
}

#[cfg(target_os = "linux")]
fn linux_gate(manager: &Arc<ConfigManager>) -> CaptureConfigPersistenceGate {
    let expected = Arc::clone(&manager.get());
    CaptureConfigPersistenceGate::new(Arc::clone(manager), &expected, true)
        .expect("capture persistence authority is reserved")
}

#[cfg(target_os = "linux")]
fn publish_token(gate: &CaptureConfigPersistenceGate, token: &str) {
    gate.publish(CaptureConfigPersistenceUpdate::RestoreToken {
        configured: None,
        resolved: Some(token.to_owned()),
    });
}

#[cfg(target_os = "linux")]
#[test]
fn restore_token_persists_without_source_status() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));

    let gate = linux_gate(&manager);
    publish_token(&gate, "granted-token");

    assert_eq!(
        manager.get().capture.restore_token.as_deref(),
        Some("granted-token"),
        "tokens authorize by epoch alone; no status identity is required"
    );
    drop(manager);
    let restarted = ConfigManager::new(path).expect("config manager reopens");
    assert_eq!(
        restarted.get().capture.restore_token.as_deref(),
        Some("granted-token")
    );
}

#[cfg(target_os = "linux")]
#[test]
fn restore_token_rotations_persist_across_session_generations() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let manager = Arc::new(
        ConfigManager::new(directory.path().join("hypercolor.toml")).expect("config manager opens"),
    );

    let gate = linux_gate(&manager);
    publish_token(&gate, "first-session-token");
    publish_token(&gate, "successor-session-token");
    publish_token(&gate, "post-flap-token");

    assert_eq!(
        manager.get().capture.restore_token.as_deref(),
        Some("post-flap-token"),
        "later sessions must rotate freely; a first-persist pin strands \
         consumed tokens"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn revoked_gate_drops_restore_token_updates() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let manager = Arc::new(
        ConfigManager::new(directory.path().join("hypercolor.toml")).expect("config manager opens"),
    );

    let gate = linux_gate(&manager);
    gate.revoke();
    publish_token(&gate, "late-token");

    assert_eq!(manager.get().capture.restore_token, None);
}
