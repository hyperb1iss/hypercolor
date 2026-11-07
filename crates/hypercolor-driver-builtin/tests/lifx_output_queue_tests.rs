//! The LIFX backend behind the host's real latest-value output queue
//! (Spec 88 §9). Frames go through `BackendManager::write_frame`, the
//! per-device output worker, and the LIFX frame sink, and land on the
//! loopback fake responder shared with the driver crate's tests.

#![cfg(feature = "lifx")]

#[path = "../../hypercolor-driver-lifx/tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hypercolor_core::device::BackendManager;
use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_lifx::LifxBackend;
use hypercolor_driver_lifx::color::{Hsbk, HsbkEncoder};
use hypercolor_driver_lifx::config::LifxConfig;
use hypercolor_driver_lifx::scanner::{LifxScanOptions, LifxScanner, discovered_device};
use hypercolor_driver_lifx::transport::RequestPolicy;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::{
    EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};
use support::{FakeDevice, FakeKind, FakeLifx, Received, TestResult, assert_budget_respected};

const ZONES: usize = 16;
const LAYOUT_DEVICE: &str = "lifx:strip";
const ZONE_ID: &str = "strip";
const INTERVAL: Duration = Duration::from_millis(50);
const BURST: usize = 5;

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

fn layout() -> SpatialLayout {
    SpatialLayout {
        id: "lifx-queue".into(),
        name: "LIFX queue".into(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![Output {
            id: ZONE_ID.into(),
            name: ZONE_ID.into(),
            device_id: LAYOUT_DEVICE.into(),
            zone_name: None,
            position: NormalizedPosition { x: 0.5, y: 0.5 },
            size: NormalizedPosition { x: 1.0, y: 1.0 },
            rotation: 0.0,
            scale: 1.0,
            brightness: Some(1.0),
            orientation: None,
            topology: LedTopology::Strip {
                count: u32::try_from(ZONES).expect("fits"),
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn frame(colors: Vec<[u8; 3]>) -> Vec<ZoneColors> {
    vec![ZoneColors {
        zone_id: ZONE_ID.into(),
        colors,
    }]
}

fn alternating(first: [u8; 3], second: [u8; 3]) -> Vec<[u8; 3]> {
    (0..ZONES)
        .map(|zone| if zone % 2 == 0 { first } else { second })
        .collect()
}

/// A legacy 16-zone LIFX Z behind a `BackendManager`, ready for frames.
async fn legacy_strip() -> TestResult<(FakeLifx, BackendManager)> {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip {
            zones: u8::try_from(ZONES).expect("fits"),
        },
    )])
    .await?;
    let policy = RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    };
    let profiles = LifxScanner::new(LifxScanOptions {
        unicast_targets: vec![fake.addr()],
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(600),
        request_policy: policy,
        ..LifxScanOptions::default()
    })
    .scan()
    .await?
    .found;
    let config = LifxConfig::default();
    let discovered = discovered_device(&profiles[0], config.max_messages_per_second);
    let backend = LifxBackend::new(config).with_request_policy(policy);
    backend.adopt_device(&discovered)?;

    let mut manager = BackendManager::new();
    manager.register_backend(Arc::new(backend));
    manager
        .connect_device("lifx", discovered.info.id, LAYOUT_DEVICE)
        .await?;
    fake.clear();
    Ok((fake, manager))
}

fn encode(rgb: [u8; 3]) -> Hsbk {
    HsbkEncoder::new(6_500, true).encode(rgb)
}

fn run_color(message: &Received) -> Hsbk {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&message.payload[2..10]);
    Hsbk::from_le_bytes(raw)
}

fn is_solid(message: &Received, rgb: [u8; 3]) -> bool {
    message.payload[0] == 0
        && usize::from(message.payload[1]) == ZONES - 1
        && run_color(message) == encode(rgb)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newer_frames_replace_a_paced_worst_case_tail() -> TestResult {
    let (fake, mut manager) = legacy_strip().await?;
    let layout = layout();

    // Sixteen different runs would take the burst plus about 550 ms of
    // paced messages. Solid frames pushed meanwhile each cost one message,
    // so the first of them replaces the tail, and the newest one wins.
    let started = Instant::now();
    manager.write_frame(&frame(alternating(RED, BLUE)), &layout);
    tokio::time::sleep(Duration::from_millis(20)).await;
    for color in [GREEN, WHITE, GREEN, WHITE] {
        manager.write_frame(&frame(vec![color; ZONES]), &layout);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    manager.write_frame(&frame(vec![BLUE; ZONES]), &layout);
    tokio::time::sleep(Duration::from_millis(700)).await;

    let runs = fake.received_of_type(501);
    let first_solid = runs
        .iter()
        .position(|run| run.payload[0] == 0 && usize::from(run.payload[1]) == ZONES - 1)
        .expect("a solid frame was sent");
    assert!(
        first_solid < ZONES - 1,
        "the worst-case tail was abandoned after {first_solid} runs"
    );
    assert!(
        runs[..first_solid].iter().all(|run| run.payload[14] == 0),
        "the abandoned frame never applied"
    );
    let last = runs.last().expect("runs");
    assert!(
        is_solid(last, BLUE),
        "the newest frame is what the light shows"
    );
    assert!(
        runs[first_solid..]
            .iter()
            .all(|run| usize::from(run.payload[1]) == ZONES - 1 && run.payload[14] == 1),
        "only whole solid frames follow"
    );
    assert_budget_respected(&runs, started, BURST, INTERVAL);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uniform_animation_runs_at_the_full_message_rate() -> TestResult {
    let (fake, mut manager) = legacy_strip().await?;
    let layout = layout();

    let started = Instant::now();
    let mut step = 0_u8;
    while started.elapsed() < Duration::from_secs(1) {
        step = step.wrapping_add(7);
        manager.write_frame(&frame(vec![[step, 0, 255 - step]; ZONES]), &layout);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(60)).await;

    let runs = fake.received_of_type(501);
    // A one-run frame costs one message, so frames flow near the budget's
    // 20 per second. The lower bound only guards against a worst-case
    // cadence (one frame a second for this strip) and survives a loaded
    // machine; the budget check is the upper bound.
    assert!(runs.len() >= 8, "{} one-run frames in a second", runs.len());
    assert!(
        runs.iter()
            .all(|run| run.payload[1] == 15 && run.payload[14] == 1)
    );
    assert_budget_respected(&runs, started, BURST, INTERVAL);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worst_case_streams_finish_frames_within_the_budget() -> TestResult {
    let (fake, mut manager) = legacy_strip().await?;
    let layout = layout();

    let started = Instant::now();
    let mut flip = false;
    while started.elapsed() < Duration::from_millis(1_500) {
        let colors = if flip {
            alternating(RED, BLUE)
        } else {
            alternating(BLUE, RED)
        };
        flip = !flip;
        manager.write_frame(&frame(colors), &layout);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(900)).await;

    let runs = fake.received_of_type(501);
    // Every frame costs sixteen messages, so newer frames never replace a
    // tail and every frame that starts also finishes.
    let applied = runs.iter().filter(|run| run.payload[14] == 1).count();
    assert!(
        applied >= 2,
        "{applied} frames applied out of {} runs",
        runs.len()
    );
    for (index, run) in runs.iter().enumerate() {
        let position = index % ZONES;
        assert_eq!(
            usize::from(run.payload[0]),
            position,
            "runs go out whole and in order"
        );
    }
    assert_budget_respected(&runs, started, BURST, INTERVAL);
    Ok(())
}

/// Every frame the render loop offers lands in exactly one bucket, whichever
/// way a replacement goes: a frame the sender abandons counts as dropped and
/// never as sent, and one it finishes counts as sent and never as dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_frame_lands_in_one_bucket_across_won_and_refused_replacements() -> TestResult {
    let (_fake, mut manager) = legacy_strip().await?;
    let layout = layout();
    let settled = |manager: &BackendManager, offered: u64| {
        let stats = manager.device_output_statistics();
        stats[0].frames_sent + stats[0].frames_dropped >= offered
    };

    // A cheap frame replaces the worst-case tail: the first frame is dropped
    // and the second is sent.
    manager.write_frame(&frame(alternating(RED, BLUE)), &layout);
    tokio::time::sleep(Duration::from_millis(120)).await;
    manager.write_frame(&frame(vec![GREEN; ZONES]), &layout);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !settled(&manager, 2) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;

    // An equally costly frame is refused, so it waits and both are sent.
    manager.write_frame(&frame(alternating(RED, BLUE)), &layout);
    tokio::time::sleep(Duration::from_millis(120)).await;
    manager.write_frame(&frame(alternating(BLUE, RED)), &layout);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !settled(&manager, 4) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;

    let stats = manager.device_output_statistics();
    assert_eq!(stats[0].frames_received, 4);
    assert_eq!(
        stats[0].frames_sent, 3,
        "the replacement and the refused pair"
    );
    assert_eq!(stats[0].frames_dropped, 1, "only the abandoned tail");
    assert_eq!(stats[0].coalesced_backend_overrun, 1);
    assert_eq!(stats[0].coalesced_target_cadence, 0);
    Ok(())
}
