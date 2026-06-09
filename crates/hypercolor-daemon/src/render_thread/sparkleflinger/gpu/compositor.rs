use anyhow::{Context, Result};
use wgpu::util::DeviceExt;

use super::super::{
    ComposedFrameSet, CompositionLayer, CompositionMode, CompositionPlan, PreviewSurfaceRequest,
};
use super::frame_set::{
    gpu_bypassed_canvas_frame, gpu_bypassed_surface_frame, gpu_bypassed_without_surfaces,
    gpu_composed_from_surface, gpu_composed_with_preview_surface, gpu_composed_without_surfaces,
};
use super::preview::{
    CachedPreviewSurfaceKey, bypass_preview_surface, preview_request_matches_plan,
};
use super::readback::{CachedReadbackKey, CachedReadbackSurface};
use super::source::{
    CachedSourceUpload, cached_readback_key, cached_source_upload, copy_frame_into_output_texture,
    copy_gpu_source_frame_into_texture, gpu_source_frame, upload_frame_into_cached_texture,
    upload_frame_into_source_texture,
};
use super::telemetry::record_gpu_source_upload_skipped;
use super::{
    COMPOSE_PARAM_BYTES, COMPOSE_WORKGROUP_HEIGHT, COMPOSE_WORKGROUP_WIDTH,
    GpuCompositorOutputSurface, GpuCompositorPipeline, GpuCompositorSurfaceSet, GpuSparkleFlinger,
    texture_extent,
};
use crate::performance::CompositorBackendKind;
use crate::render_thread::producer_queue::{GpuTextureFrameOrigin, ProducerFrame};

impl GpuSparkleFlinger {
    pub(crate) fn compose(
        &mut self,
        plan: &CompositionPlan,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
        let requires_preview_surface = preview_surface_request.is_some();
        let readback_key = cached_readback_key(plan);
        if plan.layers.len() == 1
            && let Some(layer) = plan.layers.first()
            && self.layer_reuses_current_output_texture(layer, plan.width, plan.height)
        {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_composed_without_surfaces());
            }
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                readback_key,
                requires_cpu_sampling_canvas,
                preview_surface_request,
                None,
            );
        }
        if plan.layers.len() == 1
            && let Some(layer) = plan.layers.first()
            && layer.is_bypass_candidate()
            && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
        {
            return self.compose_bypass_layer(
                plan,
                readback_key,
                layer,
                requires_cpu_sampling_canvas,
                preview_surface_request,
            );
        }

        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width != plan.width || current_height != plan.height
        ) {
            self.discard_superseded_preview_work();
        }
        self.ensure_surface_size(plan.width, plan.height);
        if let Some(key) = readback_key.as_ref()
            && self.current_output.is_some()
            && self.cached_composition_key.as_ref() == Some(key)
        {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_composed_without_surfaces());
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_with_preview_surface(cached));
            }
            if let Some(surface) = self
                .cached_readback_surface
                .as_ref()
                .filter(|cached| {
                    cached.key.as_ref() == Some(key)
                        && preview_request_matches_plan(
                            preview_surface_request,
                            plan.width,
                            plan.height,
                        )
                })
                .map(|cached| cached.surface.clone())
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_from_surface(
                    surface,
                    requires_cpu_sampling_canvas,
                ));
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && self.has_pending_or_ready_preview_for(request)
            {
                return Ok(gpu_composed_without_surfaces());
            }
            let pending_output_submission = self.pending_output_submission.take();
            if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
                self.clear_superseded_preview_outputs();
            } else {
                self.discard_ready_and_pending_preview_surface();
            }
            return self.read_back_current_output_surface(
                plan.width,
                plan.height,
                Some(key.clone()),
                requires_cpu_sampling_canvas,
                preview_surface_request,
                pending_output_submission,
            );
        }
        if preview_surface_request.is_some() && !requires_cpu_sampling_canvas {
            self.clear_superseded_preview_outputs();
        } else {
            self.discard_superseded_preview_work();
        }

        let surfaces = self
            .surfaces
            .as_mut()
            .expect("surface allocation should succeed before composition");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU compose"),
            });

        let mut use_front_as_current = true;
        let mut layers = plan.layers.iter();
        let first_layer = layers
            .next()
            .context("GPU composition requires at least one layer")?;

        if first_layer.can_bypass_for_size(plan.width, plan.height) {
            copy_frame_into_output_texture(
                &self.device,
                &self.queue,
                &self.pipeline,
                &surfaces.front,
                &mut surfaces.front_contents,
                &mut surfaces.pending_upload_buffers,
                &mut encoder,
                &first_layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
        } else {
            let full_range = wgpu::ImageSubresourceRange::default();
            encoder.clear_texture(&surfaces.front.texture, &full_range);
            surfaces.front_contents = None;
            compose_layer_into_gpu(
                &self.device,
                &self.pipeline,
                surfaces,
                &mut encoder,
                first_layer,
                true,
            );
            use_front_as_current = false;
        }

        for layer in layers {
            compose_layer_into_gpu(
                &self.device,
                &self.pipeline,
                surfaces,
                &mut encoder,
                layer,
                use_front_as_current,
            );
            use_front_as_current = !use_front_as_current;
        }

        let current_output = if use_front_as_current {
            GpuCompositorOutputSurface::Front
        } else {
            GpuCompositorOutputSurface::Back
        };
        self.current_output = Some(current_output);
        self.cached_composition_key.clone_from(&readback_key);
        self.output_generation = self.output_generation.saturating_add(1);
        self.cached_sample_result = None;
        if !requires_cpu_sampling_canvas && !requires_preview_surface {
            self.pending_output_submission = Some(encoder);
            return Ok(gpu_composed_without_surfaces());
        }

        if let Some(key) = readback_key.as_ref()
            && let Some(cached) = self.cached_readback_surface.as_ref()
            && cached.key.as_ref() == Some(key)
            && preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
        {
            let cached_surface = cached.surface.clone();
            self.queue.submit(Some(encoder.finish()));
            self.clear_pending_upload_buffers();
            return Ok(gpu_composed_from_surface(
                cached_surface,
                requires_cpu_sampling_canvas,
            ));
        }

        self.read_back_current_output_surface(
            plan.width,
            plan.height,
            readback_key,
            requires_cpu_sampling_canvas,
            preview_surface_request,
            Some(encoder),
        )
    }

    pub(crate) fn ensure_surface_size(&mut self, width: u32, height: u32) {
        if matches!(
            self.surfaces,
            Some(GpuCompositorSurfaceSet {
                width: current_width,
                height: current_height,
                ..
            }) if current_width == width && current_height == height
        ) {
            return;
        }

        self.discard_pending_preview_map();
        self.surfaces = Some(GpuCompositorSurfaceSet::new(
            &self.device,
            &self.pipeline,
            width,
            height,
        ));
        self.preview_surfaces = None;
        self.current_output = None;
        self.cached_composition_key = None;
        self.cached_readback_surface = None;
        self.cached_preview_surfaces.clear();
        self.pending_output_submission = None;
        self.pending_preview_readback = None;
        self.pending_preview_submission = None;
        self.pending_preview_map = None;
        self.ready_preview_surface = None;
        self.cached_sample_result = None;
        self.spatial_sampler.clear_bind_groups();
    }

    fn layer_reuses_current_output_texture(
        &self,
        layer: &CompositionLayer,
        width: u32,
        height: u32,
    ) -> bool {
        self.current_output.is_some()
            && layer.mode == CompositionMode::Replace
            && layer.opacity >= 1.0
            && layer.transform.is_none()
            && layer.adjust.is_none()
            && matches!(
                &layer.frame,
                ProducerFrame::GpuTexture(frame)
                    if frame.origin == GpuTextureFrameOrigin::CompositorOutput
                        && frame.storage_id == self.output_generation
                        && frame.width == width
                        && frame.height == height
            )
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "sibling compose_* methods return Result; keeping this one wrapped preserves call-site uniformity"
    )]
    fn compose_bypass_layer(
        &mut self,
        plan: &CompositionPlan,
        readback_key: Option<CachedReadbackKey>,
        layer: &CompositionLayer,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
    ) -> Result<ComposedFrameSet> {
        let requires_preview_surface = preview_surface_request.is_some();
        let same_surface_canvas = match &layer.frame {
            ProducerFrame::Canvas(canvas) => {
                self.current_output == Some(GpuCompositorOutputSurface::Front)
                    && self.cached_readback_surface.as_ref().is_some_and(|cached| {
                        cached.surface.width() == plan.width
                            && cached.surface.height() == plan.height
                            && cached.surface.storage_identity() == canvas.storage_identity()
                    })
            }
            ProducerFrame::Surface(_) => false,
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => false,
            ProducerFrame::GpuTexture(_) => false,
        };
        let same_output = readback_key.as_ref().is_some_and(|key| {
            self.current_output == Some(GpuCompositorOutputSurface::Front)
                && self.cached_composition_key.as_ref() == Some(key)
        }) || same_surface_canvas;
        if same_output {
            if !requires_cpu_sampling_canvas && !requires_preview_surface {
                return Ok(gpu_bypassed_without_surfaces());
            }
            if !requires_cpu_sampling_canvas
                && let Some(request) = preview_surface_request
                && !preview_request_matches_plan(Some(request), plan.width, plan.height)
                && let Some(key) = readback_key.as_ref()
                && let Some(cached) = self.cached_preview_surface(&CachedPreviewSurfaceKey {
                    composition: key.clone(),
                    request,
                })
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_composed_with_preview_surface(cached));
            }
            if let Some(surface) = self
                .cached_readback_surface
                .as_ref()
                .filter(|_| {
                    preview_request_matches_plan(preview_surface_request, plan.width, plan.height)
                })
                .map(|cached| cached.surface.clone())
            {
                self.discard_superseded_preview_work();
                return Ok(gpu_bypassed_surface_frame(
                    &surface,
                    requires_cpu_sampling_canvas,
                    requires_preview_surface,
                ));
            }
        }

        self.discard_superseded_preview_work();
        self.ensure_surface_size(plan.width, plan.height);
        if let Some(surfaces) = self.surfaces.as_mut() {
            upload_frame_into_cached_texture(
                &self.queue,
                &surfaces.front.texture,
                &mut surfaces.front_contents,
                &layer.frame,
                #[cfg(test)]
                &mut surfaces.front_upload_count,
            );
            surfaces.back_contents = None;
        }
        self.current_output = Some(GpuCompositorOutputSurface::Front);
        self.cached_composition_key.clone_from(&readback_key);
        if !same_output {
            self.output_generation = self.output_generation.saturating_add(1);
            self.cached_sample_result = None;
        }

        let mut composed = match &layer.frame {
            ProducerFrame::Surface(surface) => gpu_bypassed_surface_frame(
                surface,
                requires_cpu_sampling_canvas,
                requires_preview_surface,
            ),
            ProducerFrame::Canvas(canvas) => gpu_bypassed_canvas_frame(
                canvas,
                requires_cpu_sampling_canvas,
                requires_preview_surface,
            ),
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => {
                unreachable!("GPU producer frames are composed instead of bypassed")
            }
            ProducerFrame::GpuTexture(_) => {
                unreachable!("GPU producer frames are composed instead of bypassed")
            }
        };
        let cached_surface = composed
            .preview_surface
            .as_ref()
            .or(composed.sampling_surface.as_ref())
            .cloned()
            .or_else(|| bypass_preview_surface(&layer.frame));
        self.cached_readback_surface = cached_surface.map(|surface| CachedReadbackSurface {
            key: readback_key,
            surface,
        });
        composed.backend = CompositorBackendKind::Gpu;
        self.ready_preview_surface = None;
        Ok(composed)
    }

    fn read_back_current_output_surface(
        &mut self,
        width: u32,
        height: u32,
        readback_key: Option<CachedReadbackKey>,
        requires_cpu_sampling_canvas: bool,
        preview_surface_request: Option<PreviewSurfaceRequest>,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<ComposedFrameSet> {
        if requires_cpu_sampling_canvas {
            if let Some(encoder) = encoder {
                self.pending_output_submission = Some(encoder);
            }
            return Ok(gpu_composed_without_surfaces());
        }
        let Some(current_output) = self.current_output else {
            anyhow::bail!("GPU readback requested without a composed output surface");
        };
        if let Some(request) = preview_surface_request {
            let cache_as_full_size = preview_request_matches_plan(Some(request), width, height);
            return self.stage_preview_surface_readback(
                current_output,
                width,
                height,
                readback_key,
                request,
                cache_as_full_size,
                encoder,
            );
        }
        if let Some(encoder) = encoder {
            self.pending_output_submission = Some(encoder);
        }
        Ok(gpu_composed_without_surfaces())
    }
}

fn compose_layer_into_gpu(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    layer: &CompositionLayer,
    use_front_as_current: bool,
) {
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

    if let Some(frame) = gpu_source_frame(&layer.frame)
        && shader_mode == ComposeShaderMode::Replace
        && !layer.needs_processing_for_size(surfaces.width, surfaces.height)
    {
        record_gpu_source_upload_skipped();
        let output = if use_front_as_current {
            &surfaces.back
        } else {
            &surfaces.front
        };
        copy_gpu_source_frame_into_texture(
            device,
            pipeline,
            encoder,
            &mut surfaces.pending_upload_buffers,
            &frame,
            output,
        );
        set_texture_contents(surfaces, output_surface, None);
        return;
    }

    let gpu_frame = gpu_source_frame(&layer.frame);

    if let Some(frame) = gpu_frame.as_ref()
        && frame.needs_shader_copy()
    {
        record_gpu_source_upload_skipped();
        let source = &surfaces.source;
        copy_gpu_source_frame_into_texture(
            device,
            pipeline,
            encoder,
            &mut surfaces.pending_upload_buffers,
            frame,
            source,
        );
        surfaces.cached_source_upload = None;
    } else if gpu_frame.is_none() {
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
            return;
        }
    }

    let params = encode_compose_params(surfaces.width, surfaces.height, shader_mode, layer);
    encode_compose_params_upload(device, pipeline, surfaces, encoder, &params);
    #[cfg(test)]
    {
        surfaces.compose_dispatch_count = surfaces.compose_dispatch_count.saturating_add(1);
    }
    if let Some(frame) = gpu_frame {
        record_gpu_source_upload_skipped();
        let bind_group = {
            let (current_view, output_view) = if use_front_as_current {
                (&surfaces.front.view, &surfaces.back.view)
            } else {
                (&surfaces.back.view, &surfaces.front.view)
            };
            let source_view = if frame.needs_shader_copy() {
                &surfaces.source.view
            } else {
                frame.view()
            };
            create_compose_bind_group(
                device,
                pipeline,
                current_view,
                source_view,
                output_view,
                "SparkleFlinger GPU imported producer bind group",
            )
        };
        dispatch_compose_pass(
            encoder,
            pipeline,
            &bind_group,
            surfaces.width,
            surfaces.height,
        );
        set_texture_contents(surfaces, output_surface, None);
        return;
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
        surfaces.width,
        surfaces.height,
    );
    set_texture_contents(surfaces, output_surface, None);
}

fn dispatch_compose_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &GpuCompositorPipeline,
    bind_group: &wgpu::BindGroup,
    width: u32,
    height: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("SparkleFlinger GPU compose pass"),
        timestamp_writes: None,
    });
    pass.set_pipeline(&pipeline.compose_pipeline);
    pass.set_bind_group(0, bind_group, &[]);
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

pub(super) fn create_compose_bind_group(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    current: &wgpu::TextureView,
    source: &wgpu::TextureView,
    output: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &pipeline.compose_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(current),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(output),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: pipeline.params_buffer.as_entire_binding(),
            },
        ],
    })
}

fn encode_compose_params_upload(
    device: &wgpu::Device,
    pipeline: &GpuCompositorPipeline,
    surfaces: &mut GpuCompositorSurfaceSet,
    encoder: &mut wgpu::CommandEncoder,
    params: &[u8; COMPOSE_PARAM_BYTES],
) {
    if surfaces.cached_compose_params.as_ref() == Some(params) {
        return;
    }
    let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("SparkleFlinger GPU compose params upload"),
        contents: params,
        usage: wgpu::BufferUsages::COPY_SRC,
    });
    encoder.copy_buffer_to_buffer(
        &upload,
        0,
        &pipeline.params_buffer,
        0,
        COMPOSE_PARAM_BYTES as u64,
    );
    surfaces.pending_upload_buffers.push(upload);
    surfaces.cached_compose_params = Some(*params);
    #[cfg(test)]
    {
        surfaces.compose_param_write_count = surfaces.compose_param_write_count.saturating_add(1);
    }
}

fn encode_compose_params(
    width: u32,
    height: u32,
    mode: ComposeShaderMode,
    layer: &CompositionLayer,
) -> [u8; COMPOSE_PARAM_BYTES] {
    let mut bytes = [0u8; COMPOSE_PARAM_BYTES];
    let transform = layer.transform.unwrap_or_default();
    let adjust = layer.adjust.unwrap_or_default();
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&(mode as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(fit_mode(transform.fit) as u32).to_le_bytes());
    bytes[16..20].copy_from_slice(&layer.frame.width().to_le_bytes());
    bytes[20..24].copy_from_slice(&layer.frame.height().to_le_bytes());
    let processing = if layer.needs_processing_for_size(width, height) {
        1_u32
    } else {
        0_u32
    };
    bytes[24..28].copy_from_slice(&processing.to_le_bytes());
    bytes[32..36].copy_from_slice(&layer.opacity.to_le_bytes());
    bytes[36..40].copy_from_slice(&transform.anchor.x.to_le_bytes());
    bytes[40..44].copy_from_slice(&transform.anchor.y.to_le_bytes());
    bytes[44..48].copy_from_slice(&transform.scale[0].to_le_bytes());
    bytes[48..52].copy_from_slice(&transform.scale[1].to_le_bytes());
    bytes[52..56].copy_from_slice(&transform.rotation.cos().to_le_bytes());
    bytes[56..60].copy_from_slice(&transform.rotation.sin().to_le_bytes());
    bytes[64..68].copy_from_slice(&adjust.brightness.to_le_bytes());
    bytes[68..72].copy_from_slice(&adjust.saturation.to_le_bytes());
    bytes[72..76].copy_from_slice(&adjust.hue_shift.to_le_bytes());
    let tint_strength = (adjust.tint_strength * adjust.tint[3].clamp(0.0, 1.0)).clamp(0.0, 1.0);
    bytes[76..80].copy_from_slice(&tint_strength.to_le_bytes());
    bytes[80..84].copy_from_slice(&adjust.tint[0].to_le_bytes());
    bytes[84..88].copy_from_slice(&adjust.tint[1].to_le_bytes());
    bytes[88..92].copy_from_slice(&adjust.tint[2].to_le_bytes());
    bytes[92..96].copy_from_slice(&adjust.contrast.to_le_bytes());
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeShaderMode {
    Replace = 0,
    Alpha = 1,
    Add = 2,
    Screen = 3,
    Multiply = 4,
    Overlay = 5,
    SoftLight = 6,
    ColorDodge = 7,
    Difference = 8,
    Tint = 9,
    LumaReveal = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ComposeFitMode {
    Contain = 0,
    Cover = 1,
    Stretch = 2,
    Tile = 3,
    Mirror = 4,
}

fn fit_mode(mode: hypercolor_types::viewport::FitMode) -> ComposeFitMode {
    match mode {
        hypercolor_types::viewport::FitMode::Contain => ComposeFitMode::Contain,
        hypercolor_types::viewport::FitMode::Cover => ComposeFitMode::Cover,
        hypercolor_types::viewport::FitMode::Stretch => ComposeFitMode::Stretch,
        hypercolor_types::viewport::FitMode::Tile => ComposeFitMode::Tile,
        hypercolor_types::viewport::FitMode::Mirror => ComposeFitMode::Mirror,
    }
}
