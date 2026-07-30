use anyhow::{Context, Result};

use super::gpu_sampling::GpuSampleSource;

const SAT_WORKGROUP_SIZE: u32 = 256;
const SAT_VALUE_BYTES: u64 = 16;
const SAT_PARAM_BYTES: u64 = 16;

pub(super) struct GpuAreaPipeline {
    bind_group_layout: wgpu::BindGroupLayout,
    horizontal_tiles: wgpu::ComputePipeline,
    horizontal_blocks: wgpu::ComputePipeline,
    horizontal_add: wgpu::ComputePipeline,
    vertical_tiles: wgpu::ComputePipeline,
    vertical_blocks: wgpu::ComputePipeline,
    vertical_add: wgpu::ComputePipeline,
}

pub(super) struct GpuAreaResources {
    width: u32,
    height: u32,
    horizontal_block_count: u32,
    vertical_block_count: u32,
    values_a: wgpu::Buffer,
    values_b: wgpu::Buffer,
    horizontal_sums: wgpu::Buffer,
    vertical_sums: wgpu::Buffer,
    params: wgpu::Buffer,
    bind_groups: [Option<wgpu::BindGroup>; 2],
}

impl GpuAreaPipeline {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SparkleFlinger GPU area SAT bind group layout"),
            entries: &[
                texture_entry(0),
                storage_entry(1),
                storage_entry(2),
                storage_entry(3),
                storage_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(SAT_PARAM_BYTES),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SparkleFlinger GPU area SAT pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU area SAT shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("area_sat.wgsl").into()),
        });
        let create_pipeline = |label, entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };

        Self {
            bind_group_layout,
            horizontal_tiles: create_pipeline(
                "SparkleFlinger GPU area horizontal tile scan",
                "scan_horizontal_tiles",
            ),
            horizontal_blocks: create_pipeline(
                "SparkleFlinger GPU area horizontal block scan",
                "scan_horizontal_blocks",
            ),
            horizontal_add: create_pipeline(
                "SparkleFlinger GPU area horizontal block add",
                "add_horizontal_blocks",
            ),
            vertical_tiles: create_pipeline(
                "SparkleFlinger GPU area vertical tile scan",
                "scan_vertical_tiles",
            ),
            vertical_blocks: create_pipeline(
                "SparkleFlinger GPU area vertical block scan",
                "scan_vertical_blocks",
            ),
            vertical_add: create_pipeline(
                "SparkleFlinger GPU area vertical block add",
                "add_vertical_blocks",
            ),
        }
    }

    pub(super) fn try_prepare(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> Result<GpuAreaResources> {
        let geometry = GpuAreaGeometry::try_new(device.limits(), width, height)?;
        let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let resources = GpuAreaResources::new(device, geometry);
        let validation_error = pollster::block_on(validation_scope.pop());
        let internal_error = pollster::block_on(internal_scope.pop());
        let out_of_memory_error = pollster::block_on(out_of_memory_scope.pop());
        if let Some(error) = validation_error.or(internal_error).or(out_of_memory_error) {
            anyhow::bail!("GPU area summed-area resources could not be admitted: {error}");
        }
        Ok(resources)
    }

    pub(super) fn encode(
        &self,
        device: &wgpu::Device,
        source: GpuSampleSource,
        source_view: &wgpu::TextureView,
        resources: &mut GpuAreaResources,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let bind_group = resources.bind_group(device, &self.bind_group_layout, source, source_view);
        encode_pass(
            encoder,
            "SparkleFlinger GPU area horizontal tile scan",
            &self.horizontal_tiles,
            &bind_group,
            resources.horizontal_block_count,
            resources.height,
        );
        encode_pass(
            encoder,
            "SparkleFlinger GPU area horizontal block scan",
            &self.horizontal_blocks,
            &bind_group,
            1,
            resources.height,
        );
        encode_pass(
            encoder,
            "SparkleFlinger GPU area horizontal block add",
            &self.horizontal_add,
            &bind_group,
            resources.horizontal_block_count,
            resources.height,
        );
        encode_pass(
            encoder,
            "SparkleFlinger GPU area vertical tile scan",
            &self.vertical_tiles,
            &bind_group,
            resources.width,
            resources.vertical_block_count,
        );
        encode_pass(
            encoder,
            "SparkleFlinger GPU area vertical block scan",
            &self.vertical_blocks,
            &bind_group,
            resources.width,
            1,
        );
        encode_pass(
            encoder,
            "SparkleFlinger GPU area vertical block add",
            &self.vertical_add,
            &bind_group,
            resources.width,
            resources.vertical_block_count,
        );
    }
}

impl GpuAreaResources {
    fn new(device: &wgpu::Device, geometry: GpuAreaGeometry) -> Self {
        let values_a = storage_buffer(
            device,
            "SparkleFlinger GPU area SAT A",
            geometry.value_bytes,
        );
        let values_b = storage_buffer(
            device,
            "SparkleFlinger GPU area SAT B",
            geometry.value_bytes,
        );
        let horizontal_sums = storage_buffer(
            device,
            "SparkleFlinger GPU area horizontal block sums",
            geometry.horizontal_sum_bytes,
        );
        let vertical_sums = storage_buffer(
            device,
            "SparkleFlinger GPU area vertical block sums",
            geometry.vertical_sum_bytes,
        );
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU area SAT params"),
            size: SAT_PARAM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        params
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(&encode_params(
                geometry.width,
                geometry.height,
                geometry.horizontal_block_count,
                geometry.vertical_block_count,
            ));
        params.unmap();

        Self {
            width: geometry.width,
            height: geometry.height,
            horizontal_block_count: geometry.horizontal_block_count,
            vertical_block_count: geometry.vertical_block_count,
            values_a,
            values_b,
            horizontal_sums,
            vertical_sums,
            params,
            bind_groups: std::array::from_fn(|_| None),
        }
    }

    pub(super) const fn matches(&self, width: u32, height: u32) -> bool {
        self.width == width && self.height == height
    }

    pub(super) fn summed_area_buffer(&self) -> &wgpu::Buffer {
        &self.values_b
    }

    pub(super) fn clear_bind_groups(&mut self) {
        self.bind_groups.fill(None);
    }

    fn bind_group(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        source: GpuSampleSource,
        source_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let index = source.index();
        if let Some(bind_group) = &self.bind_groups[index] {
            return bind_group.clone();
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SparkleFlinger GPU area SAT bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                buffer_entry(1, &self.values_a),
                buffer_entry(2, &self.values_b),
                buffer_entry(3, &self.horizontal_sums),
                buffer_entry(4, &self.vertical_sums),
                buffer_entry(5, &self.params),
            ],
        });
        self.bind_groups[index] = Some(bind_group.clone());
        bind_group
    }
}

#[derive(Debug, Clone, Copy)]
struct GpuAreaGeometry {
    width: u32,
    height: u32,
    horizontal_block_count: u32,
    vertical_block_count: u32,
    value_bytes: u64,
    horizontal_sum_bytes: u64,
    vertical_sum_bytes: u64,
}

impl GpuAreaGeometry {
    fn try_new(limits: wgpu::Limits, width: u32, height: u32) -> Result<Self> {
        anyhow::ensure!(width > 0 && height > 0, "GPU area canvas must be non-empty");
        anyhow::ensure!(
            limits.max_compute_workgroup_size_x >= SAT_WORKGROUP_SIZE
                && limits.max_compute_invocations_per_workgroup >= SAT_WORKGROUP_SIZE,
            "GPU area scan requires {SAT_WORKGROUP_SIZE} compute invocations per workgroup"
        );
        let horizontal_block_count = width.div_ceil(SAT_WORKGROUP_SIZE);
        let vertical_block_count = height.div_ceil(SAT_WORKGROUP_SIZE);
        anyhow::ensure!(
            horizontal_block_count <= SAT_WORKGROUP_SIZE
                && vertical_block_count <= SAT_WORKGROUP_SIZE,
            "GPU area scan supports dimensions up to {} pixels per axis",
            SAT_WORKGROUP_SIZE * SAT_WORKGROUP_SIZE
        );
        let max_dispatch = limits.max_compute_workgroups_per_dimension;
        anyhow::ensure!(
            width <= max_dispatch
                && height <= max_dispatch
                && horizontal_block_count <= max_dispatch
                && vertical_block_count <= max_dispatch,
            "GPU area scan dispatch exceeds the device workgroup grid limit"
        );

        let value_bytes = checked_buffer_bytes(width, height, "summed-area values")?;
        let horizontal_sum_bytes =
            checked_buffer_bytes(horizontal_block_count, height, "horizontal block sums")?;
        let vertical_sum_bytes =
            checked_buffer_bytes(width, vertical_block_count, "vertical block sums")?;
        for (name, bytes) in [
            ("summed-area values", value_bytes),
            ("horizontal block sums", horizontal_sum_bytes),
            ("vertical block sums", vertical_sum_bytes),
        ] {
            anyhow::ensure!(
                bytes <= limits.max_buffer_size,
                "GPU area {name} require {bytes} bytes but max_buffer_size is {}",
                limits.max_buffer_size
            );
            anyhow::ensure!(
                bytes <= limits.max_storage_buffer_binding_size,
                "GPU area {name} require {bytes} bytes but max_storage_buffer_binding_size is {}",
                limits.max_storage_buffer_binding_size
            );
        }

        Ok(Self {
            width,
            height,
            horizontal_block_count,
            vertical_block_count,
            value_bytes,
            horizontal_sum_bytes,
            vertical_sum_bytes,
        })
    }
}

fn checked_buffer_bytes(width: u32, height: u32, name: &str) -> Result<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|values| values.checked_mul(SAT_VALUE_BYTES))
        .with_context(|| format!("GPU area {name} byte size overflowed"))
}

fn storage_buffer(device: &wgpu::Device, label: &'static str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    })
}

const fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

const fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn buffer_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn encode_params(
    width: u32,
    height: u32,
    horizontal_blocks: u32,
    vertical_blocks: u32,
) -> [u8; SAT_PARAM_BYTES as usize] {
    let mut bytes = [0_u8; SAT_PARAM_BYTES as usize];
    bytes[0..4].copy_from_slice(&width.to_le_bytes());
    bytes[4..8].copy_from_slice(&height.to_le_bytes());
    bytes[8..12].copy_from_slice(&horizontal_blocks.to_le_bytes());
    bytes[12..16].copy_from_slice(&vertical_blocks.to_le_bytes());
    bytes
}

fn encode_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &'static str,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    x: u32,
    y: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.dispatch_workgroups(x, y, 1);
}

#[cfg(test)]
mod tests {
    use super::{GpuAreaGeometry, SAT_VALUE_BYTES, SAT_WORKGROUP_SIZE};

    #[test]
    fn geometry_work_is_independent_of_sampling_radius() {
        let limits = wgpu::Limits {
            max_buffer_size: u64::MAX,
            max_storage_buffer_binding_size: u64::MAX,
            max_compute_workgroup_size_x: SAT_WORKGROUP_SIZE,
            max_compute_invocations_per_workgroup: SAT_WORKGROUP_SIZE,
            max_compute_workgroups_per_dimension: u32::MAX,
            ..wgpu::Limits::default()
        };
        let geometry = GpuAreaGeometry::try_new(limits, 5120, 2160)
            .expect("admitted geometry should not depend on an Area radius");

        assert_eq!(geometry.horizontal_block_count, 20);
        assert_eq!(geometry.vertical_block_count, 9);
        assert_eq!(geometry.value_bytes, 5120 * 2160 * SAT_VALUE_BYTES);
    }
}
