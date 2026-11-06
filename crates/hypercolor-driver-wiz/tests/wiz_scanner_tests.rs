//! Discovery and probing against the loopback fake (Spec 93 §3).

mod support;

use std::time::Duration;

use hypercolor_driver_wiz::scanner::{
    addr_from_metadata, collect_registrations, discovered_device, fingerprint_for_mac,
};
use hypercolor_driver_wiz::{ModuleClass, RequestPolicy, WizScanOptions, WizScanner};
use hypercolor_types::device::DeviceTopologyHint;
use support::{FakeSpec, FakeWiz, TestResult};

fn policy() -> RequestPolicy {
    RequestPolicy {
        reply_timeout: Duration::from_millis(200),
        attempts: 2,
    }
}

fn options(
    unicast: Vec<std::net::SocketAddr>,
    broadcast: Vec<std::net::SocketAddr>,
) -> WizScanOptions {
    WizScanOptions {
        unicast_targets: unicast,
        broadcast_targets: broadcast,
        timeout: Duration::from_millis(800),
        request_policy: policy(),
    }
}

#[tokio::test]
async fn registration_finds_the_device_and_probes_it() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    // A loopback "broadcast" target exercises the discovery path without
    // putting anything on the LAN.
    let profiles = WizScanner::new(options(Vec::new(), vec![fake.addr()]))
        .scan_profiles()
        .await?;
    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.addr, fake.addr());
    assert_eq!(profile.mac, "a8:bb:50:00:00:01");
    assert_eq!(profile.class, ModuleClass::Rgb);
    assert_eq!(profile.min_dimming, 10);
    assert_eq!(profile.firmware.as_deref(), Some("1.25.0"));
    assert_eq!(
        fake.methods(),
        ["registration", "getSystemConfig", "getModelConfig"]
    );
    Ok(())
}

#[tokio::test]
async fn registration_replies_report_the_device_port() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec::color_bulb()).await?;
    let found = collect_registrations(&[fake.addr()], Duration::from_millis(300), 8).await?;
    assert_eq!(found, [fake.addr()]);
    Ok(())
}

#[tokio::test]
async fn older_firmware_without_model_config_uses_the_default_floor() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec {
        min_dim_level: None,
        module_name: "ESP15_SHTW1_01I".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = WizScanner::new(options(vec![fake.addr()], Vec::new()))
        .scan_profiles()
        .await?;
    assert_eq!(profiles[0].class, ModuleClass::TunableWhite);
    assert_eq!(profiles[0].min_dimming, 10);
    Ok(())
}

#[tokio::test]
async fn smart_plugs_are_skipped() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec {
        module_name: "ESP10_SOCKET_06".to_owned(),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = WizScanner::new(options(vec![fake.addr()], Vec::new()))
        .scan_profiles()
        .await?;
    assert!(profiles.is_empty());
    Ok(())
}

#[tokio::test]
async fn silent_addresses_are_skipped() -> TestResult {
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
    let profiles = WizScanner::new(options(vec![silent.local_addr()?], Vec::new()))
        .scan_profiles()
        .await?;
    assert!(profiles.is_empty());
    Ok(())
}

#[tokio::test]
async fn discovered_device_is_one_point_light() -> TestResult {
    let fake = FakeWiz::spawn(FakeSpec {
        min_dim_level: Some(1),
        ..FakeSpec::color_bulb()
    })
    .await?;
    let profiles = WizScanner::new(options(vec![fake.addr(), fake.addr()], Vec::new()))
        .scan_profiles()
        .await?;
    assert_eq!(profiles.len(), 1);
    let discovered = discovered_device(&profiles[0], 20);
    assert_eq!(
        discovered.fingerprint,
        fingerprint_for_mac("a8:bb:50:00:00:01")
    );
    assert_eq!(discovered.info.name, "WiZ Color 000001");
    assert_eq!(discovered.info.vendor, "WiZ");
    assert_eq!(discovered.info.model.as_deref(), Some("ESP01_SHRGB1C_31"));
    assert_eq!(discovered.info.total_led_count(), 1);
    assert_eq!(
        discovered.info.segments[0].topology,
        DeviceTopologyHint::Point
    );
    assert_eq!(discovered.info.capabilities.max_fps, 20);
    assert!(discovered.claim.is_some());
    assert_eq!(discovered.metadata["module_class"], "rgb");
    assert_eq!(discovered.metadata["min_dimming"], "1");
    assert_eq!(addr_from_metadata(&discovered.metadata)?, fake.addr());
    Ok(())
}
