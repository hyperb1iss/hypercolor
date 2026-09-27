//! In-process Push 2 stand-in for LED flow-control tests.
//!
//! `FakePush2` applies MIDI the way Ableton's Push 2 interface manual
//! describes it (palette-indexed LEDs, Reapply Color Palette, in-order
//! replies to palette reads) and can stall like a wedged endpoint or drop a
//! message like a lossy OS buffer. `Push2Rig` plays the USB actor: it encodes
//! the newest frame, delivers each command in order with the transport's
//! inter-message spacing on a simulated clock, and hands replies back to the
//! protocol. Nothing here touches real USB or MIDI.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the rig"
)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hypercolor_hal::drivers::push2::Push2Protocol;
use hypercolor_hal::protocol::{Protocol, ProtocolCommand, ResponseTolerance};

pub const LED_COUNT: usize = 160;
pub const RGB_LED_COUNT: usize = 92;
pub const MIDI_LED_COUNT: usize = 129;
pub const FRAME_PERIOD: Duration = Duration::from_micros(16_667);
/// Response budget the USB actor uses for Push 2 replies.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
/// Deadline the rawmidi writer gives one message.
pub const WRITE_DEADLINE: Duration = Duration::from_secs(1);
/// Kernel rawmidi output buffer size on Linux.
pub const KERNEL_BUFFER_BYTES: usize = 4096;
/// Bulk OUT max packet of the Push 2 MIDI streaming interface.
pub const ENDPOINT_PACKET_BYTES: usize = 512;

const PAD_NOTE_BASE: u8 = 36;
const RGB_BUTTON_CCS: [u8; 28] = [
    102, 103, 104, 105, 106, 107, 108, 109, 20, 21, 22, 23, 24, 25, 26, 27, 43, 42, 41, 40, 39, 38,
    37, 36, 85, 86, 3, 9,
];
const WHITE_BUTTON_CCS: [u8; 37] = [
    28, 29, 30, 31, 35, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 59, 61, 62, 63, 87, 88, 89,
    90, 110, 111, 112, 113, 116, 117, 118, 119, 56, 57, 58, 60,
];
const PREFIX: [u8; 6] = [0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];

/// USB-MIDI 1.0 wire bytes for one MIDI message.
#[must_use]
pub fn wire_bytes(message: &[u8]) -> usize {
    message.len().div_ceil(3) * 4
}

/// Inter-message spacing the Push 2 transport applies before each send.
#[must_use]
pub fn transport_spacing(message: &[u8]) -> Duration {
    if message.len() <= 3 {
        Duration::from_micros(500)
    } else {
        Duration::from_millis(1)
    }
}

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

    pub fn advance_to(&self, at: Duration) {
        let target = self.origin + at;
        let mut now = self.now.lock().expect("sim clock lock");
        if *now < target {
            *now = target;
        }
    }
}

impl Default for SimClock {
    fn default() -> Self {
        Self::new()
    }
}

/// One message the host handed to the device.
#[derive(Debug, Clone)]
pub struct Sent {
    pub at: Duration,
    pub bytes: Vec<u8>,
    pub expects_response: bool,
    pub optional_response: bool,
}

/// Device-side model of the Push 2 LED engine.
pub struct FakePush2 {
    pub palette: [[u8; 4]; 128],
    /// Palette index each MIDI LED is set to (pads, RGB buttons, white).
    pub led_index: [u8; MIDI_LED_COUNT],
    /// RGBW each MIDI LED is actually lit with: latched from the palette on
    /// its note/CC, and refreshed only by Reapply Color Palette.
    pub lit: [[u8; 4]; MIDI_LED_COUNT],
    pub touch_strip: Option<Vec<u8>>,
    pub midi_mode: u8,
    /// Endpoint stops draining: sends queue in the kernel buffer.
    pub stalled: bool,
    /// Drop the next note/CC silently, as an overrun OS buffer would.
    pub drop_next_led_message: bool,
    pub backlog: VecDeque<Vec<u8>>,
    pub sent: Vec<Sent>,
    /// Wire bytes accepted since the last answered palette read.
    pub unconfirmed_wire_bytes: usize,
    pub max_unconfirmed_wire_bytes: usize,
    pub answered_acks: usize,
}

impl FakePush2 {
    #[must_use]
    pub fn new() -> Self {
        let mut palette = [[0_u8; 4]; 128];
        // A recognizable factory palette: every slot distinct and non-black
        // except slot 0, so restores and readbacks are observable.
        for (index, entry) in palette.iter_mut().enumerate().skip(1) {
            let value = u8::try_from(index).expect("palette index fits in u8");
            *entry = [value, 255 - value, value / 2, value];
        }
        Self {
            palette,
            led_index: [0; MIDI_LED_COUNT],
            lit: [[0; 4]; MIDI_LED_COUNT],
            touch_strip: None,
            midi_mode: 0,
            stalled: false,
            drop_next_led_message: false,
            backlog: VecDeque::new(),
            sent: Vec::new(),
            unconfirmed_wire_bytes: 0,
            max_unconfirmed_wire_bytes: 0,
            answered_acks: 0,
        }
    }

    #[must_use]
    pub fn backlog_bytes(&self) -> usize {
        self.backlog.iter().map(Vec::len).sum()
    }

    /// Resume draining: the device works through its backlog in order.
    pub fn unstall(&mut self) {
        self.stalled = false;
        while let Some(message) = self.backlog.pop_front() {
            let _ = self.process(&message);
        }
    }

    /// RGB each RGB LED is lit with.
    #[must_use]
    pub fn lit_rgb(&self, led: usize) -> [u8; 3] {
        let [r, g, b, _] = self.lit[led];
        [r, g, b]
    }

    /// Process one message; returns a reply when the command has one.
    fn process(&mut self, message: &[u8]) -> Option<Vec<u8>> {
        match message.first().copied() {
            Some(0x90) => {
                let led = usize::from(message[1].checked_sub(PAD_NOTE_BASE)?);
                self.set_led(led, message[2]);
                None
            }
            Some(0xB0) => {
                let cc = message[1];
                let led = RGB_BUTTON_CCS
                    .iter()
                    .position(|candidate| *candidate == cc)
                    .map(|position| 64 + position)
                    .or_else(|| {
                        WHITE_BUTTON_CCS
                            .iter()
                            .position(|candidate| *candidate == cc)
                            .map(|position| RGB_LED_COUNT + position)
                    })?;
                self.set_led(led, message[2]);
                None
            }
            Some(0xF0) if message.len() >= 8 && message[..6] == PREFIX => {
                self.process_sysex(message[6], &message[7..message.len() - 1])
            }
            Some(0xF0) if message == [0xF0, 0x7E, 0x01, 0x06, 0x01, 0xF7] => Some(vec![
                0xF0, 0x7E, 0x01, 0x06, 0x02, 0x00, 0x21, 0x1D, 0x67, 0x32, 0x02, 0x00, 0x01, 0x00,
                0x2F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xF7,
            ]),
            _ => None,
        }
    }

    fn set_led(&mut self, led: usize, index: u8) {
        self.led_index[led] = index;
        self.lit[led] = self.palette[usize::from(index)];
    }

    fn process_sysex(&mut self, command: u8, args: &[u8]) -> Option<Vec<u8>> {
        match command {
            0x03 => {
                let index = usize::from(args[0]);
                let mut entry = [0_u8; 4];
                for (channel, value) in entry.iter_mut().enumerate() {
                    *value = args[1 + channel * 2] | (args[2 + channel * 2] << 7);
                }
                self.palette[index] = entry;
                None
            }
            0x04 => {
                let index = args[0];
                let entry = self.palette[usize::from(index)];
                let mut reply = PREFIX.to_vec();
                reply.push(0x04);
                reply.push(index);
                for value in entry {
                    reply.push(value & 0x7F);
                    reply.push(value >> 7);
                }
                reply.push(0xF7);
                Some(reply)
            }
            0x05 => {
                for led in 0..MIDI_LED_COUNT {
                    self.lit[led] = self.palette[usize::from(self.led_index[led])];
                }
                None
            }
            0x0A => {
                self.midi_mode = args[0];
                let mut reply = PREFIX.to_vec();
                reply.extend_from_slice(&[0x0A, args[0], 0xF7]);
                Some(reply)
            }
            0x19 => {
                self.touch_strip = Some(args.to_vec());
                None
            }
            _ => None,
        }
    }
}

impl Default for FakePush2 {
    fn default() -> Self {
        Self::new()
    }
}

/// What one actor pass put on the wire.
#[derive(Debug, Default, Clone)]
pub struct Pass {
    pub commands: usize,
    pub wire_bytes: usize,
    pub failed: bool,
}

/// Why a delivery stopped early.
#[derive(Debug)]
pub struct WriteTimeout;

/// USB actor stand-in driving `Push2Protocol` into `FakePush2`.
pub struct Push2Rig {
    pub clock: SimClock,
    pub protocol: Push2Protocol,
    pub device: FakePush2,
    pub commands: Vec<ProtocolCommand>,
    last_frame_index: Option<u64>,
}

impl Push2Rig {
    /// Build a rig and run the protocol's init sequence against the fake.
    #[must_use]
    pub fn connected() -> Self {
        let clock = SimClock::new();
        let protocol_clock = clock.clone();
        let protocol = Push2Protocol::with_clock(Arc::new(move || protocol_clock.now()));
        let mut rig = Self {
            clock,
            protocol,
            device: FakePush2::new(),
            commands: Vec::new(),
            last_frame_index: None,
        };
        let init = rig.protocol.init_sequence();
        rig.deliver(&init)
            .expect("init should complete against a healthy fake");
        rig
    }

    /// One actor pass: encode `frame` and deliver whatever it produced.
    pub fn pump(&mut self, frame: &[[u8; 3]]) -> Pass {
        self.protocol.encode_frame_into(frame, &mut self.commands);
        let commands = std::mem::take(&mut self.commands);
        let wire_bytes = commands
            .iter()
            .map(|command| wire_bytes(&command.data))
            .sum();
        let failed = self.deliver(&commands).is_err();
        let pass = Pass {
            commands: commands.len(),
            wire_bytes,
            failed,
        };
        self.commands = commands;
        pass
    }

    /// Run the actor loop for `duration` of simulated time against a frame
    /// source published at 60 fps. Like the watch channel feeding the real
    /// actor, only the newest frame is ever encoded; frames published while
    /// a batch is on the wire are superseded, never queued.
    pub fn run_for(&mut self, duration: Duration, mut frame: impl FnMut(u64) -> Vec<[u8; 3]>) {
        let end = self.clock.elapsed() + duration;
        while self.clock.elapsed() < end {
            let now_index = frame_index_at(self.clock.elapsed());
            let next_index = match self.last_frame_index {
                Some(last) if last >= now_index => last + 1,
                _ => now_index,
            };
            let publish_at = frame_time(next_index);
            if publish_at >= end {
                self.clock.advance_to(end);
                break;
            }
            self.clock.advance_to(publish_at);
            self.last_frame_index = Some(next_index);
            let colors = frame(next_index);
            let _ = self.pump(&colors);
        }
    }

    /// Deliver commands in order the way the USB actor does: stop at the
    /// first failed write, wait for replies, and treat a missing optional
    /// reply as a completed command.
    pub fn deliver(&mut self, commands: &[ProtocolCommand]) -> Result<(), WriteTimeout> {
        for command in commands {
            self.clock.advance(transport_spacing(&command.data));
            let optional = command.response.tolerance == ResponseTolerance::Optional;
            self.device.sent.push(Sent {
                at: self.clock.elapsed(),
                bytes: command.data.clone(),
                expects_response: command.expects_response,
                optional_response: optional,
            });

            if self.device.stalled {
                if self.device.backlog_bytes() + command.data.len() > KERNEL_BUFFER_BYTES {
                    self.clock.advance(WRITE_DEADLINE);
                    return Err(WriteTimeout);
                }
                self.device.backlog.push_back(command.data.clone());
                if command.expects_response {
                    self.clock.advance(RESPONSE_TIMEOUT);
                    assert!(optional, "a required reply timed out in the rig");
                }
                continue;
            }

            self.device.unconfirmed_wire_bytes += wire_bytes(&command.data);
            self.device.max_unconfirmed_wire_bytes = self
                .device
                .max_unconfirmed_wire_bytes
                .max(self.device.unconfirmed_wire_bytes);

            let is_led_message = matches!(command.data.first(), Some(0x90 | 0xB0));
            if is_led_message && self.device.drop_next_led_message {
                self.device.drop_next_led_message = false;
                continue;
            }

            let reply = self.device.process(&command.data);
            if !command.expects_response {
                continue;
            }
            if let Some(reply) = reply {
                if command.data.get(6) == Some(&0x04) {
                    self.device.unconfirmed_wire_bytes = 0;
                    self.device.answered_acks += 1;
                }
                self.protocol
                    .parse_response(&reply)
                    .expect("fake replies should parse");
            } else {
                self.clock.advance(RESPONSE_TIMEOUT);
                assert!(optional, "a required reply never came in the rig");
            }
        }
        Ok(())
    }

    /// Messages sent at or after `since`.
    #[must_use]
    pub fn sent_since(&self, since: Duration) -> Vec<Sent> {
        self.device
            .sent
            .iter()
            .filter(|sent| sent.at >= since)
            .cloned()
            .collect()
    }
}

fn frame_index_at(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_nanos() / FRAME_PERIOD.as_nanos()).unwrap_or(u64::MAX)
}

fn frame_time(index: u64) -> Duration {
    FRAME_PERIOD.saturating_mul(u32::try_from(index).unwrap_or(u32::MAX))
}

/// HSV to RGB for synthetic effects.
#[must_use]
pub fn hsv(hue: f32, saturation: f32, value: f32) -> [u8; 3] {
    let chroma = value * saturation;
    let sector = hue.rem_euclid(1.0) * 6.0;
    let secondary = chroma * (1.0 - ((sector % 2.0) - 1.0).abs());
    let (red, green, blue) = if sector < 1.0 {
        (chroma, secondary, 0.0)
    } else if sector < 2.0 {
        (secondary, chroma, 0.0)
    } else if sector < 3.0 {
        (0.0, chroma, secondary)
    } else if sector < 4.0 {
        (0.0, secondary, chroma)
    } else if sector < 5.0 {
        (secondary, 0.0, chroma)
    } else {
        (chroma, 0.0, secondary)
    };
    let floor = value - chroma;
    [
        to_byte(red + floor),
        to_byte(green + floor),
        to_byte(blue + floor),
    ]
}

fn to_byte(value: f32) -> u8 {
    let scaled = (value.clamp(0.0, 1.0) * 255.0).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is clamped to 0..=255 before the cast"
    )]
    {
        scaled as u8
    }
}

/// A rainbow sweeping across all 160 LEDs: every RGB LED changes every
/// frame and no two share a color, the worst case for palette churn.
#[must_use]
pub fn rainbow_sweep(frame: u64) -> Vec<[u8; 3]> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "synthetic effect timing tolerates f32 precision"
    )]
    let phase = frame as f32 / 90.0;
    (0..LED_COUNT)
        .map(|led| {
            #[expect(clippy::cast_precision_loss, reason = "LED index is at most 160")]
            let offset = led as f32 / 64.0;
            hsv(offset + phase, 1.0, 1.0)
        })
        .collect()
}

/// A static frame with distinct colors on every RGB LED.
#[must_use]
pub fn static_gradient() -> Vec<[u8; 3]> {
    (0..LED_COUNT)
        .map(|led| {
            #[expect(clippy::cast_precision_loss, reason = "LED index is at most 160")]
            let hue = led as f32 / 160.0;
            hsv(hue, 0.85, 0.9)
        })
        .collect()
}
