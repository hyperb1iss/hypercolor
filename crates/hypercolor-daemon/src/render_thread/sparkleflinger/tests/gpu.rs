use super::*;

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
