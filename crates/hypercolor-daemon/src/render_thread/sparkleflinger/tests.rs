use hypercolor_core::blend_math::{
    RgbaBlendMode, blend_rgba_pixels_in_place, decode_srgb_channel, encode_srgb_channel,
    screen_blend,
};
use hypercolor_core::types::canvas::{BlendMode, Canvas, PublishedSurface, Rgba, RgbaF32};
use hypercolor_types::config::RenderAccelerationMode;
use hypercolor_types::scene::DisplayFaceBlendMode;
use hypercolor_types::spatial::NormalizedPosition;
use hypercolor_types::viewport::FitMode;

use super::{
    CompositionAdjust, CompositionLayer, CompositionMode, CompositionPlan, CompositionTransform,
    PreviewSurfaceRequest, SparkleFlinger,
};
#[cfg(feature = "wgpu")]
use super::{gpu_frame_without_cpu_fallback, new_preview_surface_pool};
#[cfg(feature = "wgpu")]
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::ProducerFrame;

fn solid_canvas(color: Rgba) -> Canvas {
    let mut canvas = Canvas::new(2, 2);
    canvas.fill(color);
    canvas
}

fn row_canvas(colors: &[Rgba]) -> Canvas {
    let mut rgba = Vec::with_capacity(colors.len() * 4);
    for color in colors {
        rgba.extend_from_slice(&[color.r, color.g, color.b, color.a]);
    }
    Canvas::from_vec(
        rgba,
        u32::try_from(colors.len()).expect("test row width should fit u32"),
        1,
    )
}

fn compose_transformed_source(source: Canvas, width: u32, height: u32, fit: FitMode) -> Canvas {
    let mut sparkleflinger = SparkleFlinger::cpu();
    sparkleflinger
        .compose(CompositionPlan::single(
            width,
            height,
            CompositionLayer::replace(ProducerFrame::Canvas(source)).with_transform(
                CompositionTransform {
                    anchor: NormalizedPosition::new(0.5, 0.5),
                    scale: [1.0, 1.0],
                    rotation: 0.0,
                    fit,
                },
            ),
        ))
        .sampling_canvas
        .expect("transformed layer should materialize a canvas")
}

fn expected_blend(dst: Rgba, src: Rgba, mode: BlendMode, opacity: f32) -> Rgba {
    let dst = dst.to_linear_f32();
    let src = src.to_linear_f32();
    let blended = mode.blend(
        [dst.r, dst.g, dst.b, dst.a],
        [src.r, src.g, src.b, src.a],
        opacity,
    );
    RgbaF32::new(blended[0], blended[1], blended[2], blended[3]).to_srgba()
}

fn patterned_surface(seed: u8) -> PublishedSurface {
    let rgba = vec![
        seed,
        32,
        224,
        255,
        192,
        seed,
        48,
        192,
        12,
        180,
        seed,
        96,
        240,
        220,
        seed / 2,
        255,
    ];
    PublishedSurface::from_owned_canvas(Canvas::from_vec(rgba, 2, 2), 7, 11)
}

fn legacy_face_overlay_rgba(
    scene: &PublishedSurface,
    face: &PublishedSurface,
    blend_mode: DisplayFaceBlendMode,
    opacity: f32,
) -> Vec<u8> {
    // Independent copy of the previous display encoder math, kept as a regression fence.
    let mut target_rgba = scene.rgba_bytes().to_vec();
    match blend_mode {
        DisplayFaceBlendMode::Replace => {
            legacy_replace_face_rgba_in_place(&mut target_rgba, face.rgba_bytes(), opacity);
        }
        DisplayFaceBlendMode::Tint => {
            legacy_blend_face_material_tint_rgba(&mut target_rgba, face.rgba_bytes(), opacity);
        }
        DisplayFaceBlendMode::LumaReveal => {
            legacy_blend_face_luma_reveal_rgba(&mut target_rgba, face.rgba_bytes(), opacity);
        }
        _ => {
            let Some(canvas_blend_mode) = blend_mode.standard_canvas_blend_mode() else {
                return target_rgba;
            };
            blend_rgba_pixels_in_place(
                &mut target_rgba,
                face.rgba_bytes(),
                RgbaBlendMode::from(canvas_blend_mode),
                opacity,
            );
        }
    }

    for pixel in target_rgba.chunks_exact_mut(4) {
        pixel[3] = u8::MAX;
    }
    target_rgba
}

fn legacy_replace_face_rgba_in_place(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    for (target_pixel, source_pixel) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let source_alpha = (f32::from(source_pixel[3]) / 255.0) * opacity;
        target_pixel[0] = encode_srgb_channel(decode_srgb_channel(source_pixel[0]) * source_alpha);
        target_pixel[1] = encode_srgb_channel(decode_srgb_channel(source_pixel[1]) * source_alpha);
        target_pixel[2] = encode_srgb_channel(decode_srgb_channel(source_pixel[2]) * source_alpha);
        target_pixel[3] = u8::MAX;
    }
}

fn legacy_blend_face_material_tint_rgba(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    for (dst_px, src_px) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let alpha = (f32::from(src_px[3]) / 255.0) * opacity;
        if alpha <= 0.0 {
            continue;
        }

        let dst = [
            decode_srgb_channel(dst_px[0]),
            decode_srgb_channel(dst_px[1]),
            decode_srgb_channel(dst_px[2]),
        ];
        let src = [
            decode_srgb_channel(src_px[0]),
            decode_srgb_channel(src_px[1]),
            decode_srgb_channel(src_px[2]),
        ];
        let material = legacy_effect_tint_material(dst, src);

        dst_px[0] = encode_srgb_channel(dst[0].mul_add(1.0 - alpha, material[0] * alpha));
        dst_px[1] = encode_srgb_channel(dst[1].mul_add(1.0 - alpha, material[1] * alpha));
        dst_px[2] = encode_srgb_channel(dst[2].mul_add(1.0 - alpha, material[2] * alpha));
    }
}

fn legacy_blend_face_luma_reveal_rgba(target_rgba: &mut [u8], source_rgba: &[u8], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    for (dst_px, src_px) in target_rgba
        .chunks_exact_mut(4)
        .zip(source_rgba.chunks_exact(4))
    {
        let alpha = (f32::from(src_px[3]) / 255.0) * opacity;
        if alpha <= 0.0 {
            continue;
        }

        let dst = [
            decode_srgb_channel(dst_px[0]),
            decode_srgb_channel(dst_px[1]),
            decode_srgb_channel(dst_px[2]),
        ];
        let src = [
            decode_srgb_channel(src_px[0]),
            decode_srgb_channel(src_px[1]),
            decode_srgb_channel(src_px[2]),
        ];
        let material = legacy_effect_tint_material(dst, src);
        let reveal = legacy_smoothstep(0.18, 0.92, legacy_linear_rgb_luma(src));
        let inside = [
            src[0].mul_add(1.0 - reveal, material[0] * reveal),
            src[1].mul_add(1.0 - reveal, material[1] * reveal),
            src[2].mul_add(1.0 - reveal, material[2] * reveal),
        ];

        dst_px[0] = encode_srgb_channel(dst[0].mul_add(1.0 - alpha, inside[0] * alpha));
        dst_px[1] = encode_srgb_channel(dst[1].mul_add(1.0 - alpha, inside[1] * alpha));
        dst_px[2] = encode_srgb_channel(dst[2].mul_add(1.0 - alpha, inside[2] * alpha));
    }
}

fn legacy_effect_tint_material(effect_rgb: [f32; 3], face_rgb: [f32; 3]) -> [f32; 3] {
    let luma = legacy_linear_rgb_luma(face_rgb);
    let colorfulness = legacy_rgb_colorfulness(face_rgb);
    let neutral = 0.18_f32.mul_add(1.0 - luma, luma).clamp(0.18, 1.0);
    let emission_strength = (1.0 - colorfulness) * luma * 0.12;

    std::array::from_fn(|index| {
        let tint = neutral.mul_add(1.0 - 0.72, face_rgb[index].max(neutral * 0.75) * 0.72);
        let filtered = effect_rgb[index] * tint;
        screen_blend(filtered, face_rgb[index] * emission_strength)
    })
}

fn legacy_linear_rgb_luma(rgb: [f32; 3]) -> f32 {
    (rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722).clamp(0.0, 1.0)
}

fn legacy_rgb_colorfulness(rgb: [f32; 3]) -> f32 {
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    (max - min).clamp(0.0, 1.0)
}

fn legacy_smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 >= edge1 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[test]
fn sparkleflinger_rejects_unresolved_auto_mode() {
    let error = SparkleFlinger::new(RenderAccelerationMode::Auto)
        .expect_err("auto mode must be resolved during daemon startup");
    assert!(
        error
            .to_string()
            .contains("must be resolved before constructing SparkleFlinger")
    );
}

#[test]
fn sparkleflinger_face_overlay_matches_legacy_math_for_every_mode() {
    let scene = patterned_surface(48);
    let face = patterned_surface(144);
    let mut sparkleflinger = SparkleFlinger::cpu();

    for blend_mode in [
        DisplayFaceBlendMode::Replace,
        DisplayFaceBlendMode::Alpha,
        DisplayFaceBlendMode::Tint,
        DisplayFaceBlendMode::LumaReveal,
        DisplayFaceBlendMode::Add,
        DisplayFaceBlendMode::Screen,
        DisplayFaceBlendMode::Multiply,
        DisplayFaceBlendMode::Overlay,
        DisplayFaceBlendMode::SoftLight,
        DisplayFaceBlendMode::ColorDodge,
        DisplayFaceBlendMode::Difference,
    ] {
        let expected = legacy_face_overlay_rgba(&scene, &face, blend_mode, 0.6);
        let mut composed = scene.rgba_bytes().to_vec();
        SparkleFlinger::blend_face_overlay_rgba(&mut composed, face.rgba_bytes(), blend_mode, 0.6);

        assert_eq!(
            composed, expected,
            "slice face overlay mismatch for {blend_mode:?}",
        );

        let surface = sparkleflinger.compose_face_overlay(&scene, &face, blend_mode, 0.6);
        assert_eq!(
            surface.rgba_bytes(),
            expected.as_slice(),
            "surface face overlay mismatch for {blend_mode:?}",
        );
    }
}

#[test]
fn sparkleflinger_composes_face_modes_as_general_layers() {
    let scene = patterned_surface(48);
    let face = patterned_surface(144);
    let mut sparkleflinger = SparkleFlinger::cpu();

    for (composition_mode, face_mode) in [
        (CompositionMode::Tint, DisplayFaceBlendMode::Tint),
        (
            CompositionMode::LumaReveal,
            DisplayFaceBlendMode::LumaReveal,
        ),
    ] {
        let mut expected = legacy_face_overlay_rgba(&scene, &face, face_mode, 0.6);
        for (expected_pixel, scene_pixel) in expected
            .chunks_exact_mut(4)
            .zip(scene.rgba_bytes().chunks_exact(4))
        {
            expected_pixel[3] = scene_pixel[3];
        }
        let composed = sparkleflinger.compose(CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace_opaque(ProducerFrame::Surface(scene.clone())),
                CompositionLayer::from_parts(
                    ProducerFrame::Surface(face.clone()),
                    composition_mode,
                    0.6,
                    true,
                ),
            ],
        ));
        assert_eq!(
            composed
                .sampling_canvas
                .expect("general layer composition should materialize a canvas")
                .as_rgba_bytes(),
            expected.as_slice(),
            "general layer composition mismatch for {composition_mode:?}",
        );
    }
}

#[test]
fn sparkleflinger_face_overlay_uses_black_when_scene_dims_do_not_match_face() {
    let scene = PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 7, 11);
    let mut face_canvas = Canvas::new(1, 1);
    face_canvas.fill(Rgba::new(0, 0, 255, 255));
    let face = PublishedSurface::from_owned_canvas(face_canvas, 8, 12);
    let black = PublishedSurface::from_owned_canvas(Canvas::new(1, 1), 0, 0);
    let expected = legacy_face_overlay_rgba(&black, &face, DisplayFaceBlendMode::Tint, 0.75);
    let mut sparkleflinger = SparkleFlinger::cpu();

    let surface =
        sparkleflinger.compose_face_overlay(&scene, &face, DisplayFaceBlendMode::Tint, 0.75);

    assert_eq!(surface.rgba_bytes(), expected.as_slice());
}

#[test]
fn sparkleflinger_bypasses_single_replace_surface() {
    let source =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose(CompositionPlan::single(
        2,
        2,
        CompositionLayer::replace(ProducerFrame::Surface(source.clone())),
    ));

    let surface = composed
        .sampling_surface
        .expect("single replace layer should bypass into a surface");
    assert_eq!(surface.rgba_bytes().as_ptr(), source.rgba_bytes().as_ptr());
    assert!(composed.preview_surface.is_none());
    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("bypass path should materialize a canvas view")
            .as_rgba_bytes()
            .as_ptr(),
        source.rgba_bytes().as_ptr()
    );
}

#[test]
fn sparkleflinger_preview_only_frame_reuses_full_size_surface() {
    let source =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
    let mut sparkleflinger = SparkleFlinger::cpu();

    let composed = sparkleflinger.preview_only_frame(
        ProducerFrame::Surface(source.clone()),
        Some(PreviewSurfaceRequest {
            width: 2,
            height: 2,
        }),
    );

    let preview_surface = composed
        .preview_surface
        .expect("full-size preview-only path should reuse the existing surface");
    assert_eq!(
        preview_surface.storage_identity(),
        source.storage_identity()
    );
    assert!(composed.bypassed);
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
}

#[test]
fn sparkleflinger_preview_only_frame_scales_surface_preview() {
    let mut source_canvas = Canvas::new(2, 2);
    source_canvas.set_pixel(0, 0, Rgba::new(255, 0, 0, 255));
    source_canvas.set_pixel(1, 0, Rgba::new(0, 255, 0, 255));
    source_canvas.set_pixel(0, 1, Rgba::new(0, 0, 255, 255));
    source_canvas.set_pixel(1, 1, Rgba::new(255, 255, 0, 255));
    let source = PublishedSurface::from_owned_canvas(source_canvas, 7, 11);
    let mut sparkleflinger = SparkleFlinger::cpu();

    let composed = sparkleflinger.preview_only_frame(
        ProducerFrame::Surface(source),
        Some(PreviewSurfaceRequest {
            width: 1,
            height: 1,
        }),
    );

    let preview_surface = composed
        .preview_surface
        .expect("scaled preview-only path should materialize a preview surface");
    assert_eq!(preview_surface.width(), 1);
    assert_eq!(preview_surface.height(), 1);
}

#[test]
fn sparkleflinger_scaled_preview_reuses_surface_pool_after_warmup() {
    let source =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 96, 255)), 7, 11);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let request = Some(PreviewSurfaceRequest {
        width: 1,
        height: 1,
    });

    let first = sparkleflinger
        .preview_only_frame(ProducerFrame::Surface(source.clone()), request)
        .preview_surface
        .expect("first scaled preview should publish")
        .rgba_bytes()
        .as_ptr()
        .addr();
    let second = sparkleflinger
        .preview_only_frame(ProducerFrame::Surface(source.clone()), request)
        .preview_surface
        .expect("second scaled preview should publish")
        .rgba_bytes()
        .as_ptr()
        .addr();
    let third = sparkleflinger
        .preview_only_frame(ProducerFrame::Surface(source), request)
        .preview_surface
        .expect("third scaled preview should publish")
        .rgba_bytes()
        .as_ptr()
        .addr();

    assert_ne!(first, second);
    assert_eq!(first, third);
}

#[test]
fn sparkleflinger_composed_frame_reuses_surface_pool_after_warmup() {
    let base = PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 7, 11);
    let overlay =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(0, 0, 255, 255)), 8, 12);
    let plan = CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(base)),
            CompositionLayer::alpha(ProducerFrame::Surface(overlay), 0.5),
        ],
    )
    .with_cpu_replay_cacheable(false);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let request = Some(PreviewSurfaceRequest {
        width: 2,
        height: 2,
    });

    let first_surface = sparkleflinger
        .compose_for_outputs(plan.clone(), false, request)
        .sampling_surface
        .expect("first composed surface should publish");
    let first = first_surface.rgba_bytes().as_ptr().addr();
    let second_surface = sparkleflinger
        .compose_for_outputs(plan.clone(), false, request)
        .sampling_surface
        .expect("second composed surface should publish");
    let second = second_surface.rgba_bytes().as_ptr().addr();

    drop(first_surface);
    drop(second_surface);

    let third = sparkleflinger
        .compose_for_outputs(plan, false, request)
        .sampling_surface
        .expect("third composed surface should publish")
        .rgba_bytes()
        .as_ptr()
        .addr();

    assert_ne!(first, second);
    assert_eq!(first, third);
}

#[test]
fn sparkleflinger_skips_sampling_surface_when_not_requested() {
    let base = solid_canvas(Rgba::new(255, 0, 0, 255));
    let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(base)),
                CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
            ],
        ),
        true,
        None,
    );

    assert!(composed.sampling_canvas.is_some());
    assert!(composed.sampling_surface.is_none());
}

#[test]
fn sparkleflinger_skips_sampling_surface_for_uncacheable_shared_multilayer_plans() {
    let base_surface =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 0, 0, 255)), 0, 0);
    let overlay_surface =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(0, 0, 255, 255)), 0, 0);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace_canvas(Canvas::from_published_surface(&base_surface)),
                CompositionLayer::alpha_canvas(
                    Canvas::from_published_surface(&overlay_surface),
                    0.5,
                ),
            ],
        )
        .with_cpu_replay_cacheable(false),
        true,
        None,
    );

    assert!(composed.sampling_canvas.is_some());
    assert!(composed.sampling_surface.is_none());
}

#[test]
fn sparkleflinger_cpu_scales_preview_surface_when_requested() {
    let base = solid_canvas(Rgba::new(255, 0, 0, 255));
    let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::with_layers(
            2,
            2,
            vec![
                CompositionLayer::replace(ProducerFrame::Canvas(base)),
                CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
            ],
        ),
        true,
        Some(PreviewSurfaceRequest {
            width: 1,
            height: 1,
        }),
    );

    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU sampling should keep the full-size canvas")
            .width(),
        2
    );
    let preview_surface = composed
        .preview_surface
        .expect("scaled preview requests should publish a preview surface");
    assert_eq!(preview_surface.width(), 1);
    assert_eq!(preview_surface.height(), 1);
}

#[test]
fn sparkleflinger_alpha_layers_respect_order() {
    let base = Rgba::new(255, 0, 0, 255);
    let overlay = Rgba::new(0, 0, 255, 255);
    let opacity = 0.25;
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
            CompositionLayer::alpha(ProducerFrame::Canvas(solid_canvas(overlay)), opacity),
        ],
    ));
    let reversed = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(overlay))),
            CompositionLayer::alpha(ProducerFrame::Canvas(solid_canvas(base)), opacity),
        ],
    ));

    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU alpha compose should materialize a canvas")
            .get_pixel(0, 0),
        expected_blend(base, overlay, BlendMode::Normal, opacity)
    );
    assert_ne!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU alpha compose should materialize a canvas")
            .get_pixel(0, 0),
        reversed
            .sampling_canvas
            .as_ref()
            .expect("CPU alpha compose should materialize a canvas")
            .get_pixel(0, 0)
    );
    let composed_surface = composed
        .sampling_surface
        .expect("composed frame should publish an immutable sampling surface");
    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU alpha compose should materialize a canvas")
            .as_rgba_bytes()
            .as_ptr(),
        composed_surface.rgba_bytes().as_ptr()
    );
}

#[test]
fn sparkleflinger_add_layers_use_additive_blend() {
    let base = Rgba::new(64, 0, 0, 255);
    let glow = Rgba::new(0, 96, 64, 255);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
            CompositionLayer::add(ProducerFrame::Canvas(solid_canvas(glow)), 1.0),
        ],
    ));

    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU add compose should materialize a canvas")
            .get_pixel(0, 0),
        expected_blend(base, glow, BlendMode::Add, 1.0)
    );
}

#[test]
fn sparkleflinger_screen_layers_use_screen_blend() {
    let base = Rgba::new(32, 64, 96, 255);
    let overlay = Rgba::new(96, 64, 32, 255);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
            CompositionLayer::screen(ProducerFrame::Canvas(solid_canvas(overlay)), 1.0),
        ],
    ));

    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU screen compose should materialize a canvas")
            .get_pixel(0, 0),
        expected_blend(base, overlay, BlendMode::Screen, 1.0)
    );
}

#[test]
fn sparkleflinger_extended_blend_modes_use_linear_blend_math() {
    let base = Rgba::new(96, 128, 192, 255);
    let overlay = Rgba::new(128, 96, 64, 255);
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(base))),
            CompositionLayer::from_parts(
                ProducerFrame::Canvas(solid_canvas(overlay)),
                CompositionMode::Multiply,
                1.0,
                false,
            ),
        ],
    ));

    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU multiply compose should materialize a canvas")
            .get_pixel(0, 0),
        expected_blend(base, overlay, BlendMode::Multiply, 1.0)
    );
}

#[test]
fn sparkleflinger_transform_fit_modes_sample_expected_pixels() {
    let red = Rgba::new(255, 0, 0, 255);
    let green = Rgba::new(0, 255, 0, 255);
    let source = row_canvas(&[red, green]);

    let stretch = compose_transformed_source(source.clone(), 4, 1, FitMode::Stretch);
    assert_eq!(stretch.get_pixel(0, 0), red);
    assert_eq!(stretch.get_pixel(1, 0), red);
    assert_eq!(stretch.get_pixel(2, 0), green);
    assert_eq!(stretch.get_pixel(3, 0), green);

    let tile = compose_transformed_source(source.clone(), 4, 1, FitMode::Tile);
    assert_eq!(tile.get_pixel(0, 0), red);
    assert_eq!(tile.get_pixel(1, 0), green);
    assert_eq!(tile.get_pixel(2, 0), red);
    assert_eq!(tile.get_pixel(3, 0), green);

    let mirror = compose_transformed_source(source.clone(), 4, 1, FitMode::Mirror);
    assert_eq!(mirror.get_pixel(0, 0), red);
    assert_eq!(mirror.get_pixel(1, 0), green);
    assert_eq!(mirror.get_pixel(2, 0), green);
    assert_eq!(mirror.get_pixel(3, 0), red);

    let contain = compose_transformed_source(source, 4, 4, FitMode::Contain);
    assert_eq!(contain.get_pixel(0, 0), Rgba::TRANSPARENT);
    assert_eq!(contain.get_pixel(0, 1), red);
    assert_eq!(contain.get_pixel(3, 2), green);
    assert_eq!(contain.get_pixel(0, 3), Rgba::TRANSPARENT);
}

#[test]
fn sparkleflinger_layer_adjust_applies_before_blending() {
    let mut sparkleflinger = SparkleFlinger::cpu();
    let adjusted = sparkleflinger.compose(CompositionPlan::single(
        2,
        2,
        CompositionLayer::replace(ProducerFrame::Canvas(solid_canvas(Rgba::WHITE))).with_adjust(
            CompositionAdjust {
                tint: [0.0, 0.0, 1.0, 1.0],
                tint_strength: 1.0,
                ..CompositionAdjust::default()
            },
        ),
    ));

    assert_eq!(
        adjusted
            .sampling_canvas
            .as_ref()
            .expect("adjusted layer should materialize a canvas")
            .get_pixel(0, 0),
        Rgba::new(0, 0, 255, 255)
    );
}

#[test]
fn sparkleflinger_reuses_first_replace_canvas_for_multi_layer_plans() {
    let base = solid_canvas(Rgba::new(255, 0, 0, 255));
    let base_ptr = base.as_rgba_bytes().as_ptr();
    let overlay = solid_canvas(Rgba::new(0, 0, 255, 255));
    let mut sparkleflinger = SparkleFlinger::cpu();
    let composed = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace(ProducerFrame::Canvas(base)),
            CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
        ],
    ));

    assert_eq!(
        composed
            .sampling_canvas
            .as_ref()
            .expect("CPU multi-layer compose should materialize a canvas")
            .as_rgba_bytes()
            .as_ptr(),
        base_ptr
    );
}

#[test]
fn sparkleflinger_reuses_cached_shared_multilayer_compositions() {
    let base_surface =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(255, 32, 0, 255)), 1, 1);
    let overlay_surface =
        PublishedSurface::from_owned_canvas(solid_canvas(Rgba::new(32, 64, 255, 255)), 1, 1);
    let mut sparkleflinger = SparkleFlinger::cpu();

    let first = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace_canvas(Canvas::from_published_surface(&base_surface)),
            CompositionLayer::alpha_canvas(Canvas::from_published_surface(&overlay_surface), 0.35),
        ],
    ));
    let second = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace_canvas(Canvas::from_published_surface(&base_surface)),
            CompositionLayer::alpha_canvas(Canvas::from_published_surface(&overlay_surface), 0.35),
        ],
    ));

    let first_surface = first
        .sampling_surface
        .expect("initial shared composition should publish a sampling surface");
    let second_surface = second
        .sampling_surface
        .expect("cached shared composition should publish a sampling surface");
    assert_eq!(
        first_surface.storage_identity(),
        second_surface.storage_identity()
    );
    assert_eq!(
        first_surface.rgba_bytes().as_ptr(),
        second_surface.rgba_bytes().as_ptr()
    );
    assert!(!second.bypassed);
}

#[test]
fn sparkleflinger_does_not_reuse_cached_composition_after_canvas_mutation() {
    let mut base = solid_canvas(Rgba::new(255, 32, 0, 255));
    let overlay = solid_canvas(Rgba::new(32, 64, 255, 255));
    let mut sparkleflinger = SparkleFlinger::cpu();

    let first = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace_canvas(base.clone()),
            CompositionLayer::alpha_canvas(overlay.clone(), 0.35),
        ],
    ));
    base.set_pixel(0, 0, Rgba::new(0, 255, 0, 255));
    let second = sparkleflinger.compose(CompositionPlan::with_layers(
        2,
        2,
        vec![
            CompositionLayer::replace_canvas(base),
            CompositionLayer::alpha_canvas(overlay, 0.35),
        ],
    ));

    let first_surface = first
        .sampling_surface
        .expect("initial composition should publish a sampling surface");
    let second_surface = second
        .sampling_surface
        .expect("mutated composition should publish a sampling surface");
    assert_ne!(
        first_surface.storage_identity(),
        second_surface.storage_identity()
    );
    assert_ne!(
        first
            .sampling_canvas
            .as_ref()
            .expect("initial composition should materialize a canvas")
            .get_pixel(0, 0),
        second
            .sampling_canvas
            .as_ref()
            .expect("mutated composition should materialize a canvas")
            .get_pixel(0, 0)
    );
}

#[cfg(feature = "wgpu")]
mod gpu;
