#[cfg(all(target_os = "macos", feature = "screen-capture"))]
use crate::render_thread::producer_queue::MacosScreenTextureLease;
#[cfg(any(
    target_os = "windows",
    all(target_os = "macos", feature = "screen-capture")
))]
use crate::render_thread::producer_queue::NativeScreenCacheLease;
use crate::render_thread::producer_queue::{GpuTextureFrame, ProducerFrame};
use hypercolor_core::types::canvas::PublishedSurfaceStorageIdentity;
use hypercolor_types::viewport::FitMode;

use super::super::CompositionMode;
use super::GpuCompositorTexture;
mod copy;

pub(super) use copy::{
    cached_readback_key, cached_source_upload, copy_frame_into_output_texture,
    copy_gpu_source_frame_into_texture, prepare_display_source_texture,
    upload_frame_into_cached_texture, upload_frame_into_source_texture, write_rgba_texture,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CachedCpuSourceStorage {
    PublishedSurface(PublishedSurfaceStorageIdentity),
    ScreenPublication {
        plan_generation: u64,
        descriptor_identity: u64,
        branch_sequence: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedSourceUpload {
    storage: CachedCpuSourceStorage,
    generation: u64,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CachedReadbackKey {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) layers: Vec<CachedReadbackLayer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedReadbackLayer {
    pub(super) source: CachedSourceUpload,
    pub(super) mode: CompositionMode,
    pub(super) opacity_bits: u32,
    pub(super) transform: Option<CachedReadbackTransform>,
    pub(super) adjust: Option<CachedReadbackAdjust>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedReadbackTransform {
    pub(super) anchor_x_bits: u32,
    pub(super) anchor_y_bits: u32,
    pub(super) scale_x_bits: u32,
    pub(super) scale_y_bits: u32,
    pub(super) rotation_bits: u32,
    pub(super) fit: FitMode,
    pub(super) sample_target_space: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedReadbackAdjust {
    pub(super) brightness: u32,
    pub(super) saturation: u32,
    pub(super) hue_shift: u32,
    pub(super) tint: [u32; 4],
    pub(super) tint_strength: u32,
    pub(super) contrast: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedGpuSourceCopy {
    pub(super) storage_id: u64,
    pub(super) content_generation: u64,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// Source-copy bind groups cached by view identity. wgpu views compare by
/// resource identity, so a hit means the exact same textures; entries keep
/// their views (and thus textures) alive, bounded by the cache cap.
#[derive(Default)]
pub(super) struct SourceCopyBindGroupCache {
    entries: Vec<CachedSourceCopyBindGroup>,
    #[cfg(test)]
    pub(super) creation_count: usize,
}

pub(super) struct GpuDisplaySourceTexture {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) texture: GpuCompositorTexture,
    pub(super) cached_upload: Option<CachedSourceUpload>,
    pub(super) cached_gpu_copy: Option<CachedGpuSourceCopy>,
    pub(super) bind_group_cache: SourceCopyBindGroupCache,
}

impl GpuDisplaySourceTexture {
    pub(in crate::render_thread::sparkleflinger::gpu) fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        label: &'static str,
    ) -> Self {
        Self {
            width,
            height,
            texture: GpuCompositorTexture::new(device, width, height, label),
            cached_upload: None,
            cached_gpu_copy: None,
            bind_group_cache: SourceCopyBindGroupCache::default(),
        }
    }
}

struct CachedSourceCopyBindGroup {
    source_view: wgpu::TextureView,
    output_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    #[cfg(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    ))]
    native_screen_lease: Option<NativeScreenCacheLease>,
}

const SOURCE_COPY_BIND_GROUP_CACHE_CAP: usize = 8;

pub(super) enum GpuSourceFrame<'a> {
    #[cfg(feature = "servo-gpu-import")]
    Imported(&'a hypercolor_core::effect::ImportedEffectFrame),
    Texture(&'a GpuTextureFrame),
}

impl GpuSourceFrame<'_> {
    pub(super) const fn width(&self) -> u32 {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.width,
            Self::Texture(frame) => frame.width,
        }
    }

    pub(super) const fn height(&self) -> u32 {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.height,
            Self::Texture(frame) => frame.height,
        }
    }

    fn texture(&self) -> &wgpu::Texture {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.texture.as_ref(),
            Self::Texture(frame) => &frame.texture,
        }
    }

    pub(super) fn view(&self) -> &wgpu::TextureView {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => frame.view.as_ref(),
            Self::Texture(frame) => &frame.view,
        }
    }

    /// Whether copying this frame into a compositor texture needs the
    /// y-flipping source-copy shader instead of copy_texture_to_texture.
    /// Blend-mode composition no longer consults this: it binds the frame's
    /// view directly and flips in the compose shader via params.
    pub(super) const fn needs_shader_copy(&self) -> bool {
        match self {
            #[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
            Self::Imported(_) => true,
            #[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
            Self::Imported(_) => true,
            #[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
            Self::Imported(_) => false,
            Self::Texture(_) => false,
        }
    }

    fn requires_shader_copy_to(&self, output: &wgpu::Texture) -> bool {
        self.needs_shader_copy()
            || self.texture().format().remove_srgb_suffix() != output.format().remove_srgb_suffix()
    }

    pub(super) const fn needs_display_source_copy(&self) -> bool {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(_) => true,
            Self::Texture(_) => false,
        }
    }

    pub(super) const fn cached_display_source_copy(&self) -> CachedGpuSourceCopy {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => CachedGpuSourceCopy {
                storage_id: frame.storage_id,
                content_generation: frame.storage_id,
                width: frame.width,
                height: frame.height,
            },
            Self::Texture(frame) => CachedGpuSourceCopy {
                storage_id: frame.storage_id,
                content_generation: frame.content_generation,
                width: frame.width,
                height: frame.height,
            },
        }
    }

    pub(super) const fn flip_y_on_shader_copy(&self) -> bool {
        match self {
            #[cfg(all(feature = "servo-gpu-import", target_os = "macos"))]
            Self::Imported(_) => true,
            #[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
            Self::Imported(_) => true,
            #[cfg(all(feature = "servo-gpu-import", target_os = "linux"))]
            Self::Imported(_) => false,
            Self::Texture(_) => false,
        }
    }

    #[cfg(any(
        target_os = "windows",
        all(target_os = "macos", feature = "screen-capture")
    ))]
    pub(super) fn native_screen_cache_lease(&self) -> Option<NativeScreenCacheLease> {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(_) => None,
            Self::Texture(frame) => frame.native_screen_cache_lease(),
        }
    }

    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    pub(super) fn macos_screen_lease(&self) -> Option<MacosScreenTextureLease> {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(_) => None,
            Self::Texture(frame) => frame.macos_screen_lease.clone(),
        }
    }
}

pub(super) fn gpu_source_frame(frame: &ProducerFrame) -> Option<GpuSourceFrame<'_>> {
    match frame {
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(frame) => Some(GpuSourceFrame::Imported(frame)),
        ProducerFrame::GpuTexture(frame) => Some(GpuSourceFrame::Texture(frame)),
        ProducerFrame::Canvas(_)
        | ProducerFrame::Surface(_)
        | ProducerFrame::ScreenPublication(_) => None,
    }
}
