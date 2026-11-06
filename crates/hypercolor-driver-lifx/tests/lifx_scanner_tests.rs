//! Discovery and probing against the loopback fake responder
//! (Spec 88 §4 and §11.4). Broadcast is never used here.

mod support;

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_driver_lifx::protocol::Serial;
use hypercolor_driver_lifx::scanner::{
    LifxScanOptions, LifxScanner, MAX_CONCURRENT_PROBES, MAX_DISCOVERY_CANDIDATES,
    discovered_device, ensure_round_covered_its_targets, metadata_key, probe_at,
    route_from_metadata,
};
use hypercolor_driver_lifx::topology::{LifxTopology, Orientation};
use hypercolor_driver_lifx::transport::{DiscoveryRound, LifxClient, RequestPolicy};
use hypercolor_types::portable::PortableIdentityClaim;
use support::{FakeDevice, FakeKind, FakeLifx, FakeTile, TestResult};

fn options(targets: Vec<SocketAddr>) -> LifxScanOptions {
    LifxScanOptions {
        unicast_targets: targets,
        broadcast_targets: Vec::new(),
        timeout: Duration::from_millis(600),
        request_policy: RequestPolicy {
            reply_timeout: Duration::from_millis(200),
            attempts: 2,
        },
        ..LifxScanOptions::default()
    }
}

fn serial(tail: u8) -> Serial {
    Serial([0xd0, 0x73, 0xd5, 0x00, 0x00, tail])
}

fn fleet() -> Vec<FakeDevice> {
    let mut chain_tiles = vec![FakeTile::upright(8, 8); 3];
    chain_tiles[1].user_x = 1.0;
    chain_tiles[2].user_x = 2.0;
    chain_tiles[2].accel = (100, 0, 0);
    let mut ceiling = FakeTile::upright(16, 8);
    ceiling.frame_buffers = 2;
    let mut foreign = FakeDevice::new(9, 27, (3, 70), FakeKind::Bulb);
    foreign.vendor = 2;
    vec![
        FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb),
        FakeDevice::new(2, 31, (2, 60), FakeKind::LegacyStrip { zones: 16 }),
        FakeDevice::new(3, 38, (2, 77), FakeKind::ExtendedStrip { zones: 120 }),
        FakeDevice::new(4, 38, (2, 60), FakeKind::ExtendedStrip { zones: 80 }),
        FakeDevice::new(5, 55, (3, 50), FakeKind::Matrix { tiles: chain_tiles }),
        FakeDevice::new(
            6,
            201,
            (4, 10),
            FakeKind::Matrix {
                tiles: vec![ceiling],
            },
        ),
        FakeDevice::new(7, 70, (3, 70), FakeKind::Bulb),
        foreign,
    ]
}

#[tokio::test]
async fn scan_finds_every_light_behind_one_port() -> TestResult {
    let fake = FakeLifx::spawn(fleet()).await?;
    let profiles = LifxScanner::new(options(vec![fake.addr()]))
        .scan_profiles()
        .await?;

    let serials: Vec<Serial> = profiles.iter().map(|profile| profile.serial).collect();
    assert_eq!(
        serials,
        vec![
            serial(1),
            serial(2),
            serial(3),
            serial(4),
            serial(5),
            serial(6)
        ],
        "switches and foreign vendors are skipped"
    );
    for profile in &profiles {
        assert_eq!(profile.addr, fake.addr());
    }

    assert_eq!(profiles[0].topology, LifxTopology::SingleZone);
    assert_eq!(profiles[0].label, "Fake 1");
    assert_eq!(profiles[0].firmware.to_string(), "3.70");
    assert_eq!(
        profiles[1].topology,
        LifxTopology::Multizone {
            zones: 16,
            extended: false,
        }
    );
    assert_eq!(
        profiles[2].topology,
        LifxTopology::Multizone {
            zones: 120,
            extended: true,
        }
    );
    assert_eq!(
        profiles[3].topology,
        LifxTopology::Multizone {
            zones: 80,
            extended: false,
        },
        "a Beam before firmware 2.77 falls back to legacy zones"
    );

    let LifxTopology::Matrix { tiles, .. } = &profiles[4].topology else {
        panic!("tile chain should be a matrix");
    };
    assert_eq!(tiles.len(), 3);
    assert_eq!(tiles[1].user_x, 1.0);
    assert_eq!(tiles[2].orientation, Orientation::RotatedRight);
    assert_eq!(profiles[4].topology.led_count(), 192);

    let LifxTopology::Matrix { tiles, uplight, .. } = &profiles[5].topology else {
        panic!("ceiling should be a matrix");
    };
    assert_eq!((tiles[0].width, tiles[0].height), (16, 8));
    assert_eq!(tiles[0].supported_frame_buffers, 2);
    assert_eq!(*uplight, Some((15, 7)));

    let discovery = fake.received_of_type(2);
    assert_eq!(discovery.len(), 1);
    assert!(discovery[0].tagged);
    assert_eq!(discovery[0].target, [0; 6]);
    assert_eq!(
        fake.received_of_type(511).len(),
        1,
        "only the extended-capable strip is asked for extended zones"
    );
    Ok(())
}

#[tokio::test]
async fn scan_merges_several_targets() -> TestResult {
    let first = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let second = FakeLifx::spawn(vec![FakeDevice::new(
        2,
        141,
        (3, 90),
        FakeKind::ExtendedStrip { zones: 48 },
    )])
    .await?;
    let profiles = LifxScanner::new(options(vec![first.addr(), second.addr(), first.addr()]))
        .scan_profiles()
        .await?;
    assert_eq!(profiles.len(), 2);
    assert_eq!(profiles[0].addr, first.addr());
    assert_eq!(profiles[1].addr, second.addr());
    Ok(())
}

#[tokio::test]
async fn discovered_devices_carry_routing_metadata() -> TestResult {
    let fake = FakeLifx::spawn(fleet()).await?;
    let profiles = LifxScanner::new(options(vec![fake.addr()]))
        .scan_profiles()
        .await?;
    let chain = profiles
        .iter()
        .find(|profile| profile.serial == serial(5))
        .expect("tile chain");

    let discovered = discovered_device(chain, 20);
    assert_eq!(discovered.info.id, chain.device_id());
    assert_eq!(discovered.info.total_led_count(), 192);
    assert_eq!(discovered.info.capabilities.max_fps, 6);
    assert_eq!(discovered.metadata[metadata_key::SERIAL], "d073d5000005");
    assert_eq!(discovered.metadata[metadata_key::PRODUCT_ID], "55");
    assert_eq!(discovered.metadata[metadata_key::DEVICE_CLASS], "matrix");
    assert_eq!(discovered.metadata[metadata_key::TILES], "3");
    assert_eq!(discovered.metadata[metadata_key::ZONES], "192");
    assert_eq!(
        route_from_metadata(&discovered.metadata)?,
        (fake.addr(), serial(5))
    );
    assert!(matches!(
        discovered.claim,
        Some(PortableIdentityClaim::MacAddress { .. })
    ));
    assert_eq!(
        discovered.fingerprint,
        hypercolor_driver_lifx::topology::fingerprint_for_serial(serial(5))
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows accepts a UDP send to port 0, so it cannot fail locally"
)]
async fn probing_a_silent_address_times_out() -> TestResult {
    let silent = tokio::net::UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let error = probe_at(
        silent.local_addr()?,
        serial(1),
        RequestPolicy {
            reply_timeout: Duration::from_millis(30),
            attempts: 2,
        },
    )
    .await
    .expect_err("nobody answers");
    assert!(
        error
            .to_string()
            .contains("did not answer message 32 after 2 attempts")
    );
    Ok(())
}

#[tokio::test]
async fn probing_a_wrong_serial_gets_no_answer() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    let result = probe_at(
        fake.addr(),
        serial(42),
        RequestPolicy {
            reply_timeout: Duration::from_millis(30),
            attempts: 1,
        },
    )
    .await;
    assert!(result.is_err());
    Ok(())
}

fn addr(last: u8) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::new(192, 0, 2, last), 56_700))
}

#[test]
fn rounds_fail_when_any_light_could_not_be_asked() {
    let round = |failed: Vec<SocketAddr>, dropped| DiscoveryRound {
        sends_attempted: 4,
        sends_failed: failed.len(),
        failed_targets: failed,
        dropped,
        ..DiscoveryRound::default()
    };
    let broadcast = [addr(255), addr(254)];

    assert!(ensure_round_covered_its_targets("IPv4", &round(Vec::new(), 0), &broadcast).is_ok());
    assert!(
        ensure_round_covered_its_targets("IPv4", &round(vec![addr(10)], 0), &broadcast).is_err(),
        "a tracked light that was never asked must not be retired"
    );
    assert!(
        ensure_round_covered_its_targets("IPv4", &round(vec![addr(255)], 0), &broadcast).is_ok(),
        "one failed broadcast interface is covered by unicast probes"
    );
    assert!(
        ensure_round_covered_its_targets("IPv4", &round(vec![addr(255), addr(254)], 0), &broadcast)
            .is_err(),
        "no local network was asked"
    );
    assert!(
        ensure_round_covered_its_targets("IPv4", &round(Vec::new(), 3), &broadcast).is_err(),
        "lights past the candidate limit were never probed"
    );
    assert!(
        ensure_round_covered_its_targets("IPv6", &DiscoveryRound::default(), &[]).is_ok(),
        "an unused family is not a failure"
    );
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows accepts a UDP send to port 0, so it cannot fail locally"
)]
async fn one_failed_unicast_send_fails_the_scan_even_when_others_succeed() -> TestResult {
    let fake = FakeLifx::spawn(vec![FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb)]).await?;
    // Port 0 is rejected locally, so this send never leaves the host.
    let rejected = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let error = LifxScanner::new(options(vec![fake.addr(), rejected]))
        .scan()
        .await
        .expect_err("a scan that skipped a tracked light must not look complete");
    assert!(error.to_string().contains("scan is incomplete"), "{error}");
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "Windows accepts a UDP send to port 0, so it cannot fail locally"
)]
async fn a_scan_that_cannot_reach_the_network_fails_instead_of_finding_nothing() -> TestResult {
    // Sending to port 0 is rejected locally, so this never leaves the host.
    let unreachable = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let error = LifxScanner::new(options(vec![unreachable]))
        .scan()
        .await
        .expect_err("a scan whose only send failed must not look empty");
    assert!(error.to_string().contains("scan is incomplete"), "{error}");
    Ok(())
}

#[tokio::test]
async fn unused_address_families_still_scan_successfully() -> TestResult {
    let outcome = LifxScanner::new(options(Vec::new())).scan().await?;
    assert!(outcome.profiles.is_empty());
    assert!(outcome.unprobed.is_empty());
    Ok(())
}

#[tokio::test]
async fn lights_that_answer_but_fail_their_probe_are_reported_as_unprobed() -> TestResult {
    let mut silent = FakeDevice::new(2, 27, (3, 70), FakeKind::Bulb);
    silent.silent_after_discovery = true;
    let fake = FakeLifx::spawn(vec![
        FakeDevice::new(1, 27, (3, 70), FakeKind::Bulb),
        silent,
    ])
    .await?;
    let outcome = LifxScanner::new(options(vec![fake.addr()])).scan().await?;
    assert_eq!(outcome.profiles.len(), 1);
    assert_eq!(outcome.profiles[0].serial, serial(1));
    assert_eq!(outcome.unprobed.len(), 1);
    assert_eq!(outcome.unprobed[0].serial, serial(2));
    assert_eq!(outcome.unprobed[0].addr, fake.addr());
    Ok(())
}

#[tokio::test]
async fn discovery_rounds_keep_at_most_their_cap() -> TestResult {
    let fleet = (1..=5)
        .map(|tail| FakeDevice::new(tail, 27, (3, 70), FakeKind::Bulb))
        .collect();
    let fake = FakeLifx::spawn(fleet).await?;
    let mut client = LifxClient::bind_for(&fake.addr()).await?;
    let round = client
        .discover(&[fake.addr()], Duration::from_millis(300), 3)
        .await?;
    assert_eq!(round.replies.len(), 3);
    assert_eq!(round.dropped, 2);
    assert_eq!((round.sends_attempted, round.sends_failed), (1, 0));
    Ok(())
}

fn bulb_fleet(count: usize) -> Vec<FakeDevice> {
    (0..count)
        .map(|index| {
            let mut device = FakeDevice::new(0, 27, (3, 70), FakeKind::Bulb);
            let index = u16::try_from(index).expect("fits");
            device.serial = [
                0xd0,
                0x73,
                0xd5,
                0x01,
                (index >> 8) as u8,
                (index & 0xff) as u8,
            ];
            device
        })
        .collect()
}

#[test]
fn default_limits_leave_headroom_for_large_homes() {
    let defaults = LifxScanOptions::default();
    assert_eq!(defaults.max_candidates, MAX_DISCOVERY_CANDIDATES);
    assert_eq!(MAX_DISCOVERY_CANDIDATES, 512);
    assert_eq!(defaults.max_concurrent_probes, MAX_CONCURRENT_PROBES);
    assert_eq!(MAX_CONCURRENT_PROBES, 32);
}

// The limit tests use small limits so they exercise the same code without
// depending on a loaded machine draining hundreds of replies in time.

#[tokio::test]
async fn a_reply_flood_past_the_cap_fails_the_scan_without_probing() -> TestResult {
    let fake = FakeLifx::spawn(bulb_fleet(12)).await?;
    let error = LifxScanner::new(LifxScanOptions {
        max_candidates: 8,
        ..options(vec![fake.addr()])
    })
    .scan()
    .await
    .expect_err("a truncated scan must not look complete");
    assert!(error.to_string().contains("scan is incomplete"), "{error}");
    assert!(
        fake.received()
            .iter()
            .all(|message| message.message_type == 2),
        "nothing past discovery ran"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn probes_never_exceed_the_concurrency_limit() -> TestResult {
    let fake = FakeLifx::spawn(bulb_fleet(24)).await?;
    let outcome = LifxScanner::new(LifxScanOptions {
        max_candidates: 24,
        max_concurrent_probes: 4,
        ..options(vec![fake.addr()])
    })
    .scan()
    .await?;
    assert_eq!(outcome.profiles.len() + outcome.unprobed.len(), 24);

    // Each probe owns one socket and one serial from its first request to
    // its last, so overlapping probe lifetimes measure the concurrency. The
    // key includes the serial because a closed probe's port can be reused.
    let mut lifetimes: HashMap<(SocketAddr, [u8; 6]), (std::time::Instant, std::time::Instant)> =
        HashMap::new();
    for message in fake.received() {
        if message.message_type == 2 {
            continue;
        }
        let entry = lifetimes
            .entry((message.from, message.target))
            .or_insert((message.arrived, message.arrived));
        entry.1 = message.arrived;
    }
    assert_eq!(lifetimes.len(), 24);
    let mut edges: Vec<(std::time::Instant, i32)> = lifetimes
        .values()
        .flat_map(|(start, end)| [(*start, 1), (*end, -1)])
        .collect();
    // At equal instants an end sorts before a start, so touching lifetimes
    // do not count as overlapping.
    edges.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut live = 0;
    let mut peak = 0;
    for (_, delta) in edges {
        live += delta;
        peak = peak.max(live);
    }
    assert!(peak <= 4, "{peak} probes overlapped with a limit of 4");
    Ok(())
}
