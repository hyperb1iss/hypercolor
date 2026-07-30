use anyhow::{Context, Result};

use super::gpu_sampling::GpuSampleSource;

const SAT_WORKGROUP_SIZE: u32 = 256;
pub(super) const SAT_VALUE_BYTES: u64 = 24;
const SAT_PARAM_BYTES: u64 = 16;
const HIERARCHY_PARAM_BYTES: u64 = 16;

pub(super) struct GpuAreaPipeline {
    bind_group_layout: wgpu::BindGroupLayout,
    hierarchy_bind_group_layout: wgpu::BindGroupLayout,
    horizontal_tiles: wgpu::ComputePipeline,
    horizontal_add: wgpu::ComputePipeline,
    vertical_tiles: wgpu::ComputePipeline,
    vertical_add: wgpu::ComputePipeline,
    hierarchy_scan: wgpu::ComputePipeline,
    hierarchy_add: wgpu::ComputePipeline,
}

pub(super) struct GpuAreaResources {
    width: u32,
    height: u32,
    horizontal_block_count: u32,
    vertical_block_count: u32,
    values: wgpu::Buffer,
    horizontal_sums: GpuAreaHierarchy,
    vertical_sums: GpuAreaHierarchy,
    params: wgpu::Buffer,
    bind_groups: [Option<wgpu::BindGroup>; 2],
}

struct GpuAreaHierarchy {
    buffer: wgpu::Buffer,
    segment_count: u32,
    levels: Vec<GpuAreaHierarchyLevel>,
}

struct GpuAreaHierarchyLevel {
    block_count: u32,
    _params: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
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
        let hierarchy_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SparkleFlinger GPU area hierarchy bind group layout"),
                entries: &[
                    storage_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(HIERARCHY_PARAM_BYTES),
                        },
                        count: None,
                    },
                ],
            });
        let hierarchy_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("SparkleFlinger GPU area hierarchy pipeline layout"),
                bind_group_layouts: &[Some(&hierarchy_bind_group_layout)],
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
        let hierarchy_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SparkleFlinger GPU area hierarchy shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("area_hierarchy.wgsl").into()),
        });
        let create_hierarchy_pipeline = |label, entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&hierarchy_pipeline_layout),
                module: &hierarchy_shader,
                entry_point: Some(entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };

        Self {
            bind_group_layout,
            hierarchy_bind_group_layout,
            horizontal_tiles: create_pipeline(
                "SparkleFlinger GPU area horizontal tile scan",
                "scan_horizontal_tiles",
            ),
            horizontal_add: create_pipeline(
                "SparkleFlinger GPU area horizontal block add",
                "add_horizontal_blocks",
            ),
            vertical_tiles: create_pipeline(
                "SparkleFlinger GPU area vertical tile scan",
                "scan_vertical_tiles",
            ),
            vertical_add: create_pipeline(
                "SparkleFlinger GPU area vertical block add",
                "add_vertical_blocks",
            ),
            hierarchy_scan: create_hierarchy_pipeline(
                "SparkleFlinger GPU area hierarchy scan",
                "scan_sum_tiles",
            ),
            hierarchy_add: create_hierarchy_pipeline(
                "SparkleFlinger GPU area hierarchy add",
                "add_sum_offsets",
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
        let resources = GpuAreaResources::new(device, &self.hierarchy_bind_group_layout, geometry);
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
        encode_hierarchy(
            encoder,
            &self.hierarchy_scan,
            &self.hierarchy_add,
            &resources.horizontal_sums,
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
        encode_hierarchy(
            encoder,
            &self.hierarchy_scan,
            &self.hierarchy_add,
            &resources.vertical_sums,
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
    fn new(
        device: &wgpu::Device,
        hierarchy_layout: &wgpu::BindGroupLayout,
        geometry: GpuAreaGeometry,
    ) -> Self {
        let values = storage_buffer(
            device,
            "SparkleFlinger GPU area SAT values",
            geometry.value_bytes,
        );
        let horizontal_sums = GpuAreaHierarchy::new(
            device,
            hierarchy_layout,
            "SparkleFlinger GPU area horizontal block sums",
            geometry.horizontal_hierarchy,
        );
        let vertical_sums = GpuAreaHierarchy::new(
            device,
            hierarchy_layout,
            "SparkleFlinger GPU area vertical block sums",
            geometry.vertical_hierarchy,
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
            values,
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
        &self.values
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
                buffer_entry(1, &self.values),
                buffer_entry(2, &self.horizontal_sums.buffer),
                buffer_entry(3, &self.vertical_sums.buffer),
                buffer_entry(4, &self.params),
            ],
        });
        self.bind_groups[index] = Some(bind_group.clone());
        bind_group
    }
}

impl GpuAreaHierarchy {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        label: &'static str,
        geometry: GpuAreaHierarchyGeometry,
    ) -> Self {
        let buffer = storage_buffer(device, label, geometry.byte_len);
        let levels = geometry
            .levels
            .into_iter()
            .map(|level| {
                let params = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger GPU area hierarchy params"),
                    size: HIERARCHY_PARAM_BYTES,
                    usage: wgpu::BufferUsages::UNIFORM,
                    mapped_at_creation: true,
                });
                params
                    .slice(..)
                    .get_mapped_range_mut()
                    .copy_from_slice(&encode_hierarchy_params(level));
                params.unmap();
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("SparkleFlinger GPU area hierarchy bind group"),
                    layout,
                    entries: &[buffer_entry(0, &buffer), buffer_entry(1, &params)],
                });
                GpuAreaHierarchyLevel {
                    block_count: level.block_count,
                    _params: params,
                    bind_group,
                }
            })
            .collect();
        Self {
            buffer,
            segment_count: geometry.segment_count,
            levels,
        }
    }
}

#[derive(Debug, Clone)]
struct GpuAreaGeometry {
    width: u32,
    height: u32,
    horizontal_block_count: u32,
    vertical_block_count: u32,
    value_bytes: u64,
    horizontal_hierarchy: GpuAreaHierarchyGeometry,
    vertical_hierarchy: GpuAreaHierarchyGeometry,
}

#[derive(Debug, Clone)]
struct GpuAreaHierarchyGeometry {
    segment_count: u32,
    byte_len: u64,
    levels: Vec<GpuAreaHierarchyLevelGeometry>,
}

#[derive(Debug, Clone, Copy)]
struct GpuAreaHierarchyLevelGeometry {
    offset: u32,
    next_offset: u32,
    length: u32,
    next_length: u32,
    block_count: u32,
}

impl GpuAreaGeometry {
    fn try_new(limits: wgpu::Limits, width: u32, height: u32) -> Result<Self> {
        anyhow::ensure!(width > 0 && height > 0, "GPU area canvas must be non-empty");
        anyhow::ensure!(
            width <= limits.max_texture_dimension_2d && height <= limits.max_texture_dimension_2d,
            "GPU area canvas {width}x{height} exceeds max_texture_dimension_2d {}",
            limits.max_texture_dimension_2d
        );
        anyhow::ensure!(
            limits.max_compute_workgroup_size_x >= SAT_WORKGROUP_SIZE
                && limits.max_compute_invocations_per_workgroup >= SAT_WORKGROUP_SIZE,
            "GPU area scan requires {SAT_WORKGROUP_SIZE} compute invocations per workgroup"
        );
        let horizontal_block_count = width.div_ceil(SAT_WORKGROUP_SIZE);
        let vertical_block_count = height.div_ceil(SAT_WORKGROUP_SIZE);
        let max_dispatch = limits.max_compute_workgroups_per_dimension;
        anyhow::ensure!(
            width <= max_dispatch
                && height <= max_dispatch
                && horizontal_block_count <= max_dispatch
                && vertical_block_count <= max_dispatch,
            "GPU area scan dispatch exceeds the device workgroup grid limit"
        );
        let pixel_count = u64::from(width)
            .checked_mul(u64::from(height))
            .context("GPU area pixel count overflowed")?;
        anyhow::ensure!(
            u32::try_from(pixel_count).is_ok(),
            "GPU area pixel count {pixel_count} exceeds u32 shader indexing"
        );
        pixel_count
            .checked_mul(u64::from(u16::MAX))
            .context("GPU area accumulator range overflowed")?;

        let value_bytes = checked_entry_bytes(pixel_count, "summed-area values")?;
        let horizontal_hierarchy = GpuAreaHierarchyGeometry::try_new(
            horizontal_block_count,
            height,
            max_dispatch,
            "horizontal block sums",
        )?;
        let vertical_hierarchy = GpuAreaHierarchyGeometry::try_new(
            vertical_block_count,
            width,
            max_dispatch,
            "vertical block sums",
        )?;
        for (name, bytes) in [
            ("summed-area values", value_bytes),
            ("horizontal block sums", horizontal_hierarchy.byte_len),
            ("vertical block sums", vertical_hierarchy.byte_len),
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
            horizontal_hierarchy,
            vertical_hierarchy,
        })
    }
}

impl GpuAreaHierarchyGeometry {
    fn try_new(
        initial_length: u32,
        segment_count: u32,
        max_dispatch: u32,
        name: &str,
    ) -> Result<Self> {
        let mut levels = Vec::new();
        let mut entry_count = 0_u64;
        let mut length = initial_length;
        loop {
            let block_count = length.div_ceil(SAT_WORKGROUP_SIZE);
            anyhow::ensure!(
                block_count <= max_dispatch && segment_count <= max_dispatch,
                "GPU area {name} hierarchy dispatch exceeds the device workgroup grid limit"
            );
            let level_entries = u64::from(segment_count)
                .checked_mul(u64::from(length))
                .with_context(|| format!("GPU area {name} hierarchy entry count overflowed"))?;
            let next_entry_count = entry_count
                .checked_add(level_entries)
                .with_context(|| format!("GPU area {name} hierarchy entry count overflowed"))?;
            anyhow::ensure!(
                u32::try_from(next_entry_count).is_ok(),
                "GPU area {name} hierarchy exceeds u32 shader indexing"
            );
            let next_length = (length > 1).then(|| length.div_ceil(SAT_WORKGROUP_SIZE));
            levels.push(GpuAreaHierarchyLevelGeometry {
                offset: u32::try_from(entry_count).context("hierarchy offset exceeds u32")?,
                next_offset: next_length.map_or(0, |_| {
                    u32::try_from(next_entry_count).expect("checked hierarchy offset fits u32")
                }),
                length,
                next_length: next_length.unwrap_or(0),
                block_count,
            });
            entry_count = next_entry_count;
            let Some(next_length) = next_length else {
                break;
            };
            length = next_length;
        }
        let entry_count =
            u32::try_from(entry_count).context("hierarchy entry count exceeds u32")?;
        Ok(Self {
            segment_count,
            byte_len: checked_entry_bytes(u64::from(entry_count), name)?,
            levels,
        })
    }
}

fn checked_entry_bytes(entry_count: u64, name: &str) -> Result<u64> {
    entry_count
        .checked_mul(SAT_VALUE_BYTES)
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

fn encode_hierarchy_params(
    level: GpuAreaHierarchyLevelGeometry,
) -> [u8; HIERARCHY_PARAM_BYTES as usize] {
    let mut bytes = [0_u8; HIERARCHY_PARAM_BYTES as usize];
    bytes[0..4].copy_from_slice(&level.offset.to_le_bytes());
    bytes[4..8].copy_from_slice(&level.next_offset.to_le_bytes());
    bytes[8..12].copy_from_slice(&level.length.to_le_bytes());
    bytes[12..16].copy_from_slice(&level.next_length.to_le_bytes());
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

fn encode_hierarchy(
    encoder: &mut wgpu::CommandEncoder,
    scan_pipeline: &wgpu::ComputePipeline,
    add_pipeline: &wgpu::ComputePipeline,
    hierarchy: &GpuAreaHierarchy,
) {
    for level in &hierarchy.levels {
        encode_pass(
            encoder,
            "SparkleFlinger GPU area hierarchy tile scan",
            scan_pipeline,
            &level.bind_group,
            level.block_count,
            hierarchy.segment_count,
        );
    }
    for level in hierarchy.levels.iter().rev().skip(1) {
        encode_pass(
            encoder,
            "SparkleFlinger GPU area hierarchy offset add",
            add_pipeline,
            &level.bind_group,
            level.block_count,
            hierarchy.segment_count,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuAreaGeometry, SAT_VALUE_BYTES, SAT_WORKGROUP_SIZE};

    fn synthetic_limits() -> wgpu::Limits {
        wgpu::Limits {
            max_texture_dimension_2d: u32::MAX,
            max_buffer_size: u64::MAX,
            max_storage_buffer_binding_size: u64::MAX,
            max_compute_workgroup_size_x: SAT_WORKGROUP_SIZE,
            max_compute_invocations_per_workgroup: SAT_WORKGROUP_SIZE,
            max_compute_workgroups_per_dimension: u32::MAX,
            ..wgpu::Limits::default()
        }
    }

    #[derive(Clone, Copy)]
    struct Wide64 {
        lo: u32,
        hi: u32,
    }

    impl Wide64 {
        fn from_u64(value: u64) -> Self {
            Self {
                lo: u32::try_from(value & u64::from(u32::MAX)).expect("masked low limb fits u32"),
                hi: u32::try_from(value >> 32).expect("high limb fits u32"),
            }
        }

        fn as_u64(self) -> u64 {
            u64::from(self.lo) | (u64::from(self.hi) << 32)
        }

        fn subtract(self, right: Self) -> Self {
            let (lo, borrow) = self.lo.overflowing_sub(right.lo);
            Self {
                lo,
                hi: self.hi - right.hi - u32::from(borrow),
            }
        }
    }

    #[test]
    fn geometry_work_is_independent_of_sampling_radius() {
        let geometry = GpuAreaGeometry::try_new(synthetic_limits(), 5120, 2160)
            .expect("admitted geometry should not depend on an Area radius");

        assert_eq!(geometry.horizontal_block_count, 20);
        assert_eq!(geometry.vertical_block_count, 9);
        assert_eq!(geometry.value_bytes, 5120 * 2160 * SAT_VALUE_BYTES);
    }

    #[test]
    fn geometry_accepts_axes_above_the_former_two_level_limit() {
        let geometry = GpuAreaGeometry::try_new(synthetic_limits(), 65_537, 1)
            .expect("hierarchical scans must not impose an axis ceiling");

        assert_eq!(geometry.horizontal_block_count, 257);
        let levels = &geometry.horizontal_hierarchy.levels;
        assert_eq!(levels.len(), 3);
        assert_eq!((levels[0].offset, levels[0].length), (0, 257));
        assert_eq!((levels[1].offset, levels[1].length), (257, 2));
        assert_eq!((levels[2].offset, levels[2].length), (259, 1));
        assert_eq!(
            geometry.horizontal_hierarchy.byte_len,
            260 * SAT_VALUE_BYTES
        );
    }

    #[test]
    fn geometry_rejects_pixel_counts_above_u32_shader_indexing() {
        let error = GpuAreaGeometry::try_new(synthetic_limits(), 65_536, 65_536)
            .expect_err("width times height must remain u32-addressable in WGSL");

        assert!(error.to_string().contains("exceeds u32 shader indexing"));
    }

    #[test]
    fn exact_limbs_preserve_one_pixel_at_8k_prefix_magnitude() {
        let width = 7680_u64;
        let height = 4320_u64;
        let value = u64::from(u16::MAX);
        let bottom_right = Wide64::from_u64(width * height * value);
        let bottom_left = Wide64::from_u64((width - 1) * height * value);
        let top_right = Wide64::from_u64(width * (height - 1) * value);
        let top_left = Wide64::from_u64((width - 1) * (height - 1) * value);

        let exact = bottom_right
            .subtract(bottom_left)
            .subtract(top_right.subtract(top_left))
            .as_u64();
        assert_eq!(exact, value);
    }
}
