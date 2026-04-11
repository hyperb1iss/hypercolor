use std::collections::HashMap;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Result;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hypercolor_core::bus::CanvasFrame;
use hypercolor_core::device::{BackendInfo, BackendManager, DeviceBackend};
use hypercolor_core::effect::builtin::{
    ColorWaveRenderer, GradientRenderer, RainbowRenderer, SolidColorRenderer,
    register_builtin_effects,
};
use hypercolor_core::effect::{
    EffectEngine, EffectPool, EffectRegistry, EffectRenderer, FrameInput,
};
use hypercolor_core::input::InputSource;
use hypercolor_core::input::InteractionData;
use hypercolor_core::input::audio::AudioInput;
use hypercolor_core::input::audio::beat::{BeatDetector, BeatFrame};
use hypercolor_core::input::audio::fft::FftPipeline;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::{AudioData, AudioPipelineConfig, AudioSourceType};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlType, ControlValue, EffectCategory,
    EffectId, EffectMetadata, EffectSource,
};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::{RenderGroup, RenderGroupId};
use hypercolor_types::sensor::SystemSnapshot;
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::runtime::Runtime;
use uuid::Uuid;

const CANVAS_WIDTH: u32 = 320;
const CANVAS_HEIGHT: u32 = 200;
const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const SAMPLE_RATE_HZ: u32 = 48_000;

static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static DEFAULT_INTERACTION: LazyLock<InteractionData> = LazyLock::new(InteractionData::default);
static EMPTY_SENSORS: LazyLock<SystemSnapshot> = LazyLock::new(SystemSnapshot::empty);
static BINDING_SNAPSHOTS: LazyLock<[SystemSnapshot; 2]> =
    LazyLock::new(|| [binding_snapshot(false), binding_snapshot(true)]);

struct BindingBenchRenderer {
    controls: HashMap<String, ControlValue>,
}

impl BindingBenchRenderer {
    fn new() -> Self {
        Self {
            controls: HashMap::new(),
        }
    }
}

impl EffectRenderer for BindingBenchRenderer {
    fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
        Ok(())
    }

    fn render_into(&mut self, input: &FrameInput<'_>, target: &mut Canvas) -> Result<()> {
        if target.width() != input.canvas_width || target.height() != input.canvas_height {
            *target = Canvas::new(input.canvas_width, input.canvas_height);
        }
        black_box(self.controls.len());
        Ok(())
    }

    fn set_control(&mut self, name: &str, value: &ControlValue) {
        self.controls.insert(name.to_owned(), value.clone());
    }

    fn destroy(&mut self) {}
}

struct NullBenchBackend;

#[async_trait::async_trait]
impl DeviceBackend for NullBenchBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "bench".to_owned(),
            name: "Null Bench Backend".to_owned(),
            description: "Discards frame writes during routing benchmarks".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<hypercolor_types::device::DeviceInfo>> {
        Ok(Vec::new())
    }

    async fn connect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn disconnect(&mut self, _id: &DeviceId) -> Result<()> {
        Ok(())
    }

    async fn write_colors(&mut self, _id: &DeviceId, _colors: &[[u8; 3]]) -> Result<()> {
        Ok(())
    }
}

fn benchmark_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(50)
}

fn ambient_metadata(name: &str) -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: name.to_owned(),
        author: "hypercolor".to_owned(),
        version: "0.1.0".to_owned(),
        description: format!("{name} benchmark"),
        category: EffectCategory::Ambient,
        tags: vec!["benchmark".to_owned()],
        controls: Vec::new(),
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        source: EffectSource::Native {
            path: PathBuf::from(format!("builtin/{name}")),
        },
        license: None,
    }
}

fn binding_metadata(binding_count: usize) -> EffectMetadata {
    const SENSOR_LABELS: [&str; 5] = ["cpu_temp", "gpu_temp", "gpu_load", "ram_used", "cpu_load"];

    let mut metadata = ambient_metadata("binding_bench");
    metadata.controls = SENSOR_LABELS
        .iter()
        .enumerate()
        .map(|(index, sensor)| ControlDefinition {
            id: format!("control_{index}"),
            name: format!("Control {index}"),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(0.5),
            min: Some(0.0),
            max: Some(1.0),
            step: Some(0.01),
            labels: Vec::new(),
            group: Some("Bench".to_owned()),
            tooltip: None,
            binding: (index < binding_count).then(|| ControlBinding {
                sensor: (*sensor).to_owned(),
                sensor_min: 0.0,
                sensor_max: 100.0,
                target_min: 0.0,
                target_max: 1.0,
                deadband: 0.0,
                smoothing: 0.0,
            }),
        })
        .collect();
    metadata
}

fn binding_snapshot(hot: bool) -> SystemSnapshot {
    SystemSnapshot {
        cpu_load_percent: if hot { 71.0 } else { 41.0 },
        cpu_loads: if hot {
            vec![68.0, 72.0, 70.0, 74.0]
        } else {
            vec![38.0, 44.0, 40.0, 42.0]
        },
        cpu_temp_celsius: Some(if hot { 84.0 } else { 58.0 }),
        gpu_temp_celsius: Some(if hot { 79.0 } else { 63.0 }),
        gpu_load_percent: Some(if hot { 91.0 } else { 72.0 }),
        gpu_vram_used_mb: Some(if hot { 3_072.0 } else { 2_048.0 }),
        ram_used_percent: if hot { 73.0 } else { 54.0 },
        ram_used_mb: if hot { 22_528.0 } else { 16_384.0 },
        ram_total_mb: 32_768.0,
        components: Vec::new(),
        polled_at_ms: 1_715_000_000,
    }
}

fn frame_input(time_secs: f32, frame_number: u64, audio: &AudioData) -> FrameInput<'_> {
    FrameInput {
        time_secs,
        delta_secs: FRAME_DT_SECONDS,
        frame_number,
        audio,
        interaction: &DEFAULT_INTERACTION,
        screen: None,
        sensors: &EMPTY_SENSORS,
        canvas_width: CANVAS_WIDTH,
        canvas_height: CANVAS_HEIGHT,
    }
}

#[expect(clippy::cast_precision_loss, clippy::as_conversions)]
fn frame_time(frame_number: u64) -> f32 {
    frame_number as f32 * FRAME_DT_SECONDS
}

#[expect(clippy::cast_precision_loss, clippy::as_conversions)]
fn frame_time_f64(frame_number: u64) -> f64 {
    frame_number as f64 * f64::from(FRAME_DT_SECONDS)
}

fn manual_audio_config() -> AudioPipelineConfig {
    AudioPipelineConfig {
        source: AudioSourceType::None,
        ..AudioPipelineConfig::default()
    }
}

fn patterned_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    let width_span = width.saturating_sub(1).max(1);
    let height_span = height.saturating_sub(1).max(1);
    let diagonal_span = width_span + height_span;

    for y in 0..height {
        for x in 0..width {
            let red = u8::try_from((x * 255) / width_span).expect("red channel fits in u8");
            let green = u8::try_from((y * 255) / height_span).expect("green channel fits in u8");
            let blue =
                u8::try_from(((x + y) * 255) / diagonal_span).expect("blue channel fits in u8");
            canvas.set_pixel(x, y, Rgba::new(red, green, blue, 255));
        }
    }

    canvas
}

fn full_canvas_zone(id: &str, topology: LedTopology) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: format!("bench:{id}"),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology,
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
    }
}

fn layout_with_zone(zone: DeviceZone) -> SpatialLayout {
    SpatialLayout {
        id: "benchmark-layout".to_owned(),
        name: "Benchmark Layout".to_owned(),
        description: None,
        canvas_width: CANVAS_WIDTH,
        canvas_height: CANVAS_HEIGHT,
        zones: vec![zone],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn registry_with_builtins() -> EffectRegistry {
    let mut registry = EffectRegistry::new(Vec::new());
    register_builtin_effects(&mut registry);
    registry
}

fn builtin_effect_id(registry: &EffectRegistry, stem: &str) -> EffectId {
    registry
        .iter()
        .find_map(|(id, entry)| (entry.metadata.source.source_stem() == Some(stem)).then_some(*id))
        .expect("builtin effect should be registered")
}

fn render_group(
    zone_id: &str,
    device_id: &str,
    led_count: u32,
    color: [f32; 4],
    effect_id: EffectId,
) -> RenderGroup {
    RenderGroup {
        id: RenderGroupId::new(),
        name: zone_id.to_owned(),
        description: None,
        effect_id: Some(effect_id),
        controls: HashMap::from([("color".to_owned(), ControlValue::Color(color))]),
        preset_id: None,
        layout: layout_with_zone(bench_routing_zone(zone_id, device_id, led_count, None)),
        brightness: 1.0,
        enabled: true,
        color: None,
    }
}

fn bench_routing_zone(
    id: &str,
    device_id: &str,
    led_count: u32,
    led_mapping: Option<Vec<u32>>,
) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: None,
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(1.0, 1.0),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Strip {
            count: led_count,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
    }
}

#[expect(clippy::cast_precision_loss, clippy::as_conversions)]
fn sine_wave(freq_hz: f32, sample_rate_hz: u32, sample_count: usize) -> Vec<f32> {
    (0..sample_count)
        .map(|index| {
            let time = index as f32 / sample_rate_hz as f32;
            (2.0 * std::f32::consts::PI * freq_hz * time).sin()
        })
        .collect()
}

fn synthetic_beat_frame(frame_number: u64) -> BeatFrame {
    let downbeat = frame_number.is_multiple_of(16);
    let accent = frame_number.is_multiple_of(4);

    BeatFrame {
        bass: if downbeat {
            0.92
        } else if accent {
            0.38
        } else {
            0.08
        },
        mid: if accent { 0.24 } else { 0.10 },
        treble: if frame_number.is_multiple_of(2) {
            0.18
        } else {
            0.06
        },
        spectral_flux: if downbeat {
            0.95
        } else if accent {
            0.32
        } else {
            0.04
        },
        dt: FRAME_DT_SECONDS,
        current_time: frame_time_f64(frame_number),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the benchmark wires up a representative matrix of renderer entry points"
)]
fn bench_builtin_renderers(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_render");
    group.throughput(Throughput::Elements(
        u64::from(CANVAS_WIDTH) * u64::from(CANVAS_HEIGHT),
    ));

    let mut solid = SolidColorRenderer::new();
    solid
        .init(&ambient_metadata("solid_color"))
        .expect("solid color renderer should initialize");
    let mut solid_frame = 0_u64;
    group.bench_function(
        BenchmarkId::new("solid_color", format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}")),
        |b| {
            b.iter(|| {
                let input = frame_input(frame_time(solid_frame), solid_frame, &SILENCE);
                solid_frame += 1;
                let canvas = solid
                    .tick(black_box(&input))
                    .expect("solid color renderer should tick");
                black_box(canvas);
            });
        },
    );
    let mut solid_into = SolidColorRenderer::new();
    solid_into
        .init(&ambient_metadata("solid_color"))
        .expect("solid color renderer should initialize");
    let mut solid_into_frame = 0_u64;
    let mut solid_into_canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    group.bench_function(
        BenchmarkId::new(
            "solid_color_render_into",
            format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}"),
        ),
        |b| {
            b.iter(|| {
                let input = frame_input(frame_time(solid_into_frame), solid_into_frame, &SILENCE);
                solid_into_frame += 1;
                solid_into
                    .render_into(black_box(&input), black_box(&mut solid_into_canvas))
                    .expect("solid color renderer should render into target");
                black_box(solid_into_canvas.as_rgba_bytes());
            });
        },
    );

    let mut gradient = GradientRenderer::new();
    gradient
        .init(&ambient_metadata("gradient"))
        .expect("gradient renderer should initialize");
    let mut gradient_frame = 0_u64;
    group.bench_function(
        BenchmarkId::new("gradient", format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}")),
        |b| {
            b.iter(|| {
                let input = frame_input(frame_time(gradient_frame), gradient_frame, &SILENCE);
                gradient_frame += 1;
                let canvas = gradient
                    .tick(black_box(&input))
                    .expect("gradient renderer should tick");
                black_box(canvas);
            });
        },
    );
    let mut gradient_into = GradientRenderer::new();
    gradient_into
        .init(&ambient_metadata("gradient"))
        .expect("gradient renderer should initialize");
    let mut gradient_into_frame = 0_u64;
    let mut gradient_into_canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    group.bench_function(
        BenchmarkId::new(
            "gradient_render_into",
            format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}"),
        ),
        |b| {
            b.iter(|| {
                let input = frame_input(
                    frame_time(gradient_into_frame),
                    gradient_into_frame,
                    &SILENCE,
                );
                gradient_into_frame += 1;
                gradient_into
                    .render_into(black_box(&input), black_box(&mut gradient_into_canvas))
                    .expect("gradient renderer should render into target");
                black_box(gradient_into_canvas.as_rgba_bytes());
            });
        },
    );

    let mut rainbow = RainbowRenderer::new();
    rainbow
        .init(&ambient_metadata("rainbow"))
        .expect("rainbow renderer should initialize");
    let mut rainbow_frame = 0_u64;
    group.bench_function(
        BenchmarkId::new("rainbow", format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}")),
        |b| {
            b.iter(|| {
                let input = frame_input(frame_time(rainbow_frame), rainbow_frame, &SILENCE);
                rainbow_frame += 1;
                let canvas = rainbow
                    .tick(black_box(&input))
                    .expect("rainbow renderer should tick");
                black_box(canvas);
            });
        },
    );
    let mut rainbow_into = RainbowRenderer::new();
    rainbow_into
        .init(&ambient_metadata("rainbow"))
        .expect("rainbow renderer should initialize");
    let mut rainbow_into_frame = 0_u64;
    let mut rainbow_into_canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    group.bench_function(
        BenchmarkId::new(
            "rainbow_render_into",
            format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}"),
        ),
        |b| {
            b.iter(|| {
                let input =
                    frame_input(frame_time(rainbow_into_frame), rainbow_into_frame, &SILENCE);
                rainbow_into_frame += 1;
                rainbow_into
                    .render_into(black_box(&input), black_box(&mut rainbow_into_canvas))
                    .expect("rainbow renderer should render into target");
                black_box(rainbow_into_canvas.as_rgba_bytes());
            });
        },
    );

    let mut color_wave = ColorWaveRenderer::new();
    color_wave
        .init(&ambient_metadata("color_wave"))
        .expect("color wave renderer should initialize");
    for warmup_frame in 0..60_u64 {
        let input = frame_input(frame_time(warmup_frame), warmup_frame, &SILENCE);
        let _ = color_wave
            .tick(&input)
            .expect("color wave warmup frame should render");
    }
    let mut color_wave_frame = 60_u64;
    group.bench_function(
        BenchmarkId::new("color_wave", format!("{CANVAS_WIDTH}x{CANVAS_HEIGHT}")),
        |b| {
            b.iter(|| {
                let input = frame_input(frame_time(color_wave_frame), color_wave_frame, &SILENCE);
                color_wave_frame += 1;
                let canvas = color_wave
                    .tick(black_box(&input))
                    .expect("color wave renderer should tick");
                black_box(canvas);
            });
        },
    );

    group.finish();
}

fn bench_spatial_sampling(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_spatial");
    let canvas = patterned_canvas(CANVAS_WIDTH, CANVAS_HEIGHT);

    let strip_500 = SpatialEngine::new(layout_with_zone(full_canvas_zone(
        "strip-500",
        LedTopology::Strip {
            count: 500,
            direction: StripDirection::LeftToRight,
        },
    )));
    let mut strip_500_output = Vec::new();
    group.throughput(Throughput::Elements(500));
    group.bench_function(BenchmarkId::new("sample_into", "strip_500"), |b| {
        b.iter(|| {
            strip_500.sample_into(black_box(&canvas), &mut strip_500_output);
            black_box(&strip_500_output);
        });
    });

    let matrix_2000 = SpatialEngine::new(layout_with_zone(full_canvas_zone(
        "matrix-2000",
        LedTopology::Matrix {
            width: 50,
            height: 40,
            serpentine: true,
            start_corner: Corner::TopLeft,
        },
    )));
    let mut matrix_2000_output = Vec::new();
    group.throughput(Throughput::Elements(2_000));
    group.bench_function(BenchmarkId::new("sample_into", "matrix_2000"), |b| {
        b.iter(|| {
            matrix_2000.sample_into(black_box(&canvas), &mut matrix_2000_output);
            black_box(&matrix_2000_output);
        });
    });

    let matrix_5000 = SpatialEngine::new(layout_with_zone(full_canvas_zone(
        "matrix-5000",
        LedTopology::Matrix {
            width: 100,
            height: 50,
            serpentine: true,
            start_corner: Corner::TopLeft,
        },
    )));
    let mut matrix_5000_output = Vec::new();
    group.throughput(Throughput::Elements(5_000));
    group.bench_function(BenchmarkId::new("sample_into", "matrix_5000"), |b| {
        b.iter(|| {
            matrix_5000.sample_into(black_box(&canvas), &mut matrix_5000_output);
            black_box(&matrix_5000_output);
        });
    });

    group.finish();
}

fn bench_render_groups(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_render_groups");
    let registry = registry_with_builtins();
    let solid_id = builtin_effect_id(&registry, "solid_color");

    for group_count in [2_usize, 4_usize] {
        let groups = (0..group_count)
            .map(|index| {
                let color = match index % 4 {
                    0 => [1.0, 0.0, 0.0, 1.0],
                    1 => [0.0, 1.0, 0.0, 1.0],
                    2 => [0.0, 0.0, 1.0, 1.0],
                    _ => [1.0, 1.0, 0.0, 1.0],
                };
                render_group(
                    &format!("zone_group_{index}"),
                    &format!("bench:group-{index}"),
                    120,
                    color,
                    solid_id,
                )
            })
            .collect::<Vec<_>>();
        let throughput_leds = u64::try_from(group_count)
            .unwrap_or(u64::MAX)
            .saturating_mul(120);
        let mut pool = EffectPool::new();
        pool.reconcile(&groups, &registry)
            .expect("group pool should reconcile");
        let mut canvases = groups
            .iter()
            .map(|group| Canvas::new(group.layout.canvas_width, group.layout.canvas_height))
            .collect::<Vec<_>>();
        let spatial_engines = groups
            .iter()
            .map(|group| SpatialEngine::new(group.layout.clone()))
            .collect::<Vec<_>>();
        let mut sampled = vec![Vec::<ZoneColors>::new(); group_count];

        group.throughput(Throughput::Elements(throughput_leds));
        group.bench_function(BenchmarkId::new("render_sample", group_count), |b| {
            b.iter(|| {
                for (index, render_group) in groups.iter().enumerate() {
                    pool.render_group_into(
                        black_box(render_group),
                        FRAME_DT_SECONDS,
                        black_box(&SILENCE),
                        black_box(&DEFAULT_INTERACTION),
                        None,
                        black_box(&EMPTY_SENSORS),
                        black_box(&mut canvases[index]),
                    )
                    .expect("render group should render");
                    spatial_engines[index]
                        .sample_into(black_box(&canvases[index]), &mut sampled[index]);
                }
                black_box(&sampled);
            });
        });
    }

    group.finish();
}

fn bench_sensor_control_bindings(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_effect_bindings");

    for binding_count in [0_usize, 1, 5] {
        let mut engine = EffectEngine::new().with_canvas_size(CANVAS_WIDTH, CANVAS_HEIGHT);
        engine
            .activate(
                Box::new(BindingBenchRenderer::new()),
                binding_metadata(binding_count),
            )
            .expect("binding benchmark effect should activate");
        let mut canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
        let mut iteration = 0_usize;

        group.bench_function(
            BenchmarkId::new("tick_with_sensor_bindings", binding_count),
            |b| {
                b.iter(|| {
                    let sensors = &BINDING_SNAPSHOTS[iteration % BINDING_SNAPSHOTS.len()];
                    iteration = iteration.wrapping_add(1);
                    engine
                        .tick_with_inputs_and_sensors_into(
                            FRAME_DT_SECONDS,
                            &SILENCE,
                            &DEFAULT_INTERACTION,
                            None,
                            black_box(sensors),
                            black_box(&mut canvas),
                        )
                        .expect("binding benchmark tick should succeed");
                });
            },
        );
    }

    group.finish();
}

fn bench_audio_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_audio");

    let fft_1024_samples = sine_wave(440.0, SAMPLE_RATE_HZ, 1_024);
    let mut fft_1024 = FftPipeline::new(1_024, SAMPLE_RATE_HZ);
    group.throughput(Throughput::Elements(1_024));
    group.bench_function(BenchmarkId::new("fft_process", "1024"), |b| {
        b.iter(|| {
            let result = fft_1024
                .process(black_box(&fft_1024_samples))
                .expect("1024-point FFT should succeed");
            black_box(result.spectral_flux);
            black_box(result.spectrum[0]);
        });
    });

    let fft_4096_samples = sine_wave(880.0, SAMPLE_RATE_HZ, 4_096);
    let mut fft_4096 = FftPipeline::new(4_096, SAMPLE_RATE_HZ);
    group.throughput(Throughput::Elements(4_096));
    group.bench_function(BenchmarkId::new("fft_process", "4096"), |b| {
        b.iter(|| {
            let result = fft_4096
                .process(black_box(&fft_4096_samples))
                .expect("4096-point FFT should succeed");
            black_box(result.spectral_flux);
            black_box(result.spectrum[0]);
        });
    });

    let mut beat_detector = BeatDetector::default();
    for warmup_frame in 0..64_u64 {
        let frame = synthetic_beat_frame(warmup_frame);
        let _ = beat_detector.update(&frame);
    }
    let mut beat_frame_number = 64_u64;
    group.bench_function("beat_detection", |b| {
        b.iter(|| {
            let frame = synthetic_beat_frame(beat_frame_number);
            beat_frame_number += 1;
            let state = beat_detector.update(black_box(&frame));
            black_box(state.beat_pulse);
            black_box(state.bpm);
        });
    });

    let config = manual_audio_config();
    let signal_samples = sine_wave(440.0, SAMPLE_RATE_HZ, 2_048);
    let silence_samples = vec![0.0_f32; 2_048];
    let mut signal_input = AudioInput::new(&config);
    signal_input.start().expect("audio input should start");
    signal_input
        .set_capture_active(true)
        .expect("audio capture should enable");
    let mut silence_input = AudioInput::new(&config);
    silence_input.start().expect("audio input should start");
    silence_input
        .set_capture_active(true)
        .expect("audio capture should enable");

    group.throughput(Throughput::Elements(2_048));
    group.bench_function("audio_input_sample_signal", |b| {
        b.iter(|| {
            signal_input.push_samples(black_box(&signal_samples));
            let data = signal_input
                .sample_with_delta_secs(FRAME_DT_SECONDS)
                .expect("audio sample should succeed");
            black_box(data);
        });
    });

    group.bench_function("audio_input_sample_silence", |b| {
        b.iter(|| {
            silence_input.push_samples(black_box(&silence_samples));
            let data = silence_input
                .sample_with_delta_secs(FRAME_DT_SECONDS)
                .expect("audio sample should succeed");
            black_box(data);
        });
    });

    group.finish();
}

fn bench_canvas_handoff(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_canvas_handoff");
    let canvas_bytes = u64::from(CANVAS_WIDTH)
        .saturating_mul(u64::from(CANVAS_HEIGHT))
        .saturating_mul(4);
    group.throughput(Throughput::Bytes(canvas_bytes));

    group.bench_function("into_rgba_bytes_unique", |b| {
        b.iter(|| {
            let canvas = patterned_canvas(CANVAS_WIDTH, CANVAS_HEIGHT);
            let rgba = black_box(canvas).into_rgba_bytes_with_copy_info();
            black_box(rgba);
        });
    });

    group.bench_function("into_rgba_bytes_shared", |b| {
        b.iter(|| {
            let canvas = patterned_canvas(CANVAS_WIDTH, CANVAS_HEIGHT);
            let shared = canvas.clone();
            black_box(&shared);
            let rgba = black_box(canvas).into_rgba_bytes_with_copy_info();
            black_box(rgba);
        });
    });

    group.bench_function("canvas_frame_from_owned_unique", |b| {
        b.iter(|| {
            let canvas = patterned_canvas(CANVAS_WIDTH, CANVAS_HEIGHT);
            let frame = CanvasFrame::from_owned_canvas_with_copy_info(black_box(canvas), 1, 16);
            black_box(frame);
        });
    });

    group.bench_function("canvas_frame_from_owned_shared", |b| {
        b.iter(|| {
            let canvas = patterned_canvas(CANVAS_WIDTH, CANVAS_HEIGHT);
            let shared = canvas.clone();
            black_box(&shared);
            let frame = CanvasFrame::from_owned_canvas_with_copy_info(black_box(canvas), 1, 16);
            black_box(frame);
        });
    });

    group.finish();
}

fn bench_backend_routing(c: &mut Criterion) {
    let mut group = c.benchmark_group("core_backend_routing");
    group.throughput(Throughput::Elements(120));

    let runtime = Runtime::new().expect("benchmark runtime should initialize");

    let cached_device_id = DeviceId::new();
    let mut cached_manager = BackendManager::new();
    cached_manager.register_backend(Box::new(NullBenchBackend));
    cached_manager.map_device("bench:cached-strip", "bench", cached_device_id);
    let cached_layout = layout_with_zone(bench_routing_zone(
        "zone_0",
        "bench:cached-strip",
        120,
        None,
    ));
    let zone_colors = vec![ZoneColors {
        zone_id: "zone_0".to_owned(),
        colors: vec![[255, 0, 0]; 120],
    }];
    let _ = runtime.block_on(cached_manager.write_frame(&zone_colors, &cached_layout));

    group.bench_function("write_frame_cached_layout", |b| {
        b.iter(|| {
            let stats = runtime.block_on(
                cached_manager.write_frame(black_box(&zone_colors), black_box(&cached_layout)),
            );
            black_box(stats.devices_written);
            black_box(cached_manager.routing_plan_rebuild_count());
        });
    });

    let churn_device_id = DeviceId::new();
    let mut churn_manager = BackendManager::new();
    churn_manager.register_backend(Box::new(NullBenchBackend));
    churn_manager.map_device("bench:churn-strip", "bench", churn_device_id);
    let base_layout =
        layout_with_zone(bench_routing_zone("zone_0", "bench:churn-strip", 120, None));
    let remapped_layout = layout_with_zone(bench_routing_zone(
        "zone_0",
        "bench:churn-strip",
        120,
        Some((0_u32..120_u32).rev().collect()),
    ));
    let mut use_remapped_layout = false;

    group.bench_function("write_frame_layout_churn", |b| {
        b.iter(|| {
            let layout = if use_remapped_layout {
                &remapped_layout
            } else {
                &base_layout
            };
            use_remapped_layout = !use_remapped_layout;
            let stats = runtime
                .block_on(churn_manager.write_frame(black_box(&zone_colors), black_box(layout)));
            black_box(stats.devices_written);
            black_box(churn_manager.routing_plan_rebuild_count());
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = benchmark_config();
    targets = bench_builtin_renderers, bench_spatial_sampling, bench_render_groups, bench_sensor_control_bindings, bench_audio_pipeline, bench_canvas_handoff, bench_backend_routing
}
criterion_main!(benches);
