//! Backend connect, write, brightness, and cadence against the loopback
//! fake responder (Spec 88 §9 and §11.5).

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_driver_api::{
    DeviceBackend, DeviceDeliveryAck, DeviceDeliveryId, DeviceDeliveryObserver,
    DeviceDeliveryStatus, DiscoveredDevice,
};
use hypercolor_driver_lifx::LifxBackend;
use hypercolor_driver_lifx::color::{Hsbk, HsbkEncoder};
use hypercolor_driver_lifx::config::LifxConfig;
use hypercolor_driver_lifx::scanner::{LifxScanOptions, LifxScanner, discovered_device};
use hypercolor_driver_lifx::transport::RequestPolicy;
use hypercolor_types::device::DeviceError;
use support::{
    FakeDevice, FakeKind, FakeLifx, FakeTile, Received, TestResult, assert_within_budget,
};

const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

fn policy() -> RequestPolicy {
    RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    }
}

async fn discover(fake: &FakeLifx, config: &LifxConfig) -> TestResult<Vec<DiscoveredDevice>> {
    let profiles = LifxScanner::new(LifxScanOptions {
        unicast_targets: vec![fake.addr()],
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(600),
        request_policy: policy(),
        ..LifxScanOptions::default()
    })
    .scan_profiles()
    .await?;
    Ok(profiles
        .iter()
        .map(|profile| discovered_device(profile, config.max_messages_per_second))
        .collect())
}

fn hsbk_at(payload: &[u8], offset: usize) -> Hsbk {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&payload[offset..offset + 8]);
    Hsbk::from_le_bytes(raw)
}

fn last(messages: &[Received]) -> &Received {
    messages.last().expect("at least one message")
}

#[tokio::test]
async fn connect_powers_on_and_streams_extended_zones() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        3,
        38,
        (3, 70),
        FakeKind::ExtendedStrip { zones: 82 },
    )])
    .await?;
    let config = LifxConfig {
        transition_ms: 30,
        ..LifxConfig::default()
    };
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let power = fake.wait_for(117, 1).await;
    assert_eq!(power.len(), 1);
    assert!(
        power[0].ack_required,
        "power on asks for an acknowledgement"
    );
    assert_eq!(&power[0].payload, &[0xff, 0xff, 0, 0, 0, 0]);

    let info = backend
        .connected_device_info(&device_id)
        .await?
        .expect("connected info");
    assert_eq!(info.total_led_count(), 82);
    assert_eq!(info.capabilities.max_fps, 20);
    let metadata = backend
        .connected_device_metadata(&device_id)
        .await?
        .expect("connected metadata");
    assert_eq!(metadata["device_class"], "extended_multizone");

    let cadence = backend.output_cadence(&device_id).expect("cadence");
    assert_eq!(cadence.min_interval(), Some(Duration::from_millis(50)));
    assert_eq!(cadence.max_frame_silence(), Some(Duration::from_secs(1)));
    assert_eq!(backend.target_fps(&device_id), Some(20));

    backend.write_colors(&device_id, &[RED; 82]).await?;
    let frames = fake.wait_for(510, 1).await;
    let frame = last(&frames);
    assert!(!frame.ack_required && !frame.res_required && !frame.tagged);
    assert_eq!(&frame.payload[0..4], &30_u32.to_le_bytes());
    assert_eq!(frame.payload[4], 1);
    assert_eq!(frame.payload[7], 82);
    let full = HsbkEncoder::new(6_500, true).encode(RED);
    assert_eq!(hsbk_at(&frame.payload, 8), full);

    backend.set_brightness(&device_id, 128).await?;
    backend.write_colors(&device_id, &[RED; 82]).await?;
    let frames = fake.wait_for(510, 2).await;
    let dimmed = hsbk_at(&last(&frames).payload, 8);
    assert_eq!(dimmed.hue, full.hue);
    assert_eq!(dimmed.saturation, full.saturation);
    assert_eq!(dimmed.brightness, 32_896);

    backend.disconnect(&device_id).await?;
    assert!(matches!(
        backend.write_colors(&device_id, &[RED; 82]).await,
        Err(DeviceError::Disconnected { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn tile_chains_stream_one_set64_per_tile_at_the_budgeted_cadence() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        5,
        55,
        (3, 50),
        FakeKind::Matrix {
            tiles: vec![FakeTile::upright(8, 8); 2],
        },
    )])
    .await?;
    let config = LifxConfig::default();
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let cadence = backend.output_cadence(&device_id).expect("cadence");
    assert_eq!(cadence.min_interval(), Some(Duration::from_millis(100)));
    assert_eq!(cadence.target_fps(), 10);

    // Three frames are six messages: the first five fit the burst and the
    // sixth waits one message interval.
    let started = Instant::now();
    for _ in 0..3 {
        backend.write_colors(&device_id, &[RED; 128]).await?;
    }
    assert!(
        started.elapsed() >= Duration::from_millis(45),
        "the budget holds the sixth message back"
    );

    let frames = fake.wait_for(715, 6).await;
    assert_eq!(frames.len(), 6);
    let tiles: Vec<u8> = frames.iter().map(|frame| frame.payload[0]).collect();
    assert_eq!(tiles, vec![0, 1, 0, 1, 0, 1]);
    assert_within_budget(
        &frames,
        5,
        Duration::from_millis(50),
        Duration::from_millis(25),
    );
    Ok(())
}

fn alternating(zones: usize) -> Vec<[u8; 3]> {
    (0..zones)
        .map(|zone| if zone % 2 == 0 { RED } else { BLUE })
        .collect()
}

async fn connect_legacy_strip(
    fake: &FakeLifx,
) -> TestResult<(LifxBackend, hypercolor_types::device::DeviceId)> {
    let config = LifxConfig::default();
    let discovered = discover(fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;
    Ok((backend, device_id))
}

#[tokio::test]
async fn worst_case_legacy_frames_are_paced_message_by_message() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )])
    .await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;

    let cadence = backend.output_cadence(&device_id).expect("cadence");
    assert_eq!(
        cadence.min_interval(),
        Some(Duration::from_millis(50)),
        "the host offers frames at the cheapest frame's cost"
    );
    assert_eq!(backend.target_fps(&device_id), Some(20));

    let started = Instant::now();
    backend.write_colors(&device_id, &alternating(16)).await?;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(540),
        "16 runs cost 11 paced intervals after the burst, took {elapsed:?}"
    );

    let runs = fake.wait_for(501, 16).await;
    assert_eq!(runs.len(), 16);
    assert!(runs[..15].iter().all(|run| run.payload[14] == 0));
    assert_eq!(runs[15].payload[14], 1, "only the last run applies");
    assert_within_budget(
        &runs,
        5,
        Duration::from_millis(50),
        Duration::from_millis(25),
    );
    Ok(())
}

#[tokio::test]
async fn sink_declines_a_frame_while_the_budget_is_spent() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        31,
        (2, 60),
        FakeKind::LegacyStrip { zones: 16 },
    )])
    .await?;
    let (backend, device_id) = connect_legacy_strip(&fake).await?;
    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());
    let id = |sequence| DeviceDeliveryId {
        queue_generation: 1,
        sequence,
    };

    let first = sink
        .deliver_colors_shared_observed(id(1), Arc::new(alternating(16)), observer.clone())
        .await;
    assert_eq!(first.status, DeviceDeliveryStatus::Completed);

    let declined = sink
        .deliver_colors_shared_observed(id(2), Arc::new(vec![RED; 16]), observer.clone())
        .await;
    assert_eq!(declined.status, DeviceDeliveryStatus::SuppressedCadence);
    assert!(!declined.transport_started);

    tokio::time::sleep(Duration::from_millis(60)).await;
    let accepted = sink
        .deliver_colors_shared_observed(id(3), Arc::new(vec![RED; 16]), observer.clone())
        .await;
    assert_eq!(accepted.status, DeviceDeliveryStatus::Completed);
    assert_eq!(
        observer.started.lock().expect("observer lock").as_slice(),
        &[id(1), id(3)],
        "a declined frame never starts transport"
    );
    let runs = fake.wait_for(501, 17).await;
    assert_eq!(runs.len(), 17);
    assert_eq!((runs[16].payload[0], runs[16].payload[1]), (0, 15));
    Ok(())
}

#[derive(Default)]
struct RecordingObserver {
    started: Mutex<Vec<DeviceDeliveryId>>,
}

impl DeviceDeliveryObserver for RecordingObserver {
    fn transport_started(&self, id: DeviceDeliveryId) {
        self.started.lock().expect("observer lock").push(id);
    }

    fn delivery_terminal(&self, _ack: &DeviceDeliveryAck) {}
}

#[tokio::test]
async fn frame_sink_reports_completed_deliveries() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let config = LifxConfig::default();
    let discovered = discover(&fake, &config).await?.remove(0);
    let device_id = discovered.info.id;
    let backend = LifxBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&device_id).await?;

    let sink = backend.frame_sink(&device_id).expect("frame sink");
    let observer = Arc::new(RecordingObserver::default());
    let id = DeviceDeliveryId {
        queue_generation: 1,
        sequence: 1,
    };
    let ack = sink
        .deliver_colors_shared_observed(id, Arc::new(vec![RED]), observer.clone())
        .await;
    assert_eq!(ack.status, DeviceDeliveryStatus::Completed);
    assert_eq!(
        observer.started.lock().expect("observer lock").as_slice(),
        &[id]
    );
    let frames = fake.wait_for(102, 1).await;
    assert_eq!(frames.len(), 1);
    Ok(())
}

#[tokio::test]
async fn adoption_requires_routing_metadata() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let mut discovered = discover(&fake, &LifxConfig::default()).await?.remove(0);
    discovered.metadata = HashMap::from([("ip".to_owned(), "127.0.0.1".to_owned())]);
    let backend = LifxBackend::new(LifxConfig::default());
    assert!(matches!(
        backend.adopt_device(&discovered),
        Err(DeviceError::NotAdopted { .. })
    ));
    assert!(matches!(
        backend.connect(&discovered.info.id).await,
        Err(DeviceError::NotAdopted { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn connect_fails_when_the_light_goes_away() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let discovered = discover(&fake, &LifxConfig::default()).await?.remove(0);
    drop(fake);
    let backend = LifxBackend::new(LifxConfig::default()).with_request_policy(RequestPolicy {
        reply_timeout: Duration::from_millis(30),
        attempts: 1,
    });
    backend.adopt_device(&discovered)?;
    assert!(backend.connect(&discovered.info.id).await.is_err());
    assert!(backend.output_cadence(&discovered.info.id).is_none());
    Ok(())
}
