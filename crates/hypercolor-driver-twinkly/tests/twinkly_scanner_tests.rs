//! Discovery and probing against the loopback fake (Spec 92 §3 and §10.4).

mod support;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_driver_api::CoverageGap;
use hypercolor_driver_support::discovery::CandidateAdmission;
use hypercolor_driver_twinkly::scanner::{
    canonical_mac, collect_discovery_replies, discovered_device, endpoint_from_metadata,
    fingerprint_for_mac,
};
use hypercolor_driver_twinkly::{
    LedProfile, RtProtocol, TwinklyEndpoint, TwinklyScanOptions, TwinklyScanner,
};
use hypercolor_types::device::{DeviceFingerprint, DeviceTopologyHint};
use support::{FakeSpec, FakeTwinkly, TestResult};

fn unicast(endpoints: Vec<TwinklyEndpoint>) -> TwinklyScanOptions {
    TwinklyScanOptions::new(endpoints, Vec::new(), Duration::from_millis(800))
}

#[tokio::test]
async fn discovery_datagram_finds_the_device_and_probes_it() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(250)).await?;
    // A loopback "broadcast" target exercises the discovery path without
    // putting anything on the LAN.
    let mut options = TwinklyScanOptions::new(
        Vec::new(),
        vec![fake.discovery_addr()],
        Duration::from_millis(800),
    );
    options.http_port = fake.http_port();
    options.rt_port = fake.rt_port();
    let profiles = TwinklyScanner::new(options).scan().await?.found;

    assert_eq!(fake.discovery_requests(), [b"\x01discover".to_vec()]);
    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.endpoint, fake.endpoint());
    assert_eq!(profile.mac, "98:f4:ab:1c:c1:90");
    assert_eq!(profile.led_count, 250);
    assert_eq!(profile.led_profile, LedProfile::Rgb);
    assert_eq!(profile.rt_protocol(), RtProtocol::V3);
    assert_eq!(profile.device_name.as_deref(), Some("Porch String"));

    // Discovery never logs in: that would sign out an active stream.
    assert!(
        fake.requests()
            .iter()
            .all(|request| request.path == "/xled/v1/gestalt"
                || request.path == "/xled/v1/fw/version"),
        "scan must only read unauthenticated endpoints: {:?}",
        fake.requests()
    );
    Ok(())
}

#[tokio::test]
async fn discovery_replies_report_the_sender() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(10)).await?;
    let found = collect_discovery_replies(
        &[fake.discovery_addr()],
        Duration::from_millis(300),
        CandidateAdmission::new(8, []),
    )
    .await?
    .found;
    assert_eq!(found, [IpAddr::V4(Ipv4Addr::LOCALHOST)]);
    Ok(())
}

#[tokio::test]
async fn unicast_endpoints_are_probed_and_deduplicated_by_mac() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(600)).await?;
    let endpoint = fake.endpoint();
    let profiles = TwinklyScanner::new(unicast(vec![endpoint, endpoint]))
        .scan()
        .await?
        .found;
    assert_eq!(profiles.len(), 1);

    let discovered = discovered_device(&profiles[0], 60);
    assert_eq!(
        discovered.fingerprint,
        fingerprint_for_mac("98:f4:ab:1c:c1:90")
    );
    assert_eq!(discovered.info.id, profiles[0].device_id());
    assert_eq!(discovered.info.name, "Porch String");
    assert_eq!(discovered.info.vendor, "Twinkly");
    assert_eq!(discovered.info.model.as_deref(), Some("TWS250STP"));
    assert_eq!(discovered.info.firmware_version.as_deref(), Some("2.8.3"));
    assert_eq!(discovered.info.total_led_count(), 600);
    assert_eq!(discovered.info.capabilities.max_fps, 60);
    assert_eq!(
        discovered.info.segments[0].topology,
        DeviceTopologyHint::Strip
    );
    assert!(discovered.connect_behavior.should_auto_connect());
    assert!(discovered.claim.is_some());
    assert_eq!(discovered.metadata["mac"], "98:f4:ab:1c:c1:90");
    assert_eq!(discovered.metadata["rt_protocol"], "v3");
    assert_eq!(discovered.metadata["led_profile"], "RGB");
    assert_eq!(discovered.metadata["fw_family"], "F");
    assert_eq!(discovered.metadata["gestalt_frame_rate"], "23.81");
    assert_eq!(endpoint_from_metadata(&discovered.metadata)?, endpoint);
    Ok(())
}

#[tokio::test]
async fn unreachable_endpoints_are_skipped() -> TestResult {
    let closed = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let outcome = TwinklyScanner::new(unicast(vec![TwinklyEndpoint {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        http_port: port,
        rt_port: 7777,
    }]))
    .scan()
    .await?;
    assert!(outcome.found.is_empty());
    assert!(
        outcome.gaps.is_empty(),
        "a remembered endpoint that never answers is gone: {:?}",
        outcome.gaps
    );
    Ok(())
}

#[tokio::test]
async fn a_tracked_device_that_answers_but_fails_its_probe_holds_only_itself() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(10)).await?;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let fingerprint = DeviceFingerprint::from_persisted("net:twinkly:lamp".to_owned());
    let outcome = TwinklyScanner::new(TwinklyScanOptions {
        http_port: port,
        tracked: HashMap::from([(IpAddr::V4(Ipv4Addr::LOCALHOST), fingerprint.clone())]),
        ..TwinklyScanOptions::new(
            Vec::new(),
            vec![fake.discovery_addr()],
            Duration::from_millis(600),
        )
    })
    .scan()
    .await?;
    assert!(outcome.found.is_empty());
    let [gap] = outcome.gaps.as_slice() else {
        panic!("expected one gap, got {:?}", outcome.gaps);
    };
    assert_eq!(gap.device(), Some(&fingerprint));
    Ok(())
}

#[tokio::test]
async fn a_device_that_answers_discovery_but_fails_its_probe_leaves_a_gap() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(10)).await?;
    let closed = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = closed.local_addr()?.port();
    drop(closed);
    let outcome = TwinklyScanner::new(TwinklyScanOptions {
        http_port: port,
        ..TwinklyScanOptions::new(
            Vec::new(),
            vec![fake.discovery_addr()],
            Duration::from_millis(600),
        )
    })
    .scan()
    .await?;
    assert!(outcome.found.is_empty());
    assert!(
        matches!(outcome.gaps.as_slice(), [CoverageGap::ProbeFailed { .. }]),
        "a device that answered is alive, so absence proves nothing: {:?}",
        outcome.gaps
    );
    Ok(())
}

fn rejected_broadcast(endpoints: Vec<TwinklyEndpoint>) -> TwinklyScanner {
    // Sending to port 0 is rejected locally, so this never leaves the host.
    TwinklyScanner::new(TwinklyScanOptions::new(
        endpoints,
        vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 0))],
        Duration::from_millis(300),
    ))
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows accepts a UDP send to port 0, so it cannot fail locally"
)]
async fn a_scan_that_got_no_request_out_is_an_error() {
    let error = rejected_broadcast(Vec::new())
        .scan()
        .await
        .expect_err("a scan that asked nobody could not start");
    assert!(error.to_string().contains("left this host"), "{error}");
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows accepts a UDP send to port 0, so it cannot fail locally"
)]
async fn a_failed_broadcast_is_a_gap_when_known_endpoints_can_still_be_asked() -> TestResult {
    let fake = FakeTwinkly::spawn(FakeSpec::gen2_rgb(10)).await?;
    let outcome = rejected_broadcast(vec![fake.endpoint()]).scan().await?;
    assert_eq!(outcome.found.len(), 1, "the known device still applies");
    assert!(
        matches!(outcome.gaps.as_slice(), [CoverageGap::SourceFailed { .. }]),
        "{:?}",
        outcome.gaps
    );
    Ok(())
}

#[tokio::test]
async fn rgbw_generation_one_device_profiles_correctly() -> TestResult {
    let spec = FakeSpec {
        fw_family: None,
        firmware: "2.3.8".to_owned(),
        led_profile: "RGBW".to_owned(),
        ..FakeSpec::gen2_rgb(105)
    };
    let fake = FakeTwinkly::spawn(spec).await?;
    let profiles = TwinklyScanner::new(unicast(vec![fake.endpoint()]))
        .scan()
        .await?
        .found;
    assert_eq!(profiles[0].led_profile, LedProfile::Rgbw);
    assert_eq!(profiles[0].rt_protocol(), RtProtocol::V1);
    Ok(())
}

#[test]
fn macs_canonicalize() {
    assert_eq!(
        canonical_mac("98:F4:AB:1C:C1:90").as_deref(),
        Some("98:f4:ab:1c:c1:90")
    );
    assert_eq!(
        canonical_mac("98f4ab1cc190").as_deref(),
        Some("98:f4:ab:1c:c1:90")
    );
    assert_eq!(canonical_mac("98:f4:ab"), None);
    assert_eq!(canonical_mac("zz:f4:ab:1c:c1:90"), None);
}

#[test]
fn endpoint_metadata_falls_back_to_standard_ports() -> TestResult {
    let metadata = std::collections::HashMap::from([("ip".to_owned(), "10.0.0.9".to_owned())]);
    let endpoint = endpoint_from_metadata(&metadata)?;
    assert_eq!(
        endpoint.rt_addr(),
        SocketAddr::from((Ipv4Addr::new(10, 0, 0, 9), 7777))
    );
    assert_eq!(endpoint.http_addr().port(), 80);
    assert_eq!(endpoint.base_url(), "http://10.0.0.9:80/xled/v1/");
    assert!(endpoint_from_metadata(&std::collections::HashMap::new()).is_err());
    Ok(())
}
