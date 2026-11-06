//! In-repo fake LIFX responder bound to 127.0.0.1.
//!
//! Several fake devices share one UDP socket and are routed by header
//! target, the way the lifx-emulator project hosts a fleet. Replies are
//! built by hand here rather than with the crate's encoder, so the tests
//! check the driver against an independent byte layout.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A tile in a fake device chain.
#[derive(Debug, Clone, Copy)]
pub struct FakeTile {
    pub width: u8,
    pub height: u8,
    pub user_x: f32,
    pub user_y: f32,
    pub accel: (i16, i16, i16),
    pub frame_buffers: u8,
}

impl FakeTile {
    pub const fn upright(width: u8, height: u8) -> Self {
        Self {
            width,
            height,
            user_x: 0.0,
            user_y: 0.0,
            accel: (0, -100, 0),
            frame_buffers: 1,
        }
    }
}

/// What kind of zones a fake device reports.
#[derive(Debug, Clone)]
pub enum FakeKind {
    Bulb,
    LegacyStrip { zones: u8 },
    ExtendedStrip { zones: u16 },
    Matrix { tiles: Vec<FakeTile> },
}

/// One fake device.
#[derive(Debug, Clone)]
pub struct FakeDevice {
    pub serial: [u8; 6],
    pub vendor: u32,
    pub product: u32,
    pub firmware: (u16, u16),
    pub label: String,
    pub kind: FakeKind,
    /// Answer discovery but ignore every probe, like a light that drops
    /// off Wi-Fi mid-scan.
    pub silent_after_discovery: bool,
}

impl FakeDevice {
    pub fn new(serial_tail: u8, product: u32, firmware: (u16, u16), kind: FakeKind) -> Self {
        Self {
            serial: [0xd0, 0x73, 0xd5, 0x00, 0x00, serial_tail],
            vendor: 1,
            product,
            firmware,
            label: format!("Fake {serial_tail}"),
            kind,
            silent_after_discovery: false,
        }
    }
}

/// A datagram the fake received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub target: [u8; 6],
    pub tagged: bool,
    pub ack_required: bool,
    pub res_required: bool,
    pub sequence: u8,
    pub source: u32,
    pub message_type: u16,
    pub payload: Vec<u8>,
    pub arrived: Instant,
    pub from: SocketAddr,
}

/// Running fake responder.
pub struct FakeLifx {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
    task: JoinHandle<()>,
}

impl FakeLifx {
    pub async fn spawn(devices: Vec<FakeDevice>) -> TestResult<Self> {
        Self::spawn_on(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), devices).await
    }

    /// Bind a specific loopback address, for example to come back on the
    /// port of a fake that was dropped.
    pub async fn spawn_on(bind: SocketAddr, devices: Vec<FakeDevice>) -> TestResult<Self> {
        let socket = UdpSocket::bind(bind).await?;
        let addr = socket.local_addr()?;
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        let task = tokio::spawn(async move {
            let mut buf = vec![0_u8; 2048];
            loop {
                let Ok((len, from)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                let Some(message) = parse(&buf[..len], from) else {
                    continue;
                };
                log.lock().expect("fake log lock").push(message.clone());
                for device in &devices {
                    if !message.tagged && message.target != device.serial {
                        continue;
                    }
                    if message.ack_required {
                        let ack = reply(&message, device.serial, 45, &[]);
                        let _ = socket.send_to(&ack, from).await;
                    }
                    if let Some((message_type, payload)) = respond(device, &message, addr.port()) {
                        let bytes = reply(&message, device.serial, message_type, &payload);
                        let _ = socket.send_to(&bytes, from).await;
                    }
                }
            }
        });
        Ok(Self {
            addr,
            received,
            task,
        })
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn received(&self) -> Vec<Received> {
        self.received.lock().expect("fake log lock").clone()
    }

    pub fn received_of_type(&self, message_type: u16) -> Vec<Received> {
        self.received()
            .into_iter()
            .filter(|message| message.message_type == message_type)
            .collect()
    }

    /// Wait until at least `count` messages of `message_type` arrived.
    pub async fn wait_for(&self, message_type: u16, count: usize) -> Vec<Received> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let messages = self.received_of_type(message_type);
            if messages.len() >= count || tokio::time::Instant::now() >= deadline {
                return messages;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub fn clear(&self) {
        self.received.lock().expect("fake log lock").clear();
    }
}

impl Drop for FakeLifx {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn parse(bytes: &[u8], from: SocketAddr) -> Option<Received> {
    if bytes.len() < 36 {
        return None;
    }
    let size = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
    if size != bytes.len() {
        return None;
    }
    let flags = u16::from_le_bytes([bytes[2], bytes[3]]);
    if flags & 0x0FFF != 1024 || flags & 0x1000 == 0 {
        return None;
    }
    let mut target = [0_u8; 6];
    target.copy_from_slice(&bytes[8..14]);
    Some(Received {
        target,
        tagged: flags & 0x2000 != 0,
        ack_required: bytes[22] & 0x02 != 0,
        res_required: bytes[22] & 0x01 != 0,
        sequence: bytes[23],
        source: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        message_type: u16::from_le_bytes([bytes[32], bytes[33]]),
        payload: bytes[36..].to_vec(),
        arrived: Instant::now(),
        from,
    })
}

fn reply(request: &Received, serial: [u8; 6], message_type: u16, payload: &[u8]) -> Vec<u8> {
    let size = u16::try_from(36 + payload.len()).expect("reply fits");
    let mut bytes = Vec::with_capacity(36 + payload.len());
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&0x1400_u16.to_le_bytes());
    bytes.extend_from_slice(&request.source.to_le_bytes());
    bytes.extend_from_slice(&serial);
    bytes.extend_from_slice(&[0, 0]);
    // Real devices write "LIFXV2" into the reserved frame address bytes.
    bytes.extend_from_slice(b"LIFXV2");
    bytes.push(0);
    bytes.push(request.sequence);
    bytes.extend_from_slice(&[0xAA; 8]);
    bytes.extend_from_slice(&message_type.to_le_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(payload);
    bytes
}

fn respond(device: &FakeDevice, request: &Received, port: u16) -> Option<(u16, Vec<u8>)> {
    if device.silent_after_discovery && request.message_type != 2 {
        return None;
    }
    match request.message_type {
        2 => {
            let mut payload = vec![1];
            payload.extend_from_slice(&u32::from(port).to_le_bytes());
            Some((3, payload))
        }
        14 => {
            let mut payload = vec![0; 16];
            payload.extend_from_slice(&device.firmware.1.to_le_bytes());
            payload.extend_from_slice(&device.firmware.0.to_le_bytes());
            Some((15, payload))
        }
        23 => {
            let mut payload = device.label.as_bytes().to_vec();
            payload.resize(32, 0);
            Some((25, payload))
        }
        32 => {
            let mut payload = device.vendor.to_le_bytes().to_vec();
            payload.extend_from_slice(&device.product.to_le_bytes());
            payload.extend_from_slice(&[0; 4]);
            Some((33, payload))
        }
        502 => match device.kind {
            FakeKind::LegacyStrip { zones } => {
                let mut payload = vec![zones, 0];
                payload.resize(66, 0);
                Some((506, payload))
            }
            FakeKind::ExtendedStrip { zones } => {
                let mut payload = vec![u8::try_from(zones).unwrap_or(u8::MAX), 0];
                payload.resize(66, 0);
                Some((506, payload))
            }
            _ => None,
        },
        511 => match device.kind {
            FakeKind::ExtendedStrip { zones } => {
                let mut payload = zones.to_le_bytes().to_vec();
                payload.extend_from_slice(&0_u16.to_le_bytes());
                payload.push(u8::try_from(zones.min(82)).unwrap_or(82));
                payload.resize(661, 0);
                Some((512, payload))
            }
            _ => Some((223, 511_u16.to_le_bytes().to_vec())),
        },
        701 => match &device.kind {
            FakeKind::Matrix { tiles } => Some((702, device_chain(device, tiles))),
            _ => None,
        },
        _ => None,
    }
}

fn device_chain(device: &FakeDevice, tiles: &[FakeTile]) -> Vec<u8> {
    let mut payload = vec![0];
    for slot in 0..16 {
        let mut record = vec![0_u8; 55];
        if let Some(tile) = tiles.get(slot) {
            record[0..2].copy_from_slice(&tile.accel.0.to_le_bytes());
            record[2..4].copy_from_slice(&tile.accel.1.to_le_bytes());
            record[4..6].copy_from_slice(&tile.accel.2.to_le_bytes());
            record[8..12].copy_from_slice(&tile.user_x.to_le_bytes());
            record[12..16].copy_from_slice(&tile.user_y.to_le_bytes());
            record[16] = tile.width;
            record[17] = tile.height;
            record[18] = tile.frame_buffers;
            record[19..23].copy_from_slice(&device.vendor.to_le_bytes());
            record[23..27].copy_from_slice(&device.product.to_le_bytes());
            record[47..49].copy_from_slice(&device.firmware.1.to_le_bytes());
            record[49..51].copy_from_slice(&device.firmware.0.to_le_bytes());
        }
        payload.extend_from_slice(&record);
    }
    payload.push(u8::try_from(tiles.len()).expect("tile count fits"));
    payload
}

/// Assert that datagrams arrived no faster than a token bucket of `burst`
/// messages refilled every `interval` could have sent them, starting full
/// at `started` (taken before the first send). Every datagram arrives after
/// it was sent, so receiver scheduling delays only loosen this bound; it
/// fails only when the sender really outran the budget.
pub fn assert_budget_respected(
    messages: &[Received],
    started: Instant,
    burst: usize,
    interval: Duration,
) {
    for (index, message) in messages.iter().enumerate() {
        let elapsed = message.arrived.saturating_duration_since(started);
        let refilled =
            usize::try_from(elapsed.as_nanos() / interval.as_nanos()).unwrap_or(usize::MAX);
        let allowed = burst.saturating_add(refilled);
        assert!(
            index < allowed,
            "{} datagrams had arrived {elapsed:?} after the first send; the budget allows {allowed}",
            index + 1
        );
    }
}

/// Decode a hex string, ignoring whitespace.
pub fn hex(raw: &str) -> Vec<u8> {
    let digits: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
    (0..digits.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&digits[index..index + 2], 16).expect("valid hex"))
        .collect()
}
