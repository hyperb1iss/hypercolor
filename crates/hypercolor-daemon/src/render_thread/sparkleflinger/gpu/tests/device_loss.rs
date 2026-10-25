//! Device loss destroys every buffer the device owns, including readback
//! buffers whose map already completed. These tests lose the device at
//! that point and check that the compositor reports errors instead of
//! panicking.

use super::*;
use crate::render_thread::sparkleflinger::gpu::display_finalize::GpuDisplayFinalizeDispatch;
use crate::render_thread::sparkleflinger::gpu::readback::copy_mapped_readback_buffer_into_surface;

#[test]
fn yuv_display_readback_reports_a_buffer_lost_after_its_map_completed() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let scene = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(0, 0, 0, 255)));
    let face = ProducerFrame::Canvas(solid_canvas_with_size(2, 2, Rgba::new(255, 0, 0, 255)));
    let mut params = display_finalize_params(2, 2, BlendMode::Replace);
    params.cache_key.frame_format = DisplayFrameFormat::Jpeg;
    let GpuDisplayFinalizeDispatch::Pending(mut pending) = compositor
        .begin_finalize_display_face_yuv420(&scene, &face, params)
        .expect("YUV display finalize should dispatch")
    else {
        panic!("a 2x2 YUV display finalize should be admitted");
    };
    compositor.wait_for_gpu_idle();
    compositor.defer_next_display_finalize_resolve();
    assert!(
        compositor
            .try_finish_pending_display_finalization(&mut pending)
            .expect("the completed map should resolve")
            .is_none()
    );
    assert!(pending.map_ready(), "the map completes before the loss");

    compositor.lose_device();

    let Err(error) = compositor.try_finish_pending_display_finalization(&mut pending) else {
        panic!("a readback destroyed after its map must report an error");
    };
    assert!(format!("{error:#}").contains("not mapped"), "{error:#}");
    assert_eq!(compositor.display_finalize_readback_slots_in_use(), 0);
}

#[test]
fn surface_readback_reports_a_buffer_lost_after_its_map_completed() {
    let Some(compositor) = super::gpu_test_compositor() else {
        return;
    };
    let buffer = compositor.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SparkleFlinger device loss readback test"),
        size: u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let (sender, receiver) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    compositor.wait_for_gpu_idle();
    receiver
        .recv()
        .expect("the map callback should run")
        .expect("the map should complete before the loss");

    compositor.lose_device();

    let mut surfaces = RenderSurfacePool::try_new(SurfaceDescriptor::rgba8888(1, 1))
        .expect("a 1x1 surface pool should allocate");
    let mut last_readback_bytes = 0;
    let error = copy_mapped_readback_buffer_into_surface(
        &buffer,
        u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
        1,
        1,
        wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        &mut surfaces,
        &mut last_readback_bytes,
    )
    .expect_err("a readback destroyed after its map must report an error");
    assert!(error.to_string().contains("not mapped"), "{error:#}");
}
