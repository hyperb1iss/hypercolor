//! Push 2 LED output flow control.
//!
//! Ableton documents no MIDI input rate for the Push 2. What the interface
//! manual does document is a request/reply contract: before sending the next
//! command, the host should wait for the reply of the previous one. Replies
//! come back in order, after the firmware has processed every earlier message,
//! so a reply is also proof that the device drained everything sent before
//! it. The LED lane paces itself on that proof:
//!
//! - at most one USB-MIDI endpoint packet of LED traffic is ever
//!   unconfirmed; a batch that needs more ends with an acknowledgement
//!   request (a documented palette read) and the next batch waits for it;
//! - every batch is computed from the newest frame against the device state
//!   the host believes, so superseded frames are never queued, only
//!   overwritten;
//! - a request that goes unanswered puts the lane on hold: no LED traffic is
//!   piled behind a stalled endpoint, a single small probe is retried with
//!   backoff, and the first answer schedules a full resync;
//! - a periodic resync re-sends the believed state so a lost message heals
//!   without waiting for the effect to touch that LED again.

use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::protocol::{CommandBuffer, ProtocolCommand, ResponseTolerance, TransferType};

use super::{
    PUSH2_CMD_GET_PALETTE_ENTRY, PUSH2_CMD_SET_MIDI_MODE, PUSH2_CMD_SET_TOUCH_STRIP_CONFIG,
    PUSH2_MANUFACTURER_PREFIX, PUSH2_MIDI_MODE_USER, PUSH2_PALETTE_SIZE,
    PUSH2_TOUCH_STRIP_HOST_CONFIG, Push2State,
};

/// Bytes per USB-MIDI 1.0 event packet on the wire.
const USB_MIDI_EVENT_BYTES: usize = 4;

/// Bulk OUT `wMaxPacketSize` of the Push 2 MIDI streaming interface.
///
/// The Push 2 enumerates at high speed, where a bulk endpoint's max packet
/// is 512 bytes. Keeping unconfirmed LED traffic within one packet means the
/// firmware never has more than one endpoint buffer of our data to absorb.
pub(super) const PUSH2_MIDI_ENDPOINT_PACKET_BYTES: usize = 512;

/// Unconfirmed traffic that earns an acknowledgement request even when a
/// batch still had room. Small trickles (a single pad change) skip the
/// request until they add up to half a packet.
const ACK_WATERMARK_BYTES: usize = PUSH2_MIDI_ENDPOINT_PACKET_BYTES / 2;

/// Acknowledgement request: Get LED Color Palette Entry (0x04), whose reply
/// echoes the requested index and so correlates with exactly one request.
const ACK_REQUEST_LEN: usize = PUSH2_MANUFACTURER_PREFIX.len() + 3;

/// How often the believed device state is re-sent in full.
pub(super) const PUSH2_RESYNC_INTERVAL: Duration = Duration::from_secs(10);

/// How often User mode and touch-strip host control are re-asserted.
pub(super) const PUSH2_MODE_ASSERT_INTERVAL: Duration = Duration::from_secs(5);

const STALL_PROBE_BACKOFF_MIN: Duration = Duration::from_millis(250);
const STALL_PROBE_BACKOFF_MAX: Duration = Duration::from_secs(5);
const STALL_REPORT_INTERVAL: Duration = Duration::from_mins(1);

/// Bytes a MIDI message occupies on the USB wire.
///
/// USB-MIDI 1.0 carries a channel message in one 4-byte event packet and a
/// sysex in one packet per started group of three bytes.
pub(super) const fn usb_midi_wire_bytes(message_len: usize) -> usize {
    message_len.div_ceil(3) * USB_MIDI_EVENT_BYTES
}

/// USB-MIDI wire bytes one batch may still spend.
#[derive(Debug)]
pub(super) struct WireBudget {
    remaining: usize,
    spent: usize,
}

impl WireBudget {
    pub(super) const fn new(remaining: usize) -> Self {
        Self {
            remaining,
            spent: 0,
        }
    }

    pub(super) const fn remaining(&self) -> usize {
        self.remaining
    }

    pub(super) const fn spent(&self) -> usize {
        self.spent
    }

    /// Spend the wire cost of a `message_len`-byte MIDI message, or refuse
    /// without spending anything when it does not fit.
    pub(super) fn try_spend(&mut self, message_len: usize) -> bool {
        let cost = usb_midi_wire_bytes(message_len);
        if cost > self.remaining {
            return false;
        }
        self.remaining -= cost;
        self.spent += cost;
        true
    }
}

/// Link bookkeeping for the LED lane.
#[derive(Debug, Default)]
pub(super) struct Push2Link {
    /// Wire bytes sent since the last acknowledged request.
    unconfirmed_wire_bytes: usize,
    /// Palette index of the acknowledgement request still awaiting a reply.
    awaiting_ack: Option<u8>,
    /// Next palette index to read back as an acknowledgement request.
    ack_cursor: u8,
    last_resync_at: Option<Instant>,
    last_mode_assert_at: Option<Instant>,
    stall: Option<Push2Stall>,
    /// First LED the next batch serves, so a budget-limited batch rotates
    /// through every LED instead of starving the tail of the list.
    pub(super) led_scan_offset: usize,
    /// First RGB LED the next batch assigns palette writes to.
    pub(super) palette_scan_offset: usize,
}

#[derive(Debug, Clone, Copy)]
struct Push2Stall {
    since: Instant,
    probes: u32,
    next_probe_at: Instant,
    last_report_at: Instant,
}

/// Whether a batch may carry LED traffic.
pub(super) enum BatchStart {
    /// The previous acknowledgement request is unanswered; only a probe (or
    /// nothing, between probes) goes out.
    Hold,
    /// Normal batch with this much wire budget for LED traffic.
    Proceed(WireBudget),
}

/// Open a batch: hold while an acknowledgement is outstanding, otherwise
/// schedule any due resync or mode re-assert and hand back the budget.
pub(super) fn begin_batch(
    state: &mut Push2State,
    now: Instant,
    buffer: &mut CommandBuffer<'_>,
) -> BatchStart {
    if state.link.awaiting_ack.is_some() {
        hold_for_ack(state, now, buffer);
        return BatchStart::Hold;
    }

    // The init sequence already cleared every LED and set User mode, so the
    // first frame starts both clocks instead of repeating that work.
    match state.link.last_resync_at {
        None => state.link.last_resync_at = Some(now),
        Some(at) if now.saturating_duration_since(at) >= PUSH2_RESYNC_INTERVAL => {
            state.link.last_resync_at = Some(now);
            schedule_resync(state);
        }
        Some(_) => {}
    }

    let mut budget = WireBudget::new(
        PUSH2_MIDI_ENDPOINT_PACKET_BYTES
            .saturating_sub(state.link.unconfirmed_wire_bytes)
            .saturating_sub(usb_midi_wire_bytes(ACK_REQUEST_LEN)),
    );

    let mode_due = match state.link.last_mode_assert_at {
        None => {
            state.link.last_mode_assert_at = Some(now);
            false
        }
        Some(at) => now.saturating_duration_since(at) >= PUSH2_MODE_ASSERT_INTERVAL,
    };
    if mode_due {
        let set_mode = mode_assert_message();
        let strip_config = strip_config_message();
        if budget.try_spend(set_mode.len()) && budget.try_spend(strip_config.len()) {
            // Set MIDI Mode replies; waiting for that reply keeps the command
            // from nesting with the next one, per the interface manual. A
            // quiet device must not fail the batch, so the reply is optional.
            buffer
                .push_slice(
                    &set_mode,
                    true,
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                )
                .response
                .tolerance = ResponseTolerance::Optional;
            buffer.push_slice(
                &strip_config,
                false,
                Duration::ZERO,
                Duration::ZERO,
                TransferType::Primary,
            );
            state.link.last_mode_assert_at = Some(now);
        }
    }

    BatchStart::Proceed(budget)
}

/// Close a batch: account for what it spent and request an acknowledgement
/// when the batch left work behind or the unconfirmed window is half full.
pub(super) fn finish_batch(
    state: &mut Push2State,
    budget: &WireBudget,
    deferred: bool,
    buffer: &mut CommandBuffer<'_>,
) {
    let spent = budget.spent();
    if spent == 0 && !deferred {
        return;
    }
    state.link.unconfirmed_wire_bytes += spent;
    if deferred || state.link.unconfirmed_wire_bytes >= ACK_WATERMARK_BYTES {
        push_ack_request(state, buffer);
    }
}

/// Handle a palette read reply. Returns `true` when it answered the
/// outstanding acknowledgement request.
///
/// The reply reports what the device holds after processing everything sent
/// before the request, so a mismatch with the believed palette is a lost
/// write; adopting the device value makes the next batch rewrite it.
pub(super) fn acknowledge(state: &mut Push2State, index: u8, entry: [u8; 4], now: Instant) -> bool {
    if state.link.awaiting_ack != Some(index) {
        return false;
    }
    state.link.awaiting_ack = None;
    state.link.unconfirmed_wire_bytes = 0;

    let slot = usize::from(index);
    if state.palette[slot] != entry {
        debug!(
            slot,
            believed = ?state.palette[slot],
            device = ?entry,
            "push2 palette readback differs from the believed entry; rewriting it"
        );
        state.palette[slot] = entry;
    }
    state.force_palette[slot] = false;

    if let Some(stall) = state.link.stall.take() {
        info!(
            stalled_ms = u64::try_from(now.saturating_duration_since(stall.since).as_millis())
                .unwrap_or(u64::MAX),
            probes = stall.probes,
            "Push 2 acknowledged MIDI again; resyncing every LED"
        );
    }
    true
}

/// Split a one-shot burst (the init clear, the shutdown restore) into chunks
/// of at most one endpoint packet, each closed by a palette read the backend
/// must see answered before it sends the next chunk.
///
/// A command that already expects a reply confirms everything before it, so
/// it closes its chunk too.
pub(super) fn chunk_with_ack_reads(burst: Vec<ProtocolCommand>) -> Vec<ProtocolCommand> {
    let ack_cost = usb_midi_wire_bytes(ACK_REQUEST_LEN);
    let limit = PUSH2_MIDI_ENDPOINT_PACKET_BYTES - ack_cost;
    let mut chunked = Vec::with_capacity(burst.len() + burst.len() / 64 + 2);
    let mut unconfirmed = 0_usize;
    for command in burst {
        let cost = usb_midi_wire_bytes(command.data.len());
        if unconfirmed > 0 && unconfirmed + cost > limit {
            chunked.push(ack_read_command());
            unconfirmed = 0;
        }
        unconfirmed = if command.expects_response {
            0
        } else {
            unconfirmed + cost
        };
        chunked.push(command);
    }
    if unconfirmed > 0 {
        chunked.push(ack_read_command());
    }
    chunked
}

fn ack_read_command() -> ProtocolCommand {
    ProtocolCommand {
        data: ack_request_message(0).to_vec(),
        expects_response: true,
        transfer_type: TransferType::Primary,
        ..ProtocolCommand::default()
    }
}

/// Forget everything about the link, as after a fresh init.
pub(super) fn reset(state: &mut Push2State) {
    state.link = Push2Link::default();
}

fn hold_for_ack(state: &mut Push2State, now: Instant, buffer: &mut CommandBuffer<'_>) {
    let mut stall = if let Some(stall) = state.link.stall {
        stall
    } else {
        warn!(
            unconfirmed_wire_bytes = state.link.unconfirmed_wire_bytes,
            "Push 2 stopped acknowledging MIDI; holding LED output and probing until it answers"
        );
        // Whatever the unanswered traffic did is unknown now, so the whole
        // believed state is re-sent once the device answers again.
        schedule_rollback(state);
        Push2Stall {
            since: now,
            probes: 0,
            next_probe_at: now,
            last_report_at: now,
        }
    };

    if now >= stall.next_probe_at {
        push_ack_request(state, buffer);
        stall.probes = stall.probes.saturating_add(1);
        stall.next_probe_at = now + probe_backoff(stall.probes);
        debug!(probes = stall.probes, "push2 acknowledgement probe sent");
    }

    if now.saturating_duration_since(stall.last_report_at) >= STALL_REPORT_INTERVAL {
        warn!(
            stalled_ms = u64::try_from(now.saturating_duration_since(stall.since).as_millis())
                .unwrap_or(u64::MAX),
            probes = stall.probes,
            "Push 2 is still not acknowledging MIDI; power-cycle it if this persists"
        );
        stall.last_report_at = now;
    }

    state.link.stall = Some(stall);
}

fn probe_backoff(probes: u32) -> Duration {
    let doublings = probes.saturating_sub(1).min(16);
    STALL_PROBE_BACKOFF_MIN
        .saturating_mul(1_u32 << doublings)
        .min(STALL_PROBE_BACKOFF_MAX)
}

fn push_ack_request(state: &mut Push2State, buffer: &mut CommandBuffer<'_>) {
    let index = state.link.ack_cursor;
    state.link.ack_cursor = (index + 1) % u8::try_from(PUSH2_PALETTE_SIZE).unwrap_or(u8::MAX);

    let message = ack_request_message(index);
    // The reply is optional to the backend so a silent device never fails
    // the frame; the protocol notices the missing reply itself and holds.
    buffer
        .push_slice(
            &message,
            true,
            Duration::ZERO,
            Duration::ZERO,
            TransferType::Primary,
        )
        .response
        .tolerance = ResponseTolerance::Optional;
    state.link.awaiting_ack = Some(index);
    state.link.unconfirmed_wire_bytes += usb_midi_wire_bytes(message.len());
}

/// Re-send the believed state: every LED index, the touch strip, and every
/// palette slot an LED currently shows. Nothing is reassigned, so a resync
/// is invisible on a healthy device.
fn schedule_resync(state: &mut Push2State) {
    state.force_leds.fill(true);
    state.force_touch_strip = true;
    for slot in state.prev_led_indices {
        if slot != 0 {
            state.force_palette[usize::from(slot)] = true;
        }
    }
}

/// Distrust every palette slot as well, since an unanswered batch may have
/// left any slot it wrote in either its old or new state.
fn schedule_rollback(state: &mut Push2State) {
    state.force_leds.fill(true);
    state.force_touch_strip = true;
    state.force_palette[1..].fill(true);
}

fn ack_request_message(index: u8) -> [u8; ACK_REQUEST_LEN] {
    let mut message = [0_u8; ACK_REQUEST_LEN];
    message[..6].copy_from_slice(&PUSH2_MANUFACTURER_PREFIX);
    message[6] = PUSH2_CMD_GET_PALETTE_ENTRY;
    message[7] = index;
    message[8] = 0xF7;
    message
}

fn mode_assert_message() -> [u8; 9] {
    let mut message = [0_u8; 9];
    message[..6].copy_from_slice(&PUSH2_MANUFACTURER_PREFIX);
    message[6] = PUSH2_CMD_SET_MIDI_MODE;
    message[7] = PUSH2_MIDI_MODE_USER;
    message[8] = 0xF7;
    message
}

fn strip_config_message() -> [u8; 9] {
    let mut message = [0_u8; 9];
    message[..6].copy_from_slice(&PUSH2_MANUFACTURER_PREFIX);
    message[6] = PUSH2_CMD_SET_TOUCH_STRIP_CONFIG;
    message[7] = PUSH2_TOUCH_STRIP_HOST_CONFIG;
    message[8] = 0xF7;
    message
}
