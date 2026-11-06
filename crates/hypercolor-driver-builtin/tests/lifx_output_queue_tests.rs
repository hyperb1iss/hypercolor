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
use support::{FakeDevice, FakeKind, FakeLifx, Received, TestResult, assert_within_budget};

const ZONES: usize = 16;
const LAYOUT_DEVICE: &str = "lifx:strip";
const ZONE_ID: &str = "strip";
const INTERVAL: Duration = Duration::from_millis(50);
const BURST: usize = 5;
const SLACK: Duration = Duration::from_millis(25);

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
    })
    .scan_profiles()
    .await?;
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
async fn the_newest_frame_follows_a_paced_worst_case_frame() -> TestResult {
    let (fake, mut manager) = legacy_strip().await?;
    let layout = layout();

    // Sixteen different runs: the burst plus eleven paced messages, about
    // 550 ms on the wire. Frames pushed meanwhile replace each other in the
    // host's slot; only the last of them may follow.
    manager.write_frame(&frame(alternating(RED, BLUE)), &layout);
    tokio::time::sleep(Duration::from_millis(20)).await;
    for color in [GREEN, WHITE, GREEN, WHITE] {
        manager.write_frame(&frame(vec![color; ZONES]), &layout);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    manager.write_frame(&frame(vec![BLUE; ZONES]), &layout);
    tokio::time::sleep(Duration::from_millis(700)).await;

    let runs = fake.received_of_type(501);
    assert!(
        runs.len() > ZONES,
        "the newest frame followed the paced one"
    );
    assert!(
        runs[..ZONES]
            .iter()
            .enumerate()
            .all(|(zone, run)| usize::from(run.payload[0]) == zone),
        "the worst-case frame went out whole and in order"
    );
    let after = &runs[ZONES..];
    assert!(
        after.iter().all(|run| is_solid(run, BLUE)),
        "only the newest frame follows; replaced frames never reach the light"
    );
    assert_within_budget(&runs, BURST, INTERVAL, SLACK);
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
    let elapsed = started.elapsed();
    tokio::time::sleep(Duration::from_millis(60)).await;

    let runs = fake.received_of_type(501);
    // A one-run frame costs one message, so frames go out at the budget's
    // 20 per second (plus the initial burst), not at a worst-case rate.
    assert!(
        (15..=27).contains(&runs.len()),
        "{} one-run frames in {elapsed:?}",
        runs.len()
    );
    assert!(
        runs.iter()
            .all(|run| run.payload[1] == 15 && run.payload[14] == 1)
    );
    assert_within_budget(&runs, BURST, INTERVAL, SLACK);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worst_case_streams_never_burst_past_the_budget() -> TestResult {
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
    assert!(runs.len() >= 2 * ZONES, "{} runs", runs.len());
    assert_within_budget(&runs, BURST, INTERVAL, SLACK);
    Ok(())
}
