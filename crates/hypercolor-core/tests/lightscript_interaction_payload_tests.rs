//! Interaction payloads the LightScript runtime hands to HTML effects.
//!
//! The page holds whatever the last interaction payload carried, so these
//! tests pin when a payload is emitted as well as what it carries.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use hypercolor_core::effect::{
    FrameDataSources, FrameInput, LightScriptFrameUpdateOptions, LightscriptRuntime,
};
use hypercolor_core::input::{DeviceButtonHold, InteractionData, TouchContact};
use hypercolor_core::spatial::DeviceSurfaceMap;
use hypercolor_types::audio::AudioData;
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent, TouchPhase};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{
    Corner, EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
};

const CANVAS_WIDTH: u32 = 321;
const CANVAS_HEIGHT: u32 = 201;

fn interaction_options() -> LightScriptFrameUpdateOptions<'static> {
    LightScriptFrameUpdateOptions {
        include_audio: false,
        include_screen: false,
        include_sensors: false,
        include_interaction: true,
        include_media: false,
        include_net: false,
        include_lighting: false,
        emit_frame_timing: false,
        render_host_frame: false,
        selected_sensor_labels: None,
    }
}

struct Frame {
    audio: AudioData,
    sensors: SystemSnapshot,
}

impl Frame {
    fn new() -> Self {
        Self {
            audio: AudioData::silence(),
            sensors: SystemSnapshot::empty(),
        }
    }

    fn payload(
        &self,
        runtime: &mut LightscriptRuntime,
        interaction: &InteractionData,
        sources: FrameDataSources<'_>,
    ) -> Option<serde_json::Value> {
        self.payload_sized(runtime, interaction, sources, (CANVAS_WIDTH, CANVAS_HEIGHT))
    }

    fn payload_sized(
        &self,
        runtime: &mut LightscriptRuntime,
        interaction: &InteractionData,
        sources: FrameDataSources<'_>,
        (canvas_width, canvas_height): (u32, u32),
    ) -> Option<serde_json::Value> {
        let input = FrameInput {
            time_secs: 1.0,
            delta_secs: 1.0 / 60.0,
            frame_number: 1,
            audio: &self.audio,
            interaction,
            screen: None,
            sensors: &self.sensors,
            sources,
            canvas_width,
            canvas_height,
        };
        runtime
            .frame_payload_json(&input, &HashMap::new(), interaction_options())
            .map(|json| serde_json::from_str(&json).expect("payload should be valid JSON"))
    }
}

fn key_press(key: &str, seq: u64) -> TimedInputEvent {
    TimedInputEvent {
        event: InputEvent::Key {
            source_id: "kbd".into(),
            key: key.into(),
            state: InputButtonState::Pressed,
        },
        at_ms: 100,
        seq,
        physical_code: None,
        repeat_count: 1,
    }
}

#[test]
fn frame_after_an_edge_clears_the_edge_from_the_page() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);

    let mut pressed = InteractionData {
        generation: 4,
        ..InteractionData::default()
    };
    pressed.keyboard.pressed_keys = vec!["a".into()];
    pressed.keyboard.recent_keys = vec!["a".into()];
    pressed.batch.events = vec![key_press("a", 1)];
    let first = frame
        .payload(&mut runtime, &pressed, FrameDataSources::default())
        .expect("an edge frame emits");
    assert_eq!(first["interaction"]["events"][0]["key"], "a");

    // Same held state and generation, nothing new: the page must still be
    // told that the edge is gone, or it replays it every frame.
    let mut held = pressed.clone();
    held.keyboard.recent_keys.clear();
    held.batch.events.clear();
    let second = frame
        .payload(&mut runtime, &held, FrameDataSources::default())
        .expect("the frame after an edge re-sends the empty batch");
    assert!(second["interaction"].get("events").is_none());
    assert_eq!(
        second["interaction"]["keyboard"]["recent"],
        serde_json::json!([])
    );
    assert_eq!(second["interaction"]["keyboard"]["keys"][0], "a");

    // Once the page holds an empty batch, an idle frame stays quiet.
    assert!(
        frame
            .payload(&mut runtime, &held, FrameDataSources::default())
            .is_none()
    );
}

#[test]
fn re_sent_batch_never_repeats_recent_keys() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);

    let mut pressed = InteractionData {
        generation: 6,
        ..InteractionData::default()
    };
    pressed.keyboard.recent_keys = vec!["a".into()];
    pressed.batch.events = vec![key_press("a", 1)];
    let first = frame
        .payload(&mut runtime, &pressed, FrameDataSources::default())
        .expect("an edge frame emits");
    assert_eq!(first["interaction"]["keyboard"]["recent"][0], "a");

    // A reused frame keeps the snapshot's recent keys but clears its batch.
    let mut reused = pressed.clone();
    reused.batch.events.clear();
    let second = frame
        .payload(&mut runtime, &reused, FrameDataSources::default())
        .expect("the frame after an edge re-sends the empty batch");
    assert_eq!(
        second["interaction"]["keyboard"]["recent"],
        serde_json::json!([])
    );
    assert!(
        frame
            .payload(&mut runtime, &reused, FrameDataSources::default())
            .is_none()
    );
}

#[test]
fn frame_after_pointer_motion_resets_velocity() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);

    let mut moving = InteractionData {
        generation: 2,
        ..InteractionData::default()
    };
    moving.batch.motion.distance = 0.2;
    let first = frame
        .payload(&mut runtime, &moving, FrameDataSources::default())
        .expect("motion emits");
    assert!(
        first["interaction"]["mouse"]["velocity"]
            .as_f64()
            .expect("velocity")
            > 0.0
    );

    let mut resting = moving.clone();
    resting.batch.motion = hypercolor_core::input::MotionAggregate::default();
    let second = frame
        .payload(&mut runtime, &resting, FrameDataSources::default())
        .expect("the frame after motion re-sends a resting pointer");
    assert_eq!(
        second["interaction"]["mouse"]["velocity"],
        serde_json::json!(0.0)
    );
}

/// A layout with one 15x15 Lightpad output bound to `pad`.
fn pad_surfaces(pad: DeviceId, position: (f32, f32), size: f32, rotation: f32) -> DeviceSurfaceMap {
    let layout = SpatialLayout {
        id: "rig".into(),
        name: "Rig".into(),
        description: None,
        canvas_width: CANVAS_WIDTH,
        canvas_height: CANVAS_HEIGHT,
        zones: vec![Output {
            id: "pad".into(),
            name: "Lightpad".into(),
            device_id: "blocks:lightpad".into(),
            zone_name: None,
            position: NormalizedPosition::new(position.0, position.1),
            size: NormalizedPosition::new(size, size),
            rotation,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Matrix {
                width: 15,
                height: 15,
                serpentine: false,
                start_corner: Corner::TopLeft,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: None,
            edge_behavior: None,
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    };
    DeviceSurfaceMap::from_layout(&layout, |layout_device_id| {
        (layout_device_id == "blocks:lightpad").then_some(pad)
    })
}

fn touch(device_id: DeviceId, contact: u32, x: f32, y: f32, pressure: f32) -> TouchContact {
    TouchContact {
        device_id,
        contact,
        x,
        y,
        pressure,
    }
}

/// One unit in Q16.16.
const Q: i64 = 1 << 16;

/// A touch edge at Q16.16 surface coordinates, at half pressure.
fn touch_edge(
    device_id: DeviceId,
    contact: u32,
    phase: TouchPhase,
    x_q16_16: i64,
    y_q16_16: i64,
    seq: u64,
) -> TimedInputEvent {
    TimedInputEvent {
        event: InputEvent::Touch {
            source_id: format!("device:{device_id}"),
            device_id,
            contact,
            phase,
            x_q16_16,
            y_q16_16,
            pressure_q16_16: Q / 2,
        },
        at_ms: 200,
        seq,
        physical_code: None,
        repeat_count: 1,
    }
}

fn assert_number(value: &serde_json::Value, expected: f64) {
    let actual = value
        .as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {value}"));
    assert!(
        (actual - expected).abs() < 1.0e-4,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn touches_and_buttons_reach_the_page_with_canvas_positions() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    let pad = DeviceId::new();
    let loose = DeviceId::new();
    // A quarter-turned pad, half the canvas wide, centered.
    let surfaces = pad_surfaces(pad, (0.5, 0.5), 0.5, FRAC_PI_2);

    let mut interaction = InteractionData {
        generation: 3,
        ..InteractionData::default()
    };
    interaction.device.touches = vec![
        touch(pad, 2, 1.0, 0.5, 0.75),
        touch(loose, 0, 0.1, 0.9, 0.5),
    ];
    interaction.device.buttons = vec![DeviceButtonHold {
        device_id: pad,
        button: Arc::from("mode"),
    }];
    interaction.batch.events = vec![
        touch_edge(pad, 2, TouchPhase::Began, Q, Q / 2, 7),
        touch_edge(loose, 0, TouchPhase::Cancelled, Q / 4, 3 * Q / 4, 8),
        TimedInputEvent {
            event: InputEvent::DeviceButton {
                source_id: format!("device:{pad}"),
                device_id: pad,
                button: "mode".into(),
                state: InputButtonState::Pressed,
            },
            at_ms: 201,
            seq: 9,
            physical_code: None,
            repeat_count: 1,
        },
    ];

    let payload = frame
        .payload(
            &mut runtime,
            &interaction,
            FrameDataSources {
                device_surfaces: Some(&surfaces),
                ..FrameDataSources::default()
            },
        )
        .expect("device input emits");
    let device = &payload["interaction"]["device"];

    let placed = &device["touches"][0];
    assert_eq!(placed["device"], pad.to_string());
    assert_eq!(placed["contact"], 2);
    assert_number(&placed["x"], 1.0);
    assert_number(&placed["y"], 0.5);
    assert_number(&placed["pressure"], 0.75);
    // The surface's right edge swings below the center under a quarter turn.
    assert_number(&placed["canvas"]["nx"], 0.5);
    assert_number(&placed["canvas"]["ny"], 0.75);
    // Canvas 2D coordinates of the sampled pixel's center.
    assert_number(&placed["canvas"]["x"], 160.5);
    assert_number(&placed["canvas"]["y"], 150.5);

    let unplaced = &device["touches"][1];
    assert_eq!(unplaced["device"], loose.to_string());
    assert_number(&unplaced["x"], 0.1);
    assert!(
        unplaced["canvas"].is_null(),
        "a device outside the layout keeps surface coordinates and no canvas point"
    );

    assert_eq!(
        device["buttons"],
        serde_json::json!([{ "device": pad.to_string(), "button": "mode" }])
    );

    let events = payload["interaction"]["events"]
        .as_array()
        .expect("events array");
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["kind"], "touch");
    assert_eq!(events[0]["phase"], "began");
    assert_eq!(events[0]["device"], pad.to_string());
    assert_eq!(events[0]["contact"], 2);
    assert_eq!(events[0]["seq"], 7);
    assert_number(&events[0]["pressure"], 0.5);
    assert_number(&events[0]["canvas"]["nx"], 0.5);
    assert_number(&events[0]["canvas"]["ny"], 0.75);
    assert!(events[0].get("state").is_none());

    assert_eq!(events[1]["kind"], "touch");
    assert_eq!(events[1]["phase"], "cancelled");
    assert_number(&events[1]["x"], 0.25);
    assert_number(&events[1]["y"], 0.75);
    assert!(events[1]["canvas"].is_null());

    assert_eq!(events[2]["kind"], "device_button");
    assert_eq!(events[2]["device"], pad.to_string());
    assert_eq!(events[2]["button"], "mode");
    assert_eq!(events[2]["state"], "pressed");
    assert!(events[2].get("canvas").is_none());
}

#[test]
fn touches_without_any_placement_carry_surface_positions_only() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    let pad = DeviceId::new();
    let mut interaction = InteractionData {
        generation: 1,
        ..InteractionData::default()
    };
    interaction.device.touches = vec![touch(pad, 0, 0.25, 0.75, 0.4)];
    interaction.batch.events = vec![touch_edge(pad, 0, TouchPhase::Began, Q / 4, 3 * Q / 4, 1)];

    let payload = frame
        .payload(&mut runtime, &interaction, FrameDataSources::default())
        .expect("device input emits");

    let held = &payload["interaction"]["device"]["touches"][0];
    assert_number(&held["x"], 0.25);
    assert_number(&held["y"], 0.75);
    assert!(held["canvas"].is_null());
    assert!(payload["interaction"]["events"][0]["canvas"].is_null());
}

#[test]
fn held_touch_follows_its_placement_across_frames() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    let pad = DeviceId::new();
    let left = pad_surfaces(pad, (0.25, 0.5), 0.2, 0.0);
    let right = pad_surfaces(pad, (0.75, 0.5), 0.2, 0.0);
    let on = |surfaces| FrameDataSources {
        device_surfaces: Some(surfaces),
        ..FrameDataSources::default()
    };

    let mut held = InteractionData {
        generation: 1,
        ..InteractionData::default()
    };
    held.device.touches = vec![touch(pad, 0, 0.5, 0.5, 0.8)];
    let first = frame
        .payload(&mut runtime, &held, on(&left))
        .expect("a new hold emits");
    assert_number(
        &first["interaction"]["device"]["touches"][0]["canvas"]["nx"],
        0.25,
    );

    // A resting finger on an unchanged placement adds nothing.
    assert!(frame.payload(&mut runtime, &held, on(&left)).is_none());

    // Moving the pad in the layout moves the resting finger's canvas point.
    let moved = frame
        .payload(&mut runtime, &held, on(&right))
        .expect("a placement change re-sends held touches");
    assert_number(
        &moved["interaction"]["device"]["touches"][0]["canvas"]["nx"],
        0.75,
    );

    // So does resizing the canvas, since canvas pixels change.
    let resized = frame
        .payload_sized(&mut runtime, &held, on(&right), (641, 401))
        .expect("a resize re-sends held touches");
    assert_number(
        &resized["interaction"]["device"]["touches"][0]["canvas"]["x"],
        480.5,
    );
    assert_number(
        &resized["interaction"]["device"]["touches"][0]["canvas"]["y"],
        200.5,
    );

    // The lift empties held state, and the device block disappears.
    let lifted_state = InteractionData {
        generation: 2,
        ..InteractionData::default()
    };
    let lifted = frame
        .payload_sized(&mut runtime, &lifted_state, on(&right), (641, 401))
        .expect("the lift emits");
    assert!(lifted["interaction"].get("device").is_none());
}

#[test]
fn touches_are_ignored_by_effects_that_do_not_declare_input() {
    let frame = Frame::new();
    let mut runtime = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    let pad = DeviceId::new();
    let surfaces = pad_surfaces(pad, (0.5, 0.5), 0.5, 0.0);
    let mut interaction = InteractionData {
        generation: 1,
        ..InteractionData::default()
    };
    interaction.device.touches = vec![touch(pad, 0, 0.5, 0.5, 1.0)];
    interaction.batch.events = vec![touch_edge(pad, 0, TouchPhase::Began, Q / 2, Q / 2, 1)];
    let input = FrameInput {
        time_secs: 1.0,
        delta_secs: 1.0 / 60.0,
        frame_number: 1,
        audio: &frame.audio,
        interaction: &interaction,
        screen: None,
        sensors: &frame.sensors,
        sources: FrameDataSources {
            device_surfaces: Some(&surfaces),
            ..FrameDataSources::default()
        },
        canvas_width: CANVAS_WIDTH,
        canvas_height: CANVAS_HEIGHT,
    };

    let payload = runtime.frame_payload_json(
        &input,
        &HashMap::new(),
        LightScriptFrameUpdateOptions {
            include_interaction: false,
            ..interaction_options()
        },
    );
    assert!(
        payload.is_none(),
        "undeclared effects receive no interaction"
    );
}

#[test]
fn bootstrap_installs_touch_and_device_button_helpers() {
    let script = LightscriptRuntime::new(CANVAS_WIDTH, CANVAS_HEIGHT).bootstrap_script();

    assert!(script.contains("window.engine.touch.isDown = function(device)"));
    assert!(script.contains("window.engine.touch.touches = []"));
    assert!(script.contains("window.engine.deviceButtons.isDown = function(button, device)"));
    assert!(script.contains("window.engine.deviceButtons.held = []"));
    assert!(script.contains("engine.touch.events = touchEvents"));
    assert!(script.contains("engine.deviceButtons.events = deviceButtonEvents"));
}
