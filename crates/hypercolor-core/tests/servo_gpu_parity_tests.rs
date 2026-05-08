#![cfg(all(target_os = "linux", feature = "servo-gpu-import"))]

use std::fs;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use hypercolor_core::effect::{
    EffectRenderOutput, EffectRenderer, FrameInput, ServoRenderer, install_servo_gpu_import_device,
    servo_gpu_import_device, servo_telemetry_snapshot, set_servo_gpu_import_mode,
};
use hypercolor_core::input::InteractionData;
use hypercolor_linux_gpu_interop::check_wgpu_vulkan_external_memory_fd;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::config::ServoGpuImportMode;
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::sensor::SystemSnapshot;
use tempfile::tempdir;
use uuid::Uuid;

const WIDTH: u32 = 8;
const HEIGHT: u32 = 8;
const EXPECTED_PIXEL: [u8; 4] = [0, 255, 255, 255];
const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const RENDER_ATTEMPTS: u64 = 180;
const RUN_PARITY_ENV: &str = "HYPERCOLOR_RUN_SERVO_GPU_PARITY";
const CHILD_PARITY_ENV: &str = "HYPERCOLOR_SERVO_GPU_PARITY_CHILD";
const PARITY_MARKER_ENV: &str = "HYPERCOLOR_SERVO_GPU_PARITY_MARKER";

#[test]
fn deterministic_servo_gpu_import_matches_cpu_readback() {
    if std::env::var_os(RUN_PARITY_ENV).is_none() {
        eprintln!("set {RUN_PARITY_ENV}=1 to run the Servo GPU parity fixture");
        return;
    }
    if std::env::var_os(CHILD_PARITY_ENV).is_none() {
        let marker_dir = tempdir().expect("parity marker temp dir should be created");
        let marker_path = marker_dir.path().join("servo-gpu-parity.ok");
        let output = Command::new(std::env::current_exe().expect("test binary path"))
            .arg("--exact")
            .arg("deterministic_servo_gpu_import_matches_cpu_readback")
            .arg("--nocapture")
            .env(RUN_PARITY_ENV, "1")
            .env(CHILD_PARITY_ENV, "1")
            .env(PARITY_MARKER_ENV, &marker_path)
            .output()
            .expect("Servo GPU parity child process should run");
        let marker_ok = fs::read_to_string(&marker_path).is_ok_and(|value| value == "ok");
        assert!(
            output.status.success() || marker_ok,
            "Servo GPU parity child failed before proof: status={}; stdout={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    run_deterministic_servo_gpu_import_parity();
    unreachable!("Servo GPU parity child exits after proving parity");
}

fn run_deterministic_servo_gpu_import_parity() {
    let temp_dir = tempdir().expect("parity fixture temp dir should be created");
    let html_path = temp_dir.path().join("servo-gpu-parity.html");
    fs::write(&html_path, parity_html()).expect("parity fixture HTML should be written");
    let metadata = html_metadata(html_path);

    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
    let cpu_canvas = render_cpu_canvas(metadata.clone());
    assert_solid_cyan(&cpu_canvas);

    let wgpu = WgpuFixture::new().expect("Servo GPU parity fixture should create wgpu device");
    if servo_gpu_import_device().is_err() {
        install_servo_gpu_import_device(wgpu.device.clone())
            .expect("Servo GPU parity fixture should install wgpu device");
    }
    set_servo_gpu_import_mode(ServoGpuImportMode::On);
    let before_gpu = servo_telemetry_snapshot();
    let gpu_pixels = render_gpu_pixels(metadata, &wgpu.device, &wgpu.queue);
    let after_gpu = servo_telemetry_snapshot();

    assert_eq!(gpu_pixels, cpu_canvas.as_rgba_bytes());
    assert!(
        after_gpu.render_gpu_frames_total > before_gpu.render_gpu_frames_total,
        "Servo GPU path should emit an imported frame"
    );
    assert!(
        after_gpu.render_gpu_import_total_us > before_gpu.render_gpu_import_total_us,
        "Servo GPU import timing should advance"
    );
    assert_eq!(
        after_gpu.render_readback_total_us, before_gpu.render_readback_total_us,
        "Servo GPU import path should not add CPU readback time"
    );
    let marker_path =
        std::env::var_os(PARITY_MARKER_ENV).expect("parity child should receive marker path");
    fs::write(marker_path, "ok").expect("parity child should write proof marker");
    set_servo_gpu_import_mode(ServoGpuImportMode::Off);
    std::mem::forget(gpu_pixels);
    std::mem::forget(cpu_canvas);
    std::mem::forget(wgpu);
    std::mem::forget(temp_dir);
    // Servo can trip a pthread mutex destroy failure during GPU-import
    // teardown. The parent test observes this child status.
    std::process::exit(0);
}

fn parity_html() -> &'static str {
    r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<style>
html, body {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
  background: #00ffff;
}
</style>
</head>
<body></body>
</html>
"#
}

fn html_metadata(path: std::path::PathBuf) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "servo-gpu-parity".to_owned(),
        author: "hypercolor-tests".to_owned(),
        version: "0.1.0".to_owned(),
        description: "deterministic Servo GPU parity fixture".to_owned(),
        category: EffectCategory::Ambient,
        tags: vec!["servo".to_owned(), "gpu".to_owned(), "parity".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Html { path },
        license: None,
    }
}

fn render_cpu_canvas(metadata: EffectMetadata) -> Canvas {
    let mut renderer = ServoRenderer::new();
    renderer
        .init_with_canvas_size(&metadata, WIDTH, HEIGHT)
        .expect("CPU Servo renderer should initialize");

    for frame_number in 0..RENDER_ATTEMPTS {
        let input = frame_input(frame_number);
        let output = renderer
            .render_output(&input)
            .expect("CPU Servo render should succeed");
        if let EffectRenderOutput::Cpu(canvas) = output
            && canvas_is_solid_cyan(&canvas)
        {
            std::mem::forget(renderer);
            return canvas;
        }
        thread::sleep(Duration::from_millis(16));
    }

    std::mem::forget(renderer);
    panic!("CPU Servo renderer did not produce the deterministic fixture frame");
}

fn render_gpu_pixels(
    metadata: EffectMetadata,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Vec<u8> {
    let mut renderer = ServoRenderer::new();
    renderer
        .init_with_canvas_size(&metadata, WIDTH, HEIGHT)
        .expect("GPU Servo renderer should initialize");

    for frame_number in 0..RENDER_ATTEMPTS {
        let input = frame_input(frame_number);
        let output = renderer
            .render_output(&input)
            .expect("GPU Servo render should succeed");
        if let EffectRenderOutput::Gpu(frame) = output {
            let pixels =
                read_texture_pixels(device, queue, &frame.texture, frame.width, frame.height);
            std::mem::forget(frame);
            std::mem::forget(renderer);
            return pixels;
        }
        thread::sleep(Duration::from_millis(16));
    }

    std::mem::forget(renderer);
    panic!("GPU Servo renderer did not produce an imported texture frame");
}

fn frame_input(frame_number: u64) -> FrameInput<'static> {
    static AUDIO: std::sync::LazyLock<AudioData> = std::sync::LazyLock::new(AudioData::silence);
    static INTERACTION: std::sync::LazyLock<InteractionData> =
        std::sync::LazyLock::new(InteractionData::default);
    static SENSORS: std::sync::LazyLock<SystemSnapshot> =
        std::sync::LazyLock::new(SystemSnapshot::empty);

    FrameInput {
        time_secs: frame_number as f32 * FRAME_DT_SECONDS,
        delta_secs: FRAME_DT_SECONDS,
        frame_number,
        audio: &AUDIO,
        interaction: &INTERACTION,
        screen: None,
        sensors: &SENSORS,
        canvas_width: WIDTH,
        canvas_height: HEIGHT,
    }
}

fn assert_solid_cyan(canvas: &Canvas) {
    assert!(
        canvas_is_solid_cyan(canvas),
        "CPU Servo fixture did not produce solid cyan"
    );
}

fn canvas_is_solid_cyan(canvas: &Canvas) -> bool {
    canvas
        .as_rgba_bytes()
        .chunks_exact(4)
        .all(|pixel| pixel == EXPECTED_PIXEL)
}

struct WgpuFixture {
    _instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl WgpuFixture {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })) {
                Ok(adapter) => adapter,
                Err(error) => return Err(format!("could not create wgpu adapter: {error}")),
            };
        let adapter_info = adapter.get_info();
        if adapter_info.backend != wgpu::Backend::Vulkan {
            return Err(format!(
                "requires Vulkan wgpu backend, got {:?}",
                adapter_info.backend
            ));
        }

        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("hypercolor Servo GPU parity fixture"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })) {
                Ok(result) => result,
                Err(error) => return Err(format!("could not create wgpu device: {error}")),
            };

        check_wgpu_vulkan_external_memory_fd(&device)
            .map_err(|error| format!("missing Vulkan external memory: {error}"))?;

        Ok(Self {
            _instance: instance,
            device,
            queue,
        })
    }
}

fn read_texture_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = padded_bytes_per_row(width);
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(height);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hypercolor Servo GPU parity readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("hypercolor Servo GPU parity readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..buffer_size);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })
        .expect("parity readback poll should complete");
    receiver
        .recv()
        .expect("parity readback channel should receive map result")
        .expect("parity readback buffer should map");

    let mapped = slice.get_mapped_range();
    let mut pixels = vec![0; (height * unpadded_bytes_per_row) as usize];
    for (target, source) in pixels
        .chunks_exact_mut(unpadded_bytes_per_row as usize)
        .zip(
            mapped
                .chunks(padded_bytes_per_row as usize)
                .take(height as usize),
        )
    {
        target.copy_from_slice(&source[..unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    buffer.unmap();

    pixels
}

const fn padded_bytes_per_row(width: u32) -> u32 {
    let unpadded = width * 4;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    unpadded.div_ceil(alignment) * alignment
}
