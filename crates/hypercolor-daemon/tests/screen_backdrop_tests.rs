//! Each screen shows the backdrop of the zone that places it, seen through
//! that zone's placement of the screen (Spec 95 §5.10).
//!
//! These tests run the render thread and the display output thread together
//! and read what reaches each panel.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, watch};

use hypercolor_core::asset::AssetLibrary;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::device::{BackendManager, DeviceRegistry};
use hypercolor_core::effect::{EffectRegistry, builtin::register_builtin_effects};
use hypercolor_core::engine::{FpsTier, RenderLoop};
use hypercolor_core::input::InputManager;
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
use hypercolor_driver_api::{BackendInfo, DeviceBackend, DiscoveredDevice};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::control::ControlValue;
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceError,
    DeviceFamily, DeviceFeatures, DeviceId, DeviceInfo, DeviceOrigin, DeviceState,
    DeviceTopologyHint, DisplayFrameFormat, OwnedDisplayFramePayload, SegmentInfo,
};
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::{BlendMode, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{
    DisplayFaceTarget, Scene, UnassignedBehavior, Zone, ZoneId, ZoneRole,
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
    writes: PanelWrites,
}

#[async_trait]
impl DeviceBackend for PanelBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: BACKEND_ID.to_owned(),
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

fn panel_info(device_id: DeviceId, format: DisplayFrameFormat, max_fps: u32) -> DeviceInfo {
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
                width: PANEL,
                height: PANEL,
                circular: false,
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
            display_resolution: Some((PANEL, PANEL)),
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
    _power: watch::Sender<OutputPowerState>,
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
        backend_manager.register_backend(Arc::new(PanelBackend {
            writes: writes.clone(),
        }));
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
            _power: power,
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

    /// Connect a panel whose box a layout may already hold.
    async fn connect_screen(&self, screen: &Screen, format: DisplayFrameFormat, max_fps: u32) {
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
        self.state
            .backend_manager
            .lock()
            .await
            .connect_device(BACKEND_ID, device_id, &layout_id)
            .await
            .expect("panel should connect");
        let tracked = self
            .state
            .device_registry
            .add(panel_info(device_id, format, max_fps))
            .await;
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
        let mut manager = self.state.scene_manager.snapshot().await;
        manager.create(scene.clone()).expect("create scene");
        manager.activate(&scene.id, None).expect("activate scene");
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
            display_frames: Arc::new(RwLock::new(DisplayFrameRuntime::new())),
            face_fps_cap: self.state.face_fps_cap,
        });
        self.state.render_loop.write().await.start();
        let render = RenderThread::spawn(self.state.clone());
        RunningRig {
            render,
            display,
            render_loop: Arc::clone(&self.state.render_loop),
        }
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

/// One zone, no overlapping screens: each panel shows the crop of the scene
/// canvas its layout box names, which is what a panel showed when every
/// screen cropped the scene canvas. The scene canvas of a lone zone at the
/// scene extent is that zone's frame, so the crop is unchanged.
///
/// `with_face` adds a screen with a face. It is left out on a compositor
/// that does not finalize, which showed no routed face at all while every
/// screen cropped the scene canvas; the overlapping screens cover it there.
async fn lone_zone_screens_show_their_crop_of_the_scene_canvas(
    mode: RenderAccelerationMode,
    with_face: bool,
) {
    let mut rig = Rig::new(mode);
    let red_screen = rig.add_screen("column-one").await;
    let split_screen = rig.add_screen("columns-two-three").await;
    let face_screen = rig.add_screen("column-four").await;
    // Columns are a quarter of the canvas wide: red, green, blue, white.
    let layout = zone_layout(vec![
        strip_output("strip:all", (0.5, 0.05), (0.9, 0.05)),
        red_screen.output((0.125, 0.3), (0.15, 0.3)),
        split_screen.output((0.5, 0.6), (0.3, 0.3)),
        face_screen.output((0.875, 0.7), (0.15, 0.3)),
    ]);
    let mut zone = scene_zone(
        "Columns",
        ZoneRole::Primary,
        vec![rig.effects.columns()],
        Vec::new(),
    );
    zone.layout = layout.clone();
    let mut zones = vec![zone];
    if with_face {
        zones.push(blending_face_zone(
            &face_screen,
            vec![rig.effects.solid(GREEN)],
        ));
    }
    let mut canvas_rx = rig.state.event_bus.canvas_receiver();

    let running = rig.run(zones).await;
    rig.writes
        .wait_for(red_screen.device_id, "the red column", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    let split = rig
        .writes
        .wait_for(
            split_screen.device_id,
            "the green and blue columns",
            |panel| {
                near(panel.pixel(1, PANEL / 2), [0, 255, 0])
                    && near(panel.pixel(PANEL - 2, PANEL / 2), [0, 0, 255])
            },
        )
        .await;
    if with_face {
        rig.writes
            .wait_for(
                face_screen.device_id,
                "green over the white column",
                |panel| {
                    let [r, g, b] = panel.center();
                    g >= 250 && (100..=240).contains(&r) && r.abs_diff(b) <= 3
                },
            )
            .await;
    } else {
        rig.writes
            .wait_for(face_screen.device_id, "the white column", |panel| {
                near(panel.center(), [255, 255, 255])
            })
            .await;
    }
    tokio::time::timeout(WAIT, canvas_rx.wait_for(|frame| frame.width == CANVAS.0))
        .await
        .expect("the scene canvas should publish")
        .expect("canvas stream should stay open");
    let canvas = canvas_rx.borrow().clone();
    running.stop().await;

    // The crop every screen took before: the scene canvas through the
    // spatial layout's box for the screen, checked away from column edges.
    let logical_devices = rig.logical_devices.read().await;
    let viewport = screen_viewport_in_layout(
        &layout,
        &layout_ids(&logical_devices, split_screen.device_id),
        false,
    )
    .expect("the layout places the split screen");
    let panel = Panel(&split.data);
    for (x, y) in [
        (1, 2),
        (1, PANEL - 3),
        (PANEL - 2, 2),
        (PANEL - 2, PANEL - 3),
    ] {
        let local = |pixel: u32| (f32::from(u16::try_from(pixel).expect("small")) + 0.5) / 16.0;
        let canvas_x = viewport.position.x + (local(x) - 0.5) * viewport.size.x;
        let canvas_y = viewport.position.y + (local(y) - 0.5) * viewport.size.y;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions,
            reason = "normalized canvas positions inside the canvas map to pixel indices"
        )]
        let expected = canvas
            .surface()
            .get_pixel((canvas_x * 320.0) as u32, (canvas_y * 200.0) as u32);
        assert!(
            near(panel.pixel(x, y), [expected.r, expected.g, expected.b]),
            "panel pixel ({x}, {y}) was {:?}, the scene canvas crop is {expected:?}",
            panel.pixel(x, y)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cpu_lone_zone_screens_show_their_crop_of_the_scene_canvas() {
    lone_zone_screens_show_their_crop_of_the_scene_canvas(RenderAccelerationMode::Cpu, false).await;
}

#[cfg(feature = "wgpu")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gpu_lone_zone_screens_show_their_crop_of_the_scene_canvas() {
    lone_zone_screens_show_their_crop_of_the_scene_canvas(RenderAccelerationMode::Gpu, true).await;
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

/// Switching between two scenes that light a screen the same way never
/// shows the screen black. A screen's zone is found in the scene its frame
/// renders, so no frame is resolved against another scene's zones.
async fn scene_switch_never_shows_a_lit_screen_black(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let screen = rig.add_screen("lit").await;
    let scene = |name: &str| {
        test_scene(
            name,
            vec![scene_zone(
                "Red",
                ZoneRole::Primary,
                vec![rig.effects.solid(RED)],
                vec![
                    strip_output("strip:red", (0.5, 0.1), (0.8, 0.1)),
                    screen.output((0.5, 0.5), (0.4, 0.4)),
                ],
            )],
            0,
        )
    };
    let (first, second) = (scene("First"), scene("Second"));

    let running = rig.run_scene(first.clone()).await;
    rig.writes
        .wait_for(screen.device_id, "its red zone", |panel| {
            near(panel.center(), [255, 0, 0])
        })
        .await;
    let seen = rig.writes.of(screen.device_id).len();
    for round in 0..24 {
        rig.activate(if round % 2 == 0 { &second } else { &first })
            .await;
        tokio::time::sleep(Duration::from_millis(45)).await;
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    running.stop().await;

    for write in rig.writes.of(screen.device_id).iter().skip(seen) {
        assert!(
            near(Panel(&write.data).center(), [255, 0, 0]),
            "a scene switch showed the screen {:?}",
            Panel(&write.data).center()
        );
    }
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

/// Two screens whose boxes overlap in two zones keep showing their own zone
/// while one scene fades into another with the same arrangement. Each
/// screen's backdrop crossfades on its own, so the fade never passes through
/// a picture of the other zone.
async fn overlapping_screens_keep_their_zones_through_a_transition(mode: RenderAccelerationMode) {
    let mut rig = Rig::new(mode);
    let red_screen = rig.add_screen("red").await;
    let blue_screen = rig.add_screen("blue").await;
    let scene = |name: &str, transition_ms: u64| {
        test_scene(
            name,
            vec![
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
            ],
            transition_ms,
        )
    };
    let (first, second) = (scene("First", 0), scene("Second", 700));

    let running = rig.run_scene(first).await;
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
    let red_seen = rig.writes.of(red_screen.device_id).len();
    let blue_seen = rig.writes.of(blue_screen.device_id).len();
    rig.activate(&second).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    running.stop().await;

    for write in rig.writes.of(red_screen.device_id).iter().skip(red_seen) {
        assert!(
            near(Panel(&write.data).center(), [255, 0, 0]),
            "the red zone's panel showed {:?} during the transition",
            Panel(&write.data).center()
        );
    }
    for write in rig.writes.of(blue_screen.device_id).iter().skip(blue_seen) {
        assert!(
            near(Panel(&write.data).center(), [0, 0, 255]),
            "the blue zone's panel showed {:?} during the transition",
            Panel(&write.data).center()
        );
    }
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
        screens: [&one, &two, &three]
            .into_iter()
            .map(|screen| FixtureScreen {
                device_id: screen.device_id,
                has_led_segments: false,
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
            },
            FixtureScreen {
                device_id: mixed,
                has_led_segments: false,
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
        }],
    });
    fixtures.push(LayoutFixture {
        name: "an empty layout",
        layout: zone_layout(Vec::new()),
        logical_devices: HashMap::new(),
        screens: vec![FixtureScreen {
            device_id: DeviceId::new(),
            has_led_segments: false,
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
/// screen equals the one taken from the spatial service's layout: in a
/// scene whose one zone follows the layout, and in a scene that splits the
/// layout's outputs between two zones.
#[test]
fn owning_zone_viewport_equals_the_spatial_service_viewport_for_every_fixture_layout() {
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
            let from_spatial = screen_viewport_in_layout(
                spatial.layout().as_ref(),
                &layout_ids(&fixture.logical_devices, screen.device_id),
                screen.has_led_segments,
            );
            for (scene, zones) in &scenes {
                let placement = screen_placement(
                    zones,
                    &layout_ids(&fixture.logical_devices, screen.device_id),
                    screen.has_led_segments,
                );
                assert_eq!(
                    placement.map(|placement| placement.viewport),
                    from_spatial,
                    "{} ({scene}): the owning zone and the spatial service disagree",
                    fixture.name
                );
                if let Some(placement) = placement {
                    let owner = zones
                        .iter()
                        .find(|zone| zone.id == placement.zone_id)
                        .expect("the placing zone is one of the scene's zones");
                    assert!(
                        screen_viewport_in_layout(
                            &owner.layout,
                            &layout_ids(&fixture.logical_devices, screen.device_id),
                            screen.has_led_segments
                        )
                        .is_some(),
                        "{} ({scene}): the placing zone should hold the screen",
                        fixture.name
                    );
                }
            }
        }
    }
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
