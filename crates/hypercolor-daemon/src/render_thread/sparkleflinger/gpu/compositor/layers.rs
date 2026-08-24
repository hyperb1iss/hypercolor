use anyhow::{Context, Result};

use super::super::super::{CompositionLayer, CompositionMode};
use super::super::source::{
    CachedSourceUpload, cached_source_upload, copy_gpu_source_frame_into_texture, gpu_source_frame,
    upload_frame_into_source_texture,
};
use super::super::telemetry::record_gpu_source_upload_skipped;
use super::super::{
    COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH, GpuCompositorOutputSurface,
    GpuCompositorPipeline, GpuCompositorSurfaceSet, ScreenUploadContentKey, texture_extent,
};
use super::bind_groups::{ComposeShaderMode, encode_compose_params, encode_compose_params_upload};
use crate::render_thread::producer_queue::NativeScreenTextureLease;
use crate::render_thread::producer_queue::{GpuTextureFrame, GpuTextureFrameOrigin, ProducerFrame};

pub(super) fn compose_layer_into_gpu(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &mut GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    layer: &CompositionLayer,
    uploaded_screen_frame: Option<&GpuTextureFrame>,
    use_front_as_current: bool,
    native_screen_leases: &mut Vec<NativeScreenTextureLease>,
) -> Result<()> {
    let shader_mode = if layer.mode == CompositionMode::Replace && layer.opacity >= 1.0 {
        ComposeShaderMode::Replace
    } else {
        match layer.mode {
            CompositionMode::Replace | CompositionMode::Alpha => ComposeShaderMode::Alpha,
            CompositionMode::Add => ComposeShaderMode::Add,
            CompositionMode::Screen => ComposeShaderMode::Screen,
            CompositionMode::Multiply => ComposeShaderMode::Multiply,
            CompositionMode::Overlay => ComposeShaderMode::Overlay,
            CompositionMode::SoftLight => ComposeShaderMode::SoftLight,
            CompositionMode::ColorDodge => ComposeShaderMode::ColorDodge,
            CompositionMode::Difference => ComposeShaderMode::Difference,
            CompositionMode::Tint => ComposeShaderMode::Tint,
            CompositionMode::LumaReveal => ComposeShaderMode::LumaReveal,
        }
    };
    let output_surface = if use_front_as_current {
        GpuCompositorOutputSurface::Back
    } else {
        GpuCompositorOutputSurface::Front
    };

    let gpu_frame = uploaded_screen_frame
        .map(super::super::source::GpuSourceFrame::Texture)
        .or_else(|| gpu_source_frame(&layer.frame));

    if let Some(frame) = gpu_frame.as_ref()
        && shader_mode == ComposeShaderMode::Replace
        && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
    {
        if uploaded_screen_frame.is_none() {
            record_gpu_source_upload_skipped();
        }
        let output = if use_front_as_current {
            &surfaces.back
        } else {
            &surfaces.front
        };
        copy_gpu_source_frame_into_texture(
            device,
            queue,
            pipeline,
            encoder,
            &mut surfaces.pending_upload_buffers,
            &mut surfaces.source_copy_bind_groups,
            frame,
            output,
            native_screen_leases,
        );
        set_texture_contents(
            surfaces,
            output_surface,
            uploaded_screen_frame
                .as_ref()
                .and_then(|_| cached_source_upload(&layer.frame)),
        );
        return Ok(());
    }

    if gpu_frame.is_none() {
        upload_frame_into_source_texture(device, encoder, surfaces, &layer.frame);
        if shader_mode == ComposeShaderMode::Replace
            && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
        {
            let output_texture = if use_front_as_current {
                &surfaces.back.texture
            } else {
                &surfaces.front.texture
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &surfaces.source.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: output_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                texture_extent(surfaces.width, surfaces.height),
            );
            set_texture_contents(surfaces, output_surface, cached_source_upload(&layer.frame));
            return Ok(());
        }
    }

    let source_flip_y = gpu_frame
        .as_ref()
        .is_some_and(super::super::source::GpuSourceFrame::flip_y_on_shader_copy);
    let params = encode_compose_params(
        surfaces.width,
        surfaces.height,
        shader_mode,
        layer,
        source_flip_y,
    );
    let params_offset =
        encode_compose_params_upload(device, queue, pipeline, surfaces, encoder, &params);
    #[cfg(test)]
    {
        surfaces.compose_dispatch_count = surfaces.compose_dispatch_count.saturating_add(1);
    }
    if let Some(frame) = gpu_frame.as_ref() {
        if let Some(lease) = frame.native_screen_lease() {
            native_screen_leases.push(lease);
        }
        if uploaded_screen_frame.is_none() {
            record_gpu_source_upload_skipped();
        }
        let (width, height) = (surfaces.width, surfaces.height);
        let projected_source = uploaded_screen_frame.is_none()
            && matches!(
                &layer.frame,
                ProducerFrame::GpuTexture(frame)
                    if frame.origin == GpuTextureFrameOrigin::ProjectionSnapshot
            );
        let bind_group = {
            let GpuCompositorSurfaceSet {
                generation,
                front,
                back,
                source,
                compose_source_bind_groups,
                ..
            } = surfaces;
            let (current_view, output_view) = if use_front_as_current {
                (&front.view, &back.view)
            } else {
                (&back.view, &front.view)
            };
            let _ = source;
            let source_view = frame.view();
            let source_storage_id = frame.cached_display_source_copy().storage_id;
            if projected_source {
                compose_source_bind_groups
                    .get_projected(
                        *generation,
                        source_storage_id,
                        source_view,
                        use_front_as_current,
                    )
                    .context("projected source bind group was not admitted")?
            } else {
                compose_source_bind_groups.get_or_create_transient(
                    device,
                    pipeline,
                    *generation,
                    source_storage_id,
                    source_view,
                    use_front_as_current,
                    current_view,
                    output_view,
                    frame.native_screen_cache_lease(),
                )
            }
        };
        dispatch_compose_pass(encoder, pipeline, &bind_group, params_offset, width, height);
        set_texture_contents(surfaces, output_surface, None);
        return Ok(());
    }

    let bind_group = if use_front_as_current {
        &surfaces.bind_groups.front_to_back
    } else {
        &surfaces.bind_groups.back_to_front
    };
    dispatch_compose_pass(
        encoder,
        pipeline,
        bind_group,
        params_offset,
        surfaces.width,
        surfaces.height,
    );
    set_texture_contents(surfaces, output_surface, None);
    Ok(())
}

pub(super) fn return_screen_frame_scratch(
    surfaces: &mut GpuCompositorSurfaceSet,
    scratch: &mut Option<Vec<Option<GpuTextureFrame>>>,
) {
    if let Some(mut frames) = scratch.take() {
        frames.clear();
        surfaces.uploaded_screen_frame_scratch = frames;
    }
}

pub(super) fn upload_screen_layers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surfaces: &mut GpuCompositorSurfaceSet,
    layers: &[CompositionLayer],
) -> Result<()> {
    let mut uploaded = std::mem::take(&mut surfaces.uploaded_screen_frame_scratch);
    uploaded.clear();
    let prior_capacity = uploaded.capacity();
    let result = (|| -> Result<()> {
        if prior_capacity < layers.len() {
            uploaded.try_reserve_exact(layers.len() - prior_capacity)?;
        }
        for layer in layers {
            let ProducerFrame::ScreenPublication(publication) = &layer.frame else {
                uploaded.push(None);
                continue;
            };
            let surface = publication.surface();
            let GpuCompositorSurfaceSet {
                screen_upload_pool,
                compose_source_bind_groups,
                ..
            } = surfaces;
            let content_key = ScreenUploadContentKey::new(
                publication.plan_generation(),
                publication.descriptor_identity(),
                publication.branch_sequence(),
                surface.extent().width(),
                surface.extent().height(),
            );
            let (frame, wrote_texture) = screen_upload_pool.upload_rgba(
                device,
                queue,
                surface.extent().width(),
                surface.extent().height(),
                surface.pixels(),
                content_key,
                |storage_id| compose_source_bind_groups.release_source(storage_id),
            )?;
            #[cfg(test)]
            if wrote_texture {
                surfaces.source_upload_count = surfaces.source_upload_count.saturating_add(1);
            }
            #[cfg(not(test))]
            let _ = wrote_texture;
            uploaded.push(Some(frame));
        }
        Ok(())
    })();
    #[cfg(test)]
    if uploaded.capacity() > prior_capacity {
        surfaces.screen_layer_host_allocation_count = surfaces
            .screen_layer_host_allocation_count
            .saturating_add(1);
    }
    surfaces.uploaded_screen_frame_scratch = uploaded;
    result
}

fn dispatch_compose_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &GpuCompositorPipeline,
    bind_group: &wgpu::BindGroup,
    params_offset: u32,
    width: u32,
    height: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU compose pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.compose_pipeline);
    pass.set_bind_group(0, bind_group, &[params_offset]);
    pass.dispatch_workgroups(
        width.div_ceil(COMPOSE_WORKGROUP_WIDTH),
        height.div_ceil(COMPOSE_WORKGROUP_HEIGHT),
        1,
    );
}

fn set_texture_contents(
    surfaces: &mut GpuCompositorSurfaceSet,
    output: GpuCompositorOutputSurface,
    contents: Option<CachedSourceUpload>,
) {
    match output {
        GpuCompositorOutputSurface::Front => surfaces.front_contents = contents,
        GpuCompositorOutputSurface::Back => surfaces.back_contents = contents,
    }
}
