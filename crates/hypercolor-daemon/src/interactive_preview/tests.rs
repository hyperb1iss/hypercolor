use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::{EffectEntry, EffectRegistry};
use hypercolor_core::input::routing::{ConsumerIncarnation, SourceIncarnation};
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputAttachment, BrowserInputChildKey, BrowserInputSource,
    BrowserPreviewId, InputManager, InputSource,
};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_types::config::InteractionRoutePolicy;
use hypercolor_types::effect::{
    ControlBinding, EffectCategory, EffectId, EffectMetadata, EffectSource, EffectState,
};
use hypercolor_types::layer::{
    BindingMap, BindingSource, LayerAdjust, LayerBinding, LayerBlendMode, LayerParameter,
    LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use hypercolor_types::viewport::ViewportRect;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{
    InteractivePreviewAcceleration, InteractivePreviewContext, InteractivePreviewExecutor,
    InteractivePreviewFrame, InteractivePreviewSpec, InteractivePreviewTarget,
    PreviewCapacityLedger, PreviewLaneCommand, PreviewLaneId, PreviewLaneInput,
    PreviewResourceLedger, ResolvedPreviewScene, advance_deadline, duration_millis_u32,
    preview_input_demand, request_preview_lane_update,
};
use crate::interaction_routing::InteractionRoutingControl;
use crate::preview_runtime::PreviewPixelFormat;
use crate::render_thread::{InputPublicationConsumer, InputPublicationDemandHandle};

struct PreviewTestRig {
    executor: InteractivePreviewExecutor,
    browser: BrowserInputSource,
    browser_handle: hypercolor_core::input::BrowserInputHandle,
    demands: InputPublicationDemandHandle,
}

impl PreviewTestRig {
    async fn new(color: [f32; 4]) -> Self {
        Self::with_capacity(color, 64 * 1024 * 1024).await
    }

    async fn with_capacity(color: [f32; 4], resource_capacity_bytes: u64) -> Self {
        let mut browser = BrowserInputSource::new();
        browser.start().expect("browser input should start");
        let browser_handle = browser.handle();
        let routing = InteractionRoutingControl::new(
            browser_handle.registry(),
            1,
            InteractionRoutePolicy::Host,
            InteractionRoutePolicy::Browser,
        );
        let demands = InputPublicationDemandHandle::new();
        let executor = InteractivePreviewExecutor::start_cpu(InteractivePreviewContext {
            scene_manager: Arc::new(RwLock::new(scene_manager(color))),
            effect_registry: Arc::new(RwLock::new(EffectRegistry::new(Vec::new()))),
            asset_library: None,
            event_bus: Arc::new(HypercolorBus::new()),
            input_graph: InputManager::new().input_graph_handle(),
            interaction_routing: routing,
            input_demands: demands.clone(),
            canvas_width: 8,
            canvas_height: 6,
            acceleration: InteractivePreviewAcceleration::cpu(),
            resource_capacity_bytes,
        })
        .await
        .expect("preview executor should start");
        Self {
            executor,
            browser,
            browser_handle,
            demands,
        }
    }

    fn attach(&self, connection: u64, preview_id: &str) -> BrowserInputAttachment {
        self.browser_handle
            .attach(BrowserInputChildKey::new(
                BrowserConnectionIncarnation::new(connection),
                BrowserPreviewId::new(preview_id),
            ))
            .expect("browser preview should attach")
    }
}

#[test]
fn preview_lane_routes_one_exact_browser_publication_with_coherent_generations() {
    let mut browser = BrowserInputSource::new();
    browser.start().expect("browser input should start");
    let handle = browser.handle();
    let attachment = handle
        .attach(BrowserInputChildKey::new(
            BrowserConnectionIncarnation::new(41),
            BrowserPreviewId::new("catalog-lane"),
        ))
        .expect("browser preview should attach");
    let manager = InputManager::new();
    let graph = manager.input_graph_handle();
    let routing = InteractionRoutingControl::new(
        handle.registry(),
        19,
        InteractionRoutePolicy::Host,
        InteractionRoutePolicy::Browser,
    );
    let consumer = ConsumerIncarnation::new(23);
    let graph_generation = graph.snapshot().generation();
    let browser_generation = routing.browser_registry_snapshot().generation();
    let mut input = PreviewLaneInput::new(graph, routing, attachment.publication_id(), consumer);

    input.read();

    assert_eq!(input.routed.diagnostics.consumer, consumer);
    assert_eq!(input.routed.diagnostics.config_generation, 19);
    assert_eq!(
        input.routed.diagnostics.source_graph_generation,
        graph_generation
    );
    assert_eq!(
        input.routed.diagnostics.browser_registry_generation,
        browser_generation
    );
    assert_eq!(input.routed.diagnostics.selected.len(), 1);
    assert_eq!(
        input.routed.diagnostics.selected[0].incarnation,
        SourceIncarnation::browser_child(attachment.publication_id().get())
    );
}

fn lane_resource_bytes(spec: InteractivePreviewSpec) -> u64 {
    PreviewResourceLedger::for_lane(spec, 8, 6, false, "preview".len())
        .expect("test resource geometry should fit")
        .total_bytes()
        .expect("test resource total should fit")
}

#[test]
fn resource_ledger_keeps_transport_payload_and_metadata_disjoint() {
    let ledger = PreviewResourceLedger::for_lane(spec(4, 3), 8, 6, false, "preview".len())
        .expect("test resource geometry should fit");

    assert_eq!(ledger.encoded_transport_bytes, 4 * 3 * 4);
    assert!(ledger.metadata_bytes > 0);
}

fn scene_manager(color: [f32; 4]) -> SceneManager {
    let mut scene = make_scene("Interactive Preview Test");
    scene.groups = vec![color_group(color)];
    scene.unassigned_behavior = UnassignedBehavior::Off;
    let scene_id = scene.id;
    let mut manager = SceneManager::new();
    manager.create(scene).expect("test scene should be valid");
    manager
        .activate(&scene_id, None)
        .expect("test scene should activate");
    manager
}

fn color_group(color: [f32; 4]) -> Zone {
    Zone {
        id: ZoneId::new(),
        name: "Preview".to_owned(),
        description: None,
        effect_id: None,
        controls: HashMap::new(),
        control_bindings: HashMap::new(),
        preset_id: None,
        layers: vec![SceneLayer {
            id: SceneLayerId::new(),
            name: None,
            source: LayerSource::ColorFill { rgba: color },
            blend: LayerBlendMode::Replace,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        }],
        layout: SpatialLayout {
            id: "preview-test".to_owned(),
            name: "Preview Test".to_owned(),
            description: None,
            canvas_width: 8,
            canvas_height: 6,
            zones: Vec::new(),
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        },
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        controls_version: 0,
        layers_version: 0,
    }
}

fn preview_effect_entry(effect_id: EffectId) -> EffectEntry {
    EffectEntry {
        metadata: EffectMetadata {
            id: effect_id,
            name: "Preview sensors".into(),
            author: "test".into(),
            version: "0.1.0".into(),
            description: "sensor demand fixture".into(),
            category: EffectCategory::Ambient,
            tags: Vec::new(),
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Native {
                path: "native/preview-sensors.wgsl".into(),
            },
            license: None,
        },
        source_path: "/effects/native/preview-sensors.wgsl".into(),
        modified: std::time::SystemTime::now(),
        state: EffectState::Loading,
    }
}

fn preview_effect_group(effect_id: EffectId) -> Zone {
    let mut group = color_group([0.0, 0.0, 0.0, 1.0]);
    group.effect_id = Some(effect_id);
    group
}

fn resolved_preview_scene(group: Zone, entry: EffectEntry) -> ResolvedPreviewScene {
    let mut registry = EffectRegistry::default();
    registry.register(entry);
    ResolvedPreviewScene {
        scene_id: None,
        groups_revision: 0,
        groups: Arc::from([group]),
        registry: Arc::new(registry),
        catalog_generation: 0,
        canvas_width: 8,
        canvas_height: 6,
    }
}

fn preview_control_binding() -> ControlBinding {
    ControlBinding {
        sensor: "cpu.temperature".into(),
        sensor_min: 20.0,
        sensor_max: 100.0,
        target_min: 0.0,
        target_max: 1.0,
        deadband: 0.0,
        smoothing: 0.0,
    }
}

fn spec(width: u32, height: u32) -> InteractivePreviewSpec {
    InteractivePreviewSpec {
        target: InteractivePreviewTarget::ActiveScene,
        fps: 60,
        width,
        height,
        format: PreviewPixelFormat::Rgba,
    }
}

async fn next_frame(
    receiver: &mut tokio::sync::watch::Receiver<Option<Arc<InteractivePreviewFrame>>>,
) -> Arc<InteractivePreviewFrame> {
    timeout(Duration::from_secs(2), receiver.changed())
        .await
        .expect("preview frame should arrive before timeout")
        .expect("preview lane should stay open");
    receiver
        .borrow_and_update()
        .clone()
        .expect("preview publication should carry a frame")
}

async fn wait_for_preview_demands(demands: &InputPublicationDemandHandle, expected: usize) {
    timeout(Duration::from_secs(2), async {
        while demands.registration_count(InputPublicationConsumer::Preview) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("preview demand count should converge");
}

#[test]
fn preview_spec_validates_rate_and_addressable_dimensions() {
    let valid = InteractivePreviewSpec {
        target: InteractivePreviewTarget::ActiveScene,
        fps: 60,
        width: 7_680,
        height: 4_320,
        format: PreviewPixelFormat::Rgba,
    };

    assert!(valid.validate().is_ok());
    assert!(
        InteractivePreviewSpec { fps: 0, ..valid }
            .validate()
            .is_err()
    );
    assert!(
        InteractivePreviewSpec { fps: 61, ..valid }
            .validate()
            .is_err()
    );
    assert!(
        InteractivePreviewSpec { width: 0, ..valid }
            .validate()
            .is_err()
    );
    assert!(
        InteractivePreviewSpec {
            width: 13,
            height: 17,
            ..valid
        }
        .validate()
        .is_ok()
    );
    assert!(
        InteractivePreviewSpec {
            width: u32::MAX,
            height: u32::MAX,
            ..valid
        }
        .validate()
        .is_err()
    );
}

#[test]
fn deadline_advance_skips_missed_intervals() {
    let start = Instant::now();
    let now = start + Duration::from_millis(35);
    let next = advance_deadline(start, Duration::from_millis(10), now);

    assert_eq!(next, start + Duration::from_millis(40));
}

#[test]
fn wire_timestamp_wraps_at_u32_boundary() {
    let duration = Duration::from_millis(u64::from(u32::MAX) + 3);

    assert_eq!(duration_millis_u32(duration), 2);
}

#[test]
fn screen_preview_demands_the_resolved_scene_extent() {
    let mut group = color_group([0.0, 0.0, 0.0, 1.0]);
    group.layers[0].source = LayerSource::ScreenRegion {
        viewport: ViewportRect::default(),
    };
    let scene = ResolvedPreviewScene {
        scene_id: None,
        groups_revision: 0,
        groups: Arc::from([group]),
        registry: Arc::new(EffectRegistry::new(Vec::new())),
        catalog_generation: 0,
        canvas_width: 5_120,
        canvas_height: 720,
    };

    let demand = preview_input_demand(&scene, 45);

    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            demand.requested_hz(hypercolor_core::input::SourceKind::Screen),
            45
        );
        assert_eq!(demand.screen_requested_extent(), None);
        let requests = demand.macos_renderer_screen_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].requested_hz().get(), 45);
        let hypercolor_core::input::screen::ScreenExtentRequest::Bounded(bounds) =
            requests[0].extent()
        else {
            panic!("preview request should preserve bounded renderer dimensions");
        };
        assert_eq!(bounds.max_width().map(NonZeroU32::get), Some(5_120));
        assert_eq!(bounds.max_height().map(NonZeroU32::get), Some(720));
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(
            demand.requested_hz(hypercolor_core::input::SourceKind::Screen),
            45
        );
        assert_eq!(
            demand.screen_requested_extent(),
            Some(
                hypercolor_core::input::screen::PixelExtent::new(5_120, 720)
                    .expect("test screen extent should be non-empty")
            )
        );
    }
}

#[test]
fn preview_sensor_demand_covers_metadata_control_and_layer_bindings() {
    let metadata_effect_id = EffectId::new(uuid::Uuid::now_v7());
    let mut metadata_entry = preview_effect_entry(metadata_effect_id);
    metadata_entry.metadata.tags = vec!["system-monitor".into()];
    let metadata_demand = preview_input_demand(
        &resolved_preview_scene(preview_effect_group(metadata_effect_id), metadata_entry),
        45,
    );
    assert_eq!(
        metadata_demand.requested_hz(hypercolor_core::input::SourceKind::Sensors),
        1
    );

    let control_effect_id = EffectId::new(uuid::Uuid::now_v7());
    let mut control_group = preview_effect_group(control_effect_id);
    control_group
        .control_bindings
        .insert("intensity".into(), preview_control_binding());
    let control_demand = preview_input_demand(
        &resolved_preview_scene(control_group, preview_effect_entry(control_effect_id)),
        45,
    );
    assert_eq!(
        control_demand.requested_hz(hypercolor_core::input::SourceKind::Sensors),
        1
    );

    let layer_effect_id = EffectId::new(uuid::Uuid::now_v7());
    let mut layer_group = preview_effect_group(layer_effect_id);
    layer_group.layers[0].bindings.push(LayerBinding {
        target: LayerParameter::Opacity,
        source: BindingSource::Sensor {
            name: "gpu.temperature".into(),
        },
        map: BindingMap::linear(20.0..=100.0, 0.0..=1.0),
    });
    let layer_demand = preview_input_demand(
        &resolved_preview_scene(layer_group, preview_effect_entry(layer_effect_id)),
        45,
    );
    assert_eq!(
        layer_demand.requested_hz(hypercolor_core::input::SourceKind::Sensors),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_scene_publishes_real_requested_size_frames() {
    let rig = PreviewTestRig::new([1.0, 0.0, 0.0, 1.0]).await;
    let attachment = rig.attach(1, "red");
    let lane = rig
        .executor
        .open(&attachment, spec(4, 3))
        .await
        .expect("preview lane should open");
    let mut receiver = lane.frame_receiver();

    let frame = next_frame(&mut receiver).await;

    assert_eq!((frame.width, frame.height), (4, 3));
    assert_eq!(frame.publication_id, attachment.publication_id());
    assert_eq!(&frame.surface.rgba_bytes()[..4], &[255, 0, 0, 255]);
    assert_eq!(frame.surface.rgba_len(), 4 * 3 * 4);
    drop(lane);
    drop(rig.browser);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lanes_render_concurrently_and_own_independent_demand_lifetimes() {
    let rig = PreviewTestRig::new([0.0, 1.0, 0.0, 1.0]).await;
    let first_attachment = rig.attach(1, "first");
    let second_attachment = rig.attach(2, "second");
    let mut first = rig
        .executor
        .open(&first_attachment, spec(3, 2))
        .await
        .expect("first lane should open");
    let second = rig
        .executor
        .open(&second_attachment, spec(5, 4))
        .await
        .expect("second lane should open");
    wait_for_preview_demands(&rig.demands, 2).await;
    let mut first_receiver = first.frame_receiver();
    let mut second_receiver = second.frame_receiver();

    let (first_frame, second_frame) = tokio::join!(
        next_frame(&mut first_receiver),
        next_frame(&mut second_receiver)
    );

    assert_eq!((first_frame.width, first_frame.height), (3, 2));
    assert_eq!((second_frame.width, second_frame.height), (5, 4));
    assert_ne!(
        first.snapshot().consumer_incarnation,
        second.snapshot().consumer_incarnation
    );
    drop(first_frame);
    drop(second_frame);
    let resources_with_both = rig.executor.resource_snapshot().used;
    assert!(first.close_and_wait().await);
    assert!(!first.close());
    assert_eq!(rig.executor.lane_count(), 1);
    assert_ne!(rig.executor.resource_snapshot().used, resources_with_both);
    assert_eq!(
        rig.demands
            .registration_count(InputPublicationConsumer::Preview),
        1
    );
    let _ = next_frame(&mut second_receiver).await;
    let mut second = second;
    assert!(second.close_and_wait().await);
    assert_eq!(rig.executor.lane_count(), 0);
    assert_eq!(
        rig.executor
            .resource_snapshot()
            .used
            .total_bytes()
            .expect("resource total should remain representable"),
        0
    );
    assert_eq!(
        rig.demands
            .registration_count(InputPublicationConsumer::Preview),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_panic_retires_lane_demand_and_resources_via_raii() {
    let rig = PreviewTestRig::new([0.3, 0.2, 0.8, 1.0]).await;
    let attachment = rig.attach(8, "panic");
    let mut lane = rig
        .executor
        .open(&attachment, spec(3, 2))
        .await
        .expect("preview lane should open");
    wait_for_preview_demands(&rig.demands, 1).await;
    let id = PreviewLaneId {
        key: attachment.key().clone(),
        publication_id: attachment.publication_id(),
    };
    let commands = rig
        .executor
        .inner
        .commands_exact(&id)
        .expect("opened lane should accept commands");

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    commands
        .send(PreviewLaneCommand::Panic {
            started: started_tx,
        })
        .await
        .expect("panic command should reach the worker");
    started_rx
        .await
        .expect("panic command should begin on the worker");
    let _ = lane.close_and_wait().await;
    wait_for_preview_demands(&rig.demands, 0).await;

    assert_eq!(rig.executor.lane_count(), 0);
    assert!(!lane.snapshot().active);
    assert_eq!(
        rig.executor
            .resource_snapshot()
            .used
            .total_bytes()
            .expect("resource total should remain representable"),
        0
    );
}

#[tokio::test]
async fn cancelled_update_never_enters_a_full_lane_mailbox() {
    let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
    let (started_tx, _started_rx) = tokio::sync::oneshot::channel();
    commands
        .try_send(PreviewLaneCommand::Panic {
            started: started_tx,
        })
        .expect("first command should fill the bounded mailbox");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let capacity = PreviewCapacityLedger::new(1);
    let resources = capacity
        .try_reserve(PreviewResourceLedger {
            metadata_bytes: 1,
            ..PreviewResourceLedger::default()
        })
        .expect("test update reservation should fit");

    let result = timeout(
        Duration::from_millis(100),
        request_preview_lane_update(&commands, &cancel, spec(1, 1), resources),
    )
    .await
    .expect("cancelled update should not wait for mailbox capacity");

    assert!(matches!(
        result,
        Err(super::InteractivePreviewError::WorkerClosed)
    ));
    assert!(matches!(
        receiver.try_recv(),
        Ok(PreviewLaneCommand::Panic { .. })
    ));
    assert_eq!(
        capacity
            .snapshot()
            .used
            .total_bytes()
            .expect("resource total should remain representable"),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_admits_many_small_lanes_without_spawning_per_lane_workers() {
    let rig = PreviewTestRig::new([0.2, 0.4, 0.8, 1.0]).await;
    let render_workers = rig.executor.render_worker_count();
    let encode_workers = rig.executor.encode_worker_count();
    let mut lanes = Vec::new();
    let mut attachments = Vec::new();

    for index in 0..80_u64 {
        let attachment = rig.attach(index + 1, &format!("lane-{index}"));
        lanes.push(
            rig.executor
                .open(&attachment, spec(1, 1))
                .await
                .expect("small lane should fit the aggregate byte ledger"),
        );
        attachments.push(attachment);
    }

    assert_eq!(rig.executor.lane_count(), 80);
    assert_eq!(rig.executor.render_worker_count(), render_workers);
    assert_eq!(rig.executor.encode_worker_count(), encode_workers);
    assert!(render_workers >= 1);
    assert!(encode_workers >= 1);
    drop(lanes);
    drop(attachments);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lane_control_mailbox_keeps_one_pending_update() {
    let rig = PreviewTestRig::new([0.2, 0.4, 0.8, 1.0]).await;
    let attachment = rig.attach(1, "mailbox");
    let mut lane = rig
        .executor
        .open(&attachment, spec(1, 1))
        .await
        .expect("preview lane should open");
    let capacity = super::lock(&rig.executor.inner.lanes)
        .get(attachment.key())
        .expect("opened lane should remain indexed")
        .commands
        .max_capacity();

    assert_eq!(capacity, 1);
    assert!(lane.close_and_wait().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_exhaustion_rejects_only_candidate_and_preserves_active_lane() {
    let requested_spec = spec(4, 3);
    let one_lane_bytes = lane_resource_bytes(requested_spec);
    let rig =
        PreviewTestRig::with_capacity([0.6, 0.3, 0.1, 1.0], one_lane_bytes + one_lane_bytes / 2)
            .await;
    let first_attachment = rig.attach(1, "preview");
    let first = rig
        .executor
        .open(&first_attachment, requested_spec)
        .await
        .expect("first lane should fit");
    let used_before = rig.executor.resource_snapshot().used;
    let second_attachment = rig.attach(2, "preview");

    let Err(error) = rig.executor.open(&second_attachment, requested_spec).await else {
        panic!("second lane should exceed aggregate bytes");
    };

    assert!(matches!(error, super::InteractivePreviewError::Capacity(_)));
    assert_eq!(rig.executor.lane_count(), 1);
    assert_eq!(rig.executor.resource_snapshot().used, used_before);
    assert!(first.snapshot().active);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_resize_keeps_original_spec_and_resource_reservation() {
    let original = spec(2, 2);
    let large = spec(512, 512);
    let capacity = lane_resource_bytes(original) + lane_resource_bytes(large) / 2;
    let rig = PreviewTestRig::with_capacity([0.3, 0.7, 0.2, 1.0], capacity).await;
    let attachment = rig.attach(1, "preview");
    let lane = rig
        .executor
        .open(&attachment, original)
        .await
        .expect("original lane should fit");
    let resources_before = rig.executor.resource_snapshot().used;

    let error = lane
        .resize_or_retarget(large)
        .await
        .expect_err("candidate resize should exceed overlap capacity");

    assert!(matches!(error, super::InteractivePreviewError::Capacity(_)));
    assert_eq!(lane.snapshot().spec, original);
    assert_eq!(rig.executor.resource_snapshot().used, resources_before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resize_retains_lane_identity_and_changes_output_dimensions() {
    let rig = PreviewTestRig::new([0.0, 0.0, 1.0, 1.0]).await;
    let attachment = rig.attach(1, "resize");
    let lane = rig
        .executor
        .open(&attachment, spec(2, 2))
        .await
        .expect("preview lane should open");
    let before = lane.snapshot();
    let mut receiver = lane.frame_receiver();
    let _ = next_frame(&mut receiver).await;

    lane.resize_or_retarget(spec(7, 5))
        .await
        .expect("preview lane should resize in place");
    let resized = loop {
        let frame = next_frame(&mut receiver).await;
        if (frame.width, frame.height) == (7, 5) {
            break frame;
        }
    };
    let after = lane.snapshot();

    assert_eq!((resized.width, resized.height), (7, 5));
    assert_eq!(before.consumer_incarnation, after.consumer_incarnation);
    assert_eq!(before.publication_id, after.publication_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_publication_cannot_close_reopened_lane() {
    let rig = PreviewTestRig::new([1.0, 0.0, 1.0, 1.0]).await;
    let key = BrowserInputChildKey::new(
        BrowserConnectionIncarnation::new(1),
        BrowserPreviewId::new("reopen"),
    );
    let old_attachment = rig
        .browser_handle
        .attach(key.clone())
        .expect("old browser preview should attach");
    let mut old_lane = rig
        .executor
        .open(&old_attachment, spec(2, 2))
        .await
        .expect("old preview lane should open");
    let stale_id = PreviewLaneId {
        key: key.clone(),
        publication_id: old_lane.publication_id(),
    };
    assert!(old_lane.close_and_wait().await);
    assert!(old_attachment.close());

    let new_attachment = rig
        .browser_handle
        .attach(key.clone())
        .expect("new browser preview should attach");
    let new_lane = rig
        .executor
        .open(&new_attachment, spec(2, 2))
        .await
        .expect("new preview lane should open");

    assert_ne!(stale_id.publication_id, new_lane.publication_id());
    assert!(!rig.executor.inner.close_exact(&stale_id));
    assert!(rig.executor.lane_snapshot(&key).is_some());
}
