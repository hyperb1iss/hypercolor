use std::hint::black_box;
use std::sync::LazyLock;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

#[path = "../../hypercolor-core/tests/support/effect_engine.rs"]
mod effect_engine;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::mock::{MockDeviceBackend, MockDeviceConfig, MockEffectRenderer};
use hypercolor_core::device::{BackendManager, DeviceBackend};
use hypercolor_core::input::InteractionData;
use hypercolor_core::spatial::SpatialEngine;
#[cfg(feature = "wgpu")]
use hypercolor_daemon::render_thread::sparkleflinger::PreviewSurfaceRequest;
use hypercolor_daemon::render_thread::sparkleflinger::{
    CompositionLayer, CompositionPlan, SparkleFlinger,
};
use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::{
    Canvas, PublishedSurface, RenderSurfacePool, Rgba, SurfaceDescriptor,
};
#[cfg(feature = "wgpu")]
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::device::DeviceId;
use hypercolor_types::event::FrameData;
use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    StripDirection,
};
use tokio::runtime::Runtime;

use effect_engine::EffectEngine;

const CANVAS_WIDTH: u32 = 320;
const CANVAS_HEIGHT: u32 = 200;
const PREVIEW_WIDTH: u32 = 640;
const PREVIEW_HEIGHT: u32 = 480;
const FRAME_DT_SECONDS: f32 = 1.0 / 60.0;
const FRAME_INTERVAL_MS: u32 = 16;
const BENCH_DEVICE_COUNT: usize = 3;
const BENCH_LEDS_PER_DEVICE: u32 = 120;
const CANVAS_RGBA_BYTES: u64 = 320_u64 * 200_u64 * 4;

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
        brightness: None,
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
    split_surface_for(CANVAS_WIDTH, CANVAS_HEIGHT)
}

fn split_surface_for(width: u32, height: u32) -> PublishedSurface {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let color = if x < width / 2 {
                Rgba::new(255, 0, 0, 255)
            } else {
                Rgba::new(0, 0, 255, 255)
            };
            canvas.set_pixel(x, y, color);
        }
    }
    PublishedSurface::from_owned_canvas(canvas, 0, 0)
}

fn patterned_canvas() -> Canvas {
    patterned_canvas_for(CANVAS_WIDTH, CANVAS_HEIGHT)
}

fn patterned_canvas_for(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let red = u8::try_from((x * 255) / width.saturating_sub(1).max(1)).expect("red fits");
            let green =
                u8::try_from((y * 255) / height.saturating_sub(1).max(1)).expect("green fits");
            let blue = u8::try_from(
                ((x + y) * 255)
                    / (width
                        .saturating_sub(1)
                        .saturating_add(height.saturating_sub(1))
                        .max(1)),
            )
            .expect("blue fits");
            canvas.set_pixel(x, y, Rgba::new(red, green, blue, 255));
        }
    }
    canvas
}

fn inverse_patterned_canvas() -> Canvas {
    inverse_patterned_canvas_for(CANVAS_WIDTH, CANVAS_HEIGHT)
}

fn inverse_patterned_canvas_for(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let red = u8::try_from(
                ((width.saturating_sub(1).saturating_sub(x)) * 255)
                    / width.saturating_sub(1).max(1),
            )
            .expect("red fits");
            let green = u8::try_from(
                ((height.saturating_sub(1).saturating_sub(y)) * 255)
                    / height.saturating_sub(1).max(1),
            )
            .expect("green fits");
            let blue = u8::try_from(u64::from((x ^ y) & u32::from(u8::MAX))).expect("blue fits");
            canvas.set_pixel(x, y, Rgba::new(red, green, blue, 255));
        }
    }
    canvas
}

fn multi_blend_plan_for(width: u32, height: u32) -> CompositionPlan {
    CompositionPlan::with_layers(
        width,
        height,
        vec![
            CompositionLayer::replace_canvas(patterned_canvas_for(width, height)),
            CompositionLayer::alpha_canvas(inverse_patterned_canvas_for(width, height), 0.35),
            CompositionLayer::add_canvas(patterned_canvas_for(width, height), 0.20),
            CompositionLayer::screen_canvas(inverse_patterned_canvas_for(width, height), 0.45),
        ],
    )
}

fn preview_fresh_plans() -> Vec<CompositionPlan> {
    (0..4)
        .map(|variant| {
            let mut base = patterned_canvas_for(PREVIEW_WIDTH, PREVIEW_HEIGHT);
            let mut overlay = inverse_patterned_canvas_for(PREVIEW_WIDTH, PREVIEW_HEIGHT);
            base.set_pixel(variant, 0, Rgba::new((variant * 17) as u8, 32, 96, 255));
            overlay.set_pixel(variant, 0, Rgba::new(64, (variant * 29) as u8, 192, 255));
            CompositionPlan::with_layers(
                PREVIEW_WIDTH,
                PREVIEW_HEIGHT,
                vec![
                    CompositionLayer::replace_canvas(base),
                    CompositionLayer::alpha_canvas(overlay, 0.35),
                ],
            )
        })
        .collect()
}

#[cfg(feature = "wgpu")]
fn resolve_preview_surface_for_bench(
    sparkleflinger: &mut SparkleFlinger,
) -> Option<PublishedSurface> {
    for _ in 0..8 {
        if let Some(surface) = sparkleflinger
            .resolve_preview_surface()
            .expect("GPU scaled preview bench should finalize preview readback")
        {
            return Some(surface);
        }
        std::thread::yield_now();
    }

    None
}

fn bench_publish_handoff(c: &mut Criterion) {
    let mut group = c.benchmark_group("daemon_publish_handoff");
    group.throughput(Throughput::Bytes(CANVAS_RGBA_BYTES));

    let copy_bus = HypercolorBus::new();
    let mut copy_canvas = patterned_canvas();
    let mut copy_frame_number = 0_u32;
    group.bench_function("owned_canvas_shared_copy", |b| {
        b.iter(|| {
            let cached_alias = copy_canvas.clone();
            let publish_canvas = std::mem::replace(&mut copy_canvas, cached_alias);
            let timestamp_ms = copy_frame_number.saturating_mul(FRAME_INTERVAL_MS);
            let (frame, copied) = CanvasFrame::from_owned_canvas_with_copy_info(
                black_box(publish_canvas),
                copy_frame_number,
                timestamp_ms,
            );
            let _ = copy_bus.canvas_sender().send(frame);
            black_box(copied);
            copy_frame_number = copy_frame_number.saturating_add(1);
        });
    });

    let pooled_bus = HypercolorBus::new();
    let mut pooled_surface_pool =
        RenderSurfacePool::new(SurfaceDescriptor::rgba8888(CANVAS_WIDTH, CANVAS_HEIGHT));
    let mut pooled_frame_number = 0_u32;
    group.bench_function("slot_backed_surface", |b| {
        b.iter(|| {
            let mut lease = pooled_surface_pool
                .dequeue()
                .expect("surface pool should recycle under watch semantics");
            {
                let target = lease.canvas_mut();
                let bytes = target.as_rgba_bytes_mut();
                bytes[0] = u8::try_from(pooled_frame_number & u32::from(u8::MAX))
                    .expect("frame byte fits");
            }
            let timestamp_ms = pooled_frame_number.saturating_mul(FRAME_INTERVAL_MS);
            let surface = lease.submit(pooled_frame_number, timestamp_ms);
            let _ = pooled_bus
                .canvas_sender()
                .send(CanvasFrame::from_surface(black_box(surface)));
            pooled_frame_number = pooled_frame_number.saturating_add(1);
        });
    });

    group.finish();
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
    let mut active_surface_pool =
        RenderSurfacePool::new(SurfaceDescriptor::rgba8888(CANVAS_WIDTH, CANVAS_HEIGHT));
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
            let mut lease = active_surface_pool
                .dequeue()
                .expect("render surface pool should recycle under watch semantics");
            active_effect
                .tick_with_inputs_into(
                    FRAME_DT_SECONDS,
                    black_box(&*SILENCE),
                    black_box(&*DEFAULT_INTERACTION),
                    None,
                    lease.canvas_mut(),
                )
                .expect("active effect frame should render");

            let active_canvas = lease.canvas_mut().clone();
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
            let surface = lease.submit(active_frame_number, timestamp_ms);
            let _ = active_bus
                .canvas_sender()
                .send(CanvasFrame::from_surface(black_box(surface)));
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

#[expect(
    clippy::too_many_lines,
    reason = "Bench wires several end-to-end compositor scenarios into one comparison group"
)]
fn bench_sparkleflinger(c: &mut Criterion) {
    let mut group = c.benchmark_group("daemon_sparkleflinger");

    let mut sparkleflinger = SparkleFlinger::cpu();
    let bypass_surface = split_surface();
    group.throughput(Throughput::Bytes(CANVAS_RGBA_BYTES));
    group.bench_function("single_replace_bypass", |b| {
        b.iter(|| {
            let composed = sparkleflinger.compose(CompositionPlan::single(
                CANVAS_WIDTH,
                CANVAS_HEIGHT,
                CompositionLayer::replace_surface(black_box(bypass_surface.clone())),
            ));
            black_box(
                composed
                    .sampling_surface
                    .as_ref()
                    .map(hypercolor_types::canvas::PublishedSurface::rgba_len),
            );
        });
    });

    let base = patterned_canvas();
    let overlay = inverse_patterned_canvas();
    group.throughput(Throughput::Bytes(CANVAS_RGBA_BYTES));
    group.bench_function("alpha_two_layer_compose", |b| {
        b.iter(|| {
            let composed = sparkleflinger.compose(CompositionPlan::with_layers(
                CANVAS_WIDTH,
                CANVAS_HEIGHT,
                vec![
                    CompositionLayer::replace_canvas(black_box(base.clone())),
                    CompositionLayer::alpha_canvas(black_box(overlay.clone()), 0.35),
                ],
            ));
            black_box(
                composed
                    .sampling_canvas
                    .as_ref()
                    .expect("compose benchmark expects a materialized canvas")
                    .get_pixel(0, 0),
            );
        });
    });

    let preview_rgba_bytes = u64::from(PREVIEW_WIDTH) * u64::from(PREVIEW_HEIGHT) * 4;
    let preview_base = patterned_canvas_for(PREVIEW_WIDTH, PREVIEW_HEIGHT);
    let preview_overlay = inverse_patterned_canvas_for(PREVIEW_WIDTH, PREVIEW_HEIGHT);
    let preview_fresh_plans = preview_fresh_plans();
    let mut preview_sparkleflinger = SparkleFlinger::cpu();
    group.throughput(Throughput::Bytes(preview_rgba_bytes));
    group.bench_function("alpha_two_layer_compose_640x480", |b| {
        b.iter(|| {
            let composed = preview_sparkleflinger.compose(CompositionPlan::with_layers(
                PREVIEW_WIDTH,
                PREVIEW_HEIGHT,
                vec![
                    CompositionLayer::replace_canvas(black_box(preview_base.clone())),
                    CompositionLayer::alpha_canvas(black_box(preview_overlay.clone()), 0.35),
                ],
            ));
            black_box(
                composed
                    .sampling_canvas
                    .as_ref()
                    .expect("preview compose benchmark expects a materialized canvas")
                    .get_pixel(0, 0),
            );
        });
    });
    let mut fresh_plan_index = 0_usize;
    group.bench_function("alpha_two_layer_compose_640x480_fresh", |b| {
        b.iter(|| {
            let composed = preview_sparkleflinger.compose(black_box(
                preview_fresh_plans[fresh_plan_index]
                    .clone()
                    .with_cpu_replay_cacheable(false),
            ));
            fresh_plan_index = (fresh_plan_index + 1) % preview_fresh_plans.len();
            black_box(
                composed
                    .sampling_canvas
                    .as_ref()
                    .expect("fresh preview compose benchmark expects a materialized canvas")
                    .get_pixel(0, 0),
            );
        });
    });
    let mut fresh_canvas_only_plan_index = 0_usize;
    group.bench_function("alpha_two_layer_compose_640x480_fresh_canvas_only", |b| {
        b.iter(|| {
            let composed = preview_sparkleflinger.compose_for_outputs(
                black_box(
                    preview_fresh_plans[fresh_canvas_only_plan_index]
                        .clone()
                        .with_cpu_replay_cacheable(false),
                ),
                true,
                None,
            );
            fresh_canvas_only_plan_index =
                (fresh_canvas_only_plan_index + 1) % preview_fresh_plans.len();
            black_box(
                composed
                    .sampling_canvas
                    .as_ref()
                    .expect("fresh canvas-only preview compose benchmark expects a canvas")
                    .get_pixel(0, 0),
            );
        });
    });

    let multi_blend_plan = multi_blend_plan_for(PREVIEW_WIDTH, PREVIEW_HEIGHT);
    let mut multi_blend_sparkleflinger = SparkleFlinger::cpu();
    group.bench_function("multi_blend_alpha_add_screen_640x480", |b| {
        b.iter(|| {
            let composed = multi_blend_sparkleflinger.compose(black_box(multi_blend_plan.clone()));
            black_box(
                composed
                    .sampling_canvas
                    .as_ref()
                    .expect("multi-blend benchmark expects a materialized canvas")
                    .get_pixel(0, 0),
            );
        });
    });
    let mut multi_blend_canvas_only_sparkleflinger = SparkleFlinger::cpu();
    group.bench_function("multi_blend_alpha_add_screen_640x480_canvas_only", |b| {
        b.iter(|| {
            let composed = multi_blend_canvas_only_sparkleflinger.compose_for_outputs(
                black_box(multi_blend_plan.clone()),
                true,
                None,
            );
            black_box(
                composed
                    .sampling_canvas
                    .as_ref()
                    .expect("multi-blend canvas-only benchmark expects a materialized canvas")
                    .get_pixel(0, 0),
            );
        });
    });

    #[cfg(feature = "wgpu")]
    {
        let sampling_engine = SpatialEngine::new(layout_with_zones(vec![
            strip_zone("bench-zone-0", "bench-device-0", 120),
            strip_zone("bench-zone-1", "bench-device-1", 120),
            strip_zone("bench-zone-2", "bench-device-2", 120),
        ]));
        let sampling_plan = sampling_engine.sampling_plan();
        let bypass_surface_plan = CompositionPlan::single(
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
            CompositionLayer::replace_surface(split_surface_for(PREVIEW_WIDTH, PREVIEW_HEIGHT)),
        );
        let preview_plan = CompositionPlan::with_layers(
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
            vec![
                CompositionLayer::replace_canvas(preview_base.clone()),
                CompositionLayer::alpha_canvas(preview_overlay.clone(), 0.35),
            ],
        );
        let scaled_preview_request = PreviewSurfaceRequest {
            width: 320,
            height: 240,
        };
        let cpu_preview = SparkleFlinger::cpu().compose(preview_plan.clone());
        let mut cpu_sampled = Vec::new();
        group.bench_function("cpu_zone_sample_640x480", |b| {
            b.iter(|| {
                sampling_engine.sample_into(
                    cpu_preview
                        .sampling_canvas
                        .as_ref()
                        .expect("CPU preview benchmark expects a materialized canvas"),
                    &mut cpu_sampled,
                );
                black_box(cpu_sampled.first());
            });
        });

        let mut sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Gpu)
            .expect("GPU SparkleFlinger should initialize for the benchmark");
        group.throughput(Throughput::Bytes(CANVAS_RGBA_BYTES));
        group.bench_function("gpu_alpha_two_layer_compose", |b| {
            b.iter(|| {
                let composed = sparkleflinger.compose(CompositionPlan::with_layers(
                    CANVAS_WIDTH,
                    CANVAS_HEIGHT,
                    vec![
                        CompositionLayer::replace_canvas(black_box(base.clone())),
                        CompositionLayer::alpha_canvas(black_box(overlay.clone()), 0.35),
                    ],
                ));
                black_box(
                    composed
                        .sampling_canvas
                        .as_ref()
                        .expect("GPU compose benchmark expects a materialized canvas")
                        .get_pixel(0, 0),
                );
            });
        });
        group.bench_function("gpu_alpha_two_layer_compose_no_readback", |b| {
            b.iter(|| {
                let composed = sparkleflinger.compose_for_outputs(
                    CompositionPlan::with_layers(
                        CANVAS_WIDTH,
                        CANVAS_HEIGHT,
                        vec![
                            CompositionLayer::replace_canvas(black_box(base.clone())),
                            CompositionLayer::alpha_canvas(black_box(overlay.clone()), 0.35),
                        ],
                    ),
                    false,
                    None,
                );
                black_box(composed.bypassed);
            });
        });

        let mut preview_sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Gpu)
            .expect("GPU SparkleFlinger should initialize for the preview benchmark");
        group.throughput(Throughput::Bytes(preview_rgba_bytes));
        group.bench_function("gpu_alpha_two_layer_compose_640x480", |b| {
            b.iter(|| {
                let composed = preview_sparkleflinger.compose(CompositionPlan::with_layers(
                    PREVIEW_WIDTH,
                    PREVIEW_HEIGHT,
                    vec![
                        CompositionLayer::replace_canvas(black_box(preview_base.clone())),
                        CompositionLayer::alpha_canvas(black_box(preview_overlay.clone()), 0.35),
                    ],
                ));
                black_box(
                    composed
                        .sampling_canvas
                        .as_ref()
                        .expect("GPU preview compose benchmark expects a materialized canvas")
                        .get_pixel(0, 0),
                );
            });
        });
        group.bench_function("gpu_alpha_two_layer_compose_640x480_no_readback", |b| {
            b.iter(|| {
                let composed = preview_sparkleflinger.compose_for_outputs(
                    CompositionPlan::with_layers(
                        PREVIEW_WIDTH,
                        PREVIEW_HEIGHT,
                        vec![
                            CompositionLayer::replace_canvas(black_box(preview_base.clone())),
                            CompositionLayer::alpha_canvas(
                                black_box(preview_overlay.clone()),
                                0.35,
                            ),
                        ],
                    ),
                    false,
                    None,
                );
                black_box(composed.bypassed);
            });
        });
        let mut gpu_multi_blend_sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Gpu)
            .expect("GPU SparkleFlinger should initialize for multi-blend benchmark");
        group.bench_function("gpu_multi_blend_alpha_add_screen_640x480", |b| {
            b.iter(|| {
                let composed =
                    gpu_multi_blend_sparkleflinger.compose(black_box(multi_blend_plan.clone()));
                black_box(
                    composed
                        .sampling_canvas
                        .as_ref()
                        .expect("GPU multi-blend benchmark expects a materialized canvas")
                        .get_pixel(0, 0),
                );
            });
        });
        group.bench_function(
            "gpu_multi_blend_alpha_add_screen_640x480_no_readback",
            |b| {
                b.iter(|| {
                    let composed = gpu_multi_blend_sparkleflinger.compose_for_outputs(
                        black_box(multi_blend_plan.clone()),
                        false,
                        None,
                    );
                    black_box(composed.bypassed);
                });
            },
        );
        let mut scaled_preview_plan_index = 0_usize;
        group.throughput(Throughput::Bytes(
            u64::from(scaled_preview_request.width) * u64::from(scaled_preview_request.height) * 4,
        ));
        group.bench_function(
            "gpu_alpha_two_layer_compose_640x480_scaled_preview_320x240",
            |b| {
                b.iter(|| {
                    let composed = preview_sparkleflinger.compose_for_outputs(
                        black_box(
                            preview_fresh_plans[scaled_preview_plan_index]
                                .clone()
                                .with_cpu_replay_cacheable(false),
                        ),
                        false,
                        Some(scaled_preview_request),
                    );
                    scaled_preview_plan_index =
                        (scaled_preview_plan_index + 1) % preview_fresh_plans.len();
                    black_box(composed.bypassed);
                    let preview_surface =
                        resolve_preview_surface_for_bench(&mut preview_sparkleflinger);
                    black_box(
                        preview_surface
                            .as_ref()
                            .map(|surface| surface.rgba_bytes()[0]),
                    );
                });
            },
        );
        let mut gpu_bypass_sparkleflinger = SparkleFlinger::new(RenderAccelerationMode::Gpu)
            .expect("GPU SparkleFlinger should initialize for bypass sampling");
        group.bench_function(
            "gpu_single_replace_surface_compose_640x480_no_readback",
            |b| {
                b.iter(|| {
                    let composed = gpu_bypass_sparkleflinger.compose_for_outputs(
                        bypass_surface_plan.clone(),
                        false,
                        None,
                    );
                    black_box(composed.bypassed);
                });
            },
        );
        let mut gpu_sample_plan_index = 0_usize;
        group.bench_function("gpu_zone_sample_640x480", |b| {
            b.iter(|| {
                let composed = preview_sparkleflinger.compose_for_outputs(
                    black_box(
                        preview_fresh_plans[gpu_sample_plan_index]
                            .clone()
                            .with_cpu_replay_cacheable(false),
                    ),
                    false,
                    None,
                );
                gpu_sample_plan_index = (gpu_sample_plan_index + 1) % preview_fresh_plans.len();
                black_box(composed.bypassed);
                let sampled = preview_sparkleflinger
                    .sample_zone_plan(sampling_plan.as_ref())
                    .expect("GPU zone sampling should not fail")
                    .expect("bilinear sampling plan should stay GPU-supported");
                black_box(sampled.first());
            });
        });

        let mut cpu_end_to_end = SparkleFlinger::cpu();
        let mut cpu_end_to_end_sampled = Vec::new();
        group.bench_function("cpu_compose_and_zone_sample_640x480", |b| {
            b.iter(|| {
                let composed = cpu_end_to_end.compose(preview_plan.clone());
                sampling_engine.sample_into(
                    composed
                        .sampling_canvas
                        .as_ref()
                        .expect("CPU end-to-end benchmark expects a materialized canvas"),
                    &mut cpu_end_to_end_sampled,
                );
                black_box(cpu_end_to_end_sampled.first());
            });
        });
        let mut cpu_fresh_end_to_end = SparkleFlinger::cpu();
        let mut cpu_fresh_end_to_end_sampled = Vec::new();
        let mut cpu_fresh_plan_index = 0_usize;
        group.bench_function("cpu_compose_and_zone_sample_640x480_fresh", |b| {
            b.iter(|| {
                let composed = cpu_fresh_end_to_end.compose(black_box(
                    preview_fresh_plans[cpu_fresh_plan_index]
                        .clone()
                        .with_cpu_replay_cacheable(false),
                ));
                cpu_fresh_plan_index = (cpu_fresh_plan_index + 1) % preview_fresh_plans.len();
                sampling_engine.sample_into(
                    composed
                        .sampling_canvas
                        .as_ref()
                        .expect("fresh CPU end-to-end benchmark expects a materialized canvas"),
                    &mut cpu_fresh_end_to_end_sampled,
                );
                black_box(cpu_fresh_end_to_end_sampled.first());
            });
        });
        let mut cpu_fresh_canvas_only = SparkleFlinger::cpu();
        let mut cpu_fresh_canvas_only_sampled = Vec::new();
        let mut cpu_fresh_canvas_only_plan_index = 0_usize;
        group.bench_function(
            "cpu_compose_and_zone_sample_640x480_fresh_canvas_only",
            |b| {
                b.iter(|| {
                    let composed = cpu_fresh_canvas_only.compose_for_outputs(
                        black_box(
                            preview_fresh_plans[cpu_fresh_canvas_only_plan_index]
                                .clone()
                                .with_cpu_replay_cacheable(false),
                        ),
                        true,
                        None,
                    );
                    cpu_fresh_canvas_only_plan_index =
                        (cpu_fresh_canvas_only_plan_index + 1) % preview_fresh_plans.len();
                    sampling_engine.sample_into(
                        composed.sampling_canvas.as_ref().expect(
                            "fresh canvas-only CPU benchmark expects a materialized canvas",
                        ),
                        &mut cpu_fresh_canvas_only_sampled,
                    );
                    black_box(cpu_fresh_canvas_only_sampled.first());
                });
            },
        );
        let mut cpu_bypass_end_to_end = SparkleFlinger::cpu();
        let mut cpu_bypass_sampled = Vec::new();
        group.bench_function("cpu_single_replace_surface_and_zone_sample_640x480", |b| {
            b.iter(|| {
                let composed = cpu_bypass_end_to_end.compose(bypass_surface_plan.clone());
                sampling_engine.sample_into(
                    composed
                        .sampling_canvas
                        .as_ref()
                        .expect("CPU bypass benchmark expects a materialized canvas"),
                    &mut cpu_bypass_sampled,
                );
                black_box(cpu_bypass_sampled.first());
            });
        });

        let mut gpu_end_to_end = SparkleFlinger::new(RenderAccelerationMode::Gpu)
            .expect("GPU SparkleFlinger should initialize for end-to-end sampling");
        let mut gpu_end_to_end_sampled = Vec::new();
        let mut gpu_end_to_end_plan_index = 0_usize;
        group.bench_function("gpu_compose_and_zone_sample_640x480", |b| {
            b.iter(|| {
                let composed = gpu_end_to_end.compose_for_outputs(
                    black_box(
                        preview_fresh_plans[gpu_end_to_end_plan_index]
                            .clone()
                            .with_cpu_replay_cacheable(false),
                    ),
                    false,
                    None,
                );
                gpu_end_to_end_plan_index =
                    (gpu_end_to_end_plan_index + 1) % preview_fresh_plans.len();
                black_box(composed.bypassed);
                assert!(
                    gpu_end_to_end
                        .sample_zone_plan_into(sampling_plan.as_ref(), &mut gpu_end_to_end_sampled,)
                        .expect("GPU end-to-end zone sampling should not fail")
                );
                black_box(gpu_end_to_end_sampled.first());
            });
        });
        let mut gpu_bypass_end_to_end = SparkleFlinger::new(RenderAccelerationMode::Gpu)
            .expect("GPU SparkleFlinger should initialize for bypass end-to-end sampling");
        let mut gpu_bypass_end_to_end_sampled = Vec::new();
        group.bench_function("gpu_single_replace_surface_and_zone_sample_640x480", |b| {
            b.iter(|| {
                let composed = gpu_bypass_end_to_end.compose_for_outputs(
                    bypass_surface_plan.clone(),
                    false,
                    None,
                );
                black_box(composed.bypassed);
                assert!(
                    gpu_bypass_end_to_end
                        .sample_zone_plan_into(
                            sampling_plan.as_ref(),
                            &mut gpu_bypass_end_to_end_sampled,
                        )
                        .expect("GPU bypass zone sampling should not fail")
                );
                black_box(gpu_bypass_end_to_end_sampled.first());
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = benchmark_config();
    targets = bench_render_pipeline, bench_publish_handoff, bench_sparkleflinger
}
criterion_main!(benches);
