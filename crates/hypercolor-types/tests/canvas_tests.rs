//! Tests for the canvas surface, its pooling, the authored blend modes,
//! and the canvas-facing re-export of the color kernel.
//!
//! Color math itself is tested in `hypercolor-color`. What is tested
//! here is that canvas's public paths still resolve to the kernel's
//! types and that their serialized shapes did not move.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hypercolor_types::canvas::{
    BYTES_PER_PIXEL, BlendMode, Canvas, Color, ColorFormat, DEFAULT_CANVAS_HEIGHT,
    DEFAULT_CANVAS_WIDTH, LinearRgba, Oklab, Oklch, PublishedSurface, RenderSurfacePool, Rgb, Rgba,
    SamplingMethod, SurfaceDescriptor, SurfaceResourceError, SurfaceState, linear_to_srgb,
    srgb_to_linear,
};

// ── Kernel re-export identity ──────────────────────────────────────────────

/// The canvas paths are the kernel's types, not copies of them. Each
/// assignment below fails to compile if the identity is ever broken by a
/// re-introduced local definition.
#[test]
fn canvas_color_paths_are_the_kernel_types() {
    let rgb: hypercolor_color::Rgb = Rgb::new(1, 2, 3);
    let rgba: hypercolor_color::Rgba = Rgba::new(1, 2, 3, 4);
    let linear: hypercolor_color::LinearRgba = LinearRgba::new(0.1, 0.2, 0.3, 0.4);
    let color: Color = linear;
    let lab: hypercolor_color::Oklab = Oklab::new(0.5, 0.0, 0.0, 1.0);
    let lch: hypercolor_color::Oklch = Oklch::new(0.5, 0.1, 90.0, 1.0);

    assert_eq!(rgb, hypercolor_color::Rgb::new(1, 2, 3));
    assert_eq!(rgba, hypercolor_color::Rgba::new(1, 2, 3, 4));
    assert_eq!(color, linear);
    assert_eq!(lab.l, 0.5);
    assert_eq!(lch.h, 90.0);
}

/// The transfer functions and LUT entry points keep their canvas paths.
#[test]
fn canvas_transfer_paths_still_resolve() {
    use hypercolor_types::canvas::{
        linear_to_output_u8, linear_to_srgb, linear_to_srgb_u8, srgb_to_linear, srgb_u8_to_linear,
    };

    assert_eq!(srgb_u8_to_linear(0), 0.0);
    assert_eq!(linear_to_srgb_u8(1.0), 255);
    assert_eq!(linear_to_output_u8(1.0), 255);
    assert!((linear_to_srgb(srgb_to_linear(0.5)) - 0.5).abs() < 0.001);
}

// ── Serialized shape stability ─────────────────────────────────────────────

/// Byte colors serialize as struct maps with `r`/`g`/`b`/`a` keys. The
/// kernel gates serde behind a feature while canvas derived it
/// unconditionally, so this pins the shape across the swap. Persisted
/// scenes and presets read these bytes.
#[test]
fn byte_color_json_shape_is_unchanged() {
    let rgba = Rgba::new(42, 128, 255, 200);
    assert_eq!(
        serde_json::to_string(&rgba).expect("serialize"),
        r#"{"r":42,"g":128,"b":255,"a":200}"#
    );
    assert_eq!(
        serde_json::from_str::<Rgba>(r#"{"r":42,"g":128,"b":255,"a":200}"#).expect("deserialize"),
        rgba
    );

    let rgb = Rgb::new(7, 8, 9);
    assert_eq!(
        serde_json::to_string(&rgb).expect("serialize"),
        r#"{"r":7,"g":8,"b":9}"#
    );
    assert_eq!(
        serde_json::from_str::<Rgb>(r#"{"r":7,"g":8,"b":9}"#).expect("deserialize"),
        rgb
    );
}

/// The float color keeps its `r`/`g`/`b`/`a` map under both its
/// canonical name and the canvas `Color` alias.
#[test]
fn linear_color_json_shape_is_unchanged() {
    let linear = LinearRgba::new(0.25, 0.5, 0.75, 1.0);
    let json = serde_json::to_string(&linear).expect("serialize");
    assert_eq!(json, r#"{"r":0.25,"g":0.5,"b":0.75,"a":1.0}"#);
    assert_eq!(
        serde_json::from_str::<Color>(&json).expect("deserialize"),
        linear
    );
}

/// The perceptual types keep their alpha-bearing field names.
#[test]
fn perceptual_color_json_shape_is_unchanged() {
    let lab = Oklab::new(0.5, 0.1, -0.2, 0.75);
    assert_eq!(
        serde_json::to_string(&lab).expect("serialize"),
        r#"{"l":0.5,"a":0.1,"b":-0.2,"alpha":0.75}"#
    );

    let lch = Oklch::new(0.65, 0.2, 180.0, 0.95);
    assert_eq!(
        serde_json::to_string(&lch).expect("serialize"),
        r#"{"l":0.65,"c":0.2,"h":180.0,"alpha":0.95}"#
    );
    assert_eq!(
        serde_json::from_str::<Oklch>(r#"{"l":0.65,"c":0.2,"h":180.0,"alpha":0.95}"#)
            .expect("deserialize"),
        lch
    );
}

// ── Canvas-facing color constants ──────────────────────────────────────────

#[test]
fn rgba_constants_and_default() {
    assert_eq!(Rgba::BLACK, Rgba::new(0, 0, 0, 255));
    assert_eq!(Rgba::WHITE, Rgba::new(255, 255, 255, 255));
    assert_eq!(Rgba::TRANSPARENT, Rgba::new(0, 0, 0, 0));
    assert_eq!(Rgba::default(), Rgba::BLACK);
    assert_eq!(Rgb::default(), Rgb::new(0, 0, 0));
}

#[test]
fn byte_color_promotion_and_demotion() {
    assert_eq!(
        Rgb::new(100, 150, 200).to_rgba(),
        Rgba::new(100, 150, 200, 255)
    );
    assert_eq!(
        Rgba::new(100, 150, 200, 128).to_rgb(),
        Rgb::new(100, 150, 200)
    );
}

#[test]
fn linear_color_default_is_opaque_black() {
    let c = Color::default();
    assert_eq!(c, LinearRgba::new(0.0, 0.0, 0.0, 1.0));
}

// ── Canvas Construction ────────────────────────────────────────────────────

#[test]
fn canvas_new_default_size() {
    let c = Canvas::default();
    assert_eq!(c.width(), DEFAULT_CANVAS_WIDTH);
    assert_eq!(c.height(), DEFAULT_CANVAS_HEIGHT);
}

#[test]
fn canvas_new_custom_size() {
    let c = Canvas::new(10, 20);
    assert_eq!(c.width(), 10);
    assert_eq!(c.height(), 20);
    assert_eq!(c.as_rgba_bytes().len(), 10 * 20 * BYTES_PER_PIXEL);
}

#[test]
fn canvas_new_filled_opaque_black() {
    let c = Canvas::new(4, 4);
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 0, 255]);
    }
}

#[test]
fn surface_dimensions_are_checked_without_a_fixed_resolution_ceiling() {
    let descriptor = SurfaceDescriptor::rgba8888(7_680, 4_320);
    assert_eq!(descriptor.try_byte_len(), Ok(132_710_400));

    let portrait = SurfaceDescriptor::rgba8888(1_081, 1_921);
    assert_eq!(portrait.try_byte_len(), Ok(8_306_404));
}

#[test]
fn surface_dimensions_reject_zero_and_address_space_overflow() {
    assert!(matches!(
        Canvas::try_new(0, 1),
        Err(SurfaceResourceError::EmptyDimensions {
            width: 0,
            height: 1
        })
    ));
    assert_eq!(
        SurfaceDescriptor::rgba8888(u32::MAX, u32::MAX).try_byte_len(),
        Err(SurfaceResourceError::ByteLengthOverflow {
            width: u32::MAX,
            height: u32::MAX,
        })
    );
}

#[test]
fn fallible_canvas_construction_supports_one_pixel_and_odd_shapes() {
    let one = Canvas::try_new(1, 1).expect("one pixel canvas");
    assert_eq!(one.as_rgba_bytes(), &[0, 0, 0, 255]);

    let odd =
        Canvas::try_from_vec(vec![9; 13 * 17 * BYTES_PER_PIXEL], 13, 17).expect("odd canvas shape");
    assert_eq!((odd.width(), odd.height()), (13, 17));
}

#[test]
fn fallible_canvas_construction_reports_buffer_mismatches() {
    assert!(matches!(
        Canvas::try_from_vec(vec![0; 7], 2, 1),
        Err(SurfaceResourceError::BufferLengthMismatch {
            expected: 8,
            actual: 7
        })
    ));
}

#[test]
fn canvas_from_rgba() {
    let data = vec![255, 0, 0, 255, 0, 255, 0, 255];
    let c = Canvas::from_rgba(&data, 2, 1);
    assert_eq!(c.get_pixel(0, 0), Rgba::new(255, 0, 0, 255));
    assert_eq!(c.get_pixel(1, 0), Rgba::new(0, 255, 0, 255));
}

#[test]
#[should_panic(expected = "does not match")]
fn canvas_from_rgba_wrong_size_panics() {
    let data = vec![0u8; 10];
    let _ = Canvas::from_rgba(&data, 2, 2);
}

#[test]
fn canvas_from_vec() {
    let data = vec![100, 150, 200, 255, 50, 25, 75, 128];
    let c = Canvas::from_vec(data, 2, 1);
    assert_eq!(c.get_pixel(0, 0), Rgba::new(100, 150, 200, 255));
    assert_eq!(c.get_pixel(1, 0), Rgba::new(50, 25, 75, 128));
}

#[test]
fn published_surface_from_owned_canvas_reuses_unique_storage() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(100, 150, 200, 255));
    canvas.set_pixel(1, 0, Rgba::new(50, 25, 75, 128));
    let original_ptr = canvas.as_rgba_bytes().as_ptr();

    let (surface, copied) = PublishedSurface::from_owned_canvas_with_copy_info(canvas, 7, 42);

    assert!(!copied);
    assert_eq!(surface.frame_number(), 7);
    assert_eq!(surface.timestamp_ms(), 42);
    assert_eq!(surface.rgba_bytes().as_ptr(), original_ptr);
}

#[test]
fn published_surface_from_owned_canvas_reuses_shared_storage_without_copy() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    canvas.set_pixel(1, 0, Rgba::new(40, 50, 60, 255));
    let original = [10, 20, 30, 255, 40, 50, 60, 255];
    let original_ptr = canvas.as_rgba_bytes().as_ptr();
    let mut shared = canvas.clone();

    let (surface, copied) = PublishedSurface::from_owned_canvas_with_copy_info(canvas, 8, 84);

    assert!(!copied);
    assert_eq!(surface.rgba_bytes().as_ptr(), original_ptr);
    assert_eq!(surface.rgba_bytes()[..8], original);

    shared.set_pixel(0, 0, Rgba::new(90, 80, 70, 255));

    assert_eq!(surface.rgba_bytes()[..8], original);
    assert_eq!(
        shared.as_rgba_bytes()[..8],
        [90, 80, 70, 255, 40, 50, 60, 255]
    );
}

#[test]
fn canvas_copy_from_surface_reuses_matching_unique_storage() {
    let mut source = Canvas::new(2, 1);
    source.fill(Rgba::new(90, 80, 70, 255));
    let surface = PublishedSurface::from_owned_canvas(source, 7, 42);
    let mut target = Canvas::new(2, 1);
    let target_storage = target.as_rgba_bytes().as_ptr();

    target
        .try_copy_from_published_surface(&surface)
        .expect("matching surface copy should reuse admitted storage");

    assert_eq!(target.as_rgba_bytes().as_ptr(), target_storage);
    assert_eq!(target.get_pixel(1, 0), Rgba::new(90, 80, 70, 255));
}

#[test]
fn canvas_copy_from_surface_preserves_shared_readers() {
    let mut source = Canvas::new(2, 1);
    source.fill(Rgba::new(90, 80, 70, 255));
    let surface = PublishedSurface::from_owned_canvas(source, 7, 42);
    let mut target = Canvas::new(2, 1);
    target.fill(Rgba::new(1, 2, 3, 255));
    let published = target.clone();
    let published_storage = published.as_rgba_bytes().as_ptr();

    target
        .try_copy_from_published_surface(&surface)
        .expect("shared target replacement should allocate fallibly");

    assert_eq!(published.as_rgba_bytes().as_ptr(), published_storage);
    assert_eq!(published.get_pixel(1, 0), Rgba::new(1, 2, 3, 255));
    assert_ne!(target.as_rgba_bytes().as_ptr(), published_storage);
    assert_eq!(target.get_pixel(1, 0), Rgba::new(90, 80, 70, 255));
}

#[test]
fn render_surface_pool_uses_three_slots_and_reclaims_released_surface() {
    let descriptor = SurfaceDescriptor::rgba8888(4, 2);
    let mut pool = RenderSurfacePool::new(descriptor);

    let mut lease_a = pool.dequeue().expect("first lease");
    lease_a.canvas_mut().fill(Rgba::new(1, 2, 3, 255));
    let surface_a = lease_a.submit(1, 10);

    let mut lease_b = pool.dequeue().expect("second lease");
    lease_b.canvas_mut().fill(Rgba::new(4, 5, 6, 255));
    let surface_b = lease_b.submit(2, 20);

    let mut lease_c = pool.dequeue().expect("third lease");
    lease_c.canvas_mut().fill(Rgba::new(7, 8, 9, 255));
    let surface_c = lease_c.submit(3, 30);

    assert_eq!(
        pool.slot_states(),
        vec![
            SurfaceState::Published,
            SurfaceState::Published,
            SurfaceState::Published
        ]
    );

    drop(surface_b);

    assert_eq!(
        pool.slot_states(),
        vec![
            SurfaceState::Published,
            SurfaceState::Free,
            SurfaceState::Published
        ]
    );

    let mut lease_d = pool.dequeue().expect("released slot should be reclaimed");
    lease_d.canvas_mut().fill(Rgba::new(9, 8, 7, 255));
    let surface_d = lease_d.submit(4, 40);

    assert_eq!(surface_a.generation(), 1);
    assert_eq!(surface_c.generation(), 1);
    assert_eq!(surface_d.generation(), 2);
    assert_eq!(surface_d.frame_number(), 4);
    assert_eq!(surface_d.timestamp_ms(), 40);
    assert_eq!(surface_d.rgba_bytes()[..4], [9, 8, 7, 255]);
}

#[test]
fn render_surface_pool_fallible_constructor_rejects_overflowing_descriptor() {
    let result = RenderSurfacePool::try_new(SurfaceDescriptor::rgba8888(u32::MAX, u32::MAX));

    assert!(matches!(
        result,
        Err(SurfaceResourceError::ByteLengthOverflow {
            width: u32::MAX,
            height: u32::MAX,
        })
    ));
}

#[cfg(target_pointer_width = "64")]
#[test]
fn render_surface_pool_rejects_aggregate_byte_overflow_before_allocation() {
    let descriptor = SurfaceDescriptor::rgba8888(u32::MAX, 1);
    let surface_bytes = descriptor
        .try_non_empty_byte_len()
        .expect("single surface fits 64-bit address space");
    let slot_count = usize::MAX / surface_bytes + 1;
    let result = RenderSurfacePool::try_with_slot_count(descriptor, slot_count);

    assert!(matches!(
        result,
        Err(SurfaceResourceError::PoolByteLengthOverflow {
            width: u32::MAX,
            height: 1,
            slot_count: actual,
        }) if actual == slot_count
    ));
}

#[test]
fn render_surface_pool_failed_growth_leaves_existing_slots_unchanged() {
    let descriptor = SurfaceDescriptor::rgba8888(1, 1);
    let mut pool = RenderSurfacePool::try_with_slot_count(descriptor, 1)
        .expect("one-pixel pool should allocate");
    let original_cap = pool.max_slots();

    let result = pool.try_ensure_slot_count(usize::MAX);

    assert!(matches!(
        result,
        Err(SurfaceResourceError::PoolByteLengthOverflow { .. })
    ));
    assert_eq!(pool.slot_count(), 1);
    assert_eq!(pool.max_slots(), original_cap);
}

#[test]
fn render_surface_pool_fallible_dequeue_preserves_normal_reuse() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 1);
    let mut pool =
        RenderSurfacePool::try_with_slot_count(descriptor, 1).expect("small pool should allocate");
    let lease = pool
        .try_dequeue()
        .expect("dequeue should not fail")
        .expect("slot should be available");
    let surface = lease.submit(1, 10);
    drop(surface);

    let lease = pool
        .try_dequeue()
        .expect("reclaimed dequeue should not fail")
        .expect("reclaimed slot should be available");
    assert_eq!(lease.descriptor(), descriptor);
}

#[test]
fn render_surface_pool_fallible_dequeue_grows_without_panicking() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 1);
    let mut pool = RenderSurfacePool::try_with_slot_count_and_cap(descriptor, 1, 2)
        .expect("small pool should allocate");
    let first = pool
        .try_dequeue()
        .expect("first dequeue should not fail")
        .expect("first slot should be available")
        .submit(1, 10);

    {
        let second = pool
            .try_dequeue()
            .expect("growth should not fail")
            .expect("grown slot should be available");
        assert_eq!(second.descriptor(), descriptor);
    }
    assert_eq!(pool.slot_count(), 2);
    assert_eq!(pool.grown_slots(), 1);
    drop(first);
}

#[test]
fn render_surface_pool_lazy_slots_defer_high_resolution_storage() {
    let descriptor = SurfaceDescriptor::rgba8888(7_680, 4_320);
    let pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(descriptor, 8, 16)
        .expect("addressable lazy pool should construct");

    assert_eq!(pool.slot_count(), 8);
    assert_eq!(pool.materialized_slot_count(), 0);
    assert_eq!(pool.max_slots(), 16);
}

#[test]
fn render_surface_pool_lazy_slots_materialize_only_on_dequeue() {
    let descriptor = SurfaceDescriptor::rgba8888(3, 2);
    let mut pool = RenderSurfacePool::try_with_lazy_slot_count(descriptor, 3)
        .expect("small lazy pool should construct");

    let lease = pool
        .try_dequeue()
        .expect("lazy allocation should succeed")
        .expect("lazy slot should be available");
    assert_eq!(lease.descriptor(), descriptor);
    let surface = lease.submit(1, 10);

    assert_eq!(pool.materialized_slot_count(), 1);
    assert_eq!(surface.rgba_len(), 24);
}

#[test]
fn render_surface_pool_lazy_slots_reuse_materialized_storage_first() {
    let descriptor = SurfaceDescriptor::rgba8888(3, 2);
    let mut pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(descriptor, 8, 16)
        .expect("small lazy pool should construct");

    for frame_number in 1..=16 {
        let surface = pool
            .try_dequeue()
            .expect("lazy allocation should succeed")
            .expect("lazy slot should be available")
            .submit(frame_number, frame_number);
        drop(surface);
    }

    assert_eq!(pool.slot_count(), 8);
    assert_eq!(pool.materialized_slot_count(), 1);
    assert_eq!(pool.grown_slots(), 0);
}

#[test]
fn render_surface_pool_lazy_growth_defers_new_slot_storage() {
    let descriptor = SurfaceDescriptor::rgba8888(3, 2);
    let mut pool = RenderSurfacePool::try_with_lazy_slot_count_and_cap(descriptor, 1, 4)
        .expect("small lazy pool should construct");

    pool.try_ensure_slot_count(4)
        .expect("lazy metadata growth should succeed");

    assert_eq!(pool.slot_count(), 4);
    assert_eq!(pool.materialized_slot_count(), 0);

    let surface = pool
        .try_dequeue()
        .expect("lazy allocation should succeed")
        .expect("grown lazy slot should be available")
        .submit(1, 10);

    assert_eq!(pool.materialized_slot_count(), 1);
    assert_eq!(surface.rgba_len(), 24);
}

#[test]
fn render_surface_pool_failed_lazy_growth_preserves_materialization() {
    let descriptor = SurfaceDescriptor::rgba8888(1, 1);
    let mut pool = RenderSurfacePool::try_with_lazy_slot_count(descriptor, 1)
        .expect("one-pixel lazy pool should construct");
    let original_cap = pool.max_slots();

    let result = pool.try_ensure_slot_count(usize::MAX);

    assert!(matches!(
        result,
        Err(SurfaceResourceError::PoolByteLengthOverflow { .. })
    ));
    assert_eq!(pool.slot_count(), 1);
    assert_eq!(pool.materialized_slot_count(), 0);
    assert_eq!(pool.max_slots(), original_cap);
}

#[test]
fn render_surface_pool_lazy_slots_reject_aggregate_overflow() {
    let descriptor = SurfaceDescriptor::rgba8888(1, 1);
    let slot_count = usize::MAX / 4 + 1;
    let result = RenderSurfacePool::try_with_lazy_slot_count(descriptor, slot_count);

    assert!(matches!(
        result,
        Err(SurfaceResourceError::PoolByteLengthOverflow {
            width: 1,
            height: 1,
            slot_count: failed_slot_count,
        }) if failed_slot_count == slot_count
    ));
}

#[test]
fn render_surface_pool_can_grow_without_disturbing_published_slots() {
    let descriptor = SurfaceDescriptor::rgba8888(4, 2);
    let mut pool = RenderSurfacePool::with_slot_count(descriptor, 2);

    let mut lease_a = pool.dequeue().expect("first lease");
    lease_a.canvas_mut().fill(Rgba::new(1, 2, 3, 255));
    let surface_a = lease_a.submit(1, 10);

    let mut lease_b = pool.dequeue().expect("second lease");
    lease_b.canvas_mut().fill(Rgba::new(4, 5, 6, 255));
    let surface_b = lease_b.submit(2, 20);

    pool.ensure_slot_count(4);

    assert_eq!(pool.slot_count(), 4);
    assert_eq!(
        pool.slot_states(),
        vec![
            SurfaceState::Published,
            SurfaceState::Published,
            SurfaceState::Free,
            SurfaceState::Free
        ]
    );

    let mut lease_c = pool
        .dequeue()
        .expect("grown pool should provide a third slot");
    lease_c.canvas_mut().fill(Rgba::new(7, 8, 9, 255));
    let surface_c = lease_c.submit(3, 30);

    assert_eq!(surface_a.generation(), 1);
    assert_eq!(surface_b.generation(), 1);
    assert_eq!(surface_c.generation(), 1);
}

#[test]
fn render_surface_pool_rebinds_published_slots_under_retention_pressure() {
    // Pin the pool at a single slot so dequeue must fall back to rebinding
    // the still-shared Published slot instead of growing.
    let descriptor = SurfaceDescriptor::rgba8888(2, 1);
    let mut pool = RenderSurfacePool::with_slot_count_and_cap(descriptor, 1, 1);

    let mut lease_a = pool.dequeue().expect("first lease");
    lease_a
        .canvas_mut()
        .set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    lease_a
        .canvas_mut()
        .set_pixel(1, 0, Rgba::new(40, 50, 60, 255));
    let surface_a = lease_a.submit(1, 10);

    let mut lease_b = pool
        .dequeue()
        .expect("retained published surface should not block slot reuse");
    lease_b
        .canvas_mut()
        .set_pixel(0, 0, Rgba::new(70, 80, 90, 255));
    lease_b
        .canvas_mut()
        .set_pixel(1, 0, Rgba::new(100, 110, 120, 255));
    let surface_b = lease_b.submit(2, 20);

    assert_eq!(surface_a.generation(), 1);
    assert_eq!(surface_b.generation(), 2);
    assert_eq!(
        surface_a.rgba_bytes()[..8],
        [10, 20, 30, 255, 40, 50, 60, 255]
    );
    assert_eq!(
        surface_b.rgba_bytes()[..8],
        [70, 80, 90, 255, 100, 110, 120, 255]
    );
    assert_eq!(pool.slot_states(), vec![SurfaceState::Published]);
}

#[test]
fn render_surface_pool_submit_uses_actual_canvas_dimensions() {
    let descriptor = SurfaceDescriptor::rgba8888(4, 4);
    let mut pool = RenderSurfacePool::with_slot_count(descriptor, 1);

    let mut lease = pool.dequeue().expect("lease");
    *lease.canvas_mut() = Canvas::new(2, 2);
    let surface = lease.submit(1, 10);

    assert_eq!(surface.width(), 2);
    assert_eq!(surface.height(), 2);
    assert_eq!(surface.rgba_len(), 2 * 2 * BYTES_PER_PIXEL);
}

#[test]
fn render_surface_pool_recreates_mismatched_canvas_on_next_dequeue() {
    let descriptor = SurfaceDescriptor::rgba8888(4, 4);
    let mut pool = RenderSurfacePool::with_slot_count(descriptor, 1);

    let mut lease = pool.dequeue().expect("lease");
    *lease.canvas_mut() = Canvas::new(2, 2);
    let surface = lease.submit(1, 10);
    drop(surface);

    let mut lease = pool.dequeue().expect("reclaimed lease");
    assert_eq!(lease.canvas_mut().width(), 4);
    assert_eq!(lease.canvas_mut().height(), 4);
}

#[test]
fn published_surface_storage_identity_survives_metadata_updates() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    let surface = PublishedSurface::from_owned_canvas(canvas, 1, 10);
    let updated = surface.with_frame_metadata(2, 20);

    assert_eq!(surface.storage_identity(), updated.storage_identity());
}

#[test]
fn published_surface_content_digest_is_shared_across_metadata_clones() {
    let mut canvas = Canvas::new(2, 1);
    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));
    let surface = PublishedSurface::from_owned_canvas(canvas, 1, 10);
    let updated = surface.with_frame_metadata(2, 20);

    assert_eq!(surface.cached_content_digest(), None);
    let digest = updated.content_digest();
    assert_eq!(surface.cached_content_digest(), Some(digest));
    assert_eq!(updated.cached_content_digest(), Some(digest));
    assert_eq!(surface.content_digest(), digest);
}

#[test]
fn published_surface_storage_identity_distinguishes_new_owned_surfaces() {
    let first = PublishedSurface::from_owned_canvas(Canvas::new(2, 1), 1, 10);
    let second = PublishedSurface::from_owned_canvas(Canvas::new(2, 1), 2, 20);

    assert_ne!(first.storage_identity(), second.storage_identity());
}

#[test]
fn empty_published_surfaces_share_a_stable_storage_identity() {
    assert_eq!(
        PublishedSurface::empty().storage_identity(),
        PublishedSurface::empty().storage_identity()
    );
}

#[test]
fn canvas_storage_identity_changes_when_shared_canvas_is_mutated() {
    let canvas = Canvas::new(2, 1);
    let original_identity = canvas.storage_identity();
    let mut shared = canvas.clone();

    assert_eq!(original_identity, shared.storage_identity());
    shared.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));

    assert_ne!(original_identity, shared.storage_identity());
    assert_eq!(original_identity, canvas.storage_identity());
}

#[test]
fn canvas_storage_identity_changes_when_unique_canvas_is_mutated() {
    let mut canvas = Canvas::new(2, 1);
    let original_identity = canvas.storage_identity();

    canvas.set_pixel(0, 0, Rgba::new(10, 20, 30, 255));

    assert_ne!(original_identity, canvas.storage_identity());
}

#[test]
fn render_surface_pool_slot_counts_match_visible_states() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 2);
    let mut pool = RenderSurfacePool::with_slot_count(descriptor, 3);

    let lease_a = pool.dequeue().expect("first lease");
    let _surface_a = lease_a.submit(1, 10);

    let _lease_b = pool.dequeue().expect("second lease");

    let counts = pool.slot_counts();
    assert_eq!(counts.published, 1);
    assert_eq!(counts.dequeued, 1);
    assert_eq!(counts.free, 1);
}

#[test]
fn render_surface_pool_grows_when_all_slots_are_still_shared() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 2);
    let mut pool = RenderSurfacePool::with_slot_count_and_cap(descriptor, 2, 4);

    // Publish both initial slots and hold their surfaces so reclaim_published_slots
    // cannot free them on the next dequeue.
    let lease_a = pool.dequeue().expect("first lease");
    let _surface_a = lease_a.submit(1, 10);
    let lease_b = pool.dequeue().expect("second lease");
    let _surface_b = lease_b.submit(2, 20);

    assert_eq!(pool.slot_count(), 2);
    assert_eq!(pool.grown_slots(), 0);

    // Next dequeue should grow the pool rather than realloc a shared slot.
    let _lease_c = pool.dequeue().expect("grown lease");
    assert_eq!(pool.slot_count(), 3);
    assert_eq!(pool.grown_slots(), 1);
    assert_eq!(pool.saturation_reallocs(), 0);
}

#[test]
fn render_surface_pool_falls_back_to_realloc_when_at_cap() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 2);
    let mut pool = RenderSurfacePool::with_slot_count_and_cap(descriptor, 1, 1);

    // Pin the only slot's canvas downstream so reclaim cannot free it.
    let lease = pool.dequeue().expect("first lease");
    let _surface = lease.submit(1, 10);

    assert_eq!(pool.slot_count(), 1);
    assert_eq!(pool.max_slots(), 1);

    // At cap and still shared — the realloc path must engage.
    let _lease = pool.dequeue().expect("realloc lease");
    assert_eq!(pool.slot_count(), 1);
    assert_eq!(pool.grown_slots(), 0);
    assert_eq!(pool.saturation_reallocs(), 1);
}

#[test]
fn bounded_render_surface_pool_reports_pressure_without_reallocating() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 2);
    let mut pool = RenderSurfacePool::with_slot_count_and_cap(descriptor, 3, 3);
    let mut pinned = Vec::new();
    for frame_number in 1..=3 {
        let lease = pool
            .try_dequeue_bounded()
            .expect("bounded dequeue remains fallible")
            .expect("configured slot is available");
        pinned.push(lease.submit(frame_number, 10));
    }

    assert!(
        pool.try_dequeue_bounded()
            .expect("pressure is not an allocation error")
            .is_none()
    );
    assert_eq!(pool.slot_count(), 3);
    assert_eq!(pool.grown_slots(), 0);
    assert_eq!(pool.saturation_reallocs(), 0);

    drop(pinned.pop());
    assert!(
        pool.try_dequeue_bounded()
            .expect("released storage is reusable")
            .is_some()
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn bounded_descriptor_replacement_drops_free_backing_before_allocation() {
    let original = SurfaceDescriptor::rgba8888(1, 1);
    let mut pool = RenderSurfacePool::with_slot_count_and_cap(original, 1, 1);
    let drops = Arc::new(AtomicUsize::new(0));
    let mut lease = pool.dequeue().expect("initial slot should be available");
    lease
        .canvas_mut()
        .set_resource_owner(Arc::new(CountingSurfaceOwner(Arc::clone(&drops))));
    lease.release();

    let over_isize_capacity = SurfaceDescriptor::rgba8888(u32::MAX, 536_870_913);
    let result = pool.try_dequeue_bounded_for_descriptor(over_isize_capacity);

    assert!(matches!(
        result,
        Err(SurfaceResourceError::AllocationFailed {
            width: u32::MAX,
            height: 536_870_913,
            ..
        })
    ));
    assert_eq!(drops.load(Ordering::Acquire), 1);
    assert_eq!(pool.descriptor(), original);
    assert_eq!(pool.materialized_slot_count(), 0);
    assert!(
        pool.try_dequeue_bounded_for_descriptor(original)
            .expect("vacated slot can be materialized again")
            .is_some()
    );
}

#[test]
fn render_surface_pool_prefers_reclaimed_slots_over_growing() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 2);
    let mut pool = RenderSurfacePool::with_slot_count_and_cap(descriptor, 1, 4);

    // Publish then drop, so the slot is reclaimable on next dequeue.
    let lease = pool.dequeue().expect("first lease");
    let surface = lease.submit(1, 10);
    drop(surface);

    let _lease = pool.dequeue().expect("reclaimed lease");
    assert_eq!(pool.slot_count(), 1);
    assert_eq!(pool.grown_slots(), 0);
    assert_eq!(pool.saturation_reallocs(), 0);
}

#[test]
fn render_surface_lease_release_returns_slot_without_publish() {
    let descriptor = SurfaceDescriptor::rgba8888(2, 2);
    let mut pool = RenderSurfacePool::with_slot_count(descriptor, 1);

    let mut lease = pool.dequeue().expect("lease");
    lease.canvas_mut().fill(Rgba::new(1, 1, 1, 255));
    lease.release();

    assert_eq!(pool.slot_states(), vec![SurfaceState::Free]);
    assert!(pool.dequeue().is_some());
}

#[test]
#[should_panic(expected = "does not match")]
fn canvas_from_vec_wrong_size_panics() {
    let data = vec![0u8; 5];
    let _ = Canvas::from_vec(data, 1, 1);
}

// ── Canvas Pixel Access ────────────────────────────────────────────────────

#[test]
fn canvas_get_set_pixel() {
    let mut c = Canvas::new(10, 10);
    let red = Rgba::new(255, 0, 0, 255);
    c.set_pixel(5, 5, red);
    assert_eq!(c.get_pixel(5, 5), red);
}

#[test]
fn canvas_get_pixel_oob_returns_black() {
    let c = Canvas::new(10, 10);
    assert_eq!(c.get_pixel(10, 0), Rgba::BLACK);
    assert_eq!(c.get_pixel(0, 10), Rgba::BLACK);
    assert_eq!(c.get_pixel(100, 100), Rgba::BLACK);
}

#[test]
fn canvas_set_pixel_oob_is_noop() {
    let mut c = Canvas::new(2, 2);
    let red = Rgba::new(255, 0, 0, 255);
    c.set_pixel(5, 5, red); // should not panic
    // All pixels remain opaque black
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 0, 255]);
    }
}

#[test]
fn canvas_fill() {
    let mut c = Canvas::new(4, 4);
    let blue = Rgba::new(0, 0, 255, 200);
    c.fill(blue);
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 255, 200]);
    }
}

#[test]
fn canvas_clear() {
    let mut c = Canvas::new(4, 4);
    c.fill(Rgba::WHITE);
    c.clear();
    for pixel in c.pixels() {
        assert_eq!(pixel, [0, 0, 0, 255]);
    }
}

#[test]
fn canvas_pixels_len() {
    let c = Canvas::new(8, 6);
    assert_eq!(c.pixels().len(), 48);
}

#[test]
fn canvas_as_rgba_bytes_mut() {
    let mut c = Canvas::new(2, 1);
    let bytes = c.as_rgba_bytes_mut();
    bytes[0] = 255; // R of first pixel
    assert_eq!(c.get_pixel(0, 0).r, 255);
}

#[test]
fn canvas_debug_format() {
    let c = Canvas::new(10, 20);
    let debug = format!("{c:?}");
    assert!(debug.contains("Canvas"));
    assert!(debug.contains("10"));
    assert!(debug.contains("20"));
}

// ── Canvas Sampling ────────────────────────────────────────────────────────

#[test]
fn sample_nearest_corners() {
    let mut c = Canvas::new(4, 4);
    let red = Rgba::new(255, 0, 0, 255);
    let green = Rgba::new(0, 255, 0, 255);
    c.set_pixel(0, 0, red);
    c.set_pixel(3, 3, green);

    assert_eq!(c.sample_nearest(0.0, 0.0), red);
    assert_eq!(c.sample_nearest(1.0, 1.0), green);
}

#[test]
fn sample_bilinear_midpoint() {
    let mut c = Canvas::new(2, 1);
    c.set_pixel(0, 0, Rgba::new(0, 0, 0, 255));
    c.set_pixel(1, 0, Rgba::new(200, 200, 200, 255));

    let mid = c.sample_bilinear(0.5, 0.0);
    let expected = linear_to_srgb(srgb_to_linear(200.0 / 255.0) * 0.5) * 255.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions,
        reason = "test helper: expected is a known-positive u8-range value"
    )]
    let expected_u8 = expected.round() as u8;
    assert_eq!(
        mid.r, expected_u8,
        "bilinear midpoint should interpolate in linear light, got {}",
        mid.r
    );
}

#[test]
fn sample_area_uniform() {
    let mut c = Canvas::new(10, 10);
    c.fill(Rgba::new(100, 100, 100, 255));

    let sampled = c.sample_area(0.5, 0.5, 2.0);
    assert_eq!(sampled, Rgba::new(100, 100, 100, 255));
}

#[test]
fn sample_dispatch() {
    let c = Canvas::new(4, 4);
    // Just verify dispatch works without panicking
    let _ = c.sample(0.5, 0.5, SamplingMethod::Nearest);
    let _ = c.sample(0.5, 0.5, SamplingMethod::Bilinear);
    let _ = c.sample(0.5, 0.5, SamplingMethod::Area { radius: 1.0 });
}

#[test]
fn sample_clamps_oob_coords() {
    let c = Canvas::new(4, 4);
    // Should not panic, coords are clamped
    let _ = c.sample(-1.0, -1.0, SamplingMethod::Nearest);
    let _ = c.sample(2.0, 2.0, SamplingMethod::Bilinear);
}

// ── BlendMode ──────────────────────────────────────────────────────────────

#[test]
fn blend_normal_full_opacity() {
    let dst = [0.2, 0.3, 0.4, 1.0];
    let src = [0.8, 0.7, 0.6, 1.0];
    let result = BlendMode::Normal.blend(dst, src, 1.0);
    // Normal at full opacity: result = src
    assert!((result[0] - 0.8).abs() < 0.01);
    assert!((result[1] - 0.7).abs() < 0.01);
    assert!((result[2] - 0.6).abs() < 0.01);
}

#[test]
fn blend_normal_zero_opacity() {
    let dst = [0.2, 0.3, 0.4, 1.0];
    let src = [0.8, 0.7, 0.6, 1.0];
    let result = BlendMode::Normal.blend(dst, src, 0.0);
    // Zero opacity: result = dst
    assert!((result[0] - 0.2).abs() < 0.01);
    assert!((result[1] - 0.3).abs() < 0.01);
    assert!((result[2] - 0.4).abs() < 0.01);
}

#[test]
fn blend_add_clamps() {
    let dst = [0.8, 0.9, 0.7, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];
    let result = BlendMode::Add.blend(dst, src, 1.0);
    // Add: clamped to 1.0
    assert!(result[0] <= 1.0);
    assert!(result[1] <= 1.0);
}

#[test]
fn blend_multiply_darkens() {
    let dst = [0.8, 0.6, 0.4, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];
    let result = BlendMode::Multiply.blend(dst, src, 1.0);
    // Multiply always darkens (result <= min(dst, src) when both < 1)
    assert!(result[0] <= 0.5);
    assert!(result[1] <= 0.5);
}

#[test]
fn blend_screen_brightens() {
    let dst = [0.3, 0.4, 0.5, 1.0];
    let src = [0.3, 0.4, 0.5, 1.0];
    let result = BlendMode::Screen.blend(dst, src, 1.0);
    // Screen always brightens
    assert!(result[0] > dst[0]);
    assert!(result[1] > dst[1]);
}

#[test]
fn blend_overlay_contrast() {
    // Overlay: multiply when dst < 0.5, screen when dst > 0.5
    let dark_dst = [0.2, 0.2, 0.2, 1.0];
    let light_dst = [0.8, 0.8, 0.8, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];

    let dark_result = BlendMode::Overlay.blend(dark_dst, src, 1.0);
    let light_result = BlendMode::Overlay.blend(light_dst, src, 1.0);

    // Dark gets darker, light gets lighter
    assert!(dark_result[0] < 0.5);
    assert!(light_result[0] > 0.5);
}

#[test]
fn blend_soft_light() {
    let dst = [0.5, 0.5, 0.5, 1.0];
    let src = [0.3, 0.7, 0.5, 1.0];
    let result = BlendMode::SoftLight.blend(dst, src, 1.0);
    // Soft light should produce values in [0, 1]
    for ch in &result[..3] {
        assert!(*ch >= 0.0 && *ch <= 1.0);
    }
}

#[test]
fn blend_color_dodge() {
    let dst = [0.4, 0.4, 0.4, 1.0];
    let src = [0.5, 0.5, 0.5, 1.0];
    let result = BlendMode::ColorDodge.blend(dst, src, 1.0);
    // Color dodge brightens: dst / (1 - src) = 0.4 / 0.5 = 0.8
    assert!((result[0] - 0.8).abs() < 0.01);
}

#[test]
fn blend_color_dodge_src_one_clamps() {
    let dst = [0.5, 0.5, 0.5, 1.0];
    let src = [1.0, 1.0, 1.0, 1.0];
    let result = BlendMode::ColorDodge.blend(dst, src, 1.0);
    // src=1.0 -> result=1.0 (clamped)
    assert!((result[0] - 1.0).abs() < 0.01);
}

#[test]
fn blend_difference() {
    let dst = [0.8, 0.3, 0.5, 1.0];
    let src = [0.3, 0.8, 0.5, 1.0];
    let result = BlendMode::Difference.blend(dst, src, 1.0);
    assert!((result[0] - 0.5).abs() < 0.01);
    assert!((result[1] - 0.5).abs() < 0.01);
    assert!((result[2]).abs() < 0.01); // |0.5 - 0.5| = 0
}

#[test]
fn blend_alpha_compositing() {
    let dst = [0.0, 0.0, 0.0, 1.0];
    let src = [1.0, 1.0, 1.0, 0.5];
    let result = BlendMode::Normal.blend(dst, src, 1.0);
    // dst_alpha + src_alpha - dst_alpha * src_alpha = 1.0 + 0.5 - 0.5 = 1.0
    assert!((result[3] - 1.0).abs() < 0.01);
}

#[test]
fn blend_mode_default_is_normal() {
    assert_eq!(BlendMode::default(), BlendMode::Normal);
}

#[test]
fn blend_mode_serde_roundtrip() {
    let modes = [
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::ColorDodge,
        BlendMode::Difference,
    ];
    for mode in &modes {
        let json = serde_json::to_string(mode).expect("serialize");
        let back: BlendMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*mode, back, "roundtrip failed for {json}");
    }
}

#[test]
fn blend_mode_serde_snake_case() {
    let json = serde_json::to_string(&BlendMode::SoftLight).expect("serialize");
    assert_eq!(json, "\"soft_light\"");

    let json = serde_json::to_string(&BlendMode::ColorDodge).expect("serialize");
    assert_eq!(json, "\"color_dodge\"");
}

// ── Blend delegation ───────────────────────────────────────────────────────

/// `BlendMode::blend` takes `(dst, src)` while the kernel's `blend_over`
/// takes the source as its receiver. Every mode is checked against the
/// kernel called with the orientation spelled out, so a swapped
/// delegation fails here instead of silently recoloring frames.
#[test]
fn blend_delegation_preserves_source_over_destination_orientation() {
    let dst = [0.25_f32, 0.6, 0.9, 0.8];
    let src = [0.7_f32, 0.2, 0.45, 0.6];
    let modes = [
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::SoftLight,
        BlendMode::ColorDodge,
        BlendMode::Difference,
    ];

    for opacity in [0.0_f32, 0.35, 1.0] {
        for mode in modes {
            let source = LinearRgba::new(src[0], src[1], src[2], src[3]);
            let destination = LinearRgba::new(dst[0], dst[1], dst[2], dst[3]);
            let expected = source.blend_over(destination, mode.pixel_mode(), opacity);
            assert_eq!(
                mode.blend(dst, src, opacity),
                [expected.r, expected.g, expected.b, expected.a],
                "{mode:?} at opacity {opacity}"
            );
        }
    }
}

/// Asymmetric modes prove the orientation empirically: blending a bright
/// source over a dark destination must not equal the reverse.
#[test]
fn asymmetric_blend_modes_are_orientation_sensitive() {
    let dark = [0.1_f32, 0.1, 0.1, 1.0];
    let bright = [0.9_f32, 0.8, 0.7, 1.0];
    for mode in [
        BlendMode::ColorDodge,
        BlendMode::Overlay,
        BlendMode::SoftLight,
    ] {
        assert_ne!(
            mode.blend(dark, bright, 1.0),
            mode.blend(bright, dark, 1.0),
            "{mode:?} lost its orientation"
        );
    }
}

#[test]
fn additive_blend_saturates_both_contributing_channels() {
    let result = BlendMode::Add.blend([0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0], 1.0);
    assert!((result[0] - 1.0).abs() < 0.01);
    assert!((result[2] - 1.0).abs() < 0.01);
}

// ── ColorFormat ────────────────────────────────────────────────────────────

#[test]
fn color_format_default_is_rgb() {
    assert_eq!(ColorFormat::default(), ColorFormat::Rgb);
}

#[test]
fn color_format_serde() {
    let json = serde_json::to_string(&ColorFormat::RgbW16).expect("serialize");
    assert_eq!(json, "\"rgb_w16\"");
    let back: ColorFormat = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, ColorFormat::RgbW16);
}

// ── SamplingMethod ─────────────────────────────────────────────────────────

#[test]
fn sampling_method_default_is_bilinear() {
    assert_eq!(SamplingMethod::default(), SamplingMethod::Bilinear);
}

#[test]
fn sampling_method_serde() {
    let area = SamplingMethod::Area { radius: 5.0 };
    let json = serde_json::to_string(&area).expect("serialize");
    let back: SamplingMethod = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, area);
}

#[derive(Debug)]
struct CountingSurfaceOwner(Arc<AtomicUsize>);

impl Drop for CountingSurfaceOwner {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}

#[test]
fn owner_backed_canvas_into_vec_returns_a_detached_copy() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut canvas = Canvas::new(2, 2);
    canvas.set_resource_owner(Arc::new(CountingSurfaceOwner(Arc::clone(&drops))));

    let (pixels, copied) = canvas.into_rgba_bytes_with_copy_info();

    assert!(copied);
    assert_eq!(pixels.len(), 16);
    assert_eq!(drops.load(Ordering::Acquire), 1);
}

#[test]
fn owner_backed_shared_canvas_detaches_before_mutation() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut canvas = Canvas::new(2, 2);
    canvas.set_resource_owner(Arc::new(CountingSurfaceOwner(Arc::clone(&drops))));
    let surface = PublishedSurface::from_owned_canvas(canvas, 0, 0);
    let mut alias = Canvas::from_published_surface(&surface);

    alias.as_rgba_bytes_mut()[0] = 77;
    assert_eq!(surface.rgba_bytes()[0], 0);
    assert_eq!(drops.load(Ordering::Acquire), 0);
    drop(surface);
    assert_eq!(drops.load(Ordering::Acquire), 1);
    assert_eq!(alias.as_rgba_bytes()[0], 77);
}

#[test]
fn published_canvas_alias_retains_owner_until_the_alias_drops() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut canvas = Canvas::new(2, 2);
    canvas.set_resource_owner(Arc::new(CountingSurfaceOwner(Arc::clone(&drops))));
    let surface = PublishedSurface::from_owned_canvas(canvas, 0, 0);
    let alias = Canvas::from_published_surface(&surface);

    drop(surface);
    assert_eq!(drops.load(Ordering::Acquire), 0);
    drop(alias);
    assert_eq!(drops.load(Ordering::Acquire), 1);
}
