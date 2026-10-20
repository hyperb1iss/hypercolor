//! Push 2 touches land on the LEDs under them.
//!
//! Each case lights one LED through the real Push 2 frame encoder to learn
//! which pad note or strip LED drives it, presses that control through the
//! real input decoder, and checks the resulting touch maps to the exact
//! canvas point the sampler reads for that LED. The layouts are built here
//! from the Push 2's own zone list, in every matrix start corner and strip
//! direction, which covers the topologies the UI seeds (bottom-left pads,
//! bottom-to-top strip) and the ones auto layout picks (top-left matrices,
//! left-to-right strips).

use hypercolor_core::spatial::{DeviceSurfaceMap, SpatialEngine};
use hypercolor_hal::drivers::push2::{
    PUSH2_PADS_SEGMENT, PUSH2_TOUCH_STRIP_SEGMENT, Push2InputDecode, Push2InputDecoder,
    Push2Protocol,
};
use hypercolor_hal::protocol::Protocol;
use hypercolor_types::device::{DeviceId, DeviceTopologyHint, SegmentInfo};
use hypercolor_types::device_input::DeviceInputEdge;
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    StripDirection,
};

const EPSILON: f32 = 1.0e-5;
const LAYOUT_DEVICE: &str = "usb:2982:1967:test";
const PUSH2_FRAME_LEDS: usize = 160;
const SET_TOUCH_STRIP_LEDS: u8 = 0x19;
/// Pitch bend at the strip's top: eight significant bits in fourteen.
const STRIP_TOP: u32 = 16_320;

/// The Push 2's own zone list, by segment name.
fn segment(name: &str) -> (usize, SegmentInfo) {
    let mut start = 0;
    for info in Push2Protocol::new().zones() {
        let count = usize::try_from(info.led_count).expect("LED count fits");
        if info.name == name {
            return (start, info);
        }
        start += count;
    }
    panic!("the Push 2 has no {name} zone");
}

/// Encode a frame with only zone LED `led` of `name` lit, on a fresh
/// protocol so the frame carries exactly that change.
fn frame_lighting(name: &str, led: usize) -> Vec<Vec<u8>> {
    let (start, _) = segment(name);
    let mut colors = vec![[0_u8; 3]; PUSH2_FRAME_LEDS];
    colors[start + led] = [255, 255, 255];
    Push2Protocol::new()
        .encode_frame(&colors)
        .into_iter()
        .map(|command| command.data)
        .collect()
}

/// The pad note the frame encoder drives for Pads LED `led`.
fn pad_note(led: usize) -> u8 {
    let notes: Vec<u8> = frame_lighting(PUSH2_PADS_SEGMENT, led)
        .into_iter()
        .filter(|message| message.len() == 3 && message[0] == 0x90 && message[2] != 0)
        .map(|message| message[1])
        .collect();
    assert_eq!(notes.len(), 1, "lighting one pad sends one pad note");
    notes[0]
}

/// The physical strip LED (0 at the bottom) the frame encoder drives for
/// Touch Strip LED `led`, read back from the Set Touch Strip LEDs sysex.
fn physical_strip_led(led: usize) -> usize {
    let frame = frame_lighting(PUSH2_TOUCH_STRIP_SEGMENT, led);
    let sysex = frame
        .iter()
        .find(|message| message.get(6) == Some(&SET_TOUCH_STRIP_LEDS))
        .expect("lighting a strip LED sends the strip sysex");
    let packed = &sysex[7..sysex.len() - 1];
    let lit: Vec<usize> = packed
        .iter()
        .enumerate()
        .flat_map(|(index, byte)| {
            [
                (index * 2, byte & 0x07),
                (index * 2 + 1, (byte >> 3) & 0x07),
            ]
        })
        .filter(|(_, level)| *level != 0)
        .map(|(physical, _)| physical)
        .collect();
    assert_eq!(lit.len(), 1, "lighting one strip LED lights one");
    lit[0]
}

fn decode(decoder: &mut Push2InputDecoder, bytes: &[u8]) -> Vec<DeviceInputEdge> {
    let mut edges = Vec::new();
    assert_eq!(decoder.decode(bytes, &mut edges), Push2InputDecode::Intact);
    edges
}

/// Surface position and segment of a touch's begin edge.
fn began(edges: &[DeviceInputEdge]) -> (f32, f32, String) {
    match edges {
        [
            DeviceInputEdge::TouchBegan {
                position, segment, ..
            },
        ] => (
            position.x,
            position.y,
            segment
                .as_deref()
                .expect("Push 2 touches name a segment")
                .to_owned(),
        ),
        other => panic!("expected one touch begin, got {other:?}"),
    }
}

fn output(segment: &str, topology: LedTopology, center: (f32, f32), size: (f32, f32)) -> Output {
    Output {
        id: segment.to_owned(),
        name: segment.to_owned(),
        device_id: LAYOUT_DEVICE.to_owned(),
        zone_name: Some(segment.to_owned()),
        position: NormalizedPosition::new(center.0, center.1),
        size: NormalizedPosition::new(size.0, size.1),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: Some(SamplingMode::Bilinear),
        edge_behavior: Some(EdgeBehavior::Clamp),
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
        brightness: None,
    }
}

/// A layout with the Push 2 pads and touch strip side by side, as two
/// outputs of one device; the strip is tilted so its frame is exercised
/// through a rotation too.
fn push2_layout(pad_corner: Corner, strip_direction: StripDirection) -> SpatialLayout {
    let (_, pads) = segment(PUSH2_PADS_SEGMENT);
    let (_, strip) = segment(PUSH2_TOUCH_STRIP_SEGMENT);
    let DeviceTopologyHint::Matrix { rows, cols } = pads.topology else {
        panic!("the pads are a matrix");
    };
    assert!(matches!(strip.topology, DeviceTopologyHint::Strip));
    let pads = output(
        PUSH2_PADS_SEGMENT,
        LedTopology::Matrix {
            width: cols,
            height: rows,
            serpentine: false,
            start_corner: pad_corner,
        },
        (0.6, 0.55),
        (0.5, 0.6),
    );
    let mut touch_strip = output(
        PUSH2_TOUCH_STRIP_SEGMENT,
        LedTopology::Strip {
            count: strip.led_count,
            direction: strip_direction,
        },
        (0.2, 0.55),
        (0.05, 0.6),
    );
    touch_strip.rotation = 0.2;
    SpatialLayout {
        id: "push2".into(),
        name: "Push 2".into(),
        description: None,
        canvas_width: 640,
        canvas_height: 480,
        zones: vec![pads, touch_strip],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

fn assert_close(actual: NormalizedPosition, expected: NormalizedPosition, what: &str) {
    assert!(
        (actual.x - expected.x).abs() < EPSILON && (actual.y - expected.y).abs() < EPSILON,
        "{what}: touch at {actual:?}, LED samples {expected:?}"
    );
}

/// Sampled canvas points per output, in layout order.
fn sampled(layout: &SpatialLayout) -> Vec<Vec<NormalizedPosition>> {
    SpatialEngine::new(layout.clone())
        .sampling_plan()
        .iter()
        .map(|zone| zone.sample_positions.clone())
        .collect()
}

#[test]
fn every_tested_pad_lands_on_its_own_led() {
    // Four corners and an interior pad, in Pads LED order.
    let pads = [0, 7, 56, 63, 27];
    // Bottom left is how the UI seeds the Push 2 (its physical first LED)
    // and top left how auto layout builds any matrix.
    for corner in [
        Corner::BottomLeft,
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomRight,
    ] {
        let layout = push2_layout(corner, StripDirection::BottomToTop);
        let device = DeviceId::new();
        let surfaces =
            DeviceSurfaceMap::from_layout(&layout, |id| (id == LAYOUT_DEVICE).then_some(device));
        let leds = sampled(&layout);

        for led in pads {
            let note = pad_note(led);
            let mut decoder = Push2InputDecoder::new();
            let (x, y, segment) = began(&decode(&mut decoder, &[0x90, note, 100]));
            assert_eq!(segment, PUSH2_PADS_SEGMENT);
            let touch = surfaces
                .segment_canvas_position(device, Some(&segment), x, y)
                .expect("the pads are placed");
            assert_close(
                touch,
                leds[0][led],
                &format!("{corner:?} pad LED {led} (note {note})"),
            );
        }
    }
}

#[test]
fn pads_land_where_they_sit_when_the_layout_matches_the_hardware() {
    let layout = push2_layout(Corner::BottomLeft, StripDirection::BottomToTop);
    let device = DeviceId::new();
    let surfaces =
        DeviceSurfaceMap::from_layout(&layout, |id| (id == LAYOUT_DEVICE).then_some(device));
    let press = |note| {
        let (x, y, segment) = began(&decode(&mut Push2InputDecoder::new(), &[0x90, note, 100]));
        surfaces
            .segment_canvas_position(device, Some(&segment), x, y)
            .expect("the pads are placed")
    };

    // Note 36 is the bottom-left pad and note 99 the top-right one, per the
    // interface manual; the pads output spans x 0.35 to 0.85, y 0.25 to 0.85.
    assert_close(
        press(36),
        NormalizedPosition::new(0.35, 0.85),
        "bottom-left pad",
    );
    assert_close(
        press(99),
        NormalizedPosition::new(0.85, 0.25),
        "top-right pad",
    );
}

#[test]
fn strip_positions_land_on_the_led_at_that_height() {
    // Strip LEDs whose pitch bend value is exact: LED p of 31 sits at
    // p / 30 of the strip, and pitch bend runs from 0 at the bottom to the
    // top value.
    let strip_leds = [0, 10, 20, 30];
    // As the UI seeds it (bottom to top) and as auto layout builds strips
    // (left to right), plus the remaining directions.
    for direction in [
        StripDirection::BottomToTop,
        StripDirection::LeftToRight,
        StripDirection::TopToBottom,
        StripDirection::RightToLeft,
    ] {
        let layout = push2_layout(Corner::BottomLeft, direction);
        let device = DeviceId::new();
        let surfaces =
            DeviceSurfaceMap::from_layout(&layout, |id| (id == LAYOUT_DEVICE).then_some(device));
        let leds = sampled(&layout);

        for led in strip_leds {
            let physical = physical_strip_led(led);
            let value = u32::try_from(physical).expect("small strip") * STRIP_TOP / 30;
            let value = u16::try_from(value).expect("pitch bend fits");
            let mut decoder = Push2InputDecoder::new();
            assert!(decode(&mut decoder, &[0x90, 12, 127]).is_empty());
            let bend = [
                0xE0,
                u8::try_from(value & 0x7F).expect("seven bits"),
                u8::try_from(value >> 7).expect("seven bits"),
            ];
            let (x, y, segment) = began(&decode(&mut decoder, &bend));
            assert_eq!(segment, PUSH2_TOUCH_STRIP_SEGMENT);
            let touch = surfaces
                .segment_canvas_position(device, Some(&segment), x, y)
                .expect("the strip is placed");
            assert_close(
                touch,
                leds[1][led],
                &format!("{direction:?} strip LED {led} (physical {physical})"),
            );
        }
    }
}
