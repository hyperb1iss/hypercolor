//! Integration tests for the Hypercolor REST API.
//!
//! Tests use `axum::Router` directly with tower's `ServiceExt` and
//! `Request::builder()` — no TCP server needed.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use axum::body::Body;
use http::{Request, StatusCode};
use hypercolor_core::config::ConfigManager;
use hypercolor_driver_api::{
    BackendInfo, ControlApplyTarget, DeviceBackend, DiscoveredDevice, DiscoveryCapability,
    DiscoveryConnectBehavior, DiscoveryRequest, DriverConfigView, DriverControlProvider,
    DriverControlStore, DriverDescriptor, DriverError, DriverHost, DriverModule,
    DriverRuntimeCacheProvider, ValidatedControlChanges,
};
#[cfg(feature = "builtin-drivers")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "builtin-drivers")]
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, Notify, Semaphore};
use tower::ServiceExt;
use uuid::Uuid;

use hypercolor_core::bus::{CanvasFrame, DisplayZoneFrame, DisplayZoneTarget};
use hypercolor_core::device::DeviceLifecycleManager;
use hypercolor_core::effect::EffectEntry;
use hypercolor_core::engine::RenderLoopState;
use hypercolor_core::input::screen::ScreenAdmissionCapacity;
use hypercolor_core::input::{
    AudioSource, AudioSourceRole, BrowserConnectionIncarnation, BrowserInputChildKey,
    BrowserInputEdge, BrowserPreviewId, InputData, InputSource, ManagedSourceRole, SourceIssue,
    SourceKind, SourceRoleBinding, SourceSessionWriter, SourceStatusHandle, SourceStatusReporter,
};
use hypercolor_daemon::LayoutTransactionRejection;
use hypercolor_daemon::api;
use hypercolor_daemon::api::local::TrustedLocalApi;
use hypercolor_daemon::app_state::{AppState, AppStateBuilder};
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::domain::layout::{LayoutMutationTestOperation, LayoutMutationTestPoint};
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::library::JsonLibraryStore;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::persistence::AtomicFileWriter;
use hypercolor_daemon::runtime_state;
use hypercolor_daemon::scene_store;
#[cfg(feature = "persistence-test-hooks")]
use hypercolor_daemon::simulators::SimulatedDisplayConfig;
use hypercolor_network::DriverModuleRegistry;
use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::api::scenes::{ReplaceSceneLayerRequest, ReplaceSceneRequest};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::config::{DriverConfigEntry, HypercolorConfig, RenderAccelerationMode};
use hypercolor_types::control::ControlValue as SurfaceControlValue;
use hypercolor_types::control::ControlValue;
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ApplyImpact, ControlActionDescriptor, ControlActionResult,
    ControlActionStatus, ControlAvailabilityExpr, ControlChange, ControlOwner,
    ControlSurfaceDocument, ControlSurfaceEvent, ControlSurfaceScope, ControlValueMap,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceError, DeviceFamily,
    DeviceFeatures, DeviceFingerprint, DeviceId, DeviceInfo, DeviceOrigin, DeviceState,
    DeviceTopologyHint, DisplayFrameFormat, DriverTransportKind, SegmentInfo,
};
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState, GradientStop, PresetTemplate,
};
use hypercolor_types::event::InputButtonState;
use hypercolor_types::event::{HypercolorEvent, ZoneChangeKind};
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{
    ColorInterpolation, DisplayFaceTarget, EasingFunction, Scene, SceneId, SceneKind,
    SceneMutationMode, ScenePriority, TransitionSpec, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

// ── Test Helpers ─────────────────────────────────────────────────────────

static COVER_DATA_DIR_LOCK: LazyLock<AsyncMutex<()>> = LazyLock::new(|| AsyncMutex::new(()));

fn zone_effect_controls(zone: &Zone) -> Option<&HashMap<String, ControlValue>> {
    zone.layers.iter().find_map(|layer| match &layer.source {
        LayerSource::Effect { controls, .. } => Some(controls),
        _ => None,
    })
}

fn zone_effect_preset(zone: &Zone) -> Option<String> {
    zone.layers.iter().find_map(|layer| match &layer.source {
        LayerSource::Effect { preset_id, .. } => preset_id.map(|id| id.to_string()),
        _ => None,
    })
}

std::thread_local! {
    static ISOLATED_STATE_DATA_DIRS: std::cell::RefCell<Vec<tempfile::TempDir>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(feature = "persistence-test-hooks")]
struct InjectedWriterCleanup {
    writer: AtomicFileWriter,
}

#[cfg(feature = "persistence-test-hooks")]
impl InjectedWriterCleanup {
    fn new(writer: AtomicFileWriter) -> Self {
        Self { writer }
    }

    fn writer(&self) -> &AtomicFileWriter {
        &self.writer
    }

    fn reset_and_flush(&self) {
        self.writer.set_injected_replace_failures(0);
        self.writer
            .flush(Duration::from_secs(5))
            .expect("injected persistence destination should converge");
    }
}

#[cfg(feature = "persistence-test-hooks")]
impl Drop for InjectedWriterCleanup {
    fn drop(&mut self) {
        self.writer.set_injected_replace_failures(0);
        let _ = self.writer.flush(Duration::from_secs(5));
    }
}

fn assert_canvas_frame_black(frame: &CanvasFrame) {
    assert_canvas_frame_color(frame, [0, 0, 0]);
}

fn assert_canvas_frame_color(frame: &CanvasFrame, color: [u8; 3]) {
    assert!(
        frame.rgba_bytes().chunks_exact(4).all(|pixel| {
            pixel[0] == color[0] && pixel[1] == color[1] && pixel[2] == color[2] && pixel[3] == 255
        }),
        "canvas frame should be opaque rgb({}, {}, {})",
        color[0],
        color[1],
        color[2]
    );
}

fn assert_display_zone_frame_black(frame: &DisplayZoneFrame) {
    let DisplayZoneFrame::Canvas(frame) = frame else {
        panic!("test display zone frame should be a canvas");
    };
    assert_canvas_frame_black(frame);
}

fn display_zone_frame(canvas: &Canvas, frame_number: u32, timestamp_ms: u32) -> DisplayZoneFrame {
    DisplayZoneFrame::Canvas(CanvasFrame::from_canvas(canvas, frame_number, timestamp_ms))
}

fn isolated_state() -> AppState {
    let (state, tempdir) = isolated_state_with_tempdir();
    ISOLATED_STATE_DATA_DIRS.with(|data_dirs| data_dirs.borrow_mut().push(tempdir));
    state
}

fn isolated_state_with_tempdir() -> (AppState, tempfile::TempDir) {
    let (builder, tempdir) = isolated_state_builder();
    (builder.build(), tempdir)
}

fn isolated_state_builder() -> (AppStateBuilder, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    (AppStateBuilder::new(data_dir), tempdir)
}

fn isolated_state_with_config_manager(config_manager: Arc<ConfigManager>) -> AppState {
    let (builder, tempdir) = isolated_state_builder();
    let state = builder.with_config_manager(config_manager).build();
    ISOLATED_STATE_DATA_DIRS.with(|data_dirs| data_dirs.borrow_mut().push(tempdir));
    state
}

fn isolated_state_with_driver_registry(
    driver_registry: Arc<DriverModuleRegistry>,
) -> (AppState, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    let state = AppStateBuilder::new(data_dir)
        .with_driver_registry(driver_registry)
        .build();
    (state, tempdir)
}

async fn create_stored_layout(state: &AppState, name: &str) -> SpatialLayout {
    let created = state
        .domains
        .layout
        .create(hypercolor_types::api::layouts::CreateLayoutRequest {
            name: name.to_owned(),
            ..Default::default()
        })
        .await
        .expect("test layout should create");
    state
        .domains
        .layout
        .resolve(&created.id)
        .await
        .expect("created test layout should resolve")
}

struct ObservableInputSource {
    status: SourceStatusReporter,
    session: Arc<StdMutex<Option<SourceSessionWriter>>>,
    freshness: Duration,
    running: bool,
}

impl ObservableInputSource {
    fn new(
        source_id: &str,
        demanded: bool,
        freshness: Duration,
    ) -> (Self, Arc<StdMutex<Option<SourceSessionWriter>>>) {
        let session = Arc::new(StdMutex::new(None));
        (
            Self {
                status: SourceStatusReporter::new(
                    source_id,
                    SourceKind::Audio,
                    "test_capture",
                    true,
                    true,
                    demanded,
                ),
                session: Arc::clone(&session),
                freshness,
                running: false,
            },
            session,
        )
    }
}

impl InputSource for ObservableInputSource {
    fn name(&self) -> &'static str {
        "ObservableInputSource"
    }

    fn source_status_handle(&self) -> Option<SourceStatusHandle> {
        Some(self.status.handle())
    }

    fn source_status_reporter(&mut self) -> Option<&mut SourceStatusReporter> {
        Some(&mut self.status)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        let session = self
            .status
            .begin_session()?
            .expect("manager-bound test source should create a status session");
        let sampled_at = std::time::Instant::now();
        session.record_sample(sampled_at, sampled_at + self.freshness, 1)?;
        *self
            .session
            .lock()
            .expect("test source session lock should not be poisoned") = Some(session);
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.status.stop();
        self.running = false;
    }

    fn sample(&mut self) -> anyhow::Result<InputData> {
        Ok(InputData::None)
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl SourceRoleBinding for ObservableInputSource {
    type Role = AudioSourceRole;
}

impl AudioSource for ObservableInputSource {}

struct CoverFixtureGuard {
    _tempdir: tempfile::TempDir,
    data_dir: PathBuf,
}

impl CoverFixtureGuard {
    fn install(slug: &str) -> Self {
        let tempdir = tempfile::tempdir().expect("cover fixture dir should be created");
        let data_dir = tempdir.path().join("data");
        install_cover_fixture(&data_dir, slug);
        ConfigManager::set_data_dir_override(Some(data_dir.clone()));
        Self {
            _tempdir: tempdir,
            data_dir,
        }
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }
}

impl Drop for CoverFixtureGuard {
    fn drop(&mut self) {
        ConfigManager::set_data_dir_override(None);
    }
}

fn install_cover_fixture(data_dir: &Path, slug: &str) {
    let cover_dir = data_dir
        .join("effects")
        .join("screenshots")
        .join("curated")
        .join(slug);
    fs::create_dir_all(&cover_dir).expect("cover fixture dir should be created");
    fs::write(cover_dir.join("default.webp"), b"RIFFTESTWEBP")
        .expect("cover fixture should be written");
}

/// Build a test router with fresh state.
fn test_app() -> axum::Router {
    let state = Arc::new(isolated_state());
    api::build_router(state, None)
}

/// Build a test router with shared state (for multi-step tests).
fn test_app_with_state(state: Arc<AppState>) -> axum::Router {
    api::build_router(state, None)
}

/// Build a test router with a web UI mounted, which installs the SPA
/// fallback. The returned tempdir must outlive the router.
fn test_app_with_ui() -> (axum::Router, tempfile::TempDir) {
    let ui_dir = tempfile::tempdir().expect("ui tempdir should build");
    fs::write(
        ui_dir.path().join("index.html"),
        "<!doctype html><title>hypercolor</title>",
    )
    .expect("index.html should be written");
    let state = Arc::new(isolated_state());
    let app = api::build_router(state, Some(ui_dir.path()));
    (app, ui_dir)
}

/// Assert a response is the canonical `DomainError` not-found envelope
/// for `path`, rather than a bare status or an HTML page.
async fn assert_canonical_route_404(response: axum::response::Response, path: &str) {
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "{path} should answer 404"
    );
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.starts_with("application/json"),
        "{path} should answer JSON, got content-type {content_type:?}"
    );
    let json = body_json(response).await;
    assert!(
        json["error"]["code"]
            .as_str()
            .is_some_and(|code| code.ends_with("_not_found")),
        "{path} error code"
    );
    assert_eq!(
        json["error"]["message"],
        format!("route not found: {path}"),
        "{path} error message"
    );
    assert_eq!(json["meta"]["api_version"], "1.0", "{path} envelope meta");
}

fn test_state_with_temp_config_manager() -> (Arc<AppState>, Arc<ConfigManager>, tempfile::TempDir) {
    let (builder, dir) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(dir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    let state = builder.with_config_manager(Arc::clone(&manager)).build();
    {
        let input_manager = state.input_manager();
        let capacity = input_manager.screen_resource_capacity();
        input_manager
            .set_screen_capacity_plan(capacity, capacity, capacity)
            .expect("isolated input manager should accept its default capacity");
    }
    (Arc::new(state), manager, dir)
}

struct NoopBackend {
    info: BackendInfo,
}

static RUNTIME_CACHE_TEST_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "runtime_cache_test",
    "Runtime Cache Test",
    DriverTransportKind::Network,
    false,
    false,
);

struct RuntimeCacheTestDriver {
    revision: Arc<AtomicUsize>,
}

struct BlockingRuntimeCacheTestDriver {
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
}

impl DriverModule for RuntimeCacheTestDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &RUNTIME_CACHE_TEST_DRIVER
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

impl DriverModule for BlockingRuntimeCacheTestDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &RUNTIME_CACHE_TEST_DRIVER
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheProvider> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DriverRuntimeCacheProvider for RuntimeCacheTestDriver {
    async fn snapshot(
        &self,
        _host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        Ok(BTreeMap::from([(
            "revision".to_owned(),
            serde_json::json!(self.revision.load(Ordering::Acquire)),
        )]))
    }
}

#[async_trait::async_trait]
impl DriverRuntimeCacheProvider for BlockingRuntimeCacheTestDriver {
    async fn snapshot(
        &self,
        _host: &dyn DriverHost,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        self.entered.notify_one();
        let _permit = Arc::clone(&self.release)
            .acquire_owned()
            .await
            .expect("runtime cache release should remain open");
        Ok(BTreeMap::new())
    }
}

impl NoopBackend {
    fn new(id: &str, name: &str) -> Self {
        Self {
            info: BackendInfo {
                id: id.to_owned(),
                name: name.to_owned(),
                description: "Test no-op backend".to_owned(),
            },
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for NoopBackend {
    fn info(&self) -> BackendInfo {
        self.info.clone()
    }

    fn adopt_device(
        &self,
        _discovered: &hypercolor_driver_api::DiscoveredDevice,
    ) -> std::result::Result<(), hypercolor_types::device::DeviceError> {
        Ok(())
    }

    async fn connect(&self, _id: &DeviceId) -> std::result::Result<(), DeviceError> {
        Ok(())
    }

    async fn disconnect(&self, _id: &DeviceId) -> std::result::Result<(), DeviceError> {
        Ok(())
    }

    async fn write_colors(
        &self,
        _id: &DeviceId,
        _colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        Ok(())
    }
}

static ACTION_TEST_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "action_test",
    "Action Test",
    DriverTransportKind::Network,
    false,
    false,
);

struct ActionTestDriver;

#[derive(serde::Deserialize)]
struct ActionTestConfig {
    descriptor: serde_json::Value,
}

impl DriverModule for ActionTestDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &ACTION_TEST_DRIVER
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DriverControlProvider for ActionTestDriver {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        let mut surface = ControlSurfaceDocument::empty(
            "driver:action_test",
            ControlSurfaceScope::Driver {
                driver_id: "action_test".to_owned(),
            },
        );
        surface.actions.push(ControlActionDescriptor {
            id: "ping".to_owned(),
            owner: ControlOwner::Driver {
                driver_id: "action_test".to_owned(),
            },
            group_id: None,
            label: "Ping".to_owned(),
            description: None,
            input_fields: Vec::new(),
            result_type: None,
            confirmation: None,
            apply_impact: ApplyImpact::Live,
            availability: ControlAvailabilityExpr::Always,
            ordering: 0,
        });
        if config.entry.settings.contains_key("descriptor") {
            let config = config.parse_settings::<ActionTestConfig>()?;
            let name = config
                .descriptor
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("test descriptor must have a name"))?;
            surface
                .values
                .insert("descriptor".to_owned(), ControlValue::Text(name.to_owned()));
        }
        if config.entry.settings.contains_key("persisted") {
            surface.values.insert(
                "persisted".to_owned(),
                ControlValue::Text("projected".to_owned()),
            );
        }
        Ok(Some(surface))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        _device: &hypercolor_driver_api::TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        Ok(None)
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> anyhow::Result<ValidatedControlChanges> {
        Ok(ValidatedControlChanges::new(changes.to_vec()))
    }

    async fn apply_changes(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> anyhow::Result<ApplyControlChangesResponse> {
        Ok(ApplyControlChangesResponse {
            surface_id: "driver:action_test".to_owned(),
            previous_revision: 0,
            revision: 0,
            accepted: changes
                .changes
                .into_iter()
                .map(|change| hypercolor_types::controls::AppliedControlChange {
                    field_id: change.field_id,
                    value: change.value,
                })
                .collect(),
            rejected: Vec::new(),
            impacts: changes.impacts,
            values: ControlValueMap::new(),
        })
    }

    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> anyhow::Result<ControlActionResult> {
        assert_eq!(action_id, "ping");
        Ok(ControlActionResult {
            surface_id: String::new(),
            action_id: String::new(),
            status: ControlActionStatus::Completed,
            result: None,
            revision: 0,
        })
    }
}

static RESCAN_TEST_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "rescan_test",
    "Rescan Test",
    DriverTransportKind::Network,
    true,
    false,
);

struct RescanTestDriver {
    discoveries: Arc<AtomicUsize>,
}

impl RescanTestDriver {
    fn new(discoveries: Arc<AtomicUsize>) -> Self {
        Self { discoveries }
    }
}

impl DriverModule for RescanTestDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &RESCAN_TEST_DRIVER
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DiscoveryCapability for RescanTestDriver {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> std::result::Result<Vec<DiscoveredDevice>, DriverError> {
        self.discoveries.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }
}

#[async_trait::async_trait]
impl DriverControlProvider for RescanTestDriver {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        Ok(Some(ControlSurfaceDocument::empty(
            "driver:rescan_test",
            ControlSurfaceScope::Driver {
                driver_id: "rescan_test".to_owned(),
            },
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        _device: &hypercolor_driver_api::TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        Ok(None)
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> anyhow::Result<ValidatedControlChanges> {
        Ok(ValidatedControlChanges {
            changes: changes.to_vec(),
            impacts: vec![ApplyImpact::DiscoveryRescan],
        })
    }

    async fn apply_changes(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> anyhow::Result<ApplyControlChangesResponse> {
        Ok(ApplyControlChangesResponse {
            surface_id: "driver:provider_typo".to_owned(),
            previous_revision: 0,
            revision: 0,
            accepted: changes
                .changes
                .into_iter()
                .map(|change| hypercolor_types::controls::AppliedControlChange {
                    field_id: change.field_id,
                    value: change.value,
                })
                .collect(),
            rejected: Vec::new(),
            impacts: changes.impacts,
            values: ControlValueMap::new(),
        })
    }

    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> anyhow::Result<ControlActionResult> {
        bail!("unexpected rescan test action: {action_id}")
    }
}

static BLOCKING_RECONNECT_TEST_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "blocking_reconnect_test",
    "Blocking Reconnect Test",
    DriverTransportKind::Network,
    true,
    false,
);

struct BlockingReconnectTestDriver {
    discoveries: Arc<AtomicUsize>,
    release: Arc<Semaphore>,
}

impl BlockingReconnectTestDriver {
    fn new(discoveries: Arc<AtomicUsize>, release: Arc<Semaphore>) -> Self {
        Self {
            discoveries,
            release,
        }
    }
}

impl DriverModule for BlockingReconnectTestDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &BLOCKING_RECONNECT_TEST_DRIVER
    }

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DiscoveryCapability for BlockingReconnectTestDriver {
    async fn discover(
        &self,
        _host: &dyn DriverHost,
        _request: &DiscoveryRequest,
        _config: DriverConfigView<'_>,
    ) -> std::result::Result<Vec<DiscoveredDevice>, DriverError> {
        self.discoveries.fetch_add(1, Ordering::Relaxed);
        let _permit = Arc::clone(&self.release)
            .acquire_owned()
            .await
            .expect("blocking reconnect semaphore should stay open");
        Ok(Vec::new())
    }
}

static UNSUPPORTED_IMPACT_TEST_DRIVER: DriverDescriptor = DriverDescriptor::new(
    "unsupported_impact_test",
    "Unsupported Impact Test",
    DriverTransportKind::Network,
    false,
    false,
);

struct UnsupportedImpactTestDriver;

impl DriverModule for UnsupportedImpactTestDriver {
    fn descriptor(&self) -> &'static DriverDescriptor {
        &UNSUPPORTED_IMPACT_TEST_DRIVER
    }

    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl DriverControlProvider for UnsupportedImpactTestDriver {
    async fn driver_surface(
        &self,
        _host: &dyn DriverHost,
        _config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        Ok(Some(ControlSurfaceDocument::empty(
            "driver:unsupported_impact_test",
            ControlSurfaceScope::Driver {
                driver_id: "unsupported_impact_test".to_owned(),
            },
        )))
    }

    async fn device_surface(
        &self,
        _host: &dyn DriverHost,
        device: &hypercolor_driver_api::TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>> {
        Ok(Some(ControlSurfaceDocument::empty(
            format!("driver:unsupported_impact_test:device:{}", device.device_id),
            ControlSurfaceScope::Device {
                device_id: device.device_id,
                driver_id: "unsupported_impact_test".to_owned(),
            },
        )))
    }

    async fn validate_changes(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> anyhow::Result<ValidatedControlChanges> {
        Ok(ValidatedControlChanges {
            changes: changes.to_vec(),
            impacts: vec![ApplyImpact::TopologyRebuild],
        })
    }

    async fn apply_changes(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        _changes: ValidatedControlChanges,
    ) -> anyhow::Result<ApplyControlChangesResponse> {
        bail!("unsupported impact test driver should fail before apply")
    }

    async fn invoke_action(
        &self,
        _host: &dyn DriverHost,
        _target: &ControlApplyTarget<'_>,
        action_id: &str,
        _input: ControlValueMap,
    ) -> anyhow::Result<ControlActionResult> {
        bail!("unexpected unsupported impact test action: {action_id}")
    }
}

struct DisconnectRecordingBackend {
    expected_device_id: DeviceId,
    disconnects: Arc<AtomicUsize>,
    connected: AtomicBool,
}

type StaticOutputWrites = Arc<StdMutex<Vec<(DeviceId, Vec<[u8; 3]>)>>>;

struct StaticOutputRecordingBackend {
    writes: StaticOutputWrites,
}

struct IdentifyRecordingBackend {
    writes: Arc<StdMutex<Vec<Vec<[u8; 3]>>>>,
}

#[async_trait::async_trait]
impl DeviceBackend for StaticOutputRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "static-output".to_owned(),
            name: "Static Output Recording Backend".to_owned(),
            description: "Records global static output frames".to_owned(),
        }
    }

    fn adopt_device(
        &self,
        _discovered: &hypercolor_driver_api::DiscoveredDevice,
    ) -> std::result::Result<(), hypercolor_types::device::DeviceError> {
        Ok(())
    }

    async fn connect(&self, _id: &DeviceId) -> std::result::Result<(), DeviceError> {
        Ok(())
    }

    async fn disconnect(&self, _id: &DeviceId) -> std::result::Result<(), DeviceError> {
        Ok(())
    }

    async fn write_colors(
        &self,
        id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        self.writes
            .lock()
            .expect("static output writes lock")
            .push((*id, colors.to_vec()));
        Ok(())
    }
}

#[async_trait::async_trait]
impl DeviceBackend for IdentifyRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "wled".to_owned(),
            name: "Identify Recording Backend".to_owned(),
            description: "Records identify and held output frames".to_owned(),
        }
    }

    fn adopt_device(
        &self,
        _discovered: &hypercolor_driver_api::DiscoveredDevice,
    ) -> std::result::Result<(), hypercolor_types::device::DeviceError> {
        Ok(())
    }

    async fn connect(&self, _id: &DeviceId) -> std::result::Result<(), DeviceError> {
        Ok(())
    }

    async fn disconnect(&self, _id: &DeviceId) -> std::result::Result<(), DeviceError> {
        Ok(())
    }

    async fn write_colors(
        &self,
        _id: &DeviceId,
        colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        self.writes
            .lock()
            .expect("identify output writes lock")
            .push(colors.to_vec());
        Ok(())
    }
}

impl DisconnectRecordingBackend {
    fn new(expected_device_id: DeviceId, disconnects: Arc<AtomicUsize>) -> Self {
        Self {
            expected_device_id,
            disconnects,
            connected: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl DeviceBackend for DisconnectRecordingBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "wled".to_owned(),
            name: "Disconnect Recording Backend".to_owned(),
            description: "Tracks lifecycle disconnects from the API".to_owned(),
        }
    }

    fn adopt_device(
        &self,
        _discovered: &hypercolor_driver_api::DiscoveredDevice,
    ) -> std::result::Result<(), hypercolor_types::device::DeviceError> {
        Ok(())
    }

    fn supports_temporary_direct_control(&self, _info: &DeviceInfo) -> bool {
        true
    }

    async fn connect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::protocol(id, "unexpected device id"));
        }
        self.connected.store(true, Ordering::Release);
        Ok(())
    }

    async fn disconnect(&self, id: &DeviceId) -> std::result::Result<(), DeviceError> {
        if *id != self.expected_device_id {
            return Err(DeviceError::protocol(id, "unexpected device id"));
        }
        if !self.connected.load(Ordering::Acquire) {
            return Err(DeviceError::Disconnected {
                device: id.to_string(),
            });
        }
        self.connected.store(false, Ordering::Release);
        self.disconnects.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn write_colors(
        &self,
        _id: &DeviceId,
        _colors: &[[u8; 3]],
    ) -> std::result::Result<(), DeviceError> {
        Ok(())
    }
}

async fn register_noop_backend(state: &Arc<AppState>, id: &str, name: &str) {
    let mut manager = state.backend_manager.lock().await;
    manager.register_backend(Arc::new(NoopBackend::new(id, name)));
}

/// A `PATCH /api/v1/output` request carrying the given JSON document.
fn output_patch_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri("/api/v1/output")
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .expect("failed to build output patch request")
}

/// Extract the JSON body from a response.
async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).expect("failed to parse JSON body")
}

async fn request_with_layout_ack(
    app: axum::Router,
    request: Request<Body>,
    state: &Arc<AppState>,
) -> (axum::response::Response, Vec<SpatialLayout>) {
    let request = async move {
        app.oneshot(request)
            .await
            .expect("failed to execute request")
    };
    drive_request_with_layout_ack(request, state).await
}

async fn trusted_request_with_layout_ack(
    request: Request<Body>,
    state: &Arc<AppState>,
) -> (axum::response::Response, Vec<SpatialLayout>) {
    let api = TrustedLocalApi::new(Arc::clone(state));
    let request = async move {
        api.execute(request)
            .await
            .expect("trusted local request should execute")
    };
    drive_request_with_layout_ack(request, state).await
}

#[cfg(feature = "persistence-test-hooks")]
async fn request_with_layout_ack_and_hook<F, Fut>(
    app: axum::Router,
    request: Request<Body>,
    state: &Arc<AppState>,
    before_publication: F,
) -> (axum::response::Response, Vec<SpatialLayout>)
where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let request = async move {
        app.oneshot(request)
            .await
            .expect("failed to execute request")
    };
    drive_request_with_layout_ack_and_hook(request, state, before_publication).await
}

async fn drive_request_with_layout_ack<R>(
    request: R,
    state: &Arc<AppState>,
) -> (axum::response::Response, Vec<SpatialLayout>)
where
    R: Future<Output = axum::response::Response>,
{
    tokio::pin!(request);
    let executor = state.layout_publication_test_executor();
    let mut publications: Vec<
        tokio::task::JoinHandle<Result<Option<SpatialLayout>, LayoutTransactionRejection>>,
    > = Vec::new();
    loop {
        tokio::select! {
            response = &mut request => {
                let mut applied = Vec::new();
                for publication in publications {
                    if let Some(layout) = publication
                        .await
                        .expect("layout publication task should not panic")
                        .expect("layout publication should succeed")
                    {
                        applied.push(layout);
                    }
                }
                return (response, applied);
            }
            () = tokio::time::sleep(Duration::from_millis(1)) => {
                if executor.pending_layout_publications() > 0 {
                    let executor = executor.clone();
                    publications.push(tokio::spawn(async move {
                        executor.execute_next_layout_publication().await
                    }));
                }
            }
        }
    }
}

#[cfg(feature = "persistence-test-hooks")]
async fn drive_request_with_layout_ack_and_hook<R, F, Fut>(
    request: R,
    state: &Arc<AppState>,
    before_publication: F,
) -> (axum::response::Response, Vec<SpatialLayout>)
where
    R: Future<Output = axum::response::Response>,
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::pin!(request);
    let executor = state.layout_publication_test_executor();
    let mut publications: Vec<
        tokio::task::JoinHandle<Result<Option<SpatialLayout>, LayoutTransactionRejection>>,
    > = Vec::new();
    loop {
        tokio::select! {
            response = &mut request => {
                let mut applied = Vec::new();
                for publication in publications {
                    if let Some(layout) = publication
                        .await
                        .expect("layout publication task should not panic")
                        .expect("layout publication should succeed")
                    {
                        applied.push(layout);
                    }
                }
                return (response, applied);
            }
            () = tokio::time::sleep(Duration::from_millis(1)) => {
                if executor.pending_layout_publications() > 0 {
                    let executor = executor.clone();
                    let before_publication = before_publication.clone();
                    publications.push(tokio::spawn(async move {
                        executor
                            .execute_next_layout_publication_with_hook(before_publication)
                            .await
                    }));
                }
            }
        }
    }
}

#[cfg(feature = "persistence-test-hooks")]
async fn run_two_layout_publications_with_gates(
    state: Arc<AppState>,
    first_publication_entered: Arc<Notify>,
    release_first_publication: Arc<Semaphore>,
    release_second_admission: Arc<Semaphore>,
) {
    tokio::time::timeout(Duration::from_secs(5), async move {
        let executor = state.layout_publication_test_executor();
        let mut publication_index = 0;
        while publication_index < 2 {
            if executor.pending_layout_publications() == 0 {
                tokio::task::yield_now().await;
                continue;
            }
            let index = publication_index;
            let entered = Arc::clone(&first_publication_entered);
            let release = Arc::clone(&release_first_publication);
            executor
                .execute_next_layout_publication_with_hook(move || async move {
                    if index == 0 {
                        entered.notify_one();
                        let _permit =
                            tokio::time::timeout(Duration::from_secs(2), release.acquire_owned())
                                .await
                                .expect("first publication release should arrive")
                                .expect("first publication gate should remain open");
                    }
                })
                .await
                .expect("layout publication should succeed")
                .expect("layout publication should be pending");
            publication_index += 1;
            if publication_index == 1 {
                let _permit = tokio::time::timeout(
                    Duration::from_secs(2),
                    Arc::clone(&release_second_admission).acquire_owned(),
                )
                .await
                .expect("second admission release should arrive")
                .expect("second admission gate should remain open");
            }
        }
    })
    .await
    .expect("layout publication worker should finish");
}

#[cfg(feature = "persistence-test-hooks")]
async fn run_one_layout_publication_with_gate(
    state: Arc<AppState>,
    publication_entered: Arc<Notify>,
    release_publication: Arc<Semaphore>,
) {
    tokio::time::timeout(Duration::from_secs(5), async move {
        let executor = state.layout_publication_test_executor();
        loop {
            if executor.pending_layout_publications() > 0 {
                let entered = Arc::clone(&publication_entered);
                let release = Arc::clone(&release_publication);
                executor
                    .execute_next_layout_publication_with_hook(move || async move {
                        entered.notify_one();
                        let _permit =
                            tokio::time::timeout(Duration::from_secs(2), release.acquire_owned())
                                .await
                                .expect("publication release should arrive")
                                .expect("publication gate should remain open");
                    })
                    .await
                    .expect("layout publication should succeed")
                    .expect("layout publication should be pending");
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("layout publication worker should finish");
}

async fn run_layout_publications(
    state: Arc<AppState>,
    expected_count: usize,
) -> Vec<SpatialLayout> {
    let mut applied = Vec::with_capacity(expected_count);
    let executor = state.layout_publication_test_executor();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        while applied.len() < expected_count {
            if let Some(layout) = executor
                .execute_next_layout_publication()
                .await
                .expect("layout publication should succeed")
            {
                applied.push(layout);
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "layout publication worker should finish after {expected_count} publications; observed {}",
        applied.len()
    );
    applied
}

#[cfg(feature = "persistence-test-hooks")]
async fn seed_stale_auto_layout_zone(state: &AppState, device_id: &DeviceId) -> String {
    let tracked = state
        .device_registry
        .get(device_id)
        .await
        .expect("repair target should be registered");
    let fingerprint = state.device_registry.fingerprint_for_id(device_id).await;
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&tracked.info, fingerprint.as_ref());
    let mut layout = state.spatial_engine.snapshot().layout().as_ref().clone();
    layout.id = format!("stale-auto-layout-{device_id}");
    "Stale Auto Layout".clone_into(&mut layout.name);
    assert_eq!(
        state.domains.layout.test_fixture().append_auto_zones(
            &mut layout,
            &layout_device_id,
            &tracked.info,
        ),
        1
    );
    let stale_zone = layout
        .zones
        .iter_mut()
        .find(|zone| zone.device_id == layout_device_id)
        .expect("seeded auto-layout zone should exist");
    "Stale Auto Layout Zone".clone_into(&mut stale_zone.name);
    let mut repair_probe = layout.clone();
    assert_eq!(
        state.domains.layout.test_fixture().reconcile_auto_zones(
            &mut repair_probe,
            &layout_device_id,
            &tracked.info,
        ),
        1,
        "seeded auto-layout zone should require repair"
    );
    state.domains.layout.test_fixture().replace_current(layout);
    layout_device_id
}

#[cfg(feature = "persistence-test-hooks")]
async fn wait_for_async_condition<F, Fut>(mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if condition().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("condition should become true");
}

#[cfg(feature = "persistence-test-hooks")]
async fn request_with_layout_rejection(
    app: axum::Router,
    request: Request<Body>,
    state: &Arc<AppState>,
    rejection: LayoutTransactionRejection,
) -> axum::response::Response {
    let request = app.oneshot(request);
    tokio::pin!(request);
    let executor = state.layout_publication_test_executor();
    loop {
        tokio::select! {
            response = &mut request => {
                return response.expect("failed to execute request");
            }
            () = tokio::time::sleep(Duration::from_millis(1)) => {
                executor.reject_next_layout_publication(rejection.clone());
            }
        }
    }
}

/// Extract UTF-8 text body from a response.
async fn body_text(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    String::from_utf8(bytes.to_vec()).expect("failed to decode UTF-8 body")
}

fn multipart_upload_request(file_name: &str, html: &str) -> Request<Body> {
    let boundary = "hypercolor-upload-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: text/html\r\n\r\n{html}\r\n--{boundary}--\r\n"
    );

    Request::builder()
        .method("POST")
        .uri("/api/v1/effects/install")
        .header(
            http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("failed to build multipart request")
}

fn multipart_upload_request_without_filename(html: &str) -> Request<Body> {
    let boundary = "hypercolor-upload-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"\r\nContent-Type: text/html\r\n\r\n{html}\r\n--{boundary}--\r\n"
    );

    Request::builder()
        .method("POST")
        .uri("/api/v1/effects/install")
        .header(
            http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("failed to build multipart request")
}

// ── Health / Status ──────────────────────────────────────────────────────

#[tokio::test]
async fn health_check_returns_200() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["status"], "healthy");
    assert_eq!(json["checks"]["render_loop"], "idle");
    assert_eq!(json["checks"]["device_backends"], "ok");
    assert_eq!(json["checks"]["event_bus"], "idle");
    assert!(json["version"].is_string());
}

#[tokio::test]
async fn health_check_reports_stopped_render_loop_as_degraded() {
    let state = Arc::new(isolated_state());
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.stop();
    }

    let app = test_app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let json = body_json(response).await;
    assert_eq!(json["status"], "degraded");
    assert_eq!(json["checks"]["render_loop"], "degraded");
}

#[tokio::test]
async fn spa_fallback_serves_index_html_for_client_routes() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let index_path = tempdir.path().join("index.html");
    fs::write(&index_path, "<!doctype html><title>hypercolor</title>")
        .expect("index.html should be written");

    let app = api::build_router(Arc::new(isolated_state()), Some(tempdir.path()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/layout")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let body = body_text(response).await;
    assert!(body.contains("<title>hypercolor</title>"));
}

#[tokio::test]
async fn system_returns_identity_and_status_with_envelope() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert!(
        json["data"]["status"]["running"]
            .as_bool()
            .expect("running should be bool")
    );
    assert!(
        json["data"]["status"]["global_brightness"]
            .as_u64()
            .is_some(),
        "global_brightness should be an integer percentage"
    );
    assert!(
        json["data"]["status"]["active_scene"].is_string(),
        "active_scene should be a string"
    );
    assert!(
        json["data"]["status"]["active_scene_snapshot_locked"].is_boolean(),
        "active_scene_snapshot_locked should be a bool"
    );
    assert!(json["meta"]["api_version"].is_string());
    assert!(json["meta"]["request_id"].is_string());
    assert!(json["meta"]["timestamp"].is_string());

    // Request ID should start with "req_"
    let request_id = json["meta"]["request_id"]
        .as_str()
        .expect("request_id should be a string");
    assert!(
        request_id.starts_with("req_"),
        "request_id should start with req_"
    );
    assert_eq!(
        json["data"]["status"]["config_path"],
        serde_json::json!(default_config_path())
    );
    assert!(
        json["data"]["status"]["data_dir"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "data_dir should be a non-empty string"
    );
    assert!(
        json["data"]["status"]["cache_dir"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "cache_dir should be a non-empty string"
    );
    assert_eq!(json["data"]["status"]["audio_available"], false);
    assert_eq!(
        json["data"]["status"]["capture_available"],
        serde_json::json!(
            cfg!(any(target_os = "windows", target_os = "macos"))
                || (cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some())
        )
    );
}

#[tokio::test]
async fn status_derives_audio_availability_from_registered_sources() {
    let state = Arc::new(isolated_state());
    let (source, _) = ObservableInputSource::new("available_audio", false, Duration::from_secs(1));
    state
        .input_manager()
        .add_source(ManagedSourceRole::audio(Box::new(source)))
        .expect("observable audio source should register");
    let app = test_app_with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["status"]["audio_available"], true);
}

#[tokio::test]
async fn status_reports_stale_source_health_without_captured_contents() {
    const PRIVACY_SENTINEL: &str = "capture_secret_73_do_not_expose";

    let state = Arc::new(isolated_state());
    let (source, _) =
        ObservableInputSource::new("stale_test_audio", true, Duration::from_millis(1));
    {
        let manager = state.input_manager();
        manager
            .add_source(ManagedSourceRole::audio(Box::new(source)))
            .expect("stale audio source should register");
        manager.start_all().expect("test input graph should start");
    }
    let browser_attachment = state
        .browser_input
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(1),
            BrowserPreviewId::new(PRIVACY_SENTINEL),
        ))
        .expect("browser preview should attach");
    browser_attachment
        .inject([BrowserInputEdge::Key {
            key: PRIVACY_SENTINEL.to_owned(),
            state: InputButtonState::Pressed,
        }])
        .expect("browser key should inject");
    tokio::time::sleep(Duration::from_millis(25)).await;

    let response = test_app_with_state(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    let json = body_json(response).await;
    let input = &json["data"]["status"]["input"];
    let stale = input["sources"]
        .as_array()
        .expect("sources should be an array")
        .iter()
        .find(|source| source["source_id"] == "stale_test_audio")
        .expect("test source should be exposed");

    assert_eq!(stale["freshness"], "stale");
    assert_eq!(stale["freshness_remaining_ms"], 0);
    assert_eq!(stale["freshness_issue"]["code"], "stale_data");
    assert_eq!(stale["issue"]["code"], "stale_data");
    assert!(stale["last_sample_age_ms"].is_number());
    assert!(input["source_graph_generation"].as_u64().is_some());
    assert_eq!(input["backends"], serde_json::json!([]));
    assert!(
        !serde_json::to_string(input)
            .expect("input status should serialize")
            .contains(PRIVACY_SENTINEL)
    );
}

#[tokio::test]
async fn diagnose_default_set_includes_memory_as_a_finding() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/diagnose")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let finding = json["data"]["checks"]
        .as_array()
        .expect("diagnose checks should be an array")
        .iter()
        .find(|check| check["name"] == "servo_memory")
        .expect("default diagnostics should include Servo memory");
    assert_eq!(finding["category"], "memory");
    assert!(matches!(
        finding["status"].as_str(),
        Some("pass" | "warning" | "fail")
    ));
}

#[tokio::test]
async fn input_status_and_diagnose_observe_failure_from_lock_free_handles() {
    let state = Arc::new(isolated_state());
    let (source, session) =
        ObservableInputSource::new("failed_test_audio", true, Duration::from_millis(1));
    {
        let manager = state.input_manager();
        manager
            .set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(2_000_000, 1_500_000),
                ScreenAdmissionCapacity::new(1_000_000, 900_000),
                ScreenAdmissionCapacity::new(750_000, 700_000),
            )
            .expect("empty manager should accept exact test capacity");
        manager
            .add_source(ManagedSourceRole::audio(Box::new(source)))
            .expect("failed audio source should register");
        manager.start_all().expect("test input graph should start");
    }

    let manager_guard = state.input_manager();
    let (demanded_stopped, _) =
        ObservableInputSource::new("demanded_stopped_audio", true, Duration::from_secs(30));
    manager_guard
        .add_source(ManagedSourceRole::audio(Box::new(demanded_stopped)))
        .expect("demanded stopped audio source should register");
    let (undemanded, _) =
        ObservableInputSource::new("undemanded_stopped_audio", false, Duration::from_secs(30));
    manager_guard
        .add_source(ManagedSourceRole::audio(Box::new(undemanded)))
        .expect("undemanded audio source should register");
    let session = session
        .lock()
        .expect("test source session lock should not be poisoned")
        .clone()
        .expect("test source should publish its session");
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(session.failed(SourceIssue::new(
        "capture_worker_exited",
        "test worker exited",
        true,
    )));

    let app = test_app_with_state(Arc::clone(&state));
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        app.clone().oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        ),
    )
    .await
    .expect("status must not wait for the input manager")
    .expect("status request should succeed");
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["status"]["screen_capture_capacity"]["admission_enforced"],
        true
    );
    assert_eq!(
        json["data"]["status"]["screen_capture_capacity"]["physical_transition_byte_capacity"],
        2_000_000
    );
    assert_eq!(
        json["data"]["status"]["screen_capture_capacity"]["physical_transition_backend_capacity"],
        1_500_000
    );
    assert_eq!(
        json["data"]["status"]["screen_capture_capacity"]["physical_available_bytes"],
        1_500_000
    );
    assert_eq!(
        json["data"]["status"]["screen_capture_capacity"]["steady_total_byte_budget"],
        1_000_000
    );
    let failed = json["data"]["status"]["input"]["sources"]
        .as_array()
        .expect("sources should be an array")
        .iter()
        .find(|source| source["source_id"] == "failed_test_audio")
        .expect("failed source should be exposed");
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["issue"]["code"], "stale_data");
    assert_eq!(failed["freshness_issue"]["code"], "stale_data");
    assert_eq!(failed["lifecycle_issue"]["code"], "capture_worker_exited");

    let response = tokio::time::timeout(
        Duration::from_secs(1),
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/diagnose")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"checks":["input"]}"#))
                .expect("failed to build request"),
        ),
    )
    .await
    .expect("diagnose must not wait for the input manager")
    .expect("diagnose request should succeed");

    let json = body_json(response).await;
    let checks = json["data"]["checks"]
        .as_array()
        .expect("diagnose checks should be an array");
    let failed_check = checks
        .iter()
        .find(|check| check["category"] == "input" && check["name"] == "failed_test_audio")
        .expect("failed demanded source should produce an input check");
    assert_eq!(failed_check["status"], "fail");
    assert!(
        failed_check["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("capture_worker_exited")),
        "terminal diagnostics must describe the lifecycle failure, not stale data"
    );
    assert!(checks.iter().any(|check| {
        check["category"] == "input"
            && check["name"] == "demanded_stopped_audio"
            && check["status"] == "warning"
    }));
    assert!(
        !checks
            .iter()
            .any(|check| check["name"] == "undemanded_stopped_audio")
    );
    assert!(
        json["data"]["snapshot"]["input"]["source_graph_generation"]
            .as_u64()
            .is_some_and(|generation| generation > 0)
    );
    let undemanded = json["data"]["snapshot"]["input"]["sources"]
        .as_array()
        .expect("sources should be an array")
        .iter()
        .find(|source| source["source_id"] == "undemanded_stopped_audio")
        .expect("undemanded source should remain visible in the snapshot");
    assert_eq!(undemanded["source_graph_generation"], 0);
    assert_eq!(undemanded["session_generation"], 0);
}

#[tokio::test]
async fn status_reports_stopped_render_loop_as_not_running() {
    let state = Arc::new(isolated_state());
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.stop();
    }

    let app = test_app_with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["status"]["running"], serde_json::json!(false));
    assert_eq!(json["data"]["status"]["render_loop"]["state"], "stopped");
}

#[tokio::test]
async fn status_prefers_live_config_manager_path() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let custom_config_path = tempdir.path().join("custom-settings.toml");
    let config_manager = Arc::new(
        ConfigManager::new(custom_config_path.clone()).expect("config manager should build"),
    );
    let state = isolated_state_with_config_manager(config_manager);

    let app = test_app_with_state(Arc::new(state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(
        json["data"]["status"]["config_path"],
        serde_json::json!(custom_config_path.display().to_string())
    );
}

#[tokio::test]
async fn global_brightness_endpoint_updates_status_and_persistence() {
    let (state, tmp) = test_state_with_temp_output_store();
    let app = test_app_with_state(Arc::clone(&state));

    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/output")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"brightness":0.42}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["brightness"], 0.42);
    assert_eq!(update_json["data"]["power"], "running");

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/output")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["brightness"], 0.42);

    let status_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(status_response.status(), StatusCode::OK);
    let status_json = body_json(status_response).await;
    assert_eq!(status_json["data"]["status"]["global_brightness"], 42);

    let device_settings_raw = fs::read_to_string(state.state_dir.join("device-settings.json"))
        .expect("device settings file should exist");
    let device_settings_json: serde_json::Value =
        serde_json::from_str(&device_settings_raw).expect("device settings file should be valid");
    assert_eq!(
        device_settings_json["global_brightness"],
        serde_json::json!(0.42)
    );

    assert!(
        !tmp.path().join("runtime-state.json").exists(),
        "brightness must not create a second persisted authority"
    );
}

#[tokio::test]
async fn audio_devices_returns_default_option_and_current_value() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system/audio-devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let devices = json["data"]["devices"]
        .as_array()
        .expect("devices should be an array");
    assert!(
        !devices.is_empty(),
        "devices should include the default option"
    );
    assert_eq!(devices[0]["id"], "default");
    assert_eq!(devices[0]["name"], "System Monitor");
    assert_eq!(devices[1]["id"], "microphone");
    assert_eq!(devices[2]["id"], "none");
    assert_eq!(json["data"]["current"], "default");
}

#[tokio::test]
async fn audio_devices_preserve_custom_configured_id_without_rewrite() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path).expect("config manager should build"));
    let mut config = HypercolorConfig::default();
    config.audio.device = "pulse-monitor".to_owned();
    config_manager.update(config);

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system/audio-devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["current"], "pulse-monitor");
    assert!(
        json["data"]["devices"]
            .as_array()
            .expect("devices should be an array")
            .iter()
            .any(|device| device["id"] == "pulse-monitor"),
        "configured noncanonical device id should remain visible instead of being rewritten"
    );
}

#[test]
fn audio_device_filter_hides_synthetic_outputs_from_named_input_list() {
    assert!(
        !hypercolor_daemon::api::system::should_offer_named_audio_device("PipeWire Sound Server",)
    );
    assert!(
        !hypercolor_daemon::api::system::should_offer_named_audio_device("PulseAudio Sound Server",)
    );
    assert!(
        !hypercolor_daemon::api::system::should_offer_named_audio_device(
            "Monitor of Built-in Audio Analog Stereo",
        )
    );
    assert!(
        !hypercolor_daemon::api::system::should_offer_named_audio_device(
            "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
        )
    );
    assert!(
        hypercolor_daemon::api::system::should_offer_named_audio_device(
            "Razer Seiren V3 Chroma, USB Audio",
        )
    );
    assert!(
        !hypercolor_daemon::api::system::should_offer_named_audio_device(
            "Rate Converter Plugin Using Speex Resampler",
        )
    );
    assert!(
        !hypercolor_daemon::api::system::should_offer_named_audio_device(
            "Discard all samples (playback) or generate zero samples (capture)",
        )
    );
}

/// A `PUT /api/v1/config/keys/{key}` request.
///
/// `live` gates whether the daemon re-applies the live sections the key
/// touches; omitting it takes the route's default, which is to apply.
fn config_put_request(key: &str, value: &serde_json::Value, live: Option<bool>) -> Request<Body> {
    let uri = match live {
        Some(live) => format!("/api/v1/config/keys/{key}?live={live}"),
        None => format!("/api/v1/config/keys/{key}"),
    };
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(value.to_string()))
        .expect("failed to build request")
}

/// A `DELETE /api/v1/config/keys/{key}` request: reset one key.
fn config_delete_request(key: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(format!("/api/v1/config/keys/{key}"))
        .body(Body::empty())
        .expect("failed to build request")
}

async fn execute_trusted_config_request(
    state: &Arc<AppState>,
    request: Request<Body>,
) -> axum::response::Response {
    TrustedLocalApi::new(Arc::clone(state))
        .execute(request)
        .await
        .expect("trusted config request should execute")
}

/// Read a table-driven test value the way a human types it: JSON when it
/// parses, a JSON string otherwise.
fn config_test_value(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

#[tokio::test]
async fn config_set_audio_device_persists_without_live_rebuild_by_default() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let state = Arc::new(state);

    let response = execute_trusted_config_request(
        &state,
        config_put_request(
            "audio.device",
            &serde_json::json!("microphone"),
            Some(false),
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "microphone");
    assert_eq!(json["data"]["live"], false);

    {
        let input_manager = state.input_manager();
        assert_eq!(
            input_manager.source_count(),
            0,
            "the direct browser registry stays outside the input manager"
        );
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.audio.device, "microphone");
}

#[tokio::test]
async fn config_set_publishes_exactly_one_config_changed_from_the_manager() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));
    let state = Arc::new(isolated_state_with_config_manager(config_manager));
    let mut events = state.event_bus.subscribe_all();

    let response = execute_trusted_config_request(
        &state,
        config_put_request(
            "audio.device",
            &serde_json::json!("microphone"),
            Some(false),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let mut changes = Vec::new();
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::ConfigChanged {
            key,
            old_value,
            new_value,
        } = timestamped.event
        {
            changes.push((key, old_value, new_value));
        }
    }
    assert_eq!(
        changes,
        vec![(
            "audio.device".to_owned(),
            Some(serde_json::json!("default")),
            serde_json::json!("microphone")
        )],
        "the handler no longer publishes on its own: one save, one event"
    );
}

#[tokio::test]
async fn config_set_compositor_acceleration_key_updates_and_persists() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(config_put_request(
            "effect_engine.compositor_acceleration_mode",
            &serde_json::json!("cpu"),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(
        json["data"]["key"],
        "effect_engine.compositor_acceleration_mode"
    );
    assert_eq!(json["data"]["value"], "cpu");

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(
        config.effect_engine.compositor_acceleration_mode,
        RenderAccelerationMode::Cpu
    );
}

#[tokio::test]
async fn config_set_driver_registry_key_updates_driver_config() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(config_put_request(
            "drivers.wled.known_ips",
            &serde_json::json!(["192.168.1.50"]),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "drivers.wled.known_ips");
    assert_eq!(
        json["data"]["value"],
        serde_json::json!({ "redacted": true }),
        "secret-classified keys mask on every read surface, echoes included"
    );

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(
        config.drivers["wled"].settings["known_ips"],
        serde_json::json!(["192.168.1.50"])
    );
}

#[tokio::test]
async fn config_set_driver_registry_key_rejects_non_routable_ip() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(config_put_request(
            "drivers.wled.known_ips",
            &serde_json::json!(["127.0.0.1"]),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    let message = json["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(message.contains("drivers.wled"));
    assert!(message.contains("driver validation"));
    assert!(
        !message.contains("127.0.0.1"),
        "a secret-classified key must not echo the value it refused: {message}"
    );
    assert!(
        !config_path.exists(),
        "invalid driver config should not be persisted"
    );
}

/// A rejected write must not hand the submitted value back.
///
/// Serde quotes the value it refused, so a wrong-typed write to a
/// secret-classified key would put a credential in the error body and
/// in whatever logs it.
#[tokio::test]
async fn config_write_rejection_does_not_echo_a_secret_value() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let secret = "sk-live-do-not-echo-me";
    let response = app
        .oneshot(config_put_request(
            "drivers.wled.enabled",
            &serde_json::json!(secret),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    let message = json["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("drivers.wled.enabled"),
        "the caller still learns which key failed: {message}"
    );
    assert!(
        !message.contains(secret),
        "a secret-classified key must not echo the value it refused: {message}"
    );
    assert!(
        !serde_json::to_string(&json)
            .expect("error body should serialize")
            .contains(secret),
        "the value must not survive anywhere in the error body"
    );
}

/// Plain keys keep the detail that makes a rejection actionable.
#[tokio::test]
async fn config_write_rejection_keeps_detail_for_a_plain_key() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(config_put_request(
            "daemon.target_fps",
            &serde_json::json!("not-a-number"),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    let message = json["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(message.contains("daemon.target_fps"));
    assert!(
        message.contains("invalid type"),
        "a plain key keeps the serde detail: {message}"
    );
}

#[tokio::test]
async fn config_set_rejects_invalid_capture_boundaries_before_persistence() {
    let (state, manager, _tempdir) = test_state_with_temp_config_manager();

    for (key, value) in [
        ("capture.capture_fps", "0"),
        ("capture.grid_cols", "0"),
        ("capture.smoothing", "1.1"),
        ("capture.gamma", "nan"),
    ] {
        let response = execute_trusted_config_request(
            &state,
            config_put_request(key, &config_test_value(value), None),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "validation_error");
    }

    assert!(
        !manager.path().exists(),
        "invalid capture config must not reach persistent storage"
    );
    assert!(!state.input_manager().has_screen_source());
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn config_set_rejects_capture_resource_plan_before_persistence() {
    let (state, manager, _tempdir) = test_state_with_temp_config_manager();

    let response = execute_trusted_config_request(
        &state,
        config_put_request(
            "capture.publication_memory_bytes",
            &serde_json::json!(1),
            None,
        ),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert!(!manager.path().exists());
    assert!(!state.input_manager().has_screen_source());
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn config_set_applies_windows_capture_settings_source_and_disable_live() {
    let (state, manager, _tempdir) = test_state_with_temp_config_manager();

    let source = r"monitor:\\?\DISPLAY#TEST#stable";
    for (key, value) in [
        ("capture.grid_cols", "9".to_owned()),
        (
            "capture.source",
            serde_json::to_string(source).expect("source should encode as JSON"),
        ),
    ] {
        let response = execute_trusted_config_request(
            &state,
            config_put_request(key, &config_test_value(&value), None),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["data"]["live"], true);
        assert!(state.input_manager().has_screen_source());
    }

    let response = execute_trusted_config_request(
        &state,
        config_put_request("capture.enabled", &serde_json::json!(false), None),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["live"], true);
    assert!(!state.input_manager().has_screen_source());

    let persisted = fs::read_to_string(manager.path()).expect("capture config should persist");
    let persisted: HypercolorConfig =
        toml::from_str(&persisted).expect("persisted capture config should parse");
    assert_eq!(persisted.capture.grid_cols, 9);
    assert_eq!(persisted.capture.source, source);
    assert!(!persisted.capture.enabled);
}

#[tokio::test]
async fn config_set_audio_device_rebuilds_live_input_manager_when_requested() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let state = Arc::new(state);

    let response = execute_trusted_config_request(
        &state,
        config_put_request("audio.device", &serde_json::json!("microphone"), Some(true)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "microphone");
    assert_eq!(json["data"]["live"], true);

    {
        let input_manager = state.input_manager();
        assert_eq!(
            input_manager.source_count(),
            1,
            "the rebuilt audio source is the only sampled source"
        );
        assert!(
            input_manager
                .source_names()
                .iter()
                .any(|name| name == "AudioInput(microphone)"),
            "rebuilt input manager should include the selected audio source"
        );
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.audio.device, "microphone");
}

#[tokio::test]
async fn config_set_legacy_audio_alias_persists_canonical_device_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let state = Arc::new(state);

    let response = execute_trusted_config_request(
        &state,
        config_put_request("audio.device", &serde_json::json!("mic"), Some(true)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "microphone");
    assert_eq!(json["data"]["live"], true);

    {
        let input_manager = state.input_manager();
        assert!(
            input_manager
                .source_names()
                .iter()
                .any(|name| name == "AudioInput(microphone)"),
            "legacy alias should canonicalize before rebuilding the audio input"
        );
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.audio.device, "microphone");
}

#[tokio::test]
async fn config_set_legacy_audio_alias_skips_live_rebuild_when_already_canonical() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let state = Arc::new(state);

    let response = execute_trusted_config_request(
        &state,
        config_put_request("audio.device", &serde_json::json!("auto"), Some(true)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "default");
    assert_eq!(json["data"]["live"], false);

    {
        let input_manager = state.input_manager();
        assert_eq!(
            input_manager.source_count(),
            0,
            "the direct browser registry stays outside the input manager"
        );
    }

    assert!(
        !config_path.exists(),
        "alias no-op should not persist a fresh config file"
    );
}

#[tokio::test]
async fn config_set_identical_audio_value_skips_live_rebuild() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let state = Arc::new(state);

    let response = execute_trusted_config_request(
        &state,
        config_put_request("audio.device", &serde_json::json!("default"), Some(true)),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "audio.device");
    assert_eq!(json["data"]["value"], "default");
    assert_eq!(json["data"]["live"], false);

    {
        let input_manager = state.input_manager();
        assert_eq!(
            input_manager.source_count(),
            0,
            "the direct browser registry stays outside the input manager"
        );
    }

    assert!(
        !config_path.exists(),
        "no-op config writes should not persist a fresh config file"
    );
}

#[tokio::test]
async fn config_set_render_canvas_updates_active_layout_dimensions() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);

    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));
    let mut applied_layouts = Vec::new();

    for (key, value) in [
        ("daemon.canvas_width", "1024"),
        ("daemon.canvas_height", "768"),
    ] {
        let (response, applied) = request_with_layout_ack(
            app.clone(),
            config_put_request(key, &config_test_value(value), None),
            &state,
        )
        .await;
        applied_layouts.extend(applied);

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["data"]["key"], key);
        assert_eq!(json["data"]["live"], true);
    }

    {
        let spatial = state.spatial_engine.snapshot();
        assert_eq!(spatial.layout().canvas_width, 1024);
        assert_eq!(spatial.layout().canvas_height, 768);
    }

    {
        let saved = state
            .domains
            .layout
            .resolve("default")
            .await
            .expect("active layout should remain persisted");
        assert_eq!(saved.canvas_width, 1024);
        assert_eq!(saved.canvas_height, 768);
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.daemon.canvas_width, 1024);
    assert_eq!(config.daemon.canvas_height, 768);

    assert_eq!(applied_layouts.len(), 2);
    assert!(applied_layouts.iter().any(|layout| {
        layout.id == "default" && layout.canvas_width == 1024 && layout.canvas_height == 768
    }));
    assert!(
        applied_layouts
            .iter()
            .any(|layout| layout.canvas_width == 1024)
    );
}

#[tokio::test]
async fn config_set_render_target_fps_updates_render_loop_live() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));

    let state = isolated_state_with_config_manager(config_manager);
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(config_put_request(
            "daemon.target_fps",
            &serde_json::json!(45),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "daemon.target_fps");
    assert_eq!(json["data"]["value"], 45);
    assert_eq!(json["data"]["live"], true);

    {
        let render_loop = state.render_loop.read().await;
        let stats = render_loop.stats();
        assert_eq!(stats.tier.fps(), 45);
        assert_eq!(stats.max_tier.fps(), 45);
    }

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let config: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(config.daemon.target_fps, 45);
}

/// A config carrying a retired top-level key, driver settings, a secret, and
/// a foreign section.
///
/// `acme_cloud` is deliberately not a registered driver module, so its
/// settings stand in for anything a driver may persist without the host
/// modelling the shape.
const RESET_FIXTURE_CONFIG: &str = r#"
schema_version = 5
include = ["desk-overrides.toml", "travel.toml"]

[daemon]
target_fps = 45

[audio]
device = "microphone"

[drivers.wled]
enabled = true
known_ips = ["192.168.1.50"]

[drivers.acme_cloud]
enabled = false
api_key = "sk-live-do-not-lose-me"
account = "bliss@example.com"

[cloud]
enabled = true
refresh_token = "rt-do-not-lose-me"
"#;

/// An extension document with nested tables and an array of tables.
///
/// Spec 76 §3.1 promises arbitrary extension documents survive a reset, so
/// the shape that exercises the serialization boundary gets its own fixture.
const RESET_NESTED_EXTENSION_CONFIG: &str = r#"
schema_version = 5

[telemetry]
enabled = true
endpoint = "https://telemetry.example.invalid/ingest"

[telemetry.retry]
backoff_ms = 250
max_attempts = 5

[[telemetry.rules]]
levels = ["error", "fatal"]
name = "errors"

[[telemetry.rules]]
levels = ["info"]
name = "audit"
"#;

/// A driver entry the registered WLED module rejects as invalid.
const RESET_INVALID_DRIVER_CONFIG: &str = r#"
schema_version = 5

[drivers.wled]
enabled = true
known_ips = ["127.0.0.1"]
"#;

fn reset_fixture_state(config_path: &Path) -> (Arc<AppState>, Arc<ConfigManager>) {
    reset_fixture_state_from(config_path, RESET_FIXTURE_CONFIG)
}

/// Build daemon state over a config manager seeded from an on-disk fixture.
///
/// The capacity plan matters on Windows, where capture defaults to enabled
/// and a keyless reset therefore rebuilds the screen graph.
fn reset_fixture_state_from(
    config_path: &Path,
    source: &str,
) -> (Arc<AppState>, Arc<ConfigManager>) {
    fs::write(config_path, source).expect("fixture config should be written");
    let config_manager = Arc::new(
        ConfigManager::new(config_path.to_path_buf()).expect("config manager should build"),
    );
    let state = isolated_state_with_config_manager(Arc::clone(&config_manager));
    {
        let input_manager = state.input_manager();
        let capacity = input_manager.screen_resource_capacity();
        input_manager
            .set_screen_capacity_plan(capacity, capacity, capacity)
            .expect("isolated input manager should accept its default capacity");
    }
    (Arc::new(state), config_manager)
}

fn reset_fixture_app(config_path: &Path) -> (axum::Router, Arc<AppState>, Arc<ConfigManager>) {
    let (state, config_manager) = reset_fixture_state(config_path);
    (
        test_app_with_state(Arc::clone(&state)),
        state,
        config_manager,
    )
}

/// Drive a whole-config reset, standing in for the render loop.
///
/// A reset re-applies every live section now, and the render section
/// queues a canvas transaction that waits on a pipeline acknowledgment.
/// The test state's layout starts at 320x200 against a 640x480 config
/// default, so the reset genuinely resizes and needs the ack pump.
async fn post_config_reset(state: &Arc<AppState>) -> axum::response::Response {
    trusted_request_with_layout_ack(
        Request::builder()
            .method("POST")
            .uri("/api/v1/config/reset")
            .body(Body::empty())
            .expect("failed to build request"),
        state,
    )
    .await
    .0
}

async fn delete_config_key(app: axum::Router, key: &str) -> axum::response::Response {
    app.oneshot(config_delete_request(key))
        .await
        .expect("failed to execute request")
}

#[tokio::test]
async fn config_full_reset_preserves_driver_settings_and_seeds_builtin_entries() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (state, config_manager) = reset_fixture_state(&config_path);

    let response = post_config_reset(&state).await;
    assert_eq!(response.status(), StatusCode::OK);

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let saved: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");

    let acme = saved
        .drivers
        .get("acme_cloud")
        .expect("a full reset must not destroy driver entries");
    assert_eq!(
        acme.settings["api_key"],
        serde_json::json!("sk-live-do-not-lose-me"),
        "a full reset must not destroy driver credentials"
    );
    assert_eq!(
        acme.settings["account"],
        serde_json::json!("bliss@example.com")
    );
    assert!(
        !acme.enabled,
        "a driver's enable flag is part of its preserved entry"
    );
    assert_eq!(
        saved.drivers["wled"].settings["known_ips"],
        serde_json::json!(["192.168.1.50"])
    );

    for driver_id in hypercolor_daemon::startup::default_config().drivers.keys() {
        assert!(
            saved.drivers.contains_key(driver_id),
            "reset must seed builtin driver entry {driver_id} like the load path does"
        );
    }

    assert_eq!(
        saved.daemon.target_fps,
        HypercolorConfig::default().daemon.target_fps,
        "sections the daemon owns return to defaults"
    );
    assert_eq!(saved.audio.device, HypercolorConfig::default().audio.device);

    let live = config_manager.get();
    assert_eq!(
        live.drivers["acme_cloud"].settings["api_key"],
        serde_json::json!("sk-live-do-not-lose-me")
    );
    assert_eq!(
        live.daemon.target_fps,
        HypercolorConfig::default().daemon.target_fps
    );
}

#[tokio::test]
async fn config_full_reset_preserves_extension_sections_and_retired_keys() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (state, config_manager) = reset_fixture_state(&config_path);

    let response = post_config_reset(&state).await;
    assert_eq!(response.status(), StatusCode::OK);

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let saved: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");

    let cloud = saved
        .extensions
        .get("cloud")
        .expect("an extension section must survive a full reset");
    assert_eq!(
        cloud.get("refresh_token"),
        Some(&serde_json::json!("rt-do-not-lose-me"))
    );
    assert_eq!(cloud.get("enabled"), Some(&serde_json::json!(true)));
    assert_eq!(
        config_manager.get().extensions.get("cloud"),
        saved.extensions.get("cloud")
    );

    assert_eq!(
        saved.extensions.get("include"),
        Some(&serde_json::json!(["desk-overrides.toml", "travel.toml"])),
        "a retired top-level key names files only the user knows about"
    );
    assert_eq!(
        config_manager.get().extensions.get("include"),
        saved.extensions.get("include")
    );
}

#[tokio::test]
async fn config_keyed_reset_restores_one_key_and_leaves_the_rest_intact() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (app, _state, _config_manager) = reset_fixture_app(&config_path);

    let response = delete_config_key(app, "daemon.target_fps").await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "daemon.target_fps");
    assert_eq!(
        json["data"]["value"],
        serde_json::json!(HypercolorConfig::default().daemon.target_fps)
    );
    assert_eq!(json["data"]["requires_restart"], false);

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let saved: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");

    assert_eq!(
        saved.daemon.target_fps,
        HypercolorConfig::default().daemon.target_fps
    );
    assert_eq!(
        saved.audio.device, "microphone",
        "a keyed reset leaves untargeted sections alone"
    );
    assert_eq!(
        saved.drivers["acme_cloud"].settings["api_key"],
        serde_json::json!("sk-live-do-not-lose-me")
    );
    assert_eq!(
        saved
            .extensions
            .get("cloud")
            .and_then(|section| section.get("refresh_token")),
        Some(&serde_json::json!("rt-do-not-lose-me"))
    );
    assert_eq!(
        saved.extensions.get("include"),
        Some(&serde_json::json!(["desk-overrides.toml", "travel.toml"]))
    );
}

#[tokio::test]
async fn config_reset_rejects_an_unknown_key() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (app, _state, _config_manager) = reset_fixture_app(&config_path);

    let response = delete_config_key(app, "daemon.not_a_real_key").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn config_schema_route_serves_the_key_registry() {
    let app = test_app_with_state(Arc::new(isolated_state()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/config/schema")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let entries = json["data"]
        .as_array()
        .expect("the schema is served as a list of entries");
    assert_eq!(
        entries.len(),
        hypercolor_types::config_registry::schema_entries().len()
    );

    let render = entries
        .iter()
        .find(|entry| entry["pattern"] == "daemon.target_fps")
        .expect("the render override is published");
    assert_eq!(render["apply"]["kind"], "live");
    assert_eq!(render["apply"]["section"], "render");
    assert_eq!(render["redaction"], "plain");

    let drivers = entries
        .iter()
        .find(|entry| entry["pattern"] == "drivers.*")
        .expect("the dynamic driver namespace is published");
    assert_eq!(drivers["redaction"], "secret");

    let capture = entries
        .iter()
        .find(|entry| entry["pattern"] == "capture")
        .expect("the capture section is published");
    assert_eq!(capture["has_validator"], true);
}

#[tokio::test]
async fn config_read_masks_secret_namespaces_and_keeps_plain_sections() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (app, _state, _config_manager) = reset_fixture_app(&config_path);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/config")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["daemon"]["target_fps"], 45);
    assert_eq!(json["data"]["audio"]["device"], "microphone");
    assert_eq!(
        json["data"]["drivers"]["acme_cloud"],
        serde_json::json!({ "redacted": true }),
        "driver entries carry credentials, so the generic read masks them"
    );
    assert_eq!(
        json["data"]["drivers"]["wled"],
        serde_json::json!({ "redacted": true })
    );
    assert_eq!(
        json["data"]["cloud"],
        serde_json::json!({ "redacted": true }),
        "unmodeled extension sections are deny-by-default"
    );
}

#[tokio::test]
async fn config_key_read_answers_one_key_and_masks_the_secret_ones() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (app, _state, _config_manager) = reset_fixture_app(&config_path);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/config/keys/daemon.target_fps")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["key"], "daemon.target_fps");
    assert_eq!(json["data"]["value"], 45);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/config/keys/drivers.acme_cloud.api_key")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["value"],
        serde_json::json!({ "redacted": true })
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/config/keys/daemon.not_a_real_key")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn config_key_routes_reject_a_malformed_key() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (app, _state, _config_manager) = reset_fixture_app(&config_path);

    for request in [
        Request::builder()
            .uri("/api/v1/config/keys/daemon..target_fps")
            .body(Body::empty())
            .expect("failed to build request"),
        config_put_request("daemon..target_fps", &serde_json::json!(45), None),
        config_delete_request("daemon..target_fps"),
    ] {
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("failed to execute request");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "malformed_request");
    }
}

#[tokio::test]
async fn config_write_reports_restart_classification_and_pending_restart() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    fs::write(&config_path, RESET_FIXTURE_CONFIG).expect("fixture config should be written");
    let loaded = ConfigManager::load_with_sources(hypercolor_core::config::ConfigSources {
        file: Some(config_path.clone()),
        ..hypercolor_core::config::ConfigSources::default_path()
    })
    .expect("fixture config should load");
    let state = isolated_state_with_config_manager(Arc::new(loaded.manager));
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .clone()
        .oneshot(config_put_request(
            "daemon.port",
            &serde_json::json!(9430),
            None,
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["value"], 9430);
    assert_eq!(
        json["data"]["live"], false,
        "a boot-frozen key has no subsystem to re-apply"
    );
    assert_eq!(json["data"]["requires_restart"], true);
    assert_eq!(
        json["data"]["pending_restart"],
        serde_json::json!(["daemon"]),
        "the persisted daemon section now differs from the booted one"
    );

    let response = app
        .oneshot(config_put_request(
            "session.on_suspend",
            &serde_json::json!("dim"),
            None,
        ))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["requires_restart"], false,
        "a read-fresh key takes effect without a restart"
    );
}

#[tokio::test]
async fn config_write_declining_live_persists_without_re_applying() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let config_manager =
        Arc::new(ConfigManager::new(config_path.clone()).expect("config manager should build"));
    let state = isolated_state_with_config_manager(Arc::clone(&config_manager));
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(config_put_request(
            "daemon.target_fps",
            &serde_json::json!(20),
            Some(false),
        ))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["live"], false);
    assert_eq!(config_manager.get().daemon.target_fps, 20);
    assert_ne!(
        state.render_loop.read().await.stats().max_tier.fps(),
        20,
        "declining the live apply leaves the running loop alone"
    );
}

/// A whole-config reset re-applies every live section, render included.
///
/// The hand predicate this replaced matched three exact keys and ignored
/// the whole-config case, so a reset persisted a new target FPS and left
/// the render loop running at the old one until the next restart.
#[tokio::test]
async fn config_full_reset_retunes_the_render_loop() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (state, config_manager) = reset_fixture_state(&config_path);
    let app = test_app_with_state(Arc::clone(&state));

    // Retune the running loop away from the default first. The fixture
    // already carries the config's 45, so the write has to name a
    // different tier or the unchanged-value short circuit skips it.
    let response = app
        .clone()
        .oneshot(config_put_request(
            "daemon.target_fps",
            &serde_json::json!(20),
            None,
        ))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.render_loop.read().await.stats().max_tier.fps(), 20);

    let response = post_config_reset(&state).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["live"], true);

    let default_fps = HypercolorConfig::default().daemon.target_fps;
    assert_eq!(config_manager.get().daemon.target_fps, default_fps);
    let expected_tier = hypercolor_core::engine::FpsTier::from_fps(default_fps);
    let stats = state.render_loop.read().await.stats();
    assert_eq!(stats.max_tier, expected_tier);
    assert_eq!(stats.tier, expected_tier);
}

#[tokio::test]
async fn config_full_reset_round_trips_a_nested_extension_document() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (state, config_manager) =
        reset_fixture_state_from(&config_path, RESET_NESTED_EXTENSION_CONFIG);
    let authored: HypercolorConfig =
        toml::from_str(RESET_NESTED_EXTENSION_CONFIG).expect("fixture should parse");
    let authored_telemetry = authored
        .extensions
        .get("telemetry")
        .expect("the nested fixture section lands in the catch-all");

    let response = post_config_reset(&state).await;
    assert_eq!(response.status(), StatusCode::OK);

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let saved: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    let saved_telemetry = saved
        .extensions
        .get("telemetry")
        .expect("a nested extension document must survive a full reset");

    assert_eq!(
        saved_telemetry, authored_telemetry,
        "the whole document round-trips, sub-tables and array-of-tables included"
    );
    assert_eq!(
        saved_telemetry
            .get("retry")
            .and_then(|retry| retry.get("max_attempts")),
        Some(&serde_json::json!(5)),
        "a sub-table keeps its values"
    );
    let rules = saved_telemetry
        .get("rules")
        .and_then(serde_json::Value::as_array)
        .expect("the array of tables survives as an array");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].get("name"), Some(&serde_json::json!("errors")));
    assert_eq!(
        rules[1].get("levels"),
        Some(&serde_json::json!(["info"])),
        "nested arrays inside an array of tables survive"
    );
    assert_eq!(
        config_manager.get().extensions.get("telemetry"),
        Some(saved_telemetry)
    );
}

#[tokio::test]
async fn config_full_reset_event_carries_no_config_payload() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (state, _config_manager) = reset_fixture_state(&config_path);
    let mut events = state.event_bus.subscribe_all();

    let response = post_config_reset(&state).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (key, new_value) = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ConfigChanged { key, new_value, .. } = timestamped.event
                    {
                        break (key, new_value);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before the reset event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for the config reset event");

    assert_eq!(key, "", "a whole-config reset publishes the empty key");
    assert_eq!(
        new_value,
        serde_json::Value::Null,
        "the payload stays empty: preserved driver and extension sections hold \
         credentials, and this event reaches every ws events subscriber"
    );
}

#[tokio::test]
async fn config_full_reset_is_not_blocked_by_an_invalid_driver_entry() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_path = tempdir.path().join("hypercolor.toml");
    let (state, config_manager) =
        reset_fixture_state_from(&config_path, RESET_INVALID_DRIVER_CONFIG);
    let app = test_app_with_state(Arc::clone(&state));

    // Writing a loopback address through `set` is rejected, so the seeded
    // entry is genuinely one the driver refuses rather than an inert
    // payload. The address differs from the seeded one because `set`
    // short-circuits an unchanged value before it reaches validation.
    let rejected = app
        .clone()
        .oneshot(config_put_request(
            "drivers.wled.known_ips",
            &serde_json::json!(["127.0.0.2"]),
            None,
        ))
        .await
        .expect("failed to execute request");
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = post_config_reset(&state).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "existing invalid driver config must never lock a user out of a reset"
    );

    let config_raw = fs::read_to_string(&config_path).expect("config file should be written");
    let saved: HypercolorConfig =
        toml::from_str(&config_raw).expect("saved config should deserialize");
    assert_eq!(
        saved.drivers["wled"].settings["known_ips"],
        serde_json::json!(["127.0.0.1"]),
        "the reset carries the entry through untouched rather than repairing it by deletion"
    );
    assert_eq!(
        config_manager.get().drivers["wled"].settings["known_ips"],
        serde_json::json!(["127.0.0.1"])
    );
}

async fn insert_test_effect(state: &Arc<AppState>, name: &str) {
    let _ = insert_test_effect_with_presets(state, name, Vec::new()).await;
}

async fn insert_test_effect_with_presets(
    state: &Arc<AppState>,
    name: &str,
    presets: Vec<PresetTemplate>,
) -> EffectMetadata {
    insert_test_effect_with_controls(
        state,
        name,
        vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(5.0),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(0.5),
            labels: Vec::new(),
            group: Some("General".to_owned()),
            tooltip: Some("Animation speed".to_owned()),
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets,
    )
    .await
}

async fn insert_test_effect_with_controls(
    state: &Arc<AppState>,
    name: &str,
    controls: Vec<ControlDefinition>,
    presets: Vec<PresetTemplate>,
) -> EffectMetadata {
    let metadata = EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} description"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned()],
        controls,
        presets,
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: format!("builtin/{name}").into(),
        },
        license: None,
    };
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.domains.effects.register(entry).await;
    metadata
}

fn test_html_effect_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} html effect"),
        category: EffectCategory::Ambient,
        tags: vec!["test".to_owned(), "html".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Html {
            path: format!("/tmp/{name}.html").into(),
        },
        license: None,
    }
}

async fn insert_input_reactive_test_effect(state: &Arc<AppState>, name: &str) {
    let mut metadata = test_html_effect_metadata(name);
    metadata.tags = vec!["test".to_owned()];
    metadata.input_reactive = true;
    let entry = EffectEntry {
        metadata,
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.domains.effects.register(entry).await;
}

fn test_display_face_effect_metadata(name: &str) -> EffectMetadata {
    let mut metadata = test_html_effect_metadata(name);
    metadata.category = EffectCategory::Display;
    metadata
}

async fn insert_test_display_face_effect(state: &Arc<AppState>, name: &str) -> EffectMetadata {
    let metadata = test_display_face_effect_metadata(name);
    let entry = EffectEntry {
        metadata: metadata.clone(),
        source_path: format!("/tmp/{name}.html").into(),
        modified: SystemTime::now(),
        state: EffectState::Loading,
    };
    let _ = state.domains.effects.register(entry).await;
    metadata
}

#[tokio::test]
async fn install_effect_upload_writes_file_and_registers_effect() {
    let (state, tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));
    let html = r#"<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="hypercolor-version" content="1" />
    <title>Aurora</title>
    <meta description="Northern lights" />
    <meta publisher="Hypercolor" />
    <meta property="speed" label="Speed" type="number" default="5" min="1" max="10" />
    <meta preset="Default" preset-controls='{"speed":5}' />
  </head>
  <body>
    <canvas id="exCanvas"></canvas>
    <script>console.log("ok")</script>
  </body>
</html>"#;

    let response = app
        .oneshot(multipart_upload_request("aurora.html", html))
        .await
        .expect("failed to execute upload request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Aurora");
    assert!(json["data"].get("source").is_none());
    assert_eq!(json["data"]["controls"], 1);
    assert_eq!(json["data"]["presets"], 1);

    let installed_path = tempdir.path().join("data/effects/user/aurora.html");
    assert!(
        installed_path.exists(),
        "expected uploaded effect to be written"
    );

    let effects = state.domains.effects.all_metadata().await;
    assert!(
        effects.iter().any(|metadata| metadata.name == "Aurora"),
        "installed effect should enter the domain catalog"
    );
}

#[tokio::test]
async fn install_effect_upload_requires_browser_file_part() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(state);
    let html = r#"<!DOCTYPE html><html><head><title>Aurora</title></head><body><canvas id="exCanvas"></canvas><script>1</script></body></html>"#;

    let response = app
        .oneshot(multipart_upload_request_without_filename(html))
        .await
        .expect("failed to execute upload request");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(
        json["error"]["message"],
        "Missing multipart file field named \"file\"."
    );
}

#[tokio::test]
async fn install_effect_upload_rejects_invalid_html() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(state);
    let html = r#"<html><body><canvas id="exCanvas"></canvas></body></html>"#;

    let response = app
        .oneshot(multipart_upload_request("broken.html", html))
        .await
        .expect("failed to execute upload request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    let errors = json["error"]["details"]["errors"]
        .as_array()
        .expect("validation errors should be present");
    assert!(errors.iter().any(|entry| entry == "Missing <title> tag"));
    assert!(errors.iter().any(|entry| entry == "Missing <script> tag"));
}

#[tokio::test]
async fn install_effect_upload_rejects_duplicate_preset_ids() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(state);
    let duplicate_cases = [
        (
            "fallback.html",
            r#"<meta preset="Calm" preset-controls='{}' />
<meta preset="Calm" preset-controls='{}' />"#,
        ),
        (
            "authored.html",
            r#"<meta preset="Calm" preset-id="shared" preset-controls='{}' />
<meta preset="Breeze" preset-id="shared" preset-controls='{}' />"#,
        ),
    ];

    for (file_name, presets) in duplicate_cases {
        let html = format!(
            r#"<!DOCTYPE html>
<html>
  <head><title>Duplicates</title>{presets}</head>
  <body><canvas id="exCanvas"></canvas><script>1</script></body>
</html>"#
        );
        let response = app
            .clone()
            .oneshot(multipart_upload_request(file_name, &html))
            .await
            .expect("failed to execute upload request");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "validation_error");
        let errors = json["error"]["details"]["errors"]
            .as_array()
            .expect("validation errors should be present");
        assert!(errors.iter().any(|entry| {
            entry
                .as_str()
                .is_some_and(|message| message.contains("Duplicate bundled preset id"))
        }));
    }
}

#[tokio::test]
async fn install_effect_upload_rejects_oversized_payloads() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(state);
    let script = "a".repeat((1024 * 1024) + 32);
    let html = format!(
        "<!DOCTYPE html><html><head><title>Huge</title></head><body><canvas id=\"exCanvas\"></canvas><script>{script}</script></body></html>"
    );

    let response = app
        .oneshot(multipart_upload_request("huge.html", &html))
        .await
        .expect("failed to execute upload request");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn install_effect_upload_updates_existing_file_in_place() {
    let (state, tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let app = test_app_with_state(Arc::clone(&state));
    let user_effects_dir = tempdir.path().join("data/effects/user");
    fs::create_dir_all(&user_effects_dir).expect("user effects dir should exist");
    let existing_path = user_effects_dir.join("aurora.html");
    fs::write(
        &existing_path,
        "<!DOCTYPE html><html><head><title>Aurora</title></head><body><canvas id=\"exCanvas\"></canvas><script>1</script></body></html>",
    )
    .expect("existing effect should be written");
    let html = r#"<!DOCTYPE html>
<html>
  <head>
    <title>Aurora</title>
    <meta description="Updated build" />
  </head>
  <body>
    <canvas id="exCanvas"></canvas>
    <script>console.log("updated")</script>
  </body>
</html>"#;

    let response = app
        .oneshot(multipart_upload_request("aurora.html", html))
        .await
        .expect("failed to execute upload request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    let installed_path = json["data"]["path"]
        .as_str()
        .expect("installed path should be present");
    assert!(
        installed_path.ends_with("aurora.html"),
        "same-stem uploads must update in place, got {installed_path}"
    );
    assert!(
        !user_effects_dir.join("aurora-2.html").exists(),
        "no -2 clone should appear beside the original"
    );
    let written = fs::read_to_string(&existing_path).expect("updated file should read");
    assert!(
        written.contains("updated"),
        "file content should be replaced"
    );

    // Path-derived id is stable, so the registry holds one updated entry.
    let aurora_entries = state
        .domains
        .effects
        .all_metadata()
        .await
        .iter()
        .filter(|metadata| metadata.name == "Aurora")
        .count();
    assert_eq!(aurora_entries, 1);
}

async fn activate_empty_test_scene(state: &Arc<AppState>, name: &str) -> SceneId {
    activate_empty_test_scene_with_mode(state, name, SceneMutationMode::Live).await
}

async fn activate_empty_test_scene_with_mode(
    state: &Arc<AppState>,
    name: &str,
    mutation_mode: SceneMutationMode,
) -> SceneId {
    let scene = Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        zones: Vec::new(),
        zones_revision: 0,
        transition: TransitionSpec {
            duration_ms: 0,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode,
    };

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene.clone())
        .expect("test scene should be created");
    mutation
        .activate(
            scene.id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("test scene should activate");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("test scene should commit");
    scene.id
}

async fn activate_display_face_test_scene(
    state: &Arc<AppState>,
    name: &str,
    effect_id: EffectId,
    device_id: DeviceId,
) -> SceneId {
    activate_display_face_test_scene_with_layers(state, name, effect_id, device_id, Vec::new())
        .await
}

async fn activate_display_face_test_scene_with_layers(
    state: &Arc<AppState>,
    name: &str,
    effect_id: EffectId,
    device_id: DeviceId,
    layers: Vec<SceneLayer>,
) -> SceneId {
    let layers = if layers.is_empty() {
        vec![SceneLayer::from_effect(
            SceneLayerId::new(),
            effect_id,
            HashMap::new(),
            HashMap::new(),
            None,
        )]
    } else {
        layers
    };
    let scene = Scene {
        id: SceneId::new(),
        name: name.to_owned(),
        description: None,
        zones: vec![Zone {
            id: hypercolor_types::scene::ZoneId::new(),
            name: "Display Face".to_owned(),
            description: None,
            layers,
            layout: SpatialLayout {
                id: "display-face-layout".to_owned(),
                name: "Display Face Layout".to_owned(),
                description: None,
                canvas_width: 320,
                canvas_height: 320,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: Some(DisplayFaceTarget::new(device_id)),
            role: ZoneRole::Display,
            controls_version: 0,
            layers_version: 0,
        }],
        zones_revision: 0,
        transition: TransitionSpec {
            duration_ms: 0,
            easing: EasingFunction::Linear,
            color_interpolation: ColorInterpolation::Oklab,
        },
        priority: ScenePriority::USER,
        enabled: true,
        metadata: HashMap::new(),
        unassigned_behavior: UnassignedBehavior::Off,
        layout_id: None,
        activation_brightness: None,
        kind: SceneKind::Named,
        mutation_mode: SceneMutationMode::Live,
    };

    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation
        .create_scene(scene.clone())
        .expect("display face scene should be created");
    mutation
        .activate(
            scene.id,
            None,
            hypercolor_types::event::SceneChangeReason::UserActivate,
        )
        .expect("display face scene should activate");
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("display face scene should commit");
    scene.id
}

fn default_config_path() -> String {
    ConfigManager::config_dir()
        .join("hypercolor.toml")
        .display()
        .to_string()
}

async fn insert_test_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 60,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

async fn insert_test_display_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: Some("LCD".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("wled", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "LCD".to_owned(),
            led_count: 320 * 320,
            topology: DeviceTopologyHint::Display {
                width: 320,
                height: 320,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 320 * 320,
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((320, 320)),
            max_fps: 30,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    id
}

#[cfg(feature = "builtin-drivers")]
async fn insert_test_hue_bridge_device(
    state: &Arc<AppState>,
    name: &str,
    bridge_id: &str,
    ip: &str,
    api_port: u16,
) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "Philips Hue".to_owned(),
        family: DeviceFamily::new_static("hue", "Philips Hue"),
        model: Some("Bridge".to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("hue", "hue", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Bridge".to_owned(),
            led_count: 1,
            topology: DeviceTopologyHint::Point,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("1.0.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 1,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint::from_persisted(format!("hue:{bridge_id}"));
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("bridge_id".to_owned(), bridge_id.to_owned());
    metadata.insert("ip".to_owned(), ip.to_owned());
    metadata.insert("api_port".to_owned(), api_port.to_string());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
}

#[cfg(feature = "builtin-drivers")]
async fn insert_test_nanoleaf_device(
    state: &Arc<AppState>,
    name: &str,
    device_key: &str,
    ip: &str,
    api_port: u16,
) -> DeviceId {
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "Nanoleaf".to_owned(),
        family: DeviceFamily::new_static("nanoleaf", "Nanoleaf"),
        model: Some("Shapes".to_owned()),
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("nanoleaf", "nanoleaf", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Panel".to_owned(),
            led_count: 12,
            topology: DeviceTopologyHint::Matrix { rows: 3, cols: 4 },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("12.0.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 12,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint::from_persisted(format!("nanoleaf:{device_key}"));
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("device_key".to_owned(), device_key.to_owned());
    metadata.insert("ip".to_owned(), ip.to_owned());
    metadata.insert("api_port".to_owned(), api_port.to_string());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
}

async fn insert_test_asus_smbus_device(state: &Arc<AppState>, name: &str) -> DeviceId {
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: name.to_owned(),
        vendor: "ASUS".to_owned(),
        family: DeviceFamily::new_static("asus", "ASUS"),
        model: Some("ROG STRIX Test".to_owned()),
        connection_type: ConnectionType::SmBus,
        origin: DeviceOrigin::native("asus", "smbus", ConnectionType::SmBus)
            .with_protocol_id("asus/aura-smbus"),
        segments: vec![SegmentInfo {
            name: "GPU".to_owned(),
            led_count: 24,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("AUMA0-E6K5-0107".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 24,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint::from_persisted("smbus:/dev/i2c-9:40".to_owned());
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("smbus_address".to_owned(), "0x40".to_owned());
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
}

/// Set up a spatial layout with a zone targeting the given `layout_device_id`.
///
/// This ensures that `sync_active_layout_connectivity` won't disconnect the
/// device because the active layout has a zone referencing it.
async fn set_layout_targeting_device(
    state: &Arc<AppState>,
    layout_device_id: &str,
    led_count: u32,
) {
    let layout = SpatialLayout {
        id: "test-layout".into(),
        name: "Test Layout".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: "zone_main".into(),
            name: "Main".into(),
            device_id: layout_device_id.into(),
            zone_name: None,

            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 0.1),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Strip {
                count: led_count,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: None,
        }],

        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    let publisher = tokio::spawn(run_layout_publications(Arc::clone(state), 1));
    let response =
        api::layouts::preview_layout(axum::extract::State(Arc::clone(state)), axum::Json(layout))
            .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        publisher
            .await
            .expect("layout publication worker should not panic")
            .len(),
        1
    );
}

// ── Devices ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_devices_returns_empty_list() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("items should be an array");
    assert!(items.is_empty());
    assert_eq!(json["data"]["total"], 0);
}

#[tokio::test]
async fn list_drivers_returns_registered_module_descriptors() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("drivers response should include items");

    let wled = items
        .iter()
        .find(|item| item["descriptor"]["id"] == "wled")
        .expect("WLED module descriptor should be present");
    assert_eq!(wled["descriptor"]["module_kind"], "network");
    assert_eq!(
        wled["descriptor"]["transports"],
        serde_json::json!([{
            "kind": "network",
            "availability": { "status": "available" }
        }])
    );
    assert_eq!(wled["descriptor"]["capabilities"]["discovery"], true);
    assert_eq!(wled["descriptor"]["capabilities"]["output_backend"], true);
    assert_eq!(wled["descriptor"]["capabilities"]["controls"], true);
    assert_eq!(wled["descriptor"]["capabilities"]["presentation"], true);
    assert_eq!(wled["presentation"]["label"], "WLED");
    assert_eq!(
        wled["presentation"]["accent_rgb"],
        serde_json::json!([255, 106, 193])
    );
    assert_eq!(wled["presentation"]["default_device_class"], "controller");
    assert_eq!(wled["enabled"], true);
    assert_eq!(wled["config_key"], "drivers.wled");
    assert_eq!(wled["control_surface_id"], "driver:wled");
    assert_eq!(
        wled["control_surface_path"],
        "/api/v1/drivers/wled/controls"
    );

    let nollie = items
        .iter()
        .find(|item| item["descriptor"]["id"] == "nollie")
        .expect("Nollie HAL module descriptor should be present");
    assert_eq!(nollie["descriptor"]["module_kind"], "hal");
    assert_eq!(
        nollie["descriptor"]["transports"],
        serde_json::json!([
            {
                "kind": "usb",
                "availability": { "status": "available" }
            },
            {
                "kind": "serial",
                "availability": { "status": "available" }
            }
        ])
    );
    assert_eq!(
        nollie["descriptor"]["capabilities"]["protocol_catalog"],
        true
    );
    assert_eq!(
        nollie["descriptor"]["capabilities"]["output_backend"],
        false
    );
    assert_eq!(nollie["descriptor"]["capabilities"]["controls"], false);
    assert_eq!(nollie["presentation"]["label"], "Nollie");
    assert_eq!(nollie["enabled"], true);
    assert_eq!(nollie["config_key"], "drivers.nollie");
    let nollie_protocols = nollie["protocols"]
        .as_array()
        .expect("Nollie HAL module should include protocols");
    assert!(nollie_protocols.iter().any(|protocol| {
        protocol["protocol_id"] == "nollie/nollie-8-v2"
            && protocol["transport"] == "usb"
            && protocol["route_backend_id"] == "usb"
    }));
    assert!(nollie.get("control_surface_id").is_none());
    assert!(nollie.get("control_surface_path").is_none());
}

#[tokio::test]
async fn get_driver_config_returns_current_and_default_entries() {
    let tempdir = tempfile::tempdir().expect("tempdir should build");
    let config_manager = Arc::new(
        ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("config manager should build"),
    );
    let mut config = HypercolorConfig::default();
    config.drivers.insert(
        "wled".to_owned(),
        DriverConfigEntry::enabled(BTreeMap::from([(
            "known_ips".to_owned(),
            serde_json::json!(["192.168.1.50"]),
        )])),
    );
    config_manager.update(config);

    let state = isolated_state_with_config_manager(config_manager);
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/wled/config")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let data = &json["data"];
    assert_eq!(data["driver_id"], "wled");
    assert_eq!(data["config_key"], "drivers.wled");
    assert_eq!(data["configurable"], true);
    assert_eq!(data["current"]["enabled"], true);
    assert_eq!(
        data["current"]["known_ips"],
        serde_json::json!(["192.168.1.50"])
    );
    assert_eq!(data["default"]["enabled"], true);
    assert_eq!(data["default"]["known_ips"], serde_json::json!([]));
    assert_eq!(data["default"]["default_protocol"], "ddp");
}

#[tokio::test]
async fn get_driver_config_handles_non_configurable_modules() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/nollie/config")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let data = &json["data"];
    assert_eq!(data["driver_id"], "nollie");
    assert_eq!(data["config_key"], "drivers.nollie");
    assert_eq!(data["configurable"], false);
    assert_eq!(data["current"]["enabled"], true);
    assert!(data.get("default").is_none());
}

#[tokio::test]
async fn get_unknown_driver_config_returns_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/missing/config")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_driver_controls_returns_module_control_surface() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/wled/controls")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let data = &json["data"];
    assert_eq!(data["surface_id"], "driver:wled");
    assert_eq!(data["schema_version"], 1);
    assert_eq!(data["scope"]["driver"]["driver_id"], "wled");
    assert_eq!(data["values"]["known_ips"]["kind"], "list");
    assert_eq!(data["values"]["known_ips"]["value"], serde_json::json!([]));
    assert_eq!(data["values"]["default_protocol"]["kind"], "enum");
    assert_eq!(data["values"]["default_protocol"]["value"], "ddp");
    assert_eq!(data["values"]["realtime_http_enabled"]["kind"], "bool");
    assert_eq!(data["values"]["realtime_http_enabled"]["value"], true);
    assert_eq!(data["values"]["dedup_threshold"]["kind"], "int");
    assert_eq!(data["values"]["dedup_threshold"]["value"], 2);

    let fields = data["fields"]
        .as_array()
        .expect("fields should be an array");
    assert!(fields.iter().any(|field| field["id"] == "known_ips"));
    assert!(fields.iter().any(|field| field["id"] == "default_protocol"));
    assert!(
        fields
            .iter()
            .any(|field| field["id"] == "realtime_http_enabled")
    );
    assert!(fields.iter().any(|field| field["id"] == "dedup_threshold"));
}

#[tokio::test]
async fn get_driver_controls_returns_govee_hue_and_nanoleaf_surfaces() {
    let app = test_app();

    let govee_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/govee/controls")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(govee_response.status(), StatusCode::OK);
    let govee = body_json(govee_response).await;
    assert_eq!(govee["data"]["surface_id"], "driver:govee");
    assert_eq!(govee["data"]["values"]["known_ips"]["kind"], "list");
    assert_eq!(
        govee["data"]["values"]["power_off_on_disconnect"]["kind"],
        "bool"
    );
    let govee_fields = govee["data"]["fields"]
        .as_array()
        .expect("Govee fields should be an array");
    assert!(govee_fields.iter().any(|field| {
        field["id"] == "known_ips" && field["apply_impact"] == "discovery_rescan"
    }));
    assert!(govee_fields.iter().any(|field| {
        field["id"] == "lan_state_fps" && field["apply_impact"] == "backend_rebind"
    }));

    let hue_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/hue/controls")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(hue_response.status(), StatusCode::OK);
    let hue = body_json(hue_response).await;
    assert_eq!(hue["data"]["surface_id"], "driver:hue");
    assert_eq!(hue["data"]["values"]["bridge_ips"]["kind"], "list");
    assert_eq!(hue["data"]["values"]["use_cie_xy"]["kind"], "bool");
    let hue_fields = hue["data"]["fields"]
        .as_array()
        .expect("Hue fields should be an array");
    assert!(hue_fields.iter().any(|field| {
        field["id"] == "bridge_ips" && field["apply_impact"] == "discovery_rescan"
    }));
    assert!(
        hue_fields
            .iter()
            .any(|field| field["id"] == "use_cie_xy" && field["apply_impact"] == "backend_rebind")
    );

    let nanoleaf_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/nanoleaf/controls")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(nanoleaf_response.status(), StatusCode::OK);
    let nanoleaf = body_json(nanoleaf_response).await;
    assert_eq!(nanoleaf["data"]["surface_id"], "driver:nanoleaf");
    assert_eq!(nanoleaf["data"]["values"]["device_ips"]["kind"], "list");
    assert_eq!(nanoleaf["data"]["values"]["transition_time"]["kind"], "int");
    let nanoleaf_fields = nanoleaf["data"]["fields"]
        .as_array()
        .expect("Nanoleaf fields should be an array");
    assert!(nanoleaf_fields.iter().any(|field| {
        field["id"] == "device_ips" && field["apply_impact"] == "discovery_rescan"
    }));
    assert!(nanoleaf_fields.iter().any(|field| {
        field["id"] == "transition_time" && field["apply_impact"] == "backend_rebind"
    }));
}

#[tokio::test]
async fn get_unknown_driver_controls_returns_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/drivers/missing/controls")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_control_surfaces_batches_device_and_driver_surfaces() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/control-surfaces?device_id={device_id}&include_driver=true"
                ))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let surfaces = json["data"]["surfaces"]
        .as_array()
        .expect("surfaces should be an array");
    assert_eq!(surfaces.len(), 3);
    assert!(surfaces.iter().any(|surface| {
        surface["surface_id"] == format!("device:{device_id}")
            && surface["scope"]["device"]["driver_id"] == "wled"
    }));
    assert!(surfaces.iter().any(|surface| {
        surface["surface_id"] == format!("driver:wled:device:{device_id}")
            && surface["scope"]["device"]["driver_id"] == "wled"
            && surface["values"]["led_count"]["value"] == 60
    }));
    assert!(surfaces.iter().any(|surface| {
        surface["surface_id"] == "driver:wled" && surface["scope"]["driver"]["driver_id"] == "wled"
    }));
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn list_control_surfaces_preserves_driver_action_confirmation() {
    let state = Arc::new(isolated_state());
    let device_id =
        insert_test_nanoleaf_device(&state, "Living Room Shapes", "serial42", "10.0.0.8", 16021)
            .await;
    let app = test_app_with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/control-surfaces?device_id={device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let surfaces = json["data"]["surfaces"]
        .as_array()
        .expect("surfaces should be an array");
    let nanoleaf = surfaces
        .iter()
        .find(|surface| surface["surface_id"] == format!("driver:nanoleaf:device:{device_id}"))
        .expect("Nanoleaf device surface should be returned");
    let refresh = nanoleaf["actions"]
        .as_array()
        .expect("actions should be an array")
        .iter()
        .find(|action| action["id"] == "refresh_topology")
        .expect("refresh topology action should be exposed");
    assert_eq!(refresh["confirmation"]["level"], "normal");
    assert!(
        refresh["confirmation"]["message"]
            .as_str()
            .expect("confirmation message should be a string")
            .contains("reconnect this Nanoleaf device")
    );
}

#[tokio::test]
async fn get_control_surface_returns_driver_owned_device_surface_by_id() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let settings_key = hypercolor_daemon::device_settings::resolve_device_settings_key(
        &state.device_registry,
        &state.device_settings,
        device_id,
    )
    .await;
    state
        .device_settings
        .persist_driver_control_values(
            &settings_key,
            ControlValueMap::from([
                (
                    "protocol".to_owned(),
                    SurfaceControlValue::Text("e131".to_owned()),
                ),
                ("dedup_threshold".to_owned(), SurfaceControlValue::Int(8)),
            ]),
        )
        .await
        .expect("driver controls should canonicalize");
    let app = test_app_with_state(state);
    let surface_id = format!("driver:wled:device:{device_id}");

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/control-surfaces/{surface_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], surface_id);
    assert_eq!(json["data"]["scope"]["device"]["driver_id"], "wled");
    assert_eq!(json["data"]["values"]["protocol"]["kind"], "enum");
    assert_eq!(json["data"]["values"]["protocol"]["value"], "e131");
    assert!(json["data"]["values"]["dedup_threshold"].is_null());
    assert_eq!(json["data"]["values"]["led_count"]["value"], 60);
}

#[tokio::test]
async fn patch_driver_owned_device_control_surface_persists_values() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));
    let surface_id = format!("driver:wled:device:{device_id}");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/control-surfaces/{surface_id}/values"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "protocol": { "kind": "enum", "value": "e131" }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], surface_id);
    assert_eq!(json["data"]["values"]["protocol"]["value"], "e131");
    assert_eq!(
        json["data"]["impacts"],
        serde_json::json!(["device_reconnect"])
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/control-surfaces?device_id={device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let surfaces = json["data"]["surfaces"]
        .as_array()
        .expect("surfaces should be an array");
    let driver_device_surface = surfaces
        .iter()
        .find(|surface| surface["surface_id"] == surface_id)
        .expect("driver-owned device surface should be present");
    assert_eq!(driver_device_surface["values"]["protocol"]["value"], "e131");
    assert!(driver_device_surface["values"]["dedup_threshold"].is_null());

    let raw = fs::read_to_string(state.state_dir.join("device-settings.json"))
        .expect("device settings should be persisted");
    let saved: serde_json::Value =
        serde_json::from_str(&raw).expect("device settings should be valid JSON");
    assert!(saved["driver_controls"].as_object().is_some_and(|values| {
        values
            .values()
            .any(|entry| entry["protocol"]["value"] == "e131")
    }));
}

#[tokio::test]
async fn patch_driver_owned_device_control_surface_publishes_values_changed_event() {
    let (state, _tmp) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));
    let surface_id = format!("driver:wled:device:{device_id}");

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/control-surfaces/{surface_id}/values"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "protocol": { "kind": "enum", "value": "e131" }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let updated_revision = json["data"]["revision"]
        .as_u64()
        .expect("updated revision should be an integer");

    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ControlSurfaceChanged(
                        event @ ControlSurfaceEvent::ValuesChanged { .. },
                    ) = timestamped.event
                    {
                        break event;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before driver device control event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for driver device control surface event");

    match event {
        ControlSurfaceEvent::ValuesChanged {
            surface_id: event_surface_id,
            revision,
            values,
        } => {
            assert_eq!(event_surface_id, surface_id);
            assert_eq!(revision, updated_revision);
            assert_eq!(
                values.get("protocol"),
                Some(&SurfaceControlValue::Enum("e131".to_owned()))
            );
        }
        _ => panic!("expected values_changed control surface event"),
    }
}

#[tokio::test]
async fn patch_driver_control_surface_updates_config() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:wled/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "default_protocol": { "kind": "enum", "value": "e131" },
                            "dedup_threshold": { "kind": "int", "value": 7 }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], "driver:wled");
    assert!(
        json["data"]["revision"].as_u64().expect("revision")
            > json["data"]["previous_revision"]
                .as_u64()
                .expect("previous revision")
    );
    assert_eq!(json["data"]["values"]["default_protocol"]["value"], "e131");
    assert_eq!(json["data"]["values"]["dedup_threshold"]["value"], 7);

    let config = manager.get();
    let wled = config
        .drivers
        .get("wled")
        .expect("wled config should exist");
    assert_eq!(
        wled.settings["default_protocol"],
        serde_json::json!({ "kind": "enum", "value": "e131" })
    );
    assert_eq!(
        wled.settings["dedup_threshold"],
        serde_json::json!({ "kind": "int", "value": 7 })
    );

    let backend_manager = state.backend_manager.lock().await;
    assert!(backend_manager.backend_ids().contains(&"wled"));
}

#[tokio::test]
async fn patch_govee_driver_control_surface_persists_backend_settings() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:govee/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "power_off_on_disconnect": { "kind": "bool", "value": true },
                            "lan_state_fps": { "kind": "int", "value": 12 }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], "driver:govee");
    assert_eq!(
        json["data"]["values"]["power_off_on_disconnect"]["value"],
        true
    );
    assert_eq!(json["data"]["values"]["lan_state_fps"]["value"], 12);
    assert_eq!(
        json["data"]["impacts"],
        serde_json::json!(["backend_rebind"])
    );

    let config = manager.get();
    let govee = config
        .drivers
        .get("govee")
        .expect("govee config should exist");
    assert_eq!(
        govee.settings["power_off_on_disconnect"],
        serde_json::json!({ "kind": "bool", "value": true })
    );
    assert_eq!(
        govee.settings["lan_state_fps"],
        serde_json::json!({ "kind": "int", "value": 12 })
    );
}

#[tokio::test]
async fn patch_hue_driver_control_surface_persists_backend_settings() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:hue/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "use_cie_xy": { "kind": "bool", "value": false }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], "driver:hue");
    assert_eq!(json["data"]["values"]["use_cie_xy"]["value"], false);
    assert_eq!(
        json["data"]["impacts"],
        serde_json::json!(["backend_rebind"])
    );

    let config = manager.get();
    let hue = config.drivers.get("hue").expect("hue config should exist");
    assert_eq!(
        hue.settings["use_cie_xy"],
        serde_json::json!({ "kind": "bool", "value": false })
    );
}

#[tokio::test]
async fn patch_nanoleaf_driver_control_surface_persists_backend_settings() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:nanoleaf/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "transition_time": { "kind": "int", "value": 8 }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], "driver:nanoleaf");
    assert_eq!(json["data"]["values"]["transition_time"]["value"], 8);
    assert_eq!(
        json["data"]["impacts"],
        serde_json::json!(["backend_rebind"])
    );

    let config = manager.get();
    let nanoleaf = config
        .drivers
        .get("nanoleaf")
        .expect("nanoleaf config should exist");
    assert_eq!(
        nanoleaf.settings["transition_time"],
        serde_json::json!({ "kind": "int", "value": 8 })
    );
}

#[tokio::test]
async fn patch_driver_control_surface_rejects_non_routable_ip_values() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:wled/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "known_ips": {
                                "kind": "list",
                                "value": [
                                    { "kind": "ip", "value": "127.0.0.1" }
                                ]
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(
        json["error"]["details"]["kind"],
        "driver_control_validation_failed"
    );
    assert_eq!(json["error"]["details"]["surface_id"], "driver:wled");
    assert_eq!(json["error"]["details"]["driver_id"], "wled");
    assert!(
        json["error"]["details"]["detail"]
            .as_str()
            .expect("error detail should be a string")
            .contains("invalid WLED known IP")
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("error message should be a string")
            .contains("invalid WLED known IP")
    );
    assert!(
        manager
            .get()
            .drivers
            .get("wled")
            .and_then(|entry| entry.settings.get("known_ips"))
            .is_none(),
        "invalid known IPs should not be persisted"
    );
}

#[tokio::test]
async fn patch_driver_control_surface_rejects_unknown_future_value_kind() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));
    let original_drivers = manager.get().drivers.clone();

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:wled/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "dedup_threshold": {
                                "kind": "spline_curve",
                                "value": [0.0, 0.4, 1.0]
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        manager.get().drivers,
        original_drivers,
        "unknown future value kinds should not be persisted"
    );
}

#[tokio::test]
async fn patch_driver_control_surface_rejects_transaction_without_partial_persist() {
    let (state, manager, _tmp) = test_state_with_temp_config_manager();
    let app = test_app_with_state(Arc::clone(&state));
    let original_drivers = manager.get().drivers.clone();

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:wled/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "dedup_threshold": { "kind": "int", "value": 13 },
                            "known_ips": {
                                "kind": "list",
                                "value": [
                                    { "kind": "ip", "value": "127.0.0.1" }
                                ]
                            },
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(
        json["error"]["details"]["kind"],
        "driver_control_validation_failed"
    );
    assert_eq!(
        manager.get().drivers,
        original_drivers,
        "invalid transactions should not partially persist valid changes"
    );
}

#[tokio::test]
async fn patch_driver_owned_device_control_surface_reports_validation_target() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));
    let surface_id = format!("driver:wled:device:{device_id}");

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/control-surfaces/{surface_id}/values"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "protocol": { "kind": "enum", "value": "bogus" }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(
        json["error"]["details"]["kind"],
        "driver_device_control_validation_failed"
    );
    assert_eq!(json["error"]["details"]["surface_id"], surface_id);
    assert_eq!(json["error"]["details"]["driver_id"], "wled");
    assert_eq!(json["error"]["details"]["device_id"], device_id.to_string());
}

#[tokio::test]
async fn patch_driver_control_surface_publishes_values_changed_event() {
    let (state, _manager, _tmp) = test_state_with_temp_config_manager();
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:wled/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "dedup_threshold": { "kind": "int", "value": 11 }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let updated_revision = json["data"]["revision"]
        .as_u64()
        .expect("updated revision should be an integer");

    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ControlSurfaceChanged(
                        event @ ControlSurfaceEvent::ValuesChanged { .. },
                    ) = timestamped.event
                    {
                        break event;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before control surface event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for control surface event");

    match event {
        ControlSurfaceEvent::ValuesChanged {
            surface_id,
            revision,
            values,
        } => {
            assert_eq!(surface_id, "driver:wled");
            assert_eq!(revision, updated_revision);
            assert_eq!(
                values.get("dedup_threshold"),
                Some(&SurfaceControlValue::Int(11))
            );
        }
        _ => panic!("expected values_changed control surface event"),
    }
}

#[tokio::test]
async fn invoke_driver_control_surface_action_routes_to_provider() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/control-surfaces/driver:wled/actions/missing")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({}).to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["details"]["kind"], "control_action_failed");
    assert_eq!(json["error"]["details"]["surface_id"], "driver:wled");
    assert_eq!(json["error"]["details"]["action_id"], "missing");
    assert!(
        json["error"]["details"]["detail"]
            .as_str()
            .expect("error detail")
            .contains("unknown control action")
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("unknown control action")
    );
}

#[tokio::test]
async fn driver_control_reload_preserves_raw_objects_with_kind_fields() {
    let (builder, tempdir) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    manager.modify(|config| {
        config.drivers.insert(
            "action_test".to_owned(),
            DriverConfigEntry::enabled(BTreeMap::from([(
                "descriptor".to_owned(),
                serde_json::json!({"kind": "network", "name": "fixture"}),
            )])),
        );
    });

    let mut registry = DriverModuleRegistry::new();
    registry
        .register(ActionTestDriver)
        .expect("test action driver should register");
    let registry = Arc::new(registry);
    let state = builder
        .with_config_manager(manager)
        .with_driver_registry(registry)
        .build();

    let values = state
        .driver_host()
        .load_driver_values("action_test")
        .await
        .expect("driver values should reload");

    assert_eq!(
        values.get("descriptor"),
        Some(&ControlValue::Text("fixture".to_owned()))
    );
}

#[tokio::test]
async fn driver_control_reload_rejects_malformed_canonical_envelopes() {
    let (builder, tempdir) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(tempdir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    manager.modify(|config| {
        config.drivers.insert(
            "action_test".to_owned(),
            DriverConfigEntry::enabled(BTreeMap::from([(
                "persisted".to_owned(),
                serde_json::json!({"kind": "float"}),
            )])),
        );
    });

    let mut registry = DriverModuleRegistry::new();
    registry
        .register(ActionTestDriver)
        .expect("test action driver should register");
    let registry = Arc::new(registry);
    let state = builder
        .with_config_manager(manager)
        .with_driver_registry(registry)
        .build();

    let error = state
        .driver_host()
        .load_driver_values("action_test")
        .await
        .expect_err("malformed canonical values must not fall back to a projection");

    assert!(
        error.to_string().contains(
            "invalid persisted control value for driver 'action_test' setting 'persisted'"
        )
    );
}

#[tokio::test]
async fn invoke_driver_control_surface_action_publishes_progress_event() {
    let (builder, _tempdir) = isolated_state_builder();
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(ActionTestDriver)
        .expect("test action driver should register");
    let registry = Arc::new(registry);
    let state = Arc::new(builder.with_driver_registry(registry).build());
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/control-surfaces/driver:action_test/actions/ping")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "input": {}
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], "driver:action_test");
    assert_eq!(json["data"]["action_id"], "ping");
    assert_eq!(json["data"]["status"], "completed");

    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ControlSurfaceChanged(
                        event @ ControlSurfaceEvent::ActionProgress { .. },
                    ) = timestamped.event
                    {
                        break event;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before control action event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for control action event");

    match event {
        ControlSurfaceEvent::ActionProgress {
            surface_id,
            action_id,
            status,
            progress,
        } => {
            assert_eq!(surface_id, "driver:action_test");
            assert_eq!(action_id, "ping");
            assert_eq!(status, ControlActionStatus::Completed);
            assert_eq!(progress, None);
        }
        _ => panic!("expected action_progress control surface event"),
    }
}

#[tokio::test]
async fn patch_driver_control_surface_discovery_rescan_runs_through_host() {
    let (builder, dir) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(dir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    let discoveries = Arc::new(AtomicUsize::new(0));
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(RescanTestDriver::new(Arc::clone(&discoveries)))
        .expect("test rescan driver should register");
    let registry = Arc::new(registry);
    let state = Arc::new(
        builder
            .with_config_manager(manager)
            .with_driver_registry(registry)
            .build(),
    );
    let app = test_app_with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:rescan_test/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "scan": { "kind": "bool", "value": true }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], "driver:rescan_test");
    assert_eq!(
        json["data"]["impacts"],
        serde_json::json!(["discovery_rescan"])
    );

    tokio::time::timeout(Duration::from_secs(2), async {
        while discoveries.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for driver discovery rescan");
}

#[tokio::test]
async fn patch_driver_control_surface_rejects_unsupported_driver_level_impact() {
    let (builder, dir) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(dir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(UnsupportedImpactTestDriver)
        .expect("test unsupported impact driver should register");
    let registry = Arc::new(registry);
    let state = builder
        .with_config_manager(manager)
        .with_driver_registry(registry)
        .build();
    let app = test_app_with_state(Arc::new(state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/control-surfaces/driver:unsupported_impact_test/values")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "topology": { "kind": "bool", "value": true }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = body_json(response).await;
    // An internal failure names itself in tracing, never on the wire.
    assert_eq!(json["error"]["code"], "internal_error");
    assert_eq!(json["error"]["message"], "internal error");
}

#[tokio::test]
async fn patch_driver_owned_device_control_surface_rejects_unsupported_device_level_impact() {
    let (builder, dir) = isolated_state_builder();
    let manager = Arc::new(
        ConfigManager::new(dir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(UnsupportedImpactTestDriver)
        .expect("test unsupported impact driver should register");
    let registry = Arc::new(registry);
    let state = Arc::new(
        builder
            .with_config_manager(manager)
            .with_driver_registry(registry)
            .build(),
    );
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(state);
    let surface_id = format!("driver:unsupported_impact_test:device:{device_id}");

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/control-surfaces/{surface_id}/values"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "topology": { "kind": "bool", "value": true }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = body_json(response).await;
    // An internal failure names itself in tracing, never on the wire.
    assert_eq!(json["error"]["code"], "internal_error");
    assert_eq!(json["error"]["message"], "internal error");
}

#[tokio::test]
async fn list_devices_includes_structured_segment_topology_hints() {
    let state = Arc::new(isolated_state());
    let id = DeviceId::new();
    let info = DeviceInfo {
        id,
        name: "Matrix Panel".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Panel".to_owned(),
            led_count: 96,
            topology: DeviceTopologyHint::Matrix { rows: 6, cols: 16 },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 96,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let _ = state.device_registry.add(info).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["items"][0]["layout_device_id"],
        "wled:matrix-panel"
    );
    let segment = &json["data"]["items"][0]["segments"][0];
    assert_eq!(segment["id"], "segment_0");
    assert_eq!(segment["name"], "Panel");
    assert_eq!(segment["topology_hint"]["type"], "matrix");
    assert_eq!(segment["topology_hint"]["rows"], 6);
    assert_eq!(segment["topology_hint"]["cols"], 16);
    assert!(json["data"]["items"][0].get("zones").is_none());
}

#[tokio::test]
async fn get_device_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/00000000-0000-0000-0000-000000000000")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "device_not_found");
    assert!(json["meta"]["request_id"].is_string());
}

#[tokio::test]
async fn get_device_by_unknown_name_returns_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/not-a-uuid")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "device_not_found");
}

#[tokio::test]
async fn delete_device_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/devices/00000000-0000-0000-0000-000000000000")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn discover_devices_returns_accepted() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"targets": ["wled"], "timeout_ms": 5000}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "scanning");
    assert!(
        json["data"]["scan_id"]
            .as_str()
            .expect("scan_id should be a string")
            .starts_with("scan_")
    );
}

#[tokio::test]
async fn discover_devices_wait_mode_returns_report() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"targets": ["wled"], "timeout_ms": 100, "wait": true}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "completed");
    assert!(
        json["data"]["scan_id"]
            .as_str()
            .expect("scan_id should be a string")
            .starts_with("scan_")
    );
    assert!(json["data"]["result"]["duration_ms"].is_number());
    assert!(json["data"]["result"]["scanners"].is_array());
}

// ── Effects ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_effects_returns_empty_list() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("items should be an array");
    assert!(items.is_empty());
}

#[tokio::test]
async fn list_effects_returns_items_sorted_by_name() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "zeta").await;
    insert_test_effect(&state, "Alpha").await;
    insert_test_effect(&state, "beta").await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let items = json["data"]["items"]
        .as_array()
        .expect("items should be an array");
    let names: Vec<&str> = items
        .iter()
        .map(|item| item["name"].as_str().expect("name should be a string"))
        .collect();
    assert_eq!(names, vec!["Alpha", "beta", "zeta"]);
}

#[tokio::test]
async fn list_effects_accepts_typed_category_and_source_filters() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;

    let response = test_app_with_state(Arc::clone(&state))
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects?category=ambient&source=native")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["items"][0]["category"], "ambient");
    assert_eq!(json["data"]["items"][0]["source"], "native");
}

#[tokio::test]
async fn list_effects_rejects_unknown_closed_vocabulary_values() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects?source=filesystem")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert!(
        json["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("filesystem"))
    );
}

#[tokio::test]
async fn list_effects_carries_authoritative_input_capabilities() {
    let state = Arc::new(isolated_state());
    insert_input_reactive_test_effect(&state, "Input Probe").await;

    let response = test_app_with_state(Arc::clone(&state))
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let effect = &json["data"]["items"][0];
    assert_eq!(effect["category"], "ambient");
    assert_eq!(effect["tags"], serde_json::json!(["test"]));
    assert_eq!(effect["input_reactive"], true);
    assert_eq!(effect["capabilities"]["audio_reactive"], false);
    assert_eq!(effect["capabilities"]["screen_reactive"], false);
    assert_eq!(effect["capabilities"]["input_reactive"], true);
}

#[test]
fn effect_summary_defaults_new_capabilities_for_older_payloads() {
    let summary: hypercolor_types::api::effects::EffectSummary =
        serde_json::from_value(serde_json::json!({
            "id": "legacy",
            "name": "Legacy",
            "description": "",
            "author": "",
            "category": "ambient",
            "source": "html",
            "runnable": true,
            "tags": [],
            "version": "1.0.0"
        }))
        .expect("older effect summary payload should deserialize");

    assert!(!summary.input_reactive);
    assert_eq!(
        summary.capabilities,
        hypercolor_types::api::effects::EffectCapabilitySet::default()
    );
}

#[tokio::test]
async fn get_effect_returns_controls() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/solid_color")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let controls = json["data"]["controls"]
        .as_array()
        .expect("controls should be an array");
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0]["id"], "speed");
    assert_eq!(controls[0]["kind"], "number");
}

#[tokio::test]
async fn apply_effect_upserts_primary_zone() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let manager = state.scene_manager.snapshot().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_zone)
        .expect("active scene should contain a primary zone");
    assert_eq!(primary.role, ZoneRole::Primary);
    assert!(primary.effect_ids().next().is_some());
}

#[tokio::test]
async fn apply_effect_accepts_canonical_gradient_controls() {
    let state = Arc::new(isolated_state());
    let gradient = ControlValue::Gradient(vec![
        GradientStop {
            position: 0.0,
            color: [1.0, 0.0, 0.0, 1.0],
        },
        GradientStop {
            position: 0.5,
            color: [0.5, 0.25, 0.75, 1.0],
        },
        GradientStop {
            position: 1.0,
            color: [0.0, 0.0, 1.0, 1.0],
        },
    ]);
    insert_test_effect_with_controls(
        &state,
        "gradient_test",
        vec![ControlDefinition {
            id: "palette".to_owned(),
            name: "Palette".to_owned(),
            kind: ControlKind::Other("gradient".to_owned()),
            control_type: ControlType::GradientEditor,
            default_value: gradient.clone(),
            min: None,
            max: None,
            step: None,
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        Vec::new(),
    )
    .await;
    let app = test_app_with_state(Arc::clone(&state));
    let body = serde_json::json!({
        "controls": {
            "palette": serde_json::to_value(&gradient)
                .expect("gradient should serialize for the REST request")
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/gradient_test/apply")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&body).expect("request body should serialize"),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let manager = state.scene_manager.snapshot().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_zone)
        .expect("active scene should contain a primary zone");
    assert_eq!(
        zone_effect_controls(primary).and_then(|controls| controls.get("palette")),
        Some(&gradient)
    );
}

#[tokio::test]
async fn apply_effect_targets_a_named_zone_via_zone_id() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;

    // The default scene is born with a Primary zone; add a Custom zone to
    // target, and remember the Primary's effect so the apply can be shown
    // to leave it alone.
    let (custom_id, primary_effect_before) = {
        let mut mutation = state.scene_manager.begin_mutation().await;
        let custom_id = mutation
            .create_zone(SceneId::DEFAULT, "Ambient".to_owned(), None, (320, 200))
            .expect("custom zone should be created");
        let primary_effect = mutation
            .scenes()
            .active_scene()
            .and_then(Scene::primary_zone)
            .and_then(|zone| zone.effect_ids().next());
        hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
            .await
            .expect("custom zone should commit");
        (custom_id, primary_effect)
    };

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "zone": custom_id.to_string(),
                    }))
                    .expect("request body should serialize"),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let manager = state.scene_manager.snapshot().await;
    let scene = manager.active_scene().expect("a scene should be active");
    let custom = scene
        .zones
        .iter()
        .find(|zone| zone.id == custom_id)
        .expect("the targeted zone should still exist");
    assert!(
        custom.effect_ids().next().is_some(),
        "the effect should land in the targeted zone"
    );
    assert_eq!(
        scene
            .primary_zone()
            .and_then(|zone| zone.effect_ids().next()),
        primary_effect_before,
        "a named-zone apply must leave the Primary zone untouched",
    );
}

#[tokio::test]
async fn effect_started_event_for_named_zone_carries_zone_identity() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;

    let custom_id = {
        let mut mutation = state.scene_manager.begin_mutation().await;
        let zone_id = mutation
            .create_zone(SceneId::DEFAULT, "Ambient".to_owned(), None, (320, 200))
            .expect("custom zone should be created");
        hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
            .await
            .expect("custom zone should commit");
        zone_id
    };

    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "zone": custom_id.to_string(),
                    }))
                    .expect("request body should serialize"),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let started = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::EffectStarted {
                        previous,
                        zone_id,
                        zone_name,
                        ..
                    } = timestamped.event
                    {
                        break (previous, zone_id, zone_name);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("event stream closed early: {error}"),
            }
        }
    })
    .await
    .expect("an EffectStarted event should be published");

    assert_eq!(
        started.1,
        Some(custom_id),
        "EffectStarted must name the zone the effect landed in"
    );
    assert_eq!(started.2.as_deref(), Some("Ambient"));
    assert!(
        started.0.is_none(),
        "previous must be the target zone's prior effect (idle), not the Primary's"
    );
}

#[tokio::test]
async fn get_effect_cover_returns_webp_image() {
    let _cover_lock = COVER_DATA_DIR_LOCK.lock().await;
    let cover_fixture = CoverFixtureGuard::install("rainbow");
    let state = Arc::new(AppState::new_with_data_dir(cover_fixture.data_dir()));
    insert_test_effect(&state, "rainbow").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/effects/rainbow/cover")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let cache_control = response
        .headers()
        .get(http::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");

    assert_eq!(content_type, "image/webp");
    assert_eq!(cache_control, "public, max-age=86400");
    assert!(bytes.starts_with(b"RIFF"));
}

#[tokio::test]
async fn pausing_output_darkens_display_zones_without_an_active_effect() {
    let state = Arc::new(isolated_state());
    let zone_id = hypercolor_types::scene::ZoneId::new();
    state.event_bus.upsert_display_zone_target(
        zone_id,
        DisplayZoneTarget {
            device_id: DeviceId::new(),
            blend_mode: BlendMode::Alpha,
            opacity: 1.0,
            finalized: false,
        },
    );
    let zone_sender = state.event_bus.zone_canvas_sender(zone_id);
    let mut red_canvas = Canvas::new(2, 2);
    red_canvas.fill(Rgba::new(255, 0, 0, 255));
    zone_sender.send_replace(display_zone_frame(&red_canvas, 7, 7));
    let zone_receiver = zone_sender.subscribe();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .clone()
        .oneshot(output_patch_request(r#"{"power":"paused"}"#))
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json = body_json(response).await;
    assert_eq!(response_json["data"]["power"], "paused");
    assert!(state.output_power.snapshot().manually_paused());
    assert_display_zone_frame_black(&zone_receiver.borrow());
    let snapshot = runtime_state::load(&state.runtime_state_path)
        .expect("runtime snapshot should load")
        .expect("pause should persist runtime state");
    assert!(snapshot.manual_paused);

    let resume_response = app
        .oneshot(output_patch_request(r#"{"power":"running"}"#))
        .await
        .expect("failed to execute request");
    assert_eq!(resume_response.status(), StatusCode::OK);
    assert_eq!(body_json(resume_response).await["data"]["power"], "running");
}

#[tokio::test]
async fn pause_blacks_connected_device_outside_active_layout() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Unassigned Strip").await;
    let device_info = state
        .device_registry
        .get(&device_id)
        .await
        .expect("test device should exist")
        .info;
    let layout_device_id = format!("unassigned:{device_id}");
    let writes = Arc::new(StdMutex::new(Vec::new()));
    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Arc::new(StaticOutputRecordingBackend {
            writes: Arc::clone(&writes),
        }));
        manager
            .connect_device("static-output", device_id, &layout_device_id)
            .await
            .expect("test device should connect");
        assert!(manager.set_device_zone_segments(&layout_device_id, &device_info));
    }
    assert!(
        state
            .spatial_engine
            .snapshot()
            .layout()
            .zones
            .iter()
            .all(|zone| zone.device_id != layout_device_id)
    );

    let response = test_app_with_state(Arc::clone(&state))
        .oneshot(output_patch_request(r#"{"power":"paused"}"#))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let (written_device_id, colors) = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let latest = writes
                .lock()
                .expect("static output writes lock")
                .last()
                .cloned();
            if latest.is_some() {
                break latest;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pause should write the unassigned device")
    .expect("pause should record a static output frame");
    assert_eq!(written_device_id, device_id);
    assert_eq!(colors.len(), 60);
    assert!(colors.iter().all(|color| *color == [0, 0, 0]));
}

#[tokio::test]
async fn output_power_patch_is_idempotent_and_publishes_effective_transitions_once() {
    let state = Arc::new(isolated_state());
    state.render_loop.write().await.start();
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));

    for requested in ["paused", "paused", "running", "running"] {
        let response = app
            .clone()
            .oneshot(output_patch_request(&format!(
                r#"{{"power":"{requested}"}}"#
            )))
            .await
            .expect("failed to execute request");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["data"]["power"], requested);
    }

    assert!(matches!(
        events.try_recv().expect("pause event").event,
        HypercolorEvent::Paused
    ));
    assert!(matches!(
        events.try_recv().expect("resume event").event,
        HypercolorEvent::Resumed
    ));
    assert!(events.try_recv().is_err());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/output")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(body_json(response).await["data"]["power"], "running");
}

/// One PATCH moves both knobs, and the response is the whole resource.
#[tokio::test]
async fn output_patch_sets_power_and_brightness_in_one_call() {
    let state = Arc::new(isolated_state());
    state.render_loop.write().await.start();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .clone()
        .oneshot(output_patch_request(
            r#"{"power":"paused","brightness":0.25}"#,
        ))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["power"], "paused");
    assert_eq!(json["data"]["brightness"], 0.25);
    assert!(state.output_power.snapshot().manually_paused());
    assert_eq!(state.output_power.global_brightness(), 0.25);

    // A brightness-only patch leaves power exactly where it was.
    let response = app
        .oneshot(output_patch_request(r#"{"brightness":0.75}"#))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["power"], "paused");
    assert_eq!(json["data"]["brightness"], 0.75);
}

/// The service, not the decoder, refuses a patch that asks for nothing:
/// an empty document is a client that dropped its payload, and a silent
/// 200 there hides the defect. `GET /output` is how a caller reads.
#[tokio::test]
async fn output_patch_rejects_a_document_that_sets_nothing() {
    let app = test_app();

    let response = app
        .oneshot(output_patch_request("{}"))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(
        json["error"]["message"],
        "output patch must set power, brightness, or both"
    );
}

/// Brightness range is a domain rule: the type layer takes any `f32`
/// and the handler names the offending field on refusal.
#[tokio::test]
async fn output_patch_rejects_brightness_outside_the_unit_interval() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    // `1e40` overflows the f32 cast to infinity, which fails the range
    // check the same way NaN would: every comparison against a non-finite
    // value is false, so `contains` says no.
    for rejected in ["1.5", "-0.1", "1e40", "-1e40"] {
        let response = app
            .clone()
            .oneshot(output_patch_request(&format!(
                r#"{{"brightness":{rejected}}}"#
            )))
            .await
            .expect("failed to execute request");
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "brightness {rejected} must be refused"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "validation_error");
        assert_eq!(json["error"]["details"]["field"], "brightness");
    }

    assert_eq!(state.output_power.global_brightness(), 1.0);
}

/// A rejected brightness never reaches the power half of the patch.
#[tokio::test]
async fn output_patch_validates_brightness_before_moving_power() {
    let state = Arc::new(isolated_state());
    state.render_loop.write().await.start();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(output_patch_request(
            r#"{"power":"paused","brightness":2.0}"#,
        ))
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!state.output_power.snapshot().manually_paused());
}

/// The routes this resource replaced are gone, not aliased.
///
/// Retired paths and method mismatches are both owned by the API fallback,
/// so every removed route has one canonical 404 shape.
#[tokio::test]
async fn the_merged_output_routes_leave_nothing_behind() {
    let app = test_app();

    let retired = [
        (
            Request::builder()
                .uri("/api/v1/output/power")
                .body(Body::empty())
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .method("PUT")
                .uri("/api/v1/output/power")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"state":"paused"}"#))
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .uri("/api/v1/settings/brightness")
                .body(Body::empty())
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .method("PUT")
                .uri("/api/v1/settings/brightness")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"brightness":42}"#))
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/pause")
                .body(Body::empty())
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/resume")
                .body(Body::empty())
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .uri("/api/v1/audio/devices")
                .body(Body::empty())
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
        (
            Request::builder()
                .method("PUT")
                .uri("/api/v1/output")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"power":"paused"}"#))
                .expect("failed to build request"),
            StatusCode::NOT_FOUND,
        ),
    ];

    for (request, expected) in retired {
        let uri = request.uri().to_string();
        let method = request.method().clone();
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("failed to execute request");
        assert_eq!(
            response.status(),
            expected,
            "{method} {uri} must be gone, not aliased or redirected"
        );
        if expected == StatusCode::NOT_FOUND {
            assert_canonical_route_404(response, &uri).await;
        }
    }
}

/// The SPA fallback must never answer for an API path. With a web UI
/// mounted, an unmatched `/api/v1` route still renders the canonical
/// envelope, while a real client-side route still serves the app shell.
///
/// Without this the deletion fences are theatre in exactly the
/// configuration users run: `ServeDir` misses, falls through to
/// `index.html`, and a retired endpoint answers `200 text/html`.
#[tokio::test]
async fn the_spa_fallback_never_answers_for_a_deleted_api_route() {
    let (app, _ui_dir) = test_app_with_ui();

    for path in [
        "/api/v1/audio/devices",
        "/api/v1/settings/brightness",
        "/api/v1/output/power",
        "/api/v1/there-is-no-such-route",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");
        assert_canonical_route_404(response, path).await;
    }

    // The SPA still owns everything that is not an API path.
    let spa = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/settings")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(spa.status(), StatusCode::OK);
    let body = axum::body::to_bytes(spa.into_body(), usize::MAX)
        .await
        .expect("spa body should read");
    assert!(
        String::from_utf8_lossy(&body).contains("<!doctype html>"),
        "a client-side route should still serve the app shell"
    );

    // And the API surface that does exist is untouched by the fallback.
    for path in ["/api/v1/output", "/api/v1/openapi.json"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path} should still serve"
        );
    }
}

/// The same fence without a UI mounted: the bare Axum 404 is replaced by
/// the canonical envelope, so clients get one error shape everywhere.
#[tokio::test]
async fn an_unmatched_api_path_renders_the_canonical_envelope_without_a_ui() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/there-is-no-such-route")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_canonical_route_404(response, "/api/v1/there-is-no-such-route").await;
}

#[tokio::test]
async fn apply_effect_resumes_before_release_reconnect_scan_finishes() {
    let dir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    let manager = Arc::new(
        ConfigManager::new(dir.path().join("config.toml"))
            .expect("config manager should be created"),
    );
    let discoveries = Arc::new(AtomicUsize::new(0));
    let release_scan = Arc::new(Semaphore::new(0));
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(BlockingReconnectTestDriver::new(
            Arc::clone(&discoveries),
            Arc::clone(&release_scan),
        ))
        .expect("blocking reconnect driver should register");
    let registry = Arc::new(registry);
    let state = Arc::new(
        AppStateBuilder::new(data_dir)
            .with_config_manager(manager)
            .with_driver_registry(registry)
            .build(),
    );
    insert_test_effect(&state, "solid_color").await;
    {
        let mut render_loop = state.render_loop.write().await;
        render_loop.start();
        render_loop.pause();
    }
    state
        .output_power
        .set_output_stopped(&state.event_bus)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = tokio::time::timeout(
        Duration::from_millis(200),
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        ),
    )
    .await
    .expect("effect apply should not wait for the reconnect scan")
    .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        state.render_loop.read().await.state(),
        RenderLoopState::Running
    );
    let power_state = state.output_power.snapshot();
    assert!(!power_state.sleeping());
    assert_eq!(power_state.session_brightness, 1.0);
    tokio::time::timeout(Duration::from_secs(1), async {
        while discoveries.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reconnect scan should run in the background");
    release_scan.add_permits(1);
}

#[tokio::test]
async fn apply_effect_swap_replaces_primary_effect_id() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;
    insert_test_effect(&state, "Sunset").await;
    let app = test_app_with_state(Arc::clone(&state));

    let first_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Aurora/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_response.status(), StatusCode::OK);
    let first_primary_effect_id = {
        let manager = state.scene_manager.snapshot().await;
        manager
            .active_scene()
            .and_then(Scene::primary_zone)
            .and_then(|zone| zone.effect_ids().next())
            .expect("first effect apply should populate the primary zone")
    };

    let second_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Sunset/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_response.status(), StatusCode::OK);

    let manager = state.scene_manager.snapshot().await;
    let active_scene = manager.active_scene().expect("active scene should remain");
    assert_eq!(active_scene.zones.len(), 1);
    let primary = active_scene
        .primary_zone()
        .expect("primary zone should exist after effect swap");
    assert_ne!(primary.effect_ids().next(), Some(first_primary_effect_id));
    assert!(primary.effect_ids().next().is_some());
}

#[tokio::test]
async fn apply_effect_with_preset_id_sets_zone_preset_atomically() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Saved State",
                        "effect":"solid_color",
                        "controls":{"speed":3.5}
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let preset_id = body_json(create_response).await["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let apply_body = format!(r#"{{"preset_id":"{preset_id}"}}"#);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/solid_color/apply")
                .header("content-type", "application/json")
                .body(Body::from(apply_body))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let manager = state.scene_manager.snapshot().await;
    let primary = manager
        .active_scene()
        .and_then(Scene::primary_zone)
        .expect("primary zone should exist after apply");
    assert_eq!(
        zone_effect_preset(primary),
        Some(preset_id),
        "preset_id should be set on the zone in the same transaction as the effect start"
    );
    let speed = zone_effect_controls(primary)
        .expect("primary effect layer should exist")
        .get("speed")
        .expect("preset controls should be baked into the zone");
    assert!(matches!(
        speed,
        hypercolor_types::control::ControlValue::Float(value) if (*value - 3.5).abs() < 0.01
    ));
}

#[tokio::test]
async fn apply_effect_rejects_preset_targeting_different_effect() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    insert_test_effect(&state, "aurora").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Mismatch","effect":"solid_color","controls":{}}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let preset_id = body_json(create_response).await["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let apply_body = format!(r#"{{"preset_id":"{preset_id}"}}"#);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/aurora/apply")
                .header("content-type", "application/json")
                .body(Body::from(apply_body))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn library_favorites_crud_lifecycle() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let add_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/favorites")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"effect":"solid_color"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(add_response.status(), StatusCode::OK);
    let add_json = body_json(add_response).await;
    assert_eq!(add_json["data"]["created"], true);

    let list_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/favorites")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["total"], 1);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/library/favorites/solid_color")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let list_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/favorites")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["total"], 0);
}

#[tokio::test]
async fn library_presets_create_and_get() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Warm Sweep",
                        "effect":"solid_color",
                        "controls":{"speed":7.25},
                        "tags":[" cozy ","test"]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    assert_eq!(create_json["data"]["name"], "Warm Sweep");
    assert_eq!(create_json["data"]["controls"]["speed"]["kind"], "float");
    assert_eq!(create_json["data"]["controls"]["speed"]["value"], 7.5);
    assert_eq!(create_json["data"]["tags"][0], "cozy");
    let preset_id = create_json["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/library/presets/{preset_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["id"], preset_id);
    assert_eq!(get_json["data"]["controls"]["speed"]["kind"], "float");
    assert_eq!(get_json["data"]["controls"]["speed"]["value"], 7.5);
}

#[tokio::test]
async fn library_playlists_create_with_effect_and_preset_targets() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let preset_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/presets")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"Preset A",
                        "effect":"solid_color",
                        "controls":{"speed":5}
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(preset_response.status(), StatusCode::CREATED);
    let preset_json = body_json(preset_response).await;
    let preset_id = preset_json["data"]["id"]
        .as_str()
        .expect("preset id should be string")
        .to_owned();

    let playlist_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{
                        "name":"Night Rotation",
                        "loop_enabled":true,
                        "items":[
                            {{
                                "target":{{"type":"effect","effect":"solid_color"}},
                                "duration_ms":2000
                            }},
                            {{
                                "target":{{"type":"preset","preset_id":"{preset_id}"}},
                                "duration_ms":3000
                            }}
                        ]
                    }}"#
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(playlist_response.status(), StatusCode::CREATED);
    let playlist_json = body_json(playlist_response).await;
    assert_eq!(
        playlist_json["data"]["items"]
            .as_array()
            .map_or(0, Vec::len),
        2
    );
    let playlist_id = playlist_json["data"]["id"]
        .as_str()
        .expect("playlist id should be string")
        .to_owned();

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/library/playlists/{playlist_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["items"].as_array().map_or(0, Vec::len), 2);
}

#[tokio::test]
async fn library_playlist_advance_replaces_stack_without_waking_output() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"runtime_by_name",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);

    hypercolor_daemon::domain::output::set_power(
        &state.domains.output,
        hypercolor_types::api::output::OutputPowerMode::Paused,
    )
    .await;

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/runtime_by_name/activate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);
    assert!(state.output_power.snapshot().manually_paused());

    let first_layer_id = {
        let manager = state.scene_manager.snapshot().await;
        manager
            .active_scene()
            .and_then(hypercolor_types::scene::Scene::primary_zone)
            .and_then(|zone| zone.layers.first())
            .map(|layer| layer.id)
            .expect("playlist activation should create one primary layer")
    };

    let second_activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/runtime_by_name/activate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_activate_response.status(), StatusCode::OK);
    assert!(state.output_power.snapshot().manually_paused());

    let second_layer_id = {
        let manager = state.scene_manager.snapshot().await;
        let primary = manager
            .active_scene()
            .and_then(hypercolor_types::scene::Scene::primary_zone)
            .expect("playlist activation should retain the primary zone");
        assert_eq!(primary.layers.len(), 1);
        primary.layers[0].id
    };
    assert_ne!(first_layer_id, second_layer_id);

    let deactivate_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/deactivate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(deactivate_response.status(), StatusCode::OK);
    let deactivate_json = body_json(deactivate_response).await;
    assert_eq!(deactivate_json["data"]["deactivated"], true);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "test validates full playlist replacement lifecycle"
)]
async fn library_playlist_activate_replaces_previous_runtime() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let first_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"first_runtime",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_create.status(), StatusCode::CREATED);
    let first_json = body_json(first_create).await;
    let first_id = first_json["data"]["id"]
        .as_str()
        .expect("first playlist id should be string")
        .to_owned();

    let second_create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"second_runtime",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_create.status(), StatusCode::CREATED);
    let second_json = body_json(second_create).await;
    let second_id = second_json["data"]["id"]
        .as_str()
        .expect("second playlist id should be string")
        .to_owned();

    let first_activate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{first_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(first_activate.status(), StatusCode::OK);

    let second_activate = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{second_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(second_activate.status(), StatusCode::OK);

    let active_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/playlists/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["playlist"]["id"], second_id);

    let deactivate_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists/deactivate")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(deactivate_response.status(), StatusCode::OK);
}

#[tokio::test]
async fn library_delete_active_playlist_stops_runtime() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"delete_me",
                        "items":[
                            {
                                "target":{"type":"effect","effect":"solid_color"},
                                "duration_ms":10000
                            }
                        ]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let playlist_id = create_json["data"]["id"]
        .as_str()
        .expect("playlist id should be string")
        .to_owned();

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{playlist_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/library/playlists/{playlist_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let active_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/library/playlists/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::NOT_FOUND);
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn library_delete_keeps_active_playlist_when_persistence_admission_fails() {
    let (builder, tempdir) = isolated_state_builder();
    let library = Arc::new(
        JsonLibraryStore::open(tempdir.path().join("library.json")).expect("JSON library store"),
    );
    let state = builder.with_library(library).build();
    let state = Arc::new(state);
    insert_test_effect(&state, "solid_color").await;
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/playlists")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{
                        "name":"must_keep_playing",
                        "items":[{
                            "target":{"type":"effect","effect":"solid_color"},
                            "duration_ms":10000
                        }]
                    }"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let playlist_id = create_json["data"]["id"]
        .as_str()
        .expect("playlist id should be string")
        .to_owned();

    let activate_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/library/playlists/{playlist_id}/activate"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(activate_response.status(), StatusCode::OK);
    hypercolor_daemon::persistence::set_injected_serialization_failures(1);

    let delete_response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/library/playlists/{playlist_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(delete_response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        state
            .playlist_runtime
            .lock()
            .await
            .active
            .as_ref()
            .map(|active| active.playlist_id.to_string()),
        Some(playlist_id.clone())
    );
    assert!(
        state
            .library_store()
            .list_playlists()
            .await
            .iter()
            .any(|playlist| playlist.id.to_string() == playlist_id)
    );
}

// ── Scenes ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn scene_crud_publishes_scene_library_changed_events() {
    let state = Arc::new(isolated_state());
    let mut events = state.event_bus.subscribe_all();

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Library Watch"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::CREATED);
    let scene_id = body_json(response).await["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

    let created = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::SceneLibraryChanged {
                        scene_id,
                        kind,
                        name,
                        ..
                    } = timestamped.event
                    {
                        break (scene_id, kind, name);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("event stream closed early: {error}"),
            }
        }
    })
    .await
    .expect("a SceneLibraryChanged event should be published on create");

    assert_eq!(created.0.to_string(), scene_id);
    assert_eq!(
        created.1,
        hypercolor_types::event::SceneLibraryChangeKind::Created
    );
    assert_eq!(created.2.as_deref(), Some("Library Watch"));

    let app = test_app_with_state(Arc::clone(&state));
    let delete_response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let deleted = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::SceneLibraryChanged { kind, .. } = timestamped.event {
                        break kind;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("event stream closed early: {error}"),
            }
        }
    })
    .await
    .expect("a SceneLibraryChanged event should be published on delete");
    assert_eq!(
        deleted,
        hypercolor_types::event::SceneLibraryChangeKind::Deleted
    );
}

#[tokio::test]
async fn list_scenes_excludes_default_scene() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["total"], 0);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Movie Night"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::CREATED);

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["total"], 1);
    let items = json["data"]["items"]
        .as_array()
        .expect("scene list should serialize as an array");
    assert_eq!(items[0]["name"], "Movie Night");
    assert!(
        items.iter().all(|item| item["name"] != "Default"),
        "default scene must stay hidden from the scenes list"
    );
}

#[tokio::test]
async fn snapshot_scene_creates_a_locked_copy_of_the_live_tree() {
    let state = Arc::new(isolated_state());
    let active = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .cloned()
        .expect("default scene should be active");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes/snapshot")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk capture","description":"Current runtime"}"#,
                ))
                .expect("snapshot request"),
        )
        .await
        .expect("snapshot response");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Desk capture");
    assert_eq!(json["data"]["mutation_mode"], "snapshot");
    let scene_id = json["data"]["id"]
        .as_str()
        .expect("snapshot id")
        .parse::<Uuid>()
        .expect("snapshot UUID");

    let manager = state.scene_manager.snapshot().await;
    let saved = manager
        .get(&SceneId(scene_id))
        .expect("snapshot should be stored");
    assert_eq!(saved.zones, active.zones);
    assert_eq!(saved.activation_brightness, None);
    assert_eq!(manager.active_scene_id(), Some(&active.id));
}

#[tokio::test]
async fn stored_scene_replace_is_whole_document_versioned_and_identity_safe() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/scenes")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Original","description":"old"}"#))
                .expect("create request"),
        )
        .await
        .expect("create response");
    let created = body_json(created).await;
    let scene_id = created["data"]["id"].as_str().expect("scene id");

    let fetched = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .body(Body::empty())
                .expect("get request"),
        )
        .await
        .expect("get response");
    assert_eq!(fetched.status(), StatusCode::OK);
    let etag = fetched
        .headers()
        .get(http::header::ETAG)
        .expect("stored scene ETag")
        .clone();
    let fetched = body_json(fetched).await;
    let document: SceneDocument =
        serde_json::from_value(fetched["data"].clone()).expect("full scene document");
    let old_zone_id = document.zones[0].id;
    let mut replacement = ReplaceSceneRequest::from(&document);
    replacement.name = "Replacement".to_owned();
    replacement.description = None;
    replacement.activation_brightness = Some(0.42);
    replacement.priority = ScenePriority::ALERT;
    replacement.enabled = false;
    replacement
        .metadata
        .insert("origin".to_owned(), "whole-document-test".to_owned());
    replacement.zones[0].id = None;
    replacement.zones[0].layers.push(ReplaceSceneLayerRequest {
        id: None,
        name: Some("Minted fill".to_owned()),
        source: LayerSource::ColorFill {
            rgba: [0.1, 0.2, 0.3, 1.0],
        },
        blend: BlendMode::Replace,
        opacity: 0.75,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
    });

    let replaced = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .header("content-type", "application/json")
                .header(http::header::IF_MATCH, etag.clone())
                .body(Body::from(
                    serde_json::to_vec(&replacement).expect("replacement body"),
                ))
                .expect("replace request"),
        )
        .await
        .expect("replace response");
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_ne!(
        replaced.headers().get(http::header::ETAG),
        Some(&etag),
        "successful replacement advances the scene revision"
    );
    let replaced = body_json(replaced).await;
    let document: SceneDocument =
        serde_json::from_value(replaced["data"].clone()).expect("replacement document");
    assert_eq!(document.id.to_string(), scene_id);
    assert_eq!(document.name, "Replacement");
    assert_eq!(document.description, None);
    assert_eq!(document.activation_brightness, Some(0.42));
    assert_eq!(document.priority, ScenePriority::ALERT);
    assert!(!document.enabled);
    assert_eq!(
        document.metadata.get("origin").map(String::as_str),
        Some("whole-document-test")
    );
    assert_ne!(document.zones[0].id, old_zone_id);
    assert_eq!(document.zones[0].layers.len(), 1);
    let minted_zone_id = document.zones[0].id;

    let stale = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .header("content-type", "application/json")
                .header(http::header::IF_MATCH, etag)
                .body(Body::from(
                    serde_json::to_vec(&replacement).expect("stale body"),
                ))
                .expect("stale request"),
        )
        .await
        .expect("stale response");
    assert_eq!(stale.status(), StatusCode::PRECONDITION_FAILED);

    let mut mismatch = ReplaceSceneRequest::from(&document);
    mismatch.id = Some(SceneId::new());
    let mismatch = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&mismatch).expect("mismatch body"),
                ))
                .expect("mismatch request"),
        )
        .await
        .expect("mismatch response");
    assert_eq!(mismatch.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut foreign_zone = ReplaceSceneRequest::from(&document);
    foreign_zone.zones[0].id = Some(ZoneId::new());
    let foreign_zone = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&foreign_zone).expect("foreign zone body"),
                ))
                .expect("foreign zone request"),
        )
        .await
        .expect("foreign zone response");
    assert_eq!(foreign_zone.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut foreign_layer = ReplaceSceneRequest::from(&document);
    foreign_layer.zones[0].id = Some(minted_zone_id);
    foreign_layer.zones[0].layers[0].id = Some(SceneLayerId::new());
    let foreign_layer = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/scenes/{scene_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&foreign_layer).expect("foreign layer body"),
                ))
                .expect("foreign layer request"),
        )
        .await
        .expect("foreign layer response");
    assert_eq!(foreign_layer.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn stored_scene_get_and_put_exclude_the_ephemeral_default() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let before = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/scene")
                .body(Body::empty())
                .expect("live scene request"),
        )
        .await
        .expect("live scene response");
    let etag = before
        .headers()
        .get(http::header::ETAG)
        .expect("live scene ETag")
        .clone();
    let before = body_json(before).await;
    let document: SceneDocument =
        serde_json::from_value(before["data"].clone()).expect("default scene document");

    let get = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes/default")
                .body(Body::empty())
                .expect("stored scene GET"),
        )
        .await
        .expect("stored scene GET response");
    assert_eq!(get.status(), StatusCode::NOT_FOUND);

    let put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/scenes/default")
                .header("content-type", "application/json")
                .header(http::header::IF_MATCH, etag)
                .body(Body::from(
                    serde_json::to_vec(&ReplaceSceneRequest::from(&document))
                        .expect("default replacement body"),
                ))
                .expect("stored scene PUT"),
        )
        .await
        .expect("stored scene PUT response");
    assert_eq!(put.status(), StatusCode::NOT_FOUND);

    let after = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scene")
                .body(Body::empty())
                .expect("live scene request after refusal"),
        )
        .await
        .expect("live scene response after refusal");
    let after = body_json(after).await;
    assert_eq!(after["data"], before["data"]);
}

#[tokio::test]
async fn delete_default_returns_409_or_422() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/scenes/default")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("cannot be deleted"),
    );
}

// ── Layouts ──────────────────────────────────────────────────────────────

fn layout_with_sampling_modes(
    default_sampling_mode: SamplingMode,
    output_sampling_mode: SamplingMode,
) -> SpatialLayout {
    SpatialLayout {
        id: "sampling-layout".to_owned(),
        name: "Sampling Layout".to_owned(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![Output {
            id: "sampling-output".to_owned(),
            name: "Sampling Output".to_owned(),
            device_id: "mock:sampling-output".to_owned(),
            zone_name: None,
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            display_order: 0,
            orientation: None,
            topology: LedTopology::Point,
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(output_sampling_mode),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

#[tokio::test]
async fn layout_crud_lifecycle() {
    let state = Arc::new(isolated_state());

    // Create layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name": "Main Setup", "canvas_width": 320, "canvas_height": 200}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Main Setup");
    assert_eq!(json["data"]["canvas_width"], 320);
    let layout_id = json["data"]["id"]
        .as_str()
        .expect("id should be a string")
        .to_owned();

    // Get layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    // List layouts
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["total"], 2);
    assert!(
        json["data"]["items"]
            .as_array()
            .expect("layout items should be an array")
            .iter()
            .any(|layout| layout["id"] == layout_id)
    );

    // Update layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name": "Updated Setup"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["name"], "Updated Setup");

    // Delete layout
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{layout_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["deleted"], true);
}

#[tokio::test]
async fn layout_create_defaults_canvas_to_active_layout_dimensions() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let active_layout = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Canvas Follower"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["canvas_width"], active_layout.canvas_width);
    assert_eq!(json["data"]["canvas_height"], active_layout.canvas_height);
}

#[tokio::test]
async fn layout_create_accepts_large_finite_canvas_dimensions() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"8K Canvas","canvas_width":7680,"canvas_height":4320}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CREATED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["canvas_width"], 7_680);
    assert_eq!(json["data"]["canvas_height"], 4_320);
}

#[tokio::test]
async fn layout_create_rejects_canvas_dimensions_that_overflow_resources() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Overflow","canvas_width":4294967295,"canvas_height":4294967295}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert!(
        json["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("overflows addressable memory"))
    );
}

#[tokio::test]
async fn layout_apply_updates_active_layout() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Studio Layout","canvas_width":640,"canvas_height":360}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let layout_id = create_json["data"]["id"]
        .as_str()
        .expect("id should be string")
        .to_owned();

    let (apply_response, applied_layouts) = request_with_layout_ack(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{layout_id}/apply"))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    assert_eq!(apply_response.status(), StatusCode::OK);
    let apply_json = body_json(apply_response).await;
    assert_eq!(apply_json["data"]["applied"], true);
    assert_eq!(apply_json["data"]["persistence_pending"], false);
    assert_eq!(apply_json["data"]["layout"]["id"], layout_id);

    let active_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["id"], layout_id);
    assert_eq!(active_json["data"]["name"], "Studio Layout");

    assert!(matches!(
        applied_layouts.first(),
        Some(layout)
            if layout.id == layout_id
                && layout.canvas_width == 640
                && layout.canvas_height == 360
    ));

    let list_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts?active=true")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_json = body_json(list_response).await;
    assert_eq!(list_json["data"]["total"], 1);
    assert_eq!(list_json["data"]["items"][0]["id"], layout_id);
    assert_eq!(list_json["data"]["items"][0]["is_active"], true);

    let runtime_raw = std::fs::read_to_string(&state.runtime_state_path)
        .expect("runtime state file should exist after apply");
    let runtime_json: serde_json::Value =
        serde_json::from_str(&runtime_raw).expect("runtime state should be valid JSON");
    assert_eq!(runtime_json["active_layout_id"], layout_id);
}

#[tokio::test]
async fn layout_apply_converges_a_concurrent_driver_runtime_update() {
    let revision = Arc::new(AtomicUsize::new(1));
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(RuntimeCacheTestDriver {
            revision: Arc::clone(&revision),
        })
        .expect("runtime cache test driver should register");
    let (state, _tmp) = isolated_state_with_driver_registry(Arc::new(registry));
    let state = Arc::new(state);
    let candidate = create_stored_layout(&state, "Converged Layout").await;
    let app = test_app_with_state(Arc::clone(&state));
    let update_revision = Arc::clone(&revision);

    let (response, _) = request_with_layout_ack_and_hook(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
        move || {
            let revision = Arc::clone(&update_revision);
            async move {
                revision.store(2, Ordering::Release);
            }
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["persistence_pending"], false);
    let persisted = runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state should exist");
    assert_eq!(
        persisted.active_layout_id.as_deref(),
        Some(candidate.id.as_str())
    );
    assert_eq!(
        state
            .driver_host()
            .driver_inventory()
            .driver_cache("runtime_cache_test")["revision"],
        serde_json::json!(2)
    );
}

#[tokio::test]
async fn layout_apply_returns_conflict_when_precommit_is_superseded() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let mut registry = DriverModuleRegistry::new();
    registry
        .register(BlockingRuntimeCacheTestDriver {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        })
        .expect("blocking runtime cache test driver should register");
    let (state, _tmp) = isolated_state_with_driver_registry(Arc::new(registry));
    let initial_layout_id = state.spatial_engine.snapshot().layout().id.clone();
    let state = Arc::new(state);
    let candidate = create_stored_layout(&state, "Superseded Layout").await;
    let runtime_state_path = state.runtime_state_path.clone();
    let concurrent_layout_id = initial_layout_id.clone();
    let superseding_write = tokio::spawn(async move {
        entered.notified().await;
        runtime_state::save(
            &runtime_state_path,
            &runtime_state::RuntimeSessionSnapshot {
                active_layout_id: Some(concurrent_layout_id),
                ..runtime_state::RuntimeSessionSnapshot::default()
            },
        )
        .expect("newer runtime snapshot should persist");
        release.add_permits(1);
    });
    let app = test_app_with_state(Arc::clone(&state));

    let (response, _) = request_with_layout_ack(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    superseding_write
        .await
        .expect("superseding runtime write should not panic");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        state.spatial_engine.snapshot().layout().id,
        initial_layout_id
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_apply_maps_renderer_rejections_to_explicit_statuses() {
    let cases = [
        (
            LayoutTransactionRejection::PreparationFailed {
                message: "invalid renderer plan".to_owned(),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (LayoutTransactionRejection::Superseded, StatusCode::CONFLICT),
        (
            LayoutTransactionRejection::RendererStopped,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];
    for (rejection, expected_status) in cases {
        let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
        let candidate = create_stored_layout(&state, "Rejected Apply").await;
        let app = test_app_with_state(Arc::clone(&state));

        let response = request_with_layout_rejection(
            app,
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
                .body(Body::empty())
                .expect("failed to build request"),
            &state,
            rejection,
        )
        .await;

        assert_eq!(response.status(), expected_status);
    }
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_preview_maps_renderer_rejections_to_explicit_statuses() {
    let cases = [
        (
            LayoutTransactionRejection::PreparationFailed {
                message: "invalid preview plan".to_owned(),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (LayoutTransactionRejection::Superseded, StatusCode::CONFLICT),
        (
            LayoutTransactionRejection::RendererStopped,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];
    for (rejection, expected_status) in cases {
        let state = Arc::new(isolated_state());
        let preview = SpatialLayout {
            id: "rejected-preview".to_owned(),
            name: "Rejected Preview".to_owned(),
            ..state.spatial_engine.snapshot().layout().as_ref().clone()
        };
        let app = test_app_with_state(Arc::clone(&state));

        let response = request_with_layout_rejection(
            app,
            Request::builder()
                .method("PUT")
                .uri("/api/v1/layouts/active/preview")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&preview).expect("preview should serialize"),
                ))
                .expect("failed to build request"),
            &state,
            rejection,
        )
        .await;

        assert_eq!(response.status(), expected_status);
    }
}

#[tokio::test]
async fn layout_apply_maps_persistence_failure_to_internal_error() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = tempdir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("temp data dir should be created");
    let state = AppStateBuilder::new(data_dir)
        .with_runtime_state_path(PathBuf::new())
        .build();
    let state = Arc::new(state);
    let candidate = create_stored_layout(&state, "Persistence Failure").await;
    let app = test_app_with_state(Arc::clone(&state));

    let (response, _) = request_with_layout_ack(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_apply_returns_accepted_when_convergence_retry_is_armed() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let candidate = SpatialLayout {
        id: "pending-layout".to_owned(),
        name: "Pending Layout".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(candidate.id.clone(), candidate.clone());
    let writer = AtomicFileWriter::new(&state.runtime_state_path)
        .expect("runtime state writer should initialize");
    let cleanup = InjectedWriterCleanup::new(writer);
    let app = test_app_with_state(Arc::clone(&state));
    let failure_writer = cleanup.writer().clone();

    let (response, _) = request_with_layout_ack_and_hook(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
        move || {
            let writer = failure_writer.clone();
            async move {
                writer.set_injected_replace_failures(1_000);
            }
        },
    )
    .await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let json = body_json(response).await;
    assert_eq!(json["data"]["applied"], true);
    assert_eq!(json["data"]["persistence_pending"], true);
    assert_eq!(state.spatial_engine.snapshot().layout().id, candidate.id);
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn concurrent_apply_and_delete_cannot_activate_a_removed_layout() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let candidate = SpatialLayout {
        id: "concurrent-apply-delete".to_owned(),
        name: "Concurrent Apply Delete".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(candidate.id.clone(), candidate.clone());
    let app = test_app_with_state(Arc::clone(&state));
    let first_entered = Arc::new(Notify::new());
    let release_first = Arc::new(Semaphore::new(0));
    let release_second = Arc::new(Semaphore::new(0));
    let renderer = tokio::spawn(run_two_layout_publications_with_gates(
        Arc::clone(&state),
        Arc::clone(&first_entered),
        Arc::clone(&release_first),
        Arc::clone(&release_second),
    ));
    let apply_id = candidate.id.clone();
    let apply_app = app.clone();
    let apply = tokio::spawn(async move {
        apply_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/layouts/{apply_id}/apply"))
                    .body(Body::empty())
                    .expect("failed to build apply request"),
            )
            .await
            .expect("failed to execute apply request")
    });
    tokio::time::timeout(Duration::from_secs(2), first_entered.notified())
        .await
        .expect("first publication should reach its gate");
    let delete_id = candidate.id.clone();
    let before_delete_guard = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::BeforeGuard,
        LayoutMutationTestOperation::Delete,
        &delete_id,
    );
    let delete = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{delete_id}"))
                .body(Body::empty())
                .expect("failed to build delete request"),
        )
        .await
        .expect("failed to execute delete request")
    });

    tokio::time::timeout(
        Duration::from_secs(2),
        before_delete_guard.wait_until_entered(),
    )
    .await
    .expect("delete should reach its guard");
    assert!(
        state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .contains_key(&candidate.id)
    );
    before_delete_guard.release();
    release_first.add_permits(1);
    release_second.add_permits(1);
    let apply_response = tokio::time::timeout(Duration::from_secs(5), apply)
        .await
        .expect("apply should converge after both renderer gates open")
        .expect("apply task should not panic");
    assert_eq!(apply_response.status(), StatusCode::OK);
    let delete_response = tokio::time::timeout(Duration::from_secs(5), delete)
        .await
        .expect("delete should converge after both renderer gates open")
        .expect("delete task should not panic");
    assert_eq!(delete_response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(5), renderer)
        .await
        .expect("layout publication worker should finish")
        .expect("layout publication worker should not panic");

    assert!(
        !state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .contains_key(&candidate.id)
    );
    assert_ne!(state.spatial_engine.snapshot().layout().id, candidate.id);
    let persisted = runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state should exist");
    assert_ne!(
        persisted.active_layout_id.as_deref(),
        Some(candidate.id.as_str())
    );
}

fn drain_layout_changes(
    events: &mut tokio::sync::broadcast::Receiver<hypercolor_core::bus::TimestampedEvent>,
) -> Vec<(Option<String>, String)> {
    let mut changes = Vec::new();
    while let Ok(timestamped) = events.try_recv() {
        if let HypercolorEvent::LayoutChanged { previous, current } = timestamped.event {
            changes.push((previous, current));
        }
    }
    changes
}

#[tokio::test]
async fn layout_apply_publishes_layout_changed_with_previous_and_current() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));
    let candidate = create_stored_layout(&state, "Apply Target").await;
    let previous_active = state.domains.layout.current().id;
    let mut events = state.event_bus.subscribe_all();

    let (apply_response, _) = request_with_layout_ack(
        app,
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    assert_eq!(apply_response.status(), StatusCode::OK);

    assert_eq!(
        drain_layout_changes(&mut events),
        vec![(Some(previous_active), candidate.id)],
        "an apply moves the active selection, so both ids are carried"
    );
}

#[tokio::test]
async fn layout_delete_publishes_layout_changed_for_the_removed_id() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));
    let candidate = create_stored_layout(&state, "Disposable").await;
    let mut events = state.event_bus.subscribe_all();

    let delete_response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{}", candidate.id))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    assert_eq!(
        drain_layout_changes(&mut events),
        vec![(None, candidate.id)],
        "deleting an inactive layout names it and leaves the active selection alone"
    );
}

#[tokio::test]
async fn layout_delete_of_the_active_layout_publishes_the_fallback_as_current() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));
    let candidate = create_stored_layout(&state, "Active Then Gone").await;

    let (apply_response, _) = request_with_layout_ack(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{}/apply", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    assert_eq!(apply_response.status(), StatusCode::OK);
    let mut events = state.event_bus.subscribe_all();

    let (delete_response, _) = request_with_layout_ack(
        app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/layouts/{}", candidate.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::OK);

    let fallback = state.domains.layout.current().id;
    assert_ne!(fallback, candidate.id);
    assert_eq!(
        drain_layout_changes(&mut events),
        vec![(Some(candidate.id), fallback)],
        "the deleted layout was active, so the fallback becomes current"
    );
}

#[tokio::test]
async fn layout_delete_active_falls_back_to_default_layout() {
    let state = Arc::new(isolated_state());
    let app = test_app_with_state(Arc::clone(&state));

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Cannot Delete Active"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(create_response.status(), StatusCode::CREATED);
    let create_json = body_json(create_response).await;
    let layout_id = create_json["data"]["id"]
        .as_str()
        .expect("id should be string")
        .to_owned();

    let (apply_response, _) = request_with_layout_ack(
        app.clone(),
        Request::builder()
            .method("POST")
            .uri(format!("/api/v1/layouts/{layout_id}/apply"))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    assert_eq!(apply_response.status(), StatusCode::OK);

    let (delete_response, _) = request_with_layout_ack(
        app.clone(),
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/layouts/{layout_id}"))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;
    assert_eq!(delete_response.status(), StatusCode::OK);
    let delete_json = body_json(delete_response).await;
    assert_eq!(delete_json["data"]["persistence_pending"], false);

    let active_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/layouts/active")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(active_response.status(), StatusCode::OK);
    let active_json = body_json(active_response).await;
    assert_eq!(active_json["data"]["id"], "default");
    assert_eq!(active_json["data"]["name"], "Default Layout");

    let runtime_raw = std::fs::read_to_string(&state.runtime_state_path)
        .expect("runtime state file should exist after delete");
    let runtime_json: serde_json::Value =
        serde_json::from_str(&runtime_raw).expect("runtime state should be valid JSON");
    assert_eq!(runtime_json["active_layout_id"], "default");
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn concurrent_active_and_fallback_deletes_cannot_publish_removed_fallback() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    let fallback = SpatialLayout {
        id: "fallback-delete-race".to_owned(),
        name: "Fallback Delete Race".to_owned(),
        ..active.clone()
    };
    {
        let mut layouts = state.domains.layout.test_fixture().catalog().write().await;
        layouts.insert(active.id.clone(), active.clone());
        layouts.insert(fallback.id.clone(), fallback.clone());
    }
    let app = test_app_with_state(Arc::clone(&state));
    let first_entered = Arc::new(Notify::new());
    let release_first = Arc::new(Semaphore::new(0));
    let release_second = Arc::new(Semaphore::new(0));
    let renderer = tokio::spawn(run_two_layout_publications_with_gates(
        Arc::clone(&state),
        Arc::clone(&first_entered),
        Arc::clone(&release_first),
        Arc::clone(&release_second),
    ));
    let active_id = active.id.clone();
    let first_app = app.clone();
    let first_delete = tokio::spawn(async move {
        first_app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/layouts/{active_id}"))
                    .body(Body::empty())
                    .expect("failed to build active delete request"),
            )
            .await
            .expect("failed to execute active delete request")
    });
    tokio::time::timeout(Duration::from_secs(2), first_entered.notified())
        .await
        .expect("first publication should reach its gate");
    let fallback_id = fallback.id.clone();
    let before_fallback_guard = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::BeforeGuard,
        LayoutMutationTestOperation::Delete,
        &fallback_id,
    );
    let fallback_delete = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{fallback_id}"))
                .body(Body::empty())
                .expect("failed to build fallback delete request"),
        )
        .await
        .expect("failed to execute fallback delete request")
    });

    tokio::time::timeout(
        Duration::from_secs(2),
        before_fallback_guard.wait_until_entered(),
    )
    .await
    .expect("fallback delete should reach its guard");
    assert!(
        state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .contains_key(&fallback.id)
    );
    before_fallback_guard.release();
    release_first.add_permits(1);
    release_second.add_permits(1);
    let first_response = tokio::time::timeout(Duration::from_secs(5), first_delete)
        .await
        .expect("active delete should converge after both renderer gates open")
        .expect("active delete task should not panic");
    assert_eq!(first_response.status(), StatusCode::OK);
    let fallback_response = tokio::time::timeout(Duration::from_secs(5), fallback_delete)
        .await
        .expect("fallback delete should converge after both renderer gates open")
        .expect("fallback delete task should not panic");
    assert_eq!(fallback_response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(5), renderer)
        .await
        .expect("layout publication worker should finish")
        .expect("layout publication worker should not panic");

    assert!(
        !state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .contains_key(&fallback.id)
    );
    assert_ne!(state.spatial_engine.snapshot().layout().id, fallback.id);
    let persisted = runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state should exist");
    assert_ne!(
        persisted.active_layout_id.as_deref(),
        Some(fallback.id.as_str())
    );
}

#[tokio::test]
async fn layout_preview_never_persists_runtime_state() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let preview = SpatialLayout {
        id: "preview-only".to_owned(),
        name: "Preview Only".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    let app = test_app_with_state(Arc::clone(&state));

    let (response, _) = request_with_layout_ack(
        app,
        Request::builder()
            .method("PUT")
            .uri("/api/v1/layouts/active/preview")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&preview).expect("preview layout should serialize"),
            ))
            .expect("failed to build request"),
        &state,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.spatial_engine.snapshot().layout().id, preview.id);
    assert!(!state.runtime_state_path.exists());
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_store_write_failure_rolls_back_create() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let initial_layouts = state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .read()
        .await
        .clone();
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Rejected Create"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        *state.domains.layout.test_fixture().catalog().read().await,
        initial_layouts
    );
    assert_eq!(
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load"),
        initial_layouts
    );
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_store_write_failure_rolls_back_update() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let stored = SpatialLayout {
        id: "failed-update".to_owned(),
        name: "Before Update".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(stored.id.clone(), stored.clone());
    persist_current_layouts_for_test(&state).await;
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{}", stored.id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Rejected Update"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        state.domains.layout.test_fixture().catalog().read().await[&stored.id].name,
        stored.name
    );
    let persisted =
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load");
    assert_eq!(persisted[&stored.id].name, stored.name);
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_store_write_failure_rolls_back_inactive_delete() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let stored = SpatialLayout {
        id: "failed-inactive-delete".to_owned(),
        name: "Failed Inactive Delete".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(stored.id.clone(), stored.clone());
    persist_current_layouts_for_test(&state).await;
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{}", stored.id))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .get(&stored.id),
        Some(&stored)
    );
    let persisted =
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load");
    assert_eq!(persisted.get(&stored.id), Some(&stored));
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_store_write_failure_rolls_back_active_delete() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    let fallback = SpatialLayout {
        id: "failed-active-delete-fallback".to_owned(),
        name: "Failed Active Delete Fallback".to_owned(),
        ..active.clone()
    };
    {
        let mut layouts = state.domains.layout.test_fixture().catalog().write().await;
        layouts.insert(active.id.clone(), active.clone());
        layouts.insert(fallback.id.clone(), fallback);
    }
    persist_current_layouts_for_test(&state).await;
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let app = test_app_with_state(Arc::clone(&state));

    let (response, applied) = request_with_layout_ack(
        app,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/layouts/{}", active.id))
            .body(Body::empty())
            .expect("failed to build request"),
        &state,
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(applied.len(), 2);
    assert_eq!(state.spatial_engine.snapshot().layout().id, active.id);
    assert_eq!(
        state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .get(&active.id),
        Some(&active)
    );
    let persisted =
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load");
    assert_eq!(persisted.get(&active.id), Some(&active));
    let runtime = runtime_state::load(&state.runtime_state_path)
        .expect("runtime state should load")
        .expect("runtime state should exist");
    assert_eq!(
        runtime.active_layout_id.as_deref(),
        Some(active.id.as_str())
    );
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_create() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let barrier = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::Create,
        "Cancellation Create",
    );
    let app = test_app_with_state(Arc::clone(&state));
    let request = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Cancellation Create"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
    });
    barrier.wait_until_entered().await;
    let created_id = state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .read()
        .await
        .values()
        .find(|layout| layout.name == "Cancellation Create")
        .expect("create workflow should mutate memory before the barrier")
        .id
        .clone();

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    barrier.release();
    let layouts_path = state
        .domains
        .layout
        .test_fixture()
        .catalog_path()
        .to_path_buf();
    let durable_id = created_id.clone();
    wait_for_async_condition(move || {
        let layouts_path = layouts_path.clone();
        let durable_id = durable_id.clone();
        async move {
            hypercolor_daemon::layout_store::load(&layouts_path)
                .is_ok_and(|layouts| layouts.contains_key(&durable_id))
        }
    })
    .await;

    assert!(
        state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .read()
            .await
            .contains_key(&created_id)
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_update() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let stored = SpatialLayout {
        id: "cancellation-update".to_owned(),
        name: "Before Cancellation Update".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(stored.id.clone(), stored.clone());
    persist_current_layouts_for_test(&state).await;
    let barrier = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::Update,
        &stored.id,
    );
    let app = test_app_with_state(Arc::clone(&state));
    let update_id = stored.id.clone();
    let request = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{update_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"After Cancellation Update"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
    });
    barrier.wait_until_entered().await;

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    barrier.release();
    let layouts_path = state
        .domains
        .layout
        .test_fixture()
        .catalog_path()
        .to_path_buf();
    let durable_id = stored.id.clone();
    wait_for_async_condition(move || {
        let layouts_path = layouts_path.clone();
        let durable_id = durable_id.clone();
        async move {
            hypercolor_daemon::layout_store::load(&layouts_path).is_ok_and(|layouts| {
                layouts
                    .get(&durable_id)
                    .is_some_and(|layout| layout.name == "After Cancellation Update")
            })
        }
    })
    .await;

    assert_eq!(
        state.domains.layout.test_fixture().catalog().read().await[&stored.id].name,
        "After Cancellation Update"
    );
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_apply_convergence() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let candidate = SpatialLayout {
        id: "cancellation-apply".to_owned(),
        name: "Cancellation Apply".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(candidate.id.clone(), candidate.clone());
    let app = test_app_with_state(Arc::clone(&state));
    let publication_entered = Arc::new(Notify::new());
    let release_publication = Arc::new(Semaphore::new(0));
    let renderer = tokio::spawn(run_one_layout_publication_with_gate(
        Arc::clone(&state),
        Arc::clone(&publication_entered),
        Arc::clone(&release_publication),
    ));
    let apply_id = candidate.id.clone();
    let request = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/layouts/{apply_id}/apply"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
    });
    publication_entered.notified().await;

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    release_publication.add_permits(1);
    renderer
        .await
        .expect("layout publication worker should not panic");
    let durable_state = Arc::clone(&state);
    let durable_id = candidate.id.clone();
    wait_for_async_condition(move || {
        let state = Arc::clone(&durable_state);
        let durable_id = durable_id.clone();
        async move {
            if state.spatial_engine.snapshot().layout().id != durable_id {
                return false;
            }
            runtime_state::load(&state.runtime_state_path)
                .ok()
                .flatten()
                .and_then(|snapshot| snapshot.active_layout_id)
                .as_deref()
                == Some(durable_id.as_str())
        }
    })
    .await;
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_delete() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    let fallback = SpatialLayout {
        id: "cancellation-delete-fallback".to_owned(),
        name: "Cancellation Delete Fallback".to_owned(),
        ..active.clone()
    };
    {
        let mut layouts = state.domains.layout.test_fixture().catalog().write().await;
        layouts.insert(active.id.clone(), active.clone());
        layouts.insert(fallback.id.clone(), fallback.clone());
    }
    persist_current_layouts_for_test(&state).await;
    let app = test_app_with_state(Arc::clone(&state));
    let publication_entered = Arc::new(Notify::new());
    let release_publication = Arc::new(Semaphore::new(0));
    let renderer = tokio::spawn(run_one_layout_publication_with_gate(
        Arc::clone(&state),
        Arc::clone(&publication_entered),
        Arc::clone(&release_publication),
    ));
    let active_id = active.id.clone();
    let request = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{active_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
    });
    publication_entered.notified().await;

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    release_publication.add_permits(1);
    renderer
        .await
        .expect("layout publication worker should not panic");
    let durable_state = Arc::clone(&state);
    let removed_id = active.id.clone();
    let durable_id = fallback.id.clone();
    wait_for_async_condition(move || {
        let state = Arc::clone(&durable_state);
        let removed_id = removed_id.clone();
        let durable_id = durable_id.clone();
        async move {
            if state
                .domains
                .layout
                .test_fixture()
                .catalog()
                .read()
                .await
                .contains_key(&removed_id)
                || state.spatial_engine.snapshot().layout().id != durable_id
            {
                return false;
            }
            let layouts_are_durable = hypercolor_daemon::layout_store::load(
                state.domains.layout.test_fixture().catalog_path(),
            )
            .is_ok_and(|layouts| {
                !layouts.contains_key(&removed_id) && layouts.contains_key(&durable_id)
            });
            let runtime_is_durable = runtime_state::load(&state.runtime_state_path)
                .ok()
                .flatten()
                .and_then(|snapshot| snapshot.active_layout_id)
                .as_deref()
                == Some(durable_id.as_str());
            layouts_are_durable && runtime_is_durable
        }
    })
    .await;
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_preview_connectivity_sync() {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    register_noop_backend(&state, "wled", "WLED").await;
    let repair_device_id = insert_test_device(&state, "Preview Repair Target").await;
    state
        .device_registry
        .set_state(&repair_device_id, DeviceState::Connected)
        .await;
    let repair_layout_device_id = seed_stale_auto_layout_zone(&state, &repair_device_id).await;
    let preview = SpatialLayout {
        id: "cancellation-preview".to_owned(),
        name: "Cancellation Preview".to_owned(),
        ..state.spatial_engine.snapshot().layout().as_ref().clone()
    };
    let after_renderer = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterRendererMutation,
        LayoutMutationTestOperation::Preview,
        &preview.id,
    );
    let after_workflow = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterWorkflow,
        LayoutMutationTestOperation::Preview,
        &preview.id,
    );
    let app = test_app_with_state(Arc::clone(&state));
    let renderer = tokio::spawn(run_layout_publications(Arc::clone(&state), 2));
    let request = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/layouts/active/preview")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&preview).expect("preview should serialize"),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
    });
    after_renderer.wait_until_entered().await;

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    after_renderer.release();
    after_workflow.wait_until_entered().await;
    assert_eq!(
        state.spatial_engine.snapshot().layout().id,
        "cancellation-preview"
    );
    assert!(
        state
            .spatial_engine
            .snapshot()
            .layout()
            .zones
            .iter()
            .any(|output| {
                output.device_id == repair_layout_device_id
                    && output.name == "Preview Repair Target"
            })
    );
    after_workflow.release();
    assert_eq!(
        renderer
            .await
            .expect("layout publication worker should not panic")
            .len(),
        2
    );
}

#[cfg(feature = "persistence-test-hooks")]
async fn assert_auto_layout_store_failure_rolls_back(saved_layout_present: bool) {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    let device_id = insert_test_device(&state, "Failed Auto Layout Repair").await;
    state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    seed_stale_auto_layout_zone(&state, &device_id).await;
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    if saved_layout_present {
        state
            .domains
            .layout
            .test_fixture()
            .catalog()
            .write()
            .await
            .insert(active.id.clone(), active.clone());
    }
    persist_current_layouts_for_test(&state).await;
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let mut events = state.event_bus.subscribe_all();
    let renderer = tokio::spawn(run_layout_publications(Arc::clone(&state), 2));

    let runtime = state.driver_host().discovery_runtime();
    runtime
        .layout
        .test_workflows()
        .sync_active_layout_for_renderable_devices(runtime.clone(), None)
        .await;

    let applied = renderer
        .await
        .expect("layout publication worker should not panic");
    assert_eq!(applied.len(), 2);
    assert!(
        drain_layout_changes(&mut events).is_empty(),
        "a rolled-back repair never reached the store, so it publishes nothing"
    );
    assert!(!applied[0].zones.is_empty());
    assert_eq!(applied[1], active);
    assert_eq!(state.spatial_engine.snapshot().layout().as_ref(), &active);
    let layouts = state.domains.layout.test_fixture().catalog().read().await;
    assert_eq!(
        layouts.get(&active.id),
        saved_layout_present.then_some(&active)
    );
    drop(layouts);
    let persisted =
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load");
    let persisted_active = persisted.get(&active.id).map(|layout| {
        hypercolor_core::spatial::SpatialEngine::try_new(layout.clone())
            .expect("persisted layout should rebuild")
            .layout()
            .as_ref()
            .clone()
    });
    assert_eq!(
        persisted_active.as_ref(),
        saved_layout_present.then_some(&active)
    );
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_auto_repair_publishes_layout_changed_for_the_active_layout() {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    let device_id = insert_test_device(&state, "Repaired Auto Layout").await;
    state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    seed_stale_auto_layout_zone(&state, &device_id).await;
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(active.id.clone(), active.clone());
    persist_current_layouts_for_test(&state).await;
    let mut events = state.event_bus.subscribe_all();
    let renderer = tokio::spawn(run_layout_publications(Arc::clone(&state), 1));

    let runtime = state.driver_host().discovery_runtime();
    runtime
        .layout
        .test_workflows()
        .sync_active_layout_for_renderable_devices(runtime.clone(), None)
        .await;

    let applied = renderer
        .await
        .expect("layout publication worker should not panic");
    assert_eq!(applied.len(), 1);
    assert!(!applied[0].zones.is_empty());
    assert_eq!(
        drain_layout_changes(&mut events),
        vec![(None, active.id)],
        "a persisted repair names the active layout it rewrote"
    );
}

#[cfg(feature = "persistence-test-hooks")]
fn injected_layout_store_failure(state: &AppState) -> InjectedWriterCleanup {
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    cleanup
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_create_store_failure_publishes_nothing() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let mut events = state.event_bus.subscribe_all();
    let cleanup = injected_layout_store_failure(&state);

    let result = state
        .domains
        .layout
        .create(hypercolor_types::api::layouts::CreateLayoutRequest {
            name: "Doomed".to_owned(),
            ..Default::default()
        })
        .await;

    assert!(result.is_err(), "a failed store write rejects the create");
    assert!(drain_layout_changes(&mut events).is_empty());
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_update_store_failure_publishes_nothing() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let kept = create_stored_layout(&state, "Kept").await;
    let mut events = state.event_bus.subscribe_all();
    let cleanup = injected_layout_store_failure(&state);

    let result = state
        .domains
        .layout
        .update(
            kept.id.clone(),
            hypercolor_types::api::layouts::UpdateLayoutRequest {
                name: Some("Renamed".to_owned()),
                ..Default::default()
            },
        )
        .await;

    assert!(result.is_err(), "a failed store write rejects the update");
    assert!(drain_layout_changes(&mut events).is_empty());
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_delete_store_failure_publishes_nothing() {
    let (state, _tmp) = test_state_with_temp_layout_and_runtime_store();
    let kept = create_stored_layout(&state, "Kept").await;
    let app = test_app_with_state(Arc::clone(&state));
    let mut events = state.event_bus.subscribe_all();
    let cleanup = injected_layout_store_failure(&state);

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{}", kept.id))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(drain_layout_changes(&mut events).is_empty());
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_auto_repair_store_failure_restores_saved_layout() {
    assert_auto_layout_store_failure_rolls_back(true).await;
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_auto_repair_store_failure_preserves_absent_saved_layout() {
    assert_auto_layout_store_failure_rolls_back(false).await;
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_config_canvas_resize() {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(active.id.clone(), active.clone());
    persist_current_layouts_for_test(&state).await;
    let configured_height = state
        .config_manager()
        .expect("config manager should exist")
        .get()
        .daemon
        .canvas_height;
    let reference = format!("1024x{configured_height}");
    let after_memory = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::ConfigResize,
        &reference,
    );
    let after_workflow = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterWorkflow,
        LayoutMutationTestOperation::ConfigResize,
        &reference,
    );
    let app = test_app_with_state(Arc::clone(&state));
    let request_state = Arc::clone(&state);
    let request = tokio::spawn(async move {
        request_with_layout_ack(
            app,
            config_put_request("daemon.canvas_width", &serde_json::json!(1024), None),
            &request_state,
        )
        .await
        .0
    });
    after_memory.wait_until_entered().await;

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    after_memory.release();
    after_workflow.wait_until_entered().await;
    assert_eq!(
        state.domains.layout.test_fixture().catalog().read().await[&active.id].canvas_width,
        1024
    );
    assert_eq!(
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load")[&active.id]
            .canvas_width,
        1024
    );
    after_workflow.release();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_mutation_cancellation_finishes_simulator_pruning() {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    let device_id = DeviceId::new();
    state
        .simulated_displays
        .write()
        .await
        .upsert(SimulatedDisplayConfig {
            id: device_id,
            name: "Cancellation Simulator".to_owned(),
            width: 16,
            height: 16,
            circular: false,
            enabled: true,
        });
    let mut stored = state.spatial_engine.snapshot().layout().as_ref().clone();
    stored.id = "cancellation-simulator-prune".to_owned();
    stored.name = "Cancellation Simulator Prune".to_owned();
    stored.zones = vec![simulator_target_output(device_id)];
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(stored.id.clone(), stored.clone());
    persist_current_layouts_for_test(&state).await;
    let after_memory = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::SimulatorPrune,
        device_id.to_string(),
    );
    let after_workflow = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterWorkflow,
        LayoutMutationTestOperation::SimulatorPrune,
        device_id.to_string(),
    );
    let app = test_app_with_state(Arc::clone(&state));
    let request = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/simulators/displays/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request")
    });
    after_memory.wait_until_entered().await;

    request.abort();
    assert!(
        request
            .await
            .expect_err("request task should be cancelled")
            .is_cancelled()
    );
    after_memory.release();
    after_workflow.wait_until_entered().await;
    assert!(
        state.domains.layout.test_fixture().catalog().read().await[&stored.id]
            .zones
            .is_empty()
    );
    assert!(
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load")[&stored.id]
            .zones
            .is_empty()
    );
    assert!(
        state
            .simulated_displays
            .read()
            .await
            .get(device_id)
            .is_none()
    );
    after_workflow.release();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_update_compensation_cannot_erase_config_canvas_resize() {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(active.id.clone(), active.clone());
    persist_current_layouts_for_test(&state).await;
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let update_after_memory = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::Update,
        &active.id,
    );
    let configured_height = state
        .config_manager()
        .expect("config manager should exist")
        .get()
        .daemon
        .canvas_height;
    let resize_reference = format!("1024x{configured_height}");
    let resize_before_guard = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::BeforeGuard,
        LayoutMutationTestOperation::ConfigResize,
        &resize_reference,
    );
    let resize_after_memory = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::ConfigResize,
        &resize_reference,
    );
    let app = test_app_with_state(Arc::clone(&state));
    let update_app = app.clone();
    let update_id = active.id.clone();
    let update = tokio::spawn(async move {
        update_app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/layouts/{update_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Rejected Update"}"#))
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute update request")
    });
    update_after_memory.wait_until_entered().await;
    let resize_state = Arc::clone(&state);
    let resize = tokio::spawn(async move {
        request_with_layout_ack(
            app,
            config_put_request("daemon.canvas_width", &serde_json::json!(1024), None),
            &resize_state,
        )
        .await
        .0
    });
    resize_before_guard.wait_until_entered().await;
    resize_before_guard.release();
    update_after_memory.release();
    assert_eq!(
        update.await.expect("update task should not panic").status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    resize_after_memory.wait_until_entered().await;
    {
        let layouts = state.domains.layout.test_fixture().catalog().read().await;
        assert_eq!(layouts[&active.id].name, active.name);
        assert_eq!(layouts[&active.id].canvas_width, 1024);
    }
    resize_after_memory.release();
    assert_eq!(
        resize.await.expect("resize task should not panic").status(),
        StatusCode::OK
    );
    let persisted =
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load");
    assert_eq!(persisted[&active.id].name, active.name);
    assert_eq!(persisted[&active.id].canvas_width, 1024);
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_update_compensation_cannot_erase_simulator_pruning() {
    let (state, _tmp) = test_state_with_temp_layout_config_and_simulator_stores();
    let device_id = DeviceId::new();
    state
        .simulated_displays
        .write()
        .await
        .upsert(SimulatedDisplayConfig {
            id: device_id,
            name: "Collision Simulator".to_owned(),
            width: 16,
            height: 16,
            circular: false,
            enabled: true,
        });
    let mut stored = state.spatial_engine.snapshot().layout().as_ref().clone();
    stored.id = "simulator-prune-collision".to_owned();
    stored.name = "Simulator Prune Collision".to_owned();
    stored.zones = vec![simulator_target_output(device_id)];
    state
        .domains
        .layout
        .test_fixture()
        .catalog()
        .write()
        .await
        .insert(stored.id.clone(), stored.clone());
    persist_current_layouts_for_test(&state).await;
    let cleanup = InjectedWriterCleanup::new(
        AtomicFileWriter::new(state.domains.layout.test_fixture().catalog_path())
            .expect("layout writer should initialize"),
    );
    cleanup.writer().set_injected_replace_failures(1);
    let update_after_memory = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::Update,
        &stored.id,
    );
    let prune_before_guard = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::BeforeGuard,
        LayoutMutationTestOperation::SimulatorPrune,
        device_id.to_string(),
    );
    let prune_after_memory = state.domains.layout.test_fixture().hooks().install(
        LayoutMutationTestPoint::AfterMemoryMutation,
        LayoutMutationTestOperation::SimulatorPrune,
        device_id.to_string(),
    );
    let app = test_app_with_state(Arc::clone(&state));
    let update_app = app.clone();
    let update_id = stored.id.clone();
    let update = tokio::spawn(async move {
        update_app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/layouts/{update_id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Rejected Update"}"#))
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute update request")
    });
    update_after_memory.wait_until_entered().await;
    let pruning = tokio::spawn(async move {
        app.oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/simulators/displays/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute simulator delete request")
    });
    prune_before_guard.wait_until_entered().await;
    prune_before_guard.release();
    update_after_memory.release();
    assert_eq!(
        update.await.expect("update task should not panic").status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    prune_after_memory.wait_until_entered().await;
    {
        let layouts = state.domains.layout.test_fixture().catalog().read().await;
        assert_eq!(layouts[&stored.id].name, stored.name);
        assert!(layouts[&stored.id].zones.is_empty());
    }
    prune_after_memory.release();
    assert_eq!(
        pruning
            .await
            .expect("pruning task should not panic")
            .status(),
        StatusCode::OK
    );
    let persisted =
        hypercolor_daemon::layout_store::load(state.domains.layout.test_fixture().catalog_path())
            .expect("layout store should load");
    assert_eq!(persisted[&stored.id].name, stored.name);
    assert!(persisted[&stored.id].zones.is_empty());
    cleanup.reset_and_flush();
}

#[cfg(feature = "persistence-test-hooks")]
#[tokio::test]
async fn layout_delete_rolls_back_when_the_fallback_plan_is_rejected() {
    let state = Arc::new(isolated_state());
    let active = state.spatial_engine.snapshot().layout().as_ref().clone();
    let mut invalid = layout_with_sampling_modes(
        SamplingMode::Bilinear,
        SamplingMode::GaussianArea {
            sigma: 1.0,
            radius: u32::MAX,
        },
    );
    invalid.id = "invalid-fallback".to_owned();
    invalid.name = "Invalid Fallback".to_owned();
    {
        let mut layouts = state.domains.layout.test_fixture().catalog().write().await;
        layouts.insert(active.id.clone(), active.clone());
        layouts.insert(invalid.id.clone(), invalid.clone());
    }
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/layouts/{}", active.id))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(state.spatial_engine.snapshot().layout().as_ref(), &active);
    let layouts = state.domains.layout.test_fixture().catalog().read().await;
    assert_eq!(layouts.get(&active.id), Some(&active));
    assert_eq!(layouts.get(&invalid.id), Some(&invalid));
    assert_eq!(
        state
            .layout_publication_test_executor()
            .pending_layout_publications(),
        0
    );
}

#[tokio::test]
async fn layout_create_validates_input() {
    let app = test_app();

    let empty_name_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from("{\"name\":\"   \"}"))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        empty_name_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_canvas_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/layouts")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Bad","canvas_width":0}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        invalid_canvas_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn layout_update_rejects_negative_output_sampling_radii_without_mutating() {
    let state = Arc::new(isolated_state());
    let stored = create_stored_layout(&state, "Negative Radius Target").await;

    let invalid = layout_with_sampling_modes(
        SamplingMode::Bilinear,
        SamplingMode::AreaAverage {
            radius_x: -1.0,
            radius_y: 1.0,
        },
    );
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{}", stored.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "zones": invalid.zones }).to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert!(
        state
            .domains
            .layout
            .resolve(&stored.id)
            .await
            .expect("stored layout should resolve")
            .zones
            .is_empty()
    );
}

#[tokio::test]
async fn layout_update_rejects_unaddressable_gaussian_without_mutating() {
    let state = Arc::new(isolated_state());
    let stored = create_stored_layout(&state, "Gaussian Radius Target").await;
    let invalid = layout_with_sampling_modes(
        SamplingMode::Bilinear,
        SamplingMode::GaussianArea {
            sigma: 1.0,
            radius: u32::MAX,
        },
    );
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{}", stored.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "zones": invalid.zones }).to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        state
            .domains
            .layout
            .resolve(&stored.id)
            .await
            .expect("stored layout should resolve"),
        stored
    );
    assert_eq!(
        state
            .layout_publication_test_executor()
            .pending_layout_publications(),
        0
    );
}

#[tokio::test]
async fn layout_update_rejects_invalid_geometry_without_mutating() {
    let state = Arc::new(isolated_state());
    let stored = create_stored_layout(&state, "Invalid Geometry Target").await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/layouts/{}", stored.id))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "name": "Poisoned",
                        "canvas_width": u32::MAX,
                        "canvas_height": u32::MAX,
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        state
            .domains
            .layout
            .resolve(&stored.id)
            .await
            .expect("stored layout should resolve"),
        stored
    );
}

#[tokio::test]
async fn layout_preview_rejects_invalid_sampling_radii_without_mutating() {
    let state = Arc::new(isolated_state());
    let original_layout_id = state.spatial_engine.snapshot().layout().id.clone();
    let app = test_app_with_state(Arc::clone(&state));
    let negative = SamplingMode::AreaAverage {
        radius_x: -1.0,
        radius_y: 1.0,
    };
    let invalid_layouts = [
        layout_with_sampling_modes(negative.clone(), SamplingMode::Bilinear),
        layout_with_sampling_modes(SamplingMode::Bilinear, negative),
    ];

    for layout in invalid_layouts {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/v1/layouts/active/preview")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&layout).expect("layout should serialize"),
                    ))
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    for radius in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let layout = layout_with_sampling_modes(
            SamplingMode::AreaAverage {
                radius_x: radius,
                radius_y: 0.0,
            },
            SamplingMode::Bilinear,
        );
        let response = api::layouts::preview_layout(
            axum::extract::State(Arc::clone(&state)),
            axum::Json(layout),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    for sigma in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let layout = layout_with_sampling_modes(
            SamplingMode::GaussianArea { sigma, radius: 1 },
            SamplingMode::Bilinear,
        );
        let response = api::layouts::preview_layout(
            axum::extract::State(Arc::clone(&state)),
            axum::Json(layout),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    assert_eq!(
        state.spatial_engine.snapshot().layout().id,
        original_layout_id
    );
}

#[tokio::test]
async fn layout_preview_rejects_invalid_geometry_without_mutating() {
    let state = Arc::new(isolated_state());
    let original = state.spatial_engine.snapshot().layout().as_ref().clone();

    for (width, height) in [(0, original.canvas_height), (u32::MAX, u32::MAX)] {
        let mut invalid = original.clone();
        invalid.id = format!("invalid-{width}-{height}");
        invalid.canvas_width = width;
        invalid.canvas_height = height;

        let response = api::layouts::preview_layout(
            axum::extract::State(Arc::clone(&state)),
            axum::Json(invalid),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(state.spatial_engine.snapshot().layout().as_ref(), &original);
    }
}

// ── Effect Layout Associations ──────────────────────────────────────────

fn test_state_with_temp_layout_and_runtime_store() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("test data directory should be created");
    let state = AppStateBuilder::new(data_dir)
        .with_runtime_state_path(dir.path().join("runtime-state.json"))
        .build();
    (Arc::new(state), dir)
}

#[cfg(feature = "persistence-test-hooks")]
fn test_state_with_temp_layout_config_and_simulator_stores() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir should be created");
    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("test data directory should be created");
    let config_manager = Arc::new(
        ConfigManager::new(dir.path().join("hypercolor.toml"))
            .expect("config manager should initialize"),
    );
    let state = AppStateBuilder::new(data_dir)
        .with_config_manager(config_manager)
        .with_runtime_state_path(dir.path().join("runtime-state.json"))
        .build();
    (Arc::new(state), dir)
}

#[cfg(feature = "persistence-test-hooks")]
fn simulator_target_output(device_id: DeviceId) -> Output {
    Output {
        id: "simulator-output".to_owned(),
        name: "Simulator Output".to_owned(),
        device_id: device_id.to_string(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Point,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

#[cfg(feature = "persistence-test-hooks")]
async fn persist_current_layouts_for_test(state: &Arc<AppState>) {
    let layouts = state.domains.layout.test_fixture().catalog().read().await;
    let mut entries = layouts.values().collect::<Vec<_>>();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    let payload = serde_json::to_vec_pretty(&entries).expect("layout fixture should serialize");
    std::fs::write(state.domains.layout.test_fixture().catalog_path(), payload)
        .expect("layout fixture should write");
}

fn test_state_with_temp_output_store() -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir should be created");
    let state = AppState::new_with_data_dir(dir.path().to_path_buf());
    (Arc::new(state), dir)
}

#[tokio::test]
async fn apply_effect_rejects_display_face_effects() {
    let state = Arc::new(isolated_state());
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/effects/{}/apply", face.id))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn activating_named_scene_then_applying_effect_mutates_named_scene() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Sunset").await;
    let app = test_app_with_state(Arc::clone(&state));
    let named_scene_id = activate_empty_test_scene(&state, "Focus").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Sunset/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let manager = state.scene_manager.snapshot().await;
    let default_scene = manager
        .get(&SceneId::DEFAULT)
        .expect("default scene should still exist");
    let default_primary = default_scene
        .primary_zone()
        .expect("default scene should keep its Default zone");
    assert!(
        default_primary.effect_ids().next().is_none(),
        "default scene should not be mutated while a named scene is active"
    );

    let active_scene = manager
        .active_scene()
        .expect("named scene should stay active");
    assert_eq!(active_scene.id, named_scene_id);
    assert!(
        active_scene
            .primary_zone()
            .and_then(|zone| zone.effect_ids().next())
            .is_some()
    );
}

#[tokio::test]
async fn apply_effect_conflicts_when_snapshot_scene_is_active() {
    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "Aurora").await;
    activate_empty_test_scene_with_mode(&state, "Focus", SceneMutationMode::Snapshot).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/effects/Aurora/apply")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("snapshot mode"),
    );

    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .active_scene()
            .and_then(Scene::primary_zone)
            .is_none(),
        "snapshot scene should not be rewritten by effect apply",
    );
}

// ── Error Envelope Format ────────────────────────────────────────────────

#[tokio::test]
async fn error_responses_have_correct_envelope() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/scenes/nonexistent")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let json = body_json(response).await;

    // Error envelope must have `error` and `meta` at top level.
    assert!(json["error"].is_object(), "error key should be an object");
    assert!(json["meta"].is_object(), "meta key should be an object");

    // Error object must have `code` and `message`.
    assert_eq!(json["error"]["code"], "scene_not_found");
    assert!(
        json["error"]["message"].is_string(),
        "error.message should be a string"
    );

    // Meta must have `api_version`, `request_id`, and `timestamp`.
    assert_eq!(json["meta"]["api_version"], "1.0");
    assert!(
        json["meta"]["request_id"]
            .as_str()
            .expect("request_id should be string")
            .starts_with("req_"),
        "request_id should start with req_"
    );
    assert!(
        json["meta"]["timestamp"].is_string(),
        "timestamp should be a string"
    );
}

#[tokio::test]
async fn success_responses_have_correct_envelope() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;

    // Success envelope must have `data` and `meta` at top level.
    assert!(json["data"].is_object(), "data key should be an object");
    assert!(json["meta"].is_object(), "meta key should be an object");

    // Meta must have correct fields.
    assert_eq!(json["meta"]["api_version"], "1.0");
    assert!(json["meta"]["request_id"].is_string());
    assert!(json["meta"]["timestamp"].is_string());
}

// ── Device Discovery (no body) ──────────────────────────────────────────

#[tokio::test]
async fn discover_devices_without_body() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn discover_devices_rejects_unknown_target() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"targets": ["mystery"], "timeout_ms": 5000}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn discover_devices_returns_conflict_when_scan_active() {
    let state = Arc::new(isolated_state());
    state.discovery_in_progress.store(true, Ordering::Release);
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/discover")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
}

// ── Device Identify ──────────────────────────────────────────────────────

#[tokio::test]
async fn identify_device_not_found() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/00000000-0000-0000-0000-000000000000/identify")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_device_persists_name_enabled_and_brightness_state() {
    let (state, _tmp) = test_state_with_temp_output_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let update_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Desk Strip Renamed","enabled":false,"brightness":27}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(update_response.status(), StatusCode::OK);
    let update_json = body_json(update_response).await;
    assert_eq!(update_json["data"]["name"], "Desk Strip Renamed");
    assert_eq!(update_json["data"]["status"], "disabled");
    assert_eq!(update_json["data"]["brightness"], 27);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["name"], "Desk Strip Renamed");
    assert_eq!(get_json["data"]["status"], "disabled");
    assert_eq!(get_json["data"]["brightness"], 27);

    let persisted_raw = fs::read_to_string(state.state_dir.join("device-settings.json"))
        .expect("device settings file should exist");
    let persisted_json: serde_json::Value =
        serde_json::from_str(&persisted_raw).expect("device settings file should be valid json");
    let settings_key =
        hypercolor_daemon::device_settings::device_settings_keys(&state.device_registry, device_id)
            .await
            .canonical;
    let persisted_device = &persisted_json["devices"][settings_key.as_str()];
    assert_eq!(persisted_device["name"], "Desk Strip Renamed");
    assert_eq!(persisted_device["disabled"], true);
    assert_eq!(persisted_device["brightness"], serde_json::json!(0.27));

    let reenable_response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(reenable_response.status(), StatusCode::OK);
    let reenable_json = body_json(reenable_response).await;
    assert_eq!(reenable_json["data"]["status"], "known");
    assert_eq!(reenable_json["data"]["brightness"], 27);
}

#[tokio::test]
async fn update_device_enable_activates_layout_targeted_deferred_device() {
    let state = Arc::new(isolated_state());
    register_noop_backend(&state, "wled", "WLED Test Backend").await;

    let device_id = DeviceId::new();
    let info = DeviceInfo {
        id: device_id,
        name: "Studio Strip".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 60,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let fingerprint = DeviceFingerprint::from_persisted("wled:studio-strip".to_owned());
    state
        .device_registry
        .add_discovered(DiscoveredDevice {
            fingerprint: fingerprint.clone(),
            connect_behavior: DiscoveryConnectBehavior::Deferred,
            info: info.clone(),
            metadata: HashMap::new(),
            claim: None,
        })
        .await;
    state
        .device_registry
        .update_user_settings(&device_id, None, Some(false), None)
        .await
        .expect("device settings should update");
    state
        .device_registry
        .set_state(&device_id, DeviceState::Disabled)
        .await;

    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&info, Some(&fingerprint));
    set_layout_targeting_device(&state, &layout_device_id, 60).await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":true}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "connected");

    let routing = state.backend_manager.lock().await.routing_snapshot();
    assert!(
        routing.mappings.iter().any(|entry| {
            entry.backend_id == "wled"
                && entry.device_id == device_id.to_string()
                && entry.layout_device_id == layout_device_id
        }),
        "re-enabled layout-targeted device should be mapped for rendering"
    );
}

#[tokio::test]
async fn get_device_controls_returns_host_control_surface() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}/controls"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let data = &json["data"];
    assert_eq!(data["surface_id"], format!("device:{device_id}"));
    assert_eq!(data["schema_version"], 1);
    assert_eq!(data["scope"]["device"]["driver_id"], "wled");
    assert_eq!(data["values"]["name"]["kind"], "text");
    assert_eq!(data["values"]["name"]["value"], "Desk Strip");
    assert_eq!(data["values"]["enabled"]["kind"], "bool");
    assert_eq!(data["values"]["enabled"]["value"], true);
    assert_eq!(data["values"]["brightness"]["kind"], "float");
    assert_eq!(data["values"]["brightness"]["value"], 1.0);
    assert_eq!(data["availability"]["name"]["state"], "available");
    assert_eq!(
        data["action_availability"]["identify"]["state"],
        "available"
    );

    let fields = data["fields"]
        .as_array()
        .expect("fields should be an array");
    assert!(fields.iter().any(|field| field["id"] == "name"));
    assert!(fields.iter().any(|field| field["id"] == "enabled"));
    assert!(fields.iter().any(|field| field["id"] == "brightness"));
    let actions = data["actions"]
        .as_array()
        .expect("actions should be an array");
    let identify = actions
        .iter()
        .find(|action| action["id"] == "identify")
        .expect("identify action should be exposed");
    assert_eq!(identify["owner"], "host");
    assert_eq!(identify["group_id"], "diagnostics");
    assert_eq!(identify["apply_impact"], "live");
    assert_eq!(identify["input_fields"][0]["id"], "duration_ms");
    assert_eq!(
        identify["input_fields"][0]["value_type"]["kind"],
        "duration_ms"
    );
    assert_eq!(
        identify["input_fields"][0]["default_value"]["kind"],
        "duration"
    );
    assert_eq!(identify["input_fields"][0]["default_value"]["value"], 3000);
    assert_eq!(identify["input_fields"][1]["id"], "color");
    assert_eq!(
        identify["input_fields"][1]["value_type"]["kind"],
        "color_rgb"
    );
}

#[tokio::test]
async fn patch_device_control_surface_updates_user_settings() {
    let (state, _tmp) = test_state_with_temp_output_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let surface_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}/controls"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(surface_response.status(), StatusCode::OK);
    let surface_json = body_json(surface_response).await;
    let revision = surface_json["data"]["revision"]
        .as_u64()
        .expect("revision should be an integer");
    let body = serde_json::json!({
        "values": {
            "name": { "kind": "text", "value": "Desk Strip Controls" },
            "enabled": { "kind": "bool", "value": false },
            "brightness": { "kind": "float", "value": 0.5 }
        }
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["previous_revision"], revision);
    assert!(json["data"]["revision"].as_u64().expect("revision") > revision);
    assert_eq!(
        json["data"]["accepted"].as_array().expect("accepted").len(),
        3
    );
    assert_eq!(
        json["data"]["rejected"].as_array().expect("rejected").len(),
        0
    );
    assert_eq!(
        json["data"]["values"]["name"]["value"],
        "Desk Strip Controls"
    );
    assert_eq!(json["data"]["values"]["enabled"]["value"], false);
    assert_eq!(json["data"]["values"]["brightness"]["value"], 0.5);

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_json = body_json(get_response).await;
    assert_eq!(get_json["data"]["name"], "Desk Strip Controls");
    assert_eq!(get_json["data"]["status"], "disabled");
    assert_eq!(get_json["data"]["brightness"], 50);

    let persisted_raw = fs::read_to_string(state.state_dir.join("device-settings.json"))
        .expect("device settings file should exist");
    let persisted_json: serde_json::Value =
        serde_json::from_str(&persisted_raw).expect("device settings file should be valid json");
    let settings_key =
        hypercolor_daemon::device_settings::device_settings_keys(&state.device_registry, device_id)
            .await
            .canonical;
    let persisted_device = &persisted_json["devices"][settings_key.as_str()];
    assert_eq!(persisted_device["name"], "Desk Strip Controls");
    assert_eq!(persisted_device["disabled"], true);
    assert_eq!(persisted_device["brightness"], serde_json::json!(0.5));
}

#[tokio::test]
async fn patch_device_control_surface_publishes_values_changed_event() {
    let (state, _tmp) = test_state_with_temp_output_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "values": {
                            "brightness": { "kind": "float", "value": 0.42 }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let updated_revision = json["data"]["revision"]
        .as_u64()
        .expect("updated revision should be an integer");
    let expected_surface_id = format!("device:{device_id}");

    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ControlSurfaceChanged(
                        event @ ControlSurfaceEvent::ValuesChanged { .. },
                    ) = timestamped.event
                    {
                        break event;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before control surface event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for control surface event");

    match event {
        ControlSurfaceEvent::ValuesChanged {
            surface_id,
            revision,
            values,
        } => {
            assert_eq!(surface_id, expected_surface_id);
            assert_eq!(revision, updated_revision);
            let Some(SurfaceControlValue::Float(brightness)) = values.get("brightness") else {
                panic!("brightness value should be a float");
            };
            assert!((brightness - 0.42).abs() < 1.0e-6);
        }
        _ => panic!("expected values_changed control surface event"),
    }
}

#[tokio::test]
async fn invoke_host_device_control_surface_identify_action_returns_typed_result() {
    let state = Arc::new(isolated_state());
    register_noop_backend(&state, "wled", "WLED Test Backend").await;
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/actions/identify"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "input": {
                            "duration_ms": { "kind": "duration", "value": 1 },
                            "color": {
                                "kind": "color_rgb",
                                "value": { "r": 128, "g": 64, "b": 255 }
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["surface_id"], format!("device:{device_id}"));
    assert_eq!(json["data"]["action_id"], "identify");
    assert_eq!(json["data"]["status"], "accepted");

    let event = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ControlSurfaceChanged(
                        event @ ControlSurfaceEvent::ActionProgress { .. },
                    ) = timestamped.event
                    {
                        break event;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before host action event arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for host action event");

    match event {
        ControlSurfaceEvent::ActionProgress {
            surface_id,
            action_id,
            status,
            progress,
        } => {
            assert_eq!(surface_id, format!("device:{device_id}"));
            assert_eq!(action_id, "identify");
            assert_eq!(status, ControlActionStatus::Accepted);
            assert_eq!(progress, None);
        }
        _ => panic!("expected action_progress control surface event"),
    }
}

#[tokio::test]
async fn patch_device_control_surface_revision_is_device_local() {
    let (state, _tmp) = test_state_with_temp_output_store();
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let unrelated_id = insert_test_device(&state, "Shelf Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let surface_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}/controls"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(surface_response.status(), StatusCode::OK);
    let surface_json = body_json(surface_response).await;
    let revision = surface_json["data"]["revision"]
        .as_u64()
        .expect("revision should be an integer");

    state
        .device_registry
        .update_user_settings(&unrelated_id, Some("Shelf Renamed".to_owned()), None, None)
        .await
        .expect("unrelated device should update");

    let body = serde_json::json!({
        "values": {
            "brightness": { "kind": "float", "value": 0.25 }
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["previous_revision"], revision);
    assert_eq!(json["data"]["revision"], revision + 1);
    assert_eq!(json["data"]["values"]["brightness"]["value"], 0.25);
}

#[tokio::test]
async fn patch_device_control_surface_rejects_invalid_payloads() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let cases = [
        (
            serde_json::json!({
                "unknown": { "kind": "bool", "value": true }
            }),
            "unknown_control_field",
            "unknown",
        ),
        (
            serde_json::json!({
                "brightness": { "kind": "text", "value": "bright" }
            }),
            "control_value_type_mismatch",
            "brightness",
        ),
        (
            serde_json::json!({
                "brightness": { "kind": "float", "value": 1.25 }
            }),
            "control_value_out_of_range",
            "brightness",
        ),
        (
            serde_json::json!({
                "name": { "kind": "text", "value": "   " }
            }),
            "invalid_control_value",
            "name",
        ),
    ];

    for (values, kind, field_id) in cases {
        let body = serde_json::json!({ "values": values });

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!(
                        "/api/v1/control-surfaces/device:{device_id}/values"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "validation_error");
        assert_eq!(json["error"]["details"]["kind"], kind);
        assert_eq!(json["error"]["details"]["field_id"], field_id);
    }
}

#[tokio::test]
async fn patch_device_control_surface_rejects_retired_body_identity() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let body = serde_json::json!({
        "surface_id": "device:not-the-route",
        "values": {
            "brightness": { "kind": "float", "value": 0.25 }
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn patch_device_control_surface_rejects_empty_values_with_details() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let body = serde_json::json!({ "values": {} });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(json["error"]["details"]["kind"], "empty_control_values");
    assert_eq!(
        json["error"]["details"]["surface_id"],
        format!("device:{device_id}")
    );
}

#[tokio::test]
async fn patch_device_control_surface_rejects_binding_clears() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "clear_bindings": ["brightness"]
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["details"]["field"], "clear_bindings");
}

#[tokio::test]
async fn patch_missing_device_control_surface_returns_not_found() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let app = test_app_with_state(Arc::clone(&state));

    state
        .device_registry
        .remove(&device_id)
        .await
        .expect("device should exist before removal");

    let body = serde_json::json!({
        "values": {
            "brightness": { "kind": "float", "value": 0.25 }
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!(
                    "/api/v1/control-surfaces/device:{device_id}/values"
                ))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_device_disable_runs_lifecycle_disconnect_cleanup() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Desk Strip").await;
    let disconnects = Arc::new(AtomicUsize::new(0));

    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Arc::new(DisconnectRecordingBackend::new(
            device_id,
            Arc::clone(&disconnects),
        )));
    }

    let tracked = state
        .device_registry
        .get(&device_id)
        .await
        .expect("device should exist");
    let layout_device_id = {
        let discovery = state.driver_host().discovery_runtime();
        let mut lifecycle = discovery.lifecycle_manager.lock().await;
        let _actions = lifecycle.on_discovered(device_id, &tracked.info, None);
        lifecycle
            .layout_device_id_for(device_id)
            .expect("layout id should exist")
            .to_owned()
    };

    state
        .backend_manager
        .lock()
        .await
        .connect_device("wled", device_id, &layout_device_id)
        .await
        .expect("device should connect for disable flow");

    {
        let discovery = state.driver_host().discovery_runtime();
        let mut lifecycle = discovery.lifecycle_manager.lock().await;
        lifecycle
            .on_connected(device_id)
            .expect("connect transition should succeed");
    }
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;

    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/devices/{device_id}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"enabled":false}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["status"], "disabled");
    assert_eq!(disconnects.load(Ordering::Relaxed), 1);
    assert_eq!(state.backend_manager.lock().await.mapped_device_count(), 0);
}

#[tokio::test]
async fn list_displays_only_returns_display_capable_devices() {
    let state = Arc::new(isolated_state());
    let _ = insert_test_device(&state, "Desk Strip").await;
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/displays")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let items = json["data"]
        .as_array()
        .expect("display list should be an array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], display_id.to_string());
    assert_eq!(items[0]["name"], "Pump LCD");
    assert_eq!(items[0]["width"], 320);
    assert_eq!(items[0]["height"], 320);
    assert_eq!(items[0]["circular"], true);

    assert!(
        state
            .scene_manager
            .snapshot()
            .await
            .active_scene()
            .and_then(|scene| scene.display_zone_for(display_id))
            .is_none(),
        "display listing must not mutate the active scene"
    );
}

/// A stack of identical wireless LCD fans ships identical names; the port
/// is what tells them apart.
async fn insert_test_display_device_at_port(
    state: &Arc<AppState>,
    name: &str,
    usb_path: &str,
) -> DeviceId {
    let id = DeviceId::new();
    let mut info = DeviceInfo {
        id,
        name: name.to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("lianli", "Lian Li"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("lianli", "usb", ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: 400,
                height: 400,
                circular: true,
                format: DisplayFrameFormat::Jpeg,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    };
    info.sync_display_capabilities();
    let fingerprint = DeviceFingerprint::from_persisted(format!("usb:lianli:{usb_path}"));
    let metadata = HashMap::from([("usb_path".to_owned(), usb_path.to_owned())]);
    state
        .device_registry
        .add_with_fingerprint_and_metadata(info, fingerprint, metadata)
        .await
}

#[tokio::test]
async fn list_displays_tells_identical_panels_apart_by_port_and_honours_user_names() {
    let state = Arc::new(isolated_state());
    let left = insert_test_display_device_at_port(&state, "Fan LCD", "1-1.2").await;
    let right = insert_test_display_device_at_port(&state, "Fan LCD", "1-1.3").await;
    let renamed = insert_test_display_device_at_port(&state, "Fan LCD", "1-1.4").await;
    state
        .device_registry
        .update_user_settings(&renamed, Some("Top Left Fan".to_owned()), None, None)
        .await
        .expect("device exists");
    let lone = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/displays")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let names: HashMap<String, String> = json["data"]
        .as_array()
        .expect("array")
        .iter()
        .map(|item| {
            (
                item["id"].as_str().expect("id").to_owned(),
                item["name"].as_str().expect("name").to_owned(),
            )
        })
        .collect();
    assert_eq!(names[&left.to_string()], "Fan LCD (USB 1-1.2)");
    assert_eq!(names[&right.to_string()], "Fan LCD (USB 1-1.3)");
    assert_eq!(
        names[&renamed.to_string()],
        "Top Left Fan",
        "a user name is unique on its own"
    );
    assert_eq!(
        names[&lone.to_string()],
        "Pump LCD",
        "a lone name is untouched"
    );
}

#[tokio::test]
async fn patch_display_face_controls_rejects_binding_clears() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/controls"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "clear_bindings": ["label"]
                    })
                    .to_string(),
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["details"]["field"], "clear_bindings");
}

#[tokio::test]
async fn delete_face_idempotent_when_no_zone_present() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/displays/{display_id}/face?scope=scene"))
                    .body(Body::empty())
                    .expect("failed to build request"),
            )
            .await
            .expect("failed to execute request");
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["data"]["device_id"], display_id.to_string());
        assert_eq!(json["data"]["deleted"], true);
    }
}

#[tokio::test]
async fn get_face_returns_null_when_no_display_zone() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn patch_face_controls_updates_display_zone() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let mut face = test_display_face_effect_metadata("System Monitor");
    face.controls = vec![ControlDefinition {
        id: "label".to_owned(),
        name: "Label".to_owned(),
        kind: ControlKind::Text,
        control_type: ControlType::TextInput,
        default_value: ControlValue::Text("cpu".to_owned()),
        min: None,
        max: None,
        step: None,
        labels: Vec::new(),
        group: Some("General".to_owned()),
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    }];
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata: face.clone(),
            source_path: format!("/tmp/{}.html", face.name).into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let assign_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_response.status(), StatusCode::OK);

    let patch_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/controls"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"values":{"label":{"kind":"text","value":"gpu"}}}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(patch_response.status(), StatusCode::OK);
    let patch_json = body_json(patch_response).await;
    assert_eq!(
        patch_json["data"]["zone"]["layers"][0]["source"]["controls"]["label"]["value"],
        "gpu"
    );

    let manager = state.scene_manager.snapshot().await;
    let display_zone = manager
        .active_scene()
        .and_then(|scene| scene.display_zone_for(display_id))
        .expect("display face should remain assigned");
    assert_eq!(
        zone_effect_controls(display_zone).and_then(|controls| controls.get("label")),
        Some(&ControlValue::Text("gpu".to_owned()))
    );
}

#[tokio::test]
async fn put_face_conflicts_when_snapshot_scene_is_active() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    activate_empty_test_scene_with_mode(&state, "Focus", SceneMutationMode::Snapshot).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
    assert!(
        json["error"]["message"]
            .as_str()
            .expect("message should be a string")
            .contains("snapshot mode"),
    );

    let manager = state.scene_manager.snapshot().await;
    assert!(
        manager
            .active_scene()
            .and_then(|scene| scene.display_zone_for(display_id))
            .is_none(),
        "snapshot scene should not be rewritten by face assignment",
    );
}

#[tokio::test]
async fn patch_face_composition_updates_material_blend_mode_and_normalizes_replace() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    activate_empty_test_scene(&state, "Desk Scene").await;
    let app = test_app_with_state(Arc::clone(&state));
    // Cutout/alpha is the compact default; explicit `replace` is serialized
    // because it diverges from the default and also forces opacity back to 1.0.

    let assign_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_response.status(), StatusCode::OK);

    let tint_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/composition"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"blend_mode":"tint","opacity":0.35}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(tint_response.status(), StatusCode::OK);
    let tint_json = body_json(tint_response).await;
    assert_eq!(
        tint_json["data"]["zone"]["display_target"]["blend_mode"],
        "tint"
    );
    assert_eq!(tint_json["data"]["zone"]["display_target"]["opacity"], 0.35);

    let replace_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/composition"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"blend_mode":"replace","opacity":0.05}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(replace_response.status(), StatusCode::OK);
    let replace_json = body_json(replace_response).await;
    assert_eq!(
        replace_json["data"]["zone"]["display_target"]["blend_mode"], "replace",
        "explicit replace mode should serialize since it is no longer the default"
    );
    assert!(
        replace_json["data"]["zone"]["display_target"]["opacity"].is_null(),
        "replace mode should normalize opacity back to the default"
    );

    let manager = state.scene_manager.snapshot().await;
    let zone = manager
        .active_scene()
        .and_then(|scene| scene.display_zone_for(display_id))
        .expect("display face should remain assigned");
    let target = zone
        .display_target
        .clone()
        .expect("display target should remain present");
    assert_eq!(target.device_id, display_id);
    assert_eq!(target.blend_mode, BlendMode::Replace);
    assert!((target.opacity - 1.0).abs() < f32::EPSILON);
}

#[tokio::test]
async fn reassigning_display_face_resets_composition_to_blended_default() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face_a = insert_test_display_face_effect(&state, "System Monitor").await;
    let face_b = insert_test_display_face_effect(&state, "Minimal Clock").await;
    activate_empty_test_scene(&state, "Desk Scene").await;
    let app = test_app_with_state(Arc::clone(&state));

    let assign_a = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face_a.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_a.status(), StatusCode::OK);

    let tint_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/composition"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"blend_mode":"screen","opacity":0.42}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(tint_response.status(), StatusCode::OK);

    let assign_b = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face_b.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_b.status(), StatusCode::OK);
    let assign_b_json = body_json(assign_b).await;
    assert_eq!(assign_b_json["data"]["effect"]["id"], face_b.id.to_string());
    assert!(
        assign_b_json["data"]["zone"]["display_target"]["blend_mode"].is_null(),
        "reassigning a face should reset composition mode to the blended default (alpha serializes as absent)"
    );
    assert!(
        assign_b_json["data"]["zone"]["display_target"]["opacity"].is_null(),
        "reassigning a face should reset opacity to the default"
    );

    let manager = state.scene_manager.snapshot().await;
    let zone = manager
        .active_scene()
        .and_then(|scene| scene.display_zone_for(display_id))
        .expect("display face should remain assigned");
    let target = zone
        .display_target
        .clone()
        .expect("display target should remain present");
    assert_eq!(target.blend_mode, BlendMode::Alpha);
    assert!((target.opacity - 1.0).abs() < f32::EPSILON);
}

#[tokio::test]
async fn put_face_from_cold_start_succeeds_no_409() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(
        json["data"]["scene_id"],
        hypercolor_types::scene::SceneId::DEFAULT.to_string()
    );
    assert_eq!(json["data"]["effect"]["id"], face.id.to_string());
}

#[tokio::test]
async fn display_face_endpoint_rejects_non_display_effects() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    insert_test_effect(&state, "Rainbow").await;
    activate_empty_test_scene(&state, "Desk Scene").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"effect_id":"Rainbow"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
}

#[tokio::test]
async fn list_devices_supports_filters() {
    let state = Arc::new(isolated_state());
    let _first_id = insert_test_device(&state, "Desk Strip").await;
    let second_id = insert_test_device(&state, "Ceiling Panel").await;
    let _smbus_id = insert_test_asus_smbus_device(&state, "Aura GPU").await;
    let _ = state
        .device_registry
        .set_state(&second_id, DeviceState::Disabled)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let disabled_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?status=disabled")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(disabled_response.status(), StatusCode::OK);
    let disabled_json = body_json(disabled_response).await;
    assert_eq!(disabled_json["data"]["total"], 1);
    assert_eq!(disabled_json["data"]["items"][0]["name"], "Ceiling Panel");

    let query_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?backend_id=wled&q=desk")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(query_response.status(), StatusCode::OK);
    let query_json = body_json(query_response).await;
    assert_eq!(query_json["data"]["total"], 1);
    assert_eq!(query_json["data"]["items"][0]["name"], "Desk Strip");

    let backend_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?backend_id=smbus")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(backend_response.status(), StatusCode::OK);
    let backend_json = body_json(backend_response).await;
    assert_eq!(backend_json["data"]["total"], 1);
    assert_eq!(backend_json["data"]["items"][0]["name"], "Aura GPU");

    let driver_response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?driver=asus")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(driver_response.status(), StatusCode::OK);
    let driver_json = body_json(driver_response).await;
    assert_eq!(driver_json["data"]["total"], 1);
    assert_eq!(
        driver_json["data"]["items"][0]["origin"]["backend_id"],
        "smbus"
    );
}

#[tokio::test]
async fn get_device_includes_explicit_origin_metadata() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_asus_smbus_device(&state, "Aura GPU").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/devices/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let device = &json["data"];
    assert_eq!(device["origin"]["driver_id"], "asus");
    assert_eq!(device["origin"]["backend_id"], "smbus");
    assert_eq!(device["origin"]["transport"], "smbus");
    assert_eq!(device["origin"]["protocol_id"], "asus/aura-smbus");
    assert_eq!(device["presentation"]["label"], "ASUS");
    assert_eq!(device["connection"]["transport"], "smbus");
    assert_eq!(device["connection"]["label"], "SMBus 0x40");
    assert_eq!(device["connection"]["endpoint"], "SMBus 0x40");
}

#[tokio::test]
async fn list_devices_includes_connection_summary_when_available() {
    let state = Arc::new(isolated_state());
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "Desk Strip".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.1.0".to_owned()),
        capabilities: DeviceCapabilities {
            led_count: 60,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("ip".to_owned(), "192.168.1.42".to_owned());
    metadata.insert("hostname".to_owned(), "wled-desk".to_owned());
    let _ = state
        .device_registry
        .add_with_fingerprint_and_metadata(
            info,
            DeviceFingerprint::from_persisted("net:aa:bb:cc:dd:ee:ff".to_owned()),
            metadata,
        )
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let connection = &json["data"]["items"][0]["connection"];
    assert_eq!(connection["transport"], "network");
    assert_eq!(connection["ip"], "192.168.1.42");
    assert_eq!(connection["hostname"], "wled-desk");
    assert_eq!(connection["endpoint"], "wled-desk");
}

#[tokio::test]
async fn list_devices_preserves_custom_connection_transport_id() {
    let state = Arc::new(isolated_state());
    let info = DeviceInfo {
        id: DeviceId::new(),
        name: "External Hub".to_owned(),
        vendor: "test-vendor".to_owned(),
        family: DeviceFamily::new_static("external-hub", "External Hub"),
        model: None,
        connection_type: ConnectionType::Bridge,
        origin: DeviceOrigin::new(
            "external-hub",
            "external-hub",
            DriverTransportKind::Custom("openlinkhub".to_owned()),
        ),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 24,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 24,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    };
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("serial".to_owned(), "hub-001".to_owned());
    let _ = state
        .device_registry
        .add_with_fingerprint_and_metadata(
            info,
            DeviceFingerprint::from_persisted("openlinkhub:hub-001".to_owned()),
            metadata,
        )
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let connection = &json["data"]["items"][0]["connection"];
    assert_eq!(connection["transport"], "openlinkhub");
    assert_eq!(connection["label"], "hub-001");
    assert_eq!(connection["endpoint"], "hub-001");
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn list_devices_includes_hue_auth_summary_when_pairing_required() {
    let state = Arc::new(isolated_state());
    let _device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "10.0.0.5", 80).await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    let auth = &json["data"]["items"][0]["auth"];
    assert_eq!(auth["state"], "required");
    assert_eq!(auth["can_pair"], true);
    assert_eq!(auth["descriptor"]["kind"], "physical_action");
    assert_eq!(auth["descriptor"]["action_label"], "Pair Bridge");
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn list_devices_includes_hue_auth_summary_when_configured() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let _device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "10.0.0.5", 80).await;
    state
        .driver_host()
        .credential_store()
        .store_driver_json(
            "hue",
            "test-bridge",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await
        .expect("store credentials");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);

    let json = body_json(response).await;
    assert_eq!(json["data"]["items"][0]["auth"]["state"], "configured");
}

#[tokio::test]
async fn list_devices_rejects_invalid_status_filter() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?status=invalid")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
}

#[tokio::test]
async fn list_devices_rejects_unknown_expansions() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?include=attachments,unknown")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "validation_error");
    assert_eq!(json["error"]["details"]["field"], "include");
}

#[tokio::test]
async fn list_devices_rejects_invalid_limit() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices?limit=0")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn identify_device_validates_and_returns_canonical_id() {
    let state = Arc::new(isolated_state());
    register_noop_backend(&state, "wled", "WLED Test Backend").await;
    let device_id = insert_test_device(&state, "Keyboard").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let invalid_color_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Keyboard/identify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"color":"zzzzzz"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        invalid_color_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_duration_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Keyboard/identify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":0}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(
        invalid_duration_response.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let valid_response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Keyboard/identify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":1500,"color":"ff00aa"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(valid_response.status(), StatusCode::OK);
    let valid_json = body_json(valid_response).await;
    assert_eq!(valid_json["data"]["device_id"], device_id.to_string());
    assert_eq!(valid_json["data"]["color"], "#FF00AA");
}

#[tokio::test]
async fn identify_device_requires_connected_state() {
    let state = Arc::new(isolated_state());
    let _device_id = insert_test_display_device(&state, "Known Display").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/devices/Known%20Display/identify")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
}

#[tokio::test]
async fn identify_device_temporarily_connects_known_network_device() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Known Strip").await;
    let disconnects = Arc::new(AtomicUsize::new(0));
    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Arc::new(DisconnectRecordingBackend::new(
            device_id,
            Arc::clone(&disconnects),
        )));
    }
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/identify"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":1}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["device_id"], device_id.to_string());
    assert!(
        state
            .backend_manager
            .lock()
            .await
            .is_direct_control_active("wled", device_id)
    );

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(disconnects.load(Ordering::Relaxed), 1);
    assert!(
        !state
            .backend_manager
            .lock()
            .await
            .is_direct_control_active("wled", device_id)
    );
}

#[tokio::test]
async fn pause_preempts_identify_and_holds_black_output() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Identify Strip").await;
    let device_info = state
        .device_registry
        .get(&device_id)
        .await
        .expect("test device should exist")
        .info;
    let layout_device_id = format!("identify:{device_id}");
    let writes = Arc::new(StdMutex::new(Vec::new()));
    {
        let mut manager = state.backend_manager.lock().await;
        manager.register_backend(Arc::new(IdentifyRecordingBackend {
            writes: Arc::clone(&writes),
        }));
        manager
            .connect_device("wled", device_id, &layout_device_id)
            .await
            .expect("test device should connect");
        assert!(manager.set_device_zone_segments(&layout_device_id, &device_info));
    }
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let identify_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/identify"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"duration_ms":10000}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute identify request");
    assert_eq!(identify_response.status(), StatusCode::OK);
    assert!(
        writes
            .lock()
            .expect("identify output writes lock")
            .iter()
            .any(|frame| frame.iter().any(|color| *color != [0, 0, 0]))
    );

    let pause_response = app
        .oneshot(output_patch_request(r#"{"power":"paused"}"#))
        .await
        .expect("failed to execute pause request");
    assert_eq!(pause_response.status(), StatusCode::OK);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let direct_control_active = state
                .backend_manager
                .lock()
                .await
                .is_direct_control_active("wled", device_id);
            let black_held = writes
                .lock()
                .expect("identify output writes lock")
                .last()
                .is_some_and(|frame| frame.iter().all(|color| *color == [0, 0, 0]));
            if !direct_control_active && black_held {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pause should preempt identify and hold black promptly");

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        writes
            .lock()
            .expect("identify output writes lock")
            .last()
            .is_some_and(|frame| frame.iter().all(|color| *color == [0, 0, 0]))
    );
}

#[tokio::test]
async fn identify_device_uses_discovered_smbus_backend_for_asus_devices() {
    let state = Arc::new(isolated_state());
    register_noop_backend(&state, "smbus", "SMBus Test Backend").await;
    let device_id = insert_test_asus_smbus_device(&state, "Aura GPU").await;
    let _ = state
        .device_registry
        .set_state(&device_id, DeviceState::Connected)
        .await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/identify"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["device_id"], device_id.to_string());
    assert_eq!(json["data"]["identifying"], true);
}

#[tokio::test]
async fn get_device_by_ambiguous_name_returns_conflict() {
    let state = Arc::new(isolated_state());
    let _ = insert_test_device(&state, "Same Name").await;
    let _ = insert_test_device(&state, "Same Name").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/devices/Same%20Name")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "conflict");
}

#[tokio::test]
async fn delete_device_by_name_returns_canonical_id() {
    let state = Arc::new(isolated_state());
    let device_id = insert_test_device(&state, "Panel").await;
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/devices/Panel")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], device_id.to_string());
}

#[tokio::test]
async fn delete_device_forgets_learned_wled_inventory() {
    let state = Arc::new(isolated_state());
    let device_id = DeviceId::new();
    let fingerprint = DeviceFingerprint::from_persisted("net:aa:bb:cc:dd:ee:ff".to_owned());
    let info = DeviceInfo {
        id: device_id,
        name: "WLED Gledopto".to_owned(),
        vendor: "WLED".to_owned(),
        family: DeviceFamily::new_static("wled", "WLED"),
        model: None,
        connection_type: ConnectionType::Network,
        origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
        segments: vec![SegmentInfo {
            name: "Main".to_owned(),
            led_count: 60,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: Some("0.15.3".to_owned()),
        capabilities: DeviceCapabilities::default(),
    };
    state
        .device_registry
        .add_with_fingerprint_and_metadata(
            info,
            fingerprint.clone(),
            HashMap::from([("ip".to_owned(), "10.4.22.69".to_owned())]),
        )
        .await;
    state
        .driver_host()
        .driver_inventory()
        .replace_driver(
            "wled",
            BTreeMap::from([
                (
                    "probe_ips".to_owned(),
                    serde_json::json!(["10.4.22.69", "10.4.22.169"]),
                ),
                (
                    "probe_targets".to_owned(),
                    serde_json::json!([
                        {"ip": "10.4.22.69", "fingerprint": fingerprint},
                        {"ip": "10.4.22.169"}
                    ]),
                ),
                ("future_key".to_owned(), serde_json::json!(true)),
            ]),
        )
        .await
        .expect("seed WLED inventory");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{device_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(state.device_registry.get(&device_id).await.is_none());
    let cache = state.driver_host().driver_inventory().driver_cache("wled");
    assert_eq!(cache["probe_ips"], serde_json::json!(["10.4.22.169"]));
    assert_eq!(cache["probe_targets"][0]["ip"], "10.4.22.169");
    assert_eq!(cache["future_key"], serde_json::json!(true));
}

#[tokio::test]
async fn deleting_display_device_prunes_scene_display_zones_and_persists_cleanup() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    {
        let mut mutation = state.scene_manager.begin_mutation().await;
        mutation
            .upsert_display_zone(
                display_id,
                "Pump LCD",
                &face,
                HashMap::new(),
                SpatialLayout {
                    id: "default-display-layout".to_owned(),
                    name: "Default Display Layout".to_owned(),
                    description: None,
                    canvas_width: 320,
                    canvas_height: 320,
                    zones: Vec::new(),
                    default_sampling_mode: SamplingMode::Bilinear,
                    default_edge_behavior: EdgeBehavior::Clamp,
                    version: 1,
                },
                hypercolor_types::scene::DisplayFaceTarget::new(display_id),
            )
            .expect("default scene face should be assigned");
        hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
            .await
            .expect("default scene face should commit");
    }
    let named_scene_id =
        activate_display_face_test_scene(&state, "Desk Scene", face.id, display_id).await;
    let mut mutation = state.scene_manager.begin_mutation().await;
    mutation.deactivate_current(hypercolor_types::event::SceneChangeReason::UserDeactivate);
    hypercolor_daemon::domain::scene::commit_scene(&state.domains.scene, mutation)
        .await
        .expect("default scene should reactivate");

    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{display_id}"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["data"]["id"], display_id.to_string());
    assert!(state.device_registry.get(&display_id).await.is_none());

    let mut removed_scene_ids = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while removed_scene_ids.len() < 2 {
            match events.recv().await {
                Ok(timestamped) => {
                    if let HypercolorEvent::ZoneChanged {
                        scene_id,
                        role,
                        kind,
                        ..
                    } = timestamped.event
                        && role == ZoneRole::Display
                        && kind == ZoneChangeKind::Removed
                    {
                        removed_scene_ids.push(scene_id);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before display-zone removal events arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for display-zone removal events");
    assert!(removed_scene_ids.contains(&SceneId::DEFAULT));
    assert!(removed_scene_ids.contains(&named_scene_id));

    {
        let manager = state.scene_manager.snapshot().await;
        let default_scene = manager
            .active_scene()
            .expect("default scene should remain active");
        assert!(default_scene.display_zone_for(display_id).is_none());
        let named_scene = manager
            .get(&named_scene_id)
            .expect("named scene should remain present");
        assert!(named_scene.display_zone_for(display_id).is_none());
    }

    let persisted =
        runtime_state::load(&state.runtime_state_path).expect("runtime state should load");
    let persisted = persisted.expect("runtime state should exist");
    assert!(
        persisted.default_scene_zones.iter().all(|zone| {
            zone.display_target
                .as_ref()
                .is_none_or(|target| target.device_id != display_id)
        }),
        "deleted device should not survive in the persisted default scene"
    );

    let scene_store =
        scene_store::load(&state.data_dir.join("scenes.json")).expect("scene store should reload");
    let named_scene = scene_store
        .list()
        .find(|scene| scene.id == named_scene_id)
        .expect("named scene should be persisted");
    assert!(
        named_scene.zones.iter().all(|zone| {
            zone.display_target
                .as_ref()
                .is_none_or(|target| target.device_id != display_id)
        }),
        "deleted device should not survive in persisted named scenes"
    );
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn pair_device_route_pairs_hue_by_device_id() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Hue mock server");
    let port = listener.local_addr().expect("local addr").port();
    let device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "127.0.0.1", port)
            .await;
    let app = test_app_with_state(Arc::clone(&state));

    let server_task = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_pairing_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("POST /api HTTP/1.1") {
                pairing_json_response(
                    r#"[{"success":{"username":"test-api-key","clientkey":"00112233445566778899aabbccddeeff"}}]"#,
                )
            } else if request.starts_with("GET /api/config HTTP/1.1") {
                pairing_json_response(
                    r#"{"bridgeid":"test-bridge","name":"Studio Bridge","modelid":"BSB002","swversion":"1968096020"}"#,
                )
            } else {
                pairing_not_found_response()
            };
            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"activate_after_pair":true}"#))
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "paired");
    assert_eq!(json["data"]["activated"], false);
    assert_eq!(json["data"]["device"]["auth"]["state"], "configured");

    assert_eq!(
        state
            .driver_host()
            .credential_store()
            .get_driver_json("hue", "test-bridge")
            .await,
        Some(serde_json::json!({
            "api_key": "test-api-key",
            "client_key": "00112233445566778899aabbccddeeff",
        }))
    );

    server_task.await.expect("Hue mock task should finish");
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn pair_device_route_returns_action_required_for_hue_without_button() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Hue mock server");
    let port = listener.local_addr().expect("local addr").port();
    let device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "127.0.0.1", port)
            .await;
    let app = test_app_with_state(Arc::clone(&state));

    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let _request = read_pairing_http_request(&mut stream)
            .await
            .expect("read HTTP request");
        stream
            .write_all(
                pairing_json_response(
                    r#"[{"error":{"type":101,"description":"link button not pressed"}}]"#,
                )
                .as_slice(),
            )
            .await
            .expect("write HTTP response");
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "action_required");
    assert_eq!(json["data"]["activated"], false);
    assert_eq!(json["data"]["device"]["auth"]["state"], "required");

    server_task.await.expect("Hue mock task should finish");
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn pair_device_route_pairs_nanoleaf_by_device_id() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Nanoleaf mock server");
    let port = listener.local_addr().expect("local addr").port();
    let device_id =
        insert_test_nanoleaf_device(&state, "Living Room Shapes", "serial42", "127.0.0.1", port)
            .await;
    let app = test_app_with_state(Arc::clone(&state));

    let server_task = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_pairing_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("POST /api/v1/new HTTP/1.1") {
                pairing_json_response(r#"{"auth_token":"nanoleaf-token"}"#)
            } else if request.starts_with("GET /api/v1/nanoleaf-token HTTP/1.1") {
                pairing_json_response(
                    r#"{"name":"Living Room Shapes","model":"Shapes","serialNo":"SERIAL42","firmwareVersion":"12.0.0"}"#,
                )
            } else {
                pairing_not_found_response()
            };
            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"activate_after_pair":true}"#))
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "paired");
    assert_eq!(json["data"]["activated"], false);
    assert_eq!(json["data"]["device"]["auth"]["state"], "configured");

    assert_eq!(
        state
            .driver_host()
            .credential_store()
            .get_driver_json("nanoleaf", "serial42")
            .await,
        Some(serde_json::json!({
            "auth_token": "nanoleaf-token",
        }))
    );

    server_task.await.expect("Nanoleaf mock task should finish");
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn delete_pairing_removes_hue_credentials() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let device_id =
        insert_test_hue_bridge_device(&state, "Studio Bridge", "test-bridge", "10.0.0.5", 80).await;
    state
        .driver_host()
        .credential_store()
        .store_driver_json(
            "hue",
            "test-bridge",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await
        .expect("store Hue credentials");
    state
        .driver_host()
        .credential_store()
        .store_driver_json(
            "hue",
            "ip:10.0.0.5",
            serde_json::json!({
                "api_key": "api-key",
                "client_key": "client-key",
            }),
        )
        .await
        .expect("store Hue IP credentials");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "unpaired");
    assert_eq!(json["data"]["device"]["auth"]["state"], "required");
    assert_eq!(
        state
            .driver_host()
            .credential_store()
            .get_driver_json("hue", "test-bridge")
            .await,
        None
    );
    assert_eq!(
        state
            .driver_host()
            .credential_store()
            .get_driver_json("hue", "ip:10.0.0.5")
            .await,
        None
    );
}

#[cfg(feature = "builtin-drivers")]
#[tokio::test]
async fn delete_pairing_removes_nanoleaf_credentials() {
    let (state, _tempdir) = isolated_state_with_tempdir();
    let state = Arc::new(state);
    let device_id =
        insert_test_nanoleaf_device(&state, "Living Room Shapes", "serial42", "10.0.0.8", 16021)
            .await;
    state
        .driver_host()
        .credential_store()
        .store_driver_json(
            "nanoleaf",
            "serial42",
            serde_json::json!({
                "auth_token": "auth-token",
            }),
        )
        .await
        .expect("store Nanoleaf credentials");
    state
        .driver_host()
        .credential_store()
        .store_driver_json(
            "nanoleaf",
            "ip:10.0.0.8",
            serde_json::json!({
                "auth_token": "auth-token",
            }),
        )
        .await
        .expect("store Nanoleaf IP credentials");
    let app = test_app_with_state(Arc::clone(&state));

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/devices/{device_id}/pair"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("execute request");
    let status = response.status();
    let json = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["status"], "unpaired");
    assert_eq!(json["data"]["device"]["auth"]["state"], "required");
    assert_eq!(
        state
            .driver_host()
            .credential_store()
            .get_driver_json("nanoleaf", "serial42")
            .await,
        None
    );
    assert_eq!(
        state
            .driver_host()
            .credential_store()
            .get_driver_json("nanoleaf", "ip:10.0.0.8")
            .await,
        None
    );
}

#[cfg(feature = "builtin-drivers")]
fn pairing_json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

#[cfg(feature = "builtin-drivers")]
fn pairing_not_found_response() -> Vec<u8> {
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

#[cfg(feature = "builtin-drivers")]
async fn read_pairing_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = vec![0_u8; 4096];
    let mut total = 0_usize;
    let mut header_end = None;

    loop {
        let read = stream.read(&mut buf[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;

        if header_end.is_none() {
            header_end = buf[..total]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4);
        }

        if let Some(header_end) = header_end {
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if total >= header_end + content_length {
                break;
            }
        }

        if total == buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }

    Ok(String::from_utf8_lossy(&buf[..total]).into_owned())
}

#[tokio::test]
async fn settings_mutations_publish_local_change_hints() {
    use hypercolor_types::event::{HypercolorEvent, LibraryChangeKind, LibraryCollection};

    let state = Arc::new(isolated_state());
    insert_test_effect(&state, "solid_color").await;
    let mut events = state.event_bus.subscribe_all();
    let app = test_app_with_state(Arc::clone(&state));

    let add_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/library/favorites")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"effect":"solid_color"}"#))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(add_response.status(), StatusCode::OK);

    let delete_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/library/favorites/solid_color")
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let brightness_response = app
        .oneshot(output_patch_request(r#"{"brightness":0.42}"#))
        .await
        .expect("failed to execute request");
    assert_eq!(brightness_response.status(), StatusCode::OK);

    // Every persisted mutation must hint observers that mirror the
    // stores; this is the sync engine's local-change intake.
    let mut favorite_upserted = false;
    let mut favorite_removed = false;
    let mut settings_changed = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(favorite_upserted && favorite_removed && settings_changed) {
            match events.recv().await {
                Ok(timestamped) => match timestamped.event {
                    HypercolorEvent::LibraryStoreChanged {
                        collection: LibraryCollection::Favorites,
                        kind,
                        ..
                    } => match kind {
                        LibraryChangeKind::Upserted => favorite_upserted = true,
                        LibraryChangeKind::Removed => favorite_removed = true,
                    },
                    HypercolorEvent::DeviceSettingsChanged { key: None } => {
                        settings_changed = true;
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before local-change hints arrived");
                }
            }
        }
    })
    .await
    .expect("timed out waiting for local-change hints");
}

/// Recursive key paths of a JSON value; arrays contribute their first
/// element's shape (test scenarios keep them homogeneous).
fn collect_key_paths(value: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                out.push(path.clone());
                collect_key_paths(child, &path, out);
            }
        }
        serde_json::Value::Array(items) => {
            if let Some(first) = items.first() {
                collect_key_paths(first, &format!("{prefix}[]"), out);
            }
        }
        _ => {}
    }
}

/// External HTTP clients read the face payload without any Rust type to
/// hold them to it, so this pin is what keeps the published key paths
/// stable: renaming a field in the shared
/// `hypercolor_types::api::displays::DisplayFaceResponse` moves the
/// daemon and every in-tree client together and would otherwise break
/// only the outside world, silently.
///
/// The fixture is shared with the UI's
/// `display_face_response_decodes_the_daemon_shape`, which decodes the
/// same payload and so covers the value representations this key-path
/// comparison cannot see.
#[tokio::test]
async fn display_face_response_shape_matches_the_shared_fixture() {
    let state = Arc::new(isolated_state());
    let display_id = insert_test_display_device(&state, "Pump LCD").await;
    let face = insert_test_display_face_effect(&state, "System Monitor").await;
    let app = test_app_with_state(Arc::clone(&state));

    let assign_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"effect_id":"{}","scope":"scene"}}"#,
                    face.id
                )))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(assign_response.status(), StatusCode::OK);

    let patch_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/displays/{display_id}/face/controls"))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"values":{"label":{"kind":"text","value":"gpu"}}}"#,
                ))
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(patch_response.status(), StatusCode::OK);

    let get_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/displays/{display_id}/face"))
                .body(Body::empty())
                .expect("failed to build request"),
        )
        .await
        .expect("failed to execute request");
    assert_eq!(get_response.status(), StatusCode::OK);
    let json = body_json(get_response).await;

    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/rest_v1/display_face_shape.json"))
            .expect("fixture parses");

    let mut actual_paths = Vec::new();
    collect_key_paths(&json["data"], "", &mut actual_paths);
    let mut fixture_paths = Vec::new();
    collect_key_paths(&fixture, "", &mut fixture_paths);
    actual_paths.sort();
    fixture_paths.sort();
    assert_eq!(
        actual_paths, fixture_paths,
        "the face wire shape drifted from the shared fixture; update \
         tests/fixtures/rest_v1/display_face_shape.json and re-run the UI \
         decode test that reads it"
    );
}
