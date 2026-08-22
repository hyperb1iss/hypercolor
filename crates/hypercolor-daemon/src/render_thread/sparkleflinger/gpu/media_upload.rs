use hypercolor_core::types::canvas::Canvas;

use crate::render_thread::producer_queue::{GpuTextureFrame, GpuTextureFrameOrigin};

use super::super::MediaTextureSourceKey;
use super::source::write_rgba_texture;
use super::telemetry::record_gpu_media_texture_allocation;
use super::telemetry::record_gpu_media_texture_upload;
use super::{GpuCompositorTexture, GpuSparkleFlinger};

pub(super) const MEDIA_UPLOAD_TEXTURE_RING_LEN: usize = 3;
pub(super) const MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct MediaUploadTextureKey {
    pub(super) source: MediaTextureSourceKey,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) struct MediaUploadTexturePool {
    pub(super) textures: Vec<GpuCompositorTexture>,
    pub(super) next_slot: usize,
    pub(super) last_used_epoch: u64,
}

impl MediaUploadTexturePool {
    pub(super) fn new() -> Self {
        Self {
            textures: Vec::with_capacity(MEDIA_UPLOAD_TEXTURE_RING_LEN),
            next_slot: 0,
            last_used_epoch: 0,
        }
    }

    pub(super) fn next_texture(
        &mut self,
        device: &wgpu::Device,
        key: MediaUploadTextureKey,
        media_texture_epoch: u64,
    ) -> &GpuCompositorTexture {
        self.last_used_epoch = media_texture_epoch;
        if self.textures.len() < MEDIA_UPLOAD_TEXTURE_RING_LEN {
            self.textures.push(GpuCompositorTexture::new(
                device,
                key.width,
                key.height,
                "SparkleFlinger GPU pooled media producer texture",
            ));
            record_gpu_media_texture_allocation();
        }

        let slot = self.next_slot % self.textures.len();
        self.next_slot = (slot + 1) % MEDIA_UPLOAD_TEXTURE_RING_LEN;
        &self.textures[slot]
    }
}

impl GpuSparkleFlinger {
    #[cfg(test)]
    pub(crate) fn upload_canvas_frame(&mut self, canvas: &Canvas) -> Option<GpuTextureFrame> {
        self.upload_media_canvas_frame(MediaTextureSourceKey::for_test(0), canvas)
    }

    pub(crate) fn begin_media_upload_frame(&mut self) {
        self.media_texture_epoch = self.media_texture_epoch.saturating_add(1);
        self.prune_idle_media_texture_pools();
    }

    fn prune_idle_media_texture_pools(&mut self) {
        let current_epoch = self.media_texture_epoch;
        self.media_texture_pools.retain(|_, pool| {
            current_epoch.saturating_sub(pool.last_used_epoch)
                <= MEDIA_UPLOAD_TEXTURE_POOL_IDLE_FRAMES
        });
    }

    pub(crate) fn upload_media_canvas_frame(
        &mut self,
        source: MediaTextureSourceKey,
        canvas: &Canvas,
    ) -> Option<GpuTextureFrame> {
        let max_texture_dimension = self.probe.max_texture_dimension_2d;
        if canvas.width() == 0
            || canvas.height() == 0
            || canvas.width() > max_texture_dimension
            || canvas.height() > max_texture_dimension
        {
            tracing::warn!(
                width = canvas.width(),
                height = canvas.height(),
                max_texture_dimension,
                "skipping GPU canvas upload for media frame with unsupported dimensions"
            );
            return None;
        }
        let key = MediaUploadTextureKey {
            source,
            width: canvas.width(),
            height: canvas.height(),
        };
        let pool = self
            .media_texture_pools
            .entry(key)
            .or_insert_with(MediaUploadTexturePool::new);
        let texture = pool.next_texture(&self.device, key, self.media_texture_epoch);
        record_gpu_media_texture_upload(canvas.width(), canvas.height());
        write_rgba_texture(
            &self.queue,
            &texture.texture,
            canvas.width(),
            canvas.height(),
            canvas.as_rgba_bytes(),
        );
        self.producer_content_generation = self.producer_content_generation.saturating_add(1);
        Some(GpuTextureFrame {
            width: canvas.width(),
            height: canvas.height(),
            storage_id: texture.storage_id,
            content_generation: self.producer_content_generation,
            origin: GpuTextureFrameOrigin::ProducerTexture,
            texture: texture.texture.clone(),
            view: texture.view.clone(),
            immutable_lease: None,
            #[cfg(target_os = "windows")]
            windows_screen_lease: None,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            macos_screen_lease: None,
        })
    }
}
