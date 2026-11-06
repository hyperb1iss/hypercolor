//! Discovery and probing against the loopback fake responder
//! (Spec 88 §4 and §11.4). Broadcast is never used here.

mod support;

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_driver_lifx::protocol::Serial;
use hypercolor_driver_lifx::scanner::{
    LifxScanOptions, LifxScanner, discovered_device, metadata_key, probe_at, route_from_metadata,
};
use hypercolor_driver_lifx::topology::{LifxTopology, Orientation};
use hypercolor_driver_lifx::transport::RequestPolicy;
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
