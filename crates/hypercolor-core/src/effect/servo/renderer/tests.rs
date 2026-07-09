use super::*;
#[cfg(feature = "servo-gpu-import")]
use crate::effect::servo::set_servo_gpu_import_mode;
use crate::effect::servo::worker::{
    HARD_STALL_TIMEOUT, install_running_shared_worker, reset_shared_servo_worker_state,
    shutdown_shared_servo_worker,
    test_support::{
        SHARED_WORKER_STATE_TEST_LOCK, spawn_blocking_load_test_worker, spawn_load_test_worker,
        spawn_render_test_worker, spawn_test_worker, worker_client_from,
    },
};
use hypercolor_types::audio::AudioData;
#[cfg(feature = "servo-gpu-import")]
use hypercolor_types::config::ServoGpuImportMode;
use hypercolor_types::effect::{
    ControlDefinition, ControlType, EffectCategory, EffectId, EffectSource,
};
use hypercolor_types::sensor::SystemSnapshot;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static DEFAULT_INTERACTION: LazyLock<crate::input::InteractionData> =
    LazyLock::new(crate::input::InteractionData::default);
static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);
static SOFT_STALL_TELEMETRY_TEST_LOCK: LazyLock<std::sync::Mutex<()>> =
    LazyLock::new(std::sync::Mutex::default);

fn frame_input(delta_secs: f32) -> FrameInput<'static> {
    FrameInput {
        time_secs: 0.0,
        delta_secs,
        frame_number: 0,
        audio: &SILENCE,
        interaction: &DEFAULT_INTERACTION,
        screen: None,
        sensors: &EMPTY_SENSORS,
        sources: crate::effect::traits::FrameDataSources::default(),
        canvas_width: DEFAULT_CANVAS_WIDTH,
        canvas_height: DEFAULT_CANVAS_HEIGHT,
    }
}

fn frame_payload_value(frame_payloads: &[ServoFramePayload]) -> serde_json::Value {
    let payload = frame_payloads
        .first()
        .expect("render should include a frame payload");
    serde_json::from_str(payload.as_json()).expect("frame payload should be valid JSON")
}

fn custom_interaction(
    recent_keys: &[&str],
    pressed_keys: &[&str],
) -> crate::input::InteractionData {
    crate::input::InteractionData {
        keyboard: crate::input::KeyboardData {
            pressed_keys: pressed_keys.iter().map(ToString::to_string).collect(),
            recent_keys: recent_keys.iter().map(ToString::to_string).collect(),
        },
        mouse: crate::input::MouseData::default(),
    }
}

fn custom_audio(rms_level: f32) -> AudioData {
    let mut audio = AudioData::silence();
    audio.rms_level = rms_level;
    audio
}

fn frame_input_with<'a>(
    delta_secs: f32,
    frame_number: u64,
    audio: &'a AudioData,
    interaction: &'a crate::input::InteractionData,
    canvas_width: u32,
    canvas_height: u32,
) -> FrameInput<'a> {
    FrameInput {
        time_secs: f64::from(delta_secs) * frame_number as f64,
        delta_secs,
        frame_number,
        audio,
        interaction,
        screen: None,
        sensors: &EMPTY_SENSORS,
        sources: crate::effect::traits::FrameDataSources::default(),
        canvas_width,
        canvas_height,
    }
}

fn solid_canvas(width: u32, height: u32, r: u8, g: u8, b: u8) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    canvas.fill(Rgba::new(r, g, b, 255));
    canvas
}

fn wait_for_load_completion(renderer: &mut ServoRenderer) {
    for _ in 0..20 {
        renderer.poll_load_task();
        if renderer.load_task.is_none() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Servo load task should complete");
}

fn html_metadata(path: PathBuf) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::from(Uuid::nil()),
        name: "HTML Test".to_owned(),
        author: "hypercolor".to_owned(),
        version: "0.1.0".to_owned(),
        description: "test".to_owned(),
        category: EffectCategory::Interactive,
        tags: Vec::new(),
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html { path },
        license: None,
    }
}

fn display_html_metadata(path: PathBuf) -> EffectMetadata {
    let mut metadata = html_metadata(path);
    metadata.category = EffectCategory::Display;
    metadata
}

fn attach_renderer_session(
    renderer: &mut ServoRenderer,
    worker: &crate::effect::servo::worker::ServoWorker,
) {
    let mut session = ServoSessionHandle::new(
        worker_client_from(worker),
        SessionConfig {
            render_width: DEFAULT_CANVAS_WIDTH,
            render_height: DEFAULT_CANVAS_HEIGHT,
            inject_engine_globals: true,
            producer_role: ServoProducerRole::SceneHtml,
        },
    )
    .expect("test session should initialize");
    session
        .load_html_file(std::path::Path::new("test.html"))
        .expect("test session should load");
    renderer.session = Some(session);
}

#[test]
fn destroy_clears_renderer_state_without_shutting_down_shared_worker() {
    let (worker, stopped) = spawn_test_worker();

    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.pending_scripts.push("tick()".to_owned());
    renderer
        .pending_frame_payloads
        .push(ServoFramePayload::from_json("{\"frame\":1}".to_owned()).expect("valid JSON"));
    renderer
        .controls
        .insert("speed".to_owned(), ControlValue::Float(1.0));
    renderer.html_source = Some(PathBuf::from("source.html"));
    renderer.html_resolved_path = Some(PathBuf::from("resolved.html"));
    renderer.runtime_html_path = Some(PathBuf::from("runtime.html"));
    renderer.warned_fallback_frame = true;
    renderer.warned_stalled_frame = true;
    renderer.hard_stall = Some(ServoHardStall {
        pending_age: HARD_STALL_TIMEOUT,
    });
    renderer.command_queue_saturated = true;
    renderer.include_audio_updates = false;
    renderer.host_driven_animation = true;
    renderer.queue_frame(&frame_input(1.0 / 30.0));
    renderer
        .session
        .as_mut()
        .expect("attached test session")
        .request_render_cpu(Vec::new())
        .expect("test render should queue");
    renderer.last_canvas = Some(solid_canvas(
        DEFAULT_CANVAS_WIDTH,
        DEFAULT_CANVAS_HEIGHT,
        1,
        2,
        3,
    ));

    renderer.destroy();

    assert!(!stopped.load(Ordering::SeqCst));
    assert!(renderer.session.is_none());
    assert!(renderer.pending_scripts.is_empty());
    assert!(renderer.pending_frame_payloads.is_empty());
    assert!(renderer.queued_frame.is_none());
    assert!(renderer.last_canvas.is_none());
    assert!(renderer.controls.is_empty());
    assert!(renderer.html_source.is_none());
    assert!(renderer.html_resolved_path.is_none());
    assert!(renderer.runtime_html_path.is_none());
    assert!(!renderer.initialized);
    assert!(!renderer.warned_fallback_frame);
    assert!(!renderer.warned_stalled_frame);
    assert!(renderer.hard_stall.is_none());
    assert!(!renderer.command_queue_saturated);
    assert!(renderer.include_audio_updates);
    assert!(!renderer.include_sensor_updates);
    assert!(!renderer.host_driven_animation);

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn render_admission_stays_pending_while_a_frame_is_in_flight() {
    let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
        spawn_render_test_worker();
    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);

    let session = renderer.session.as_mut().expect("attached test session");
    session
        .request_render_cpu(Vec::new())
        .expect("first render should queue");
    render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first render command");

    assert!(matches!(
        session.reserve_render().expect("render admission"),
        crate::effect::servo::session::ServoRenderAdmission::Pending
    ));

    result_tx
        .send(Ok(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            1,
            2,
            3,
        )))
        .expect("first result should be delivered");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first result delivery ack");

    renderer.destroy();
    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn bootstrap_scripts_track_default_animation_cap_without_js_throttle() {
    let mut renderer = ServoRenderer::new();

    renderer.enqueue_bootstrap_scripts();

    assert_eq!(
        renderer.last_animation_fps_cap,
        Some(DEFAULT_EFFECT_FPS_CAP)
    );
    assert!(
        renderer
            .pending_scripts
            .iter()
            .all(|script| !script.contains("__hypercolorFpsCap"))
    );
    assert!(
        renderer
            .pending_scripts
            .iter()
            .all(|script| !script.contains("__hypercolorHostDrivenAnimation"))
    );
}

#[test]
fn display_bootstrap_marks_host_driven_animation() {
    let mut renderer = ServoRenderer::new();
    renderer.host_driven_animation = true;

    renderer.enqueue_bootstrap_scripts();

    assert!(
        renderer
            .pending_scripts
            .iter()
            .any(|script| script.contains("__hypercolorHostDrivenAnimation = true"))
    );
}

#[test]
fn display_animation_cadence_stays_fixed_at_30_fps() {
    let metadata = display_html_metadata(PathBuf::from("display.html"));

    assert_eq!(animation_cadence(&metadata), AnimationCadence::Fixed(30));
    assert_eq!(animation_cadence(&metadata).fps_cap(1.0 / 60.0), 30);
    assert_eq!(animation_cadence(&metadata).fps_cap(1.0 / 20.0), 30);
}

#[cfg(feature = "servo-gpu-import")]
#[test]
fn no_ready_gpu_cache_reuse_is_display_only() {
    let html = html_metadata(PathBuf::from("effect.html"));
    let display = display_html_metadata(PathBuf::from("display.html"));

    assert!(!should_reuse_cached_gpu_frame_on_no_ready(&html));
    assert!(should_reuse_cached_gpu_frame_on_no_ready(&display));
}

#[cfg(feature = "servo-gpu-import")]
#[test]
fn display_faces_submit_gpu_preferred_renders_when_import_is_on() {
    let _lock = SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();
    set_servo_gpu_import_mode(ServoGpuImportMode::On);

    let (worker, render_rx, result_tx, delivered_rx, unload_rx, stopped) =
        spawn_render_test_worker();
    install_running_shared_worker(worker);

    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let source_path = temp_dir.path().join("face.html");
    std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
        .expect("write source html");

    let metadata = display_html_metadata(source_path);
    let mut renderer = ServoRenderer::new();
    renderer
        .init_with_canvas_size(&metadata, 640, 480)
        .expect("renderer should queue initialization");
    wait_for_load_completion(&mut renderer);

    let output = renderer
        .render_output(&frame_input_with(
            1.0 / 60.0,
            1,
            &SILENCE,
            &DEFAULT_INTERACTION,
            640,
            480,
        ))
        .expect("display render should submit");
    assert!(matches!(output, EffectRenderOutput::Pending));

    let render = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("render command should be queued");
    assert!(render.prefer_gpu);
    assert!(render.reuse_cached_on_no_ready);
    assert_eq!(render.width, 640);
    assert_eq!(render.height, 480);

    result_tx
        .send(Ok(solid_canvas(640, 480, 1, 2, 3)))
        .expect("render result should send");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("render result should deliver");

    renderer.destroy();
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("destroy should unload test worker");

    shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn sensor_updates_are_limited_to_sensor_aware_metadata() {
    let plain = html_metadata(PathBuf::from("bubble.html"));
    assert!(!effect_uses_sensor_data(&plain));

    let display = display_html_metadata(PathBuf::from("face.html"));
    assert!(!effect_uses_sensor_data(&display));

    let mut sensor_control = html_metadata(PathBuf::from("sensor.html"));
    sensor_control.category = EffectCategory::Display;
    sensor_control.controls.push(ControlDefinition {
        id: "targetSensor".to_owned(),
        name: "Sensor".to_owned(),
        kind: ControlKind::Sensor,
        control_type: ControlType::Dropdown,
        default_value: ControlValue::Enum("cpu_temp".to_owned()),
        min: None,
        max: None,
        step: None,
        labels: vec!["cpu_temp".to_owned()],
        group: None,
        tooltip: None,
        aspect_lock: None,
        preview_source: None,
        binding: None,
    });
    assert!(effect_uses_sensor_data(&sensor_control));
    assert_eq!(
        scoped_sensor_control_ids(&sensor_control),
        vec!["targetSensor".to_owned()]
    );

    let mut tagged = plain;
    tagged.tags.push("system-monitor".to_owned());
    assert!(effect_uses_sensor_data(&tagged));
    assert!(scoped_sensor_control_ids(&tagged).is_empty());
}

#[test]
fn interaction_updates_are_limited_to_interaction_aware_metadata() {
    let mut ambient = html_metadata(PathBuf::from("ambient.html"));
    ambient.category = EffectCategory::Ambient;
    assert!(!effect_uses_interaction_data(&ambient));

    let interactive = html_metadata(PathBuf::from("interactive.html"));
    assert!(effect_uses_interaction_data(&interactive));

    let mut tagged = ambient.clone();
    tagged.tags.push("mouse".to_owned());
    assert!(effect_uses_interaction_data(&tagged));
}

#[test]
fn fixed_animation_cadence_waits_for_next_due_frame() {
    let cadence = AnimationCadence::Fixed(30);

    assert!(cadence.render_due(None, 0.0));
    assert!(!cadence.render_due(Some(0.0), 0.01));
    assert!(cadence.render_due(Some(0.0), 1.0 / 30.0));
    assert!(cadence.render_due(Some(0.0), 0.05));
}

#[test]
fn fixed_animation_cadence_stays_precise_after_long_uptime() {
    let cadence = AnimationCadence::Fixed(30);
    let last_submit = Duration::from_hours(60 * 24).as_secs_f64();

    assert!(!cadence.render_due(Some(last_submit), last_submit + 0.01));
    assert!(cadence.render_due(Some(last_submit), last_submit + 1.0 / 30.0));
}

#[test]
fn take_pending_scripts_preserves_capacity() {
    let mut renderer = ServoRenderer::new();
    renderer.pending_scripts = Vec::with_capacity(8);
    renderer.pending_scripts.push("tick()".to_owned());

    let capacity = renderer.pending_scripts.capacity();
    let scripts = renderer.take_pending_scripts();

    assert_eq!(scripts, vec!["tick()"]);
    assert!(renderer.pending_scripts.is_empty());
    assert!(renderer.pending_scripts.capacity() >= capacity);
}

#[test]
fn init_with_canvas_size_returns_before_servo_session_create_completes() {
    let _lock = SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();

    let (worker, load_rx, release_tx, unload_rx, stopped) = spawn_blocking_load_test_worker();
    install_running_shared_worker(worker);

    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let source_path = temp_dir.path().join("effect.html");
    std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
        .expect("write source html");

    let metadata = html_metadata(source_path);
    let mut renderer = ServoRenderer::new();
    let started_at = Instant::now();
    renderer
        .init_with_canvas_size(&metadata, 640, 480)
        .expect("renderer should queue initialization");

    assert!(started_at.elapsed() < Duration::from_millis(50));
    assert!(renderer.load_task.is_some());
    assert!(renderer.session.is_none());

    let load = load_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("create-session command should be queued asynchronously");
    assert_eq!(load.width, 640);
    assert_eq!(load.height, 480);

    release_tx.send(()).expect("release create-session");
    wait_for_load_completion(&mut renderer);
    assert!(renderer.session.is_some());

    renderer.destroy();
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("destroy should unload test worker");

    shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn render_into_uses_placeholder_while_servo_load_is_pending() {
    let _lock = SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();

    let (worker, load_rx, release_tx, unload_rx, stopped) = spawn_blocking_load_test_worker();
    install_running_shared_worker(worker);

    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let source_path = temp_dir.path().join("effect.html");
    std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
        .expect("write source html");

    let metadata = html_metadata(source_path);
    let mut renderer = ServoRenderer::new();
    renderer
        .init_with_canvas_size(&metadata, 640, 480)
        .expect("renderer should queue initialization");

    load_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("create-session command should be queued asynchronously");

    let audio = custom_audio(0.5);
    let interaction = custom_interaction(&[], &[]);
    let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
    let mut target = Canvas::new(1, 1);
    let started_at = Instant::now();

    renderer
        .render_into(&input, &mut target)
        .expect("placeholder render should succeed while Servo load is pending");

    assert!(started_at.elapsed() < Duration::from_millis(20));
    assert!(renderer.load_task.is_some());
    assert!(renderer.session.is_none());
    assert_eq!(target.width(), 4);
    assert_eq!(target.height(), 3);
    assert_eq!(target.get_pixel(0, 0), Rgba::new(7, 127, 39, 255));

    release_tx.send(()).expect("release create-session");
    wait_for_load_completion(&mut renderer);

    renderer.destroy();
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("destroy should unload test worker");

    shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn init_with_canvas_size_reuses_previous_canvas_while_new_effect_loads() {
    let _lock = SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();

    let (worker, load_rx, release_tx, unload_rx, stopped) = spawn_blocking_load_test_worker();
    install_running_shared_worker(worker);

    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let source_path = temp_dir.path().join("effect.html");
    std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
        .expect("write source html");

    let metadata = html_metadata(source_path);
    let mut renderer = ServoRenderer::new();
    renderer.initialized = true;
    renderer.last_canvas = Some(solid_canvas(640, 480, 12, 34, 56));

    renderer
        .init_with_canvas_size(&metadata, 640, 480)
        .expect("renderer should queue initialization");

    load_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("create-session command should be queued asynchronously");

    let input = frame_input_with(1.0 / 30.0, 1, &SILENCE, &DEFAULT_INTERACTION, 640, 480);
    let mut target = Canvas::new(640, 480);
    renderer
        .render_into(&input, &mut target)
        .expect("render should reuse the previous completed frame");

    assert_eq!(target.get_pixel(0, 0), Rgba::new(12, 34, 56, 255));

    release_tx.send(()).expect("release create-session");
    wait_for_load_completion(&mut renderer);

    renderer.destroy();
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("destroy should unload test worker");

    shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn destroy_discards_completed_load_task_before_it_is_polled() {
    let (worker, load_rx, unload_rx, stopped) = spawn_load_test_worker();
    let mut session = ServoSessionHandle::new(
        worker_client_from(&worker),
        SessionConfig {
            render_width: 640,
            render_height: 480,
            inject_engine_globals: true,
            producer_role: ServoProducerRole::SceneHtml,
        },
    )
    .expect("test session should initialize");

    load_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("create-session command should be queued");
    session
        .load_html_file(std::path::Path::new("test.html"))
        .expect("test session should load");

    let (response_tx, response_rx) = std::sync::mpsc::sync_channel(1);
    response_tx
        .send(Ok(LoadedServoSession {
            session,
            runtime_source: PathBuf::from("runtime.html"),
            runtime_html_path: None,
        }))
        .expect("completed load should queue");

    let mut renderer = ServoRenderer::new();
    renderer.load_task = Some(ServoLoadTask {
        response_rx,
        shared: Arc::new(Mutex::new(ServoLoadTaskState { canceled: false })),
        started_at: Instant::now(),
    });

    renderer.destroy();
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("completed background load should be detached during destroy");

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn init_with_canvas_size_loads_servo_page_at_target_resolution() {
    let _lock = SHARED_WORKER_STATE_TEST_LOCK
        .lock()
        .expect("shared worker test lock");
    reset_shared_servo_worker_state();

    let (worker, load_rx, unload_rx, stopped) = spawn_load_test_worker();
    install_running_shared_worker(worker);

    let temp_dir = tempfile::tempdir().expect("temporary directory");
    let source_path = temp_dir.path().join("effect.html");
    std::fs::write(&source_path, "<!doctype html><html><body></body></html>")
        .expect("write source html");

    let metadata = html_metadata(source_path);
    let mut renderer = ServoRenderer::new();
    renderer
        .init_with_canvas_size(&metadata, 640, 480)
        .expect("renderer should initialize");

    let load = load_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("load command should be recorded");
    assert_eq!(load.width, 640);
    assert_eq!(load.height, 480);
    wait_for_load_completion(&mut renderer);
    assert!(
        renderer
            .pending_scripts
            .iter()
            .any(|script| script.contains("window.engine.width = 640"))
    );
    assert!(
        renderer
            .pending_scripts
            .iter()
            .any(|script| script.contains("window.engine.height = 480"))
    );

    renderer.destroy();
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("destroy should unload test worker");

    shutdown_shared_servo_worker().expect("shared worker shutdown should succeed");
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn frame_payloads_track_animation_cap_without_js_throttle() {
    let mut renderer = ServoRenderer::new();
    renderer.enqueue_bootstrap_scripts();
    renderer.pending_scripts.clear();

    renderer.enqueue_frame_payloads(&frame_input(1.0 / 30.0));
    assert_eq!(renderer.last_animation_fps_cap, Some(30));
    assert!(renderer.pending_scripts.is_empty());
    assert_eq!(renderer.pending_frame_payloads.len(), 1);

    renderer.pending_scripts.clear();
    renderer.pending_frame_payloads.clear();
    renderer.enqueue_frame_payloads(&frame_input(1.0 / 15.0));
    assert_eq!(renderer.last_animation_fps_cap, Some(20));
    assert!(renderer.pending_scripts.is_empty());
    assert!(renderer.pending_frame_payloads.is_empty());
}

#[test]
fn host_driven_frames_use_fast_host_script_without_payload() {
    let mut renderer = ServoRenderer::new();
    renderer.host_driven_animation = true;
    renderer.include_audio_updates = false;
    let mut input = frame_input(1.0 / 30.0);
    input.time_secs = 2.5;

    renderer.enqueue_frame_payloads(&input);

    assert!(renderer.pending_frame_payloads.is_empty());
    let script = renderer
        .pending_scripts
        .first()
        .expect("host-driven frame should queue a fast script");
    assert!(script.starts_with("window.__hypercolorApplyHostFrame(2.5,"));
    assert!(
        renderer
            .pending_scripts
            .iter()
            .all(|script| !script.contains("window.__hypercolorRenderHostFrame"))
    );
    assert!(
        renderer
            .pending_scripts
            .iter()
            .all(|script| !script.contains("instance.render("))
    );
}

#[test]
fn host_driven_control_updates_still_use_full_payload() {
    let mut renderer = ServoRenderer::new();
    renderer.host_driven_animation = true;
    renderer.include_audio_updates = false;
    renderer.set_control("speed", &ControlValue::Float(0.75));

    renderer.enqueue_frame_payloads(&frame_input(1.0 / 30.0));

    assert!(renderer.pending_scripts.is_empty());
    assert_eq!(
        frame_payload_value(&renderer.pending_frame_payloads)["renderHostFrame"],
        serde_json::json!(true)
    );
    assert_eq!(
        frame_payload_value(&renderer.pending_frame_payloads)["controls"]["speed"],
        serde_json::json!(0.75)
    );
}

#[test]
fn display_frame_payloads_keep_fixed_animation_cap() {
    let mut renderer = ServoRenderer::new();
    renderer.animation_cadence = AnimationCadence::Fixed(30);
    renderer.host_driven_animation = true;
    renderer.include_audio_updates = false;
    renderer.enqueue_bootstrap_scripts();
    renderer.pending_scripts.clear();

    renderer.enqueue_frame_payloads(&frame_input(1.0 / 60.0));

    assert_eq!(renderer.last_animation_fps_cap, Some(30));
    assert!(renderer.pending_frame_payloads.is_empty());
    assert!(
        renderer
            .pending_scripts
            .first()
            .is_some_and(|script| script.starts_with("window.__hypercolorApplyHostFrame("))
    );
}

#[test]
fn html_effects_use_host_driven_animation() {
    let html = html_metadata(PathBuf::from("effect.html"));
    let display = display_html_metadata(PathBuf::from("display.html"));
    let mut webgl = html_metadata(PathBuf::from("webgl.html"));
    let mut canvas2d = html_metadata(PathBuf::from("canvas2d.html"));
    webgl.tags.push("webgl".to_owned());
    canvas2d.tags.push("canvas2d".to_owned());

    assert!(host_driven_animation(&html));
    assert!(host_driven_animation(&display));
    assert!(!host_driven_animation(&webgl));
    assert!(!host_driven_animation(&canvas2d));
}

#[test]
fn soft_stall_timeout_tracks_active_animation_cap() {
    let mut renderer = ServoRenderer::new();

    assert_eq!(
        renderer.soft_stall_timeout(),
        FpsTier::Medium
            .frame_interval()
            .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
    );

    renderer.last_animation_fps_cap = Some(60);
    assert_eq!(
        renderer.soft_stall_timeout(),
        FpsTier::Full
            .frame_interval()
            .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
    );

    renderer.last_animation_fps_cap = Some(10);
    assert_eq!(
        renderer.soft_stall_timeout(),
        FpsTier::Minimal
            .frame_interval()
            .mul_f32(SOFT_STALL_FRAME_INTERVALS as f32)
    );
}

#[test]
fn poll_in_flight_render_marks_soft_stall_before_hard_timeout() {
    let _soft_stall_guard = SOFT_STALL_TELEMETRY_TEST_LOCK.lock().expect("lock");
    let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
        spawn_render_test_worker();
    let baseline_stalls = crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total;

    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.last_animation_fps_cap = Some(60);
    renderer.last_canvas = Some(solid_canvas(
        DEFAULT_CANVAS_WIDTH,
        DEFAULT_CANVAS_HEIGHT,
        20,
        40,
        60,
    ));
    renderer
        .session
        .as_mut()
        .expect("attached test session")
        .request_render_cpu(Vec::new())
        .expect("test render should queue");
    let _ = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("render command");

    thread::sleep(renderer.soft_stall_timeout() + Duration::from_millis(25));
    renderer.poll_in_flight_render();

    assert!(renderer.warned_stalled_frame);
    assert_eq!(
        crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total,
        baseline_stalls + 1
    );

    renderer.poll_in_flight_render();
    assert_eq!(
        crate::effect::servo::servo_telemetry_snapshot().soft_stalls_total,
        baseline_stalls + 1
    );

    result_tx
        .send(Ok(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            1,
            1,
            1,
        )))
        .expect("cleanup render result");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("cleanup result delivery ack");

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn poll_in_flight_render_clears_stall_warning_after_completed_frame() {
    let _soft_stall_guard = SOFT_STALL_TELEMETRY_TEST_LOCK.lock().expect("lock");
    let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
        spawn_render_test_worker();

    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.last_animation_fps_cap = Some(60);
    renderer.last_canvas = Some(solid_canvas(
        DEFAULT_CANVAS_WIDTH,
        DEFAULT_CANVAS_HEIGHT,
        20,
        40,
        60,
    ));
    renderer
        .session
        .as_mut()
        .expect("attached test session")
        .request_render_cpu(Vec::new())
        .expect("test render should queue");
    let _ = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("render command");

    thread::sleep(renderer.soft_stall_timeout() + Duration::from_millis(25));
    renderer.poll_in_flight_render();
    assert!(renderer.warned_stalled_frame);

    result_tx
        .send(Ok(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            9,
            8,
            7,
        )))
        .expect("completed render result");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("completed result delivery ack");

    renderer.poll_in_flight_render();

    assert!(!renderer.warned_stalled_frame);
    assert_eq!(
        renderer
            .last_canvas
            .as_ref()
            .expect("completed frame")
            .get_pixel(0, 0),
        Rgba::new(9, 8, 7, 255)
    );

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn hard_stalled_render_surfaces_an_error_until_a_frame_completes() {
    let mut renderer = ServoRenderer::new();
    renderer.initialized = true;
    renderer.update_stall_state(
        Some(HARD_STALL_TIMEOUT + Duration::from_millis(1)),
        Duration::from_millis(50),
    );
    let mut target = Canvas::new(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT);

    let error = renderer
        .render_into(&frame_input(1.0 / 60.0), &mut target)
        .expect_err("hard-stalled renders must reach the effect error seam");
    assert!(error.to_string().contains("hard-stalled after 501ms"));
    renderer.update_stall_state(
        Some(HARD_STALL_TIMEOUT + Duration::from_millis(300)),
        Duration::from_millis(50),
    );
    let repeated = renderer
        .render_into(&frame_input(1.0 / 60.0), &mut target)
        .expect_err("hard-stalled renders remain degraded until recovery");
    assert_eq!(repeated.to_string(), error.to_string());

    renderer.handle_poll_error(anyhow::anyhow!("render failed after the stall"));
    let after_error = renderer
        .render_into(&frame_input(1.0 / 60.0), &mut target)
        .expect_err("a failed render is not hard-stall recovery");
    assert_eq!(after_error.to_string(), error.to_string());

    renderer.accept_completed_canvas(
        solid_canvas(DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT, 4, 5, 6),
        false,
    );

    assert!(renderer.hard_stall.is_none());
    assert!(
        renderer
            .render_into(&frame_input(1.0 / 60.0), &mut target)
            .is_ok()
    );
}

#[test]
fn frame_payloads_skip_near_tier_jitter_updates() {
    let mut renderer = ServoRenderer::new();
    renderer.enqueue_bootstrap_scripts();
    renderer.pending_scripts.clear();

    renderer.enqueue_frame_payloads(&frame_input(1.0 / 60.0));
    assert_eq!(renderer.last_animation_fps_cap, Some(60));
    assert!(renderer.pending_scripts.is_empty());
    assert_eq!(renderer.pending_frame_payloads.len(), 1);

    renderer.pending_scripts.clear();
    renderer.pending_frame_payloads.clear();
    renderer.enqueue_frame_payloads(&frame_input(1.0 / 58.0));
    assert_eq!(renderer.last_animation_fps_cap, Some(60));
    assert!(renderer.pending_scripts.is_empty());
    assert!(renderer.pending_frame_payloads.is_empty());
}

#[test]
fn frame_payloads_skip_unchanged_input_updates() {
    let mut renderer = ServoRenderer::new();
    renderer.include_interaction_updates = true;

    renderer.enqueue_frame_payloads(&frame_input(1.0 / 30.0));
    let first_payload = frame_payload_value(&renderer.pending_frame_payloads);
    assert!(first_payload["interaction"].is_object());

    renderer.pending_scripts.clear();
    renderer.pending_frame_payloads.clear();
    renderer.enqueue_frame_payloads(&frame_input(1.0 / 30.0));
    assert!(renderer.pending_scripts.is_empty());
    assert!(renderer.pending_frame_payloads.is_empty());
}

#[test]
fn render_into_without_completed_frame_fills_placeholder_target() {
    let mut renderer = ServoRenderer::new();
    renderer.initialized = true;

    let audio = custom_audio(0.5);
    let interaction = custom_interaction(&[], &[]);
    let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
    let mut target = Canvas::new(1, 1);

    renderer
        .render_into(&input, &mut target)
        .expect("placeholder render should succeed");

    assert_eq!(target.width(), 4);
    assert_eq!(target.height(), 3);
    assert_eq!(target.get_pixel(0, 0), Rgba::new(7, 127, 39, 255));
    assert_eq!(target.get_pixel(3, 2), Rgba::new(7, 127, 39, 255));
}

#[test]
fn render_into_ignores_completed_frame_with_stale_dimensions() {
    let mut renderer = ServoRenderer::new();
    renderer.initialized = true;
    renderer.last_canvas = Some(Canvas::new(2, 2));

    let audio = custom_audio(0.5);
    let interaction = custom_interaction(&[], &[]);
    let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
    let mut target = Canvas::new(1, 1);

    renderer
        .render_into(&input, &mut target)
        .expect("placeholder render should succeed");

    assert_eq!(target.width(), 4);
    assert_eq!(target.height(), 3);
    assert_eq!(target.get_pixel(0, 0), Rgba::new(7, 127, 39, 255));
}

#[test]
fn render_into_copies_completed_frame_into_existing_target_storage() {
    let mut renderer = ServoRenderer::new();
    renderer.initialized = true;
    renderer.last_canvas = Some(solid_canvas(4, 3, 9, 8, 7));

    let audio = custom_audio(0.5);
    let interaction = custom_interaction(&[], &[]);
    let input = frame_input_with(1.0 / 30.0, 7, &audio, &interaction, 4, 3);
    let mut target = Canvas::new(4, 3);
    let target_ptr = target.as_rgba_bytes().as_ptr();

    renderer
        .render_into(&input, &mut target)
        .expect("completed frame render should succeed");

    assert_eq!(target.as_rgba_bytes().as_ptr(), target_ptr);
    assert_eq!(target.get_pixel(0, 0), Rgba::new(9, 8, 7, 255));
    assert_eq!(target.get_pixel(3, 2), Rgba::new(9, 8, 7, 255));
}

#[test]
fn queued_frames_submit_latest_state_after_in_flight_render_finishes() {
    let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
        spawn_render_test_worker();

    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.include_interaction_updates = true;
    renderer.enqueue_bootstrap_scripts();
    renderer.set_control("speed", &ControlValue::Float(0.25));

    let first_audio = custom_audio(0.1);
    let first_interaction = custom_interaction(&["a"], &["a"]);
    let first_frame = frame_input_with(1.0 / 30.0, 1, &first_audio, &first_interaction, 320, 200);

    let first_output = renderer.tick(&first_frame).expect("first tick");
    assert_eq!(first_output.width(), 320);
    assert_eq!(first_output.height(), 200);

    let first_render = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first render command");
    assert_eq!(first_render.width, 320);
    assert_eq!(first_render.height, 200);
    let bootstrap = first_render
        .scripts
        .first()
        .expect("first render should include bootstrap script");
    assert!(bootstrap.contains("window.__hypercolorApplyFramePayload = function(payload)"));
    assert!(first_render.scripts.iter().all(|script| {
        !script
            .trim_start()
            .starts_with("window.__hypercolorApplyFramePayload(")
    }));
    let first_payload = frame_payload_value(&first_render.frame_payloads);
    assert_eq!(first_payload["controls"]["speed"], serde_json::json!(0.25));

    renderer.set_control("speed", &ControlValue::Float(0.75));
    let second_audio = custom_audio(0.6);
    let second_interaction = custom_interaction(&["b"], &["b"]);
    let second_frame =
        frame_input_with(1.0 / 15.0, 2, &second_audio, &second_interaction, 640, 360);
    renderer.tick(&second_frame).expect("second tick");
    assert!(render_rx.recv_timeout(Duration::from_millis(20)).is_err());

    let third_interaction = custom_interaction(&["c"], &["c"]);
    let third_frame = frame_input_with(1.0 / 15.0, 3, &second_audio, &third_interaction, 640, 360);
    renderer.tick(&third_frame).expect("third tick");
    assert!(render_rx.recv_timeout(Duration::from_millis(20)).is_err());

    result_tx
        .send(Ok(solid_canvas(640, 360, 9, 8, 7)))
        .expect("first result should be delivered");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first result delivery ack");

    let resumed_output = renderer.tick(&third_frame).expect("resume tick");
    assert_eq!(resumed_output.get_pixel(0, 0), Rgba::new(9, 8, 7, 255));

    let second_render = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("second render command");
    assert_eq!(second_render.width, 640);
    assert_eq!(second_render.height, 360);
    assert!(
        second_render
            .scripts
            .iter()
            .all(|script| !script.contains("__hypercolorFpsCap"))
    );
    let second_payload = frame_payload_value(&second_render.frame_payloads);
    assert_eq!(second_payload["canvas"]["width"], serde_json::json!(640));
    assert_eq!(second_payload["controls"]["speed"], serde_json::json!(0.75));
    let recent_keys = second_payload["interaction"]["keyboard"]["recent"]
        .as_array()
        .expect("recent keys should be an array");
    assert!(recent_keys.contains(&serde_json::json!("b")));
    assert!(recent_keys.contains(&serde_json::json!("c")));
    assert_eq!(
        second_payload["interaction"]["mouse"]["down"],
        serde_json::json!(false)
    );

    result_tx
        .send(Ok(solid_canvas(640, 360, 1, 1, 1)))
        .expect("cleanup render result");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("cleanup result delivery ack");

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn queued_frames_do_not_retain_undemanded_input_domains() {
    let audio = custom_audio(0.8);
    let interaction = custom_interaction(&["space"], &["space"]);
    let screen = crate::input::ScreenData::from_zones(Vec::new(), 8, 6);
    let mut sensors = SystemSnapshot::empty();
    sensors.cpu_loads = vec![10.0, 20.0, 30.0];
    let media = hypercolor_types::media::MediaState {
        track: "heavy-track".to_owned(),
        art_data_url: Some("data:image/jpeg;base64,payload".to_owned()),
        ..Default::default()
    };
    let net = hypercolor_types::net::NetStats {
        iface: "ethernet-test".to_owned(),
        ..Default::default()
    };
    let lighting = hypercolor_types::lighting::LightingState {
        effect_names: vec!["large-lighting-state".to_owned()],
        ..Default::default()
    };
    let input = FrameInput {
        time_secs: 1.0,
        delta_secs: 1.0 / 30.0,
        frame_number: 30,
        audio: &audio,
        interaction: &interaction,
        screen: Some(&screen),
        sensors: &sensors,
        sources: crate::effect::traits::FrameDataSources {
            media: Some(&media),
            net: Some(&net),
            lighting: Some(&lighting),
        },
        canvas_width: 320,
        canvas_height: 200,
    };
    let mut renderer = ServoRenderer::new();
    renderer.include_audio_updates = false;

    renderer.queue_frame(&input);

    let queued = renderer.queued_frame.as_ref().expect("queued frame");
    assert_eq!(queued.retained_input_domains(), [false; 7]);
}

#[test]
fn queued_frames_merge_recent_keys_from_superseded_inputs() {
    let audio = custom_audio(0.0);
    let first_interaction = custom_interaction(&["a", "b"], &["a"]);
    let second_interaction = custom_interaction(&["b", "c"], &["c"]);
    let first = frame_input_with(1.0 / 30.0, 1, &audio, &first_interaction, 320, 200);
    let second = frame_input_with(1.0 / 30.0, 2, &audio, &second_interaction, 320, 200);
    let mut renderer = ServoRenderer::new();
    renderer.include_audio_updates = false;
    renderer.include_interaction_updates = true;

    renderer.queue_frame(&first);
    renderer.queue_frame(&second);

    let queued = renderer.queued_frame.as_ref().expect("queued frame");
    let interaction = queued.queued_interaction().expect("demanded interaction");
    assert_eq!(interaction.keyboard.pressed_keys, ["c"]);
    assert_eq!(interaction.keyboard.recent_keys, ["b", "c", "a"]);
}

#[test]
fn queue_saturation_preserves_runtime_deltas_until_admission() {
    let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
        spawn_render_test_worker();
    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.include_interaction_updates = true;
    renderer
        .pending_scripts
        .push("persistent-control".to_owned());
    let reservations = renderer
        .session
        .as_ref()
        .expect("attached session")
        .reserve_all_render_capacity()
        .expect("test should exhaust render admission");
    let audio = custom_audio(0.0);
    let interaction = custom_interaction(&["space"], &["space"]);
    let input = frame_input_with(1.0 / 60.0, 3, &audio, &interaction, 320, 200);

    let error = renderer
        .tick(&input)
        .expect_err("saturated render admission should report degradation");
    assert!(error.to_string().contains("render queue is saturated"));
    assert_eq!(renderer.pending_scripts, ["persistent-control"]);
    assert!(renderer.pending_frame_payloads.is_empty());
    assert_eq!(
        renderer
            .queued_frame
            .as_ref()
            .expect("latest queued frame")
            .queued_frame_number(),
        3
    );
    assert!(render_rx.try_recv().is_err());

    drop(reservations);
    renderer
        .tick(&input)
        .expect("the retained frame should submit when capacity returns");
    let command = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("retained render command");
    assert_eq!(command.scripts, ["persistent-control"]);
    assert!(frame_payload_value(&command.frame_payloads)["interaction"].is_object());

    result_tx
        .send(Ok(solid_canvas(320, 200, 1, 2, 3)))
        .expect("render result");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("render delivery");
    renderer.destroy();
    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn tick_reuses_last_completed_canvas_while_next_servo_frame_is_pending() {
    let (worker, render_rx, result_tx, delivered_rx, _unload_rx, stopped) =
        spawn_render_test_worker();

    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.enqueue_bootstrap_scripts();

    let interaction = custom_interaction(&[], &[]);
    let audio = custom_audio(0.0);
    let frame = frame_input_with(1.0 / 30.0, 1, &audio, &interaction, 320, 200);

    renderer.tick(&frame).expect("initial tick");
    let _ = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first render command");

    result_tx
        .send(Ok(solid_canvas(320, 200, 20, 40, 60)))
        .expect("first result should be delivered");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first result delivery ack");

    let first_completed = renderer.tick(&frame).expect("completed tick");
    assert_eq!(first_completed.get_pixel(0, 0), Rgba::new(20, 40, 60, 255));
    let _ = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("second render command");

    let reused = renderer.tick(&frame).expect("reused frame");
    assert_eq!(reused.get_pixel(0, 0), Rgba::new(20, 40, 60, 255));
    assert!(render_rx.recv_timeout(Duration::from_millis(20)).is_err());

    result_tx
        .send(Ok(solid_canvas(320, 200, 1, 1, 1)))
        .expect("cleanup render result");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("cleanup result delivery ack");

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}

#[test]
fn destroy_detaches_in_flight_render_before_unloading_worker_page() {
    let (worker, render_rx, result_tx, delivered_rx, unload_rx, stopped) =
        spawn_render_test_worker();

    let mut renderer = ServoRenderer::new();
    attach_renderer_session(&mut renderer, &worker);
    renderer.initialized = true;
    renderer.enqueue_bootstrap_scripts();

    let interaction = custom_interaction(&[], &[]);
    let audio = custom_audio(0.0);
    let frame = frame_input_with(1.0 / 30.0, 1, &audio, &interaction, 320, 200);

    renderer.tick(&frame).expect("initial tick");
    let _ = render_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("first render command");

    let started_at = std::time::Instant::now();

    renderer.destroy();

    assert!(started_at.elapsed() < Duration::from_millis(20));
    assert!(unload_rx.recv_timeout(Duration::from_millis(20)).is_err());
    result_tx
        .send(Ok(solid_canvas(
            DEFAULT_CANVAS_WIDTH,
            DEFAULT_CANVAS_HEIGHT,
            7,
            8,
            9,
        )))
        .expect("cleanup render result");
    delivered_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("cleanup result delivery ack");
    unload_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("destroy should unload the active Servo page");

    drop(worker);
    assert!(stopped.load(Ordering::SeqCst));
}
