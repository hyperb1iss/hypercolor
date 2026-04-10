use std::sync::mpsc;

use anyhow::{Context, Result};
use hypercolor_core::spatial::PreparedZonePlan;
use hypercolor_types::canvas::SamplingMethod;
use hypercolor_types::event::ZoneColors;

const SAMPLE_WORKGROUP_SIZE: u32 = 64;
const SAMPLE_PARAM_BYTES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum GpuSampleMethod {
    Nearest = 0,
    Bilinear = 1,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct GpuSamplePoint {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) method: GpuSampleMethod,
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

impl GpuSamplingPlan {
    pub(super) fn from_prepared_zones(prepared_zones: &[PreparedZonePlan]) -> Option<Self> {
        let total_points = prepared_zones
            .iter()
            .map(|zone| zone.sample_positions.len())
            .sum();
        let mut points = Vec::with_capacity(total_points);
        let mut zones = Vec::with_capacity(prepared_zones.len());

        for zone in prepared_zones {
            let method = match zone.sampling_method {
                SamplingMethod::Nearest => GpuSampleMethod::Nearest,
                SamplingMethod::Bilinear => GpuSampleMethod::Bilinear,
                SamplingMethod::Area { .. } => return None,
            };
            let start = points.len();
            points.extend(zone.sample_positions.iter().map(|position| GpuSamplePoint {
                x: position.x,
                y: position.y,
                method,
            }));
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
    points_buffer: Option<wgpu::Buffer>,
    output_buffer: Option<wgpu::Buffer>,
    readback_buffer: Option<wgpu::Buffer>,
    capacity: usize,
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
            points_buffer: None,
            output_buffer: None,
            readback_buffer: None,
            capacity: 0,
        }
    }

    pub(super) fn sample_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        plan: &GpuSamplingPlan,
    ) -> Result<Vec<ZoneColors>> {
        self.ensure_capacity(device, plan.points.len());
        let Some(points_buffer) = &self.points_buffer else {
            return Ok(Vec::new());
        };
        let Some(output_buffer) = &self.output_buffer else {
            return Ok(Vec::new());
        };
        let Some(readback_buffer) = &self.readback_buffer else {
            return Ok(Vec::new());
        };

        queue.write_buffer(points_buffer, 0, &encode_points(plan));
        queue.write_buffer(
            &self.params_buffer,
            0,
            &encode_sample_params(width, height, plan.points.len()),
        );

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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SparkleFlinger GPU sample encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SparkleFlinger GPU sample pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                u32::try_from(plan.points.len())
                    .unwrap_or(u32::MAX)
                    .div_ceil(SAMPLE_WORKGROUP_SIZE),
                1,
                1,
            );
        }
        encoder.copy_buffer_to_buffer(output_buffer, 0, readback_buffer, 0, output_buffer.size());

        let packed = readback_samples(
            device,
            readback_buffer,
            plan.points.len(),
            queue.submit(Some(encoder.finish())),
        )?;
        Ok(rebuild_zone_colors(plan, &packed))
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, sample_count: usize) {
        if sample_count <= self.capacity {
            return;
        }

        let sample_count = sample_count.max(1);
        let point_stride = 16_u64;
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
    }
}

fn encode_points(plan: &GpuSamplingPlan) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(plan.points.len().saturating_mul(16));
    for point in &plan.points {
        bytes.extend_from_slice(&point.x.to_le_bytes());
        bytes.extend_from_slice(&point.y.to_le_bytes());
        bytes.extend_from_slice(&(point.method as u32).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
    }
    bytes
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

fn readback_samples(
    device: &wgpu::Device,
    buffer: &wgpu::Buffer,
    sample_count: usize,
    submission_index: wgpu::SubmissionIndex,
) -> Result<Vec<u32>> {
    let slice = buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
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

    let mapped = slice.get_mapped_range();
    let mut packed = Vec::with_capacity(sample_count);
    for chunk in mapped.chunks_exact(4).take(sample_count) {
        packed.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    drop(mapped);
    buffer.unmap();
    Ok(packed)
}

fn rebuild_zone_colors(plan: &GpuSamplingPlan, packed: &[u32]) -> Vec<ZoneColors> {
    plan.zones
        .iter()
        .map(|zone| ZoneColors {
            zone_id: zone.zone_id.clone(),
            colors: packed[zone.start..zone.start.saturating_add(zone.len)]
                .iter()
                .map(|packed| {
                    [
                        u8::try_from(packed & 0xff).expect("red channel fits"),
                        u8::try_from((packed >> 8) & 0xff).expect("green channel fits"),
                        u8::try_from((packed >> 16) & 0xff).expect("blue channel fits"),
                    ]
                })
                .collect(),
        })
        .collect()
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
        let mut plans = nearest.sampling_plan().as_ref().to_vec();
        plans.extend(bilinear.sampling_plan().iter().cloned());

        let plan = GpuSamplingPlan::from_prepared_zones(&plans)
            .expect("nearest and bilinear plans should be supported");
        assert_eq!(plan.zones.len(), 2);
        assert_eq!(plan.points.len(), 8);
        assert_eq!(plan.points[0].method, GpuSampleMethod::Nearest);
        assert_eq!(plan.points[4].method, GpuSampleMethod::Bilinear);
    }

    #[test]
    fn gpu_sampling_plan_rejects_area_sampling() {
        let area = SpatialEngine::new(test_layout(SamplingMode::AreaAverage {
            radius_x: 2.0,
            radius_y: 2.0,
        }));
        assert!(GpuSamplingPlan::from_prepared_zones(area.sampling_plan().as_ref()).is_none());
    }
}
