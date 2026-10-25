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
