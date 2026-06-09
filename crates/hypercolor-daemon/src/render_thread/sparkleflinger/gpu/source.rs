use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, PublishedSurfaceStorageIdentity};
use wgpu::util::DeviceExt;

use super::super::{CompositionAdjust, CompositionPlan, CompositionTransform};
use super::readback::{
    CachedReadbackAdjust, CachedReadbackKey, CachedReadbackLayer, CachedReadbackTransform,
};
use super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, GpuCompositorPipeline,
    GpuCompositorSurfaceSet, GpuCompositorTexture, GpuDisplaySourceTexture,
    SOURCE_COPY_PARAM_BYTES, texture_extent,
};
use crate::render_thread::producer_queue::{GpuTextureFrame, ProducerFrame};
use crate::render_thread::sparkleflinger::gpu::telemetry::record_gpu_source_upload_skipped;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedSourceUpload {
    storage: PublishedSurfaceStorageIdentity,
    generation: u64,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CachedGpuSourceCopy {
    pub(super) storage_id: u64,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) fn prepare_display_source_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &GpuCompositorPipeline,
    encoder: &mut wgpu::CommandEncoder,
    source: &mut Option<GpuDisplaySourceTexture>,
    pending_upload_buffers: &mut Vec<wgpu::Buffer>,
    frame: &ProducerFrame,
    gpu_frame: Option<&GpuSourceFrame<'_>>,
    label: &'static str,
    #[cfg(test)] upload_count: &mut usize,
) {
    let Some(gpu_frame) = gpu_frame else {
        ensure_display_source_texture(device, source, frame.width(), frame.height(), label);
        let source = source
            .as_mut()
            .expect("display source texture should exist before upload");
        source.cached_gpu_copy = None;
        upload_frame_into_cached_texture(
            queue,
            &source.texture.texture,
            &mut source.cached_upload,
            frame,
            #[cfg(test)]
            upload_count,
        );
        return;
    };

    if !gpu_frame.needs_display_source_copy() {
        return;
    }

    ensure_display_source_texture(device, source, frame.width(), frame.height(), label);
    let source = source
        .as_mut()
        .expect("display source texture should exist before GPU copy");
    let next_copy = gpu_frame.cached_display_source_copy();
    if source.cached_gpu_copy == Some(next_copy) {
        return;
    }
    record_gpu_source_upload_skipped();
    copy_gpu_source_frame_into_texture(
        device,
        pipeline,
        encoder,
        pending_upload_buffers,
        gpu_frame,
        &source.texture,
    );
    source.cached_upload = None;
    source.cached_gpu_copy = Some(next_copy);
}

fn ensure_display_source_texture(
    device: &wgpu::Device,
    source: &mut Option<GpuDisplaySourceTexture>,
    width: u32,
    height: u32,
    label: &'static str,
) {
    if source
        .as_ref()
        .is_some_and(|texture| texture.width == width && texture.height == height)
    {
        return;
    }
    *source = Some(GpuDisplaySourceTexture::new(device, width, height, label));
}

pub(super) fn upload_frame_into_source_texture(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    surfaces: &mut GpuCompositorSurfaceSet,
    frame: &ProducerFrame,
) {
    upload_frame_into_cached_texture_with_encoder(
        device,
        encoder,
        &surfaces.source.texture,
        &mut surfaces.pending_upload_buffers,
        &mut surfaces.cached_source_upload,
        frame,
        #[cfg(test)]
        &mut surfaces.source_upload_count,
    );
}

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

    pub(super) const fn needs_display_source_copy(&self) -> bool {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(_) => true,
            Self::Texture(_) => false,
        }
    }

    const fn cached_display_source_copy(&self) -> CachedGpuSourceCopy {
        match self {
            #[cfg(feature = "servo-gpu-import")]
            Self::Imported(frame) => CachedGpuSourceCopy {
                storage_id: frame.storage_id,
                width: frame.width,
                height: frame.height,
            },
            Self::Texture(frame) => CachedGpuSourceCopy {
                storage_id: frame.storage_id,
                width: frame.width,
                height: frame.height,
            },
        }
    }

    const fn flip_y_on_shader_copy(&self) -> bool {
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
}

pub(super) fn gpu_source_frame(frame: &ProducerFrame) -> Option<GpuSourceFrame<'_>> {
    match frame {
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(frame) => Some(GpuSourceFrame::Imported(frame)),
        ProducerFrame::GpuTexture(frame) => Some(GpuSourceFrame::Texture(frame)),
        ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => None,
    }
}

pub(super) fn copy_frame_into_output_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &GpuCompositorPipeline,
    output: &GpuCompositorTexture,
    cached_upload: &mut Option<CachedSourceUpload>,
    pending_upload_buffers: &mut Vec<wgpu::Buffer>,
    encoder: &mut wgpu::CommandEncoder,
    frame: &ProducerFrame,
    #[cfg(test)] upload_count: &mut usize,
) {
    if let Some(frame) = gpu_source_frame(frame) {
        record_gpu_source_upload_skipped();
        copy_gpu_source_frame_into_texture(
            device,
            pipeline,
            encoder,
            pending_upload_buffers,
            &frame,
            output,
        );
        *cached_upload = None;
        return;
    }

    upload_frame_into_cached_texture(
        queue,
        &output.texture,
        cached_upload,
        frame,
        #[cfg(test)]
        upload_count,
    );
}

pub(super) fn copy_gpu_source_frame_into_texture(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    encoder: &mut wgpu::CommandEncoder,
    pending_upload_buffers: &mut Vec<wgpu::Buffer>,
    frame: &GpuSourceFrame<'_>,
    output: &GpuCompositorTexture,
) {
    if frame.needs_shader_copy() {
        encode_source_copy_params_upload(
            device,
            pipeline,
            encoder,
            pending_upload_buffers,
            &encode_source_copy_params(
                frame.width(),
                frame.height(),
                frame.flip_y_on_shader_copy(),
            ),
        );
        let bind_group =
            create_source_copy_bind_group(device, pipeline, frame.view(), &output.view);
        dispatch_source_copy_pass(
            encoder,
            pipeline,
            &bind_group,
            frame.width(),
            frame.height(),
        );
        return;
    }

    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: frame.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &output.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        texture_extent(frame.width(), frame.height()),
    );
}

fn upload_frame_into_texture(queue: &wgpu::Queue, texture: &wgpu::Texture, frame: &ProducerFrame) {
    let Some(rgba_bytes) = frame.cpu_rgba_bytes() else {
        return;
    };
    write_rgba_texture(queue, texture, frame.width(), frame.height(), rgba_bytes);
}

pub(super) fn write_rgba_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    rgba_bytes: &[u8],
) {
    let bytes_per_row = width * BYTES_PER_PIXEL as u32;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba_bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: Some(height),
        },
        texture_extent(width, height),
    );
}

pub(super) fn upload_frame_into_cached_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    cached_upload: &mut Option<CachedSourceUpload>,
    frame: &ProducerFrame,
    #[cfg(test)] upload_count: &mut usize,
) {
    let next_upload = cached_source_upload(frame);
    if next_upload.is_some() && *cached_upload == next_upload {
        return;
    }

    upload_frame_into_texture(queue, texture, frame);
    *cached_upload = next_upload;
    #[cfg(test)]
    {
        *upload_count = upload_count.saturating_add(1);
    }
}

fn upload_frame_into_cached_texture_with_encoder(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    pending_upload_buffers: &mut Vec<wgpu::Buffer>,
    cached_upload: &mut Option<CachedSourceUpload>,
    frame: &ProducerFrame,
    #[cfg(test)] upload_count: &mut usize,
) {
    let next_upload = cached_source_upload(frame);
    if next_upload.is_some() && *cached_upload == next_upload {
        return;
    }

    upload_frame_into_texture_with_encoder(device, encoder, texture, pending_upload_buffers, frame);
    *cached_upload = next_upload;
    #[cfg(test)]
    {
        *upload_count = upload_count.saturating_add(1);
    }
}

fn upload_frame_into_texture_with_encoder(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    pending_upload_buffers: &mut Vec<wgpu::Buffer>,
    frame: &ProducerFrame,
) {
    let Some(rgba_bytes) = frame.cpu_rgba_bytes() else {
        return;
    };
    let width = frame.width();
    let height = frame.height();
    let bytes_per_row = width * BYTES_PER_PIXEL as u32;
    let padded_bytes_per_row = bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let contents = if padded_bytes_per_row == bytes_per_row {
        rgba_bytes.to_vec()
    } else {
        let mut padded = vec![0_u8; padded_bytes_per_row as usize * height as usize];
        for row in 0..height as usize {
            let source_offset = row * bytes_per_row as usize;
            let target_offset = row * padded_bytes_per_row as usize;
            padded[target_offset..target_offset + bytes_per_row as usize].copy_from_slice(
                &rgba_bytes[source_offset..source_offset + bytes_per_row as usize],
            );
        }
        padded
    };
    let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("SparkleFlinger GPU source texture upload"),
        contents: &contents,
        usage: wgpu::BufferUsages::COPY_SRC,
    });
    encoder.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &upload,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        texture_extent(width, height),
    );
    pending_upload_buffers.push(upload);
}

pub(super) fn cached_source_upload(frame: &ProducerFrame) -> Option<CachedSourceUpload> {
    match frame {
        ProducerFrame::Surface(surface) => Some(CachedSourceUpload {
            storage: surface.storage_identity(),
            generation: surface.generation(),
            width: surface.width(),
            height: surface.height(),
        }),
        ProducerFrame::Canvas(canvas) if canvas.is_shared() => Some(CachedSourceUpload {
            storage: canvas.storage_identity(),
            generation: 0,
            width: canvas.width(),
            height: canvas.height(),
        }),
        ProducerFrame::Canvas(_) => None,
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => None,
        ProducerFrame::GpuTexture(_) => None,
    }
}

pub(super) fn cached_readback_key(plan: &CompositionPlan) -> Option<CachedReadbackKey> {
    let mut layers = Vec::with_capacity(plan.layers.len());
    for layer in &plan.layers {
        layers.push(CachedReadbackLayer {
            source: cached_source_upload(&layer.frame)?,
            mode: layer.mode,
            opacity_bits: layer.opacity.to_bits(),
            transform: layer.transform.map(cached_transform),
            adjust: layer.adjust.map(cached_adjust),
        });
    }
    Some(CachedReadbackKey {
        width: plan.width,
        height: plan.height,
        layers,
    })
}

fn cached_transform(transform: CompositionTransform) -> CachedReadbackTransform {
    CachedReadbackTransform {
        anchor_x_bits: transform.anchor.x.to_bits(),
        anchor_y_bits: transform.anchor.y.to_bits(),
        scale_x_bits: transform.scale[0].to_bits(),
        scale_y_bits: transform.scale[1].to_bits(),
        rotation_bits: transform.rotation.to_bits(),
        fit: transform.fit,
    }
}

fn cached_adjust(adjust: CompositionAdjust) -> CachedReadbackAdjust {
    CachedReadbackAdjust {
        brightness: adjust.brightness.to_bits(),
        saturation: adjust.saturation.to_bits(),
        hue_shift: adjust.hue_shift.to_bits(),
        tint: adjust.tint.map(f32::to_bits),
        tint_strength: adjust.tint_strength.to_bits(),
        contrast: adjust.contrast.to_bits(),
    }
}

fn dispatch_source_copy_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &GpuCompositorPipeline,
    bind_group: &wgpu::BindGroup,
    width: u32,
    height: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU source copy pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.source_copy_pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(
        width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
        height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
        1,
    );
}

fn create_source_copy_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    source: &wgpu::TextureView,
    output: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("SparkleFlinger GPU source copy bind group"),
        layout: &pipeline.source_copy_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pipeline.source_copy_params_buffer.as_entire_binding(),
            },
        ],
    })
}

fn encode_source_copy_params_upload(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    encoder: &mut wgpu::CommandEncoder,
    pending_upload_buffers: &mut Vec<wgpu::Buffer>,
    params: &[u8; SOURCE_COPY_PARAM_BYTES],
) {
    let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("SparkleFlinger GPU source copy params upload"),
        contents: params,
        usage: wgpu::BufferUsages::COPY_SRC,
    });
    encoder.copy_buffer_to_buffer(
        &upload,
        0,
        &pipeline.source_copy_params_buffer,
        0,
        SOURCE_COPY_PARAM_BYTES as u64,
    );
    pending_upload_buffers.push(upload);
}

fn encode_source_copy_params(
    width: u32,
    height: u32,
    flip_y: bool,
) -> [u8; SOURCE_COPY_PARAM_BYTES] {
    let mut bytes = [0u8; SOURCE_COPY_PARAM_BYTES];
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::from(flip_y).to_le_bytes());
    bytes
}
