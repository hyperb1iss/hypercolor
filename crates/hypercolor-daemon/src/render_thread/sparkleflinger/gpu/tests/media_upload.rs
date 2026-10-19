use super::super::source::GpuSourceFrame;
use super::*;
use crate::render_thread::producer_queue::{ProducerFrameState, ProducerQueue};

#[test]
fn gpu_media_upload_reuses_source_size_texture_ring() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let source = MediaTextureSourceKey::for_test(7);
    let canvas = solid_canvas_with_size(4, 4, Rgba::new(32, 96, 160, 255));
    let Some(frame) = compositor.upload_media_canvas_frame(source, &canvas) else {
        panic!("media upload should return a GPU texture frame");
    };
    assert_eq!(frame.width, 4);
    assert_eq!(frame.height, 4);

    let key = MediaUploadTextureKey {
        source,
        width: 4,
        height: 4,
    };
    let pool = compositor
        .media_texture_pools
        .get(&key)
        .expect("media upload should retain a source-size texture pool");
    assert_eq!(pool.textures.len(), 1);

    let mut canvas = canvas;
    let mut blue = 160_u8;
    for _ in 1..(MEDIA_UPLOAD_TEXTURE_RING_LEN * 2) {
        blue = blue.wrapping_add(1);
        canvas.fill(Rgba::new(32, 96, blue, 255));
        let Some(frame) = compositor.upload_media_canvas_frame(source, &canvas) else {
            panic!("media upload should return a GPU texture frame");
        };
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 4);
    }

    let pool = compositor
        .media_texture_pools
        .get(&key)
        .expect("media upload should retain a source-size texture pool");
    assert_eq!(pool.textures.len(), MEDIA_UPLOAD_TEXTURE_RING_LEN);
}

#[test]
fn gpu_media_upload_writes_unchanged_canvas_storage_once() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let source = MediaTextureSourceKey::for_test(7);
    let mut canvas = solid_canvas_with_size(4, 4, Rgba::new(32, 96, 160, 255));
    let first = compositor
        .upload_media_canvas_frame(source, &canvas)
        .expect("media upload should return a GPU texture frame");

    // A still image hands the same canvas storage every frame.
    for _ in 0..MEDIA_UPLOAD_TEXTURE_RING_LEN * 2 {
        let held = compositor
            .upload_media_canvas_frame(source, &canvas.clone())
            .expect("an unchanged canvas should keep its uploaded frame");
        assert_eq!(held.storage_id, first.storage_id);
        assert_eq!(held.content_generation, first.content_generation);
    }
    let key = MediaUploadTextureKey {
        source,
        width: 4,
        height: 4,
    };
    assert_eq!(
        compositor
            .media_texture_pools
            .get(&key)
            .expect("media upload should retain a source-size texture pool")
            .textures
            .len(),
        1,
        "an unchanged canvas should not advance the texture ring"
    );

    canvas.fill(Rgba::new(200, 16, 64, 255));
    let changed = compositor
        .upload_media_canvas_frame(source, &canvas)
        .expect("changed pixels should upload a new frame");
    assert_ne!(changed.storage_id, first.storage_id);
    assert!(changed.content_generation > first.content_generation);
}

#[test]
fn gpu_media_upload_keys_distinct_sources_separately() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let first_source = MediaTextureSourceKey::for_test(7);
    let second_source = MediaTextureSourceKey::for_test(8);
    let canvas = solid_canvas_with_size(4, 4, Rgba::new(32, 96, 160, 255));

    let Some(first_frame) = compositor.upload_media_canvas_frame(first_source, &canvas) else {
        panic!("first media source should upload as a GPU texture");
    };
    let Some(second_frame) = compositor.upload_media_canvas_frame(second_source, &canvas) else {
        panic!("second media source should upload as a GPU texture");
    };

    assert_ne!(first_frame.storage_id, second_frame.storage_id);
    assert!(
        compositor
            .media_texture_pools
            .contains_key(&MediaUploadTextureKey {
                source: first_source,
                width: 4,
                height: 4,
            })
    );
    assert!(
        compositor
            .media_texture_pools
            .contains_key(&MediaUploadTextureKey {
                source: second_source,
                width: 4,
                height: 4,
            })
    );
    assert_eq!(compositor.media_texture_pools.len(), 2);
}

#[test]
fn gpu_reused_storage_advances_content_freshness_and_copy_identity() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let source = MediaTextureSourceKey::for_test(7);
    let canvas = solid_canvas_with_size(4, 4, Rgba::new(32, 96, 160, 255));
    let first = compositor
        .upload_media_canvas_frame(source, &canvas)
        .expect("first media upload should return a GPU texture frame");

    let mut queue = ProducerQueue::new();
    assert!(
        queue
            .submit_latest(ProducerFrame::GpuTexture(first.clone()))
            .is_none()
    );
    assert_eq!(
        queue
            .latch_latest()
            .expect("first frame should latch")
            .state,
        ProducerFrameState::Fresh
    );
    assert!(
        queue
            .submit_latest(ProducerFrame::GpuTexture(first.clone()))
            .is_some()
    );
    assert_eq!(
        queue
            .latch_latest()
            .expect("exact duplicate should remain latched")
            .state,
        ProducerFrameState::Retained
    );

    let mut canvas = canvas;
    let mut blue = 160_u8;
    for _ in 1..MEDIA_UPLOAD_TEXTURE_RING_LEN {
        blue = blue.wrapping_add(1);
        canvas.fill(Rgba::new(32, 96, blue, 255));
        compositor
            .upload_media_canvas_frame(source, &canvas)
            .expect("ring fill should return a GPU texture frame");
    }
    canvas.fill(Rgba::new(32, 96, blue.wrapping_add(1), 255));
    let reused = compositor
        .upload_media_canvas_frame(source, &canvas)
        .expect("ring reuse should return a GPU texture frame");

    assert_eq!(reused.storage_id, first.storage_id);
    assert!(reused.content_generation > first.content_generation);
    assert_ne!(
        GpuSourceFrame::Texture(&reused).cached_display_source_copy(),
        GpuSourceFrame::Texture(&first).cached_display_source_copy(),
        "new content in reused storage must invalidate cached GPU copies"
    );
    assert!(
        queue
            .submit_latest(ProducerFrame::GpuTexture(reused.clone()))
            .is_some()
    );
    assert_eq!(
        queue
            .latch_latest()
            .expect("new content should replace the retained frame")
            .state,
        ProducerFrameState::Fresh
    );
    assert!(
        queue
            .submit_latest(ProducerFrame::GpuTexture(reused))
            .is_some()
    );
    assert_eq!(
        queue
            .latch_latest()
            .expect("exact new-content duplicate should remain latched")
            .state,
        ProducerFrameState::Retained
    );
}

#[test]
fn gpu_media_upload_prunes_idle_source_size_texture_pools() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let source = MediaTextureSourceKey::for_test(7);
    let canvas = solid_canvas_with_size(4, 4, Rgba::new(32, 96, 160, 255));

    let Some(_) = compositor.upload_media_canvas_frame(source, &canvas) else {
        panic!("media upload should return a GPU texture frame");
    };
    assert_eq!(compositor.media_texture_pools.len(), 1);

    for _ in 0..=MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES {
        compositor.begin_media_upload_frame();
    }

    assert!(compositor.media_texture_pools.is_empty());
}

#[test]
fn gpu_texture_frame_records_blocked_cpu_materialization() {
    let Some(mut compositor) = super::gpu_test_compositor() else {
        return;
    };
    let source = MediaTextureSourceKey::for_test(7);
    let canvas = solid_canvas_with_size(4, 4, Rgba::new(32, 96, 160, 255));
    let before = crate::render_thread::producer_frame_counts().gpu_cpu_materialization_blocked;

    let Some(frame) = compositor.upload_media_canvas_frame(source, &canvas) else {
        panic!("media upload should return a GPU texture frame");
    };
    let producer_frame = ProducerFrame::GpuTexture(frame);

    assert!(producer_frame.cpu_rgba_bytes().is_none());
    let after = crate::render_thread::producer_frame_counts().gpu_cpu_materialization_blocked;
    assert!(
        after > before,
        "expected blocked materialization counter to increase"
    );
}
