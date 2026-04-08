use std::hint::black_box;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hypercolor_core::effect::builtin::{ColorWaveRenderer, GradientRenderer, SolidColorRenderer};
use hypercolor_core::effect::{EffectRenderer, FrameInput};
use hypercolor_core::input::InteractionData;
use hypercolor_core::input::audio::beat::{BeatDetector, BeatFrame};
use hypercolor_core::input::audio::fft::FftPipeline;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
use hypercolor_types::spatial::{
    Corner, DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use uuid::Uuid;

const CANVAS_WIDTH: u32 = 320;
const CANVAS_HEIGHT: u32 = 200;
const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const SAMPLE_RATE_HZ: u32 = 48_000;

static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static DEFAULT_INTERACTION: LazyLock<InteractionData> = LazyLock::new(InteractionData::default);

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

fn frame_input(time_secs: f32, frame_number: u64, audio: &AudioData) -> FrameInput<'_> {
    FrameInput {
        time_secs,
        delta_secs: FRAME_DT_SECONDS,
        frame_number,
        audio,
        interaction: &DEFAULT_INTERACTION,
        screen: None,
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

    group.finish();
}

criterion_group! {
    name = benches;
    config = benchmark_config();
    targets = bench_builtin_renderers, bench_spatial_sampling, bench_audio_pipeline
}
criterion_main!(benches);
