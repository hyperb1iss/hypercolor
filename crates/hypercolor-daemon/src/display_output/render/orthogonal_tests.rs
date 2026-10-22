use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::hint::black_box;
use std::time::{Duration, Instant};

use hypercolor_color::Rgb;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::device::DisplayFrameFormat;

use super::super::DisplayGeometry;
use super::super::encode::{DisplayEncodeState, display_brightness_factor, encode_canvas_frame};
use super::*;

fn patterned_frame(width: u32, height: u32, seed: usize) -> CanvasFrame {
    let mut canvas = Canvas::new(width, height);
    for (index, pixel) in canvas
        .as_rgba_bytes_mut()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
    {
        for (channel, byte) in pixel[..3].iter_mut().enumerate() {
            *byte = u8::try_from((index * (37 + channel * 11) + seed * 17) % 256)
                .expect("pattern channels fit a byte");
        }
        pixel[3] = u8::MAX;
    }
    CanvasFrame::from_owned_canvas(canvas, 1, 16)
}

fn viewport(rotation: f32, edge_behavior: EdgeBehavior) -> DisplayViewport {
    DisplayViewport {
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.73, 0.41),
        rotation,
        scale: 1.0,
        edge_behavior,
    }
}

fn scalar_pixels(
    source: &CanvasFrame,
    viewport: &DisplayViewport,
    width: u32,
    height: u32,
    brightness: Option<&[u8; 256]>,
) -> Vec<u8> {
    let mut pixels = vec![0; rgb_buffer_len(width, height).expect("test frame size fits")];
    render_display_view_scalar(source, viewport, width, height, &mut pixels, brightness);
    pixels
}

#[test]
fn orthogonal_plans_match_scalar_sampling_byte_for_byte() {
    let brightness =
        std::array::from_fn(|index| u8::try_from(index / 3).expect("dimmed channel fits a byte"));
    for rotation in [FRAC_PI_2, PI, 3.0 * FRAC_PI_2, -FRAC_PI_2, -PI, TAU] {
        for edge in [
            EdgeBehavior::Clamp,
            EdgeBehavior::Wrap,
            EdgeBehavior::Mirror,
        ] {
            for (source_width, source_height, width, height) in
                [(13, 9, 37, 23), (17, 31, 9, 28), (1, 1, 3, 5)]
            {
                for (position, size, scale) in [
                    ((0.5, 0.5), (1.0, 1.0), 1.0),
                    ((0.23, 0.76), (0.37, 0.19), 0.8),
                    ((-0.1, 1.2), (1.3, 0.9), 1.7),
                ] {
                    let viewport = DisplayViewport {
                        position: NormalizedPosition::new(position.0, position.1),
                        size: NormalizedPosition::new(size.0, size.1),
                        scale,
                        ..viewport(rotation, edge)
                    };
                    let mut pixels = Vec::new();
                    let mut plan = None;
                    for seed in [0, 19] {
                        let source = patterned_frame(source_width, source_height, seed);
                        for lut in [None, Some(&brightness)] {
                            let expected = scalar_pixels(&source, &viewport, width, height, lut);
                            render_display_view(
                                &source,
                                &viewport,
                                width,
                                height,
                                &mut pixels,
                                &mut plan,
                                lut,
                            );
                            assert_eq!(pixels, expected, "rotation={rotation}, edge={edge:?}");
                            assert!(
                                plan.is_some(),
                                "orthogonal viewport should cache its samples"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn rotated_plan_tracks_every_viewport_and_dimension_change() {
    let source = patterned_frame(23, 17, 0);
    let mut viewport = viewport(PI, EdgeBehavior::Clamp);
    let mut pixels = Vec::new();
    let mut plan = None;
    render_display_view(&source, &viewport, 31, 19, &mut pixels, &mut plan, None);
    let original_samples = plan.as_ref().expect("cached plan").samples.as_ptr();
    render_display_view(&source, &viewport, 31, 19, &mut pixels, &mut plan, None);
    assert_eq!(
        plan.as_ref().expect("reused plan").samples.as_ptr(),
        original_samples
    );

    for change in 0..9 {
        let old_key = plan.as_ref().expect("previous plan").key.clone();
        match change {
            0 => viewport.rotation = FRAC_PI_2,
            1 => viewport.position.x = 0.1,
            2 => viewport.position.y = 0.9,
            3 => viewport.size.x = 0.21,
            4 => viewport.size.y = 0.95,
            5 => viewport.scale = 1.8,
            6 => viewport.edge_behavior = EdgeBehavior::Mirror,
            7 | 8 => {}
            _ => unreachable!("fixed change cases"),
        }
        let resized = patterned_frame(41, 11, 3);
        let frame = if change == 8 { &resized } else { &source };
        let width = if change >= 7 { 29 } else { 31 };
        let height = if change >= 7 { 7 } else { 19 };
        render_display_view(
            frame,
            &viewport,
            width,
            height,
            &mut pixels,
            &mut plan,
            None,
        );
        assert_ne!(plan.as_ref().expect("updated plan").key, old_key);
        assert_eq!(pixels, scalar_pixels(frame, &viewport, width, height, None));
    }
}

#[test]
fn nonorthogonal_and_fading_viewports_keep_scalar_semantics() {
    let source = patterned_frame(23, 17, 0);
    for viewport in [
        viewport(0.17, EdgeBehavior::Clamp),
        viewport(PI + 0.01, EdgeBehavior::Wrap),
        DisplayViewport {
            position: NormalizedPosition::new(-0.2, 0.9),
            ..viewport(PI, EdgeBehavior::FadeToBlack { falloff: 3.0 })
        },
    ] {
        let mut pixels = Vec::new();
        let mut plan = None;
        render_display_view(&source, &viewport, 31, 19, &mut pixels, &mut plan, None);
        assert!(plan.is_none());
        assert_eq!(pixels, scalar_pixels(&source, &viewport, 31, 19, None));
    }
}

#[test]
fn orthogonal_encoding_preserves_brightness_and_circular_mask() {
    let frame = patterned_frame(43, 29, 5);
    let mut state = DisplayEncodeState::new().expect("display encoder initializes");
    for rotation in [FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
        let viewport = viewport(rotation, EdgeBehavior::Clamp);
        for brightness in [0.0, 0.37, 1.0] {
            for circular in [false, true] {
                let geometry = DisplayGeometry {
                    width: 19,
                    height: 31,
                    circular,
                };
                let scale = f32::from(display_brightness_factor(brightness)) / 255.0;
                let lut = std::array::from_fn(|channel| {
                    let channel = u8::try_from(channel).expect("channel fits a byte");
                    Rgb::new(channel, channel, channel).scale(scale).r
                });
                let mut expected = scalar_pixels(&frame, &viewport, 19, 31, Some(&lut));
                if circular {
                    apply_circular_mask(&mut expected, 19, 31);
                }
                let encoded = encode_canvas_frame(
                    &frame,
                    &viewport,
                    &geometry,
                    brightness,
                    DisplayFrameFormat::Rgb,
                    false,
                    &mut state,
                )
                .expect("rotated display encodes");
                assert_eq!(encoded.data, expected);
            }
        }
    }
}

#[test]
fn empty_orthogonal_display_does_not_prepare_a_plan() {
    for (frame, width, height) in [
        (CanvasFrame::empty(), 7, 5),
        (patterned_frame(7, 5, 0), 0, 0),
    ] {
        let mut pixels = Vec::new();
        let mut plan = None;
        render_display_view(
            &frame,
            &viewport(PI, EdgeBehavior::Clamp),
            width,
            height,
            &mut pixels,
            &mut plan,
            None,
        );
        assert!(pixels.iter().all(|byte| *byte == 0));
        assert!(plan.is_none());
    }
}

/// Run with an optimized test profile and `--ignored --nocapture`.
/// Both measurements reuse their output buffers; the prepared path also
/// reuses a warmed viewport plan, as the display worker does across frames.
#[test]
#[ignore = "manual scalar-versus-prepared display throughput measurement"]
fn orthogonal_display_benchmark() {
    const FRAMES: u32 = 100;
    let source = patterned_frame(640, 630, 7);
    for rotation in [FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
        let viewport = DisplayViewport {
            position: NormalizedPosition::new(0.19874, 0.50106),
            size: NormalizedPosition::new(0.12552, 0.12739),
            ..viewport(rotation, EdgeBehavior::Clamp)
        };
        let mut pixels = vec![0; 400 * 400 * 3];
        let mut plan = None;
        render_display_view(&source, &viewport, 400, 400, &mut pixels, &mut plan, None);
        assert_eq!(pixels, scalar_pixels(&source, &viewport, 400, 400, None));
        let mut scalar = [Duration::ZERO; 5];
        let mut prepared = [Duration::ZERO; 5];
        for round in 0..5 {
            let start = Instant::now();
            for _ in 0..FRAMES {
                render_display_view_scalar(
                    black_box(&source),
                    black_box(&viewport),
                    400,
                    400,
                    black_box(&mut pixels),
                    None,
                );
                black_box(&pixels);
            }
            scalar[round] = start.elapsed() / FRAMES;
            let start = Instant::now();
            for _ in 0..FRAMES {
                render_display_view(
                    black_box(&source),
                    black_box(&viewport),
                    400,
                    400,
                    black_box(&mut pixels),
                    &mut plan,
                    None,
                );
                black_box(&pixels);
            }
            prepared[round] = start.elapsed() / FRAMES;
        }
        scalar.sort_unstable();
        prepared.sort_unstable();
        eprintln!(
            "rotation={rotation:.6} scalar={:?} prepared={:?} speedup={:.2}x",
            scalar[2],
            prepared[2],
            scalar[2].as_secs_f64() / prepared[2].as_secs_f64(),
        );
    }
}
