//! Interaction payloads the LightScript runtime hands to HTML effects.
//!
//! The page holds whatever the last interaction payload carried, so these
//! tests pin when a payload is emitted as well as what it carries.

use std::collections::HashMap;

use hypercolor_core::effect::{
    FrameDataSources, FrameInput, LightScriptFrameUpdateOptions, LightscriptRuntime,
};
use hypercolor_core::input::InteractionData;
use hypercolor_types::audio::AudioData;
use hypercolor_types::event::{InputButtonState, InputEvent, TimedInputEvent};
use hypercolor_types::sensor::SystemSnapshot;

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
        let input = FrameInput {
            time_secs: 1.0,
            delta_secs: 1.0 / 60.0,
            frame_number: 1,
            audio: &self.audio,
            interaction,
            screen: None,
            sensors: &self.sensors,
            sources,
            canvas_width: CANVAS_WIDTH,
            canvas_height: CANVAS_HEIGHT,
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
