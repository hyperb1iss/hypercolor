use hypercolor_types::canvas::{RenderSurfacePool, SurfaceDescriptor, SurfaceStateCounts};

use super::super::super::DisplayFinalizeCacheKey;
use super::super::{GpuCompositorTexture, PendingUploadBuffers, padded_bytes_per_row};
use super::{
    DISPLAY_FINALIZE_READBACK_SLOT_COUNT, DisplayYuv420Layout, GpuDisplayFinalizeFormat,
    GpuDisplayFinalizeSurfaceSet, PendingGpuDisplayFinalize,
};

impl PendingGpuDisplayFinalize {
    pub(in crate::render_thread::sparkleflinger::gpu) fn new(
        cache_key: DisplayFinalizeCacheKey,
        surface_generation: u64,
        format: GpuDisplayFinalizeFormat,
        width: u32,
        height: u32,
        padded_bytes_per_row: u32,
        yuv_layout: DisplayYuv420Layout,
        used_bytes: u64,
        mapped_bytes: u64,
        submission_index: wgpu::SubmissionIndex,
        buffer: wgpu::Buffer,
        slot: usize,
    ) -> Self {
        Self {
            cache_key,
            surface_generation,
            format,
            width,
            height,
            padded_bytes_per_row,
            yuv_layout,
            used_bytes,
            mapped_bytes,
            submission_index,
            buffer,
            receiver: None,
            map_ready: false,
            slot,
        }
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn unmap_after_failed_map(&mut self) {
        self.receiver = None;
        self.map_ready = false;
        self.buffer.unmap();
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn map_ready(&self) -> bool {
        self.map_ready
    }
}

impl DisplayYuv420Layout {
    pub(in crate::render_thread::sparkleflinger::gpu) fn new(width: u32, height: u32) -> Self {
        let y_stride = width;
        let uv_stride = width.div_ceil(2);
        let uv_height = height.div_ceil(2);
        let y_plane_len = y_stride
            .checked_mul(height)
            .expect("display Y plane size should fit in u32");
        let u_plane_len = uv_stride
            .checked_mul(uv_height)
            .expect("display U/V plane size should fit in u32");
        let total_len = y_plane_len
            .checked_add(
                u_plane_len
                    .checked_mul(2)
                    .expect("display chroma plane size should fit in u32"),
            )
            .expect("display YUV buffer size should fit in u32");
        let word_len = total_len
            .div_ceil(4)
            .checked_mul(4)
            .expect("display YUV word-aligned buffer size should fit in u32");

        Self {
            y_stride,
            uv_stride,
            y_plane_len,
            u_plane_len,
            total_len,
            word_len,
        }
    }
}

impl GpuDisplayFinalizeSurfaceSet {
    pub(in crate::render_thread::sparkleflinger::gpu) fn surface_pool_counts(
        &mut self,
    ) -> SurfaceStateCounts {
        self.readback_surfaces.slot_counts()
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn new(
        device: &wgpu::Device,
        generation: u64,
        width: u32,
        height: u32,
    ) -> Self {
        let padded_bytes_per_row = padded_bytes_per_row(width);
        let yuv_layout = DisplayYuv420Layout::new(width, height);
        let yuv_output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU display finalize YUV output"),
            size: u64::from(yuv_layout.word_len),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback_size = u64::from(padded_bytes_per_row) * u64::from(height);
        let yuv_readback_size = u64::from(yuv_layout.word_len);
        Self {
            generation,
            padded_bytes_per_row,
            yuv_layout,
            output: GpuCompositorTexture::new(
                device,
                width,
                height,
                "SparkleFlinger Display Finalize Output",
            ),
            yuv_output,
            readbacks: std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU display finalize readback"),
                    size: readback_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            }),
            yuv_readbacks: std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU display finalize YUV readback"),
                    size: yuv_readback_size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            }),
            readback_slots_in_use: [false; DISPLAY_FINALIZE_READBACK_SLOT_COUNT],
            next_readback_slot: 0,
            readback_surfaces: RenderSurfacePool::with_slot_count(
                SurfaceDescriptor::rgba8888(width, height),
                3,
            ),
            scene_source: None,
            face_source: None,
            pending_upload_buffers: PendingUploadBuffers::default(),
            #[cfg(test)]
            scene_upload_count: 0,
            #[cfg(test)]
            face_upload_count: 0,
            #[cfg(test)]
            last_readback_bytes: 0,
            #[cfg(test)]
            last_yuv_readback_bytes: 0,
        }
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn next_readback_buffer(
        &mut self,
        format: GpuDisplayFinalizeFormat,
    ) -> Option<(usize, wgpu::Buffer)> {
        for offset in 0..DISPLAY_FINALIZE_READBACK_SLOT_COUNT {
            let slot = (self.next_readback_slot + offset) % DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
            if !self.readback_slots_in_use[slot] {
                self.readback_slots_in_use[slot] = true;
                self.next_readback_slot = (slot + 1) % DISPLAY_FINALIZE_READBACK_SLOT_COUNT;
                let buffer = match format {
                    GpuDisplayFinalizeFormat::Rgba => self.readbacks[slot].clone(),
                    GpuDisplayFinalizeFormat::Yuv420 => self.yuv_readbacks[slot].clone(),
                };
                return Some((slot, buffer));
            }
        }

        None
    }

    pub(in crate::render_thread::sparkleflinger::gpu) fn release_readback_slot(
        &mut self,
        slot: usize,
    ) {
        if slot < DISPLAY_FINALIZE_READBACK_SLOT_COUNT {
            self.readback_slots_in_use[slot] = false;
        }
    }
}
