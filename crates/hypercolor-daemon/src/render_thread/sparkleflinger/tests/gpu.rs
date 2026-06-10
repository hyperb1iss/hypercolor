use super::*;

#[cfg(feature = "wgpu")]
use crate::performance::CompositorBackendKind;

#[cfg(feature = "wgpu")]
#[test]
fn sparkleflinger_uploads_canvas_as_gpu_texture_frame() {
    let Ok(mut sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
        return;
    };
    let source = solid_canvas(Rgba::new(32, 96, 160, 255));
    let Some(frame) = sparkleflinger.upload_canvas_frame(&source) else {
        panic!("GPU canvas upload should return a texture frame");
    };

    assert_eq!(frame.width, source.width());
    assert_eq!(frame.height, source.height());

    let composed = sparkleflinger.compose_for_outputs(
        CompositionPlan::single(
            source.width(),
            source.height(),
            CompositionLayer::replace(ProducerFrame::GpuTexture(frame)),
        ),
        false,
        None,
    );

    assert_eq!(composed.backend, CompositorBackendKind::Gpu);
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert!(
        sparkleflinger
            .current_output_frame()
            .is_ok_and(|frame| frame.is_some())
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn sparkleflinger_gaussian_zones_sample_latched_gpu_canvas() {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition as SpatialPosition, Output, SamplingMode,
        SpatialLayout, StripDirection,
    };

    let Ok(mut sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
        return;
    };
    let engine = SpatialEngine::new(SpatialLayout {
        id: "gaussian-latch".into(),
        name: "Gaussian Latch".into(),
        description: None,
        canvas_width: 4,
        canvas_height: 4,
        zones: vec![Output {
            id: "strip".into(),
            name: "strip".into(),
            device_id: "device:strip".into(),
            zone_name: None,
            position: SpatialPosition::new(0.5, 0.5),
            size: SpatialPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            orientation: None,
            topology: LedTopology::Strip {
                count: 4,
                direction: StripDirection::LeftToRight,
            },
            led_positions: Vec::new(),
            led_mapping: None,
            sampling_mode: Some(SamplingMode::GaussianArea {
                sigma: 1.0,
                radius: 2,
            }),
            edge_behavior: Some(EdgeBehavior::Clamp),
            shape: None,
            shape_preset: None,
            display_order: 0,
            attachment: None,
            brightness: None,
        }],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    });
    // Gaussian zones reject GPU sampling by design, so this layout forces
    // the CPU sampling path that depends on the readback latch.
    assert!(
        !sparkleflinger.can_sample_zone_plan(engine.sampling_plan().as_ref()),
        "GaussianArea zones should stay rejected by the GPU zone sampler",
    );

    let mut content = Canvas::new(4, 4);
    content.fill(Rgba::new(64, 160, 224, 255));
    let frame = sparkleflinger
        .upload_canvas_frame(&content)
        .expect("GPU canvas upload should return a texture frame");
    let compose_gpu_plan = |sparkleflinger: &mut SparkleFlinger| {
        sparkleflinger.compose_for_outputs(
            CompositionPlan::single(
                4,
                4,
                CompositionLayer::replace(ProducerFrame::GpuTexture(frame.clone())),
            )
            .with_cpu_replay_cacheable(false),
            true,
            None,
        )
    };

    let first = compose_gpu_plan(&mut sparkleflinger);
    assert_eq!(first.backend, CompositorBackendKind::Gpu);

    // The latch primes on the first compose; the next composes return the
    // previously staged readback once the GPU finishes it.
    let mut latched_canvas = first.sampling_canvas;
    let mut composes = 1_u32;
    while latched_canvas.is_none() && composes < 50 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let composed = compose_gpu_plan(&mut sparkleflinger);
        assert_eq!(composed.backend, CompositorBackendKind::Gpu);
        latched_canvas = composed.sampling_canvas;
        composes += 1;
    }
    let latched_canvas =
        latched_canvas.expect("the sampling readback latch should resolve a canvas");
    assert_eq!(
        latched_canvas.as_rgba_bytes(),
        content.as_rgba_bytes(),
        "latched canvas should carry the GPU-composed frame",
    );

    let zones = engine.sample(&latched_canvas);
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].colors.len(), 4);
    for color in &zones[0].colors {
        for (channel, expected) in color.iter().zip([64_u8, 160, 224]) {
            assert!(
                channel.abs_diff(expected) <= 2,
                "Gaussian-sampled LED color {color:?} should match the composed content",
            );
        }
    }
}

#[cfg(feature = "wgpu")]
#[test]
fn sparkleflinger_refuses_gpu_frame_cpu_readback_fallback() {
    let Ok(mut sparkleflinger) = SparkleFlinger::new(RenderAccelerationMode::Gpu) else {
        return;
    };
    let base = solid_canvas(Rgba::new(20, 40, 60, 255));
    let overlay = solid_canvas(Rgba::new(200, 40, 80, 192));
    sparkleflinger.compose(CompositionPlan::single(
        2,
        2,
        CompositionLayer::replace(ProducerFrame::Canvas(base.clone())),
    ));
    let gpu_frame = sparkleflinger
        .current_output_frame()
        .expect("GPU output frame export should not fail")
        .expect("GPU output frame should be available");
    let fallback_plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::GpuTexture(gpu_frame)),
            CompositionLayer::alpha(ProducerFrame::Canvas(overlay), 0.5),
        ],
    );

    let composed = sparkleflinger.compose_for_outputs(
        fallback_plan,
        true,
        Some(PreviewSurfaceRequest {
            width: 1,
            height: 1,
        }),
    );

    assert_eq!(composed.backend, CompositorBackendKind::GpuFallback);
    assert!(composed.gpu_readback_failed);
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert_eq!(
        composed
            .preview_surface
            .expect("scaled fallback preview should remain available")
            .rgba_bytes(),
        &[0, 0, 0, 255],
    );
}

#[cfg(feature = "wgpu")]
#[test]
fn sparkleflinger_gpu_readback_failure_composes_black() {
    let mut preview_surface_pool = new_preview_surface_pool();
    let composed = gpu_frame_without_cpu_fallback(
        2,
        2,
        Some(PreviewSurfaceRequest {
            width: 1,
            height: 1,
        }),
        &mut preview_surface_pool,
    );

    assert_eq!(composed.backend, CompositorBackendKind::GpuFallback);
    assert!(composed.gpu_readback_failed);
    assert!(composed.sampling_canvas.is_none());
    assert!(composed.sampling_surface.is_none());
    assert_eq!(
        composed
            .preview_surface
            .expect("scaled fallback preview should remain available")
            .rgba_bytes(),
        &[0, 0, 0, 255],
    );
}
