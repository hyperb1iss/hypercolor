//! Push 2 MIDI input decoding and its relay to a host input sink.
//!
//! Messages follow Ableton's Push 2 MIDI and Display Interface Manual: pads
//! are notes 36 to 99, the touch strip is note 12 plus pitch bend, buttons
//! and encoders are control changes, and aftertouch is channel or poly
//! pressure.

use std::sync::{Arc, Mutex};

use hypercolor_hal::drivers::push2::{
    PUSH2_PADS_SEGMENT, PUSH2_TOUCH_STRIP_SEGMENT, Push2InputDecode, Push2InputDecoder,
    Push2InputRelay, descriptors,
};
use hypercolor_hal::registry::TransportType;
use hypercolor_hal::transport::input::TransportInputSink;
use hypercolor_types::device_input::{DeviceInputEdge, TouchPosition};
use hypercolor_types::event::InputButtonState;

const NOTE_ON: u8 = 0x90;
const NOTE_OFF: u8 = 0x80;
const CC: u8 = 0xB0;
const STRIP: u8 = 12;
const STRIP_CONTACT: u32 = 64;
/// Pitch bend data bytes (least significant first) for a 14-bit value.
fn bend(value: u16) -> [u8; 3] {
    [
        0xE0,
        u8::try_from(value & 0x7F).expect("seven bits"),
        u8::try_from(value >> 7).expect("seven bits"),
    ]
}

fn decode(decoder: &mut Push2InputDecoder, bytes: &[u8]) -> Vec<DeviceInputEdge> {
    let mut edges = Vec::new();
    assert_eq!(
        decoder.decode(bytes, &mut edges),
        Push2InputDecode::Intact,
        "{bytes:02X?} should decode"
    );
    edges
}

fn pad_at(x: f32, y: f32, pressure: f32) -> TouchPosition {
    TouchPosition { x, y, pressure }
}

fn pad_began(contact: u32, position: TouchPosition) -> DeviceInputEdge {
    DeviceInputEdge::TouchBegan {
        contact,
        position,
        segment: Some(Arc::from(PUSH2_PADS_SEGMENT)),
    }
}

fn strip_at(along: f32) -> TouchPosition {
    TouchPosition {
        x: along,
        y: 0.5,
        pressure: 1.0,
    }
}

fn strip_began(along: f32) -> DeviceInputEdge {
    DeviceInputEdge::TouchBegan {
        contact: STRIP_CONTACT,
        position: strip_at(along),
        segment: Some(Arc::from(PUSH2_TOUCH_STRIP_SEGMENT)),
    }
}

fn button(name: &str, state: InputButtonState) -> DeviceInputEdge {
    DeviceInputEdge::Button {
        button: Arc::from(name),
        state,
    }
}

#[test]
fn pads_press_in_their_led_frame_with_velocity_as_pressure() {
    let mut decoder = Push2InputDecoder::new();

    // Bottom left is the matrix's first LED, top right its last.
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, 36, 127]),
        vec![pad_began(0, pad_at(0.0, 0.0, 1.0))]
    );
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, 43, 1]),
        vec![pad_began(7, pad_at(1.0, 0.0, 1.0 / 127.0))]
    );
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, 92, 64]),
        vec![pad_began(56, pad_at(0.0, 1.0, 64.0 / 127.0))]
    );
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, 99, 127]),
        vec![pad_began(63, pad_at(1.0, 1.0, 1.0))]
    );
    // Note 63 is row 3, column 3.
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, 63, 127]),
        vec![pad_began(27, pad_at(3.0 / 7.0, 3.0 / 7.0, 1.0))]
    );
}

#[test]
fn note_off_and_zero_velocity_note_on_both_release_a_pad() {
    let mut decoder = Push2InputDecoder::new();
    decode(&mut decoder, &[NOTE_ON, 36, 100]);
    decode(&mut decoder, &[NOTE_ON, 37, 100]);

    assert_eq!(
        decode(&mut decoder, &[NOTE_OFF, 36, 0]),
        vec![DeviceInputEdge::TouchEnded {
            contact: 0,
            position: pad_at(0.0, 0.0, 0.0),
        }]
    );
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, 37, 0]),
        vec![DeviceInputEdge::TouchEnded {
            contact: 1,
            position: pad_at(1.0 / 7.0, 0.0, 0.0),
        }]
    );
    // A pad already up, or never seen going down, has nothing to end.
    assert!(decode(&mut decoder, &[NOTE_OFF, 36, 0]).is_empty());
    assert!(decode(&mut decoder, &[NOTE_OFF, 50, 64]).is_empty());
}

#[test]
fn aftertouch_swells_held_pads_above_their_strike_and_never_below_it() {
    let mut decoder = Push2InputDecoder::new();
    decode(&mut decoder, &[NOTE_ON, 36, 100]);
    decode(&mut decoder, &[NOTE_ON, 99, 40]);
    let moved = |contact, x, y, value: f32| DeviceInputEdge::TouchMoved {
        contact,
        position: pad_at(x, y, value / 127.0),
    };

    // Poly pressure moves only its own pad, and only while it is held.
    assert_eq!(
        decode(&mut decoder, &[0xA0, 99, 127]),
        vec![moved(63, 1.0, 1.0, 127.0)]
    );
    assert_eq!(
        decode(&mut decoder, &[0xA0, 99, 20]),
        vec![moved(63, 1.0, 1.0, 40.0)],
        "easing off settles back to the strike, not below it"
    );
    assert!(decode(&mut decoder, &[0xA0, 99, 10]).is_empty());
    assert!(
        decode(&mut decoder, &[0xA0, 40, 127]).is_empty(),
        "a pad that is not held has no pressure to report"
    );

    // Channel pressure reaches every held pad through the same floor, so
    // the zero sent once a strike settles leaves both at their strikes.
    assert!(decode(&mut decoder, &[0xD0, 0]).is_empty());
    assert_eq!(
        decode(&mut decoder, &[0xD0, 110]),
        vec![moved(0, 0.0, 0.0, 110.0), moved(63, 1.0, 1.0, 110.0),]
    );
    assert_eq!(
        decode(&mut decoder, &[0xD0, 0]),
        vec![moved(0, 0.0, 0.0, 100.0), moved(63, 1.0, 1.0, 40.0)]
    );

    decode(&mut decoder, &[NOTE_OFF, 36, 0]);
    decode(&mut decoder, &[NOTE_OFF, 99, 0]);
    assert!(decode(&mut decoder, &[0xD0, 90]).is_empty());
}

#[test]
fn a_strip_touch_begins_at_its_first_position_and_ends_where_the_finger_left() {
    let mut decoder = Push2InputDecoder::new();

    // The strip still reports nothing, so the touch waits for a position.
    assert!(decode(&mut decoder, &[NOTE_ON, STRIP, 127]).is_empty());
    assert_eq!(decode(&mut decoder, &bend(4_080)), vec![strip_began(0.25)]);
    assert_eq!(
        decode(&mut decoder, &bend(12_240)),
        vec![DeviceInputEdge::TouchMoved {
            contact: STRIP_CONTACT,
            position: strip_at(0.75),
        }]
    );
    // Autoreturn reports the center just before the lift.
    assert!(decode(&mut decoder, &bend(8_192)).is_empty());
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, STRIP, 0]),
        vec![DeviceInputEdge::TouchEnded {
            contact: STRIP_CONTACT,
            position: strip_at(0.75),
        }]
    );

    // The next touch lands with the strip resting at center again, and the
    // first real value places it.
    assert!(decode(&mut decoder, &[NOTE_ON, STRIP, 127]).is_empty());
    assert_eq!(decode(&mut decoder, &bend(0)), vec![strip_began(0.0)]);
    assert_eq!(
        decode(&mut decoder, &bend(16_320)),
        vec![DeviceInputEdge::TouchMoved {
            contact: STRIP_CONTACT,
            position: strip_at(1.0),
        }]
    );
    assert_eq!(
        decode(&mut decoder, &[NOTE_OFF, STRIP, 0]),
        vec![DeviceInputEdge::TouchEnded {
            contact: STRIP_CONTACT,
            position: strip_at(1.0),
        }]
    );
}

#[test]
fn a_strip_position_reported_ahead_of_the_touch_places_it_at_once() {
    let mut decoder = Push2InputDecoder::new();

    assert!(decode(&mut decoder, &bend(4_080)).is_empty());
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, STRIP, 127]),
        vec![strip_began(0.25)]
    );
}

#[test]
fn a_strip_touch_that_never_leaves_the_center_lifts_there() {
    let mut decoder = Push2InputDecoder::new();
    decode(&mut decoder, &bend(8_192));

    assert!(decode(&mut decoder, &[NOTE_ON, STRIP, 127]).is_empty());
    assert!(decode(&mut decoder, &bend(8_192)).is_empty());
    let center = 8_192.0 / 16_320.0;
    assert_eq!(
        decode(&mut decoder, &[NOTE_ON, STRIP, 0]),
        vec![
            strip_began(center),
            DeviceInputEdge::TouchEnded {
                contact: STRIP_CONTACT,
                position: strip_at(center),
            },
        ]
    );
}

#[test]
fn a_strip_crossing_its_center_moves_on_to_the_next_value() {
    let mut decoder = Push2InputDecoder::new();
    decode(&mut decoder, &[NOTE_ON, STRIP, 127]);
    decode(&mut decoder, &bend(4_080));

    assert!(decode(&mut decoder, &bend(8_192)).is_empty());
    assert_eq!(
        decode(&mut decoder, &bend(12_240)),
        vec![DeviceInputEdge::TouchMoved {
            contact: STRIP_CONTACT,
            position: strip_at(0.75),
        }]
    );
}

#[test]
fn buttons_press_and_release_by_name() {
    let mut decoder = Push2InputDecoder::new();

    assert_eq!(
        decode(&mut decoder, &[CC, 85, 127]),
        vec![button("play", InputButtonState::Pressed)]
    );
    assert_eq!(
        decode(&mut decoder, &[CC, 85, 0]),
        vec![button("play", InputButtonState::Released)]
    );
    for (cc, name) in [
        (3, "tap_tempo"),
        (20, "select_1"),
        (43, "scene_1"),
        (36, "scene_8"),
        (46, "arrow_up"),
        (48, "select"),
        (49, "shift"),
        (59, "user"),
        (86, "record"),
        (102, "track_1"),
        (109, "track_8"),
        (119, "undo"),
    ] {
        assert_eq!(
            decode(&mut decoder, &[CC, cc, 127]),
            vec![button(name, InputButtonState::Pressed)],
            "CC {cc}"
        );
    }
}

#[test]
fn encoders_mod_wheel_and_encoder_touches_are_not_input_edges() {
    let mut decoder = Push2InputDecoder::new();

    // Relative encoder turns: tempo, swing, the eight track encoders, and
    // master, both directions.
    for cc in [14, 15, 71, 72, 73, 74, 75, 76, 77, 78, 79] {
        assert!(decode(&mut decoder, &[CC, cc, 1]).is_empty(), "CC {cc}");
        assert!(decode(&mut decoder, &[CC, cc, 127]).is_empty(), "CC {cc}");
    }
    // The strip's mod wheel mode, which the driver does not configure.
    assert!(decode(&mut decoder, &[CC, 1, 64]).is_empty());
    // Encoder capacitive touches.
    for note in 0..=10 {
        assert!(decode(&mut decoder, &[NOTE_ON, note, 127]).is_empty());
        assert!(decode(&mut decoder, &[NOTE_ON, note, 0]).is_empty());
    }
}

#[test]
fn messages_on_other_channels_are_ignored() {
    let mut decoder = Push2InputDecoder::new();

    assert!(decode(&mut decoder, &[NOTE_ON | 1, 36, 127]).is_empty());
    assert!(decode(&mut decoder, &[CC | 15, 85, 127]).is_empty());
}

#[test]
fn running_status_concatenation_and_realtime_bytes_decode() {
    let mut decoder = Push2InputDecoder::new();

    assert_eq!(
        decode(
            &mut decoder,
            &[0xF8, NOTE_ON, 0xFE, 36, 127, 37, 127, 36, 0]
        ),
        vec![
            pad_began(0, pad_at(0.0, 0.0, 1.0)),
            pad_began(1, pad_at(1.0 / 7.0, 0.0, 1.0)),
            DeviceInputEdge::TouchEnded {
                contact: 0,
                position: pad_at(0.0, 0.0, 0.0),
            },
        ]
    );
    assert_eq!(
        decode(&mut decoder, &[CC, 85, 127, NOTE_OFF, 37, 0]),
        vec![
            button("play", InputButtonState::Pressed),
            DeviceInputEdge::TouchEnded {
                contact: 1,
                position: pad_at(1.0 / 7.0, 0.0, 0.0),
            },
        ]
    );
}

#[test]
fn sysex_replies_carry_no_input() {
    let mut decoder = Push2InputDecoder::new();

    // A palette entry reply and the reply confirming User mode.
    assert!(
        decode(
            &mut decoder,
            &[
                0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x04, 0x05, 0x7F, 0x01, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0xF7,
            ],
        )
        .is_empty()
    );
    assert!(
        decode(
            &mut decoder,
            &[0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]
        )
        .is_empty()
    );
}

#[test]
fn a_message_cut_short_loses_input_and_forgets_every_hold() {
    let mut decoder = Push2InputDecoder::new();
    decode(&mut decoder, &[NOTE_ON, 40, 100]);
    decode(&mut decoder, &[NOTE_ON, STRIP, 127]);
    decode(&mut decoder, &bend(4_080));

    let mut edges = Vec::new();
    assert_eq!(
        decoder.decode(&[NOTE_ON, 36, 100, NOTE_OFF, 40], &mut edges),
        Push2InputDecode::Lost
    );
    assert_eq!(
        edges,
        vec![pad_began(0, pad_at(0.0, 0.0, 100.0 / 127.0))],
        "edges ahead of the loss stand"
    );

    // The host cancels the holds, so their later lifts end nothing.
    assert!(decode(&mut decoder, &[NOTE_OFF, 40, 0]).is_empty());
    assert!(decode(&mut decoder, &[NOTE_OFF, 36, 0]).is_empty());
    assert!(decode(&mut decoder, &[NOTE_ON, STRIP, 0]).is_empty());
}

#[test]
fn data_without_a_status_or_a_status_cut_off_by_another_is_lost() {
    let mut edges = Vec::new();
    assert_eq!(
        Push2InputDecoder::new().decode(&[36, 100], &mut edges),
        Push2InputDecode::Lost
    );
    assert_eq!(
        Push2InputDecoder::new().decode(&[NOTE_ON, 36, NOTE_OFF, 36, 0], &mut edges),
        Push2InputDecode::Lost
    );
    assert!(edges.is_empty());
}

#[test]
fn leaving_user_mode_loses_input() {
    let mut decoder = Push2InputDecoder::new();
    decode(&mut decoder, &[NOTE_ON, 36, 100]);

    // Another host switched the Push 2 to Live mode; its pads now report on
    // the Live port, so this one will not see the lift.
    let mut edges = Vec::new();
    assert_eq!(
        decoder.decode(
            &[0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x00, 0xF7],
            &mut edges
        ),
        Push2InputDecode::Lost
    );
    assert!(edges.is_empty());
    assert!(decode(&mut decoder, &[NOTE_OFF, 36, 0]).is_empty());
}

#[derive(Debug, Clone, PartialEq)]
enum SinkCall {
    Publish(Vec<DeviceInputEdge>),
    Interrupted,
    Closed,
}

#[derive(Default)]
struct RecordingSink {
    calls: Mutex<Vec<SinkCall>>,
}

impl RecordingSink {
    fn calls(&self) -> Vec<SinkCall> {
        self.calls.lock().expect("sink log").clone()
    }
}

impl TransportInputSink for RecordingSink {
    fn publish(&self, edges: &[DeviceInputEdge]) {
        self.calls
            .lock()
            .expect("sink log")
            .push(SinkCall::Publish(edges.to_vec()));
    }

    fn interrupted(&self) {
        self.calls
            .lock()
            .expect("sink log")
            .push(SinkCall::Interrupted);
    }

    fn closed(&self) {
        self.calls.lock().expect("sink log").push(SinkCall::Closed);
    }
}

#[test]
fn the_relay_publishes_decoded_edges_and_reports_loss_after_them() {
    let sink = Arc::new(RecordingSink::default());
    let mut relay = Push2InputRelay::new(Arc::clone(&sink) as Arc<dyn TransportInputSink>);

    relay.receive(&[NOTE_ON, 36, 127]);
    // A sysex reply decodes to nothing and publishes nothing.
    relay.receive(&[0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x0A, 0x01, 0xF7]);
    relay.receive(&[NOTE_OFF, 36, 0, NOTE_ON, 37]);

    assert_eq!(
        sink.calls(),
        vec![
            SinkCall::Publish(vec![pad_began(0, pad_at(0.0, 0.0, 1.0))]),
            SinkCall::Publish(vec![DeviceInputEdge::TouchEnded {
                contact: 0,
                position: pad_at(0.0, 0.0, 0.0),
            }]),
            SinkCall::Interrupted,
        ]
    );
}

#[test]
fn dropping_the_relay_closes_input() {
    let sink = Arc::new(RecordingSink::default());
    let relay = Push2InputRelay::new(Arc::clone(&sink) as Arc<dyn TransportInputSink>);

    drop(relay);

    assert_eq!(sink.calls(), vec![SinkCall::Closed]);
}

#[test]
fn the_push2_transport_publishes_input() {
    let push2 = &descriptors()[0];
    let TransportType::DriverUsb { binding } = push2.transport else {
        panic!("the Push 2 uses a driver-owned USB transport");
    };
    assert!(binding.publishes_input);
}
