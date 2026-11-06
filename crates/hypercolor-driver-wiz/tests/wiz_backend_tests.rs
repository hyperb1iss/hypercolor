//! Backend connect, color frames, power, brightness, and signing against
//! the loopback fake (Spec 93 §8).

mod support;

use std::sync::Arc;
use std::time::Duration;

use hypercolor_driver_api::{DeviceBackend, DiscoveredDevice};
use hypercolor_driver_wiz::backend::representative_color;
use hypercolor_driver_wiz::config::WizConfig;
use hypercolor_driver_wiz::{
    RequestPolicy, WizBackend, WizScanOptions, WizScanner, discovered_device,
};
use hypercolor_types::device::DeviceError;
use support::{FakeSpec, FakeWiz, TestResult};

const KEY_HEX: &str = "00112233445566778899aabbccddeeff";
const KEY: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

fn policy() -> RequestPolicy {
    RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    }
}

async fn discover(fake: &FakeWiz) -> TestResult<DiscoveredDevice> {
    let profiles = WizScanner::new(WizScanOptions {
        unicast_targets: vec![fake.addr()],
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(800),
        request_policy: policy(),
    })
    .scan_profiles()
    .await?;
    let profile = profiles.first().ok_or("fake device was not discovered")?;
    Ok(discovered_device(profile, 20))
}

async fn connected(
    fake: &FakeWiz,
    config: WizConfig,
) -> TestResult<(WizBackend, DiscoveredDevice)> {
    let discovered = discover(fake).await?;
    let backend = WizBackend::new(config).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    backend.connect(&discovered.info.id).await?;
    Ok((backend, discovered))
}

#[tokio::test]
async fn frames_become_set_pilot_with_power_on_first() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    let (backend, discovered) = connected(&fake, WizConfig::default()).await?;
    let id = discovered.info.id;

    let cadence = backend.output_cadence(&id).ok_or("cadence")?;
    assert_eq!(backend.target_fps(&id), Some(20));
    assert_eq!(cadence.min_interval(), Some(Duration::from_millis(50)));
    assert_eq!(cadence.max_frame_silence(), Some(Duration::from_secs(2)));

    backend.write_colors(&id, &[[128, 64, 0]]).await?;
    backend.write_colors(&id, &[[0, 0, 255]]).await?;
    backend.write_colors(&id, &[[0, 0, 0]]).await?;
    backend.write_colors(&id, &[[255, 255, 255]]).await?;
    let pilots = fake.wait_for_pilots(4).await;
    let params: Vec<serde_json::Value> = pilots.iter().map(|pilot| pilot.params.clone()).collect();
    assert_eq!(
        params,
        [
            serde_json::json!({ "state": true, "r": 255, "g": 128, "b": 0, "dimming": 50 }),
            serde_json::json!({ "r": 0, "g": 0, "b": 255, "dimming": 100 }),
            serde_json::json!({ "state": false }),
            serde_json::json!({ "state": true, "r": 255, "g": 255, "b": 255, "dimming": 100 }),
        ]
    );
    assert!(pilots.iter().all(|pilot| pilot.accepted));
    Ok(())
}

#[tokio::test]
async fn brightness_scales_the_dimming_level() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    let (backend, discovered) = connected(&fake, WizConfig::default()).await?;
    let id = discovered.info.id;
    backend.set_brightness(&id, 128).await?;
    let sink = backend.frame_sink(&id).ok_or("frame sink")?;
    sink.write_colors_shared(Arc::new(vec![[255, 0, 0]]))
        .await?;
    let pilots = fake.wait_for_pilots(1).await;
    assert_eq!(
        pilots[0].params,
        serde_json::json!({ "state": true, "r": 255, "g": 0, "b": 0, "dimming": 50 })
    );
    Ok(())
}

#[tokio::test]
async fn white_bulbs_take_dimming_only() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec {
        module_name: "ESP05_SHDW_21".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let (backend, discovered) = connected(&fake, WizConfig::default()).await?;
    backend
        .write_colors(&discovered.info.id, &[[255, 255, 255]])
        .await?;
    let pilots = fake.wait_for_pilots(1).await;
    assert_eq!(
        pilots[0].params,
        serde_json::json!({ "state": true, "dimming": 100 })
    );
    Ok(())
}

#[tokio::test]
async fn signed_frames_verify_against_the_home_key() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec {
        signing_key: Some(KEY),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let config = WizConfig {
        signing_key: Some(KEY_HEX.to_owned()),
        ..WizConfig::default()
    };
    let (backend, discovered) = connected(&fake, config).await?;
    backend
        .write_colors(&discovered.info.id, &[[0, 255, 0]])
        .await?;
    let pilots = fake.wait_for_pilots(1).await;
    assert!(pilots[0].accepted, "signature must verify: {pilots:?}");
    assert!(pilots[0].params.get("sigTs").is_some());
    assert_eq!(pilots[0].params["g"], 255);
    Ok(())
}

#[tokio::test]
async fn unsigned_frames_are_rejected_by_a_verified_home() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec {
        signing_key: Some(KEY),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let (backend, discovered) = connected(&fake, WizConfig::default()).await?;
    backend
        .write_colors(&discovered.info.id, &[[0, 255, 0]])
        .await?;
    let pilots = fake.wait_for_pilots(1).await;
    assert!(!pilots[0].accepted);
    Ok(())
}

#[tokio::test]
async fn disconnect_closes_the_sink() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    let (backend, discovered) = connected(&fake, WizConfig::default()).await?;
    let id = discovered.info.id;
    let sink = backend.frame_sink(&id).ok_or("frame sink")?;
    backend.disconnect(&id).await?;
    assert!(sink.is_closed());
    assert!(matches!(
        sink.write_colors_shared(Arc::new(vec![[1, 2, 3]])).await,
        Err(DeviceError::Disconnected { .. })
    ));
    assert!(matches!(
        backend.write_colors(&id, &[[1, 2, 3]]).await,
        Err(DeviceError::Disconnected { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn connect_refuses_a_different_device_at_the_address() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    let mut discovered = discover(&fake).await?;
    let other = FakeWiz::spawn(FakeSpec {
        mac: "a8bb50000099".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    discovered
        .metadata
        .insert("port".to_owned(), other.addr().port().to_string());
    let backend = WizBackend::new(WizConfig::default()).with_request_policy(policy());
    backend.adopt_device(&discovered)?;
    assert!(matches!(
        backend.connect(&discovered.info.id).await,
        Err(DeviceError::ConnectionFailed { .. })
    ));
    Ok(())
}

#[test]
fn multi_led_frames_average_to_one_color() {
    assert_eq!(representative_color(&[]), [0, 0, 0]);
    assert_eq!(representative_color(&[[10, 20, 30]]), [10, 20, 30]);
    assert_eq!(
        representative_color(&[[255, 0, 0], [0, 0, 255]]),
        [127, 0, 127]
    );
}
