//! Each screen shows the backdrop of the zone that places it, seen through
//! that zone's placement of the screen (Spec 95 §5.10), and its own layers
//! over that backdrop, whether they are stored as screen layers or as a
//! display zone (§6.4, §6.6).
//!
//! These tests run the render thread and the display output thread together
//! and read what reaches each panel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::{DisplayZoneViewport, HypercolorBus};
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
use hypercolor_core::scene::screens::{
    LegacyReason, ScreenConversion, ScreenSurface, ScreenSurfaces,
};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_core::spatial::{DISPLAY_ZONE_NAME, SpatialEngine};
use hypercolor_daemon::SceneTransactionQueue;
use hypercolor_daemon::display_frames::DisplayFrameRuntime;
use hypercolor_daemon::display_output::{
    DisplayOutputState, DisplayOutputThread, screen_placement, screen_viewport_in_layout,
};
use hypercolor_daemon::domain::scene::SceneService;
use hypercolor_daemon::domain::spatial::SpatialService;
use hypercolor_daemon::logical_devices::{LogicalDevice, LogicalDeviceKind};
use hypercolor_daemon::output_power::OutputPowerState;
use hypercolor_daemon::performance::PerformanceTracker;
use hypercolor_daemon::preview_runtime::PreviewRuntime;
use hypercolor_daemon::render_thread::{CanvasDims, RenderThread, RenderThreadState};
use hypercolor_daemon::simulators::SIMULATED_DISPLAY_BACKEND_ID;
use hypercolor_driver_api::{BackendInfo, DeviceBackend, DiscoveredDevice};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceError,
    DeviceFamily, DeviceFeatures, DeviceId, DeviceInfo, DeviceOrigin, DeviceState,
    DeviceTopologyHint, DisplayFrameFormat, OwnedDisplayFramePayload, SegmentInfo,
};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{BlendMode, LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{
    DisplayFaceTarget, DisplayRotation, Scene, UnassignedBehavior, Zone, ZoneId, ZoneRole,
};
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

const WAIT: Duration = Duration::from_secs(15);
const CANVAS: (u32, u32) = (320, 200);
/// Panels are raw RGB and small, so a write is its own picture.
const PANEL: u32 = 16;
const BACKEND_ID: &str = "usb";

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

type DeviceResult<T> = std::result::Result<T, DeviceError>;

/// One frame a panel received.
#[derive(Clone)]
struct PanelWrite {
    at: Instant,
    format: DisplayFrameFormat,
    data: Vec<u8>,
}

/// Every frame each panel received, in order.
#[derive(Clone, Default)]
struct PanelWrites(Arc<StdMutex<HashMap<DeviceId, Vec<PanelWrite>>>>);

impl PanelWrites {
    fn record(&self, device_id: DeviceId, payload: &OwnedDisplayFramePayload) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(device_id)
            .or_default()
            .push(PanelWrite {
                at: Instant::now(),
                format: payload.format,
                data: payload.data.to_vec(),
            });
    }

    fn of(&self, device_id: DeviceId) -> Vec<PanelWrite> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&device_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Wait for the panel's latest frame to satisfy `accept`.
    async fn wait_for(
        &self,
        device_id: DeviceId,
        what: &str,
        accept: impl Fn(&Panel<'_>) -> bool,
    ) -> PanelWrite {
        let started = Instant::now();
        loop {
            let latest = self.of(device_id).pop();
            if let Some(write) = latest.as_ref()
                && accept(&Panel(&write.data))
            {
                return write.clone();
            }
            assert!(
                started.elapsed() < WAIT,
                "panel never showed {what}; its latest frame center was {:?}",
                latest.map(|write| Panel(&write.data).center())
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// A raw RGB panel frame.
struct Panel<'a>(&'a [u8]);

impl Panel<'_> {
    fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let offset = usize::try_from((y * PANEL + x) * 3).expect("panel offset fits usize");
        [self.0[offset], self.0[offset + 1], self.0[offset + 2]]
    }

    fn center(&self) -> [u8; 3] {
        self.pixel(PANEL / 2, PANEL / 2)
    }
}

fn near(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= 3)
}

/// A backend whose devices are panels that record what they are sent.
struct PanelBackend {
    /// The backend's id: the rig's panel backend, or the simulator backend,
    /// whose devices are nobody's output until somebody previews them.
    id: &'static str,
    writes: PanelWrites,
}

#[async_trait]
impl DeviceBackend for PanelBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: self.id.to_owned(),
            name: "Recording panels".to_owned(),
            description: "Test backend that records display frames".to_owned(),
        }
    }

    fn adopt_device(&self, _discovered: &DiscoveredDevice) -> DeviceResult<()> {
        Ok(())
    }

    async fn connect(&self, _id: &DeviceId) -> DeviceResult<()> {
        Ok(())
    }

    async fn disconnect(&self, _id: &DeviceId) -> DeviceResult<()> {
        Ok(())
    }

    async fn retire(&self) -> DeviceResult<()> {
        Ok(())
    }

    async fn write_colors(&self, _id: &DeviceId, _colors: &[[u8; 3]]) -> DeviceResult<()> {
        Ok(())
    }

    async fn write_display_payload_owned(
        &self,
        id: &DeviceId,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> DeviceResult<()> {
        self.writes.record(*id, payload.as_ref());
        Ok(())
    }
}

/// A screen on the rig: a display-only device and the layout id its output
/// carries.
#[derive(Clone)]
struct Screen {
    device_id: DeviceId,
    layout_id: String,
}

impl Screen {
    /// A screen's identity, before its panel is connected.
    fn named(name: &str) -> Self {
        Self {
            device_id: DeviceId::new(),
            layout_id: format!("panel:{name}"),
        }
    }

    /// The screen's box on its zone's canvas.
    fn output(&self, position: (f32, f32), size: (f32, f32)) -> Output {
        Output {
            id: format!("{}-display", self.layout_id),
            name: "Screen".to_owned(),
            device_id: self.layout_id.clone(),
            zone_name: Some(DISPLAY_ZONE_NAME.to_owned()),
            position: NormalizedPosition::new(position.0, position.1),
            size: NormalizedPosition::new(size.0, size.1),
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
}

fn panel_info(
    device_id: DeviceId,
    format: DisplayFrameFormat,
    max_fps: u32,
    circular: bool,
    size: (u32, u32),
) -> DeviceInfo {
    DeviceInfo {
        id: device_id,
        name: "Test Panel".to_owned(),
        vendor: "Test".to_owned(),
        family: DeviceFamily::new_static("corsair", "Corsair"),
        model: None,
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("corsair", BACKEND_ID, ConnectionType::Usb),
        segments: vec![SegmentInfo {
            name: "Display".to_owned(),
            led_count: 0,
            topology: DeviceTopologyHint::Display {
                width: size.0,
                height: size.1,
                circular,
                format,
            },
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities {
            led_count: 0,
            supports_direct: false,
            supports_brightness: false,
            has_display: true,
            display_resolution: Some(size),
            max_fps,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

fn zone_layout(outputs: Vec<Output>) -> SpatialLayout {
    SpatialLayout {
        id: "screen-backdrop-test".into(),
        name: "Screen Backdrop Test".into(),
        description: None,
        canvas_width: CANVAS.0,
        canvas_height: CANVAS.1,
        zones: outputs,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

/// An LED strip output, so a zone that places a screen lights LEDs too.
fn strip_output(id: &str, position: (f32, f32), size: (f32, f32)) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: id.to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(position.0, position.1),
        size: NormalizedPosition::new(size.0, size.1),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 4,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn scene_zone(name: &str, role: ZoneRole, layers: Vec<SceneLayer>, outputs: Vec<Output>) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: name.into(),
        description: None,
        layers,
        layout: zone_layout(outputs),
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role,
        controls_version: 0,
        layers_version: 0,
    }
}

/// A display zone whose face lets half of the screen's backdrop through.
fn blending_face_zone(screen: &Screen, layers: Vec<SceneLayer>) -> Zone {
    let mut zone = scene_zone("Face", ZoneRole::Display, layers, Vec::new());
    zone.layout.canvas_width = PANEL;
    zone.layout.canvas_height = PANEL;
    zone.display_target = Some(DisplayFaceTarget {
        device_id: screen.device_id,
        blend_mode: BlendMode::Alpha,
        opacity: 0.5,
    });
    zone
}

/// A scene of `zones` that fades in over `transition_ms`.
fn test_scene(name: &str, zones: Vec<Zone>, transition_ms: u64) -> Scene {
    let mut scene: Scene = make_scene(name);
    scene.zones = zones;
    scene.unassigned_behavior = UnassignedBehavior::Off;
    scene.transition.duration_ms = transition_ms;
    scene
}

/// A layer that never changes: a fill has no renderer to produce new frames.
fn fill_layer(rgba: [f32; 4]) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: None,
        source: hypercolor_types::layer::LayerSource::ColorFill { rgba },
        blend: BlendMode::Replace,
        opacity: 1.0,
        transform: hypercolor_types::layer::LayerTransform::default(),
        adjust: hypercolor_types::layer::LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
        scope: None,
    }
}

/// The effects a test scene is built from.
struct Effects {
    solid: EffectId,
    color_zones: EffectId,
    rainbow: EffectId,
}

impl Effects {
    fn solid(&self, color: [f32; 4]) -> SceneLayer {
        SceneLayer::from_effect(
            SceneLayerId::new(),
            self.solid,
            HashMap::from([("color".into(), ControlValue::linear_color(color))]),
            HashMap::new(),
            None,
        )
    }

    /// Four hard-edged columns: red, green, blue, white.
    fn columns(&self) -> SceneLayer {
        SceneLayer::from_effect(
            SceneLayerId::new(),
            self.color_zones,
            HashMap::from([
                ("zone_count".into(), ControlValue::Enum("4".to_owned())),
                ("layout".into(), ControlValue::Enum("Columns".to_owned())),
                ("blend".into(), ControlValue::Float(0.0)),
                ("zone_1".into(), ControlValue::linear_color(RED)),
                ("zone_2".into(), ControlValue::linear_color(GREEN)),
                ("zone_3".into(), ControlValue::linear_color(BLUE)),
                ("zone_4".into(), ControlValue::linear_color(WHITE)),
            ]),
            HashMap::new(),
            None,
        )
    }

    /// A layer whose picture changes every frame.
    fn moving(&self) -> SceneLayer {
        SceneLayer::from_effect(
            SceneLayerId::new(),
            self.rainbow,
            HashMap::new(),
            HashMap::new(),
            None,
        )
    }
}

/// A render thread and a display output thread sharing one bus, one scene,
/// and one set of recording panels.
struct Rig {
    state: RenderThreadState,
    logical_devices: Arc<RwLock<HashMap<String, LogicalDevice>>>,
    writes: PanelWrites,
    effects: Effects,
    /// The power policy. Display workers stop when its sender goes away.
    power: watch::Sender<OutputPowerState>,
    /// What each panel last showed, for whoever previews it.
    display_frames: Arc<RwLock<DisplayFrameRuntime>>,
    /// The screens the scene manager is told about before a run.
    surfaces: ScreenSurfaces,
    /// The kept faces installed before a run.
    kept_faces: Vec<Zone>,
}

impl Rig {
    fn new(mode: RenderAccelerationMode) -> Self {
        let (power, power_state) = watch::channel(OutputPowerState::default());
        let event_bus = Arc::new(HypercolorBus::new());
        let scene_manager = SceneService::with_temporary_store(
            SceneManager::with_default(),
            Arc::clone(&event_bus),
        )
        .expect("temporary scene store should open");
        let scene_plan = scene_manager.plan_reader();
        let asset_dir = tempfile::tempdir()
            .expect("test asset tempdir should be created")
            .keep();
        let mut registry = EffectRegistry::new(Vec::new());
        register_builtin_effects(&mut registry);
        let builtin = |stem: &str| {
            registry
                .iter()
                .find_map(|(_, entry)| {
                    (entry.metadata.source.source_stem() == Some(stem)).then_some(entry.metadata.id)
                })
                .expect("builtin effect should exist")
        };
        let effects = Effects {
            solid: builtin("solid_color"),
            color_zones: builtin("color_zones"),
            rainbow: builtin("rainbow"),
        };
        let writes = PanelWrites::default();
        let mut backend_manager = BackendManager::new();
        for id in [BACKEND_ID, SIMULATED_DISPLAY_BACKEND_ID] {
            backend_manager.register_backend(Arc::new(PanelBackend {
                id,
                writes: writes.clone(),
            }));
        }
        let state = RenderThreadState {
            effect_registry: Arc::new(RwLock::new(registry)),
            asset_library: Arc::new(RwLock::new(
                AssetLibrary::open(asset_dir.join("assets"))
                    .expect("test asset library should open"),
            )),
            spatial_engine: SpatialService::new(SpatialEngine::new(zone_layout(Vec::new()))),
            backend_manager: Arc::new(Mutex::new(backend_manager)),
            device_registry: DeviceRegistry::new(),
            performance: Arc::new(RwLock::new(PerformanceTracker::default())),
            discovery_runtime: None,
            event_bus: Arc::clone(&event_bus),
            preview_runtime: Arc::new(PreviewRuntime::new(event_bus)),
            zone_layout_previews: Arc::new(
                hypercolor_daemon::zone_layout_preview::ZoneLayoutPreviewStore::default(),
            ),
            render_loop: Arc::new(RwLock::new(RenderLoop::new(60))),
            scene_manager,
            scene_plan,
            input_manager: InputManager::new(),
            interaction_routing:
                hypercolor_daemon::interaction_routing::InteractionRoutingControl::default(),
            power_state,
            scene_transactions: SceneTransactionQueue::default(),
            screen_capture_configured: false,
            canvas_dims: CanvasDims::new(CANVAS.0, CANVAS.1),
            render_acceleration_mode: mode,
            #[cfg(feature = "wgpu")]
            render_gpu_device: None,
            configured_max_fps_tier: FpsTier::Full.into(),
            face_fps_cap: 30,
        };
        Self {
            state,
            logical_devices: Arc::new(RwLock::new(HashMap::new())),
            writes,
            effects,
            power,
            display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
            surfaces: ScreenSurfaces::new(),
            kept_faces: Vec::new(),
        }
    }

    /// Connect a raw RGB panel.
    async fn add_screen(&self, name: &str) -> Screen {
        self.add_screen_with(name, DisplayFrameFormat::Rgb, 30)
            .await
    }

    async fn add_screen_with(
        &self,
        name: &str,
        format: DisplayFrameFormat,
        max_fps: u32,
    ) -> Screen {
        let screen = Screen::named(name);
        self.connect_screen(&screen, format, max_fps).await;
        screen
    }

    /// Connect a square panel whose box a layout may already hold.
    async fn connect_screen(&self, screen: &Screen, format: DisplayFrameFormat, max_fps: u32) {
        self.connect_panel(screen, format, max_fps, false).await;
    }

    /// Connect a panel, square or circular, whose box a layout may already
    /// hold.
    async fn connect_panel(
        &self,
        screen: &Screen,
        format: DisplayFrameFormat,
        max_fps: u32,
        circular: bool,
    ) {
        self.connect_panel_as(screen, format, max_fps, circular, false, (PANEL, PANEL))
            .await;
    }

    /// Connect a raw RGB panel of a given size.
    async fn connect_sized(&self, screen: &Screen, size: (u32, u32)) {
        self.connect_panel_as(screen, DisplayFrameFormat::Rgb, 30, false, false, size)
            .await;
    }

    /// Connect a simulated panel: one that display output drives only
    /// while somebody previews it, and that is nobody's physical output.
    async fn connect_simulator(&self, screen: &Screen) {
        self.connect_panel_as(
            screen,
            DisplayFrameFormat::Rgb,
            30,
            false,
            true,
            (PANEL, PANEL),
        )
        .await;
    }

    async fn connect_panel_as(
        &self,
        screen: &Screen,
        format: DisplayFrameFormat,
        max_fps: u32,
        circular: bool,
        simulator: bool,
        size: (u32, u32),
    ) {
        let device_id = screen.device_id;
        let layout_id = screen.layout_id.clone();
        let name = layout_id.as_str();
        self.logical_devices.write().await.insert(
            layout_id.clone(),
            LogicalDevice {
                id: layout_id.clone(),
                physical_device_id: device_id,
                name: name.to_owned(),
                led_start: 0,
                led_count: 0,
                enabled: true,
                kind: LogicalDeviceKind::Default,
            },
        );
        let backend_id = if simulator {
            SIMULATED_DISPLAY_BACKEND_ID
        } else {
            BACKEND_ID
        };
        self.state
            .backend_manager
            .lock()
            .await
            .connect_device(backend_id, device_id, &layout_id)
            .await
            .expect("panel should connect");
        let mut info = panel_info(device_id, format, max_fps, circular, size);
        info.origin = DeviceOrigin::native("corsair", backend_id, ConnectionType::Usb);
        let tracked = if simulator {
            self.state
                .device_registry
                .add_with_fingerprint_and_metadata(
                    info,
                    hypercolor_types::device::DeviceFingerprint::from_persisted(format!(
                        "simulator:{device_id}"
                    )),
                    HashMap::from([("simulator".to_owned(), "true".to_owned())]),
                )
                .await
        } else {
            self.state.device_registry.add(info).await
        };
        assert_eq!(tracked, device_id);
        assert!(
            self.state
                .device_registry
                .set_state(&device_id, DeviceState::Active)
                .await
        );
    }

    /// Make `scene` the live scene of a running rig.
    async fn activate(&self, scene: &Scene) {
        let mut mutation = self.state.scene_manager.begin_mutation().await;
        // A scene the rig has shown before is already registered.
        let _ = mutation.create_scene(scene.clone());
        mutation
            .activate(
                scene.id,
                None,
                hypercolor_types::event::SceneChangeReason::UserActivate,
            )
            .expect("scene should activate");
        self.state
            .scene_manager
            .commit_mutation(mutation)
            .await
            .expect("scene activation should commit");
    }

    /// Make `zones` the live scene and start both threads.
    async fn run(&mut self, zones: Vec<Zone>) -> RunningRig {
        self.run_scene(test_scene("Screen Backdrops", zones, 0))
            .await
    }

    /// Make `scene` the live scene and start both threads.
    async fn run_scene(&mut self, scene: Scene) -> RunningRig {
        self.run_scene_with(scene, false).await.0
    }

    /// Make `scene` the live scene and start both threads, converting its
    /// stored display zones first when asked, as activation does.
    async fn run_scene_with(
        &mut self,
        scene: Scene,
        convert: bool,
    ) -> (RunningRig, ScreenConversion) {
        let mut manager = self.state.scene_manager.snapshot().await;
        manager.set_screen_surfaces(self.surfaces.clone());
        for kept in &self.kept_faces {
            manager.set_default_display_zone(kept.clone());
        }
        manager.create(scene.clone()).expect("create scene");
        manager.activate(&scene.id, None).expect("activate scene");
        let conversion = if convert {
            manager.convert_legacy_display_zones(&scene.id)
        } else {
            ScreenConversion::default()
        };
        let scene_manager =
            SceneService::with_temporary_store(manager, Arc::clone(&self.state.event_bus))
                .expect("temporary scene store should open");
        self.state.scene_plan = scene_manager.plan_reader();
        self.state.scene_manager = scene_manager;

        let display = DisplayOutputThread::spawn(DisplayOutputState {
            backend_manager: Arc::clone(&self.state.backend_manager),
            device_registry: self.state.device_registry.clone(),
            logical_devices: Arc::clone(&self.logical_devices),
            event_bus: Arc::clone(&self.state.event_bus),
            power_state: self.state.power_state.clone(),
            static_hold_refresh_interval:
                hypercolor_daemon::display_output::DEFAULT_STATIC_HOLD_REFRESH_INTERVAL,
            display_frames: Arc::clone(&self.display_frames),
            face_fps_cap: self.state.face_fps_cap,
        });
        self.state.render_loop.write().await.start();
        let render = RenderThread::spawn(self.state.clone());
        (
            RunningRig {
                render,
                display,
                render_loop: Arc::clone(&self.state.render_loop),
            },
            conversion,
        )
    }
}

struct RunningRig {
    render: RenderThread,
    display: DisplayOutputThread,
    render_loop: Arc<RwLock<RenderLoop>>,
}

impl RunningRig {
    async fn stop(mut self) {
        self.render_loop.write().await.stop();
        self.render.shutdown().await.expect("render thread stops");
        self.display.shutdown().await.expect("display thread stops");
    }
}

/// Two screens whose boxes overlap, in two zones with different fills: each
/// panel shows its own zone. Before each screen resolved its own backdrop,
/// both cropped one flattened scene canvas, where the later box had painted
/// over the earlier one.
async fn overlapping_screens_show_their_own_zones(mode: RenderAccelerationMode, with_faces: bool) {
    let mut rig = Rig::new(mode);
    let red_screen = rig.add_screen("red").await;
    let blue_screen = rig.add_screen("blue").await;
    let mut zones = vec![
        scene_zone(
            "Red",
            ZoneRole::Primary,
            vec![rig.effects.solid(RED)],
            vec![
                strip_output("strip:red", (0.5, 0.1), (0.8, 0.1)),
                red_screen.output((0.5, 0.5), (0.4, 0.4)),
            ],
        ),
        scene_zone(
            "Blue",
            ZoneRole::Custom,
            vec![rig.effects.solid(BLUE)],
            vec![
                strip_output("strip:blue", (0.5, 0.9), (0.8, 0.1)),
                blue_screen.output((0.55, 0.5), (0.4, 0.4)),
            ],
        ),
    ];
    if with_faces {
        zones.push(blending_face_zone(
            &red_screen,
            vec![rig.effects.solid(GREEN)],
        ));
        zones.push(blending_face_zone(
            &blue_screen,
            vec![rig.effects.solid(GREEN)],
        ));
    }

    let running = rig.run(zones).await;
    if with_faces {
        // Half a green face over each zone: the zone's own channel shows
        // through, and the other zone's never does.
        let red = rig
            .writes
            .wait_for(red_screen.device_id, "green over red", |panel| {
                let [r, g, b] = panel.center();
                r > 24 && g > 24 && b <= 3
            })
            .await;
        let blue = rig
            .writes
            .wait_for(blue_screen.device_id, "green over blue", |panel| {
                let [r, g, b] = panel.center();
                b > 24 && g > 24 && r <= 3
            })
            .await;
        assert_eq!(red.format, DisplayFrameFormat::Rgb);
        assert_eq!(blue.format, DisplayFrameFormat::Rgb);
    } else {
        rig.writes
            .wait_for(red_screen.device_id, "its red zone", |panel| {
                near(panel.center(), [255, 0, 0])
            })
            .await;
        rig.writes
            .wait_for(blue_screen.device_id, "its blue zone", |panel| {
                near(panel.center(), [0, 0, 255])
            })
            .await;
    }
    // The first frames could have been right by luck; the panels keep
    // showing their own zone on every frame that follows.
    let red_seen = rig.writes.of(red_screen.device_id).len();
    let blue_seen = rig.writes.of(blue_screen.device_id).len();
    tokio::time::sleep(Duration::from_millis(250)).await;
    running.stop().await;
    for write in rig
        .writes
        .of(red_screen.device_id)
        .iter()
        .skip(red_seen.saturating_sub(1))
    {
        let [r, _, b] = Panel(&write.data).center();
        assert!(
            r > 24 && b <= 3,
            "the red zone's panel showed {:?}",
            Panel(&write.data).center()
        );
    }
    for write in rig
        .writes
        .of(blue_screen.device_id)
        .iter()
        .skip(blue_seen.saturating_sub(1))
    {
        let [r, _, b] = Panel(&write.data).center();
        assert!(
            b > 24 && r <= 3,
            "the blue zone's panel showed {:?}",
            Panel(&write.data).center()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_overlapping_screens_without_faces_show_their_own_zones() {
    overlapping_screens_show_their_own_zones(RenderAccelerationMode::Cpu, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_overlapping_screens_with_faces_show_their_own_zones() {
    overlapping_screens_show_their_own_zones(RenderAccelerationMode::Cpu, true).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_overlapping_screens_without_faces_show_their_own_zones() {
    overlapping_screens_show_their_own_zones(RenderAccelerationMode::Gpu, false).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_overlapping_screens_with_faces_show_their_own_zones() {
    overlapping_screens_show_their_own_zones(RenderAccelerationMode::Gpu, true).await;
}

// ── One zone: every panel against a reference ────────────────────────────

/// The four columns of the parity scene, left to right, each a quarter of
/// the canvas wide.
const COLUMNS: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];

/// The face of the faced panel: opaque green, laid on at half strength.
const FACE: [u8; 3] = [0, 255, 0];

/// One panel of the parity scene.
struct ParityPanel {
    name: &'static str,
    format: DisplayFrameFormat,
    circular: bool,
    mount: DisplayRotation,
    faced: bool,
    /// The panel's box on the zone's canvas: its center and its size.
    placed: ((f32, f32), (f32, f32)),
}

/// Both output formats, a circular panel, a panel mounted a quarter turn
/// round, and a panel with a face. Every box straddles a column edge, so a
/// crop that is off by a pixel, mirrored or turned the wrong way shows.
const PARITY_PANELS: [ParityPanel; 5] = [
    ParityPanel {
        name: "rgb",
        format: DisplayFrameFormat::Rgb,
        circular: false,
        mount: DisplayRotation::Deg0,
        faced: false,
        placed: ((0.5, 0.6), (0.3, 0.3)),
    },
    ParityPanel {
        name: "jpeg",
        format: DisplayFrameFormat::Jpeg,
        circular: false,
        mount: DisplayRotation::Deg0,
        faced: false,
        placed: ((0.25, 0.3), (0.3, 0.3)),
    },
    ParityPanel {
        name: "round",
        format: DisplayFrameFormat::Rgb,
        circular: true,
        mount: DisplayRotation::Deg0,
        faced: false,
        placed: ((0.75, 0.6), (0.3, 0.3)),
    },
    ParityPanel {
        name: "turned",
        format: DisplayFrameFormat::Rgb,
        circular: false,
        mount: DisplayRotation::Deg90,
        faced: false,
        placed: ((0.5, 0.25), (0.3, 0.3)),
    },
    ParityPanel {
        name: "faced",
        format: DisplayFrameFormat::Rgb,
        circular: false,
        mount: DisplayRotation::Deg0,
        faced: true,
        placed: ((0.75, 0.3), (0.3, 0.3)),
    },
];

fn small(value: u32) -> f32 {
    f32::from(u16::try_from(value).expect("test coordinates are small"))
}

/// The column a normalized canvas x falls in.
fn column_at(canvas_x: f32) -> [u8; 3] {
    let quarter = canvas_x.clamp(0.0, 1.0) * 4.0;
    COLUMNS
        .iter()
        .zip([1.0_f32, 2.0, 3.0, 4.0])
        .find_map(|(color, right_edge)| (quarter < right_edge).then_some(*color))
        .unwrap_or(COLUMNS[3])
}

fn srgb_to_linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    let rounded = (encoded.clamp(0.0, 1.0) * 255.0).round();
    (0..=u8::MAX)
        .find(|byte| f32::from(*byte) >= rounded)
        .unwrap_or(u8::MAX)
}

/// The face at half strength over `backdrop`: mixed in encoded bytes, and
/// mixed in linear light. A compositor may do either, and nothing else.
fn faced(backdrop: [u8; 3]) -> [[u8; 3]; 2] {
    let encoded = std::array::from_fn(|channel| {
        u8::try_from((u16::from(backdrop[channel]) + u16::from(FACE[channel])).div_ceil(2))
            .expect("the mean of two bytes is a byte")
    });
    let linear = std::array::from_fn(|channel| {
        linear_to_srgb(f32::midpoint(
            srgb_to_linear(backdrop[channel]),
            srgb_to_linear(FACE[channel]),
        ))
    });
    [encoded, linear]
}

/// The colors a panel pixel may take, from a reference that knows the scene
/// without drawing it.
///
/// The scene canvas of a lone zone is four columns. A panel pixel at panel
/// point `p`, measured from the panel's center in panel widths, shows the
/// canvas at `center + turn(p * size)`, where the turn is the panel's mount
/// rotation, clockwise. That is the crop a panel showed when every screen
/// cropped the one scene canvas, restated here with nothing shared with the
/// code that draws it.
///
/// Resampling may pull a neighbor into a pixel, so a pixel answers for the
/// eight around it too: inside one column that is one color, and across an
/// edge it is any mix of the two. A circular panel is black outside its
/// circle, and its rim may be either.
fn reference_pixel(panel: &ParityPanel, x: u32, y: u32) -> Vec<[u8; 3]> {
    let side = small(PANEL);
    let ((center_x, _), (width, height)) = panel.placed;
    let (sin, cos) = panel.mount.radians().sin_cos();
    let mut colors = Vec::new();
    for neighbor_y in [-1.0_f32, 0.0, 1.0] {
        for neighbor_x in [-1.0_f32, 0.0, 1.0] {
            let across = ((small(x) + 0.5 + neighbor_x) / side - 0.5) * width;
            let down = ((small(y) + 0.5 + neighbor_y) / side - 0.5) * height;
            let backdrop = column_at(center_x + across * cos - down * sin);
            if panel.faced {
                colors.extend(faced(backdrop));
            } else {
                colors.push(backdrop);
            }
        }
    }
    if panel.circular {
        let radius = side / 2.0;
        let from_center = (small(x) + 0.5 - radius).hypot(small(y) + 0.5 - radius);
        if from_center > radius + 1.0 {
            return vec![[0, 0, 0]];
        }
        if from_center > radius - 1.0 {
            colors.push([0, 0, 0]);
        }
    }
    colors.sort_unstable();
    colors.dedup();
    colors
}

/// Whether `actual` lies among `allowed`: each channel between the least
/// and the greatest it may be, give or take `tolerance`.
fn among(actual: [u8; 3], allowed: &[[u8; 3]], tolerance: u8) -> bool {
    (0..3).all(|channel| {
        let least = allowed.iter().map(|color| color[channel]).min();
        let greatest = allowed.iter().map(|color| color[channel]).max();
        least.zip(greatest).is_some_and(|(least, greatest)| {
            actual[channel] >= least.saturating_sub(tolerance)
                && actual[channel] <= greatest.saturating_add(tolerance)
        })
    })
}

/// A panel frame as raw RGB, decoded if it was sent as JPEG.
fn decoded(write: &PanelWrite) -> Vec<u8> {
    if write.format == DisplayFrameFormat::Rgb {
        return write.data.clone();
    }
    let side = usize::try_from(PANEL).expect("panel side fits usize");
    let mut rgb = vec![0; side * side * 3];
    turbojpeg::Decompressor::new()
        .expect("the JPEG decoder should start")
        .decompress(
            &write.data,
            turbojpeg::Image {
                pixels: rgb.as_mut_slice(),
                width: side,
                pitch: side * 3,
                height: side,
                format: turbojpeg::PixelFormat::RGB,
            },
        )
        .expect("the panel's JPEG should decode");
    rgb
}

/// Every pixel of `frame` that the reference does not allow.
fn mismatches(panel: &ParityPanel, frame: &[u8]) -> Vec<String> {
    // JPEG rings around an edge, and a 16 pixel panel is all edge.
    let tolerance = match panel.format {
        DisplayFrameFormat::Rgb => 3,
        DisplayFrameFormat::Jpeg => 40,
    };
    let picture = Panel(frame);
    (0..PANEL)
        .flat_map(|y| (0..PANEL).map(move |x| (x, y)))
        .filter_map(|(x, y)| {
            let allowed = reference_pixel(panel, x, y);
            let actual = picture.pixel(x, y);
            (!among(actual, &allowed, tolerance))
                .then(|| format!("({x}, {y}) is {actual:?}, not among {allowed:?}"))
        })
        .collect()
}

/// One zone at the scene extent, no overlapping zones: every panel shows,
/// in every pixel, the crop of the scene canvas its layout box names. That
/// is what each panel showed when all screens cropped the one scene canvas,
/// and the scene canvas of a lone zone is that zone's frame.
///
/// The published scene canvas is checked against the same columns first, so
/// the reference is a statement about the scene canvas and not only about
/// the panels.
///
/// One panel is a stated exception to "what it showed before". On a
/// compositor that does not finalize, a face routed to a screen used not to
/// be shown at all: the worker cropped the scene canvas and sent that. The
/// worker now blends the face over the backdrop, as the compositor that
/// finalizes always did. So the faced panel is held to the blend on both
/// backends, which on that backend is a change made on purpose.
async fn lone_zone_panels_match_the_reference_crop(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let mut screens = Vec::new();
    for panel in &PARITY_PANELS {
        let screen = Screen::named(panel.name);
        rig.connect_panel(&screen, panel.format, 30, panel.circular)
            .await;
        rig.state
            .device_registry
            .update_user_settings(&screen.device_id, None, None, None, Some(panel.mount))
            .await
            .expect("the panel is registered");
        screens.push(screen);
    }
    let mut outputs = vec![strip_output("strip:all", (0.5, 0.05), (0.9, 0.05))];
    outputs.extend(
        PARITY_PANELS
            .iter()
            .zip(&screens)
            .map(|(panel, screen)| screen.output(panel.placed.0, panel.placed.1)),
    );
    let mut zones = vec![scene_zone(
        "Columns",
        ZoneRole::Primary,
        vec![rig.effects.columns()],
        outputs,
    )];
    for (panel, screen) in PARITY_PANELS.iter().zip(&screens) {
        if panel.faced {
            zones.push(blending_face_zone(screen, vec![rig.effects.solid(GREEN)]));
        }
    }
    let mut canvas_rx = rig.state.event_bus.canvas_receiver();

    let running = rig.run(zones).await;
    let deadline = Instant::now() + WAIT;
    let latest = loop {
        let latest = PARITY_PANELS
            .iter()
            .zip(&screens)
            .map(|(panel, screen)| {
                rig.writes.of(screen.device_id).pop().map_or_else(
                    || vec!["no frame yet".to_owned()],
                    |write| mismatches(panel, &decoded(&write)),
                )
            })
            .collect::<Vec<_>>();
        if latest.iter().all(Vec::is_empty) || Instant::now() >= deadline {
            break latest;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    tokio::time::timeout(WAIT, canvas_rx.wait_for(|frame| frame.width == CANVAS.0))
        .await
        .expect("the scene canvas should publish")
        .expect("canvas stream should stay open");
    let canvas = canvas_rx.borrow().clone();
    running.stop().await;

    // The scene canvas is the columns the reference describes, away from
    // the one pixel either side of each edge.
    for y in 0..CANVAS.1 {
        for x in 0..CANVAS.0 {
            let left = column_at((small(x) - 0.5) / small(CANVAS.0));
            let right = column_at((small(x) + 1.5) / small(CANVAS.0));
            if left != right {
                continue;
            }
            let pixel = canvas.surface().get_pixel(x, y);
            assert!(
                near([pixel.r, pixel.g, pixel.b], left),
                "scene canvas pixel ({x}, {y}) is {pixel:?}, not the column {left:?}"
            );
        }
    }
    for (panel, wrong) in PARITY_PANELS.iter().zip(&latest) {
        assert!(
            wrong.is_empty(),
            "the {} panel differs from the reference crop in {} pixels: {:?}",
            panel.name,
            wrong.len(),
            wrong.iter().take(6).collect::<Vec<_>>()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_lone_zone_panels_match_the_reference_crop() {
    lone_zone_panels_match_the_reference_crop(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_lone_zone_panels_match_the_reference_crop() {
    lone_zone_panels_match_the_reference_crop(RenderAccelerationMode::Gpu).await;
}

/// A screen turned in its zone's layout shows its backdrop turned once, the
/// same way on a compositor that finalizes the frame and on one that leaves
/// the crop to the screen's worker.
async fn turned_screen_shows_its_backdrop_turned_once(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("turned").await;
    // Over the green and blue columns, turned half way round: blue lands on
    // the panel's left and green on its right.
    let mut turned = screen.output((0.5, 0.5), (0.3, 0.3));
    turned.rotation = std::f32::consts::PI;
    let zones = vec![scene_zone(
        "Columns",
        ZoneRole::Primary,
        vec![rig.effects.columns()],
        vec![strip_output("strip:all", (0.5, 0.05), (0.9, 0.05)), turned],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(screen.device_id, "blue left of green", |panel| {
            near(panel.pixel(1, PANEL / 2), [0, 0, 255])
                && near(panel.pixel(PANEL - 2, PANEL / 2), [0, 255, 0])
        })
        .await;
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_turned_screen_shows_its_backdrop_turned_once() {
    turned_screen_shows_its_backdrop_turned_once(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_turned_screen_shows_its_backdrop_turned_once() {
    turned_screen_shows_its_backdrop_turned_once(RenderAccelerationMode::Gpu).await;
}

/// A screen whose zone renders nothing shows black, while a screen in a zone
/// that renders keeps its picture: a zone's pixels never reach a screen
/// another zone places, even through an overlapping box.
async fn idle_zone_screens_show_black(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let lit_screen = rig.add_screen("lit").await;
    let idle_screen = rig.add_screen("idle").await;
    let zones = vec![
        scene_zone(
            "Lit",
            ZoneRole::Primary,
            vec![rig.effects.solid(RED)],
            vec![lit_screen.output((0.5, 0.5), (0.6, 0.6))],
        ),
        scene_zone(
            "Idle",
            ZoneRole::Custom,
            Vec::new(),
            vec![idle_screen.output((0.5, 0.5), (0.3, 0.3))],
        ),
    ];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(lit_screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.writes
        .wait_for(idle_screen.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    running.stop().await;

    for write in rig.writes.of(idle_screen.device_id) {
        assert!(
            near(Panel(&write.data).center(), [0, 0, 0]),
            "a screen in a zone that renders nothing showed {:?}",
            Panel(&write.data).center()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_idle_zone_screens_show_black() {
    idle_zone_screens_show_black(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_idle_zone_screens_show_black() {
    idle_zone_screens_show_black(RenderAccelerationMode::Gpu).await;
}

/// A scene in which no zone renders still drives its placed screens, to
/// black, once: a still picture is not sent again every cadence tick.
async fn idle_scene_sends_each_placed_screen_one_black_frame(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("idle").await;
    let zones = vec![scene_zone(
        "Idle",
        ZoneRole::Primary,
        Vec::new(),
        vec![screen.output((0.5, 0.5), (0.5, 0.5))],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(screen.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    running.stop().await;

    let writes = rig.writes.of(screen.device_id);
    assert!(
        writes.len() <= 2,
        "an idle scene sent its screen {} frames in over half a second",
        writes.len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_idle_scene_sends_each_placed_screen_one_black_frame() {
    idle_scene_sends_each_placed_screen_one_black_frame(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_idle_scene_sends_each_placed_screen_one_black_frame() {
    idle_scene_sends_each_placed_screen_one_black_frame(RenderAccelerationMode::Gpu).await;
}

/// A screen without a face holds the cadence cap of a screen showing its
/// backdrop alone (15 fps encoded, 30 fps raw), and a screen with a face
/// holds the face cadence (30 fps), on a 60 fps loop over devices that
/// accept 60.
async fn screens_hold_their_cadence(mode: RenderAccelerationMode) {
    const WINDOW: Duration = Duration::from_millis(1500);
    let mut rig = Rig::new(mode);
    let encoded = rig
        .add_screen_with("encoded", DisplayFrameFormat::Jpeg, 60)
        .await;
    let raw = rig
        .add_screen_with("raw", DisplayFrameFormat::Rgb, 60)
        .await;
    let faced = rig
        .add_screen_with("faced", DisplayFrameFormat::Jpeg, 60)
        .await;
    let zones = vec![
        scene_zone(
            "Moving",
            ZoneRole::Primary,
            vec![rig.effects.moving()],
            vec![
                strip_output("strip:all", (0.5, 0.05), (0.9, 0.05)),
                encoded.output((0.2, 0.5), (0.3, 0.6)),
                raw.output((0.5, 0.5), (0.3, 0.6)),
                faced.output((0.8, 0.5), (0.3, 0.6)),
            ],
        ),
        blending_face_zone(&faced, vec![rig.effects.moving()]),
    ];

    let running = rig.run(zones).await;
    for screen in [&encoded, &raw, &faced] {
        let started = Instant::now();
        while rig.writes.of(screen.device_id).len() < 3 {
            assert!(started.elapsed() < WAIT, "a panel never started updating");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let window_start = Instant::now();
    tokio::time::sleep(WINDOW).await;
    let window_end = Instant::now();
    running.stop().await;

    let frames_in_window = |screen: &Screen| {
        rig.writes
            .of(screen.device_id)
            .iter()
            .filter(|write| write.at >= window_start && write.at < window_end)
            .count()
    };
    let (encoded_frames, raw_frames, faced_frames) = (
        frames_in_window(&encoded),
        frames_in_window(&raw),
        frames_in_window(&faced),
    );
    // A cap is a ceiling the pacing never exceeds, give or take the frame
    // that straddles a window edge. The floors are loose so a loaded
    // machine fails only a lane that lost most of its frames.
    assert!(
        (8..=25).contains(&encoded_frames),
        "an encoded panel showing its backdrop took {encoded_frames} frames in 1.5 s at a 15 fps cap"
    );
    assert!(
        (16..=48).contains(&raw_frames),
        "a raw panel showing its backdrop took {raw_frames} frames in 1.5 s at a 30 fps cap"
    );
    assert!(
        (16..=48).contains(&faced_frames),
        "a panel showing a face took {faced_frames} frames in 1.5 s at the 30 fps face cadence"
    );
    assert!(
        faced_frames > encoded_frames,
        "the face ({faced_frames} frames) should outpace the encoded backdrop ({encoded_frames})"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screens_hold_their_cadence() {
    screens_hold_their_cadence(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screens_hold_their_cadence() {
    screens_hold_their_cadence(RenderAccelerationMode::Gpu).await;
}

/// Switching between two scenes that light a screen never shows the screen
/// black. A screen's zone is found in the scene its frame renders, so no
/// frame is resolved against another scene's zones. Each switch is
/// acknowledged by a frame of the scene switched to before the next one, so
/// a screen that stopped updating fails here instead of passing unseen.
async fn scene_switch_never_shows_a_lit_screen_black(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("lit").await;
    let red = lit_scene(&rig.effects, &screen, "Red", RED, 0, false);
    let green = lit_scene(&rig.effects, &screen, "Green", GREEN, 0, false);

    let running = rig.run_scene(red.clone()).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    let seen = rig.writes.of(screen.device_id).len();
    for _ in 0..10 {
        rig.activate(&green).await;
        rig.writes
            .wait_for(screen.device_id, "green", |panel| {
                near(panel.center(), [0, 255, 0])
            })
            .await;
        rig.activate(&red).await;
        rig.writes
            .wait_for(screen.device_id, "red", |panel| {
                near(panel.center(), [255, 0, 0])
            })
            .await;
    }
    running.stop().await;

    let shown = centers_since(&rig.writes, screen.device_id, seen);
    assert!(
        shown.len() >= 20,
        "ten round trips sent {} frames",
        shown.len()
    );
    assert!(
        shown
            .iter()
            .all(|center| near(*center, [255, 0, 0]) || near(*center, [0, 255, 0])),
        "a scene switch showed the screen something that is neither scene: {shown:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_scene_switch_never_shows_a_lit_screen_black() {
    scene_switch_never_shows_a_lit_screen_black(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_scene_switch_never_shows_a_lit_screen_black() {
    scene_switch_never_shows_a_lit_screen_black(RenderAccelerationMode::Gpu).await;
}

/// A screen no zone of the scene places has no backdrop, and with no face
/// either it is not driven: placing a screen is the scene's decision, read
/// by the render thread from the scene it renders.
async fn unplaced_screen_is_not_driven(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let placed = rig.add_screen("placed").await;
    let unplaced = rig.add_screen("unplaced").await;
    let zones = vec![scene_zone(
        "Red",
        ZoneRole::Primary,
        vec![rig.effects.solid(RED)],
        vec![
            strip_output("strip:red", (0.5, 0.1), (0.8, 0.1)),
            placed.output((0.5, 0.5), (0.4, 0.4)),
        ],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(placed.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.stop().await;

    assert!(
        rig.writes.of(unplaced.device_id).is_empty(),
        "a screen no zone places was sent {} frames",
        rig.writes.of(unplaced.device_id).len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_unplaced_screen_is_not_driven() {
    unplaced_screen_is_not_driven(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_unplaced_screen_is_not_driven() {
    unplaced_screen_is_not_driven(RenderAccelerationMode::Gpu).await;
}

/// Two screens whose boxes overlap in two zones each fade between their own
/// zone's pictures while one scene fades into another with the same
/// arrangement. The red zone's screen goes to green and the blue zone's to
/// white, so the first never has any blue and the second never loses any:
/// a frame that read the other zone, or one flattened picture of both,
/// breaks one of the two. Each fade is seen part way and seen to finish.
async fn overlapping_screens_keep_their_zones_through_a_transition(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let warm_screen = rig.add_screen("warm").await;
    let cool_screen = rig.add_screen("cool").await;
    let scene = |name: &str, warm: [f32; 4], cool: [f32; 4], transition_ms: u64| {
        test_scene(
            name,
            vec![
                scene_zone(
                    "Warm",
                    ZoneRole::Primary,
                    vec![rig.effects.solid(warm)],
                    vec![
                        strip_output("strip:warm", (0.5, 0.1), (0.8, 0.1)),
                        warm_screen.output((0.5, 0.5), (0.4, 0.4)),
                    ],
                ),
                scene_zone(
                    "Cool",
                    ZoneRole::Custom,
                    vec![rig.effects.solid(cool)],
                    vec![
                        strip_output("strip:cool", (0.5, 0.9), (0.8, 0.1)),
                        cool_screen.output((0.55, 0.5), (0.4, 0.4)),
                    ],
                ),
            ],
            transition_ms,
        )
    };
    let first = scene("First", RED, BLUE, 0);
    let second = scene("Second", GREEN, WHITE, 2500);

    let running = rig.run_scene(first).await;
    rig.writes
        .wait_for(warm_screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.writes
        .wait_for(cool_screen.device_id, "its blue zone", |panel| {
            near(panel.center(), [0, 0, 255])
        })
        .await;
    let warm_seen = rig.writes.of(warm_screen.device_id).len();
    let cool_seen = rig.writes.of(cool_screen.device_id).len();
    rig.activate(&second).await;
    rig.writes
        .wait_for(warm_screen.device_id, "red part way to green", |panel| {
            let [r, g, _] = panel.center();
            (50..=205).contains(&r) && (50..=205).contains(&g)
        })
        .await;
    rig.writes
        .wait_for(cool_screen.device_id, "blue part way to white", |panel| {
            let [r, g, _] = panel.center();
            (50..=205).contains(&r) && (50..=205).contains(&g)
        })
        .await;
    rig.writes
        .wait_for(warm_screen.device_id, "its green zone", |panel| {
            near(panel.center(), [0, 255, 0])
        })
        .await;
    rig.writes
        .wait_for(cool_screen.device_id, "its white zone", |panel| {
            near(panel.center(), [255, 255, 255])
        })
        .await;
    running.stop().await;

    let warm = centers_since(&rig.writes, warm_screen.device_id, warm_seen);
    assert!(
        warm.iter().all(|[_, _, b]| *b <= 6),
        "the warm zone's panel showed blue it never had: {warm:?}"
    );
    let cool = centers_since(&rig.writes, cool_screen.device_id, cool_seen);
    assert!(
        cool.iter()
            .all(|[r, g, b]| *b >= 249 && r.abs_diff(*g) <= 6),
        "the cool zone's panel left the line from blue to white: {cool:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_overlapping_screens_keep_their_zones_through_a_transition() {
    overlapping_screens_keep_their_zones_through_a_transition(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_overlapping_screens_keep_their_zones_through_a_transition() {
    overlapping_screens_keep_their_zones_through_a_transition(RenderAccelerationMode::Gpu).await;
}

/// A lit screen whose zone renders nothing in the next scene fades to black
/// with the scene, and fades back in from black: black is one end of the
/// crossfade, not a cut at its start.
async fn screen_fades_with_the_scene_when_its_zone_goes_dark(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("fading").await;
    let scene = |name: &str, screen_layers: Vec<SceneLayer>| {
        test_scene(
            name,
            vec![
                scene_zone(
                    "Lit",
                    ZoneRole::Primary,
                    vec![rig.effects.solid(GREEN)],
                    vec![strip_output("strip:lit", (0.5, 0.1), (0.8, 0.1))],
                ),
                scene_zone(
                    "Screen",
                    ZoneRole::Custom,
                    screen_layers,
                    vec![
                        strip_output("strip:screen", (0.5, 0.9), (0.8, 0.1)),
                        screen.output((0.5, 0.5), (0.4, 0.4)),
                    ],
                ),
            ],
            900,
        )
    };
    let lit = scene("Lit", vec![rig.effects.solid(RED)]);
    let dark = scene("Dark", Vec::new());
    let mid_fade = |panel: &Panel<'_>| {
        let [r, g, b] = panel.center();
        (40..=215).contains(&r) && g <= 3 && b <= 3
    };

    let running = rig.run_scene(lit.clone()).await;
    rig.writes
        .wait_for(screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    for (scene, end, what) in [(&dark, [0, 0, 0], "black"), (&lit, [255, 0, 0], "red")] {
        let seen = rig.writes.of(screen.device_id).len();
        rig.activate(scene).await;
        rig.writes
            .wait_for(screen.device_id, what, |panel| near(panel.center(), end))
            .await;
        let fade = rig.writes.of(screen.device_id);
        assert!(
            fade.iter()
                .skip(seen)
                .any(|write| mid_fade(&Panel(&write.data))),
            "the screen cut to {what} without fading: {:?}",
            fade.iter()
                .skip(seen)
                .map(|write| Panel(&write.data).center())
                .collect::<Vec<_>>()
        );
    }
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_fades_with_the_scene_when_its_zone_goes_dark() {
    screen_fades_with_the_scene_when_its_zone_goes_dark(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_fades_with_the_scene_when_its_zone_goes_dark() {
    screen_fades_with_the_scene_when_its_zone_goes_dark(RenderAccelerationMode::Gpu).await;
}

/// A scene that renders nothing at all still gets its fade: the render loop
/// idles once it has shown the incoming black, but the transition keeps
/// stepping, so the screen fades out instead of freezing on its first step.
async fn screen_fades_out_when_the_whole_scene_goes_dark(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("fading-out").await;
    let scene = |name: &str, layers: Vec<SceneLayer>| {
        test_scene(
            name,
            vec![scene_zone(
                "Only",
                ZoneRole::Primary,
                layers,
                vec![
                    strip_output("strip:only", (0.5, 0.1), (0.8, 0.1)),
                    screen.output((0.5, 0.5), (0.4, 0.4)),
                ],
            )],
            900,
        )
    };
    let lit = scene("Lit", vec![rig.effects.solid(RED)]);
    let dark = scene("Dark", Vec::new());

    let running = rig.run_scene(lit).await;
    rig.writes
        .wait_for(screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    let seen = rig.writes.of(screen.device_id).len();
    rig.activate(&dark).await;
    rig.writes
        .wait_for(screen.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    running.stop().await;

    let fade = rig.writes.of(screen.device_id);
    let centers = fade
        .iter()
        .skip(seen)
        .map(|write| Panel(&write.data).center())
        .collect::<Vec<_>>();
    assert!(
        centers
            .iter()
            .any(|[r, g, b]| (40..=215).contains(r) && *g <= 3 && *b <= 3),
        "the screen cut to black without fading: {centers:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_fades_out_when_the_whole_scene_goes_dark() {
    screen_fades_out_when_the_whole_scene_goes_dark(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_fades_out_when_the_whole_scene_goes_dark() {
    screen_fades_out_when_the_whole_scene_goes_dark(RenderAccelerationMode::Gpu).await;
}

/// A scene that does not change sends a screen its backdrop once. Finalizing
/// and reading back a still picture at the screen's cadence would cost a
/// pass per screen per tick for nothing.
async fn still_scene_sends_its_screen_one_frame(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("still").await;
    let zones = vec![scene_zone(
        "Still",
        ZoneRole::Primary,
        vec![fill_layer(RED)],
        vec![
            strip_output("strip:still", (0.5, 0.1), (0.8, 0.1)),
            screen.output((0.5, 0.5), (0.4, 0.4)),
        ],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    let settled = rig.writes.of(screen.device_id).len();
    tokio::time::sleep(Duration::from_millis(900)).await;
    running.stop().await;

    let resent = rig.writes.of(screen.device_id).len() - settled;
    assert!(
        resent <= 1,
        "a still scene sent its screen {resent} more frames in under a second"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_still_scene_sends_its_screen_one_frame() {
    still_scene_sends_its_screen_one_frame(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_still_scene_sends_its_screen_one_frame() {
    still_scene_sends_its_screen_one_frame(RenderAccelerationMode::Gpu).await;
}

/// A screen that connects while nothing renders is driven to black like the
/// screens that were there when the loop went idle. The idle loop composes
/// nothing, so a new screen has to bring it back for one frame.
async fn screen_connected_while_idle_gets_its_backdrop(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let early = rig.add_screen("early").await;
    let late = Screen::named("late");
    let zones = vec![scene_zone(
        "Idle",
        ZoneRole::Primary,
        Vec::new(),
        vec![
            early.output((0.3, 0.5), (0.3, 0.3)),
            late.output((0.7, 0.5), (0.3, 0.3)),
        ],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(early.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    // Let the loop settle into idling before the second panel appears.
    tokio::time::sleep(Duration::from_millis(400)).await;
    rig.connect_screen(&late, DisplayFrameFormat::Rgb, 30).await;
    rig.writes
        .wait_for(late.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_connected_while_idle_gets_its_backdrop() {
    screen_connected_while_idle_gets_its_backdrop(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_connected_while_idle_gets_its_backdrop() {
    screen_connected_while_idle_gets_its_backdrop(RenderAccelerationMode::Gpu).await;
}

/// Output stopped for the session takes a placed screen dark with the LEDs.
/// The frame that clears the output composes no scene, so it has to carry
/// the screens' black itself.
async fn stopped_output_takes_a_placed_screen_dark(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("stopped").await;
    let zones = vec![scene_zone(
        "Red",
        ZoneRole::Primary,
        vec![rig.effects.solid(RED)],
        vec![
            strip_output("strip:red", (0.5, 0.1), (0.8, 0.1)),
            screen.output((0.5, 0.5), (0.4, 0.4)),
        ],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.power.send_modify(|power| {
        power.output_override = hypercolor_daemon::output_power::OutputOverride::Stopped;
    });
    rig.writes
        .wait_for(screen.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_stopped_output_takes_a_placed_screen_dark() {
    stopped_output_takes_a_placed_screen_dark(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_stopped_output_takes_a_placed_screen_dark() {
    stopped_output_takes_a_placed_screen_dark(RenderAccelerationMode::Gpu).await;
}

// ── Transitions: what a screen shows while one runs ──────────────────────

/// A scene whose one zone fills with `color` and places `screen` in the
/// middle, with a half-strength green face over it when `faced`.
fn lit_scene(
    effects: &Effects,
    screen: &Screen,
    name: &str,
    color: [f32; 4],
    transition_ms: u64,
    faced: bool,
) -> Scene {
    let mut zones = vec![scene_zone(
        "Lit",
        ZoneRole::Primary,
        vec![effects.solid(color)],
        vec![
            strip_output("strip:lit", (0.5, 0.1), (0.8, 0.1)),
            screen.output((0.5, 0.5), (0.4, 0.4)),
        ],
    )];
    if faced {
        zones.push(blending_face_zone(screen, vec![effects.solid(GREEN)]));
    }
    test_scene(name, zones, transition_ms)
}

/// Every frame `device_id` has been sent since `seen`, as panel centers.
fn centers_since(writes: &PanelWrites, device_id: DeviceId, seen: usize) -> Vec<[u8; 3]> {
    writes
        .of(device_id)
        .iter()
        .skip(seen)
        .map(|write| Panel(&write.data).center())
        .collect()
}

/// Part way from red to blue: a real mix, neither end.
fn between_red_and_blue(center: [u8; 3]) -> bool {
    let [r, g, b] = center;
    (70..=185).contains(&r) && (70..=185).contains(&b) && g <= 6
}

/// Stopping output part way through a transition takes the screen black and
/// leaves it black. Nothing of the fade outlives the frame that stops it.
async fn stopping_output_mid_transition_ends_on_black(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("stopped-mid-fade").await;
    let red = lit_scene(&rig.effects, &screen, "Red", RED, 0, false);
    let blue = lit_scene(&rig.effects, &screen, "Blue", BLUE, 4000, false);

    let running = rig.run_scene(red).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.activate(&blue).await;
    rig.writes
        .wait_for(screen.device_id, "a mix of red and blue", |panel| {
            between_red_and_blue(panel.center())
        })
        .await;
    rig.power.send_modify(|power| {
        power.output_override = hypercolor_daemon::output_power::OutputOverride::Stopped;
    });
    rig.writes
        .wait_for(screen.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    let dark_from = rig.writes.of(screen.device_id).len() - 1;
    tokio::time::sleep(Duration::from_millis(500)).await;
    running.stop().await;

    let after = centers_since(&rig.writes, screen.device_id, dark_from);
    assert!(
        after.iter().all(|center| near(*center, [0, 0, 0])),
        "a stopped screen lit up again: {after:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_stopping_output_mid_transition_ends_on_black() {
    stopping_output_mid_transition_ends_on_black(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_stopping_output_mid_transition_ends_on_black() {
    stopping_output_mid_transition_ends_on_black(RenderAccelerationMode::Gpu).await;
}

/// A transition cut short by a scene that takes over at once never shows
/// the picture it was heading for. Red was fading to blue; green replaces
/// both, and no frame is ever all blue.
async fn cancelled_transition_never_shows_its_destination(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("cancelled").await;
    let red = lit_scene(&rig.effects, &screen, "Red", RED, 0, false);
    let blue = lit_scene(&rig.effects, &screen, "Blue", BLUE, 4000, false);
    let green = lit_scene(&rig.effects, &screen, "Green", GREEN, 0, false);

    let running = rig.run_scene(red).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    for round in 0..6 {
        let seen = rig.writes.of(screen.device_id).len();
        rig.activate(&blue).await;
        rig.writes
            .wait_for(screen.device_id, "a mix of red and blue", |panel| {
                between_red_and_blue(panel.center())
            })
            .await;
        rig.activate(&green).await;
        rig.writes
            .wait_for(screen.device_id, "green", |panel| {
                near(panel.center(), [0, 255, 0])
            })
            .await;
        tokio::time::sleep(Duration::from_millis(120)).await;
        let shown = centers_since(&rig.writes, screen.device_id, seen);
        assert!(
            shown.iter().all(|[r, _, b]| !(*b >= 215 && *r <= 40)),
            "round {round}: the cancelled blue scene was shown whole: {shown:?}"
        );
        // Back to red, at once, for the next round.
        let red_again = lit_scene(&rig.effects, &screen, "Red again", RED, 0, false);
        rig.activate(&red_again).await;
        rig.writes
            .wait_for(screen.device_id, "red", |panel| {
                near(panel.center(), [255, 0, 0])
            })
            .await;
    }
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_cancelled_transition_never_shows_its_destination() {
    cancelled_transition_never_shows_its_destination(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_cancelled_transition_never_shows_its_destination() {
    cancelled_transition_never_shows_its_destination(RenderAccelerationMode::Gpu).await;
}

/// A transition that interrupts another starts from the mix that was on the
/// panel. Red was half way to blue when green began to fade in: the screen
/// goes from purple to green, and never back through red or on to blue.
async fn interrupting_transition_starts_from_the_mix(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("interrupted").await;
    let red = lit_scene(&rig.effects, &screen, "Red", RED, 0, false);
    let blue = lit_scene(&rig.effects, &screen, "Blue", BLUE, 4000, false);
    let green = lit_scene(&rig.effects, &screen, "Green", GREEN, 1500, false);

    let running = rig.run_scene(red).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.activate(&blue).await;
    let mix = rig
        .writes
        .wait_for(screen.device_id, "a mix of red and blue", |panel| {
            between_red_and_blue(panel.center())
        })
        .await;
    let [mix_r, _, mix_b] = Panel(&mix.data).center();
    let seen = rig.writes.of(screen.device_id).len();
    rig.activate(&green).await;
    rig.writes
        .wait_for(screen.device_id, "green", |panel| {
            near(panel.center(), [0, 255, 0])
        })
        .await;
    running.stop().await;

    let shown = centers_since(&rig.writes, screen.device_id, seen);
    // The blue fade may land a frame or two more before green takes over,
    // so the mix can drift a little past where it was seen.
    let (most_red, most_blue) = (mix_r.saturating_add(12), mix_b.saturating_add(60));
    assert!(
        shown
            .iter()
            .all(|[r, _, b]| *r <= most_red && *b <= most_blue),
        "the second fade left the mix of {:?} it began on: {shown:?}",
        Panel(&mix.data).center()
    );
    assert!(
        shown
            .iter()
            .any(|[r, g, b]| *r >= 25 && *b >= 25 && (40..=215).contains(g)),
        "no frame mixed green into the mix it began on: {shown:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_interrupting_transition_starts_from_the_mix() {
    interrupting_transition_starts_from_the_mix(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_interrupting_transition_starts_from_the_mix() {
    interrupting_transition_starts_from_the_mix(RenderAccelerationMode::Gpu).await;
}

/// A blending face keeps its own pixels through a transition while the
/// backdrop under it moves. The face is green at half strength over red and
/// then over blue, neither of which has any green: the panel's green is the
/// face alone, and it does not move.
async fn blending_face_stays_sharp_through_a_transition(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("faced-fade").await;
    let red = lit_scene(&rig.effects, &screen, "Red", RED, 0, true);
    let blue = lit_scene(&rig.effects, &screen, "Blue", BLUE, 1500, true);

    let running = rig.run_scene(red).await;
    let settled = rig
        .writes
        .wait_for(screen.device_id, "the face over red", |panel| {
            let [r, g, b] = panel.center();
            r >= 100 && g >= 100 && b <= 6
        })
        .await;
    let [_, face_green, _] = Panel(&settled.data).center();
    let seen = rig.writes.of(screen.device_id).len();
    rig.activate(&blue).await;
    rig.writes
        .wait_for(screen.device_id, "the face over blue", |panel| {
            let [r, g, b] = panel.center();
            r <= 6 && g >= 100 && b >= 100
        })
        .await;
    running.stop().await;

    let shown = centers_since(&rig.writes, screen.device_id, seen);
    assert!(
        shown.iter().all(|[_, g, _]| g.abs_diff(face_green) <= 6),
        "the face's green moved from {face_green} during the transition: {shown:?}"
    );
    assert!(
        shown.iter().any(|[r, _, b]| *r >= 25 && *b >= 25),
        "the backdrop under the face cut from red to blue: {shown:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_blending_face_stays_sharp_through_a_transition() {
    blending_face_stays_sharp_through_a_transition(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_blending_face_stays_sharp_through_a_transition() {
    blending_face_stays_sharp_through_a_transition(RenderAccelerationMode::Gpu).await;
}

/// A screen that gains a face with the scene, and one that loses it, still
/// fades its backdrop from the outgoing picture. The face itself arrives
/// and leaves at once; the red under it becomes blue gradually.
async fn screen_changing_its_face_still_fades_its_backdrop(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("face-change").await;
    let bare_red = lit_scene(&rig.effects, &screen, "Bare red", RED, 1500, false);
    let faced_blue = lit_scene(&rig.effects, &screen, "Faced blue", BLUE, 1500, true);

    let running = rig.run_scene(bare_red.clone()).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;

    // Gains a face: green appears over a backdrop still part red.
    let seen = rig.writes.of(screen.device_id).len();
    rig.activate(&faced_blue).await;
    rig.writes
        .wait_for(screen.device_id, "the face over blue", |panel| {
            let [r, g, b] = panel.center();
            r <= 6 && g >= 100 && b >= 100
        })
        .await;
    let gaining = centers_since(&rig.writes, screen.device_id, seen);
    assert!(
        gaining
            .iter()
            .any(|[r, g, b]| *g >= 100 && *r >= 20 && *b >= 20),
        "the backdrop cut to blue when the face arrived: {gaining:?}"
    );

    // Loses it: the face is gone and the backdrop is part blue, part red.
    let seen = rig.writes.of(screen.device_id).len();
    rig.activate(&bare_red).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    running.stop().await;
    let losing = centers_since(&rig.writes, screen.device_id, seen);
    assert!(
        losing.iter().any(|center| between_red_and_blue(*center)),
        "the backdrop cut to red when the face left: {losing:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_changing_its_face_still_fades_its_backdrop() {
    screen_changing_its_face_still_fades_its_backdrop(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_changing_its_face_still_fades_its_backdrop() {
    screen_changing_its_face_still_fades_its_backdrop(RenderAccelerationMode::Gpu).await;
}

/// A placed screen that connects while output is off goes dark like the
/// screens that were there when it went off: with output stopped, which
/// releases the devices, and with output paused, which holds a static
/// color.
async fn screen_connected_while_output_is_off_goes_dark(
    mode: RenderAccelerationMode,
    off: hypercolor_daemon::output_power::OutputOverride,
) {
    let mut rig = Rig::new(mode);
    let early = rig.add_screen("early").await;
    let late = Screen::named("late");
    let zones = vec![scene_zone(
        "Red",
        ZoneRole::Primary,
        vec![rig.effects.solid(RED)],
        vec![
            strip_output("strip:red", (0.5, 0.1), (0.8, 0.1)),
            early.output((0.3, 0.5), (0.3, 0.3)),
            late.output((0.7, 0.5), (0.3, 0.3)),
        ],
    )];

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(early.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.power.send_modify(|power| power.output_override = off);
    rig.writes
        .wait_for(early.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    // Let the loop settle into its off state before the panel appears.
    tokio::time::sleep(Duration::from_millis(400)).await;
    rig.connect_screen(&late, DisplayFrameFormat::Rgb, 30).await;
    rig.writes
        .wait_for(late.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_connected_while_output_is_stopped_goes_dark() {
    screen_connected_while_output_is_off_goes_dark(
        RenderAccelerationMode::Cpu,
        hypercolor_daemon::output_power::OutputOverride::Stopped,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_connected_while_output_is_paused_goes_dark() {
    screen_connected_while_output_is_off_goes_dark(
        RenderAccelerationMode::Cpu,
        hypercolor_daemon::output_power::OutputOverride::Paused,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_connected_while_output_is_stopped_goes_dark() {
    screen_connected_while_output_is_off_goes_dark(
        RenderAccelerationMode::Gpu,
        hypercolor_daemon::output_power::OutputOverride::Stopped,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_connected_while_output_is_paused_goes_dark() {
    screen_connected_while_output_is_off_goes_dark(
        RenderAccelerationMode::Gpu,
        hypercolor_daemon::output_power::OutputOverride::Paused,
    )
    .await;
}

/// A simulated panel is nobody's output until somebody previews it, so a
/// rig with nothing else to drive parks its render loop. The preview is
/// then the only demand there is: the loop has to wake for it, place the
/// simulator, and give it the lane display output is waiting for.
async fn previewed_simulator_wakes_a_parked_render_loop(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let simulator = Screen::named("simulator");
    rig.connect_simulator(&simulator).await;
    let zones = vec![scene_zone(
        "Red",
        ZoneRole::Primary,
        vec![rig.effects.solid(RED)],
        vec![
            strip_output("strip:red", (0.5, 0.1), (0.8, 0.1)),
            simulator.output((0.5, 0.5), (0.4, 0.4)),
        ],
    )];

    let running = rig.run(zones).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        rig.writes.of(simulator.device_id).is_empty(),
        "nobody previews the simulator yet"
    );
    let _preview = rig
        .display_frames
        .write()
        .await
        .subscribe(simulator.device_id);
    rig.writes
        .wait_for(simulator.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_previewed_simulator_wakes_a_parked_render_loop() {
    previewed_simulator_wakes_a_parked_render_loop(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_previewed_simulator_wakes_a_parked_render_loop() {
    previewed_simulator_wakes_a_parked_render_loop(RenderAccelerationMode::Gpu).await;
}

/// A previewed simulator holds render demand for the lane that is coming.
/// When no zone places it and no face is shown on it, no lane is coming:
/// once the render thread has looked and said so, nothing is left asking
/// for frames, and a rig with nothing else to drive parks again. A scene
/// that places the simulator later is a new question, and its lane arrives.
async fn previewed_simulator_no_zone_places_lets_the_loop_park(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let simulator = Screen::named("unplaced-simulator");
    rig.connect_simulator(&simulator).await;
    let unplaced = test_scene(
        "Unplaced",
        vec![scene_zone(
            "Red",
            ZoneRole::Primary,
            vec![rig.effects.solid(RED)],
            vec![strip_output("strip:red", (0.5, 0.1), (0.8, 0.1))],
        )],
        0,
    );
    let placed = test_scene(
        "Placed",
        vec![scene_zone(
            "Blue",
            ZoneRole::Primary,
            vec![rig.effects.solid(BLUE)],
            vec![
                strip_output("strip:blue", (0.5, 0.1), (0.8, 0.1)),
                simulator.output((0.5, 0.5), (0.4, 0.4)),
            ],
        )],
        0,
    );
    let bus = Arc::clone(&rig.state.event_bus);
    let frame_number = || bus.frame_lane().borrow().frame_number;

    let running = rig.run_scene(unplaced).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _preview = rig
        .display_frames
        .write()
        .await
        .subscribe(simulator.device_id);
    // Display output has the simulator on its list of screens, and asks.
    let deadline = Instant::now() + WAIT;
    while !bus
        .screen_backdrop_routes_snapshot()
        .1
        .contains_key(&simulator.device_id)
    {
        assert!(
            Instant::now() < deadline,
            "the previewed simulator never became a screen display output drives"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The answer is no lane, so the hold goes and the loop parks.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let before = frame_number();
        tokio::time::sleep(Duration::from_millis(400)).await;
        if bus.output_demand().hold_count() == 0 && frame_number() == before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a simulator no zone places kept the loop rendering: {} demand holds, frame {} then {}",
            bus.output_demand().hold_count(),
            before,
            frame_number()
        );
    }
    assert!(rig.writes.of(simulator.device_id).is_empty());

    rig.activate(&placed).await;
    rig.writes
        .wait_for(simulator.device_id, "its blue zone", |panel| {
            near(panel.center(), [0, 0, 255])
        })
        .await;
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_previewed_simulator_no_zone_places_lets_the_loop_park() {
    previewed_simulator_no_zone_places_lets_the_loop_park(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_previewed_simulator_no_zone_places_lets_the_loop_park() {
    previewed_simulator_no_zone_places_lets_the_loop_park(RenderAccelerationMode::Gpu).await;
}

/// A simulator no zone places still gets a lane when a face is shown on
/// it. The render thread's answer names it for that reason, so in a rig
/// with nothing else to render for, its preview keeps asking until the
/// face has arrived.
async fn previewed_simulator_with_only_a_face_gets_its_lane(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let simulator = Screen::named("faced-simulator");
    rig.connect_simulator(&simulator).await;
    let zones = vec![
        scene_zone(
            "Red",
            ZoneRole::Primary,
            vec![rig.effects.solid(RED)],
            vec![strip_output("strip:red", (0.5, 0.1), (0.8, 0.1))],
        ),
        blending_face_zone(&simulator, vec![rig.effects.solid(GREEN)]),
    ];

    let running = rig.run(zones).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _preview = rig
        .display_frames
        .write()
        .await
        .subscribe(simulator.device_id);
    rig.writes
        .wait_for(simulator.device_id, "its green face", |panel| {
            let [r, g, b] = panel.center();
            r <= 12 && g >= 60 && b <= 12
        })
        .await;
    let (_, plan) = rig.state.event_bus.screen_lane_plan_snapshot();
    running.stop().await;

    assert!(
        plan.is_some_and(|plan| plan.screens.contains(&simulator.device_id)),
        "a screen a live face is shown on has a lane coming"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_previewed_simulator_with_only_a_face_gets_its_lane() {
    previewed_simulator_with_only_a_face_gets_its_lane(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_previewed_simulator_with_only_a_face_gets_its_lane() {
    previewed_simulator_with_only_a_face_gets_its_lane(RenderAccelerationMode::Gpu).await;
}

/// Output that goes off takes every fade with it. Red was fading to blue
/// when output went off; that fade ran out unseen, and green began to fade
/// in while the screen was still dark. When output comes back the screen
/// fades in from the dark it was showing, with none of the red and blue it
/// showed before.
async fn transition_begun_while_output_was_off_fades_in_from_dark(
    mode: RenderAccelerationMode,
    off: hypercolor_daemon::output_power::OutputOverride,
) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("slept").await;
    let red = lit_scene(&rig.effects, &screen, "Red", RED, 0, false);
    let blue = lit_scene(&rig.effects, &screen, "Blue", BLUE, 800, false);
    let green = lit_scene(&rig.effects, &screen, "Green", GREEN, 4000, false);

    let running = rig.run_scene(red).await;
    rig.writes
        .wait_for(screen.device_id, "red", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    rig.activate(&blue).await;
    rig.writes
        .wait_for(screen.device_id, "a mix of red and blue", |panel| {
            between_red_and_blue(panel.center())
        })
        .await;
    rig.power.send_modify(|power| power.output_override = off);
    rig.writes
        .wait_for(screen.device_id, "black", |panel| {
            near(panel.center(), [0, 0, 0])
        })
        .await;
    // The fade to blue runs out while output is off.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    rig.activate(&green).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let seen = rig.writes.of(screen.device_id).len();
    rig.power.send_modify(|power| {
        power.output_override = hypercolor_daemon::output_power::OutputOverride::None;
    });
    rig.writes
        .wait_for(screen.device_id, "green", |panel| {
            near(panel.center(), [0, 255, 0])
        })
        .await;
    running.stop().await;

    let shown = centers_since(&rig.writes, screen.device_id, seen);
    assert!(
        shown.iter().all(|[r, _, b]| *r <= 12 && *b <= 12),
        "the screen came back with what it showed before output went off: {shown:?}"
    );
    assert!(
        shown.iter().any(|[_, g, _]| (20..=215).contains(g)),
        "the screen cut to green instead of fading in from dark: {shown:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_transition_begun_while_output_was_paused_fades_in_from_dark() {
    transition_begun_while_output_was_off_fades_in_from_dark(
        RenderAccelerationMode::Cpu,
        hypercolor_daemon::output_power::OutputOverride::Paused,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_transition_begun_while_output_was_stopped_fades_in_from_dark() {
    transition_begun_while_output_was_off_fades_in_from_dark(
        RenderAccelerationMode::Cpu,
        hypercolor_daemon::output_power::OutputOverride::Stopped,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_transition_begun_while_output_was_paused_fades_in_from_dark() {
    transition_begun_while_output_was_off_fades_in_from_dark(
        RenderAccelerationMode::Gpu,
        hypercolor_daemon::output_power::OutputOverride::Paused,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_transition_begun_while_output_was_stopped_fades_in_from_dark() {
    transition_begun_while_output_was_off_fades_in_from_dark(
        RenderAccelerationMode::Gpu,
        hypercolor_daemon::output_power::OutputOverride::Stopped,
    )
    .await;
}

// ── Viewports: the owning zone against the spatial service ────────────────

/// The layout ids a physical device goes by, the way the display output
/// thread reads them from the logical-device store.
fn layout_ids(
    logical_devices: &HashMap<String, LogicalDevice>,
    device_id: DeviceId,
) -> Vec<String> {
    let mut ids = logical_devices
        .iter()
        .filter(|(_, entry)| entry.physical_device_id == device_id)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

/// A layout device and the physical device it maps to.
fn logical(layout_id: &str, device_id: DeviceId) -> (String, LogicalDevice) {
    (
        layout_id.to_owned(),
        LogicalDevice {
            id: layout_id.to_owned(),
            physical_device_id: device_id,
            name: layout_id.to_owned(),
            led_start: 0,
            led_count: 0,
            enabled: true,
            kind: LogicalDeviceKind::Default,
        },
    )
}

/// One screen of a fixture layout.
struct FixtureScreen {
    device_id: DeviceId,
    /// Whether the device also drives LEDs.
    has_led_segments: bool,
    /// The viewport display output took from the spatial service's layout
    /// for this screen while every screen cropped one scene canvas. It is
    /// written out as numbers, not computed, so the resolver under test is
    /// never checked against itself.
    expected: Option<DisplayZoneViewport>,
}

/// A frozen viewport: position, size, rotation, scale and edge behavior.
fn frozen(
    position: (f32, f32),
    size: (f32, f32),
    rotation: f32,
    scale: f32,
    edge_behavior: EdgeBehavior,
) -> DisplayZoneViewport {
    DisplayZoneViewport {
        position: NormalizedPosition::new(position.0, position.1),
        size: NormalizedPosition::new(size.0, size.1),
        rotation,
        scale,
        edge_behavior,
    }
}

/// A layout with the screens it should be asked about, placed or not.
struct LayoutFixture {
    name: &'static str,
    layout: SpatialLayout,
    logical_devices: HashMap<String, LogicalDevice>,
    screens: Vec<FixtureScreen>,
}

fn boxed(mut output: Output, rotation: f32, scale: f32, edge: Option<EdgeBehavior>) -> Output {
    output.rotation = rotation;
    output.scale = scale;
    output.edge_behavior = edge;
    output
}

/// Layouts covering every way a layout places a screen, and the ways it
/// does not.
fn layout_fixtures() -> Vec<LayoutFixture> {
    let screen = |layout_id: &str| Screen {
        device_id: DeviceId::new(),
        layout_id: layout_id.to_owned(),
    };
    let mut fixtures = Vec::new();

    // A screen with its own box, among LED outputs that overlap it.
    let pump = screen("pump");
    fixtures.push(LayoutFixture {
        name: "a screen box among overlapping LED boxes",
        layout: zone_layout(vec![
            strip_output("ring", (0.75, 0.6), (0.2, 0.2)),
            pump.output((0.755, 0.6), (0.115, 0.117)),
            strip_output("strip", (0.5, 0.5), (1.0, 0.1)),
        ]),
        logical_devices: HashMap::from([logical("pump", pump.device_id)]),
        screens: vec![FixtureScreen {
            device_id: pump.device_id,
            has_led_segments: false,
            expected: Some(frozen(
                (0.755, 0.6),
                (0.115, 0.117),
                0.0,
                1.0,
                EdgeBehavior::Clamp,
            )),
        }],
    });

    // Three screens, turned, scaled, and with their own edge behavior.
    let (one, two, three) = (screen("lcd-1"), screen("lcd-2"), screen("lcd-3"));
    fixtures.push(LayoutFixture {
        name: "turned and scaled screen boxes",
        layout: zone_layout(vec![
            boxed(
                one.output((0.2, 0.25), (0.125, 0.127)),
                std::f32::consts::FRAC_PI_2,
                1.0,
                None,
            ),
            boxed(
                two.output((0.2, 0.5), (0.125, 0.127)),
                0.3,
                1.5,
                Some(EdgeBehavior::Mirror),
            ),
            boxed(
                three.output((0.2, 0.75), (0.125, 0.127)),
                -0.7,
                0.5,
                Some(EdgeBehavior::FadeToBlack { falloff: 0.25 }),
            ),
        ]),
        logical_devices: HashMap::from([
            logical("lcd-1", one.device_id),
            logical("lcd-2", two.device_id),
            logical("lcd-3", three.device_id),
        ]),
        screens: [
            (
                &one,
                Some(frozen(
                    (0.2, 0.25),
                    (0.125, 0.127),
                    std::f32::consts::FRAC_PI_2,
                    1.0,
                    EdgeBehavior::Clamp,
                )),
            ),
            (
                &two,
                Some(frozen(
                    (0.2, 0.5),
                    (0.125, 0.127),
                    0.3,
                    1.5,
                    EdgeBehavior::Mirror,
                )),
            ),
            (
                &three,
                Some(frozen(
                    (0.2, 0.75),
                    (0.125, 0.127),
                    -0.7,
                    0.5,
                    EdgeBehavior::FadeToBlack { falloff: 0.25 },
                )),
            ),
        ]
        .into_iter()
        .map(|(screen, expected)| FixtureScreen {
            device_id: screen.device_id,
            has_led_segments: false,
            expected,
        })
        .collect(),
    });

    // A device laid out as one whole-device box, with no screen box.
    let whole = DeviceId::new();
    let mut whole_box = strip_output("whole", (0.4, 0.4), (0.3, 0.2));
    "whole-device".clone_into(&mut whole_box.device_id);
    fixtures.push(LayoutFixture {
        name: "a whole-device box",
        layout: zone_layout(vec![whole_box]),
        logical_devices: HashMap::from([logical("whole-device", whole)]),
        screens: vec![FixtureScreen {
            device_id: whole,
            has_led_segments: false,
            expected: Some(frozen(
                (0.4, 0.4),
                (0.3, 0.2),
                0.0,
                1.0,
                EdgeBehavior::Clamp,
            )),
        }],
    });

    // A device whose LED segments are laid out and whose screen is not: it
    // shows the whole canvas when it has LEDs, and its segment's box when
    // the laid-out segment is all it has.
    let mixed = DeviceId::new();
    let mut ring = strip_output("mixed-ring", (0.3, 0.7), (0.2, 0.2));
    "mixed".clone_into(&mut ring.device_id);
    ring.zone_name = Some("Ring".to_owned());
    fixtures.push(LayoutFixture {
        name: "LED segments without a screen box",
        layout: zone_layout(vec![ring]),
        logical_devices: HashMap::from([logical("mixed", mixed)]),
        screens: vec![
            FixtureScreen {
                device_id: mixed,
                has_led_segments: true,
                expected: Some(frozen(
                    (0.5, 0.5),
                    (1.0, 1.0),
                    0.0,
                    1.0,
                    EdgeBehavior::Clamp,
                )),
            },
            FixtureScreen {
                device_id: mixed,
                has_led_segments: false,
                expected: Some(frozen(
                    (0.3, 0.7),
                    (0.2, 0.2),
                    0.0,
                    1.0,
                    EdgeBehavior::Clamp,
                )),
            },
        ],
    });

    // A screen the layout does not hold, and a layout with nothing in it.
    let absent = DeviceId::new();
    fixtures.push(LayoutFixture {
        name: "a screen no layout holds",
        layout: zone_layout(vec![strip_output("strip", (0.5, 0.5), (1.0, 0.1))]),
        logical_devices: HashMap::from([logical("absent", absent)]),
        screens: vec![FixtureScreen {
            device_id: absent,
            has_led_segments: false,
            expected: None,
        }],
    });
    fixtures.push(LayoutFixture {
        name: "an empty layout",
        layout: zone_layout(Vec::new()),
        logical_devices: HashMap::new(),
        screens: vec![FixtureScreen {
            device_id: DeviceId::new(),
            has_led_segments: false,
            expected: None,
        }],
    });

    fixtures
}

/// The zones of a scene that claims `claimed` output ids for a custom zone
/// and leaves the rest of `layout` to the primary zone, each output keeping
/// the placement the layout gives it.
fn split_zones(layout: &SpatialLayout, claimed: &[String]) -> Vec<Zone> {
    let (custom, primary): (Vec<_>, Vec<_>) = layout
        .zones
        .iter()
        .cloned()
        .partition(|output| claimed.contains(&output.id));
    vec![
        scene_zone("Primary", ZoneRole::Primary, Vec::new(), primary),
        scene_zone("Custom", ZoneRole::Custom, Vec::new(), custom),
    ]
}

/// For every fixture layout, the viewport taken from the zone that places a
/// screen is the one display output took from the spatial service's layout
/// before: in a scene whose one zone follows the layout, and in scenes that
/// split the layout's outputs between two zones. Both are held to the
/// frozen numbers, so a mistake the two share cannot pass.
#[test]
fn owning_zone_viewport_equals_the_spatial_service_viewport_for_every_fixture_layout() {
    let mut placed = 0;
    for fixture in layout_fixtures() {
        let spatial = SpatialService::new(SpatialEngine::new(fixture.layout.clone()));
        // A scene with no custom zone: its primary zone follows the layout.
        let lone = SceneManager::with_default_layout(spatial.layout().as_ref().clone());
        let lone_zones = lone.resolved_zones();
        // Scenes that hand every other output, and then every output, to a
        // custom zone.
        let every_other = fixture
            .layout
            .zones
            .iter()
            .step_by(2)
            .map(|output| output.id.clone())
            .collect::<Vec<_>>();
        let every = fixture
            .layout
            .zones
            .iter()
            .map(|output| output.id.clone())
            .collect::<Vec<_>>();
        let scenes = [
            ("one zone", lone_zones.to_vec()),
            ("two zones", split_zones(&fixture.layout, &every_other)),
            ("custom zone only", split_zones(&fixture.layout, &every)),
        ];

        for screen in &fixture.screens {
            let layout_ids = layout_ids(&fixture.logical_devices, screen.device_id);
            assert_eq!(
                screen_viewport_in_layout(
                    spatial.layout().as_ref(),
                    &layout_ids,
                    screen.has_led_segments,
                ),
                screen.expected,
                "{}: the spatial service's layout gives another viewport",
                fixture.name
            );
            placed += usize::from(screen.expected.is_some());
            for (scene, zones) in &scenes {
                let placement = screen_placement(zones, &layout_ids, screen.has_led_segments);
                assert_eq!(
                    placement.map(|placement| placement.viewport),
                    screen.expected,
                    "{} ({scene}): the owning zone gives another viewport",
                    fixture.name
                );
                if let Some(placement) = placement {
                    let owner = zones
                        .iter()
                        .find(|zone| zone.id == placement.zone_id)
                        .expect("the placing zone is one of the scene's zones");
                    assert_eq!(
                        screen_viewport_in_layout(
                            &owner.layout,
                            &layout_ids,
                            screen.has_led_segments
                        ),
                        screen.expected,
                        "{} ({scene}): the placing zone should hold the screen",
                        fixture.name
                    );
                }
            }
        }
    }
    assert_eq!(placed, 7, "the fixtures place seven screens");
}

/// The two differ in exactly one situation: a zone moves a screen on its own
/// canvas. The spatial service still holds the placement the zone started
/// from, and the zone's own placement is the right one, because the zone's
/// frame is drawn in the zone's layout.
#[test]
fn a_zone_that_moves_its_screen_overrides_the_spatial_service_viewport() {
    let pump = Screen {
        device_id: DeviceId::new(),
        layout_id: "pump".to_owned(),
    };
    let logical_devices = HashMap::from([logical("pump", pump.device_id)]);
    let layout = zone_layout(vec![
        strip_output("strip", (0.5, 0.5), (1.0, 0.1)),
        pump.output((0.755, 0.6), (0.115, 0.117)),
    ]);
    let mut manager = SceneManager::with_default();
    let mut scene = make_scene("Moved Screen");
    scene.zones = split_zones(&layout, &["strip".to_owned()]);
    let scene_id = scene.id;
    let primary_id = scene.zones[0].id;
    manager.create(scene).expect("create scene");
    manager.activate(&scene_id, None).expect("activate scene");

    let mut moved = manager.resolved_zones()[0].layout.clone();
    moved.zones[0].position = NormalizedPosition::new(0.7567, 0.597);
    manager
        .update_zone_layout(&scene_id, primary_id, moved)
        .expect("a zone may move the outputs it holds");
    // The scene has a custom zone, so a layout publication leaves the
    // primary zone's own placements alone.
    manager.sync_primary_zone_layout(&layout);

    let from_spatial = screen_viewport_in_layout(
        &layout,
        &layout_ids(&logical_devices, pump.device_id),
        false,
    )
    .expect("the layout places the screen");
    let placement = screen_placement(
        &manager.resolved_zones(),
        &layout_ids(&logical_devices, pump.device_id),
        false,
    )
    .expect("the primary zone places the screen");

    assert_eq!(placement.zone_id, primary_id);
    assert_eq!(from_spatial.position, NormalizedPosition::new(0.755, 0.6));
    assert_eq!(
        placement.viewport.position,
        NormalizedPosition::new(0.7567, 0.597),
        "the screen shows the box its zone gives it"
    );
    assert_eq!(placement.viewport.size, from_spatial.size);
}

/// A screen's box in one zone places it there even when another zone holds
/// the same device's LEDs, and an earlier zone's LED claim does not win.
#[test]
fn the_zone_holding_a_screens_box_places_it_over_a_zone_holding_its_leds() {
    let push = Screen {
        device_id: DeviceId::new(),
        layout_id: "push".to_owned(),
    };
    let logical_devices = HashMap::from([logical("push", push.device_id)]);
    let mut pads = strip_output("push-pads", (0.5, 0.7), (0.4, 0.4));
    "push".clone_into(&mut pads.device_id);
    pads.zone_name = Some("Pads".to_owned());
    let pads_zone = scene_zone("Pads", ZoneRole::Custom, Vec::new(), vec![pads]);
    let screen_zone = scene_zone(
        "Default",
        ZoneRole::Primary,
        Vec::new(),
        vec![push.output((0.54, 0.31), (0.39, 0.11))],
    );
    let screen_zone_id = screen_zone.id;

    for zones in [
        vec![pads_zone.clone(), screen_zone.clone()],
        vec![screen_zone, pads_zone],
    ] {
        let placement =
            screen_placement(&zones, &layout_ids(&logical_devices, push.device_id), true)
                .expect("a zone places the screen");
        assert_eq!(placement.zone_id, screen_zone_id);
        assert_eq!(
            placement.viewport.position,
            NormalizedPosition::new(0.54, 0.31)
        );
    }
}

// ── Screen layers ────────────────────────────────────────────────────────

/// The screens a rig knows, as the daemon reads them from its registry.
fn known_screens(screens: &[(&Screen, (u32, u32))]) -> ScreenSurfaces {
    screens
        .iter()
        .map(|(screen, size)| {
            (
                screen.layout_id.clone(),
                ScreenSurface {
                    device_id: screen.device_id,
                    connected: true,
                    name: screen.layout_id.clone(),
                    width: size.0,
                    height: size.1,
                },
            )
        })
        .collect()
}

/// What a panel shows once its picture has stopped changing.
async fn settled(writes: &PanelWrites, device_id: DeviceId) -> Vec<u8> {
    const STILL: Duration = Duration::from_millis(400);
    let started = Instant::now();
    let mut held: Option<(Vec<u8>, Instant)> = None;
    loop {
        if let Some(write) = writes.of(device_id).pop() {
            match &held {
                Some((frame, since)) if *frame == write.data => {
                    if since.elapsed() >= STILL {
                        return write.data;
                    }
                }
                _ => held = Some((write.data, Instant::now())),
            }
        }
        assert!(
            started.elapsed() < WAIT,
            "the panel never settled on one picture"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn pixel_at(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let offset = usize::try_from((y * width + x) * 3).expect("panel offset fits usize");
    [frame[offset], frame[offset + 1], frame[offset + 2]]
}

/// Two frames of one panel are the same picture, pixel for pixel.
fn assert_same_frame(what: &str, before: &[u8], after: &[u8]) {
    assert_eq!(before.len(), after.len(), "{what}: the panel changed size");
    let differing = before
        .as_chunks::<3>()
        .0
        .iter()
        .zip(after.as_chunks::<3>().0)
        .filter(|(before, after)| before != after)
        .count();
    let widest = before
        .iter()
        .zip(after)
        .map(|(before, after)| before.abs_diff(*after))
        .max()
        .unwrap_or(0);
    assert_eq!(
        differing, 0,
        "{what}: {differing} pixels differ, by up to {widest} per channel"
    );
}

fn with_composition(mut layer: SceneLayer, blend: BlendMode, opacity: f32) -> SceneLayer {
    layer.blend = blend;
    layer.opacity = opacity;
    layer
}

/// A scene whose one screen sits over the boundary of two backdrop columns,
/// with a stored display zone for it holding `stack` under `target`.
///
/// A placed screen has its own box in the zone. An unplaced one has none:
/// the zone holds only its device's LED ring, which is how the render
/// thread still finds it a backdrop.
fn stored_face_scene(
    effects: &Effects,
    screen: &Screen,
    stack: Vec<SceneLayer>,
    target: (BlendMode, f32),
    placed: bool,
) -> Scene {
    let on_device = if placed {
        screen.output((0.5, 0.5), (0.4, 0.4))
    } else {
        let mut ring = strip_output("pump-ring", (0.5, 0.5), (0.4, 0.4));
        ring.device_id.clone_from(&screen.layout_id);
        ring.zone_name = Some("Ring".to_owned());
        ring
    };
    let owner = scene_zone(
        "Main",
        ZoneRole::Primary,
        vec![effects.columns()],
        vec![
            strip_output("strip:main", (0.5, 0.1), (0.8, 0.1)),
            on_device,
        ],
    );
    let mut zones = vec![owner];
    if !stack.is_empty() {
        let mut face = blending_face_zone(screen, stack);
        face.display_target = Some(DisplayFaceTarget {
            device_id: screen.device_id,
            blend_mode: target.0,
            opacity: target.1,
        });
        zones.push(face);
    }
    test_scene("Stored Face", zones, 0)
}

/// Run a stored face on a fresh rig and return what its panel settles on,
/// what conversion did, and the scene as the manager holds it afterwards.
async fn stored_face_frame(
    mode: RenderAccelerationMode,
    screen: &Screen,
    stack: &dyn Fn(&Effects) -> Vec<SceneLayer>,
    target: (BlendMode, f32),
    placed: bool,
    convert: bool,
) -> (Vec<u8>, ScreenConversion, Scene) {
    let mut rig = Rig::new(mode);
    rig.connect_screen(screen, DisplayFrameFormat::Rgb, 30)
        .await;
    rig.surfaces = known_screens(&[(screen, (PANEL, PANEL))]);
    let scene = stored_face_scene(&rig.effects, screen, stack(&rig.effects), target, placed);
    let scene_id = scene.id;
    let (running, conversion) = rig.run_scene_with(scene, convert).await;
    let frame = settled(&rig.writes, screen.device_id).await;
    running.stop().await;
    let held = rig
        .state
        .scene_manager
        .snapshot()
        .await
        .get(&scene_id)
        .cloned()
        .expect("the scene stays registered");
    (frame, conversion, held)
}

/// A stored display zone shows the same frames after conversion as before:
/// converted when the result is exact, left as stored for `stays` otherwise.
async fn stored_face_keeps_its_frames(
    mode: RenderAccelerationMode,
    stack: &dyn Fn(&Effects) -> Vec<SceneLayer>,
    target: (BlendMode, f32),
    placed: bool,
    stays: Option<LegacyReason>,
) {
    let screen = Screen::named("pump");
    let (backdrop, _, _) =
        stored_face_frame(mode, &screen, &|_| Vec::new(), target, placed, false).await;
    let (before, _, stored) = stored_face_frame(mode, &screen, stack, target, placed, false).await;
    let (after, conversion, held) =
        stored_face_frame(mode, &screen, stack, target, placed, true).await;

    assert!(
        backdrop.iter().any(|channel| *channel > 8),
        "the screen has a lit backdrop for the face to blend over"
    );

    assert!(
        stored.display_zone_for(screen.device_id).is_some(),
        "the first run renders the display zone as it was stored"
    );
    assert_ne!(before, backdrop, "the stored face reaches the panel");
    if let Some(reason) = stays {
        assert!(!conversion.changed());
        assert_eq!(
            conversion
                .legacy
                .iter()
                .map(|(_, reason)| *reason)
                .collect::<Vec<_>>(),
            vec![reason]
        );
        assert!(held.display_zone_for(screen.device_id).is_some());
        let outputs = |scene: &Scene| {
            scene
                .zones
                .iter()
                .map(|zone| {
                    zone.layout
                        .zones
                        .iter()
                        .map(|output| output.id.clone())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            outputs(&held),
            outputs(&stored),
            "no zone gains an output on the screen's behalf"
        );
    } else {
        assert_eq!(conversion.converted.len(), 1);
        assert!(conversion.legacy.is_empty());
        assert!(
            held.display_zone_for(screen.device_id).is_none(),
            "the display zone became screen layers"
        );
        let owner = held
            .primary_zone()
            .expect("the zone that places the screen");
        assert!(
            owner
                .layers
                .iter()
                .filter(|layer| layer.scope.is_some())
                .count()
                >= 1
        );
    }
    assert_same_frame("before and after conversion", &before, &after);
}

fn three_layer_stack(effects: &Effects) -> Vec<SceneLayer> {
    vec![
        effects.solid(RED),
        with_composition(fill_layer(BLUE), BlendMode::Alpha, 0.5),
        with_composition(fill_layer(WHITE), BlendMode::Add, 0.25),
    ]
}

fn two_layer_stack(effects: &Effects) -> Vec<SceneLayer> {
    vec![
        effects.solid(RED),
        with_composition(fill_layer(GREEN), BlendMode::Alpha, 0.5),
    ]
}

fn translucent_first_layer_stack(effects: &Effects) -> Vec<SceneLayer> {
    vec![
        with_composition(effects.solid(RED), BlendMode::Replace, 0.5),
        with_composition(fill_layer(GREEN), BlendMode::Alpha, 0.3),
    ]
}

fn disabled_first_layer_stack(effects: &Effects) -> Vec<SceneLayer> {
    let mut first = effects.solid(RED);
    first.enabled = false;
    vec![
        first,
        with_composition(fill_layer(GREEN), BlendMode::Alpha, 0.6),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_three_layer_face_shows_the_same_frames_once_converted() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Cpu,
        &three_layer_stack,
        (BlendMode::Alpha, 1.0),
        true,
        None,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_three_layer_face_shows_the_same_frames_once_converted() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Gpu,
        &three_layer_stack,
        (BlendMode::Alpha, 1.0),
        true,
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_translucent_target_over_a_stack_shows_the_same_frames_once_converted() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Cpu,
        &two_layer_stack,
        (BlendMode::Alpha, 0.5),
        true,
        None,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_translucent_target_over_a_stack_shows_the_same_frames_once_converted() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Gpu,
        &two_layer_stack,
        (BlendMode::Alpha, 0.5),
        true,
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_screen_blended_target_shows_the_same_frames_once_converted() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Cpu,
        &two_layer_stack,
        (BlendMode::Screen, 0.8),
        true,
        None,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_screen_blended_target_shows_the_same_frames_once_converted() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Gpu,
        &two_layer_stack,
        (BlendMode::Screen, 0.8),
        true,
        None,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_translucent_first_layer_stays_stored_and_shows_the_same_frames() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Cpu,
        &translucent_first_layer_stack,
        (BlendMode::Alpha, 0.7),
        true,
        Some(LegacyReason::FirstLayerOpacity),
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_translucent_first_layer_stays_stored_and_shows_the_same_frames() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Gpu,
        &translucent_first_layer_stack,
        (BlendMode::Alpha, 0.7),
        true,
        Some(LegacyReason::FirstLayerOpacity),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_disabled_first_layer_stays_stored_and_shows_the_same_frames() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Cpu,
        &disabled_first_layer_stack,
        (BlendMode::Alpha, 0.7),
        true,
        Some(LegacyReason::FirstLayerDisabled),
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_disabled_first_layer_stays_stored_and_shows_the_same_frames() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Gpu,
        &disabled_first_layer_stack,
        (BlendMode::Alpha, 0.7),
        true,
        Some(LegacyReason::FirstLayerDisabled),
    )
    .await;
}

/// A screen no zone places has no output to scope a layer to, and its
/// backdrop comes from wherever the render thread finds its device. A box
/// of the scene manager's choosing would move that backdrop under a
/// translucent face, so the stored zone is left alone and the panel shows
/// the same frames.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_unplaced_screen_face_stays_stored_and_shows_the_same_frames() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Cpu,
        &two_layer_stack,
        (BlendMode::Alpha, 0.5),
        false,
        Some(LegacyReason::ScreenUnplaced),
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_unplaced_screen_face_stays_stored_and_shows_the_same_frames() {
    stored_face_keeps_its_frames(
        RenderAccelerationMode::Gpu,
        &two_layer_stack,
        (BlendMode::Alpha, 0.5),
        false,
        Some(LegacyReason::ScreenUnplaced),
    )
    .await;
}

/// One layer scoped to two screens of different sizes is rendered once per
/// panel at that panel's size: each shows all four columns across its own
/// width.
async fn one_layer_on_two_screens_renders_at_each_panels_size(mode: RenderAccelerationMode) {
    const SMALL: (u32, u32) = (16, 16);
    const WIDE: (u32, u32) = (32, 8);
    let mut rig = Rig::new(mode);
    let small = Screen::named("small");
    let wide = Screen::named("wide");
    rig.connect_sized(&small, SMALL).await;
    rig.connect_sized(&wide, WIDE).await;
    rig.surfaces = known_screens(&[(&small, SMALL), (&wide, WIDE)]);
    let small_box = small.output((0.25, 0.5), (0.2, 0.2));
    let wide_box = wide.output((0.75, 0.5), (0.2, 0.2));
    let mut face = rig.effects.columns();
    face.scope = Some(LayerScope {
        outputs: vec![small_box.id.clone(), wide_box.id.clone()],
    });
    let zone = scene_zone(
        "Main",
        ZoneRole::Primary,
        vec![rig.effects.solid(BLUE), face],
        vec![
            strip_output("strip:main", (0.5, 0.1), (0.8, 0.1)),
            small_box,
            wide_box,
        ],
    );

    let running = rig.run(vec![zone]).await;
    let small_frame = settled(&rig.writes, small.device_id).await;
    let wide_frame = settled(&rig.writes, wide.device_id).await;
    running.stop().await;

    for (name, frame, size) in [("small", &small_frame, SMALL), ("wide", &wide_frame, WIDE)] {
        assert_eq!(
            frame.len(),
            usize::try_from(size.0 * size.1 * 3).expect("frame size fits usize"),
            "the {name} panel is sent frames of its own size"
        );
        let row = size.1 / 2;
        for (column, expected) in COLUMNS.iter().enumerate() {
            let column = u32::try_from(column).expect("four columns");
            let x = size.0 * (2 * column + 1) / 8;
            assert!(
                near(pixel_at(frame, size.0, x, row), *expected),
                "the {name} panel shows {:?} in column {column}, not {expected:?}",
                pixel_at(frame, size.0, x, row)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_one_layer_on_two_screens_renders_at_each_panels_size() {
    one_layer_on_two_screens_renders_at_each_panels_size(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_one_layer_on_two_screens_renders_at_each_panels_size() {
    one_layer_on_two_screens_renders_at_each_panels_size(RenderAccelerationMode::Gpu).await;
}

/// The effects a captured scene names are not installed on a test rig, so
/// each effect layer is pointed at a still built-in picture, the same one
/// on every run.
fn make_renderable(scene: &mut Scene, effects: &Effects) {
    let pictures = [
        effects.columns(),
        effects.solid(RED),
        effects.solid(GREEN),
        effects.solid(WHITE),
    ];
    let mut next = 0;
    for layer in scene
        .zones
        .iter_mut()
        .flat_map(|zone| zone.layers.iter_mut())
    {
        if matches!(layer.source, LayerSource::Effect { .. }) {
            layer.source = pictures[next % pictures.len()].source.clone();
            next += 1;
        }
    }
}

const STORED_PANELS: [(&str, (u32, u32)); 3] = [
    ("usb:pump-lcd", (480, 480)),
    ("usb:fan-lcd", (400, 400)),
    ("usb:push2", (960, 160)),
];

/// The 0.9.0 scene store, as a release build wrote it, renders the same
/// frames on every screen once its display zones are converted: one face, a
/// three-layer stack under a screen-blended target, and a bare surface whose
/// screen shows its kept face.
async fn stored_scene_frames(
    mode: RenderAccelerationMode,
    convert: bool,
) -> (Vec<Vec<u8>>, ScreenConversion, Scene) {
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/storage_v0_9_0");
    let data = tempfile::tempdir().expect("tempdir should be created");
    let scenes_path = data.path().join("scenes.json");
    std::fs::copy(fixtures.join("scenes.json"), &scenes_path).expect("copy the scene store");
    let store = hypercolor_daemon::scene_store::load(&scenes_path).expect("the 0.9.0 store loads");
    let mut scene = store
        .list()
        .find(|scene| scene.name == "Studio Session")
        .cloned()
        .expect("the captured scene");
    let kept: HashMap<DeviceId, serde_json::Value> = serde_json::from_str(
        &std::fs::read_to_string(fixtures.join("display-preferences.json"))
            .expect("read the kept faces"),
    )
    .expect("the kept faces parse");

    let mut rig = Rig::new(mode);
    make_renderable(&mut scene, &rig.effects);
    let screens = STORED_PANELS
        .iter()
        .map(|(layout_id, size)| {
            let output_id = format!("{layout_id}-display");
            let device_id = scene
                .zones
                .iter()
                .filter_map(|zone| zone.display_target.as_ref().map(|target| (zone, target)))
                .find(|(zone, _)| (zone.layout.canvas_width, zone.layout.canvas_height) == *size)
                .map(|(_, target)| target.device_id)
                .expect("each panel has a stored display zone of its size");
            assert!(
                scene.zones.iter().any(|zone| zone
                    .layout
                    .zones
                    .iter()
                    .any(|output| output.id == output_id)),
                "the scene places {layout_id}"
            );
            (
                Screen {
                    device_id,
                    layout_id: (*layout_id).to_owned(),
                },
                *size,
            )
        })
        .collect::<Vec<_>>();
    for (screen, size) in &screens {
        rig.connect_sized(screen, *size).await;
    }
    rig.surfaces = known_screens(
        &screens
            .iter()
            .map(|(screen, size)| (screen, *size))
            .collect::<Vec<_>>(),
    );
    for device_id in kept.keys() {
        let (screen, size) = screens
            .iter()
            .find(|(screen, _)| screen.device_id == *device_id)
            .expect("the kept face belongs to a stored screen");
        let mut face = blending_face_zone(screen, vec![rig.effects.solid(BLUE)]);
        face.layout.canvas_width = size.0;
        face.layout.canvas_height = size.1;
        face.display_target = Some(DisplayFaceTarget::new(*device_id));
        rig.kept_faces.push(face);
    }
    scene.transition.duration_ms = 0;
    let scene_id = scene.id;

    let (running, conversion) = rig.run_scene_with(scene, convert).await;
    let mut frames = Vec::new();
    for (screen, _) in &screens {
        frames.push(settled(&rig.writes, screen.device_id).await);
    }
    running.stop().await;
    let held = rig
        .state
        .scene_manager
        .snapshot()
        .await
        .get(&scene_id)
        .cloned()
        .expect("the scene stays registered");
    (frames, conversion, held)
}

async fn stored_scene_shows_the_same_frames_once_converted(mode: RenderAccelerationMode) {
    let (before, _, stored) = stored_scene_frames(mode, false).await;
    let (after, conversion, held) = stored_scene_frames(mode, true).await;

    assert_eq!(
        stored
            .zones
            .iter()
            .filter(|zone| zone.role == ZoneRole::Display)
            .count(),
        3,
        "the store holds one display zone per screen"
    );
    assert_eq!(
        conversion.converted.len(),
        2,
        "the face and the three-layer stack become screen layers"
    );
    assert_eq!(conversion.removed.len(), 1, "the bare surface is removed");
    assert_eq!(
        conversion.legacy,
        Vec::new(),
        "no zone of the captured store is left in the legacy spelling"
    );
    assert!(
        held.zones.iter().all(|zone| zone.role != ZoneRole::Display),
        "no display zone is left in the scene"
    );
    for (((layout_id, _), before), after) in STORED_PANELS.iter().zip(&before).zip(&after) {
        assert!(
            before.iter().any(|channel| *channel > 8),
            "{layout_id} shows a picture"
        );
        assert_same_frame(layout_id, before, after);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_stored_scene_shows_the_same_frames_once_converted() {
    stored_scene_shows_the_same_frames_once_converted(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_stored_scene_shows_the_same_frames_once_converted() {
    stored_scene_shows_the_same_frames_once_converted(RenderAccelerationMode::Gpu).await;
}

// ── A screen that leaves its zone and returns ────────────────────────────

/// The box a moving screen's device holds on the canvas whether or not its
/// screen is placed: its LED ring, over the whole canvas. The render thread
/// finds an unplaced screen its backdrop through this output, and the tests
/// give the screen's own output the same box, so both placements show one
/// backdrop and any difference between two frames is the face.
const DEVICE_BOX: ((f32, f32), (f32, f32)) = ((0.5, 0.5), (1.0, 1.0));

fn device_ring(screen: &Screen) -> Output {
    let mut ring = strip_output("pump-ring", DEVICE_BOX.0, DEVICE_BOX.1);
    ring.device_id.clone_from(&screen.layout_id);
    ring.zone_name = Some("Ring".to_owned());
    ring
}

/// A layout over the rig's canvas, with the device's ring and, when
/// `placed`, its screen.
fn moving_layout(screen: &Screen, placed: bool) -> SpatialLayout {
    let mut outputs = vec![
        strip_output("strip:main", (0.5, 0.1), (0.8, 0.1)),
        device_ring(screen),
    ];
    if placed {
        outputs.push(screen.output(DEVICE_BOX.0, DEVICE_BOX.1));
    }
    zone_layout(outputs)
}

/// A translucent two-layer face as screen layers of a zone that places the
/// screen.
fn placed_face_scene(effects: &Effects, screen: &Screen, name: &str) -> Scene {
    let output_id = screen.output(DEVICE_BOX.0, DEVICE_BOX.1).id;
    let mut layers = vec![effects.columns()];
    for mut layer in [
        with_composition(effects.solid(RED), BlendMode::Screen, 0.6),
        with_composition(fill_layer(GREEN), BlendMode::Alpha, 0.5),
    ] {
        layer.scope = Some(LayerScope {
            outputs: vec![output_id.clone()],
        });
        layers.push(layer);
    }
    let mut zone = scene_zone("Main", ZoneRole::Primary, layers, Vec::new());
    zone.layout = moving_layout(screen, true);
    test_scene(name, vec![zone], 0)
}

impl Rig {
    /// Rewrite the live scene's primary zone from `layout`, as a layout
    /// publication does, and commit it.
    async fn publish_layout(&self, layout: &SpatialLayout) {
        let mut mutation = self.state.scene_manager.begin_mutation().await;
        mutation.sync_primary_layout(layout);
        self.state
            .scene_manager
            .commit_mutation(mutation)
            .await
            .expect("the layout should commit");
    }

    /// Activate `scene` and align its primary zone with `layout`, as
    /// activation does for a scene that follows a layout.
    async fn activate_on(&self, scene: &Scene, layout: &SpatialLayout) {
        let mut mutation = self.state.scene_manager.begin_mutation().await;
        let _ = mutation.create_scene(scene.clone());
        mutation
            .activate(
                scene.id,
                None,
                hypercolor_types::event::SceneChangeReason::UserActivate,
            )
            .expect("scene should activate");
        mutation.realign_primary_layout(layout);
        self.state
            .scene_manager
            .commit_mutation(mutation)
            .await
            .expect("scene activation should commit");
    }

    async fn stored(&self, scene: &Scene) -> Scene {
        self.state
            .scene_manager
            .snapshot()
            .await
            .get(&scene.id)
            .cloned()
            .expect("the scene stays registered")
    }
}

/// Every frame the panel was sent from `seen` on is `expected`.
fn assert_frames_held(writes: &PanelWrites, device_id: DeviceId, seen: usize, expected: &[u8]) {
    for (index, write) in writes.of(device_id).iter().enumerate().skip(seen) {
        assert_same_frame(&format!("frame {index}"), expected, &write.data);
    }
}

/// A face on a placed screen is screen layers; the same face once the
/// screen's output leaves the zone is a stored display zone. The two are
/// rendered by different paths and must put the same bytes on the panel:
/// when the layout under a running scene loses the screen and gets it
/// back, the panel never shows anything else.
async fn face_survives_its_screen_leaving_the_layout_and_returning(mode: RenderAccelerationMode) {
    let screen = Screen::named("pump");
    let mut rig = Rig::new(mode);
    rig.connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    rig.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let scene = placed_face_scene(&rig.effects, &screen, "Moving");
    let running = rig.run_scene(scene.clone()).await;
    let placed = settled(&rig.writes, screen.device_id).await;

    let mut backdrop_rig = Rig::new(mode);
    backdrop_rig
        .connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    backdrop_rig.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let mut bare = placed_face_scene(&backdrop_rig.effects, &screen, "Bare");
    bare.zones[0].layers.truncate(1);
    let backdrop_run = backdrop_rig.run_scene(bare).await;
    let backdrop = settled(&backdrop_rig.writes, screen.device_id).await;
    backdrop_run.stop().await;
    assert_ne!(placed, backdrop, "the face reaches the panel");

    for (placed_now, what) in [(false, "left"), (true, "returned"), (false, "left again")] {
        let seen = rig.writes.of(screen.device_id).len();
        rig.publish_layout(&moving_layout(&screen, placed_now))
            .await;
        tokio::time::sleep(Duration::from_millis(400)).await;

        let held = rig.stored(&scene).await;
        assert_eq!(
            held.display_zone_for(screen.device_id).is_some(),
            !placed_now,
            "once the screen {what}, its stack follows its placement"
        );
        assert_eq!(
            held.zones[0]
                .layers
                .iter()
                .filter(|layer| layer.scope.is_some())
                .count(),
            if placed_now { 2 } else { 0 },
            "no layer scoped to the screen is left behind in the zone"
        );
        assert_frames_held(&rig.writes, screen.device_id, seen, &placed);
        let now = settled(&rig.writes, screen.device_id).await;
        assert_same_frame(&format!("after the screen {what}"), &placed, &now);
    }
    running.stop().await;

    // The stored spelling, run on its own from a cold start, is the same
    // picture: the comparison above was not one frame held over.
    let unplaced_doc = rig.stored(&scene).await;
    let mut cold = Rig::new(mode);
    cold.connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    cold.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let mut cold_scene = unplaced_doc;
    cold_scene.id = hypercolor_types::scene::SceneId::new();
    let cold_run = cold.run_scene(cold_scene).await;
    let from_stored = settled(&cold.writes, screen.device_id).await;
    cold_run.stop().await;
    assert_same_frame("the stored zone on a cold start", &placed, &from_stored);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_face_survives_its_screen_leaving_the_layout_and_returning() {
    face_survives_its_screen_leaving_the_layout_and_returning(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_face_survives_its_screen_leaving_the_layout_and_returning() {
    face_survives_its_screen_leaving_the_layout_and_returning(RenderAccelerationMode::Gpu).await;
}

/// The shape of a rig whose stored layouts hold no screens: screens are
/// placed only inside a scene's primary zone. Activating a scene that
/// follows such a layout, and coming back, realigns each primary zone to a
/// layout without the screen. The faces stay on the panel, and activating
/// a scene twice changes nothing.
async fn faces_survive_activating_scenes_that_follow_a_layout_without_screens(
    mode: RenderAccelerationMode,
) {
    let screen = Screen::named("pump");
    let mut rig = Rig::new(mode);
    rig.connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    rig.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let layout = moving_layout(&screen, false);
    // Home places the screen in its own primary zone and holds its face
    // as screen layers. Play follows the layout and holds a stored face.
    let home = placed_face_scene(&rig.effects, &screen, "Home");
    let mut play_zone = scene_zone(
        "Main",
        ZoneRole::Primary,
        vec![rig.effects.columns()],
        Vec::new(),
    );
    play_zone.layout = moving_layout(&screen, true);
    let mut play_face = blending_face_zone(
        &screen,
        vec![
            rig.effects.solid(BLUE),
            with_composition(fill_layer(WHITE), BlendMode::Alpha, 0.3),
        ],
    );
    play_face.display_target = Some(DisplayFaceTarget {
        device_id: screen.device_id,
        blend_mode: BlendMode::Alpha,
        opacity: 0.7,
    });
    let play = test_scene("Play", vec![play_zone, play_face], 0);

    let running = rig.run_scene(home.clone()).await;
    let home_frame = settled(&rig.writes, screen.device_id).await;

    rig.activate_on(&play, &layout).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let play_frame = settled(&rig.writes, screen.device_id).await;
    assert_ne!(play_frame, home_frame, "Play shows its own face");
    let play_stored = rig.stored(&play).await;
    assert!(
        play_stored.display_zone_for(screen.device_id).is_some(),
        "the layout Play follows holds no screen, so its face stays stored"
    );
    let seen = rig.writes.of(screen.device_id).len();
    rig.activate_on(&play, &layout).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        rig.stored(&play).await.zones,
        play_stored.zones,
        "activating it again changes nothing"
    );
    assert_frames_held(&rig.writes, screen.device_id, seen, &play_frame);

    rig.activate_on(&home, &layout).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let back = settled(&rig.writes, screen.device_id).await;
    let home_stored = rig.stored(&home).await;
    assert!(
        home_stored.display_zone_for(screen.device_id).is_some(),
        "coming back realigned Home to the layout, and its face went with its screen"
    );
    assert_same_frame("Home after coming back", &home_frame, &back);
    running.stop().await;

    // Play's stored face on a cold start is the picture Play showed.
    let mut cold = Rig::new(mode);
    cold.connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    cold.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let mut cold_scene = play_stored;
    cold_scene.id = hypercolor_types::scene::SceneId::new();
    let cold_run = cold.run_scene(cold_scene).await;
    let from_stored = settled(&cold.writes, screen.device_id).await;
    cold_run.stop().await;
    assert_same_frame("Play on a cold start", &play_frame, &from_stored);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_faces_survive_activating_scenes_that_follow_a_layout_without_screens() {
    faces_survive_activating_scenes_that_follow_a_layout_without_screens(
        RenderAccelerationMode::Cpu,
    )
    .await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_faces_survive_activating_scenes_that_follow_a_layout_without_screens() {
    faces_survive_activating_scenes_that_follow_a_layout_without_screens(
        RenderAccelerationMode::Gpu,
    )
    .await;
}

// ── Published layouts ────────────────────────────────────────────────────

impl Rig {
    /// Publish `layout` through the layout transaction's own write to the
    /// scene manager, the path a layout apply takes.
    async fn publish(&self, layout: SpatialLayout) {
        assert!(
            self.state
                .scene_manager
                .publish_layout_for_test(&self.state.spatial_engine, layout)
                .await,
            "the layout should publish"
        );
    }
}

/// The moving screen's layout with its screen held under another output
/// id, as a second stored layout names the same panel.
fn renamed_layout(screen: &Screen, output_id: &str) -> SpatialLayout {
    let mut layout = moving_layout(screen, true);
    for output in &mut layout.zones {
        if output.zone_name.as_deref() == Some(DISPLAY_ZONE_NAME) {
            output_id.clone_into(&mut output.id);
        }
    }
    layout
}

/// A translucent face holds the panel byte for byte through published
/// layouts that name its screen differently, drop it, and bring it back.
async fn face_holds_through_published_layouts(mode: RenderAccelerationMode) {
    let screen = Screen::named("pump");
    let mut rig = Rig::new(mode);
    rig.connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    rig.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let scene = placed_face_scene(&rig.effects, &screen, "Published");
    let authored_id = screen.output(DEVICE_BOX.0, DEVICE_BOX.1).id;
    let running = rig.run_scene(scene.clone()).await;
    let placed = settled(&rig.writes, screen.device_id).await;

    for (layout, held_as) in [
        (renamed_layout(&screen, "lcd-other"), Some("lcd-other")),
        (moving_layout(&screen, false), None),
        (renamed_layout(&screen, "lcd-third"), Some("lcd-third")),
        (moving_layout(&screen, true), Some(authored_id.as_str())),
    ] {
        let seen = rig.writes.of(screen.device_id).len();
        rig.publish(layout).await;
        tokio::time::sleep(Duration::from_millis(400)).await;

        let held = rig.stored(&scene).await;
        let scopes = held.zones[0]
            .layers
            .iter()
            .filter_map(|layer| layer.scope.as_ref())
            .map(|scope| scope.outputs.join(","))
            .collect::<Vec<_>>();
        if let Some(output_id) = held_as {
            assert!(held.display_zone_for(screen.device_id).is_none());
            assert_eq!(
                scopes,
                vec![output_id.to_owned(), output_id.to_owned()],
                "the face follows its screen to the id this layout gives it"
            );
        } else {
            assert!(held.display_zone_for(screen.device_id).is_some());
            assert!(scopes.is_empty(), "no layer is left behind in the zone");
        }
        assert_frames_held(&rig.writes, screen.device_id, seen, &placed);
        let now = settled(&rig.writes, screen.device_id).await;
        assert_same_frame("after the layout was published", &placed, &now);
    }
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_face_holds_through_published_layouts() {
    face_holds_through_published_layouts(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_face_holds_through_published_layouts() {
    face_holds_through_published_layouts(RenderAccelerationMode::Gpu).await;
}

/// A face that changes every frame keeps changing through the same
/// layouts: its renderer is never stopped, and the panel never falls back
/// to its bare backdrop.
async fn animated_face_keeps_running_through_published_layouts(mode: RenderAccelerationMode) {
    let screen = Screen::named("pump");
    let mut rig = Rig::new(mode);
    rig.connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    rig.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let mut scene = placed_face_scene(&rig.effects, &screen, "Animated");
    let output_id = screen.output(DEVICE_BOX.0, DEVICE_BOX.1).id;
    let mut face = rig.effects.moving();
    face.scope = Some(LayerScope {
        outputs: vec![output_id],
    });
    scene.zones[0].layers.truncate(1);
    scene.zones[0].layers.push(face);
    let mut bare = scene.clone();
    bare.id = hypercolor_types::scene::SceneId::new();
    bare.zones[0].layers.truncate(1);

    let mut backdrop_rig = Rig::new(mode);
    backdrop_rig
        .connect_screen(&screen, DisplayFrameFormat::Rgb, 30)
        .await;
    backdrop_rig.surfaces = known_screens(&[(&screen, (PANEL, PANEL))]);
    let backdrop_run = backdrop_rig.run_scene(bare).await;
    let backdrop = settled(&backdrop_rig.writes, screen.device_id).await;
    backdrop_run.stop().await;

    let running = rig.run_scene(scene.clone()).await;
    let moves = |from: usize| {
        let frames = rig.writes.of(screen.device_id);
        let mut distinct: Vec<&Vec<u8>> = Vec::new();
        for write in frames.iter().skip(from) {
            assert_ne!(write.data, backdrop, "the panel fell back to its backdrop");
            if !distinct.contains(&&write.data) {
                distinct.push(&write.data);
            }
        }
        distinct.len()
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(moves(0) >= 3, "the face animates before any layout changes");

    for layout in [
        renamed_layout(&screen, "lcd-other"),
        moving_layout(&screen, false),
        moving_layout(&screen, true),
    ] {
        let seen = rig.writes.of(screen.device_id).len();
        rig.publish(layout).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            moves(seen) >= 3,
            "the face keeps animating after the layout was published"
        );
    }
    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_animated_face_keeps_running_through_published_layouts() {
    animated_face_keeps_running_through_published_layouts(RenderAccelerationMode::Cpu).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_animated_face_keeps_running_through_published_layouts() {
    animated_face_keeps_running_through_published_layouts(RenderAccelerationMode::Gpu).await;
}
