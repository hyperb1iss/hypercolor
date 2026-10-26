//! A buffer the device cannot create comes back invalid while the device
//! stays alive, the way wgpu reports a buffer allocation that ran out of
//! memory. A composition whose upload failed must commit nothing, so the
//! same frame uploads and composes again once the allocation succeeds.

use super::*;
use crate::render_thread::sparkleflinger::gpu_buffer_init::fail_buffer_with_contents_after_for_test;

const PREVIEW: PreviewSurfaceRequest = PreviewSurfaceRequest {
    width: 4,
    height: 4,
};

/// A 4x4 canvas that shares its storage, so plans built from it get stable
/// composition and upload cache keys: the keys a wrongly committed frame
/// would be reused under.
fn shared_canvas(color: Rgba) -> (Canvas, Canvas) {
    let canvas = solid_canvas_with_size(4, 4, color);
    let keepalive = canvas.clone();
    assert!(canvas.is_shared());
    (canvas, keepalive)
}

fn plan_over_black(base: &Canvas, overlays: &[(&Canvas, f32)]) -> CompositionPlan {
    let mut layers = vec![CompositionLayer::replace(ProducerFrame::Canvas(
        base.clone(),
    ))];
    layers.extend(overlays.iter().map(|(overlay, opacity)| {
        CompositionLayer::alpha(ProducerFrame::Canvas((*overlay).clone()), *opacity)
    }));
    CompositionPlan::with_layers(4, 4, layers)
}

fn composed_preview(compositor: &mut GpuSparkleFlinger, plan: &CompositionPlan) -> Vec<u8> {
    compositor
        .compose(plan, false, Some(PREVIEW))
        .expect("composition should succeed");
    resolve_preview_surface_blocking(compositor)
        .rgba_bytes()
        .to_vec()
}

/// Read the compositor's current output texture straight from the GPU, so
/// no cached CPU copy of a frame can stand in for what the texture holds.
fn read_current_output(compositor: &mut GpuSparkleFlinger) -> Vec<u8> {
    let frame = compositor
        .current_output_frame()
        .expect("flushing the output submission should succeed")
        .expect("a composed output should exist");
    let row_bytes = frame.width * 4;
    let padded_row_bytes = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = compositor.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SparkleFlinger upload failure output readback"),
        size: u64::from(padded_row_bytes) * u64::from(frame.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = compositor
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        frame.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(frame.height),
            },
        },
        wgpu::Extent3d {
            width: frame.width,
            height: frame.height,
            depth_or_array_layers: 1,
        },
    );
    compositor.queue.submit(Some(encoder.finish()));
    let (sender, receiver) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    compositor.wait_for_gpu_idle();
    receiver
        .recv()
        .expect("the output readback callback should run")
        .expect("the output readback should map");
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .expect("the output readback should be readable");
    let pixels = mapped
        .chunks_exact(padded_row_bytes as usize)
        .flat_map(|row| row[..row_bytes as usize].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    pixels
}

/// A failed composition's queued texture writes still run at the next
/// submit, so the previous plan's output cannot be reused afterwards even
/// though its cache key never changed.
#[test]
fn a_failed_composition_does_not_leave_the_previous_output_reusable() {
    let (Some(mut compositor), Some(mut reference)) =
        (super::gpu_test_compositor(), super::gpu_test_compositor())
    else {
        return;
    };
    let (base, _base) = shared_canvas(Rgba::new(0, 0, 0, 255));
    let (red, _red) = shared_canvas(Rgba::new(255, 0, 0, 255));
    let (green, _green) = shared_canvas(Rgba::new(0, 255, 0, 255));
    let (blue, _blue) = shared_canvas(Rgba::new(0, 0, 255, 255));
    let plan_a = plan_over_black(&base, &[(&red, 1.0), (&green, 0.5)]);
    let plan_b = plan_over_black(&base, &[(&blue, 1.0)]);

    reference
        .compose(&plan_a, false, None)
        .expect("the reference frame should compose");
    let expected = read_current_output(&mut reference);
    compositor
        .compose(&plan_a, false, None)
        .expect("frame A should compose");
    assert_eq!(read_current_output(&mut compositor), expected);

    // B queues its base straight into the texture holding A, then fails.
    fail_buffer_with_contents_after_for_test(0);
    compositor
        .compose(&plan_b, false, None)
        .expect_err("a failed overlay upload must fail its composition");

    compositor
        .compose(&plan_a, false, None)
        .expect("frame A should compose again");
    assert_eq!(
        read_current_output(&mut compositor),
        expected,
        "frame A must not reuse an output the failed frame overwrote"
    );
}

#[test]
fn a_failed_layer_upload_commits_nothing_and_the_next_frame_recomposes() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let (base, _base) = shared_canvas(Rgba::new(0, 0, 0, 255));
    let (red, _red) = shared_canvas(Rgba::new(255, 0, 0, 255));
    let (green, _green) = shared_canvas(Rgba::new(0, 255, 0, 255));

    let red_frame = composed_preview(&mut compositor, &plan_over_black(&base, &[(&red, 1.0)]));
    assert_eq!(&red_frame[..4], &[255, 0, 0, 255]);

    let green_plan = plan_over_black(&base, &[(&green, 1.0)]);
    fail_buffer_with_contents_after_for_test(0);
    let error = compositor
        .compose(&green_plan, false, Some(PREVIEW))
        .expect_err("a failed overlay upload must fail its composition");
    assert!(
        format!("{error:#}").contains("could not create"),
        "{error:#}"
    );

    let retried = composed_preview(&mut compositor, &green_plan);
    assert_eq!(
        &retried[..4],
        &[0, 255, 0, 255],
        "the retried frame must show its own overlay, not a cached frame"
    );
}

/// The failed composition's encoder is dropped, taking every upload it
/// recorded with it, including ones that succeeded before the failing layer.
/// The next frame must upload those layers again rather than trust a cache
/// that describes work the GPU never ran.
#[test]
fn a_failed_upload_also_forgets_the_layers_encoded_before_it() {
    let (Some(mut compositor), Some(mut reference)) =
        (super::gpu_test_compositor(), super::gpu_test_compositor())
    else {
        return;
    };
    let (base, _base) = shared_canvas(Rgba::new(0, 0, 0, 255));
    let (red, _red) = shared_canvas(Rgba::new(255, 0, 0, 255));
    let (green, _green) = shared_canvas(Rgba::new(0, 255, 0, 255));
    let (blue, _blue) = shared_canvas(Rgba::new(0, 0, 255, 255));
    let (white, _white) = shared_canvas(Rgba::new(255, 255, 255, 255));

    composed_preview(
        &mut compositor,
        &plan_over_black(&base, &[(&red, 1.0), (&green, 0.5)]),
    );

    let next_plan = plan_over_black(&base, &[(&blue, 1.0), (&white, 0.5)]);
    // The blue layer's upload succeeds, then the white layer's fails.
    fail_buffer_with_contents_after_for_test(1);
    compositor
        .compose(&next_plan, false, Some(PREVIEW))
        .expect_err("a failed overlay upload must fail its composition");

    let retried = composed_preview(&mut compositor, &next_plan);
    let expected = composed_preview(&mut reference, &next_plan);
    assert_eq!(
        &retried[..4],
        &expected[..4],
        "the retried frame must match a compositor that never failed"
    );
}

/// A preview that fails after its composition committed leaves that
/// composition valid: its deferred encoder still goes out under the same
/// output generation, and the same plan is a cache hit afterwards.
#[test]
fn a_failed_preview_keeps_its_committed_composition() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let (red, _red) = shared_canvas(Rgba::new(255, 0, 0, 255));
    let plan = CompositionPlan::single(
        4,
        4,
        CompositionLayer::replace(ProducerFrame::Canvas(red.clone())),
    );
    let scaled_preview = PreviewSurfaceRequest {
        width: 2,
        height: 2,
    };

    compositor
        .pipeline
        .preview_scale_params
        .force_next_overflow_for_test();
    fail_buffer_with_contents_after_for_test(0);
    let Err(error) = compositor.compose(&plan, false, Some(scaled_preview)) else {
        panic!("a failed preview params upload must fail the preview");
    };
    assert!(
        format!("{error:#}").contains("preview staging failed"),
        "{error:#}"
    );
    let generation = compositor.output_generation;

    // Exporting flushes the kept encoder, which must still match the
    // output generation it was staged under.
    let output = read_current_output(&mut compositor);
    assert_eq!(&output[..4], &[255, 0, 0, 255]);

    compositor
        .compose(&plan, false, Some(scaled_preview))
        .expect("the same plan should restage its preview");
    assert_eq!(
        compositor.output_generation, generation,
        "the same plan must reuse the committed composition"
    );
    let preview = resolve_preview_surface_blocking(&mut compositor);
    assert_eq!(&preview.rgba_bytes()[..4], &[255, 0, 0, 255]);
}
