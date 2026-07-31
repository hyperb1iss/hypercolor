use std::sync::Arc;

use hypercolor_core::config::ConfigManager;
use hypercolor_core::input::screen::{
    PixelExtent, ResolvedCaptureSource, ScreenAdmissionCapacity, ScreenCaptureDemand,
};
use hypercolor_core::input::{SourceKind, SourceStatusHandle, SourceStatusReporter};

use super::{
    CaptureConfigPersistenceGate, screen_analysis_plan_for_demand,
    screen_capacity_plan_for_backend, screen_capture_config_from, windows_capture_source_sink,
};
#[cfg(target_os = "windows")]
use super::{screen_capacity_plan, windows_screen_capture_config_from};

#[cfg(target_os = "windows")]
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

fn persistence_gate(
    manager: &Arc<ConfigManager>,
    committed: bool,
) -> (
    CaptureConfigPersistenceGate,
    Arc<hypercolor_types::config::HypercolorConfig>,
) {
    let expected = Arc::clone(&manager.get());
    let persistence = CaptureConfigPersistenceGate::new(Arc::clone(manager), &expected, committed)
        .expect("capture persistence authority is reserved");
    persistence.bind_source(live_screen_status());
    (persistence, expected)
}

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

#[test]
fn resolved_windows_capture_source_survives_daemon_restart() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:1".to_owned());
    manager.save().expect("legacy source is persisted");

    let (persistence, _) = persistence_gate(&manager, true);
    windows_capture_source_sink(persistence)(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stable".to_owned(),
    });
    drop(manager);

    let restarted = ConfigManager::new(path).expect("config manager reopens after restart");
    assert_eq!(restarted.get().capture.source, "monitor:display:stable");
}

#[test]
fn resolved_windows_capture_source_does_not_overwrite_a_newer_selection() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:new-choice".to_owned());
    manager.save().expect("new source is persisted");

    let (persistence, _) = persistence_gate(&manager, true);
    windows_capture_source_sink(persistence)(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stale".to_owned(),
    });
    drop(manager);

    let restarted = ConfigManager::new(path).expect("config manager reopens after restart");
    assert_eq!(restarted.get().capture.source, "monitor:new-choice");
}

#[test]
fn resolved_windows_capture_source_waits_for_graph_commit() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path.clone()).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:1".to_owned());
    let (persistence, expected) = persistence_gate(&manager, false);

    windows_capture_source_sink(persistence.clone())(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stable".to_owned(),
    });

    assert_eq!(manager.get().capture.source, "monitor:1");
    assert!(!path.exists());
    let activated = manager
        .save_capture_and_activate_if_current(
            &expected,
            persistence.epoch(),
            persistence.source_identity(),
            expected.capture.clone(),
        )
        .expect("prepared capture config is persisted")
        .expect("prepared capture authority activates");
    assert_eq!(activated.capture.source, "monitor:1");
    persistence.commit();
    assert_eq!(manager.get().capture.source, "monitor:display:stable");
    assert!(path.exists());
}

#[test]
fn stale_windows_callback_cannot_rewrite_a_newer_capture_epoch() {
    let directory = tempfile::tempdir().expect("test config directory is created");
    let path = directory.path().join("hypercolor.toml");
    let manager = Arc::new(ConfigManager::new(path).expect("config manager opens"));
    manager.modify(|config| config.capture.source = "monitor:1".to_owned());
    let (stale, _) = persistence_gate(&manager, true);
    let (current, expected) = persistence_gate(&manager, false);
    let activated = manager
        .save_capture_and_activate_if_current(
            &expected,
            current.epoch(),
            current.source_identity(),
            expected.capture.clone(),
        )
        .expect("new capture config is persisted")
        .expect("new capture epoch activates");
    assert_eq!(activated.capture.source, "monitor:1");
    current.commit();

    windows_capture_source_sink(stale)(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:stale".to_owned(),
    });
    assert_eq!(manager.get().capture.source, "monitor:1");

    windows_capture_source_sink(current)(ResolvedCaptureSource {
        configured_source: "monitor:1".to_owned(),
        stable_source: "monitor:display:current".to_owned(),
    });
    assert_eq!(manager.get().capture.source, "monitor:display:current");
}

#[test]
fn screen_capture_config_conversion_preserves_validated_values_exactly() {
    let capture = hypercolor_types::config::CaptureConfig {
        capture_fps: hypercolor_core::input::screen::MAX_REPRESENTABLE_CAPTURE_FPS,
        grid_cols: 64,
        grid_rows: 1,
        smoothing: 1.0,
        gamma: 5.0,
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
}

#[cfg(target_os = "windows")]
#[test]
fn windows_capture_config_installs_the_steady_analysis_budget() {
    let capture = hypercolor_types::config::CaptureConfig::default();
    let capacity = ScreenAdmissionCapacity::new(1_000_000, 2_000_000);

    let runtime = windows_screen_capture_config_from(&capture, capacity)
        .expect("Windows analysis capacity should be admitted");

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

    let result = super::DaemonState::initialize(&config, directory.path().join("hypercolor.toml"));
    let Err(error) = result else {
        panic!("invalid capture config must stop daemon initialization");
    };

    assert!(format!("{error:#}").contains("capture.capture_fps"));
}
