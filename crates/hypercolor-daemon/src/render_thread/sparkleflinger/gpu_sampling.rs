use std::sync::mpsc;

use anyhow::{Context, Result};
use hypercolor_core::spatial::{PreparedZonePlan, PreparedZoneSamples};
use hypercolor_core::types::canvas::{BYTES_PER_PIXEL, PublishedSurface, RenderSurfacePool};
use hypercolor_types::canvas::SamplingMethod;
use hypercolor_types::event::ZoneColors;

const SAMPLE_WORKGROUP_SIZE: u32 = 64;
const SAMPLE_PARAM_BYTES: usize = 16;
const SAMPLE_POINT_BYTES: u64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum GpuSampleMethod {
    Nearest = 0,
    Bilinear = 1,
    Area = 2,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct GpuSamplePoint {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) method: GpuSampleMethod,
    packed_extra: u32,
}

impl GpuSamplePoint {
    fn new(x: f32, y: f32, method: GpuSampleMethod, attenuation: u16, radius: u32) -> Self {
        let packed_radius = radius.min(u32::from(u16::MAX));
        Self {
            x,
            y,
            method,
            packed_extra: u32::from(attenuation) | (packed_radius << 16),
        }
    }

    #[cfg(test)]
    fn attenuation(self) -> u16 {
        u16::try_from(self.packed_extra & u32::from(u16::MAX)).unwrap_or(u16::MAX)
    }

    #[cfg(test)]
    fn radius(self) -> u32 {
        self.packed_extra >> 16
    }
}

#[derive(Debug, Clone)]
pub(super) struct GpuZoneRange {
    pub(super) zone_id: String,
    pub(super) start: usize,
    pub(super) len: usize,
}

#[derive(Debug, Clone)]
pub(super) struct GpuSamplingPlan {
    pub(super) points: Vec<GpuSamplePoint>,
    pub(super) zones: Vec<GpuZoneRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GpuSamplingPlanKey {
    ptr: usize,
    len: usize,
}

#[derive(Debug, Clone)]
struct CachedGpuSamplingPlan {
    key: GpuSamplingPlanKey,
    plan: GpuSamplingPlan,
    encoded_points: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UploadedGpuSamplingPlan {
    key: GpuSamplingPlanKey,
    buffer_generation: u64,
}

struct CachedGpuSamplingBindGroup {
    source_view_ptr: usize,
    buffer_generation: u64,
    bind_group: wgpu::BindGroup,
}

pub(super) struct GpuSurfaceReadback<'a> {
    pub(super) buffer: &'a wgpu::Buffer,
    pub(super) used_bytes: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) padded_bytes_per_row: u32,
    pub(super) surfaces: &'a mut RenderSurfacePool,
}

impl GpuSamplingPlan {
    pub(super) fn key(prepared_zones: &[PreparedZonePlan]) -> Option<GpuSamplingPlanKey> {
        Self::supports_prepared_zones(prepared_zones).then_some(GpuSamplingPlanKey {
            ptr: prepared_zones.as_ptr() as usize,
            len: prepared_zones.len(),
        })
    }

    pub(super) fn supports_prepared_zones(prepared_zones: &[PreparedZonePlan]) -> bool {
        prepared_zones.iter().all(|zone| {
            matches!(
                zone.sampling_method,
                SamplingMethod::Nearest | SamplingMethod::Bilinear | SamplingMethod::Area { .. }
            )
        })
    }

    pub(super) fn from_prepared_zones(prepared_zones: &[PreparedZonePlan]) -> Option<Self> {
        let total_points = prepared_zones
            .iter()
            .map(|zone| zone.sample_positions.len())
            .sum();
        let mut points = Vec::with_capacity(total_points);
        let mut zones = Vec::with_capacity(prepared_zones.len());

        for zone in prepared_zones {
            let start = points.len();
            match (&zone.sampling_method, &zone.prepared_samples) {
                (SamplingMethod::Nearest, PreparedZoneSamples::Nearest(samples)) => {
                    points.extend(
                        zone.sample_positions
                            .iter()
                            .zip(samples)
                            .map(|(position, sample)| gpu_sample_point(
                                position,
                                GpuSampleMethod::Nearest,
                                sample.attenuation,
                                0,
                            )),
                    );
                }
                (SamplingMethod::Bilinear, PreparedZoneSamples::Bilinear(samples)) => {
                    points.extend(
                        zone.sample_positions
                            .iter()
                            .zip(samples)
                            .map(|(position, sample)| gpu_sample_point(
                                position,
                                GpuSampleMethod::Bilinear,
                                sample.attenuation,
                                0,
                            )),
                    );
                }
                (SamplingMethod::Area { .. }, PreparedZoneSamples::Area(samples)) => {
                    points.extend(
                        zone.sample_positions
                            .iter()
                            .zip(samples)
                            .map(|(position, sample)| gpu_sample_point(
                                position,
                                GpuSampleMethod::Area,
                                sample.attenuation,
                                u32::try_from(sample.radius.max(0)).unwrap_or_default(),
                            )),
                    );
                }
                _ => return None,
            }
            zones.push(GpuZoneRange {
                zone_id: zone.zone_id.clone(),
                start,
                len: points.len().saturating_sub(start),
            });
        }

        Some(Self { points, zones })
    }
}

pub(super) struct GpuSpatialSampler {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    params_buffer: wgpu::Buffer,
    cached_params: Option<[u8; SAMPLE_PARAM_BYTES]>,
    points_buffer: Option<wgpu::Buffer>,
    output_buffer: Option<wgpu::Buffer>,
    readback_buffer: Option<wgpu::Buffer>,
    capacity: usize,
    buffer_generation: u64,
    cached_plan: Option<CachedGpuSamplingPlan>,
    uploaded_plan: Option<UploadedGpuSamplingPlan>,
    cached_bind_groups: Vec<CachedGpuSamplingBindGroup>,
    #[cfg(test)]
    sample_dispatch_count: usize,
    #[cfg(test)]
    sample_param_write_count: usize,
    #[cfg(test)]
    last_readback_copy_bytes: u64,
}

impl GpuSpatialSampler {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SparkleFlinger GPU sample bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(
                            wgpu::BufferSize::new(SAMPLE_PARAM_BYTES as u64)
                                .expect("sample params must be non-zero"),
                        ),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SparkleFlinger GPU sample pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU sample shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sample.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("SparkleFlinger GPU sample pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("sample_pixels"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU sample params"),
            size: SAMPLE_PARAM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            bind_group_layout,
            pipeline,
            params_buffer,
            cached_params: None,
            points_buffer: None,
            output_buffer: None,
            readback_buffer: None,
            capacity: 0,
            buffer_generation: 0,
            cached_plan: None,
            uploaded_plan: None,
            cached_bind_groups: Vec::with_capacity(2),
            #[cfg(test)]
            sample_dispatch_count: 0,
            #[cfg(test)]
            sample_param_write_count: 0,
            #[cfg(test)]
            last_readback_copy_bytes: 0,
        }
    }

    pub(super) fn sample_texture_into(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
        encoder: Option<wgpu::CommandEncoder>,
        extra_surface_readback: Option<GpuSurfaceReadback<'_>>,
    ) -> Result<(bool, Option<wgpu::SubmissionIndex>, Option<PublishedSurface>)> {
        if !self.ensure_plan(prepared_zones) {
            return Ok((false, None, None));
        }

        let sample_count = self
            .cached_plan
            .as_ref()
            .map_or(0, |cached| cached.plan.points.len());
        self.ensure_capacity(device, sample_count);
        let Some(points_buffer) = self.points_buffer.clone() else {
            zones.clear();
            return Ok((true, None, None));
        };
        let Some(output_buffer) = self.output_buffer.clone() else {
            zones.clear();
            return Ok((true, None, None));
        };
        let Some(readback_buffer) = self.readback_buffer.clone() else {
            zones.clear();
            return Ok((true, None, None));
        };

        self.ensure_points_uploaded(queue, &points_buffer);
        let params = encode_sample_params(width, height, sample_count);
        if self.cached_params != Some(params) {
            queue.write_buffer(&self.params_buffer, 0, &params);
            self.cached_params = Some(params);
            #[cfg(test)]
            {
                self.sample_param_write_count = self.sample_param_write_count.saturating_add(1);
            }
        }

        let bind_group = self.bind_group_for(device, source_view, &points_buffer, &output_buffer);

        let mut encoder = encoder.unwrap_or_else(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU sample encoder"),
            })
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SparkleFlinger GPU sample pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                u32::try_from(sample_count)
                    .unwrap_or(u32::MAX)
                    .div_ceil(SAMPLE_WORKGROUP_SIZE),
                1,
                1,
            );
        }
        #[cfg(test)]
        {
            self.sample_dispatch_count = self.sample_dispatch_count.saturating_add(1);
        }
        let output_bytes = sample_output_bytes(sample_count);
        #[cfg(test)]
        {
            self.last_readback_copy_bytes = output_bytes;
        }
        if output_bytes > 0 {
            encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback_buffer, 0, output_bytes);
        }

        let submission_index = queue.submit(Some(encoder.finish()));
        if output_bytes == 0 {
            zones.clear();
            let preview_surface = if let Some(surface_readback) = extra_surface_readback {
                Some(read_back_surface_into(
                    device,
                    surface_readback,
                    submission_index.clone(),
                )?)
            } else {
                None
            };
            return Ok((true, Some(submission_index), preview_surface));
        }
        let preview_surface = readback_zone_colors_into(
            device,
            &readback_buffer,
            output_bytes,
            self.cached_plan
                .as_ref()
                .expect("GPU sampling plan should remain cached after sampling"),
            submission_index.clone(),
            zones,
            extra_surface_readback,
        )?;
        Ok((true, Some(submission_index), preview_surface))
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, sample_count: usize) {
        if sample_count <= self.capacity {
            return;
        }

        let sample_count = sample_count.max(1);
        let point_stride = SAMPLE_POINT_BYTES;
        let output_stride = 4_u64;
        self.points_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU sample points"),
            size: point_stride * u64::try_from(sample_count).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.output_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU sample output"),
            size: output_stride * u64::try_from(sample_count).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        self.readback_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU sample readback"),
            size: output_stride * u64::try_from(sample_count).unwrap_or(u64::MAX),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
        self.capacity = sample_count;
        self.buffer_generation = self.buffer_generation.saturating_add(1);
        self.uploaded_plan = None;
        self.cached_bind_groups.clear();
    }

    fn ensure_plan(&mut self, prepared_zones: &[PreparedZonePlan]) -> bool {
        let key = GpuSamplingPlanKey {
            ptr: prepared_zones.as_ptr() as usize,
            len: prepared_zones.len(),
        };
        if self
            .cached_plan
            .as_ref()
            .is_some_and(|cached| cached.key == key)
        {
            return true;
        }

        let Some(plan) = GpuSamplingPlan::from_prepared_zones(prepared_zones) else {
            self.cached_plan = None;
            return false;
        };
        let encoded_points = encode_points(&plan);
        self.cached_plan = Some(CachedGpuSamplingPlan {
            key,
            plan,
            encoded_points,
        });
        true
    }

    fn ensure_points_uploaded(&mut self, queue: &wgpu::Queue, points_buffer: &wgpu::Buffer) {
        let cached_plan = self
            .cached_plan
            .as_ref()
            .expect("GPU sampling plan should be cached before upload");
        let upload = UploadedGpuSamplingPlan {
            key: cached_plan.key,
            buffer_generation: self.buffer_generation,
        };
        if self.uploaded_plan == Some(upload) {
            return;
        }

        queue.write_buffer(points_buffer, 0, &cached_plan.encoded_points);
        self.uploaded_plan = Some(upload);
    }

    fn bind_group_for(
        &mut self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        points_buffer: &wgpu::Buffer,
        output_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let source_view_ptr = source_view as *const wgpu::TextureView as usize;
        if let Some(cached) = self.cached_bind_groups.iter().find(|cached| {
            cached.source_view_ptr == source_view_ptr
                && cached.buffer_generation == self.buffer_generation
        }) {
            return cached.bind_group.clone();
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SparkleFlinger GPU sample bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: points_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.params_buffer.as_entire_binding(),
                },
            ],
        });
        self.cached_bind_groups.push(CachedGpuSamplingBindGroup {
            source_view_ptr,
            buffer_generation: self.buffer_generation,
            bind_group: bind_group.clone(),
        });
        bind_group
    }

    #[cfg(test)]
    pub(super) fn sample_dispatch_count(&self) -> usize {
        self.sample_dispatch_count
    }

    #[cfg(test)]
    pub(super) fn sample_param_write_count(&self) -> usize {
        self.sample_param_write_count
    }

    #[cfg(test)]
    pub(super) fn last_readback_copy_bytes(&self) -> u64 {
        self.last_readback_copy_bytes
    }
}

fn encode_points(plan: &GpuSamplingPlan) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(plan.points.len().saturating_mul(16));
    for point in &plan.points {
        bytes.extend_from_slice(&point.x.to_le_bytes());
        bytes.extend_from_slice(&point.y.to_le_bytes());
        bytes.extend_from_slice(&(point.method as u32).to_le_bytes());
        bytes.extend_from_slice(&point.packed_extra.to_le_bytes());
    }
    bytes
}

fn gpu_sample_point(
    position: &hypercolor_types::spatial::NormalizedPosition,
    method: GpuSampleMethod,
    attenuation: u16,
    radius: u32,
) -> GpuSamplePoint {
    GpuSamplePoint::new(position.x, position.y, method, attenuation, radius)
}

fn encode_sample_params(width: u32, height: u32, sample_count: usize) -> [u8; SAMPLE_PARAM_BYTES] {
    let mut bytes = [0_u8; SAMPLE_PARAM_BYTES];
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(
        &u32::try_from(sample_count)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    bytes
}

fn sample_output_bytes(sample_count: usize) -> u64 {
    u64::try_from(sample_count)
        .unwrap_or(u64::MAX)
        .saturating_mul(4)
}

fn readback_zone_colors_into(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    cached_plan: &CachedGpuSamplingPlan,
    submission_index: wgpu::SubmissionIndex,
    zones: &mut Vec<ZoneColors>,
    extra_surface_readback: Option<GpuSurfaceReadback<'_>>,
) -> Result<Option<PublishedSurface>> {
    let slice = buffer.slice(..used_bytes);
    let (sender, receiver) =
        mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    let preview_slice = extra_surface_readback
        .as_ref()
        .map(|surface_readback| surface_readback.buffer.slice(..surface_readback.used_bytes));
    let preview_receiver = if let Some(preview_slice) = preview_slice.as_ref() {
        let (sender, receiver) =
            mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
        preview_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        Some(receiver)
    } else {
        None
    };
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: None,
        })
        .context("GPU sample poll failed")?;
    receiver
        .recv()
        .context("GPU sample channel closed before map completion")?
        .context("GPU sample buffer mapping failed")?;
    if let Some(preview_receiver) = preview_receiver {
        preview_receiver
            .recv()
            .context("GPU preview channel closed before map completion")?
            .context("GPU preview buffer mapping failed")?;
    }

    let mapped = slice.get_mapped_range();
    rebuild_zone_colors_from_mapped_bytes(&cached_plan.plan, &mapped, zones);
    drop(mapped);
    buffer.unmap();
    let preview_surface = if let (Some(preview_slice), Some(surface_readback)) =
        (preview_slice, extra_surface_readback)
    {
        let mapped = preview_slice.get_mapped_range();
        let surface = copy_mapped_surface_into_pool(
            &mapped,
            surface_readback.width,
            surface_readback.height,
            surface_readback.padded_bytes_per_row,
            surface_readback.surfaces,
        )?;
        drop(mapped);
        surface_readback.buffer.unmap();
        Some(surface)
    } else {
        None
    };
    Ok(preview_surface)
}

fn read_back_surface_into(
    device: &wgpu::Device,
    surface_readback: GpuSurfaceReadback<'_>,
    submission_index: wgpu::SubmissionIndex,
) -> Result<PublishedSurface> {
    let slice = surface_readback.buffer.slice(..surface_readback.used_bytes);
    let (sender, receiver) =
        mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission_index),
            timeout: None,
        })
        .context("GPU preview poll failed")?;
    receiver
        .recv()
        .context("GPU preview channel closed before map completion")?
        .context("GPU preview buffer mapping failed")?;

    let mapped = slice.get_mapped_range();
    let surface = copy_mapped_surface_into_pool(
        &mapped,
        surface_readback.width,
        surface_readback.height,
        surface_readback.padded_bytes_per_row,
        surface_readback.surfaces,
    )?;
    drop(mapped);
    surface_readback.buffer.unmap();
    Ok(surface)
}

fn copy_mapped_surface_into_pool(
    mapped: &[u8],
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    surfaces: &mut RenderSurfacePool,
) -> Result<PublishedSurface> {
    let unpadded_bytes_per_row = width * BYTES_PER_PIXEL as u32;
    let mut lease = surfaces
        .dequeue()
        .context("GPU readback surface pool should provide a reusable slot")?;
    let target = lease.canvas_mut().as_rgba_bytes_mut();
    if padded_bytes_per_row == unpadded_bytes_per_row {
        target.copy_from_slice(
            &mapped[..usize::try_from(unpadded_bytes_per_row)
                .expect("row width should fit in usize")
                .saturating_mul(height as usize)],
        );
    } else {
        let row_width = usize::try_from(unpadded_bytes_per_row).expect("row width should fit");
        for (target_row, row) in target
            .chunks_exact_mut(row_width)
            .zip(mapped.chunks(usize::try_from(padded_bytes_per_row).expect(
                "row pitch should fit in usize",
            )))
            .take(height as usize)
        {
            target_row.copy_from_slice(&row[..row_width]);
        }
    }

    Ok(lease.submit(0, 0))
}

fn rebuild_zone_colors_from_mapped_bytes(
    plan: &GpuSamplingPlan,
    packed_bytes: &[u8],
    zones: &mut Vec<ZoneColors>,
) {
    zones.reserve(plan.zones.len().saturating_sub(zones.len()));

    for (index, zone_plan) in plan.zones.iter().enumerate() {
        if index == zones.len() {
            zones.push(ZoneColors {
                zone_id: zone_plan.zone_id.clone(),
                colors: vec![[0_u8; 3]; zone_plan.len],
            });
        }

        let zone = &mut zones[index];
        if zone.zone_id != zone_plan.zone_id {
            zone.zone_id.clone_from(&zone_plan.zone_id);
        }
        zone.colors.resize(zone_plan.len, [0_u8; 3]);
        let start = zone_plan.start.saturating_mul(4);
        let end = zone_plan
            .start
            .saturating_add(zone_plan.len)
            .saturating_mul(4);
        let packed_zone = &packed_bytes[start..end];
        for (color, packed_rgb) in zone.colors.iter_mut().zip(packed_zone.chunks_exact(4)) {
            *color = [packed_rgb[0], packed_rgb[1], packed_rgb[2]];
        }
    }

    zones.truncate(plan.zones.len());
}

#[cfg(test)]
mod tests {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
        StripDirection,
    };

    use super::{GpuSampleMethod, GpuSamplingPlan};

    fn test_layout(mode: SamplingMode) -> SpatialLayout {
        SpatialLayout {
            id: "test".into(),
            name: "Test".into(),
            description: None,
            canvas_width: 16,
            canvas_height: 16,
            zones: vec![DeviceZone {
                id: "zone".into(),
                name: "zone".into(),
                device_id: "device:zone".into(),
                zone_name: None,
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: 4,
                    direction: StripDirection::LeftToRight,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(mode),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: None,
                shape_preset: None,
                display_order: 0,
                attachment: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    #[test]
    fn gpu_sampling_plan_flattens_supported_modes() {
        let nearest = SpatialEngine::new(test_layout(SamplingMode::Nearest));
        let bilinear = SpatialEngine::new(test_layout(SamplingMode::Bilinear));
        let area = SpatialEngine::new(test_layout(SamplingMode::AreaAverage {
            radius_x: 2.0,
            radius_y: 2.0,
        }));
        let mut plans = nearest.sampling_plan().as_ref().to_vec();
        plans.extend(bilinear.sampling_plan().iter().cloned());
        plans.extend(area.sampling_plan().iter().cloned());

        let plan = GpuSamplingPlan::from_prepared_zones(&plans)
            .expect("nearest, bilinear, and area plans should be supported");
        assert_eq!(plan.zones.len(), 3);
        assert_eq!(plan.points.len(), 12);
        assert_eq!(plan.points[0].method, GpuSampleMethod::Nearest);
        assert_eq!(plan.points[4].method, GpuSampleMethod::Bilinear);
        assert_eq!(plan.points[8].method, GpuSampleMethod::Area);
        assert_eq!(plan.points[0].attenuation(), 256);
        assert_eq!(plan.points[8].radius(), 2);
    }

    #[test]
    fn gpu_sampling_plan_keeps_area_sample_radius() {
        let area = SpatialEngine::new(test_layout(SamplingMode::AreaAverage {
            radius_x: 3.0,
            radius_y: 1.0,
        }));
        let plan = GpuSamplingPlan::from_prepared_zones(area.sampling_plan().as_ref())
            .expect("area plans should stay GPU-sampleable");
        assert_eq!(plan.points[0].radius(), 3);
    }
}
