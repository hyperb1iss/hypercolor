use super::super::MediaTextureSourceKey;
use super::GpuCompositorTexture;
use super::telemetry::record_gpu_media_texture_allocation;

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
