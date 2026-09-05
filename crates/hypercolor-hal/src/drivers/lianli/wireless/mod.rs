//! Lian Li L-Wireless: the 2.4 GHz fan ecosystem behind a USB controller.
//!
//! The controller (`0x0416:0x8040` TX plus its `0x0416:0x8041` RX sibling)
//! tunnels RF frames over USB bulk. Fan PWM, per-LED RGB, and telemetry ride
//! the radio; the LCD on a wireless LCD fan stays wired and is a separate
//! device with its own protocol. Spec 80 sections 6 and 7 carry the wire
//! facts, with the corrections recorded in [`discovery`].
//!
//! The protocol is one [`Protocol`] over a companion transport: TX commands
//! travel on the primary path, the RX device table poll on
//! [`TransferType::Companion`]. Discovery happens at init and again on every
//! keepalive tick, which also holds each cluster's observed PWM steady and
//! broadcasts the 1 Hz clock the fan firmware expects.

pub mod crypto;
pub mod discovery;
pub mod frame;
pub mod lcd;
pub mod tinyuz;
pub mod transport;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceTopologyHint, SegmentInfo,
};
use tracing::debug;

use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError, ProtocolKeepalive, ProtocolResponse,
    ResponseStatus, TransferType,
};

use discovery::{
    DeviceTable, DiscoveryError, FanCluster, GET_DEV_REPLY_CAPACITY, MasterInfo,
    parse_device_table, parse_master_reply,
};
use frame::{
    DEFAULT_CHANNEL, Mac, RF_BROADCAST_SLOT, RfEnvelope, TX_RESET, TX_VIDEO_START, USB_CMD_GET_MAC,
    USB_CMD_SEND_RF, WallClock, clock_payload, clock_sync_envelope, control_packet, get_dev_poll,
    get_mac_query, pwm_envelope, reverse_fan_order, rgb_transfer, stream_prep_packet,
};

/// Reads at init, where the RX answers a two-page poll in about 30 ms and
/// the TX its status in about 1 ms; generous for a cold radio.
const INIT_TIMEOUT: Duration = Duration::from_secs(1);
/// Steady-state reads.
const STEADY_TIMEOUT: Duration = Duration::from_millis(500);
/// The radio needs this long after a reset before it answers sensibly.
const RESET_SETTLE: Duration = Duration::from_millis(500);
/// Gap between the four USB packets of one envelope, and between control
/// packets; what the reference driver ships.
const SLICE_PACING: Duration = Duration::from_millis(1);
/// Pages of the device table polled: two covers the twelve-record maximum.
const GET_DEV_PAGES: u8 = 2;
/// Upkeep cadence: fans drift to firmware defaults when PWM traffic stops,
/// and miss the clock into an autonomous fallback.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(1);
/// Frame cadence baseline until the live RF rate is measured on hardware;
/// raise on measurement, never lower (spec 80 section 11.4).
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
const MAX_FPS: u32 = 10;
/// A live frame is a one-frame animation; the interval is what the
/// reference sends for stills.
const LIVE_TOTAL_FRAMES: u16 = 1;
const LIVE_INTERVAL_MS: u16 = 5000;

/// Everything learned from the controller and its table.
#[derive(Debug, Default)]
struct WirelessState {
    master: Option<MasterInfo>,
    table: DeviceTable,
    streaming_started: bool,
    clock_sent: bool,
}

/// The L-Wireless controller protocol.
pub struct WirelessControllerProtocol {
    state: RwLock<WirelessState>,
    /// Tag stamped on each RGB transfer; receivers echo the last one they
    /// accepted in their device record.
    effect_counter: AtomicU32,
}

impl Default for WirelessControllerProtocol {
    fn default() -> Self {
        Self::new()
    }
}

impl WirelessControllerProtocol {
    /// A protocol with nothing discovered yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: RwLock::new(WirelessState::default()),
            effect_counter: AtomicU32::new(1),
        }
    }

    /// The controller's identity, once the MAC query has answered.
    #[must_use]
    pub fn master(&self) -> Option<MasterInfo> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .master
    }

    /// The fan clusters from the last device table.
    #[must_use]
    pub fn clusters(&self) -> Vec<FanCluster> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .table
            .clusters
            .clone()
    }

    /// Duty the controller reads off the motherboard PWM header.
    #[must_use]
    pub fn motherboard_pwm(&self) -> Option<u8> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .table
            .motherboard_pwm
    }

    fn master_mac(state: &WirelessState) -> Mac {
        state.master.map_or([0; 6], |master| master.mac)
    }

    fn tx_command(packet: Vec<u8>, expects_response: bool) -> ProtocolCommand {
        ProtocolCommand {
            data: packet,
            expects_response,
            post_delay: SLICE_PACING,
            ..Default::default()
        }
    }

    /// The RX device table poll, read with the capacity a two-page reply
    /// needs; the reply is page-sized rather than record-sized.
    fn get_dev_command(timeout: Duration) -> ProtocolCommand {
        ProtocolCommand {
            data: get_dev_poll(GET_DEV_PAGES),
            expects_response: true,
            transfer_type: TransferType::Companion,
            ..Default::default()
        }
        .with_response_capacity(GET_DEV_REPLY_CAPACITY)
        .with_response_timeout(timeout)
    }

    fn push_envelope(
        buffer: &mut CommandBuffer<'_>,
        envelope: &RfEnvelope,
        channel: u8,
        rx_type: u8,
    ) {
        for packet in envelope.usb_packets(channel, rx_type) {
            buffer.push_slice(
                &packet,
                false,
                Duration::ZERO,
                SLICE_PACING,
                TransferType::Primary,
            );
        }
    }

    fn envelope_commands(
        commands: &mut Vec<ProtocolCommand>,
        envelope: &RfEnvelope,
        channel: u8,
        rx_type: u8,
    ) {
        for packet in envelope.usb_packets(channel, rx_type) {
            commands.push(Self::tx_command(packet.to_vec(), false));
        }
    }

    fn next_effect_index(&self) -> [u8; 4] {
        self.effect_counter
            .fetch_add(1, Ordering::Relaxed)
            .to_be_bytes()
    }

    /// The channel envelopes ride: the first cluster's, else the default.
    fn channel(state: &WirelessState) -> u8 {
        state
            .table
            .clusters
            .first()
            .map_or(DEFAULT_CHANNEL, |cluster| cluster.channel)
    }
}

impl Protocol for WirelessControllerProtocol {
    fn name(&self) -> &'static str {
        "Lian Li L-Wireless Controller"
    }

    /// Reset the radio, learn the controller's MAC, then read the device
    /// table from the RX. Every session starts from an empty table so a
    /// cluster unbound while the daemon was down does not linger.
    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        {
            let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
            *state = WirelessState::default();
        }

        let mut reset = Self::tx_command(control_packet(&TX_RESET), true).with_optional_response();
        reset.post_delay = RESET_SETTLE;

        vec![
            reset,
            Self::tx_command(get_mac_query(DEFAULT_CHANNEL), true)
                .with_response_timeout(INIT_TIMEOUT),
            Self::get_dev_command(INIT_TIMEOUT),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        // Nothing to send: fans revert to firmware defaults when the upkeep
        // stops, the same as when L-Connect exits.
        Vec::new()
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    /// One RGB transfer per cluster, fans in slot order, the first frame of
    /// a session preceded by the streaming-mode switch.
    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let master = Self::master_mac(&state);
        let channel = Self::channel(&state);
        let mut buffer = CommandBuffer::new(commands);

        if !state.streaming_started {
            buffer.push_slice(
                &control_packet(&TX_VIDEO_START),
                false,
                Duration::ZERO,
                SLICE_PACING,
                TransferType::Primary,
            );
            for index in 0..u8::try_from(state.table.clusters.len().max(1)).unwrap_or(u8::MAX) {
                buffer.push_slice(
                    &stream_prep_packet(index, channel),
                    false,
                    Duration::ZERO,
                    SLICE_PACING,
                    TransferType::Primary,
                );
            }
            state.streaming_started = true;
        }

        let mut offset = 0_usize;
        for cluster in &state.table.clusters {
            let leds_per_fan = usize::from(cluster.model.leds_per_fan());
            let led_count = usize::from(cluster.fan_count) * leds_per_fan;
            let mut raw = Vec::with_capacity(led_count * 3);
            for led in 0..led_count {
                let color = colors.get(offset + led).copied().unwrap_or([0, 0, 0]);
                raw.extend_from_slice(&color);
            }
            offset += led_count;
            if cluster.right_attach {
                raw = reverse_fan_order(&raw, leds_per_fan, usize::from(cluster.fan_count));
            }

            let transfer = rgb_transfer(
                cluster.mac,
                master,
                self.next_effect_index(),
                cluster.model.leds_per_fan(),
                LIVE_TOTAL_FRAMES,
                LIVE_INTERVAL_MS,
                &raw,
            );
            Self::push_envelope(
                &mut buffer,
                &transfer.header,
                cluster.channel,
                cluster.rx_type,
            );
            for envelope in &transfer.data {
                Self::push_envelope(&mut buffer, envelope, cluster.channel, cluster.rx_type);
            }
        }

        buffer.finish();
    }

    fn keepalive(&self) -> Option<ProtocolKeepalive> {
        Some(ProtocolKeepalive {
            commands: Vec::new(),
            interval: KEEPALIVE_INTERVAL,
        })
    }

    /// The 1 Hz upkeep: refresh the table, hold every cluster at the PWM it
    /// reported, and broadcast the clock.
    fn keepalive_commands(&self) -> Vec<ProtocolCommand> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let master = Self::master_mac(&state);
        let channel = Self::channel(&state);
        let mut commands = vec![Self::get_dev_command(STEADY_TIMEOUT)];

        for (index, cluster) in state.table.clusters.iter().enumerate() {
            let slot_index = u8::try_from(index + 1).unwrap_or(u8::MAX);
            let envelope = pwm_envelope(
                cluster.mac,
                master,
                cluster.rx_type,
                cluster.channel,
                slot_index,
                cluster.pwm,
            );
            Self::envelope_commands(&mut commands, &envelope, cluster.channel, cluster.rx_type);
        }

        let payload = clock_payload(WallClock::now_local());
        let envelope = clock_sync_envelope(master, &payload, !state.clock_sent);
        Self::envelope_commands(&mut commands, &envelope, channel, RF_BROADCAST_SLOT);
        state.clock_sent = true;

        commands
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let Some(&echo) = data.first() else {
            return Err(ProtocolError::MalformedResponse {
                detail: "empty controller reply".to_owned(),
            });
        };

        match echo {
            USB_CMD_GET_MAC => {
                if let Some(master) = parse_master_reply(data) {
                    self.state
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .master = Some(master);
                }
            }
            USB_CMD_SEND_RF => {
                let master = self.master().map(|master| master.mac);
                match parse_device_table(data, master) {
                    Ok(table) => {
                        self.state
                            .write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .table = table;
                    }
                    Err(DiscoveryError::StatusEcho) => {
                        debug!("controller status packet where a device table was expected");
                    }
                    Err(error) => {
                        return Err(ProtocolError::MalformedResponse {
                            detail: error.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }

        Ok(ProtocolResponse {
            status: ResponseStatus::Ok,
            data: data.to_vec(),
        })
    }

    fn response_timeout(&self) -> Duration {
        STEADY_TIMEOUT
    }

    /// One ring segment per fan, clusters in table order.
    fn zones(&self) -> Vec<SegmentInfo> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        let mut zones = Vec::new();
        for (index, cluster) in state.table.clusters.iter().enumerate() {
            let led_count = u32::from(cluster.model.leds_per_fan());
            for slot in 0..cluster.fan_count {
                zones.push(SegmentInfo {
                    name: format!("{} {} Fan {}", cluster.model.name(), index + 1, slot + 1),
                    led_count,
                    topology: DeviceTopologyHint::Ring { count: led_count },
                    color_format: DeviceColorFormat::Rgb,
                    layout_hint: None,
                });
            }
        }
        zones
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.total_leds(),
            supports_direct: true,
            supports_brightness: false,
            max_fps: MAX_FPS,
            ..DeviceCapabilities::default()
        }
    }

    fn total_leds(&self) -> u32 {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .table
            .clusters
            .iter()
            .map(FanCluster::led_count)
            .sum()
    }

    fn frame_interval(&self) -> Duration {
        FRAME_INTERVAL
    }
}
