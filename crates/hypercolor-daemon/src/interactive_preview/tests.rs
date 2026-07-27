use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputAttachment, BrowserInputChildKey, BrowserInputSource,
    BrowserPreviewId, InputManager, InputSource,
};
use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_types::config::InteractionRoutePolicy;
use hypercolor_types::layer::{
    LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId, ZoneRole};
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};
use tokio::sync::RwLock;
use tokio::time::timeout;

use super::{
    InteractivePreviewAcceleration, InteractivePreviewContext, InteractivePreviewExecutor,
    InteractivePreviewFrame, InteractivePreviewSpec, InteractivePreviewTarget, PreviewLaneId,
    advance_deadline, duration_millis_u32,
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
            sensor_snapshots: None,
            interaction_routing: routing,
            input_demands: demands.clone(),
            canvas_width: 8,
            canvas_height: 6,
            acceleration: InteractivePreviewAcceleration::cpu(),
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
fn preview_spec_rejects_unbounded_work() {
    let valid = InteractivePreviewSpec {
        target: InteractivePreviewTarget::ActiveScene,
        fps: 60,
        width: 4_096,
        height: 4_096,
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
            height: 4_097,
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
    assert!(first.close());
    assert!(!first.close());
    wait_for_preview_demands(&rig.demands, 1).await;
    let _ = next_frame(&mut second_receiver).await;
    drop(second);
    wait_for_preview_demands(&rig.demands, 0).await;
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
    assert!(old_lane.close());
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
