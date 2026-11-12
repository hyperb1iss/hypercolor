//! A layer drawn through a placement lands where `LayerPlacement` says it
//! does, on both compositors. The placement is the one description of where
//! a framed layer's source goes: the CPU compositor walks it, the compose
//! shader takes the same steps from the same numbers, and input inverts it.

use std::f32::consts::PI;

use hypercolor_core::spatial::{LayerFrame, LayerPlacement, SourceRepeat};
use hypercolor_types::layer::LayerTransform;
use hypercolor_types::spatial::Corner;

use super::*;

/// A small deterministic generator, so a failing case fails every run.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1_u64 << 24) as f32)
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }

    fn pick<T: Copy>(&mut self, options: &[T]) -> T {
        let index = (self.unit() * options.len() as f32) as usize;
        options[index.min(options.len() - 1)]
    }
}

/// A canvas that tells every pixel where it is: red rises one level a
/// column, green one level a row, and blue marks the pixel as source.
fn position_canvas(width: u32, height: u32) -> Canvas {
    let mut canvas = Canvas::new(width, height);
    for y in 0..height {
        for x in 0..width {
            canvas.set_pixel(x, y, position_color(x, y, width, height));
        }
    }
    canvas
}

fn position_color(x: u32, y: u32, width: u32, height: u32) -> Rgba {
    Rgba::new(
        (x * 255 / (width - 1)) as u8,
        (y * 255 / (height - 1)) as u8,
        255,
        255,
    )
}

/// One nearest-sampled LED on every pixel of a canvas, so a sample of it is
/// the canvas, pixel for pixel.
fn every_pixel(width: u32, height: u32) -> SpatialEngine {
    let mut output = sampling_layout(SamplingMode::Nearest)
        .zones
        .into_iter()
        .next()
        .expect("the sampling fixture holds an output");
    output.position = NormalizedPosition::new(0.5, 0.5);
    output.size = NormalizedPosition::new(1.0, 1.0);
    output.rotation = 0.0;
    output.scale = 1.0;
    output.sampling_mode = Some(SamplingMode::Nearest);
    output.edge_behavior = Some(EdgeBehavior::Clamp);
    output.topology = LedTopology::Matrix {
        width,
        height,
        serpentine: false,
        start_corner: Corner::TopLeft,
    };
    SpatialEngine::new(SpatialLayout {
        id: "every-pixel".into(),
        name: "Every pixel".into(),
        description: None,
        canvas_width: width,
        canvas_height: height,
        zones: vec![output],
        default_sampling_mode: SamplingMode::Nearest,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    })
}

/// Compose `layer` over an opaque `base` color on both compositors and read
/// back every pixel of each: CPU first, GPU second.
fn compose_on_both(
    compositor: &mut GpuSparkleFlinger,
    zone: (u32, u32),
    base: Rgba,
    source: &Canvas,
    layer: impl Fn(ProducerFrame) -> CompositionLayer,
) -> (Vec<[u8; 3]>, Vec<[u8; 3]>) {
    let plan = |source: ProducerFrame| {
        let mut base_canvas = Canvas::new(zone.0, zone.1);
        base_canvas.fill(base);
        CompositionPlan::with_layers(
            zone.0,
            zone.1,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(base_canvas)),
                layer(source),
            ],
        )
    };
    let probe = every_pixel(zone.0, zone.1);

    let cpu_plan = plan(ProducerFrame::Canvas(source.clone()));
    let composed =
        CpuSparkleFlinger::new().compose(cpu_plan.clone(), true, full_preview_request(&cpu_plan));
    let cpu = probe.sample(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU compose should materialize a canvas"),
    );

    // The GPU compositor takes a source off the target's extent as a
    // texture, the way a framed effect's frame reaches it.
    let uploaded = compositor
        .upload_canvas_frame(source)
        .expect("the source should upload");
    compositor
        .compose(&plan(ProducerFrame::GpuTexture(uploaded)), false, None)
        .expect("GPU composition should succeed");
    let mut gpu = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(probe.sampling_plan().as_ref(), &mut gpu)
            .expect("GPU sampling should succeed")
    );
    (cpu[0].colors.clone(), gpu[0].colors.clone())
}

/// For random frames, canvas shapes, fits, and layer transforms: the CPU
/// compositor shows exactly the texel the placement names at every pixel,
/// and the GPU compositor shows it too, to the parity tolerance, at every
/// pixel that is not on the very edge of the placed source.
#[test]
fn random_placements_draw_where_the_placement_says_on_both_compositors() {
    // Every side is at least 256 texels, so neighbouring texels differ by
    // at most one level and a one-texel disagreement stays in tolerance.
    const SOURCES: [(u32, u32); 3] = [(256, 256), (512, 256), (256, 512)];
    const ZONES: [(u32, u32); 4] = [(96, 64), (64, 96), (80, 80), (128, 48)];
    const FITS: [FitMode; 5] = [
        FitMode::Stretch,
        FitMode::Contain,
        FitMode::Cover,
        FitMode::Tile,
        FitMode::Mirror,
    ];
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let mut random = Lcg(0x0095_2026);
    let (mut shown, mut empty) = (0_u64, 0_u64);
    for case in 0..60 {
        let zone = random.pick(&ZONES);
        let source = random.pick(&SOURCES);
        let frame = LayerFrame {
            center: NormalizedPosition::new(random.between(0.3, 0.7), random.between(0.3, 0.7)),
            extent: NormalizedPosition::new(random.between(0.25, 0.9), random.between(0.25, 0.9)),
            rotation: random.between(-PI, PI),
        };
        let transform = LayerTransform {
            anchor: NormalizedPosition::new(random.between(0.2, 0.8), random.between(0.2, 0.8)),
            scale: [random.between(0.5, 2.0), random.between(0.5, 2.0)],
            rotation: random.between(-PI, PI),
            fit: random.pick(&FITS),
        };
        let placement = LayerPlacement::new(&frame, transform, source, zone)
            .unwrap_or_else(|| panic!("case {case}: placement"));
        let source_canvas = position_canvas(source.0, source.1);
        let (cpu, gpu) = compose_on_both(
            &mut compositor,
            zone,
            Rgba::new(0, 0, 0, 255),
            &source_canvas,
            |frame| CompositionLayer::alpha(frame, 1.0).with_placement(placement),
        );

        let texel_at = |x: u32, y: u32| placement.source_texel([x as f32 + 0.5, y as f32 + 0.5]);
        for y in 0..zone.1 {
            for x in 0..zone.0 {
                let index = usize::try_from(y * zone.0 + x).expect("small canvas");
                let texel = texel_at(x, y);
                let expected = texel.map_or([0, 0, 0], |[texel_x, texel_y]| {
                    let color = position_color(texel_x, texel_y, source.0, source.1);
                    [color.r, color.g, color.b]
                });
                assert_eq!(
                    cpu[index], expected,
                    "case {case} {transform:?} pixel ({x}, {y}): the CPU compositor should \
                     show the placement's texel {texel:?}"
                );

                // Where the placed source ends, or a repeating one starts
                // over, the two may fall either side of the line.
                let neighbours = [
                    (x.saturating_sub(1), y),
                    ((x + 1).min(zone.0 - 1), y),
                    (x, y.saturating_sub(1)),
                    (x, (y + 1).min(zone.1 - 1)),
                ];
                let on_the_edge = neighbours
                    .iter()
                    .any(|&(near_x, near_y)| texel_at(near_x, near_y).is_some() != texel.is_some());
                let on_a_seam = placement.repeat != SourceRepeat::None
                    && texel.is_some_and(|[texel_x, texel_y]| {
                        texel_x == 0
                            || texel_y == 0
                            || texel_x == source.0 - 1
                            || texel_y == source.1 - 1
                    });
                if on_the_edge || on_a_seam {
                    continue;
                }
                if texel.is_some() {
                    shown += 1;
                } else {
                    empty += 1;
                }
                assert!(
                    (0..3).all(|channel| gpu[index][channel].abs_diff(expected[channel]) <= 1),
                    "case {case} {transform:?} pixel ({x}, {y}): the GPU compositor showed \
                     {:?} where the placement names texel {texel:?} ({expected:?})",
                    gpu[index]
                );
            }
        }
    }
    assert!(shown > 100_000, "only {shown} pixels showed the source");
    assert!(empty > 10_000, "only {empty} pixels were off the source");
}

/// A placed replace takes the place of what it draws on and of nothing
/// else. With a frame whose edge runs on that is the whole target; with a
/// clipped window it is the frame, and every pixel outside it is the
/// target's own, untouched, on both compositors.
#[test]
fn a_placed_replace_covers_what_it_draws_on_and_nothing_else() {
    let Some(mut compositor) = gpu_test_compositor() else {
        return;
    };
    let zone = (96, 64);
    let base = Rgba::new(31, 97, 203, 255);
    let frame = LayerFrame {
        center: NormalizedPosition::new(0.4, 0.55),
        extent: NormalizedPosition::new(0.5, 0.4),
        rotation: 0.5,
    };
    // A source half the frame's size: the rest of the frame is black under
    // a replace, and the target's own under any other mode.
    let half_size = LayerTransform {
        anchor: NormalizedPosition::new(0.5, 0.5),
        scale: [0.5, 0.5],
        rotation: 0.0,
        fit: FitMode::Stretch,
    };
    let framed = LayerPlacement::new(&frame, half_size, (256, 256), zone).expect("placement");
    let window = LayerPlacement::window(&frame, zone).expect("window");
    let pixel = |x: u32, y: u32| [x as f32 + 0.5, y as f32 + 0.5];
    let base_rgb = [base.r, base.g, base.b];

    // A framed replace: the source where it reaches, black everywhere else.
    let source = position_canvas(256, 256);
    let (cpu, gpu) = compose_on_both(&mut compositor, zone, base, &source, |frame| {
        CompositionLayer::replace(frame).with_placement(framed)
    });
    for y in 0..zone.1 {
        for x in 0..zone.0 {
            let index = usize::try_from(y * zone.0 + x).expect("small canvas");
            if framed.source_texel(pixel(x, y)).is_none() {
                assert_eq!(cpu[index], [0, 0, 0], "CPU pixel ({x}, {y})");
            }
            assert_ne!(cpu[index], base_rgb, "CPU pixel ({x}, {y}) kept the target");
            assert_ne!(gpu[index], base_rgb, "GPU pixel ({x}, {y}) kept the target");
        }
    }

    // The same placement under alpha leaves the target where the source
    // does not reach.
    let (cpu, gpu) = compose_on_both(&mut compositor, zone, base, &source, |frame| {
        CompositionLayer::alpha(frame, 1.0).with_placement(framed)
    });
    let mut kept = 0;
    for y in 2..zone.1 - 2 {
        for x in 2..zone.0 - 2 {
            let index = usize::try_from(y * zone.0 + x).expect("small canvas");
            let near_source = (-2_i32..=2).any(|dy| {
                (-2_i32..=2).any(|dx| {
                    framed
                        .source_texel(pixel(
                            x.saturating_add_signed(dx),
                            y.saturating_add_signed(dy),
                        ))
                        .is_some()
                })
            });
            if !near_source {
                kept += 1;
                assert_eq!(cpu[index], base_rgb, "CPU pixel ({x}, {y})");
                assert_eq!(gpu[index], base_rgb, "GPU pixel ({x}, {y})");
            }
        }
    }
    assert!(kept > 1_000, "only {kept} pixels were off the source");

    // A window: a zone-sized source seen through the frame alone.
    let mut picture = Canvas::new(zone.0, zone.1);
    picture.fill(Rgba::new(250, 10, 60, 255));
    let (cpu, gpu) = compose_on_both(&mut compositor, zone, base, &picture, |frame| {
        CompositionLayer::replace(frame).with_placement(window)
    });
    let (mut inside, mut outside) = (0, 0);
    for y in 1..zone.1 - 1 {
        for x in 1..zone.0 - 1 {
            let index = usize::try_from(y * zone.0 + x).expect("small canvas");
            let covers = |dx: i32, dy: i32| {
                window.covers(pixel(
                    x.saturating_add_signed(dx),
                    y.saturating_add_signed(dy),
                ))
            };
            let around = [covers(-1, 0), covers(1, 0), covers(0, -1), covers(0, 1)];
            if covers(0, 0) && around.iter().all(|covered| *covered) {
                inside += 1;
                assert_eq!(cpu[index], [250, 10, 60], "CPU pixel ({x}, {y})");
                assert_eq!(gpu[index], [250, 10, 60], "GPU pixel ({x}, {y})");
            } else if !covers(0, 0) && around.iter().all(|covered| !*covered) {
                outside += 1;
                assert_eq!(cpu[index], base_rgb, "CPU pixel ({x}, {y})");
                assert_eq!(gpu[index], base_rgb, "GPU pixel ({x}, {y})");
            }
        }
    }
    assert!(
        inside > 500 && outside > 2_000,
        "{inside} inside, {outside} outside"
    );
}
