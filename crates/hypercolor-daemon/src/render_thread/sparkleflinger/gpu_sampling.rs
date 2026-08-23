use std::sync::{
    Arc,
    mpsc::{self, TryRecvError},
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hypercolor_core::spatial::{PreparedZonePlan, PreparedZoneSamples};
use hypercolor_types::event::ZoneColors;
use hypercolor_types::spatial::SamplingMode;

use super::gpu_area_sat::{GpuAreaPipeline, GpuAreaResources, SAT_VALUE_BYTES};

const SAMPLE_WORKGROUP_SIZE: u32 = 64;
const SAMPLE_PARAM_BYTES: usize = 16;
const SAMPLE_POINT_BYTES: u64 = 32;
const SAMPLE_READBACK_SLOT_COUNT: usize = 3;
const SAMPLE_READBACK_WAIT_STEP: Duration = Duration::from_millis(50);
const SAMPLE_READBACK_WAIT_BUDGET: Duration = Duration::from_secs(2);
const SAMPLE_PREPARATION_RETRY_INITIAL: Duration = Duration::from_millis(25);
const SAMPLE_PREPARATION_RETRY_MAX: Duration = Duration::from_secs(1);

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
    attenuation: u32,
    center_x: u32,
    center_y: u32,
    radius_x: u32,
    radius_y: u32,
}

impl GpuSamplePoint {
    fn new(
        x: f32,
        y: f32,
        method: GpuSampleMethod,
        attenuation: u16,
        area: Option<(u32, u32, u32, u32)>,
    ) -> Self {
        let (center_x, center_y, radius_x, radius_y) = area.unwrap_or_default();
        Self {
            x,
            y,
            method,
            attenuation: u32::from(attenuation),
            center_x,
            center_y,
            radius_x,
            radius_y,
        }
    }

    #[cfg(test)]
    fn attenuation(self) -> u16 {
        u16::try_from(self.attenuation).unwrap_or(u16::MAX)
    }

    #[cfg(test)]
    const fn radius_x(self) -> u32 {
        self.radius_x
    }

    #[cfg(test)]
    const fn radius_y(self) -> u32 {
        self.radius_y
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
    pub(super) zones: Arc<[GpuZoneRange]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct GpuSamplingPlanKey {
    ptr: usize,
    len: usize,
    generation: u64,
}

#[derive(Debug, Clone)]
struct CachedGpuSamplingPlan {
    key: GpuSamplingPlanKey,
    plan: GpuSamplingPlan,
    encoded_points: Vec<u8>,
    dispatch_workgroups: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UploadedGpuSamplingPlan {
    key: GpuSamplingPlanKey,
    buffer_generation: u64,
}

struct CachedGpuSamplingBindGroup {
    source: GpuSampleSource,
    buffer_generation: u64,
    area_generation: u64,
    bind_group: wgpu::BindGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GpuSamplingAdmissionKey {
    plan: GpuSamplingPlanKey,
    width: u32,
    height: u32,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum GpuSamplingPreparationFailure {
    #[error("{0}")]
    Deterministic(String),
    #[error("{0}")]
    Transient(String),
}

impl GpuSamplingPreparationFailure {
    pub(super) fn deterministic(reason: impl Into<String>) -> Self {
        Self::Deterministic(reason.into())
    }

    pub(super) fn transient(reason: impl Into<String>) -> Self {
        Self::Transient(reason.into())
    }

    const fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

#[derive(Debug, Clone)]
struct GpuSamplingRetry {
    admission: GpuSamplingAdmissionKey,
    failure_count: u32,
    next_attempt_at: Instant,
    reason: String,
}

impl GpuSamplingRetry {
    fn after_failure(
        admission: GpuSamplingAdmissionKey,
        reason: String,
        previous: Option<&Self>,
    ) -> Self {
        let failure_count = previous
            .filter(|retry| retry.admission == admission)
            .map_or(1, |retry| retry.failure_count.saturating_add(1));
        let shift = failure_count.saturating_sub(1).min(31);
        let multiplier = 1_u32 << shift;
        let delay = SAMPLE_PREPARATION_RETRY_INITIAL
            .saturating_mul(multiplier)
            .min(SAMPLE_PREPARATION_RETRY_MAX);
        Self {
            admission,
            failure_count,
            next_attempt_at: Instant::now() + delay,
            reason,
        }
    }

    fn is_due(&self) -> bool {
        Instant::now() >= self.next_attempt_at
    }
}

struct PreparedGpuSampleBuffers {
    points: wgpu::Buffer,
    output: wgpu::Buffer,
    readbacks: [wgpu::Buffer; SAMPLE_READBACK_SLOT_COUNT],
    capacity: usize,
}

#[derive(Debug, Clone, Copy)]
struct GpuSampleGeometry {
    sample_count: usize,
    point_bytes: u64,
    output_bytes: u64,
    dispatch_workgroups: u32,
}

impl GpuSampleGeometry {
    fn try_new(limits: &wgpu::Limits, sample_count: usize) -> Result<Self> {
        let sample_count_u32 =
            u32::try_from(sample_count).context("GPU sample count exceeds u32 addressability")?;
        let dispatch_workgroups = sample_count_u32.div_ceil(SAMPLE_WORKGROUP_SIZE);
        anyhow::ensure!(
            dispatch_workgroups <= limits.max_compute_workgroups_per_dimension,
            "GPU sample dispatch requires {dispatch_workgroups} workgroups but the device limit is {}",
            limits.max_compute_workgroups_per_dimension
        );
        let sample_count_u64 = u64::from(sample_count_u32);
        let point_bytes = sample_count_u64
            .checked_mul(SAMPLE_POINT_BYTES)
            .context("GPU sample point buffer byte size overflowed")?;
        let output_bytes = sample_count_u64
            .checked_mul(4)
            .context("GPU sample output buffer byte size overflowed")?;
        anyhow::ensure!(
            point_bytes <= limits.max_buffer_size
                && output_bytes <= limits.max_buffer_size
                && point_bytes <= limits.max_storage_buffer_binding_size
                && output_bytes <= limits.max_storage_buffer_binding_size,
            "GPU sample buffers exceed the device buffer limits"
        );
        Ok(Self {
            sample_count,
            point_bytes,
            output_bytes,
            dispatch_workgroups,
        })
    }
}

enum PreparedGpuArea {
    NotNeeded,
    Reuse,
    Replace(GpuAreaResources),
}

pub(crate) struct GpuSamplingPreparation(GpuSamplingPreparationKind);

enum GpuSamplingPreparationKind {
    Unsupported,
    CpuFallback {
        admission: GpuSamplingAdmissionKey,
        reason: String,
        cache: GpuSamplingFallbackCache,
    },
    Reuse(GpuSamplingAdmissionKey),
    Replace {
        admission: GpuSamplingAdmissionKey,
        plan: CachedGpuSamplingPlan,
        buffers: Option<PreparedGpuSampleBuffers>,
        area: PreparedGpuArea,
    },
}

enum GpuSamplingFallbackCache {
    Deterministic,
    Retry(GpuSamplingRetry),
}

impl GpuSamplingPreparation {
    pub(super) const fn is_admitted(&self) -> bool {
        matches!(
            self.0,
            GpuSamplingPreparationKind::Reuse(_) | GpuSamplingPreparationKind::Replace { .. }
        )
    }

    const fn unsupported() -> Self {
        Self(GpuSamplingPreparationKind::Unsupported)
    }

    fn cpu_fallback(
        admission: GpuSamplingAdmissionKey,
        failure: GpuSamplingPreparationFailure,
        previous_retry: Option<&GpuSamplingRetry>,
    ) -> Self {
        let reason = failure.to_string();
        let cache = if failure.is_transient() {
            GpuSamplingFallbackCache::Retry(GpuSamplingRetry::after_failure(
                admission,
                reason.clone(),
                previous_retry,
            ))
        } else {
            GpuSamplingFallbackCache::Deterministic
        };
        Self(GpuSamplingPreparationKind::CpuFallback {
            admission,
            reason,
            cache,
        })
    }

    fn wait_for_retry(retry: GpuSamplingRetry) -> Self {
        Self(GpuSamplingPreparationKind::CpuFallback {
            admission: retry.admission,
            reason: retry.reason.clone(),
            cache: GpuSamplingFallbackCache::Retry(retry),
        })
    }

    const fn reuse(admission: GpuSamplingAdmissionKey) -> Self {
        Self(GpuSamplingPreparationKind::Reuse(admission))
    }

    fn replace(
        admission: GpuSamplingAdmissionKey,
        plan: CachedGpuSamplingPlan,
        buffers: Option<PreparedGpuSampleBuffers>,
        area: PreparedGpuArea,
    ) -> Self {
        Self(GpuSamplingPreparationKind::Replace {
            admission,
            plan,
            buffers,
            area,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GpuSampleSource {
    Front,
    Back,
    #[cfg(all(target_os = "macos", feature = "screen-capture"))]
    Diagnostic,
}

impl GpuSampleSource {
    pub(super) const fn index(self) -> usize {
        match self {
            Self::Front => 0,
            Self::Back => 1,
            #[cfg(all(target_os = "macos", feature = "screen-capture"))]
            Self::Diagnostic => 2,
        }
    }
}

pub(super) struct GpuSamplingDispatch {
    pub(super) sampled: bool,
    pub(super) queue_saturated: bool,
    pub(super) submission_index: Option<wgpu::SubmissionIndex>,
    pub(super) pending_readback: Option<PendingGpuSampleReadback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadbackLease {
    generation: u64,
    slot: usize,
}

pub(super) struct PendingGpuSampleReadback {
    submission_index: wgpu::SubmissionIndex,
    used_bytes: u64,
    buffer: wgpu::Buffer,
    zones: Arc<[GpuZoneRange]>,
    receiver: Option<mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>>,
    map_ready: bool,
    lease: ReadbackLease,
}

impl PendingGpuSampleReadback {
    #[cfg(test)]
    pub(super) fn submission_index(&self) -> wgpu::SubmissionIndex {
        self.submission_index.clone()
    }

    #[cfg(test)]
    pub(super) fn readback_slot(&self) -> usize {
        self.lease.slot
    }

    #[cfg(test)]
    pub(super) fn readback_generation(&self) -> u64 {
        self.lease.generation
    }

    fn unmap_after_failed_map(&mut self) {
        self.receiver = None;
        self.map_ready = false;
        self.buffer.unmap();
    }
}

impl GpuSamplingPlan {
    pub(super) fn key(prepared_zones: &[PreparedZonePlan]) -> Option<GpuSamplingPlanKey> {
        Self::supports_prepared_zones(prepared_zones).then_some(GpuSamplingPlanKey {
            ptr: prepared_zones.as_ptr() as usize,
            len: prepared_zones.len(),
            generation: plan_generation(prepared_zones),
        })
    }

    pub(super) fn supports_prepared_zones(prepared_zones: &[PreparedZonePlan]) -> bool {
        prepared_zones.iter().all(|zone| {
            matches!(
                zone.sampling_mode,
                SamplingMode::Nearest | SamplingMode::Bilinear | SamplingMode::AreaAverage { .. }
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
            match (&zone.sampling_mode, &zone.prepared_samples) {
                (SamplingMode::Nearest, PreparedZoneSamples::Nearest(samples)) => {
                    points.extend(zone.sample_positions.iter().zip(samples).map(
                        |(position, sample)| {
                            gpu_sample_point(
                                position,
                                GpuSampleMethod::Nearest,
                                sample.attenuation,
                                None,
                            )
                        },
                    ));
                }
                (SamplingMode::Bilinear, PreparedZoneSamples::Bilinear(samples)) => {
                    points.extend(zone.sample_positions.iter().zip(samples).map(
                        |(position, sample)| {
                            gpu_sample_point(
                                position,
                                GpuSampleMethod::Bilinear,
                                sample.attenuation,
                                None,
                            )
                        },
                    ));
                }
                (SamplingMode::AreaAverage { .. }, PreparedZoneSamples::Area(samples)) => {
                    points.extend(zone.sample_positions.iter().zip(samples).map(
                        |(position, sample)| {
                            gpu_sample_point(
                                position,
                                GpuSampleMethod::Area,
                                sample.attenuation,
                                Some((
                                    sample.center_x,
                                    sample.center_y,
                                    sample.radius_x,
                                    sample.radius_y,
                                )),
                            )
                        },
                    ));
                }
                _ => return None,
            }
            zones.push(GpuZoneRange {
                zone_id: zone.zone_id.clone(),
                start,
                len: points.len().saturating_sub(start),
            });
        }

        Some(Self {
            points,
            zones: zones.into(),
        })
    }
}

pub(super) struct GpuSpatialSampler {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    area_pipeline: GpuAreaPipeline,
    area_resources: Option<GpuAreaResources>,
    area_generation: u64,
    admitted_plan: Option<GpuSamplingAdmissionKey>,
    deterministic_fallback_plan: Option<GpuSamplingAdmissionKey>,
    transient_retry: Option<GpuSamplingRetry>,
    dummy_summed_area_buffer: wgpu::Buffer,
    params_buffer: wgpu::Buffer,
    cached_params: Option<[u8; SAMPLE_PARAM_BYTES]>,
    points_buffer: Option<wgpu::Buffer>,
    output_buffer: Option<wgpu::Buffer>,
    readback_buffers: Option<[wgpu::Buffer; SAMPLE_READBACK_SLOT_COUNT]>,
    readback_slots_in_use: [bool; SAMPLE_READBACK_SLOT_COUNT],
    next_readback_slot: usize,
    capacity: usize,
    buffer_generation: u64,
    cached_plan: Option<CachedGpuSamplingPlan>,
    uploaded_plan: Option<UploadedGpuSamplingPlan>,
    cached_bind_groups: Vec<CachedGpuSamplingBindGroup>,
    last_readback_wait_blocked: bool,
    #[cfg(test)]
    sample_dispatch_count: usize,
    #[cfg(test)]
    sample_param_write_count: usize,
    #[cfg(test)]
    last_readback_copy_bytes: u64,
    #[cfg(test)]
    sample_readback_wait_count: usize,
    #[cfg(test)]
    fail_next_plan_preparation: bool,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
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
        let dummy_summed_area_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU empty summed-area table"),
            size: SAT_VALUE_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        Self {
            bind_group_layout,
            pipeline,
            area_pipeline: GpuAreaPipeline::new(device),
            area_resources: None,
            area_generation: 0,
            admitted_plan: None,
            deterministic_fallback_plan: None,
            transient_retry: None,
            dummy_summed_area_buffer,
            params_buffer,
            cached_params: None,
            points_buffer: None,
            output_buffer: None,
            readback_buffers: None,
            readback_slots_in_use: [false; SAMPLE_READBACK_SLOT_COUNT],
            next_readback_slot: 0,
            capacity: 0,
            buffer_generation: 0,
            cached_plan: None,
            uploaded_plan: None,
            cached_bind_groups: Vec::with_capacity(3),
            last_readback_wait_blocked: false,
            #[cfg(test)]
            sample_dispatch_count: 0,
            #[cfg(test)]
            sample_param_write_count: 0,
            #[cfg(test)]
            last_readback_copy_bytes: 0,
            #[cfg(test)]
            sample_readback_wait_count: 0,
            #[cfg(test)]
            fail_next_plan_preparation: false,
        }
    }

    pub(super) fn can_sample_plan(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
    ) -> bool {
        let preparation = self.prepare_plan(device, width, height, prepared_zones);
        let admitted = preparation.is_admitted();
        self.apply_preparation(preparation);
        admitted
    }

    pub(super) fn prepare_plan(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
    ) -> GpuSamplingPreparation {
        let Some(plan_key) = GpuSamplingPlan::key(prepared_zones) else {
            return GpuSamplingPreparation::unsupported();
        };
        let admission = GpuSamplingAdmissionKey {
            plan: plan_key,
            width,
            height,
        };
        if self.admitted_plan == Some(admission) {
            return GpuSamplingPreparation::reuse(admission);
        }
        if self.deterministic_fallback_plan == Some(admission) {
            return GpuSamplingPreparation::cpu_fallback(
                admission,
                GpuSamplingPreparationFailure::deterministic(
                    "GPU sampling resources exceed deterministic device limits",
                ),
                None,
            );
        }
        if let Some(retry) = self
            .transient_retry
            .as_ref()
            .filter(|retry| retry.admission == admission && !retry.is_due())
        {
            return GpuSamplingPreparation::wait_for_retry(retry.clone());
        }
        let Some(plan) = GpuSamplingPlan::from_prepared_zones(prepared_zones) else {
            return GpuSamplingPreparation::unsupported();
        };
        let sample_count = plan.points.len();
        let geometry = match GpuSampleGeometry::try_new(&device.limits(), sample_count) {
            Ok(geometry) => geometry,
            Err(error) => {
                return GpuSamplingPreparation::cpu_fallback(
                    admission,
                    GpuSamplingPreparationFailure::deterministic(error.to_string()),
                    None,
                );
            }
        };
        let buffers = if sample_count > self.capacity {
            match try_prepare_sample_buffers(device, geometry) {
                Ok(buffers) => Some(buffers),
                Err(error) => {
                    return GpuSamplingPreparation::cpu_fallback(
                        admission,
                        error,
                        self.transient_retry.as_ref(),
                    );
                }
            }
        } else {
            None
        };
        let uses_area = plan
            .points
            .iter()
            .any(|point| point.method == GpuSampleMethod::Area);
        let area = if !uses_area {
            PreparedGpuArea::NotNeeded
        } else if self
            .area_resources
            .as_ref()
            .is_some_and(|resources| resources.matches(width, height))
        {
            PreparedGpuArea::Reuse
        } else {
            match self.area_pipeline.try_prepare(device, width, height) {
                Ok(resources) => PreparedGpuArea::Replace(resources),
                Err(error) => {
                    return GpuSamplingPreparation::cpu_fallback(
                        admission,
                        error,
                        self.transient_retry.as_ref(),
                    );
                }
            }
        };
        if self.take_plan_preparation_failure_injection() {
            return GpuSamplingPreparation::cpu_fallback(
                admission,
                GpuSamplingPreparationFailure::transient(
                    "injected GPU sampling plan preparation failure",
                ),
                self.transient_retry.as_ref(),
            );
        }
        let encoded_points = encode_points(&plan);
        GpuSamplingPreparation::replace(
            admission,
            CachedGpuSamplingPlan {
                key: plan_key,
                plan,
                encoded_points,
                dispatch_workgroups: geometry.dispatch_workgroups,
            },
            buffers,
            area,
        )
    }

    pub(super) fn apply_preparation(&mut self, preparation: GpuSamplingPreparation) {
        match preparation.0 {
            GpuSamplingPreparationKind::Unsupported => {
                self.admitted_plan = None;
                self.deterministic_fallback_plan = None;
                self.transient_retry = None;
                self.cached_plan = None;
            }
            GpuSamplingPreparationKind::CpuFallback {
                admission,
                reason,
                cache,
            } => {
                if self.deterministic_fallback_plan != Some(admission)
                    && self
                        .transient_retry
                        .as_ref()
                        .is_none_or(|retry| retry.admission != admission)
                {
                    tracing::warn!(
                        %reason,
                        width = admission.width,
                        height = admission.height,
                        "using exact CPU spatial sampling because GPU resources were not admitted"
                    );
                }
                self.admitted_plan = None;
                match cache {
                    GpuSamplingFallbackCache::Deterministic => {
                        self.deterministic_fallback_plan = Some(admission);
                        self.transient_retry = None;
                    }
                    GpuSamplingFallbackCache::Retry(retry) => {
                        self.deterministic_fallback_plan = None;
                        self.transient_retry = Some(retry);
                    }
                }
                self.cached_plan = None;
            }
            GpuSamplingPreparationKind::Reuse(admission) => {
                self.admitted_plan = Some(admission);
                self.deterministic_fallback_plan = None;
                self.transient_retry = None;
            }
            GpuSamplingPreparationKind::Replace {
                admission,
                plan,
                buffers,
                area,
            } => {
                if let Some(buffers) = buffers {
                    self.points_buffer = Some(buffers.points);
                    self.output_buffer = Some(buffers.output);
                    self.readback_buffers = Some(buffers.readbacks);
                    self.readback_slots_in_use = [false; SAMPLE_READBACK_SLOT_COUNT];
                    self.next_readback_slot = 0;
                    self.capacity = buffers.capacity;
                    self.buffer_generation = self.buffer_generation.saturating_add(1);
                    self.uploaded_plan = None;
                    self.cached_bind_groups.clear();
                }
                if let PreparedGpuArea::Replace(resources) = area {
                    self.area_resources = Some(resources);
                    self.area_generation = self.area_generation.saturating_add(1);
                    self.cached_bind_groups.clear();
                }
                self.cached_plan = Some(plan);
                self.admitted_plan = Some(admission);
                self.deterministic_fallback_plan = None;
                self.transient_retry = None;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn fail_next_plan_preparation(&mut self) {
        self.fail_next_plan_preparation = true;
    }

    #[cfg(test)]
    pub(super) fn make_transient_retry_due(&mut self) {
        if let Some(retry) = &mut self.transient_retry {
            retry.next_attempt_at = Instant::now();
        }
    }

    #[cfg(test)]
    pub(super) const fn has_transient_retry(&self) -> bool {
        self.transient_retry.is_some()
    }

    #[cfg(test)]
    pub(super) const fn has_deterministic_fallback(&self) -> bool {
        self.deterministic_fallback_plan.is_some()
    }

    fn take_plan_preparation_failure_injection(&mut self) -> bool {
        #[cfg(test)]
        {
            std::mem::take(&mut self.fail_next_plan_preparation)
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "callers dispatch uniformly with `?`; the GPU sampling path may fall back to fallible code without changing the signature"
    )]
    pub(super) fn sample_texture_into(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: GpuSampleSource,
        source_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        prepared_zones: &[PreparedZonePlan],
        zones: &mut Vec<ZoneColors>,
        encoder: Option<wgpu::CommandEncoder>,
    ) -> Result<GpuSamplingDispatch> {
        let admission = GpuSamplingPlan::key(prepared_zones).map(|plan| GpuSamplingAdmissionKey {
            plan,
            width,
            height,
        });
        if self.admitted_plan != admission {
            let preparation = self.prepare_plan(device, width, height, prepared_zones);
            self.apply_preparation(preparation);
        }
        if admission.is_none() || self.admitted_plan != admission {
            return Ok(GpuSamplingDispatch {
                sampled: false,
                queue_saturated: false,
                submission_index: encoder.map(|encoder| queue.submit(Some(encoder.finish()))),
                pending_readback: None,
            });
        }
        let uses_area = self.cached_plan.as_ref().is_some_and(|cached| {
            cached
                .plan
                .points
                .iter()
                .any(|point| point.method == GpuSampleMethod::Area)
        });
        let (sample_count, dispatch_workgroups) =
            self.cached_plan.as_ref().map_or((0, 0), |cached| {
                (cached.plan.points.len(), cached.dispatch_workgroups)
            });
        let Some(points_buffer) = self.points_buffer.clone() else {
            zones.clear();
            return Ok(GpuSamplingDispatch {
                sampled: true,
                queue_saturated: false,
                submission_index: encoder.map(|encoder| queue.submit(Some(encoder.finish()))),
                pending_readback: None,
            });
        };
        let Some(output_buffer) = self.output_buffer.clone() else {
            zones.clear();
            return Ok(GpuSamplingDispatch {
                sampled: true,
                queue_saturated: false,
                submission_index: encoder.map(|encoder| queue.submit(Some(encoder.finish()))),
                pending_readback: None,
            });
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

        let mut encoder = encoder.unwrap_or_else(|| {
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("SparkleFlinger GPU sample encoder"),
            })
        });
        if uses_area {
            let area_pipeline = &self.area_pipeline;
            let area_resources = self
                .area_resources
                .as_mut()
                .expect("GPU area resources should be admitted before encoding");
            area_pipeline.encode(device, source, source_view, area_resources, &mut encoder);
        }
        let summed_area_buffer = self.area_resources.as_ref().map_or_else(
            || self.dummy_summed_area_buffer.clone(),
            |resources| resources.summed_area_buffer().clone(),
        );
        let bind_group = self.bind_group_for(
            device,
            source,
            source_view,
            &points_buffer,
            &output_buffer,
            &summed_area_buffer,
        );
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("SparkleFlinger GPU sample pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(dispatch_workgroups, 1, 1);
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
        if output_bytes == 0 {
            let submission_index = queue.submit(Some(encoder.finish()));
            zones.clear();
            return Ok(GpuSamplingDispatch {
                sampled: true,
                queue_saturated: false,
                submission_index: Some(submission_index),
                pending_readback: None,
            });
        }
        let Some((readback_lease, readback_buffer)) = self.next_readback_buffer() else {
            zones.clear();
            return Ok(GpuSamplingDispatch {
                sampled: false,
                queue_saturated: true,
                submission_index: Some(queue.submit(Some(encoder.finish()))),
                pending_readback: None,
            });
        };
        encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback_buffer, 0, output_bytes);
        let submission_index = queue.submit(Some(encoder.finish()));
        let zone_ranges = Arc::clone(
            &self
                .cached_plan
                .as_ref()
                .expect("GPU sampling plan should be cached before readback")
                .plan
                .zones,
        );
        Ok(GpuSamplingDispatch {
            sampled: true,
            queue_saturated: false,
            submission_index: Some(submission_index.clone()),
            pending_readback: Some(begin_zone_color_readback(
                &readback_buffer,
                output_bytes,
                submission_index,
                zone_ranges,
                readback_lease,
            )),
        })
    }

    pub(super) fn finish_pending_readback(
        &mut self,
        device: &wgpu::Device,
        mut pending_readback: PendingGpuSampleReadback,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<()> {
        self.last_readback_wait_blocked = false;
        if !self.try_finish_pending_readback(device, &mut pending_readback, zones)? {
            self.last_readback_wait_blocked = true;
            #[cfg(test)]
            {
                self.sample_readback_wait_count = self.sample_readback_wait_count.saturating_add(1);
            }
            if let Err(error) = wait_for_zone_color_readback(device, &mut pending_readback) {
                self.release_readback_slot(pending_readback.lease);
                return Err(error);
            }
            finish_zone_color_readback(&pending_readback, zones);
            self.release_readback_slot(pending_readback.lease);
        }

        Ok(())
    }

    pub(super) fn try_finish_pending_readback(
        &mut self,
        device: &wgpu::Device,
        pending_readback: &mut PendingGpuSampleReadback,
        zones: &mut Vec<ZoneColors>,
    ) -> Result<bool> {
        self.last_readback_wait_blocked = false;
        if let Err(error) = poll_zone_color_readback_ready(device, pending_readback) {
            self.release_readback_slot(pending_readback.lease);
            return Err(error);
        }
        if !pending_readback.map_ready {
            return Ok(false);
        }
        finish_zone_color_readback(pending_readback, zones);
        self.release_readback_slot(pending_readback.lease);
        Ok(true)
    }

    pub(super) fn take_last_readback_wait_blocked(&mut self) -> bool {
        std::mem::take(&mut self.last_readback_wait_blocked)
    }

    pub(super) const fn max_pending_readbacks(&self) -> usize {
        SAMPLE_READBACK_SLOT_COUNT
    }

    pub(super) fn discard_pending_readback(&mut self, pending_readback: PendingGpuSampleReadback) {
        pending_readback.buffer.unmap();
        self.release_readback_slot(pending_readback.lease);
    }

    fn next_readback_buffer(&mut self) -> Option<(ReadbackLease, wgpu::Buffer)> {
        let readback_buffers = self.readback_buffers.as_ref()?;
        for offset in 0..SAMPLE_READBACK_SLOT_COUNT {
            let slot = (self.next_readback_slot + offset) % SAMPLE_READBACK_SLOT_COUNT;
            if !self.readback_slots_in_use[slot] {
                self.readback_slots_in_use[slot] = true;
                self.next_readback_slot = (slot + 1) % SAMPLE_READBACK_SLOT_COUNT;
                return Some((
                    ReadbackLease {
                        generation: self.buffer_generation,
                        slot,
                    },
                    readback_buffers[slot].clone(),
                ));
            }
        }
        None
    }

    fn release_readback_slot(&mut self, lease: ReadbackLease) {
        if lease.generation == self.buffer_generation && lease.slot < SAMPLE_READBACK_SLOT_COUNT {
            self.readback_slots_in_use[lease.slot] = false;
        }
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
        source: GpuSampleSource,
        source_view: &wgpu::TextureView,
        points_buffer: &wgpu::Buffer,
        output_buffer: &wgpu::Buffer,
        summed_area_buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        if let Some(cached) = self.cached_bind_groups.iter().find(|cached| {
            cached.source == source
                && cached.buffer_generation == self.buffer_generation
                && cached.area_generation == self.area_generation
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
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: summed_area_buffer.as_entire_binding(),
                },
            ],
        });
        self.cached_bind_groups.push(CachedGpuSamplingBindGroup {
            source,
            buffer_generation: self.buffer_generation,
            area_generation: self.area_generation,
            bind_group: bind_group.clone(),
        });
        bind_group
    }

    pub(super) fn clear_bind_groups(&mut self) {
        self.cached_bind_groups.clear();
        if let Some(resources) = &mut self.area_resources {
            resources.clear_bind_groups();
        }
    }

    #[cfg(test)]
    pub(super) fn cached_bind_group_count(&self) -> usize {
        self.cached_bind_groups.len()
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

    #[cfg(test)]
    pub(super) fn sample_readback_wait_count(&self) -> usize {
        self.sample_readback_wait_count
    }

    #[cfg(test)]
    pub(super) const fn area_generation(&self) -> u64 {
        self.area_generation
    }

    #[cfg(test)]
    pub(super) const fn buffer_generation(&self) -> u64 {
        self.buffer_generation
    }
}

fn try_prepare_sample_buffers(
    device: &wgpu::Device,
    geometry: GpuSampleGeometry,
) -> std::result::Result<PreparedGpuSampleBuffers, GpuSamplingPreparationFailure> {
    let out_of_memory_scope = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal_scope = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let points = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SparkleFlinger GPU sample points"),
        size: geometry.point_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SparkleFlinger GPU sample output"),
        size: geometry.output_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readbacks = std::array::from_fn(|_| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SparkleFlinger GPU sample readback"),
            size: geometry.output_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    });
    let validation_error = pollster::block_on(validation_scope.pop());
    let internal_error = pollster::block_on(internal_scope.pop());
    let out_of_memory_error = pollster::block_on(out_of_memory_scope.pop());
    if let Some(error) = out_of_memory_error {
        return Err(GpuSamplingPreparationFailure::transient(format!(
            "GPU sample buffer allocation ran out of memory: {error}"
        )));
    }
    if let Some(error) = internal_error {
        return Err(GpuSamplingPreparationFailure::transient(format!(
            "GPU sample buffer allocation hit an internal device error: {error}"
        )));
    }
    if let Some(error) = validation_error {
        return Err(GpuSamplingPreparationFailure::deterministic(format!(
            "GPU sample buffers failed device validation: {error}"
        )));
    }
    Ok(PreparedGpuSampleBuffers {
        points,
        output,
        readbacks,
        capacity: geometry.sample_count,
    })
}

fn encode_points(plan: &GpuSamplingPlan) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(
        plan.points
            .len()
            .saturating_mul(SAMPLE_POINT_BYTES as usize),
    );
    for point in &plan.points {
        bytes.extend_from_slice(&point.x.to_le_bytes());
        bytes.extend_from_slice(&point.y.to_le_bytes());
        bytes.extend_from_slice(&(point.method as u32).to_le_bytes());
        bytes.extend_from_slice(&point.attenuation.to_le_bytes());
        bytes.extend_from_slice(&point.center_x.to_le_bytes());
        bytes.extend_from_slice(&point.center_y.to_le_bytes());
        bytes.extend_from_slice(&point.radius_x.to_le_bytes());
        bytes.extend_from_slice(&point.radius_y.to_le_bytes());
    }
    bytes
}

fn gpu_sample_point(
    position: &hypercolor_types::spatial::NormalizedPosition,
    method: GpuSampleMethod,
    attenuation: u16,
    area: Option<(u32, u32, u32, u32)>,
) -> GpuSamplePoint {
    GpuSamplePoint::new(position.x, position.y, method, attenuation, area)
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

fn plan_generation(prepared_zones: &[PreparedZonePlan]) -> u64 {
    prepared_zones
        .first()
        .map_or(0, |zone| zone.plan_generation)
}

fn begin_zone_color_readback(
    buffer: &wgpu::Buffer,
    used_bytes: u64,
    submission_index: wgpu::SubmissionIndex,
    zones: Arc<[GpuZoneRange]>,
    lease: ReadbackLease,
) -> PendingGpuSampleReadback {
    let slice = buffer.slice(..used_bytes);
    let (sender, receiver) = mpsc::channel::<std::result::Result<(), wgpu::BufferAsyncError>>();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    PendingGpuSampleReadback {
        submission_index,
        used_bytes,
        buffer: buffer.clone(),
        zones,
        receiver: Some(receiver),
        map_ready: false,
        lease,
    }
}

fn poll_zone_color_readback_ready(
    device: &wgpu::Device,
    pending_readback: &mut PendingGpuSampleReadback,
) -> Result<bool> {
    if pending_readback.map_ready {
        return Ok(true);
    }
    device
        .poll(wgpu::PollType::Poll)
        .context("GPU sample callback poll failed")?;
    if take_zone_color_readback_ready(pending_readback)?.unwrap_or(false) {
        return Ok(true);
    }
    match device.poll(wgpu::PollType::Wait {
        submission_index: Some(pending_readback.submission_index.clone()),
        timeout: Some(std::time::Duration::ZERO),
    }) {
        Ok(_) | Err(wgpu::PollError::Timeout) => {}
        Err(error) => return Err(error).context("GPU sample readiness poll failed"),
    }
    device
        .poll(wgpu::PollType::Poll)
        .context("GPU sample callback poll failed")?;
    Ok(take_zone_color_readback_ready(pending_readback)?.unwrap_or(false))
}

fn take_zone_color_readback_ready(
    pending_readback: &mut PendingGpuSampleReadback,
) -> Result<Option<bool>> {
    let Some(receiver) = pending_readback.receiver.as_mut() else {
        anyhow::bail!("GPU sample channel was unavailable before map completion")
    };
    match receiver.try_recv() {
        Ok(Ok(())) => {
            pending_readback.receiver = None;
            pending_readback.map_ready = true;
            Ok(Some(true))
        }
        Ok(Err(error)) => {
            pending_readback.unmap_after_failed_map();
            Err(error).context("GPU sample buffer mapping failed")
        }
        Err(TryRecvError::Disconnected) => {
            pending_readback.unmap_after_failed_map();
            anyhow::bail!("GPU sample channel closed before map completion")
        }
        Err(TryRecvError::Empty) => Ok(None),
    }
}

fn wait_for_zone_color_readback(
    device: &wgpu::Device,
    pending_readback: &mut PendingGpuSampleReadback,
) -> Result<()> {
    if pending_readback.map_ready {
        return Ok(());
    }
    // A wedged GPU must not stall the render thread forever; callers fall
    // back to CPU sampling when this errors.
    let deadline = std::time::Instant::now() + SAMPLE_READBACK_WAIT_BUDGET;
    loop {
        match device.poll(wgpu::PollType::Wait {
            submission_index: Some(pending_readback.submission_index.clone()),
            timeout: Some(SAMPLE_READBACK_WAIT_STEP),
        }) {
            Ok(_) => break,
            Err(wgpu::PollError::Timeout) => {
                if std::time::Instant::now() >= deadline {
                    pending_readback.unmap_after_failed_map();
                    anyhow::bail!(
                        "GPU sample readback exceeded {}ms wait budget",
                        SAMPLE_READBACK_WAIT_BUDGET.as_millis()
                    );
                }
            }
            Err(error) => {
                pending_readback.unmap_after_failed_map();
                return Err(error).context("GPU sample poll failed");
            }
        }
    }
    let Some(receiver) = pending_readback.receiver.take() else {
        pending_readback.unmap_after_failed_map();
        anyhow::bail!("GPU sample channel was unavailable before wait completion");
    };
    match receiver.recv_timeout(SAMPLE_READBACK_WAIT_STEP) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            pending_readback.unmap_after_failed_map();
            return Err(error).context("GPU sample buffer mapping failed");
        }
        Err(error) => {
            pending_readback.unmap_after_failed_map();
            return Err(error).context("GPU sample map callback did not complete after wait");
        }
    }
    pending_readback.map_ready = true;
    Ok(())
}

fn finish_zone_color_readback(
    pending_readback: &PendingGpuSampleReadback,
    zones: &mut Vec<ZoneColors>,
) {
    let slice = pending_readback.buffer.slice(..pending_readback.used_bytes);
    let mapped = slice.get_mapped_range();
    rebuild_zone_colors_from_mapped_bytes(&pending_readback.zones, &mapped, zones);
    drop(mapped);
    pending_readback.buffer.unmap();
}

fn rebuild_zone_colors_from_mapped_bytes(
    zone_plans: &[GpuZoneRange],
    packed_bytes: &[u8],
    zones: &mut Vec<ZoneColors>,
) {
    zones.reserve(zone_plans.len().saturating_sub(zones.len()));

    for (index, zone_plan) in zone_plans.iter().enumerate() {
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
        if zone.colors.len() != zone_plan.len {
            zone.colors.resize(zone_plan.len, [0_u8; 3]);
        }
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

    zones.truncate(zone_plans.len());
}

#[cfg(test)]
mod tests {
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
        StripDirection,
    };

    use super::{GpuSampleGeometry, GpuSampleMethod, GpuSamplingPlan, SAMPLE_WORKGROUP_SIZE};

    fn synthetic_sample_limits(max_dispatch: u32) -> wgpu::Limits {
        wgpu::Limits {
            max_buffer_size: u64::MAX,
            max_storage_buffer_binding_size: u64::MAX,
            max_compute_workgroups_per_dimension: max_dispatch,
            ..wgpu::Limits::default()
        }
    }

    #[test]
    fn gpu_sample_admission_enforces_65535_workgroup_boundary() {
        let limits = synthetic_sample_limits(65_535);
        let admitted_count = 65_535_usize * SAMPLE_WORKGROUP_SIZE as usize;
        let admitted = GpuSampleGeometry::try_new(&limits, admitted_count)
            .expect("65,535 workgroups should fit the synthetic device limit");
        assert_eq!(admitted.dispatch_workgroups, 65_535);

        let error = GpuSampleGeometry::try_new(&limits, admitted_count + 1)
            .expect_err("65,536 workgroups must exceed the synthetic device limit");
        assert!(error.to_string().contains("requires 65536 workgroups"));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn gpu_sample_admission_rejects_counts_above_u32() {
        let limits = synthetic_sample_limits(u32::MAX);
        GpuSampleGeometry::try_new(&limits, u32::MAX as usize)
            .expect("u32::MAX samples remain shader-addressable");

        let error = GpuSampleGeometry::try_new(&limits, u32::MAX as usize + 1)
            .expect_err("sample counts above u32 must be rejected before encoding");
        assert!(error.to_string().contains("exceeds u32 addressability"));
    }

    fn test_layout(mode: SamplingMode) -> SpatialLayout {
        SpatialLayout {
            id: "test".into(),
            name: "Test".into(),
            description: None,
            canvas_width: 16,
            canvas_height: 16,
            zones: vec![Output {
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
                brightness: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
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
        assert_eq!(plan.points[8].radius_x(), 2);
        assert_eq!(plan.points[8].radius_y(), 2);
    }

    #[test]
    fn gpu_sampling_plan_keeps_area_sample_radius() {
        let area = SpatialEngine::new(test_layout(SamplingMode::AreaAverage {
            radius_x: 3.0,
            radius_y: 1.0,
        }));
        let plan = GpuSamplingPlan::from_prepared_zones(area.sampling_plan().as_ref())
            .expect("area plans should stay GPU-sampleable");
        assert_eq!(plan.points[0].radius_x(), 3);
        assert_eq!(plan.points[0].radius_y(), 1);
    }

    #[test]
    fn gpu_sampling_plan_keeps_radii_above_u16() {
        let area = SpatialEngine::new(test_layout(SamplingMode::AreaAverage {
            radius_x: 65_536.0,
            radius_y: 131_072.0,
        }));
        let plan = GpuSamplingPlan::from_prepared_zones(area.sampling_plan().as_ref())
            .expect("large area radii should stay GPU-sampleable");

        assert_eq!(plan.points[0].radius_x(), 65_536);
        assert_eq!(plan.points[0].radius_y(), 131_072);
    }

    #[test]
    fn gpu_area_query_shader_has_no_radius_proportional_loop() {
        let shader = include_str!("sample.wgsl");

        assert!(shader.contains("rectangle_sum"));
        assert!(!shader.contains("radius_i"));
        assert!(!shader.contains("var dx"));
        assert!(!shader.contains("var dy"));
    }

    #[test]
    fn gpu_sampling_plan_rejects_gaussian_without_aliasing() {
        let gaussian = SpatialEngine::new(test_layout(SamplingMode::GaussianArea {
            sigma: 1.0,
            radius: 2,
        }));
        let sampling_plan = gaussian.sampling_plan();
        let prepared_zones = sampling_plan.as_ref();

        assert!(!GpuSamplingPlan::supports_prepared_zones(prepared_zones));
        assert!(GpuSamplingPlan::key(prepared_zones).is_none());
        assert!(GpuSamplingPlan::from_prepared_zones(prepared_zones).is_none());
    }

    #[test]
    fn gpu_sampling_plan_key_changes_when_generation_advances() {
        let mut engine = SpatialEngine::new(test_layout(SamplingMode::Bilinear));
        let first_plan = engine.sampling_plan();
        let first_key =
            GpuSamplingPlan::key(first_plan.as_ref()).expect("bilinear plan should be supported");

        engine.update_layout(test_layout(SamplingMode::Bilinear));
        let second_plan = engine.sampling_plan();
        let second_key =
            GpuSamplingPlan::key(second_plan.as_ref()).expect("bilinear plan should be supported");

        assert_ne!(first_key, second_key);
        assert_ne!(first_key.generation, second_key.generation);
    }
}
