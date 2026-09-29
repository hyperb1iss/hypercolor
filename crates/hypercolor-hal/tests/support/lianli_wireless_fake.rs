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
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_hal::drivers::lianli::WirelessControllerProtocol;
use hypercolor_hal::drivers::lianli::wireless::discovery::{
    GET_DEV_PAGE_LEN, RECORD_LEN, RECORD_VALIDATION,
};
use hypercolor_hal::drivers::lianli::wireless::frame::{
    RF_BROADCAST_SLOT, RF_ENVELOPE_LEN, RF_SELECT, RF_SLICE_LEN, RGB_CHUNK_LEN, RGB_DATA_OFFSET,
    RfSubCommand, USB_CMD_GET_MAC, USB_CMD_RESET_PARTNER, USB_CMD_SEND_RF, USB_RF_HEADER_LEN,
    effect_index_for,
};
use hypercolor_hal::drivers::lianli::wireless::pacing::DeliveryStats;
use hypercolor_hal::drivers::lianli::wireless::tinyuz;
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseTolerance, TransferType};

/// Radio MACs of their own for every rig: the protocol keeps its TX reset
/// budget per cluster for the whole process, and tests run in parallel.
fn fresh_mac(prefix: [u8; 5]) -> [u8; 6] {
    static NEXT: AtomicU8 = AtomicU8::new(1);
    let mut mac = [0; 6];
    mac[..5].copy_from_slice(&prefix);
    mac[5] = NEXT.fetch_add(1, Ordering::Relaxed);
    mac
}

fn fresh_master_mac() -> [u8; 6] {
    fresh_mac([0xA0, 0x71, 0xAE, 0x72, 0xAB])
}

fn fresh_cluster_mac() -> [u8; 6] {
    fresh_mac([0x11, 0x11, 0x11, 0x11, 0x11])
}

/// The pixel hash of `colors`: what a transfer shows, whatever its tag.
#[must_use]
pub fn content_of(colors: &[[u8; 3]]) -> [u8; 4] {
    let raw: Vec<u8> = colors.iter().flatten().copied().collect();
    effect_index_for(&raw)
}

/// A transfer part way through arriving.
#[derive(Debug, Clone)]
struct PartialTransfer {
    tag: [u8; 4],
    envelopes: u8,
    compressed_len: usize,
    leds: usize,
    /// Data chunks by envelope index; the header's slot stays empty.
    chunks: Vec<Option<Vec<u8>>>,
}

/// Rebuilds an RGB transfer from its envelopes, as a receiver does.
#[derive(Debug, Clone, Default)]
struct TransferAssembler {
    current: Option<PartialTransfer>,
}

impl TransferAssembler {
    /// Take one RGB envelope; returns the tag and pixel hash of a transfer
    /// it completes.
    fn take(&mut self, envelope: &[u8; RF_ENVELOPE_LEN]) -> Option<([u8; 4], [u8; 4])> {
        let mut tag = [0_u8; 4];
        tag.copy_from_slice(&envelope[14..18]);
        let index = envelope[18];
        if index == 0 {
            if self
                .current
                .as_ref()
                .is_some_and(|current| current.tag == tag)
            {
                return None;
            }
            let compressed_len =
                u32::from_be_bytes([envelope[20], envelope[21], envelope[22], envelope[23]]);
            self.current = Some(PartialTransfer {
                tag,
                envelopes: envelope[19],
                compressed_len: usize::try_from(compressed_len).expect("length"),
                leds: usize::from(envelope[27]),
                chunks: vec![None; usize::from(envelope[19])],
            });
            return None;
        }
        let current = self.current.as_mut()?;
        if current.tag != tag || index >= current.envelopes {
            return None;
        }
        current.chunks[usize::from(index)] =
            Some(envelope[RGB_DATA_OFFSET..RGB_DATA_OFFSET + RGB_CHUNK_LEN].to_vec());
        if !current.chunks.iter().skip(1).all(Option::is_some) {
            return None;
        }
        let mut compressed: Vec<u8> = current
            .chunks
            .iter()
            .skip(1)
            .flatten()
            .flatten()
            .copied()
            .collect();
        compressed.truncate(current.compressed_len);
        let raw = tinyuz::decompress(&compressed, current.leds * 3).expect("a transfer decodes");
        let done = (current.tag, effect_index_for(&raw));
        self.current = None;
        Some(done)
    }
}

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
    /// Pixel hash of every transfer it took, in order.
    pub applied_log: Vec<[u8; 4]>,
    /// What the RX last heard from it: echoed tag and clock.
    pub reported: [u8; 4],
    pub clock: u32,
    next_report_at: Duration,
    /// Tags taken, with when the RX can first report each.
    reportable: VecDeque<(Duration, [u8; 4])>,
    assembler: TransferAssembler,
    /// The receiver hears nothing the TX sends, but still reports.
    pub deaf: bool,
    /// This receiver's reports lag by this much instead of the radio's.
    pub report_delay: Option<Duration>,
    /// A fan-side stall: the receiver drops every RGB transfer until it
    /// hears a session start (the first clock broadcast of a session) after
    /// a rest of at least this long with no RGB addressed to it. A TX reset
    /// alone never clears it, nor does a session start without the rest.
    /// `Duration::MAX` never clears.
    pub stall: Option<Duration>,
    last_rgb_at: Duration,
    /// The stalled receiver has had its rest.
    rested: bool,
    /// When the stall cleared.
    pub stall_cleared_at: Option<Duration>,
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
            assembler: TransferAssembler::default(),
            deaf: false,
            report_delay: None,
            stall: None,
            last_rgb_at: Duration::ZERO,
            rested: false,
            stall_cleared_at: None,
        }
    }

    /// Stall from `now`, until a session start that follows a rest of at
    /// least `quiet` with no RGB addressed to this receiver.
    pub fn stall_from(&mut self, now: Duration, quiet: Duration) {
        self.stall = Some(quiet);
        self.last_rgb_at = now;
        self.rested = false;
        self.stall_cleared_at = None;
    }

    fn note_rest(&mut self, now: Duration) {
        if let Some(quiet) = self.stall
            && now.saturating_sub(self.last_rgb_at) >= quiet
        {
            self.rested = true;
        }
    }

    fn hear_session_start(&mut self, now: Duration) {
        self.note_rest(now);
        if self.stall.is_some() && self.rested {
            self.stall = None;
            self.rested = false;
            self.stall_cleared_at = Some(now);
        }
    }

    /// Pixel hash of what the fans show now.
    #[must_use]
    pub fn showing(&self) -> Option<[u8; 4]> {
        self.applied_log.last().copied()
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
        if self.stall.is_some() {
            self.note_rest(now);
            self.last_rgb_at = now;
            return;
        }
        if let Some((tag, content)) = self.assembler.take(envelope) {
            self.applied = tag;
            self.applied_log.push(content);
            let delay = self.report_delay.unwrap_or(delay);
            self.reportable.push_back((now + delay, tag));
        }
    }
}

/// Whether `bytes` is the clock broadcast a session sends first: its
/// leading 50 payload bytes all carry the sub-command.
fn is_session_start_clock(bytes: &[u8; RF_ENVELOPE_LEN]) -> bool {
    let marker = RfSubCommand::ClockSync as u8;
    bytes[1] == marker && bytes[14..64].iter().all(|byte| *byte == marker)
}

/// An envelope the TX has queued for the air.
#[derive(Debug, Clone)]
struct AirEnvelope {
    bytes: [u8; RF_ENVELOPE_LEN],
    rx_type: u8,
}

/// The TX, the air, the fan receivers, and the RX.
pub struct FakeRadio {
    pub master: [u8; 6],
    pub clusters: Vec<FakeCluster>,
    /// Time one envelope occupies the air.
    pub air_time: Duration,
    /// How often each receiver reports to the RX.
    pub report_interval: Duration,
    /// How long after a receiver takes a transfer its reports carry it.
    pub report_delay: Duration,
    /// Each status comes this much earlier or later than the interval,
    /// at random.
    pub report_jitter: Duration,
    /// Each status is a snapshot up to this much older than when the RX
    /// hands it over, at random: the staleness the owner's rig showed.
    pub snapshot_lag: Duration,
    rng: u64,
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
    /// Rebuilds each transfer the TX queues, to log what it carries.
    tx_assembler: TransferAssembler,
    rgb_envelopes: u64,
    pub max_air_queue: usize,
    pub tx_packets: u64,
    /// Every RGB transfer the TX took: when its last envelope was queued,
    /// its tag, and its pixel hash.
    pub tx_transfers: Vec<(Duration, [u8; 4], [u8; 4])>,
    pub rx_polls: Vec<(Duration, u8)>,
    pub resets: Vec<Duration>,
    /// When a session-start clock broadcast went out on the air.
    pub session_starts: Vec<Duration>,
}

impl FakeRadio {
    /// One three-fan TL cluster bound to this controller.
    #[must_use]
    pub fn one_cluster() -> Self {
        Self::with_clusters(vec![FakeCluster::new(fresh_cluster_mac(), 3)])
    }

    #[must_use]
    pub fn with_clusters(clusters: Vec<FakeCluster>) -> Self {
        Self {
            master: fresh_master_mac(),
            clusters,
            air_time: Duration::from_millis(2),
            report_interval: Duration::from_millis(10),
            report_delay: Duration::ZERO,
            report_jitter: Duration::ZERO,
            snapshot_lag: Duration::ZERO,
            rng: 0x9E37_79B9_7F4A_7C15,
            rf_dead: false,
            fans_silent: false,
            drop_rgb_envelope: None,
            air: VecDeque::new(),
            air_free_at: Duration::ZERO,
            slices: vec![None],
            slice_filled: [false; 4],
            tx_assembler: TransferAssembler::default(),
            rgb_envelopes: 0,
            max_air_queue: 0,
            tx_packets: 0,
            tx_transfers: Vec::new(),
            rx_polls: Vec::new(),
            resets: Vec::new(),
            session_starts: Vec::new(),
        }
    }

    /// Two clusters, the second two fans on its own receiver slot.
    #[must_use]
    pub fn two_clusters() -> Self {
        Self::with_clusters(vec![
            FakeCluster::new(fresh_cluster_mac(), 3),
            FakeCluster::new(fresh_cluster_mac(), 4),
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
                for index in 0..self.clusters.len() {
                    if self.clusters[index].next_report_at != next {
                        continue;
                    }
                    let lag = self.random_up_to(self.snapshot_lag);
                    let jitter = self.random_up_to(self.report_jitter * 2);
                    let silent = self.fans_silent;
                    let interval = self.report_interval;
                    let early = self.report_jitter;
                    let cluster = &mut self.clusters[index];
                    if !silent {
                        cluster.report(next.saturating_sub(lag));
                    }
                    cluster.next_report_at = (next + interval + jitter)
                        .saturating_sub(early)
                        .max(next + Duration::from_millis(1));
                }
            }
        }
    }

    /// A deterministic pseudo-random duration in `0..=max`.
    fn random_up_to(&mut self, max: Duration) -> Duration {
        if max.is_zero() {
            return Duration::ZERO;
        }
        // xorshift64: reproducible runs, no dependency.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let span = u64::try_from(max.as_micros()).expect("short span");
        Duration::from_micros(self.rng % (span + 1))
    }

    fn deliver(&mut self, envelope: &AirEnvelope, now: Duration) {
        let bytes = &envelope.bytes;
        if bytes[0] == RF_SELECT && is_session_start_clock(bytes) {
            self.session_starts.push(now);
            for cluster in &mut self.clusters {
                if !cluster.deaf {
                    cluster.hear_session_start(now);
                }
            }
            return;
        }
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
            if cluster.mac == target && cluster.rx_type == envelope.rx_type && !cluster.deaf {
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
                reply[1..7].copy_from_slice(&self.master);
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
                            if bytes[1] == RfSubCommand::SetRgb as u8
                                && let Some((tag, content)) = self.tx_assembler.take(&bytes)
                            {
                                self.tx_transfers.push((now, tag, content));
                            }
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
            record[6..12].copy_from_slice(&self.master);
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
    pump: Option<Duration>,
    next_pump_at: Duration,
    frames_taken: u64,
    last_taken: Option<u64>,
    /// When the actor took each frame, and its index.
    pub taken_log: Vec<(Duration, u64)>,
    /// Commands the keepalive has returned since the protocol asked for
    /// the TX reset: none is the contract.
    pub commands_after_reset: usize,
    /// Reconnect like the device lifecycle: when a session ends (the TX
    /// reset, or the protocol asking to be connected afresh), nothing
    /// reaches the controller for this long, then a new protocol runs the
    /// whole connect sequence. `None` leaves the ended session in place.
    pub reconnect_delay: Option<Duration>,
    /// When each session connected.
    pub sessions: Vec<Duration>,
    /// Why each session ended, when the rig reconnected it.
    pub session_ends: Vec<SessionEnd>,
    reset_in_session: bool,
    down_until: Option<Duration>,
}

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    /// The protocol asked for the TX reset.
    TxReset,
    /// The protocol asked to be connected afresh.
    Restart,
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
        Self::connect_on(radio, SimClock::new())
    }

    /// Connect a new protocol to `radio` on `clock`, which keeps running.
    fn connect_on(radio: FakeRadio, clock: SimClock) -> Self {
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
            pump: None,
            next_pump_at: Duration::ZERO,
            frames_taken: 0,
            last_taken: None,
            taken_log: Vec::new(),
            commands_after_reset: 0,
            reconnect_delay: None,
            sessions: Vec::new(),
            session_ends: Vec::new(),
            reset_in_session: false,
            down_until: None,
        };
        rig.begin_session();
        rig
    }

    /// Run the connect sequence on the current protocol and start its
    /// keepalive and frame pump.
    fn begin_session(&mut self) {
        self.sessions.push(self.now());
        self.reset_in_session = false;
        self.down_until = None;
        let init = self.protocol.init_sequence();
        self.execute(&init);
        let diagnostics = self.protocol.connection_diagnostics();
        self.execute(&diagnostics);
        self.tick = self
            .protocol
            .keepalive()
            .expect("the controller keeps alive")
            .interval;
        self.next_tick_at = self.clock.elapsed() + self.tick;
        self.pump = self.protocol.frame_pump_interval();
        self.next_pump_at = self.clock.elapsed() + self.pump.unwrap_or(Duration::MAX / 4);
    }

    /// After a batch: end the session the way the actor does when the
    /// protocol asked for the TX reset or a fresh connect, if the rig
    /// reconnects.
    fn end_session_if_asked(&mut self, commands: &[ProtocolCommand]) {
        let Some(delay) = self.reconnect_delay else {
            return;
        };
        let end = if commands.iter().any(is_tx_reset) {
            SessionEnd::TxReset
        } else if self.protocol.session_restart().is_some() {
            SessionEnd::Restart
        } else {
            return;
        };
        self.session_ends.push(end);
        self.down_until = Some(self.now() + delay);
    }

    /// End this session and connect a new protocol to the same radio, as a
    /// reconnect does: the receivers keep what they last took, the RX keeps
    /// reporting it, and time runs on.
    #[must_use]
    pub fn reconnect(self) -> Self {
        let Self {
            clock,
            mut radio,
            frame,
            frame_limit,
            ..
        } = self;
        radio.resets.clear();
        let mut rig = Self::connect_on(radio, clock);
        rig.frame = frame;
        rig.frame_limit = frame_limit;
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

    /// The protocol's current window for cluster `cluster`.
    #[must_use]
    pub fn protocol_window(&self, cluster: usize) -> Option<u32> {
        self.protocol.delivery_window(cluster)
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

    /// Run the actor loop until `duration` has passed. Like the actor, the
    /// keepalive wins when several are due, then a waiting frame, then the
    /// frame pump; missed ticks are skipped.
    pub fn run_for(&mut self, duration: Duration) {
        let end = self.now() + duration;
        while self.now() < end {
            let now = self.now();
            if let Some(until) = self.down_until {
                if now >= until {
                    self.protocol = protocol_on(&self.clock);
                    self.begin_session();
                } else {
                    self.advance(
                        until
                            .min(end)
                            .saturating_sub(now)
                            .max(Duration::from_micros(10)),
                    );
                }
                continue;
            }
            if now >= self.next_tick_at {
                while self.next_tick_at <= now {
                    self.next_tick_at += self.tick;
                }
                let commands = self.protocol.keepalive_commands();
                self.run_batch(&commands);
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
                self.run_batch(&commands);
                continue;
            }
            if let Some(pump) = self.pump
                && now >= self.next_pump_at
            {
                while self.next_pump_at <= now {
                    self.next_pump_at += pump;
                }
                let mut commands = Vec::new();
                self.protocol.pump_frame_into(&mut commands);
                self.run_batch(&commands);
                continue;
            }
            let next_frame_at = self.frame_period * u32::try_from(published).expect("frames");
            let next = self
                .next_tick_at
                .min(next_frame_at)
                .min(self.next_pump_at)
                .min(end);
            let step = next.saturating_sub(now).max(Duration::from_micros(10));
            self.advance(step);
        }
    }

    fn run_batch(&mut self, commands: &[ProtocolCommand]) {
        if self.reset_in_session {
            self.commands_after_reset += commands.len();
        }
        if commands.iter().any(is_tx_reset) {
            self.reset_in_session = true;
        }
        self.execute(commands);
        self.end_session_if_asked(commands);
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

    /// RGB transfers the TX took after `since`.
    #[must_use]
    pub fn transfers_since(&self, since: Duration) -> usize {
        self.radio
            .tx_transfers
            .iter()
            .filter(|(at, ..)| *at >= since)
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
