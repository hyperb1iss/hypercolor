//! Opt-in run against the lifx-emulator project (Spec 88 §12.1).
//!
//! Start the emulator bound to loopback with the fleet in
//! `tests/fixtures/lifx-emulator.yaml`, then set
//! `HYPERCOLOR_LIFX_EMULATOR=127.0.0.1:<port>` (`just lifx-emulator-test`
//! does both). Without the variable the test returns immediately.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use hypercolor_driver_api::DeviceBackend;
use hypercolor_driver_lifx::LifxBackend;
use hypercolor_driver_lifx::color::{Hsbk, HsbkEncoder, clamp_kelvin};
use hypercolor_driver_lifx::config::LifxConfig;
use hypercolor_driver_lifx::encoder::{FrameEncoder, OutputPlan, StreamAddress};
use hypercolor_driver_lifx::protocol::Serial;
use hypercolor_driver_lifx::scanner::{LifxScanOptions, LifxScanner, discovered_device};
use hypercolor_driver_lifx::topology::{LifxDeviceProfile, LifxTopology};
use hypercolor_driver_lifx::transport::RequestPolicy;
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const SETTLE: Duration = Duration::from_millis(80);

fn emulator_addr() -> Option<SocketAddr> {
    let raw = std::env::var("HYPERCOLOR_LIFX_EMULATOR").ok()?;
    let addr: SocketAddr = raw
        .parse()
        .expect("HYPERCOLOR_LIFX_EMULATOR must be ip:port");
    assert!(
        addr.ip().is_loopback(),
        "the emulator must be bound to loopback, got {addr}"
    );
    Some(addr)
}

fn serial(tail: u8) -> Serial {
    Serial([0xd0, 0x73, 0xd5, 0x00, 0x00, tail])
}

/// Read-back client that hand-builds requests the driver never sends.
struct Probe {
    socket: UdpSocket,
    addr: SocketAddr,
    sequence: u8,
}

impl Probe {
    async fn new(addr: SocketAddr) -> TestResult<Self> {
        Ok(Self {
            socket: UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?,
            addr,
            sequence: 0,
        })
    }

    async fn send_raw(&self, datagram: &[u8]) -> TestResult {
        self.socket.send_to(datagram, self.addr).await?;
        Ok(())
    }

    /// Send one request and collect every reply of `reply_type` for 150 ms.
    async fn query(
        &mut self,
        target: Serial,
        message_type: u16,
        payload: &[u8],
        reply_type: u16,
    ) -> TestResult<Vec<Vec<u8>>> {
        self.sequence = self.sequence.wrapping_add(1);
        let size = u16::try_from(36 + payload.len())?;
        let mut packet = Vec::new();
        packet.extend_from_slice(&size.to_le_bytes());
        packet.extend_from_slice(&0x1400_u16.to_le_bytes());
        packet.extend_from_slice(&0x5150_4c43_u32.to_le_bytes());
        packet.extend_from_slice(&target.to_target());
        packet.extend_from_slice(&[0; 6]);
        packet.push(0);
        packet.push(self.sequence);
        packet.extend_from_slice(&[0; 8]);
        packet.extend_from_slice(&message_type.to_le_bytes());
        packet.extend_from_slice(&[0; 2]);
        packet.extend_from_slice(payload);
        self.socket.send_to(&packet, self.addr).await?;

        let deadline = Instant::now() + Duration::from_millis(150);
        let mut replies = Vec::new();
        let mut buf = vec![0_u8; 2048];
        while let Ok(received) = timeout_at(deadline, self.socket.recv_from(&mut buf)).await {
            let (len, _) = received?;
            let reply = &buf[..len];
            if reply.len() >= 36
                && reply[23] == self.sequence
                && u16::from_le_bytes([reply[32], reply[33]]) == reply_type
            {
                replies.push(reply[36..].to_vec());
            }
        }
        Ok(replies)
    }
}

fn hsbk_at(payload: &[u8], offset: usize) -> Hsbk {
    let mut raw = [0_u8; 8];
    raw.copy_from_slice(&payload[offset..offset + 8]);
    Hsbk::from_le_bytes(raw)
}

fn converter(profile: &LifxDeviceProfile) -> HsbkEncoder {
    HsbkEncoder::new(
        clamp_kelvin(6_500, profile.kelvin_range()),
        profile.color_capable(),
    )
}

fn gradient(count: usize) -> Vec<[u8; 3]> {
    (0..count)
        .map(|index| {
            let step = u8::try_from(index * 7 % 256).expect("fits");
            [
                step,
                255 - step,
                u8::try_from(index % 3 * 120).expect("fits"),
            ]
        })
        .collect()
}

/// Read a matrix tile's visible frame buffer back with Get64 (707).
async fn read_tile(
    probe: &mut Probe,
    target: Serial,
    tile: u8,
    width: u8,
    height: u8,
) -> TestResult<Vec<Hsbk>> {
    let rows_per_read = 64 / usize::from(width);
    let mut zones = Vec::new();
    let mut y = 0_usize;
    while y < usize::from(height) {
        let replies = probe
            .query(target, 707, &[tile, 1, 0, 0, u8::try_from(y)?, width], 711)
            .await?;
        let reply = replies.first().ok_or("no State64 reply")?;
        let rows = rows_per_read.min(usize::from(height) - y);
        for zone in 0..rows * usize::from(width) {
            zones.push(hsbk_at(reply, 5 + zone * 8));
        }
        y += rows_per_read;
    }
    Ok(zones)
}

#[tokio::test]
async fn emulated_fleet_discovers_connects_and_renders() -> TestResult {
    let Some(addr) = emulator_addr() else {
        eprintln!("HYPERCOLOR_LIFX_EMULATOR unset; skipping the lifx-emulator run");
        return Ok(());
    };
    let policy = RequestPolicy::default();
    let profiles = LifxScanner::new(LifxScanOptions {
        unicast_targets: vec![addr],
        broadcast_targets: Vec::new(),
        timeout: Duration::from_secs(1),
        request_policy: policy,
        ..LifxScanOptions::default()
    })
    .scan()
    .await?
    .profiles;

    let serials: Vec<Serial> = profiles.iter().map(|profile| profile.serial).collect();
    assert_eq!(
        serials,
        (1..=7).map(serial).collect::<Vec<_>>(),
        "the switch is skipped"
    );
    let profile = |tail: u8| {
        profiles
            .iter()
            .find(|profile| profile.serial == serial(tail))
            .expect("profile")
    };
    assert_eq!(profile(1).topology, LifxTopology::SingleZone);
    assert_eq!(
        profile(2).topology,
        LifxTopology::Multizone {
            zones: 82,
            extended: true,
        }
    );
    assert_eq!(
        profile(3).topology,
        LifxTopology::Multizone {
            zones: 40,
            extended: false,
        }
    );
    assert_eq!(profile(4).topology.led_count(), 192);
    assert_eq!(profile(5).topology.led_count(), 26);
    assert_eq!(profile(6).topology.led_count(), 64);
    assert_eq!(profile(7).topology.led_count(), 128);

    let config = LifxConfig::default();
    let backend = LifxBackend::new(config.clone()).with_request_policy(policy);
    let mut probe = Probe::new(addr).await?;

    for profile in &profiles {
        let discovered = discovered_device(profile, config.max_messages_per_second);
        let device_id = discovered.info.id;
        backend.adopt_device(&discovered)?;
        backend.connect(&device_id).await?;
        let frame = gradient(profile.topology.led_count());
        backend.write_colors(&device_id, &frame).await?;
        tokio::time::sleep(SETTLE).await;
        let expected: Vec<Hsbk> = frame
            .iter()
            .map(|rgb| converter(profile).encode(*rgb))
            .collect();

        match &profile.topology {
            LifxTopology::SingleZone => {
                let state = probe.query(profile.serial, 101, &[], 107).await?;
                let state = state.first().ok_or("no LightState")?;
                assert_eq!(hsbk_at(state, 0), expected[0], "bulb color");
                assert_eq!(
                    u16::from_le_bytes([state[10], state[11]]),
                    65_535,
                    "powered on"
                );
            }
            LifxTopology::Multizone { extended: true, .. } => {
                let replies = probe.query(profile.serial, 511, &[], 512).await?;
                let reply = replies.first().ok_or("no StateExtendedColorZones")?;
                let count = usize::from(reply[4]);
                let colors: Vec<Hsbk> = (0..count)
                    .map(|zone| hsbk_at(reply, 5 + zone * 8))
                    .collect();
                assert_eq!(colors, expected, "extended strip zones");
            }
            LifxTopology::Multizone {
                extended: false,
                zones,
            } => {
                let replies = probe.query(profile.serial, 502, &[0, 255], 506).await?;
                let mut colors = vec![Hsbk::default(); usize::from(*zones)];
                for reply in &replies {
                    let start = usize::from(reply[1]);
                    for offset in 0..8 {
                        if let Some(slot) = colors.get_mut(start + offset) {
                            *slot = hsbk_at(reply, 2 + offset * 8);
                        }
                    }
                }
                assert_eq!(colors, expected, "legacy strip zones");
            }
            LifxTopology::Matrix { tiles, .. } => {
                let layout = profile.topology.matrix_layout().expect("matrix layout");
                let mut frame_buffers = Vec::new();
                for (index, tile) in tiles.iter().enumerate() {
                    frame_buffers.push(
                        read_tile(
                            &mut probe,
                            profile.serial,
                            u8::try_from(index)?,
                            tile.width,
                            tile.height,
                        )
                        .await?,
                    );
                }
                for (led, slot) in layout.slots.iter().enumerate() {
                    assert_eq!(
                        frame_buffers[usize::from(slot.tile)][usize::from(slot.zone)],
                        expected[led],
                        "{} LED {led}",
                        profile.label
                    );
                }
            }
        }
        backend.disconnect(&device_id).await?;
    }

    // The emulator reports no spare frame buffers, so the backend writes the
    // 16 x 8 Ceiling's visible buffer directly. Drive the hidden-buffer path
    // by hand to check the Set64 plus CopyFrameBuffer sequence.
    let mut ceiling = profile(7).clone();
    let LifxTopology::Matrix { tiles, .. } = &mut ceiling.topology else {
        panic!("ceiling is a matrix");
    };
    tiles[0].supported_frame_buffers = 2;
    let plan = OutputPlan::from_topology(&ceiling.topology);
    assert_eq!(plan.worst_case_messages(), 3);
    let mut encoder = FrameEncoder::new(plan);
    let mut address = StreamAddress {
        source: 0x0bad_cafe,
        target: ceiling.serial,
        sequence: 0,
    };
    let frame: Vec<[u8; 3]> = gradient(128).into_iter().rev().collect();
    encoder.encode(&frame, &converter(&ceiling), 0, &mut address)?;
    let datagrams: Vec<Vec<u8>> = encoder.datagrams().map(<[u8]>::to_vec).collect();
    assert_eq!(datagrams.len(), 3);
    probe.send_raw(&datagrams[0]).await?;
    probe.send_raw(&datagrams[1]).await?;
    tokio::time::sleep(SETTLE).await;
    let before_copy = read_tile(&mut probe, ceiling.serial, 0, 16, 8).await?;
    probe.send_raw(&datagrams[2]).await?;
    tokio::time::sleep(SETTLE).await;
    let after_copy = read_tile(&mut probe, ceiling.serial, 0, 16, 8).await?;
    let layout = ceiling.topology.matrix_layout().expect("matrix layout");
    let mut expected = vec![Hsbk::default(); 128];
    for (led, slot) in layout.slots.iter().enumerate() {
        expected[usize::from(slot.zone)] = converter(&ceiling).encode(frame[led]);
    }
    assert_ne!(before_copy, expected, "hidden buffer writes stay hidden");
    assert_eq!(
        after_copy, expected,
        "CopyFrameBuffer shows the hidden frame"
    );
    Ok(())
}
