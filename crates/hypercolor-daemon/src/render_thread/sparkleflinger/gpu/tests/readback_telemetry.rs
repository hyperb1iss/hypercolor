use super::*;
use crate::render_thread::sparkleflinger::gpu::gpu_transfer_telemetry_snapshot;

#[test]
#[ignore = "process-global counters require an isolated filtered run with --ignored --test-threads=1"]
fn gpu_readback_accounting_excludes_padding_and_cached_publications() {
    let mut compositor = GpuSparkleFlinger::new().expect("GPU accounting needs an adapter");
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(3, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(3, 2, Rgba::new(255, 0, 0, 255)));
    let params = display_finalize_params(3, 2, BlendMode::Replace);
    let before = gpu_transfer_telemetry_snapshot();
    let surface = finalize_display_face_blocking(&mut compositor, &scene, &face, params);
    assert_eq!(surface.rgba_bytes().len(), 24);
    let rgba = gpu_transfer_telemetry_snapshot();
    assert_eq!(
        rgba.display_readback_count - before.display_readback_count,
        1
    );
    // Each GPU row is padded to 256 bytes; only its 12 color bytes count.
    assert_eq!(
        rgba.display_readback_bytes - before.display_readback_bytes,
        24
    );
    let yuv = finalize_display_face_yuv420_blocking(&mut compositor, &scene, &face, params);
    let yuv_bytes = yuv.y_plane().len() + yuv.u_plane().len() + yuv.v_plane().len();
    let after_yuv = gpu_transfer_telemetry_snapshot();
    assert_eq!(
        after_yuv.display_readback_count - rgba.display_readback_count,
        1
    );
    assert_eq!(
        after_yuv.display_readback_bytes - rgba.display_readback_bytes,
        yuv_bytes as u64
    );

    let plan = CompositionPlan::with_layers(
        4,
        4,
        vec![
            CompositionLayer::replace(ProducerFrame::Surface(slot_surface(Rgba::new(
                255, 32, 0, 255,
            )))),
            CompositionLayer::alpha(
                ProducerFrame::Surface(slot_surface(Rgba::new(32, 64, 255, 255))),
                0.35,
            ),
        ],
    );
    let request = PreviewSurfaceRequest {
        width: 3,
        height: 2,
    };
    let before_preview = gpu_transfer_telemetry_snapshot();
    compositor
        .compose(&plan, false, Some(request))
        .expect("preview must encode");
    let preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(preview.rgba_bytes().len(), 24);
    let after_preview = gpu_transfer_telemetry_snapshot();
    assert_eq!(
        after_preview.preview_readback_count - before_preview.preview_readback_count,
        1
    );
    assert_eq!(
        after_preview.preview_readback_bytes - before_preview.preview_readback_bytes,
        24
    );
    let cached = compositor
        .compose(&plan, false, Some(request))
        .expect("cached preview must compose");
    assert!(cached.preview_surface.is_some());
    assert!(compositor.pending_preview_readback().is_none());
    assert_eq!(gpu_transfer_telemetry_snapshot(), after_preview);

    let engine = SpatialEngine::new(sampling_layout(SamplingMode::Bilinear));
    let mut sampled = Vec::new();
    assert!(
        compositor
            .sample_zone_plan_into(engine.sampling_plan().as_ref(), &mut sampled)
            .expect("LED samples must read back")
    );
    let after_sample = gpu_transfer_telemetry_snapshot();
    let led_count: usize = sampled.iter().map(|zone| zone.colors.len()).sum();
    assert_eq!(
        after_sample.led_readback_count - after_preview.led_readback_count,
        1
    );
    assert_eq!(
        after_sample.led_readback_bytes - after_preview.led_readback_bytes,
        led_count as u64 * 4
    );
    assert_eq!(
        after_sample.sampling_canvas_readback_count,
        after_preview.sampling_canvas_readback_count
    );
}
