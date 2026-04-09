use std::hint::black_box;
use std::sync::LazyLock;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig, MockEffectRenderer};
use hypercolor_core::device::{BackendManager, DeviceBackend};
use hypercolor_core::effect::EffectEngine;
use hypercolor_core::input::InteractionData;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{Canvas, PublishedSurface, Rgba};
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::FrameData;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::runtime::Runtime;

const CANVAS_WIDTH: u32 = 320;
const CANVAS_HEIGHT: u32 = 200;
const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const FRAME_INTERVAL_MS: u32 = 16;
const BENCH_DEVICE_COUNT: usize = 3;
const BENCH_LEDS_PER_DEVICE: u32 = 120;

static SILENCE: LazyLock<AudioData> = LazyLock::new(AudioData::silence);
static DEFAULT_INTERACTION: LazyLock<InteractionData> = LazyLock::new(InteractionData::default);

fn benchmark_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
        .sample_size(40)
}

fn strip_zone(id: &str, device_id: &str, led_count: u32) -> DeviceZone {
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
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        display_order: 0,
        attachment: None,
    }
}

fn layout_with_zones(zones: Vec<DeviceZone>) -> SpatialLayout {
    SpatialLayout {
        id: "daemon-bench-layout".to_owned(),
        name: "Daemon Bench Layout".to_owned(),
        description: None,
        canvas_width: CANVAS_WIDTH,
        canvas_height: CANVAS_HEIGHT,
        zones,
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn build_backend_and_spatial(runtime: &Runtime) -> (BackendManager, SpatialEngine, u64) {
    let mut backend = MockDeviceBackend::new();
    let mut mappings = Vec::with_capacity(BENCH_DEVICE_COUNT);
    let mut zones = Vec::with_capacity(BENCH_DEVICE_COUNT);

    for index in 0..BENCH_DEVICE_COUNT {
        let device_id = DeviceId::new();
        let layout_device_id = format!("mock:strip-{index}");
        let config = MockDeviceConfig {
            name: format!("Bench Strip {index}"),
            led_count: BENCH_LEDS_PER_DEVICE,
            topology: LedTopology::Strip {
                count: BENCH_LEDS_PER_DEVICE,
                direction: StripDirection::LeftToRight,
            },
            id: Some(device_id),
        };
        backend = backend.with_device(&config);
        mappings.push((layout_device_id.clone(), device_id));
        zones.push(strip_zone(
            &format!("zone_{index}"),
            &layout_device_id,
            BENCH_LEDS_PER_DEVICE,
        ));
    }

    for (_, device_id) in &mappings {
        runtime
            .block_on(backend.connect(device_id))
            .expect("benchmark backend device should connect");
    }

    let mut manager = BackendManager::new();
    manager.register_backend(Box::new(backend));
    for (layout_device_id, device_id) in mappings {
        manager.map_device(&layout_device_id, "mock", device_id);
    }

    let total_leds = u64::try_from(BENCH_DEVICE_COUNT)
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::from(BENCH_LEDS_PER_DEVICE));
    (
        manager,
        SpatialEngine::new(layout_with_zones(zones)),
        total_leds,
    )
}

fn split_surface() -> PublishedSurface {
    let mut canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    for y in 0..CANVAS_HEIGHT {
        for x in 0..CANVAS_WIDTH {
            let color = if x < CANVAS_WIDTH / 2 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 0, 255, 255)
            };
            canvas.set_pixel(x, y, color);
        }
    }
    PublishedSurface::from_owned_canvas(canvas, 0, 0)
}

fn bench_render_pipeline(c: &mut Criterion) {
    let runtime = Runtime::new().expect("benchmark runtime should initialize");

    let (mut active_manager, active_spatial, total_leds) = build_backend_and_spatial(&runtime);
    let active_layout = active_spatial.layout();
    let active_bus = HypercolorBus::new();
    let mut active_effect = EffectEngine::new().with_canvas_size(CANVAS_WIDTH, CANVAS_HEIGHT);
    active_effect
        .activate(
            Box::new(MockEffectRenderer::rainbow()),
            MockEffectRenderer::sample_metadata("daemon-bench-rainbow"),
        )
        .expect("benchmark effect should activate");
    let mut active_canvas = Canvas::new(CANVAS_WIDTH, CANVAS_HEIGHT);
    let mut active_recycled_frame = FrameData::empty();
    let mut active_frame_number = 0_u32;

    let (mut screen_manager, screen_spatial, _) = build_backend_and_spatial(&runtime);
    let screen_layout = screen_spatial.layout();
    let screen_bus = HypercolorBus::new();
    let source_surface = split_surface();
    let mut screen_recycled_frame = FrameData::empty();
    let mut screen_frame_number = 0_u32;

    let mut group = c.benchmark_group("daemon_render_pipeline");
    group.throughput(Throughput::Elements(total_leds));

    group.bench_function("active_effect_shared_publish", |b| {
        b.iter(|| {
            active_effect
                .tick_with_inputs_into(
                    FRAME_DT_SECONDS,
                    black_box(&*SILENCE),
                    black_box(&*DEFAULT_INTERACTION),
                    None,
                    &mut active_canvas,
                )
                .expect("active effect frame should render");

            let cached_alias = active_canvas.clone();
            active_spatial.sample_into(black_box(&active_canvas), &mut active_recycled_frame.zones);
            let _ = runtime.block_on(active_manager.write_frame_with_brightness(
                black_box(&active_recycled_frame.zones),
                black_box(active_layout.as_ref()),
                1.0,
                None,
            ));

            let timestamp_ms = active_frame_number.saturating_mul(FRAME_INTERVAL_MS);
            let frame = FrameData::new(
                std::mem::take(&mut active_recycled_frame.zones),
                active_frame_number,
                timestamp_ms,
            );
            active_recycled_frame = active_bus.frame_sender().send_replace(frame);
            let publish_canvas = std::mem::replace(&mut active_canvas, cached_alias);
            let (canvas_frame, copied) = CanvasFrame::from_owned_canvas_with_copy_info(
                black_box(publish_canvas),
                active_frame_number,
                timestamp_ms,
            );
            let _ = active_bus.canvas_sender().send(canvas_frame);
            black_box(copied);
            active_frame_number = active_frame_number.saturating_add(1);
        });
    });

    group.bench_function("screen_passthrough_shared_surface", |b| {
        b.iter(|| {
            let canvas = Canvas::from_published_surface(black_box(&source_surface));
            screen_spatial.sample_into(black_box(&canvas), &mut screen_recycled_frame.zones);
            let _ = runtime.block_on(screen_manager.write_frame_with_brightness(
                black_box(&screen_recycled_frame.zones),
                black_box(screen_layout.as_ref()),
                1.0,
                None,
            ));

            let timestamp_ms = screen_frame_number.saturating_mul(FRAME_INTERVAL_MS);
            let frame = FrameData::new(
                std::mem::take(&mut screen_recycled_frame.zones),
                screen_frame_number,
                timestamp_ms,
            );
            screen_recycled_frame = screen_bus.frame_sender().send_replace(frame);

            let surface = source_surface.with_frame_metadata(screen_frame_number, timestamp_ms);
            let _ = screen_bus
                .canvas_sender()
                .send(CanvasFrame::from_surface(surface.clone()));
            let _ = screen_bus
                .screen_canvas_sender()
                .send(CanvasFrame::from_surface(surface));

            screen_frame_number = screen_frame_number.saturating_add(1);
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = benchmark_config();
    targets = bench_render_pipeline
}
criterion_main!(benches);
