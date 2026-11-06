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
use support::{FakeDevice, FakeKind, FakeLifx, FakeTile, Received, TestResult};

const RED: [u8; 3] = [255, 0, 0];

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

    let started = Instant::now();
    backend.write_colors(&device_id, &[RED; 128]).await?;
    backend.write_colors(&device_id, &[RED; 128]).await?;
    assert!(
        started.elapsed() >= Duration::from_millis(90),
        "the pacer holds a back-to-back frame to the message budget"
    );

    let frames = fake.wait_for(715, 4).await;
    assert_eq!(frames.len(), 4);
    let tiles: Vec<u8> = frames.iter().map(|frame| frame.payload[0]).collect();
    assert_eq!(tiles, vec![0, 1, 0, 1]);
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
