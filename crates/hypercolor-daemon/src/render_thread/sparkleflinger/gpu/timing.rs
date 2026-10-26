//! Pass-boundary GPU timestamps with asynchronous, reusable readback slots.
//!
//! Set `HYPERCOLOR_GPU_TIMING=1` before starting the daemon to collect timings.
//! Profiling adds query resolution and readback work to every measured pass;
//! the default path allocates no query resources and records no observations.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use hypercolor_types::api::system::{GpuPassTimingStatus, GpuPassTimingsStatus};

// This bounds diagnostics memory only. A busy pool skips a measurement, never
// rendering work. Every skip is exposed so incomplete coverage stays visible.
const SLOT_COUNT: u32 = 128;
const TIMESTAMP_BYTES: u64 = 16;
const INVALID_TIMESTAMPS: [u8; 16] = [
    255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0,
];
const STAGE_COUNT: usize = 7;
static CURRENT: LazyLock<Mutex<Weak<TimingState>>> = LazyLock::new(Mutex::default);
static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
pub(crate) enum GpuStage {
    Compose,
    SourceCopy,
    AreaTable,
    Sample,
    PreviewScale,
    DisplayRgba,
    DisplayYuv,
}

impl GpuStage {
    const ALL: [Self; STAGE_COUNT] = [
        Self::Compose,
        Self::SourceCopy,
        Self::AreaTable,
        Self::Sample,
        Self::PreviewScale,
        Self::DisplayRgba,
        Self::DisplayYuv,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Compose => "compose",
            Self::SourceCopy => "source_copy",
            Self::AreaTable => "area_table",
            Self::Sample => "led_sample",
            Self::PreviewScale => "preview_scale",
            Self::DisplayRgba => "display_rgba",
            Self::DisplayYuv => "display_yuv",
        }
    }
}

#[derive(Default)]
struct StageCounters {
    attempts: AtomicU64,
    completed: AtomicU64,
    skipped: AtomicU64,
    failed: AtomicU64,
    cancelled: AtomicU64,
    total_ns: AtomicU64,
    last_ns: AtomicU64,
    max_ns: AtomicU64,
}

struct Slot {
    busy: AtomicBool,
    readback: wgpu::Buffer,
}

struct Resources {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
}

struct TimingState {
    instance_id: u64,
    supported: bool,
    unavailable_reason: Option<String>,
    period_ns: f64,
    resources: Option<Resources>,
    counters: [StageCounters; STAGE_COUNT],
}

#[derive(Clone)]
pub(crate) struct GpuTimings {
    state: Arc<TimingState>,
    queue: wgpu::Queue,
}

impl GpuTimings {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::with_enabled(
            device,
            queue,
            std::env::var("HYPERCOLOR_GPU_TIMING").as_deref() == Ok("1"),
        )
    }

    fn with_enabled(device: &wgpu::Device, queue: &wgpu::Queue, enabled: bool) -> Self {
        let supported = device.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let allocation = (supported && enabled).then(|| {
            super::try_create_gpu_resources(device, "GPU timestamp resources", || Resources {
                queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("SparkleFlinger GPU timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: SLOT_COUNT * 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("SparkleFlinger timestamp resolve"),
                    size: u64::from(SLOT_COUNT) * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                slots: (0..SLOT_COUNT)
                    .map(|_| Slot {
                        busy: AtomicBool::new(false),
                        readback: device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("SparkleFlinger timestamp readback"),
                            size: TIMESTAMP_BYTES,
                            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                    })
                    .collect(),
            })
        });
        let (resources, unavailable_reason) = match allocation {
            Some(Ok(resources)) => (Some(resources), None),
            Some(Err(error)) => {
                tracing::warn!(%error, "GPU timestamps unavailable; rendering continues");
                (None, Some(error.to_string()))
            }
            None => (
                None,
                Some(
                    if supported {
                        "disabled_by_configuration"
                    } else {
                        "timestamp_query_not_supported"
                    }
                    .to_owned(),
                ),
            ),
        };
        Self {
            state: Arc::new(TimingState {
                instance_id: NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed),
                supported,
                unavailable_reason,
                period_ns: f64::from(queue.get_timestamp_period()),
                resources,
                counters: std::array::from_fn(|_| StageCounters::default()),
            }),
            queue: queue.clone(),
        }
    }

    pub(crate) fn publish(&self) {
        *CURRENT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&self.state);
    }

    pub(crate) fn compute(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        stage: GpuStage,
        label: &'static str,
        encode: impl FnOnce(&mut wgpu::ComputePass<'_>),
    ) {
        let timing = self.begin(stage);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: timing.as_ref().map(PassTiming::writes),
        });
        encode(&mut pass);
        drop(pass);
        if let Some(timing) = timing {
            timing.resolve(encoder);
        }
    }

    pub(crate) fn begin(&self, stage: GpuStage) -> Option<PassTiming> {
        let resources = self.state.resources.as_ref()?;
        let counters = &self.state.counters[stage as usize];
        counters.attempts.fetch_add(1, Ordering::Relaxed);
        for (index, slot) in resources.slots.iter().enumerate() {
            if slot
                .busy
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                // Deferred maps still run when wgpu rejects a submission.
                // A queue write is flushed before mapping even in that
                // case; only an executed resolve overwrites this sentinel.
                self.queue
                    .write_buffer(&slot.readback, 0, &INVALID_TIMESTAMPS);
                return Some(PassTiming {
                    state: Arc::clone(&self.state),
                    index,
                    stage,
                    completed: false,
                    reusable: true,
                });
            }
        }
        counters.skipped.fetch_add(1, Ordering::Release);
        None
    }

    #[cfg(test)]
    fn snapshot(&self) -> GpuPassTimingsStatus {
        self.state.snapshot()
    }
}

impl TimingState {
    fn snapshot(&self) -> GpuPassTimingsStatus {
        GpuPassTimingsStatus {
            instance_id: self.instance_id,
            supported: self.supported,
            enabled: self.resources.is_some(),
            unavailable_reason: self.unavailable_reason.clone(),
            stages: GpuStage::ALL
                .into_iter()
                .map(|stage| {
                    let c = &self.counters[stage as usize];
                    // Terminal releases follow their attempt increment. Read
                    // them first so a concurrent API snapshot cannot report
                    // more terminal observations than attempts.
                    let completed = c.completed.load(Ordering::Acquire);
                    let skipped = c.skipped.load(Ordering::Acquire);
                    let failed = c.failed.load(Ordering::Acquire);
                    let cancelled = c.cancelled.load(Ordering::Acquire);
                    GpuPassTimingStatus {
                        stage: stage.label().to_owned(),
                        attempts: c.attempts.load(Ordering::Relaxed),
                        completed,
                        skipped,
                        failed,
                        cancelled,
                        total_ns: c.total_ns.load(Ordering::Relaxed),
                        last_ns: c.last_ns.load(Ordering::Relaxed),
                        max_ns: c.max_ns.load(Ordering::Relaxed),
                    }
                })
                .collect(),
        }
    }
}

pub(crate) fn gpu_pass_timings_snapshot() -> GpuPassTimingsStatus {
    CURRENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .upgrade()
        .map_or_else(
            || GpuPassTimingsStatus {
                unavailable_reason: Some("no_current_compositor".to_owned()),
                ..GpuPassTimingsStatus::default()
            },
            |state| state.snapshot(),
        )
}

pub(crate) struct PassTiming {
    state: Arc<TimingState>,
    index: usize,
    stage: GpuStage,
    completed: bool,
    reusable: bool,
}

impl PassTiming {
    pub(crate) fn writes(&self) -> wgpu::ComputePassTimestampWrites<'_> {
        let resources = self
            .state
            .resources
            .as_ref()
            .expect("timing lease owns GPU resources");
        let index = u32::try_from(self.index).expect("timestamp slot index fits u32") * 2;
        wgpu::ComputePassTimestampWrites {
            query_set: &resources.queries,
            beginning_of_pass_write_index: Some(index),
            end_of_pass_write_index: Some(index + 1),
        }
    }

    pub(crate) fn resolve(self, encoder: &mut wgpu::CommandEncoder) {
        let resources = self
            .state
            .resources
            .as_ref()
            .expect("timing lease owns GPU resources");
        let index = u32::try_from(self.index).expect("timestamp slot index fits u32");
        let offset = u64::from(index) * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        let readback = resources.slots[self.index].readback.clone();
        encoder.resolve_query_set(
            &resources.queries,
            index * 2..index * 2 + 2,
            &resources.resolve,
            offset,
        );
        encoder.copy_buffer_to_buffer(&resources.resolve, offset, &readback, 0, TIMESTAMP_BYTES);
        encoder.map_buffer_on_submit(&readback.clone(), wgpu::MapMode::Read, .., move |result| {
            let mut lease = self;
            let counters = &lease.state.counters[lease.stage as usize];
            if result.is_ok() {
                let Ok(mapped) = readback.get_mapped_range(..) else {
                    lease.reusable = false;
                    counters.failed.fetch_add(1, Ordering::Release);
                    lease.completed = true;
                    return;
                };
                let start =
                    u64::from_le_bytes(mapped[..8].try_into().expect("timestamp is eight bytes"));
                let end =
                    u64::from_le_bytes(mapped[8..16].try_into().expect("timestamp is eight bytes"));
                let duration_ns = timestamp_duration_ns(start, end, lease.state.period_ns);
                drop(mapped);
                readback.unmap();
                if let Some(ns) = duration_ns {
                    counters.total_ns.fetch_add(ns, Ordering::Relaxed);
                    counters.last_ns.store(ns, Ordering::Relaxed);
                    counters.max_ns.fetch_max(ns, Ordering::Relaxed);
                    counters.completed.fetch_add(1, Ordering::Release);
                } else {
                    counters.failed.fetch_add(1, Ordering::Release);
                }
            } else {
                // The client retains its map range on failure, while the
                // backend may already be idle or destroyed. Do not unmap an
                // invalid backend buffer: retire this slot until device teardown.
                lease.reusable = false;
                counters.failed.fetch_add(1, Ordering::Release);
            }
            lease.completed = true;
        });
    }
}

fn timestamp_duration_ns(start: u64, end: u64, period_ns: f64) -> Option<u64> {
    // Some hardware clocks have fewer than 64 valid bits. Without the counter
    // width, a wrap cannot be corrected safely; omit that observation.
    let ticks = end.checked_sub(start)?;
    (period_ns.is_finite() && period_ns > 0.0).then(|| (ticks as f64 * period_ns).round() as u64)
}

impl Drop for PassTiming {
    fn drop(&mut self) {
        if !self.completed {
            self.state.counters[self.stage as usize]
                .cancelled
                .fetch_add(1, Ordering::Release);
        }
        if self.reusable
            && let Some(resources) = &self.state.resources
        {
            resources.slots[self.index]
                .busy
                .store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;
