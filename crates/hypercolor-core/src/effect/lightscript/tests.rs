use super::*;
use crate::input::ScreenData;

fn default_options() -> LightScriptFrameUpdateOptions<'static> {
    LightScriptFrameUpdateOptions {
        include_audio: true,
        include_screen: false,
        include_sensors: false,
        include_interaction: false,
        render_host_frame: false,
        selected_sensor_labels: None,
    }
}

fn frame_input_with<'a>(
    audio: &'a AudioData,
    interaction: &'a InteractionData,
    sensors: &'a SystemSnapshot,
    screen: Option<&'a ScreenData>,
    width: u32,
    height: u32,
) -> FrameInput<'a> {
    FrameInput {
        time_secs: 1.5,
        delta_secs: 1.0 / 30.0,
        frame_number: 42,
        audio,
        interaction,
        screen,
        sensors,
        canvas_width: width,
        canvas_height: height,
    }
}

fn payload_from_json(payload: &str) -> serde_json::Value {
    serde_json::from_str(payload).expect("payload should be valid JSON")
}

fn quiet_frame<'a>(
    audio: &'a AudioData,
    interaction: &'a InteractionData,
    sensors: &'a SystemSnapshot,
) -> FrameInput<'a> {
    frame_input_with(audio, interaction, sensors, None, 320, 200)
}

#[test]
fn bootstrap_script_contains_runtime_shape_and_frame_adapter() {
    let runtime = LightscriptRuntime::new(320, 200);
    let script = runtime.bootstrap_script();

    assert!(script.contains("window.engine.width = 320"));
    assert!(script.contains("window.engine.height = 200"));
    assert!(script.contains("window.engine.audio.freq = new Int8Array(200)"));
    assert!(script.contains("window.engine.audio.frequencyWeighted = new Float32Array(200)"));
    assert!(script.contains("window.engine.zone.hue = new Int16Array(560)"));
    assert!(script.contains("window.engine.getSensorValue = function(name)"));
    assert!(script.contains("window.engine.keyboard.isKeyDown = function(key)"));
    assert!(script.contains("window.__hypercolorApplyFramePayload = function(payload)"));
    assert!(script.contains("applyAudio(engine, payload.audio)"));
    assert!(script.contains("applyControls(payload.controls)"));
    assert!(script.contains("applyInteraction(engine, payload.interaction)"));
}

#[test]
fn normalized_level_to_db_clamps_edges() {
    assert!((normalized_level_to_db(1.0) - 0.0).abs() < f32::EPSILON);
    assert!((normalized_level_to_db(0.0) - LEVEL_FLOOR_DB).abs() < f32::EPSILON);
    assert!((normalized_level_to_db(-1.0) - LEVEL_FLOOR_DB).abs() < f32::EPSILON);
}

#[test]
fn frame_payload_json_serializes_typed_payload_only() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();

    let payload = runtime
        .frame_payload_json(
            &quiet_frame(&audio, &interaction, &sensors),
            &HashMap::new(),
            default_options(),
        )
        .expect("first quiet frame should emit payload JSON");

    assert!(!payload.contains("window.__hypercolorApplyFramePayload"));
    assert!(!payload.contains("window.engine.audio.level ="));
    assert_eq!(
        payload_from_json(&payload)["timing"]["frameNumber"],
        serde_json::json!(42)
    );
    assert_eq!(
        payload_from_json(&payload)["canvas"],
        serde_json::json!({ "width": 320, "height": 200 })
    );
}

#[test]
fn frame_payload_emits_control_deltas_only() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let input = quiet_frame(&audio, &interaction, &sensors);
    let mut controls = HashMap::new();
    controls.insert("speed".to_owned(), ControlValue::Float(0.5));
    let options = LightScriptFrameUpdateOptions {
        include_audio: false,
        ..default_options()
    };

    let first = runtime
        .frame_payload(&input, &controls, options)
        .expect("changed control should emit");
    assert_eq!(first.controls["speed"], LightScriptControlValue::Float(0.5));
    assert!(runtime.frame_payload(&input, &controls, options).is_none());

    controls.insert("speed".to_owned(), ControlValue::Float(0.8));
    let changed = runtime
        .frame_payload(&input, &controls, options)
        .expect("updated control should emit");
    assert_eq!(
        changed.controls["speed"],
        LightScriptControlValue::Float(0.8)
    );
}

#[test]
fn frame_payload_suppresses_repeated_quiet_audio_updates() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let input = quiet_frame(&audio, &interaction, &sensors);

    assert!(
        runtime
            .frame_payload(&input, &HashMap::new(), default_options())
            .expect("first quiet audio frame initializes the runtime")
            .audio
            .is_some()
    );
    assert!(
        runtime
            .frame_payload(&input, &HashMap::new(), default_options())
            .is_none()
    );
}

#[test]
fn frame_payload_emits_audio_when_quiet_state_ends() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let quiet_audio = AudioData::silence();
    let mut active_audio = AudioData::silence();
    active_audio.rms_level = 0.2;
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let quiet = quiet_frame(&quiet_audio, &interaction, &sensors);
    let active = quiet_frame(&active_audio, &interaction, &sensors);

    runtime.frame_payload(&quiet, &HashMap::new(), default_options());
    let payload = runtime
        .frame_payload(&active, &HashMap::new(), default_options())
        .expect("active audio should emit after quiet frame");
    assert!(payload.audio.is_some());
}

#[test]
fn frame_payload_scopes_repeated_sensor_updates_to_selected_labels() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let mut sensors = SystemSnapshot::empty();
    sensors.cpu_temp_celsius = Some(54.0);
    sensors.gpu_temp_celsius = Some(62.0);
    let selected = vec!["cpu_temp".to_owned()];
    let options = LightScriptFrameUpdateOptions {
        include_audio: false,
        include_sensors: true,
        selected_sensor_labels: Some(&selected),
        ..default_options()
    };

    let input = quiet_frame(&audio, &interaction, &sensors);
    let initial = runtime
        .frame_payload(&input, &HashMap::new(), options)
        .expect("initial sensors should emit");
    let initial_sensors = initial.sensors.expect("sensor payload should exist");
    assert!(initial_sensors.replace_sensor_map);
    assert!(initial_sensors.readings.contains_key("cpu_temp"));
    assert!(initial_sensors.readings.contains_key("gpu_temp"));
    assert!(initial_sensors.sensor_list.is_some());

    sensors.cpu_temp_celsius = Some(55.0);
    sensors.gpu_temp_celsius = Some(63.0);
    let changed_input = quiet_frame(&audio, &interaction, &sensors);
    let changed = runtime
        .frame_payload(&changed_input, &HashMap::new(), options)
        .expect("selected sensor update should emit");
    let changed_sensors = changed.sensors.expect("sensor payload should exist");
    assert!(!changed_sensors.replace_sensor_map);
    assert!(changed_sensors.readings.contains_key("cpu_temp"));
    assert!(!changed_sensors.readings.contains_key("gpu_temp"));
    assert!(changed_sensors.sensor_list.is_none());
}

#[test]
fn frame_payload_can_skip_sensor_updates() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let options = LightScriptFrameUpdateOptions {
        include_audio: false,
        include_sensors: false,
        ..default_options()
    };

    assert!(
        runtime
            .frame_payload(
                &quiet_frame(&audio, &interaction, &sensors),
                &HashMap::new(),
                options,
            )
            .is_none()
    );
}

#[test]
fn audio_payload_contains_level_and_frequency_vectors() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let mut audio = AudioData::silence();
    audio.rms_level = 1.0;
    audio.beat_pulse = 0.75;
    audio.onset_pulse = 0.5;
    audio.beat_phase = 0.25;
    audio.spectrum = vec![1.0; SPECTRUM_BINS];
    audio.mel_bands = vec![1.0; MEL_BANDS];
    audio.chromagram = vec![0.1, 0.6, 0.2];

    let payload = runtime.audio_payload(&audio);
    assert_eq!(payload.level_db, 0.0);
    assert!(payload.level_short > 0.5 && payload.level_short < 1.0);
    assert!(payload.level_long > 0.1 && payload.level_long < payload.level_short);
    assert_eq!(&payload.frequency_raw[..3], &[127, 127, 127]);
    assert_eq!(&payload.frequency[..3], &[1.0, 1.0, 1.0]);
    assert!((payload.frequency_weighted[0] - 0.82).abs() < 0.0001);
    assert_eq!(payload.dominant_pitch, 1.0);
    assert_eq!(payload.mel_bands.len(), MEL_BANDS);
    assert_eq!(payload.mel_bands_normalized[0], 1.0);
    assert_eq!(payload.chromagram.len(), CHROMA_BINS);
}

#[test]
fn audio_payload_sanitizes_non_finite_values() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let mut audio = AudioData::silence();
    audio.rms_level = f32::NAN;
    audio.spectrum = vec![f32::INFINITY, f32::NEG_INFINITY, f32::NAN];
    audio.mel_bands = vec![f32::INFINITY, f32::NEG_INFINITY];
    audio.chromagram = vec![f32::NAN];
    audio.bpm = f32::INFINITY;
    audio.spectral_flux = f32::NEG_INFINITY;

    let payload = runtime.audio_payload(&audio);
    assert_eq!(&payload.frequency_raw[..3], &[0, 0, 0]);
    assert_eq!(&payload.frequency[..3], &[0.0, 0.0, 0.0]);
    assert_eq!(&payload.mel_bands[..2], &[0.0, 0.0]);
    assert!(
        serde_json::to_string(&payload)
            .expect("payload serializes")
            .contains('0')
    );
    assert!(
        !serde_json::to_string(&payload)
            .expect("payload serializes")
            .contains("NaN")
    );
}

#[test]
fn normalized_mel_bands_track_running_max_decay() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let seed = vec![1.0, 0.5, 0.25];
    let initial = runtime.normalized_mel_bands(&seed);
    assert_eq!(initial[0], 1.0);
    assert_eq!(initial[1], 1.0);
    assert_eq!(initial[2], 1.0);

    let quieter = vec![0.5, 0.25, 0.125];
    let normalized = runtime.normalized_mel_bands(&quieter);
    assert!(normalized[0] > 0.49 && normalized[0] < 0.52);
    assert!(normalized[1] > 0.49 && normalized[1] < 0.52);
    assert!(normalized[2] > 0.49 && normalized[2] < 0.52);
}

#[test]
fn audio_payload_curves_live_band_energy_into_reactive_range() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let mut audio = AudioData::silence();
    audio.rms_level = 0.08;
    for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
        *value = 0.64;
    }
    for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
        *value = 0.60;
    }
    for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
        *value = 0.20;
    }
    audio.mel_bands = vec![0.64; MEL_BANDS];

    let payload = runtime.audio_payload(&audio);
    assert!(payload.bass > 0.09 && payload.bass < 0.12);
    assert!(payload.mid > 0.08 && payload.mid < 0.11);
    assert!(payload.treble > 0.01 && payload.treble < 0.02);
    assert!(payload.level_linear > 0.07 && payload.level_linear < 0.09);
    assert!((payload.raw_rms - 0.08).abs() < 0.0001);
}

#[test]
fn audio_payload_keeps_strong_bass_hits_capable_of_triggering_shockwave() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let mut audio = AudioData::silence();
    audio.rms_level = 0.28;
    audio.beat_pulse = 1.0;
    audio.onset_pulse = 1.0;
    audio.spectral_flux = 0.65;
    for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
        *value = 0.88;
    }
    for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
        *value = 0.60;
    }
    for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
        *value = 0.33;
    }

    let payload = runtime.audio_payload(&audio);
    assert!(payload.bass > 0.55);
    assert!(payload.beat_pulse > 0.75);
    assert!(payload.onset_pulse > 0.75);
    assert!(payload.level_linear > 0.40);
}

#[test]
fn audio_payload_flux_bands_track_change_not_steady_loudness() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let mut audio = AudioData::silence();
    audio.rms_level = 0.2;
    for value in &mut audio.spectrum[..SPECTRUM_BASS_END] {
        *value = 0.72;
    }
    for value in &mut audio.spectrum[SPECTRUM_BASS_END..SPECTRUM_MID_END] {
        *value = 0.44;
    }
    for value in &mut audio.spectrum[SPECTRUM_MID_END..] {
        *value = 0.18;
    }

    let first = runtime.audio_payload(&audio);
    let second = runtime.audio_payload(&audio);
    assert!(first.spectral_flux_bands[0] > second.spectral_flux_bands[0]);
}

#[test]
fn interaction_payload_populates_keyboard_and_mouse_state_on_change() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let sensors = SystemSnapshot::empty();
    let interaction = InteractionData {
        keyboard: crate::input::KeyboardData {
            pressed_keys: vec!["a".to_owned(), "Space".to_owned()],
            recent_keys: vec!["a".to_owned()],
        },
        mouse: crate::input::MouseData {
            x: 42,
            y: 24,
            buttons: vec!["left".to_owned()],
            down: true,
        },
    };
    let options = LightScriptFrameUpdateOptions {
        include_audio: false,
        include_interaction: true,
        ..default_options()
    };
    let input = quiet_frame(&audio, &interaction, &sensors);

    let payload = runtime
        .frame_payload(&input, &HashMap::new(), options)
        .expect("changed interaction should emit");
    let interaction_payload = payload.interaction.expect("interaction payload");
    assert!(interaction_payload.keyboard.keys.contains(&"A".to_owned()));
    assert!(
        interaction_payload
            .keyboard
            .keys
            .contains(&"KeyA".to_owned())
    );
    assert!(
        interaction_payload
            .keyboard
            .keys
            .contains(&"Spacebar".to_owned())
    );
    assert_eq!(interaction_payload.keyboard.recent, vec!["a".to_owned()]);
    assert_eq!(interaction_payload.mouse.x, 42);
    assert!(
        interaction_payload
            .mouse
            .buttons
            .contains(&"primary".to_owned())
    );
    assert!(
        runtime
            .frame_payload(&input, &HashMap::new(), options)
            .is_none()
    );
}

#[test]
fn canvas_dimensions_emit_only_on_change() {
    let mut runtime = LightscriptRuntime::new(320, 200);
    let audio = AudioData::silence();
    let interaction = InteractionData::default();
    let sensors = SystemSnapshot::empty();
    let options = LightScriptFrameUpdateOptions {
        include_audio: false,
        ..default_options()
    };

    assert!(
        runtime
            .frame_payload(
                &quiet_frame(&audio, &interaction, &sensors),
                &HashMap::new(),
                options,
            )
            .is_none()
    );
    let resized = runtime
        .frame_payload(
            &frame_input_with(&audio, &interaction, &sensors, None, 640, 360),
            &HashMap::new(),
            options,
        )
        .expect("resize should emit");
    assert_eq!(resized.canvas.width, 640);
    assert_eq!(resized.canvas.height, 360);
}
