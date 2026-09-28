//! In-process L-Wireless stand-in for RGB pacing tests.
//!
//! `FakeRadio` models the whole controller as the protocol sees it:
//!
//! - the TX takes 64-byte packets, rebuilds each 240-byte envelope from its
//!   four slices, and queues it for the air. The air carries one envelope
//!   every `air_time`, so the queue is the radio work the host has handed
//!   the TX and the TX has not sent yet;
//! - a fan receiver takes an RGB transfer once its header and every data
//!   envelope have arrived, and from then on echoes its tag;
//! - each receiver reports to the RX every `report_interval` (its clock
//!   advances then), and the RX answers a table poll with the latest
//!   reports.
//!
//! `Rig` plays the USB actor on a simulated clock: the render path publishes
//! a frame every `frame_period` into a latest-value slot, the keepalive
//! ticks at the protocol's interval, commands run in order with their
//! post-delays, and table replies go back through `parse_response`. Nothing
//! here touches USB.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the rig"
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_hal::drivers::lianli::WirelessControllerProtocol;
use hypercolor_hal::drivers::lianli::wireless::discovery::{
    GET_DEV_PAGE_LEN, RECORD_LEN, RECORD_VALIDATION,
};
use hypercolor_hal::drivers::lianli::wireless::frame::{
    RF_BROADCAST_SLOT, RF_ENVELOPE_LEN, RF_SELECT, RF_SLICE_LEN, RfSubCommand, USB_CMD_GET_MAC,
    USB_CMD_RESET_PARTNER, USB_CMD_SEND_RF, USB_RF_HEADER_LEN,
};
use hypercolor_hal::drivers::lianli::wireless::pacing::DeliveryStats;
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseTolerance, TransferType};

pub const MASTER_MAC: [u8; 6] = [0xA0, 0x71, 0xAE, 0x72, 0xAB, 0x3C];
pub const CLUSTER_MAC: [u8; 6] = [0x11; 6];
/// Three TL fans: 78 LEDs, one data envelope per frame.
pub const LEDS: usize = 78;
pub const FRAME_PERIOD: Duration = Duration::from_millis(33);
/// Time the host spends handing one packet to either dongle.
const USB_PACKET_TIME: Duration = Duration::from_micros(100);
/// Time the RX takes to answer, per 64-byte packet of its reply.
const RX_PACKET_TIME: Duration = Duration::from_micros(500);

/// Manually advanced clock shared with the protocol under test.
#[derive(Clone)]
pub struct SimClock {
    origin: Instant,
    now: Arc<Mutex<Instant>>,
}

impl SimClock {
    #[must_use]
    pub fn new() -> Self {
        let origin = Instant::now();
        Self {
            origin,
            now: Arc::new(Mutex::new(origin)),
        }
    }

    #[must_use]
    pub fn now(&self) -> Instant {
        *self.now.lock().expect("sim clock lock")
    }

    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.now().saturating_duration_since(self.origin)
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().expect("sim clock lock") += by;
    }
}

impl Default for SimClock {
    fn default() -> Self {
        Self::new()
    }
}

/// A protocol reading time from `clock`.
#[must_use]
pub fn protocol_on(clock: &SimClock) -> WirelessControllerProtocol {
    let clock = clock.clone();
    WirelessControllerProtocol::with_clock(Arc::new(move || clock.now()))
}

/// One fan cluster on the air.
#[derive(Debug, Clone)]
pub struct FakeCluster {
    pub mac: [u8; 6],
    pub rx_type: u8,
    /// Tag of the transfer the receiver last took.
    pub applied: [u8; 4],
    /// Every tag it took, in order.
    pub applied_log: Vec<[u8; 4]>,
    /// What the RX last heard from it: echoed tag and clock.
    pub reported: [u8; 4],
    pub clock: u32,
    next_report_at: Duration,
    /// Tags taken, with when the RX can first report each.
    reportable: VecDeque<(Duration, [u8; 4])>,
    /// Transfer being assembled: tag, envelopes expected, data seen.
    assembling: Option<([u8; 4], u8, Vec<bool>)>,
}

impl FakeCluster {
    fn new(mac: [u8; 6], rx_type: u8) -> Self {
        Self {
            mac,
            rx_type,
            applied: [0xF0, 0, 0, 0x01],
            applied_log: Vec::new(),
            reported: [0xF0, 0, 0, 0x01],
            clock: 1,
            next_report_at: Duration::ZERO,
            reportable: VecDeque::new(),
            assembling: None,
        }
    }

    fn report(&mut self, now: Duration) {
        while let Some((visible_at, tag)) = self.reportable.front().copied() {
            if visible_at > now {
                break;
            }
            self.reported = tag;
            self.reportable.pop_front();
        }
        self.clock = self.clock.wrapping_add(1);
    }

    fn take_rgb(&mut self, envelope: &[u8; RF_ENVELOPE_LEN], now: Duration, delay: Duration) {
        let mut tag = [0_u8; 4];
        tag.copy_from_slice(&envelope[14..18]);
        let index = envelope[18];
        let total = envelope[19];
        if index == 0 {
            if self
                .assembling
                .as_ref()
                .is_some_and(|(current, _, _)| *current == tag)
            {
                return;
            }
            self.assembling = Some((tag, total, vec![false; usize::from(total)]));
            return;
        }
        let Some((current, expected, seen)) = self.assembling.as_mut() else {
            return;
        };
        if *current != tag || index >= *expected {
            return;
        }
        seen[usize::from(index)] = true;
        if seen.iter().skip(1).all(|chunk| *chunk) {
            self.applied = tag;
            self.applied_log.push(tag);
            self.reportable.push_back((now + delay, tag));
            self.assembling = None;
        }
    }
}

/// An envelope the TX has queued for the air.
#[derive(Debug, Clone)]
struct AirEnvelope {
    bytes: [u8; RF_ENVELOPE_LEN],
    rx_type: u8,
}

/// The TX, the air, the fan receivers, and the RX.
pub struct FakeRadio {
    pub clusters: Vec<FakeCluster>,
    /// Time one envelope occupies the air.
    pub air_time: Duration,
    /// How often each receiver reports to the RX.
    pub report_interval: Duration,
    /// How long after a receiver takes a transfer its reports carry it.
    pub report_delay: Duration,
    /// The TX takes packets but puts nothing on the air.
    pub rf_dead: bool,
    /// The RX stops hearing the fans: they drop out of its table.
    pub fans_silent: bool,
    /// Drop RGB envelopes whose running number this returns true for.
    pub drop_rgb_envelope: Option<Box<dyn FnMut(u64) -> bool>>,
    air: VecDeque<AirEnvelope>,
    /// When the air finished its last envelope, or went idle.
    air_free_at: Duration,
    slices: Vec<Option<[u8; RF_ENVELOPE_LEN]>>,
    slice_filled: [bool; 4],
    /// Tag of the header envelope just queued, to count a repeated header
    /// as one transfer.
    last_header: Option<[u8; 4]>,
    rgb_envelopes: u64,
    pub max_air_queue: usize,
    pub tx_packets: u64,
    pub tx_rgb_headers: Vec<(Duration, [u8; 4])>,
    pub rx_polls: Vec<(Duration, u8)>,
    pub resets: Vec<Duration>,
}

impl FakeRadio {
    /// One three-fan TL cluster bound to this controller.
    #[must_use]
    pub fn one_cluster() -> Self {
        Self::with_clusters(vec![FakeCluster::new(CLUSTER_MAC, 3)])
    }

    #[must_use]
    pub fn with_clusters(clusters: Vec<FakeCluster>) -> Self {
        Self {
            clusters,
            air_time: Duration::from_millis(2),
            report_interval: Duration::from_millis(10),
            report_delay: Duration::ZERO,
            rf_dead: false,
            fans_silent: false,
            drop_rgb_envelope: None,
            air: VecDeque::new(),
            air_free_at: Duration::ZERO,
            slices: vec![None],
            slice_filled: [false; 4],
            last_header: None,
            rgb_envelopes: 0,
            max_air_queue: 0,
            tx_packets: 0,
            tx_rgb_headers: Vec::new(),
            rx_polls: Vec::new(),
            resets: Vec::new(),
        }
    }

    /// Two clusters, the second two fans on its own receiver slot.
    #[must_use]
    pub fn two_clusters() -> Self {
        Self::with_clusters(vec![
            FakeCluster::new(CLUSTER_MAC, 3),
            FakeCluster::new([0x22; 6], 4),
        ])
    }

    #[must_use]
    pub fn air_queue(&self) -> usize {
        self.air.len()
    }

    /// Run the air and the receivers' reports up to `now`.
    pub fn advance_to(&mut self, now: Duration) {
        loop {
            let air_next =
                (!self.air.is_empty() && !self.rf_dead).then(|| self.air_free_at + self.air_time);
            let report_next = self
                .clusters
                .iter()
                .map(|cluster| cluster.next_report_at)
                .min();
            let next = match (air_next, report_next) {
                (Some(air), Some(report)) => air.min(report),
                (Some(air), None) => air,
                (None, Some(report)) => report,
                (None, None) => break,
            };
            if next > now {
                break;
            }
            if air_next == Some(next) {
                let envelope = self.air.pop_front().expect("air queue");
                self.air_free_at = next;
                self.deliver(&envelope, next);
            } else {
                for cluster in &mut self.clusters {
                    if cluster.next_report_at == next {
                        if !self.fans_silent {
                            cluster.report(next);
                        }
                        cluster.next_report_at = next + self.report_interval;
                    }
                }
            }
        }
    }

    fn deliver(&mut self, envelope: &AirEnvelope, now: Duration) {
        let bytes = &envelope.bytes;
        if bytes[0] != RF_SELECT || bytes[1] != RfSubCommand::SetRgb as u8 {
            return;
        }
        let number = self.rgb_envelopes;
        self.rgb_envelopes += 1;
        if let Some(drop) = self.drop_rgb_envelope.as_mut()
            && drop(number)
        {
            return;
        }
        let mut target = [0_u8; 6];
        target.copy_from_slice(&bytes[2..8]);
        let delay = self.report_delay;
        for cluster in &mut self.clusters {
            if cluster.mac == target && cluster.rx_type == envelope.rx_type {
                cluster.take_rgb(bytes, now, delay);
            }
        }
    }

    /// Hand the TX one packet; returns its reply when it has one.
    fn tx_packet(&mut self, now: Duration, packet: &[u8]) -> Option<Vec<u8>> {
        self.tx_packets += 1;
        match packet[0] {
            USB_CMD_GET_MAC => {
                let mut reply = vec![0_u8; 64];
                reply[0] = USB_CMD_GET_MAC;
                reply[1..7].copy_from_slice(&MASTER_MAC);
                reply[7..11].copy_from_slice(&[0, 0x0A, 0xE7, 0x4B]);
                reply[11..13].copy_from_slice(&[0x00, 0x10]);
                Some(reply)
            }
            USB_CMD_SEND_RF => {
                let slice = usize::from(packet[1]);
                if slice == 0 {
                    self.slices[0] = Some([0; RF_ENVELOPE_LEN]);
                    self.slice_filled = [false; 4];
                }
                if slice < 4
                    && let Some(envelope) = self.slices[0].as_mut()
                {
                    envelope[slice * RF_SLICE_LEN..(slice + 1) * RF_SLICE_LEN]
                        .copy_from_slice(&packet[USB_RF_HEADER_LEN..]);
                    self.slice_filled[slice] = true;
                    if slice == 3 && self.slice_filled.iter().all(|filled| *filled) {
                        let bytes = self.slices[0].take().expect("envelope");
                        if bytes[0] == RF_SELECT {
                            let header = (bytes[1] == RfSubCommand::SetRgb as u8 && bytes[18] == 0)
                                .then(|| {
                                    let mut tag = [0_u8; 4];
                                    tag.copy_from_slice(&bytes[14..18]);
                                    tag
                                });
                            if let Some(tag) = header
                                && self.last_header != Some(tag)
                            {
                                self.tx_rgb_headers.push((now, tag));
                            }
                            self.last_header = header;
                            if self.air.is_empty() && self.air_free_at < now {
                                self.air_free_at = now;
                            }
                            self.air.push_back(AirEnvelope {
                                bytes,
                                rx_type: packet[3],
                            });
                            self.max_air_queue = self.max_air_queue.max(self.air.len());
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Hand the RX one packet; returns its reply when it has one.
    fn rx_packet(&mut self, now: Duration, packet: &[u8]) -> Option<Vec<u8>> {
        match packet[0] {
            USB_CMD_RESET_PARTNER => {
                self.resets.push(now);
                None
            }
            USB_CMD_SEND_RF => {
                let pages = packet[1].max(1);
                // The LCD-mode switch is not answered.
                if packet[2..4] == [0x04, 0x30] {
                    return None;
                }
                self.rx_polls.push((now, pages));
                Some(self.table(pages))
            }
            _ => None,
        }
    }

    fn table(&self, pages: u8) -> Vec<u8> {
        let heard: Vec<&FakeCluster> = if self.fans_silent {
            Vec::new()
        } else {
            self.clusters.iter().collect()
        };
        let mut reply = vec![0_u8; GET_DEV_PAGE_LEN * usize::from(pages)];
        reply[0] = USB_CMD_SEND_RF;
        reply[1] = u8::try_from(heard.len()).expect("cluster count");
        reply[2] = 0x80;
        for (index, cluster) in heard.into_iter().enumerate() {
            let start = 4 + index * RECORD_LEN;
            let record = &mut reply[start..start + RECORD_LEN];
            record[0..6].copy_from_slice(&cluster.mac);
            record[6..12].copy_from_slice(&MASTER_MAC);
            record[12] = 8;
            record[13] = cluster.rx_type;
            record[14..18].copy_from_slice(&cluster.clock.to_be_bytes());
            record[18] = 0;
            record[19] = 3;
            record[20..24].copy_from_slice(&cluster.reported);
            record[24..27].copy_from_slice(&[28, 28, 28]);
            record[36..39].copy_from_slice(&[128, 128, 128]);
            record[41] = RECORD_VALIDATION;
        }
        reply
    }
}

/// The USB actor, on a simulated clock.
pub struct Rig {
    pub clock: SimClock,
    pub protocol: WirelessControllerProtocol,
    pub radio: FakeRadio,
    pub frame_period: Duration,
    /// Colors of frame `n` the render path publishes.
    pub frame: Box<dyn FnMut(u64) -> Vec<[u8; 3]>>,
    /// Stop publishing frames after this many (a scene that went still).
    pub frame_limit: Option<u64>,
    tick: Duration,
    next_tick_at: Duration,
    frames_taken: u64,
    last_taken: Option<u64>,
    /// When the actor took each frame, and its index.
    pub taken_log: Vec<(Duration, u64)>,
    /// Commands the keepalive has returned since the protocol asked for
    /// the TX reset: none is the contract.
    pub commands_after_reset: usize,
}

/// A frame whose pixels, and so whose tag, change every frame.
#[must_use]
pub fn moving_frame(index: u64) -> Vec<[u8; 3]> {
    let bytes = index.to_le_bytes();
    vec![[bytes[0], bytes[1], 0x40]; LEDS]
}

impl Rig {
    /// Connect the protocol to `radio` and let the render path start.
    #[must_use]
    pub fn connect(radio: FakeRadio) -> Self {
        let clock = SimClock::new();
        let protocol = protocol_on(&clock);
        let mut rig = Self {
            clock,
            protocol,
            radio,
            frame_period: FRAME_PERIOD,
            frame: Box::new(moving_frame),
            frame_limit: None,
            tick: Duration::ZERO,
            next_tick_at: Duration::ZERO,
            frames_taken: 0,
            last_taken: None,
            taken_log: Vec::new(),
            commands_after_reset: 0,
        };
        let init = rig.protocol.init_sequence();
        rig.execute(&init);
        let diagnostics = rig.protocol.connection_diagnostics();
        rig.execute(&diagnostics);
        rig.tick = rig
            .protocol
            .keepalive()
            .expect("the controller keeps alive")
            .interval;
        rig.next_tick_at = rig.clock.elapsed() + rig.tick;
        rig
    }

    #[must_use]
    pub fn now(&self) -> Duration {
        self.clock.elapsed()
    }

    #[must_use]
    pub fn stats(&self) -> DeliveryStats {
        self.protocol.delivery_stats()
    }

    fn advance(&mut self, by: Duration) {
        self.clock.advance(by);
        let now = self.now();
        self.radio.advance_to(now);
    }

    fn published(&self, now: Duration) -> u64 {
        let published =
            u64::try_from(now.as_nanos() / self.frame_period.as_nanos()).expect("frame count") + 1;
        self.frame_limit
            .map_or(published, |limit| published.min(limit))
    }

    /// Run the actor loop until `duration` has passed.
    pub fn run_for(&mut self, duration: Duration) {
        let end = self.now() + duration;
        while self.now() < end {
            let now = self.now();
            if now >= self.next_tick_at {
                // Missed ticks are skipped, as the actor's interval does.
                while self.next_tick_at <= now {
                    self.next_tick_at += self.tick;
                }
                let commands = self.protocol.keepalive_commands();
                if !self.radio.resets.is_empty() {
                    self.commands_after_reset += commands.len();
                }
                self.execute(&commands);
                continue;
            }
            let published = self.published(now);
            if self.last_taken.is_none_or(|taken| taken + 1 < published) {
                let index = published - 1;
                self.last_taken = Some(index);
                self.frames_taken += 1;
                self.taken_log.push((now, index));
                let colors = (self.frame)(index);
                let mut commands = Vec::new();
                self.protocol.encode_frame_into(&colors, &mut commands);
                if !self.radio.resets.is_empty() {
                    self.commands_after_reset += commands.len();
                }
                self.execute(&commands);
                continue;
            }
            let next_frame_at = self.frame_period * u32::try_from(published).expect("frames");
            let next = self.next_tick_at.min(next_frame_at).min(end);
            let step = next.saturating_sub(now).max(Duration::from_micros(10));
            self.advance(step);
        }
    }

    /// Run commands the way the USB actor does.
    pub fn execute(&mut self, commands: &[ProtocolCommand]) {
        for command in commands {
            self.advance(USB_PACKET_TIME);
            let now = self.now();
            let reply = match command.transfer_type {
                TransferType::Companion => self.radio.rx_packet(now, &command.data),
                _ => self.radio.tx_packet(now, &command.data),
            };
            if command.expects_response {
                match reply {
                    Some(reply) => {
                        let packets = u32::try_from(reply.len().div_ceil(64)).expect("packets");
                        self.advance(RX_PACKET_TIME * packets);
                        self.protocol
                            .parse_response(&reply)
                            .expect("the fake answers in the protocol's format");
                    }
                    None => assert_eq!(
                        command.response.tolerance,
                        ResponseTolerance::Optional,
                        "a required reply the fake never sends: {:02x?}",
                        &command.data[..4]
                    ),
                }
            }
            if !command.post_delay.is_zero() {
                self.advance(command.post_delay);
            }
        }
    }

    /// RGB headers the TX took after `since`.
    #[must_use]
    pub fn headers_since(&self, since: Duration) -> usize {
        self.radio
            .tx_rgb_headers
            .iter()
            .filter(|(at, _)| *at >= since)
            .count()
    }

    /// Frames the render path handed the protocol.
    #[must_use]
    pub const fn frames_taken(&self) -> u64 {
        self.frames_taken
    }
}

/// Whether `commands` hold the TX reset the protocol asks for.
#[must_use]
pub fn is_tx_reset(command: &ProtocolCommand) -> bool {
    command.transfer_type == TransferType::Companion && command.data[0] == USB_CMD_RESET_PARTNER
}

/// Broadcast slot, re-exported for assertions on upkeep traffic.
pub const BROADCAST: u8 = RF_BROADCAST_SLOT;
