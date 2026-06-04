use super::*;

#[test]
fn gpu_media_upload_reuses_source_size_texture_ring() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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

    for _ in 1..(MEDIA_UPLOAD_TEXTURE_RING_LEN * 2) {
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
fn gpu_media_upload_keys_distinct_sources_separately() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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
fn gpu_media_upload_prunes_idle_source_size_texture_pools() {
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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
    let mut compositor = match GpuSparkleFlinger::new() {
        Ok(compositor) => compositor,
        Err(_) => return,
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
