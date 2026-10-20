//! Push 2 MIDI input: pads, touch strip, and buttons.
//!
//! In User mode the Push 2 sends what its controls do on the User port, all
//! on MIDI channel 1 (status low nibble 0), per Ableton's Push 2 MIDI and
//! Display Interface Manual:
//!
//! - Pads are notes 36 (bottom left) to 99 (top right). A press is Note On
//!   with its velocity; a release is Note Off. Aftertouch arrives as channel
//!   pressure by default, or as polyphonic key pressure per pad.
//! - The touch strip touch is Note On 12 with velocity 127, released with
//!   velocity 0. Its position is a pitch bend from 0 at the bottom to 16320
//!   at the top (eight significant bits), the mode the driver configures.
//! - Buttons are control changes, 127 pressed and 0 released. Encoders are
//!   control changes too, but relative, so they are not buttons; their
//!   capacitive touches are notes 0 to 10 and are skipped.
//!
//! Touch positions use each segment's LED frame, so a press lands on the LED
//! under it: pad `(column, row)` counts rows from the pad matrix's first LED
//! row, notes 36 to 43, which is the physical bottom row, and the strip runs
//! along `x` from its first LED, also at the bottom.

use std::sync::Arc;

use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::InputButtonState;

use super::{
    PAD_NOTE_MAP, PUSH2_CMD_SET_MIDI_MODE, PUSH2_MANUFACTURER_PREFIX, PUSH2_MIDI_MODE_LIVE,
    PUSH2_PAD_COUNT, PUSH2_PADS_SEGMENT, PUSH2_TOUCH_STRIP_SEGMENT,
};

const NOTE_OFF: u8 = 0x80;
const NOTE_ON: u8 = 0x90;
const POLY_PRESSURE: u8 = 0xA0;
const CONTROL_CHANGE: u8 = 0xB0;
const CHANNEL_PRESSURE: u8 = 0xD0;
const PITCH_BEND: u8 = 0xE0;
const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
const REALTIME_FIRST: u8 = 0xF8;

const TOUCH_STRIP_NOTE: u8 = 12;
/// Contact for the touch strip; pads use their LED index, 0 to 63.
const TOUCH_STRIP_CONTACT: u32 = 64;
/// The strip's resting value, which autoreturn reports on every lift.
const PITCH_BEND_CENTER: u16 = 8192;
/// The strip's top: eight significant bits shifted into fourteen.
const PITCH_BEND_TOP: u16 = 16_320;
const PAD_GRID: u8 = 8;
const MIDI_DATA_MAX: f32 = 127.0;

/// Momentary buttons by control change, named after the Push 2 driver spec's
/// button tables, which follow Ableton's MIDI mapping.
const BUTTON_NAMES: [(u8, &str); 65] = [
    (3, "tap_tempo"),
    (9, "metronome"),
    (20, "select_1"),
    (21, "select_2"),
    (22, "select_3"),
    (23, "select_4"),
    (24, "select_5"),
    (25, "select_6"),
    (26, "select_7"),
    (27, "select_8"),
    (28, "master"),
    (29, "stop_clip"),
    (30, "setup"),
    (31, "layout"),
    (35, "convert"),
    (36, "scene_8"),
    (37, "scene_7"),
    (38, "scene_6"),
    (39, "scene_5"),
    (40, "scene_4"),
    (41, "scene_3"),
    (42, "scene_2"),
    (43, "scene_1"),
    (44, "arrow_left"),
    (45, "arrow_right"),
    (46, "arrow_up"),
    (47, "arrow_down"),
    (48, "select"),
    (49, "shift"),
    (50, "note"),
    (51, "session"),
    (52, "add_device"),
    (53, "add_track"),
    (54, "octave_down"),
    (55, "octave_up"),
    (56, "repeat"),
    (57, "accent"),
    (58, "scale"),
    (59, "user"),
    (60, "mute"),
    (61, "solo"),
    (62, "page_left"),
    (63, "page_right"),
    (85, "play"),
    (86, "record"),
    (87, "new"),
    (88, "duplicate"),
    (89, "automate"),
    (90, "fixed_length"),
    (102, "track_1"),
    (103, "track_2"),
    (104, "track_3"),
    (105, "track_4"),
    (106, "track_5"),
    (107, "track_6"),
    (108, "track_7"),
    (109, "track_8"),
    (110, "device"),
    (111, "browse"),
    (112, "mix"),
    (113, "clip"),
    (116, "quantize"),
    (117, "double_loop"),
    (118, "delete"),
    (119, "undo"),
];

/// Outcome of decoding one MIDI input message.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Push2InputDecode {
    /// Everything decoded, or was not input.
    Intact,
    /// Input was lost: a message could not be decoded, or another host
    /// moved the Push 2 out of User mode so its input stopped reaching this
    /// port. A lift may have gone unseen. The decoder has forgotten every
    /// hold, and the caller cancels the device's holds too.
    Lost,
}

/// Decodes Push 2 MIDI input into device input edges.
///
/// Holds what the device reported pressed so aftertouch updates only held
/// pads and releases only end what began.
pub struct Push2InputDecoder {
    pad_by_note: [Option<u8>; 128],
    button_by_cc: [Option<Arc<str>>; 128],
    pads_segment: Arc<str>,
    strip_segment: Arc<str>,
    held_pads: [bool; PUSH2_PAD_COUNT],
    strip: StripState,
}

#[derive(Debug, Clone, Copy, Default)]
struct StripState {
    touching: bool,
    /// Whether the current touch has begun. A touch waits for its first
    /// position when the strip still reports its resting value.
    began: bool,
    /// Position of the current touch along the strip.
    position: f32,
    /// The value the strip last reported, touched or not.
    reported: Option<u16>,
}

impl Push2InputDecoder {
    /// Build a decoder that holds nothing.
    #[must_use]
    pub fn new() -> Self {
        let mut pad_by_note = [None; 128];
        for (led, note) in (0_u8..).zip(PAD_NOTE_MAP) {
            pad_by_note[usize::from(note)] = Some(led);
        }
        let mut button_by_cc: [Option<Arc<str>>; 128] = std::array::from_fn(|_| None);
        for (cc, name) in BUTTON_NAMES {
            button_by_cc[usize::from(cc)] = Some(Arc::from(name));
        }
        Self {
            pad_by_note,
            button_by_cc,
            pads_segment: Arc::from(PUSH2_PADS_SEGMENT),
            strip_segment: Arc::from(PUSH2_TOUCH_STRIP_SEGMENT),
            held_pads: [false; PUSH2_PAD_COUNT],
            strip: StripState::default(),
        }
    }

    /// Decode the bytes of one MIDI input message, appending edges in the
    /// order the device reported them.
    ///
    /// Native MIDI inputs deliver one whole message per call. Several
    /// messages, running status within them, and interleaved realtime bytes
    /// decode too. A message cut short, or data with no status before it,
    /// ends decoding with [`Push2InputDecode::Lost`]; edges appended before
    /// that point stand.
    pub fn decode(&mut self, bytes: &[u8], edges: &mut Vec<DeviceInputEdge>) -> Push2InputDecode {
        let mut running = None;
        let mut index = 0;
        while let Some(&byte) = bytes.get(index) {
            if byte >= REALTIME_FIRST {
                index += 1;
                continue;
            }
            if byte == SYSEX_START {
                let Some(length) = bytes[index..].iter().position(|&end| end == SYSEX_END) else {
                    // Sysex replies are whole messages; a fragment carries no input.
                    break;
                };
                let message = &bytes[index..=index + length];
                index += length + 1;
                running = None;
                if leaves_user_mode(message) {
                    return self.lose();
                }
                continue;
            }
            let status = if byte & 0x80 != 0 {
                index += 1;
                byte
            } else if let Some(status) = running {
                status
            } else {
                return self.lose();
            };
            let mut data = [0_u8; 2];
            let length = data_length(status);
            let mut filled = 0;
            while filled < length {
                let Some(&next) = bytes.get(index) else {
                    return self.lose();
                };
                index += 1;
                if next >= REALTIME_FIRST {
                    continue;
                }
                if next & 0x80 != 0 {
                    return self.lose();
                }
                data[filled] = next;
                filled += 1;
            }
            if status < SYSEX_START {
                running = Some(status);
                self.channel_message(status, data, edges);
            } else {
                running = None;
            }
        }
        Push2InputDecode::Intact
    }

    fn channel_message(
        &mut self,
        status: u8,
        [first, second]: [u8; 2],
        edges: &mut Vec<DeviceInputEdge>,
    ) {
        // Every Push 2 control reports on the first channel.
        if status & 0x0F != 0 {
            return;
        }
        match status {
            NOTE_ON if second > 0 => self.press(first, second, edges),
            NOTE_ON | NOTE_OFF => self.release(first, edges),
            POLY_PRESSURE => {
                if let Some(pad) = self.held_pad(first) {
                    edges.push(self.pad_edge(pad, second, EdgeKind::Moved));
                }
            }
            CHANNEL_PRESSURE => {
                // The Push 2 reports its hardest-pressed pad; every held pad
                // takes that pressure.
                for (pad, held) in (0_u8..).zip(self.held_pads) {
                    if held {
                        edges.push(self.pad_edge(pad, first, EdgeKind::Moved));
                    }
                }
            }
            CONTROL_CHANGE => {
                if let Some(button) = &self.button_by_cc[usize::from(first)] {
                    edges.push(DeviceInputEdge::Button {
                        button: Arc::clone(button),
                        state: if second > 0 {
                            InputButtonState::Pressed
                        } else {
                            InputButtonState::Released
                        },
                    });
                }
            }
            PITCH_BEND => self.strip_value((u16::from(second) << 7) | u16::from(first), edges),
            _ => {}
        }
    }

    fn press(&mut self, note: u8, velocity: u8, edges: &mut Vec<DeviceInputEdge>) {
        if note == TOUCH_STRIP_NOTE {
            if self.strip.touching {
                return;
            }
            self.strip.touching = true;
            self.strip.began = false;
            // A value other than the resting one arrived ahead of the touch,
            // so it is where the finger landed.
            if let Some(value) = self.strip.reported
                && value != PITCH_BEND_CENTER
            {
                self.begin_strip(strip_position(value), edges);
            }
            return;
        }
        if let Some(pad) = self.pad(note) {
            self.held_pads[usize::from(pad)] = true;
            edges.push(self.pad_edge(pad, velocity, EdgeKind::Began));
        }
    }

    fn release(&mut self, note: u8, edges: &mut Vec<DeviceInputEdge>) {
        if note == TOUCH_STRIP_NOTE {
            if !self.strip.touching {
                return;
            }
            // A touch that never left the resting value lifts where it rests.
            if !self.strip.began {
                self.begin_strip(strip_position(PITCH_BEND_CENTER), edges);
            }
            // The autoreturn value reported just before this lift is not
            // where the finger left, so the touch ends at its last position.
            edges.push(DeviceInputEdge::TouchEnded {
                contact: TOUCH_STRIP_CONTACT,
                position: strip_touch(self.strip.position),
            });
            self.strip.touching = false;
            self.strip.began = false;
            return;
        }
        if let Some(pad) = self.held_pad(note) {
            self.held_pads[usize::from(pad)] = false;
            edges.push(self.pad_edge(pad, 0, EdgeKind::Ended));
        }
    }

    fn strip_value(&mut self, value: u16, edges: &mut Vec<DeviceInputEdge>) {
        self.strip.reported = Some(value);
        // The resting value is ambiguous while touched: autoreturn sends it
        // just before a lift, so it only moves the touch once a later value
        // shows the finger is still down.
        if !self.strip.touching || value == PITCH_BEND_CENTER {
            return;
        }
        let position = strip_position(value);
        if !self.strip.began {
            self.begin_strip(position, edges);
        } else if position != self.strip.position {
            self.strip.position = position;
            edges.push(DeviceInputEdge::TouchMoved {
                contact: TOUCH_STRIP_CONTACT,
                position: strip_touch(position),
            });
        }
    }

    fn begin_strip(&mut self, position: f32, edges: &mut Vec<DeviceInputEdge>) {
        self.strip.began = true;
        self.strip.position = position;
        edges.push(DeviceInputEdge::TouchBegan {
            contact: TOUCH_STRIP_CONTACT,
            position: strip_touch(position),
            segment: Some(Arc::clone(&self.strip_segment)),
        });
    }

    /// Pad LED index for a note. Data bytes are seven bits, so every note
    /// indexes the table.
    fn pad(&self, note: u8) -> Option<u8> {
        self.pad_by_note[usize::from(note)]
    }

    fn held_pad(&self, note: u8) -> Option<u8> {
        self.pad(note)
            .filter(|pad| self.held_pads[usize::from(*pad)])
    }

    fn pad_edge(&self, pad: u8, value: u8, kind: EdgeKind) -> DeviceInputEdge {
        let contact = u32::from(pad);
        let position = TouchPosition {
            x: f32::from(pad % PAD_GRID) / f32::from(PAD_GRID - 1),
            y: f32::from(pad / PAD_GRID) / f32::from(PAD_GRID - 1),
            pressure: f32::from(value) / MIDI_DATA_MAX,
        };
        match kind {
            EdgeKind::Began => DeviceInputEdge::TouchBegan {
                contact,
                position,
                segment: Some(Arc::clone(&self.pads_segment)),
            },
            EdgeKind::Moved => DeviceInputEdge::TouchMoved { contact, position },
            EdgeKind::Ended => DeviceInputEdge::TouchEnded { contact, position },
        }
    }

    fn lose(&mut self) -> Push2InputDecode {
        self.held_pads = [false; PUSH2_PAD_COUNT];
        self.strip = StripState::default();
        Push2InputDecode::Lost
    }
}

impl Default for Push2InputDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
enum EdgeKind {
    Began,
    Moved,
    Ended,
}

/// Data bytes that follow a status byte.
const fn data_length(status: u8) -> usize {
    match status {
        0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 2,
        0xC0..=0xDF | 0xF1 | 0xF3 => 1,
        _ => 0,
    }
}

/// Whether a sysex is the reply to a Set MIDI Mode that selected Live mode.
///
/// The Push 2 sends that reply to both ports whoever asked, and in Live mode
/// its controls report on the Live port only, so this port stops seeing
/// lifts.
fn leaves_user_mode(message: &[u8]) -> bool {
    message.len() == PUSH2_MANUFACTURER_PREFIX.len() + 3
        && message.starts_with(&PUSH2_MANUFACTURER_PREFIX)
        && message[PUSH2_MANUFACTURER_PREFIX.len()] == PUSH2_CMD_SET_MIDI_MODE
        && message[PUSH2_MANUFACTURER_PREFIX.len() + 1] == PUSH2_MIDI_MODE_LIVE
}

fn strip_position(value: u16) -> f32 {
    (f32::from(value) / f32::from(PITCH_BEND_TOP)).min(1.0)
}

/// A strip touch at `position` along the strip, on its center line. The
/// strip senses no pressure, so a touch is a full press.
const fn strip_touch(position: f32) -> TouchPosition {
    TouchPosition {
        x: position,
        y: 0.5,
        pressure: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::{RGB_BUTTON_CC_MAP, WHITE_BUTTON_CC_MAP};
    use super::BUTTON_NAMES;

    #[test]
    fn every_button_with_an_led_has_a_name_and_nothing_else_does() {
        let lit: BTreeSet<u8> = RGB_BUTTON_CC_MAP
            .into_iter()
            .chain(WHITE_BUTTON_CC_MAP)
            .collect();
        let named: BTreeSet<u8> = BUTTON_NAMES.iter().map(|(cc, _)| *cc).collect();
        assert_eq!(named.len(), BUTTON_NAMES.len(), "no CC is named twice");
        assert_eq!(named, lit);
        let names: BTreeSet<&str> = BUTTON_NAMES.iter().map(|(_, name)| *name).collect();
        assert_eq!(names.len(), BUTTON_NAMES.len(), "no name is used twice");
    }
}
