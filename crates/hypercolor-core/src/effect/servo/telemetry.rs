use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct ServoTelemetrySnapshot {
    pub soft_stalls_total: u64,
    pub breaker_opens_total: u64,
    pub session_creates_total: u64,
    pub session_create_failures_total: u64,
    pub session_create_wait_total_us: u64,
    pub session_create_wait_max_us: u64,
    pub page_loads_total: u64,
    pub page_load_failures_total: u64,
    pub page_load_wait_total_us: u64,
    pub page_load_wait_max_us: u64,
    pub detached_destroys_total: u64,
    pub detached_destroy_failures_total: u64,
    pub render_requests_total: u64,
    pub render_queue_wait_total_us: u64,
    pub render_queue_wait_max_us: u64,
    pub render_cpu_frames_total: u64,
    pub render_cached_frames_total: u64,
    pub render_gpu_frames_total: u64,
    pub render_gpu_import_failures_total: u64,
    pub render_gpu_import_fallbacks_total: u64,
    pub render_gpu_import_fallback_reason: Option<&'static str>,
    pub render_gpu_import_blit_total_us: u64,
    pub render_gpu_import_blit_max_us: u64,
    pub render_gpu_import_sync_total_us: u64,
    pub render_gpu_import_sync_max_us: u64,
    pub render_gpu_import_total_us: u64,
    pub render_gpu_import_max_us: u64,
    pub render_evaluate_scripts_total_us: u64,
    pub render_evaluate_scripts_max_us: u64,
    pub render_event_loop_total_us: u64,
    pub render_event_loop_max_us: u64,
    pub render_paint_total_us: u64,
    pub render_paint_max_us: u64,
    pub render_readback_total_us: u64,
    pub render_readback_max_us: u64,
    pub render_frame_total_us: u64,
    pub render_frame_max_us: u64,
}

static SERVO_SOFT_STALLS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_BREAKER_OPENS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATE_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_SESSION_CREATE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOADS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOAD_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOAD_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_PAGE_LOAD_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_DETACHED_DESTROYS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_DETACHED_DESTROY_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_CPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_CACHED_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_FALLBACKS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_FALLBACK_REASON: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_BLIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_BLIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_SYNC_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_SYNC_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_EVALUATE_SCRIPTS_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_EVALUATE_SCRIPTS_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_EVENT_LOOP_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_EVENT_LOOP_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_PAINT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_PAINT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_READBACK_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_READBACK_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_FRAME_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_FRAME_MAX_US: AtomicU64 = AtomicU64::new(0);

pub(super) fn record_servo_soft_stall() {
    let _ = SERVO_SOFT_STALLS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_servo_breaker_open() {
    let _ = SERVO_BREAKER_OPENS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_servo_session_create(wait: Duration, success: bool) {
    record_wait(
        wait,
        &SERVO_SESSION_CREATES_TOTAL,
        &SERVO_SESSION_CREATE_WAIT_TOTAL_US,
        &SERVO_SESSION_CREATE_WAIT_MAX_US,
    );
    if !success {
        let _ = SERVO_SESSION_CREATE_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_page_load(wait: Duration, success: bool) {
    record_wait(
        wait,
        &SERVO_PAGE_LOADS_TOTAL,
        &SERVO_PAGE_LOAD_WAIT_TOTAL_US,
        &SERVO_PAGE_LOAD_WAIT_MAX_US,
    );
    if !success {
        let _ = SERVO_PAGE_LOAD_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_detached_destroy(success: bool) {
    let _ = SERVO_DETACHED_DESTROYS_TOTAL.fetch_add(1, Ordering::Relaxed);
    if !success {
        let _ = SERVO_DETACHED_DESTROY_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_render_queue_wait(wait: Duration) {
    record_wait(
        wait,
        &SERVO_RENDER_REQUESTS_TOTAL,
        &SERVO_RENDER_QUEUE_WAIT_TOTAL_US,
        &SERVO_RENDER_QUEUE_WAIT_MAX_US,
    );
}

pub(super) fn record_servo_cpu_render_frame(
    evaluate_scripts_us: u64,
    event_loop_us: u64,
    paint_us: u64,
    readback_us: u64,
    total_us: u64,
    reused_cached_canvas: bool,
) {
    let _ = SERVO_RENDER_CPU_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
    if reused_cached_canvas {
        let _ = SERVO_RENDER_CACHED_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    record_servo_render_stage_durations(evaluate_scripts_us, event_loop_us, paint_us, total_us);
    record_duration_us(
        readback_us,
        &SERVO_RENDER_READBACK_TOTAL_US,
        &SERVO_RENDER_READBACK_MAX_US,
    );
}

pub(super) fn record_servo_gpu_render_frame(
    evaluate_scripts_us: u64,
    event_loop_us: u64,
    paint_us: u64,
    total_us: u64,
) {
    record_servo_render_stage_durations(evaluate_scripts_us, event_loop_us, paint_us, total_us);
}

fn record_servo_render_stage_durations(
    evaluate_scripts_us: u64,
    event_loop_us: u64,
    paint_us: u64,
    total_us: u64,
) {
    record_duration_us(
        evaluate_scripts_us,
        &SERVO_RENDER_EVALUATE_SCRIPTS_TOTAL_US,
        &SERVO_RENDER_EVALUATE_SCRIPTS_MAX_US,
    );
    record_duration_us(
        event_loop_us,
        &SERVO_RENDER_EVENT_LOOP_TOTAL_US,
        &SERVO_RENDER_EVENT_LOOP_MAX_US,
    );
    record_duration_us(
        paint_us,
        &SERVO_RENDER_PAINT_TOTAL_US,
        &SERVO_RENDER_PAINT_MAX_US,
    );
    record_duration_us(
        total_us,
        &SERVO_RENDER_FRAME_TOTAL_US,
        &SERVO_RENDER_FRAME_MAX_US,
    );
}

#[cfg(feature = "servo-gpu-import")]
pub(super) fn record_servo_gpu_import_frame(blit_us: u64, sync_us: u64, total_us: u64) {
    let _ = SERVO_RENDER_GPU_FRAMES_TOTAL.fetch_add(1, Ordering::Relaxed);
    record_duration_us(
        blit_us,
        &SERVO_RENDER_GPU_IMPORT_BLIT_TOTAL_US,
        &SERVO_RENDER_GPU_IMPORT_BLIT_MAX_US,
    );
    record_duration_us(
        sync_us,
        &SERVO_RENDER_GPU_IMPORT_SYNC_TOTAL_US,
        &SERVO_RENDER_GPU_IMPORT_SYNC_MAX_US,
    );
    record_duration_us(
        total_us,
        &SERVO_RENDER_GPU_IMPORT_TOTAL_US,
        &SERVO_RENDER_GPU_IMPORT_MAX_US,
    );
}

#[cfg(feature = "servo-gpu-import")]
pub(super) fn record_servo_gpu_import_failure(
    reason: ServoGpuImportFallbackReason,
    fell_back_to_cpu: bool,
) {
    let _ = SERVO_RENDER_GPU_IMPORT_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    if fell_back_to_cpu {
        let _ = SERVO_RENDER_GPU_IMPORT_FALLBACKS_TOTAL.fetch_add(1, Ordering::Relaxed);
        SERVO_RENDER_GPU_IMPORT_FALLBACK_REASON.store(reason.as_u64(), Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServoGpuImportFallbackReason {
    DeviceUnavailable,
    MissingWgpuVulkanDevice,
    MissingVulkanExternalMemoryFd,
    MissingGlFunction,
    GlProcLoaderUnavailable,
    InvalidDimensions,
    Vulkan,
    GlResource,
    GlOperation,
    GlFramebufferIncomplete,
    UnsupportedPlatform,
    ImportSlotsExhausted,
    Other,
}

impl ServoGpuImportFallbackReason {
    #[cfg(feature = "servo-gpu-import")]
    const fn as_u64(self) -> u64 {
        match self {
            Self::DeviceUnavailable => 1,
            Self::MissingWgpuVulkanDevice => 2,
            Self::MissingVulkanExternalMemoryFd => 3,
            Self::MissingGlFunction => 4,
            Self::GlProcLoaderUnavailable => 5,
            Self::InvalidDimensions => 6,
            Self::Vulkan => 7,
            Self::GlResource => 8,
            Self::GlOperation => 9,
            Self::GlFramebufferIncomplete => 10,
            Self::UnsupportedPlatform => 11,
            Self::ImportSlotsExhausted => 12,
            Self::Other => 13,
        }
    }

    const fn from_u64(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::DeviceUnavailable),
            2 => Some(Self::MissingWgpuVulkanDevice),
            3 => Some(Self::MissingVulkanExternalMemoryFd),
            4 => Some(Self::MissingGlFunction),
            5 => Some(Self::GlProcLoaderUnavailable),
            6 => Some(Self::InvalidDimensions),
            7 => Some(Self::Vulkan),
            8 => Some(Self::GlResource),
            9 => Some(Self::GlOperation),
            10 => Some(Self::GlFramebufferIncomplete),
            11 => Some(Self::UnsupportedPlatform),
            12 => Some(Self::ImportSlotsExhausted),
            13 => Some(Self::Other),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceUnavailable => "device_unavailable",
            Self::MissingWgpuVulkanDevice => "missing_wgpu_vulkan_device",
            Self::MissingVulkanExternalMemoryFd => "missing_vulkan_external_memory_fd",
            Self::MissingGlFunction => "missing_gl_function",
            Self::GlProcLoaderUnavailable => "gl_proc_loader_unavailable",
            Self::InvalidDimensions => "invalid_dimensions",
            Self::Vulkan => "vulkan_error",
            Self::GlResource => "gl_resource_error",
            Self::GlOperation => "gl_operation_error",
            Self::GlFramebufferIncomplete => "gl_framebuffer_incomplete",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::ImportSlotsExhausted => "import_slots_exhausted",
            Self::Other => "other",
        }
    }
}

#[must_use]
pub fn servo_telemetry_snapshot() -> ServoTelemetrySnapshot {
    ServoTelemetrySnapshot {
        soft_stalls_total: SERVO_SOFT_STALLS_TOTAL.load(Ordering::Relaxed),
        breaker_opens_total: SERVO_BREAKER_OPENS_TOTAL.load(Ordering::Relaxed),
        session_creates_total: SERVO_SESSION_CREATES_TOTAL.load(Ordering::Relaxed),
        session_create_failures_total: SERVO_SESSION_CREATE_FAILURES_TOTAL.load(Ordering::Relaxed),
        session_create_wait_total_us: SERVO_SESSION_CREATE_WAIT_TOTAL_US.load(Ordering::Relaxed),
        session_create_wait_max_us: SERVO_SESSION_CREATE_WAIT_MAX_US.load(Ordering::Relaxed),
        page_loads_total: SERVO_PAGE_LOADS_TOTAL.load(Ordering::Relaxed),
        page_load_failures_total: SERVO_PAGE_LOAD_FAILURES_TOTAL.load(Ordering::Relaxed),
        page_load_wait_total_us: SERVO_PAGE_LOAD_WAIT_TOTAL_US.load(Ordering::Relaxed),
        page_load_wait_max_us: SERVO_PAGE_LOAD_WAIT_MAX_US.load(Ordering::Relaxed),
        detached_destroys_total: SERVO_DETACHED_DESTROYS_TOTAL.load(Ordering::Relaxed),
        detached_destroy_failures_total: SERVO_DETACHED_DESTROY_FAILURES_TOTAL
            .load(Ordering::Relaxed),
        render_requests_total: SERVO_RENDER_REQUESTS_TOTAL.load(Ordering::Relaxed),
        render_queue_wait_total_us: SERVO_RENDER_QUEUE_WAIT_TOTAL_US.load(Ordering::Relaxed),
        render_queue_wait_max_us: SERVO_RENDER_QUEUE_WAIT_MAX_US.load(Ordering::Relaxed),
        render_cpu_frames_total: SERVO_RENDER_CPU_FRAMES_TOTAL.load(Ordering::Relaxed),
        render_cached_frames_total: SERVO_RENDER_CACHED_FRAMES_TOTAL.load(Ordering::Relaxed),
        render_gpu_frames_total: SERVO_RENDER_GPU_FRAMES_TOTAL.load(Ordering::Relaxed),
        render_gpu_import_failures_total: SERVO_RENDER_GPU_IMPORT_FAILURES_TOTAL
            .load(Ordering::Relaxed),
        render_gpu_import_fallbacks_total: SERVO_RENDER_GPU_IMPORT_FALLBACKS_TOTAL
            .load(Ordering::Relaxed),
        render_gpu_import_fallback_reason: ServoGpuImportFallbackReason::from_u64(
            SERVO_RENDER_GPU_IMPORT_FALLBACK_REASON.load(Ordering::Relaxed),
        )
        .map(ServoGpuImportFallbackReason::as_str),
        render_gpu_import_blit_total_us: SERVO_RENDER_GPU_IMPORT_BLIT_TOTAL_US
            .load(Ordering::Relaxed),
        render_gpu_import_blit_max_us: SERVO_RENDER_GPU_IMPORT_BLIT_MAX_US.load(Ordering::Relaxed),
        render_gpu_import_sync_total_us: SERVO_RENDER_GPU_IMPORT_SYNC_TOTAL_US
            .load(Ordering::Relaxed),
        render_gpu_import_sync_max_us: SERVO_RENDER_GPU_IMPORT_SYNC_MAX_US.load(Ordering::Relaxed),
        render_gpu_import_total_us: SERVO_RENDER_GPU_IMPORT_TOTAL_US.load(Ordering::Relaxed),
        render_gpu_import_max_us: SERVO_RENDER_GPU_IMPORT_MAX_US.load(Ordering::Relaxed),
        render_evaluate_scripts_total_us: SERVO_RENDER_EVALUATE_SCRIPTS_TOTAL_US
            .load(Ordering::Relaxed),
        render_evaluate_scripts_max_us: SERVO_RENDER_EVALUATE_SCRIPTS_MAX_US
            .load(Ordering::Relaxed),
        render_event_loop_total_us: SERVO_RENDER_EVENT_LOOP_TOTAL_US.load(Ordering::Relaxed),
        render_event_loop_max_us: SERVO_RENDER_EVENT_LOOP_MAX_US.load(Ordering::Relaxed),
        render_paint_total_us: SERVO_RENDER_PAINT_TOTAL_US.load(Ordering::Relaxed),
        render_paint_max_us: SERVO_RENDER_PAINT_MAX_US.load(Ordering::Relaxed),
        render_readback_total_us: SERVO_RENDER_READBACK_TOTAL_US.load(Ordering::Relaxed),
        render_readback_max_us: SERVO_RENDER_READBACK_MAX_US.load(Ordering::Relaxed),
        render_frame_total_us: SERVO_RENDER_FRAME_TOTAL_US.load(Ordering::Relaxed),
        render_frame_max_us: SERVO_RENDER_FRAME_MAX_US.load(Ordering::Relaxed),
    }
}

fn record_wait(wait: Duration, count: &AtomicU64, total_us: &AtomicU64, max_us: &AtomicU64) {
    let wait_us = u64::try_from(wait.as_micros()).unwrap_or(u64::MAX);
    let _ = count.fetch_add(1, Ordering::Relaxed);
    let _ = total_us.fetch_add(wait_us, Ordering::Relaxed);
    let _ = max_us.fetch_max(wait_us, Ordering::Relaxed);
}

fn record_duration_us(value_us: u64, total_us: &AtomicU64, max_us: &AtomicU64) {
    let _ = total_us.fetch_add(value_us, Ordering::Relaxed);
    let _ = max_us.fetch_max(value_us, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static TELEMETRY_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn cpu_render_frame_metrics_accumulate_stage_timings() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .expect("telemetry tests should not poison lock");
        let before = servo_telemetry_snapshot();

        record_servo_cpu_render_frame(11, 22, 33, 44, 110, true);

        let after = servo_telemetry_snapshot();
        assert!(after.render_cpu_frames_total > before.render_cpu_frames_total);
        assert!(after.render_cached_frames_total > before.render_cached_frames_total);
        assert!(
            after.render_evaluate_scripts_total_us >= before.render_evaluate_scripts_total_us + 11
        );
        assert!(after.render_event_loop_total_us >= before.render_event_loop_total_us + 22);
        assert!(after.render_paint_total_us >= before.render_paint_total_us + 33);
        assert!(after.render_readback_total_us >= before.render_readback_total_us + 44);
        assert!(after.render_frame_total_us >= before.render_frame_total_us + 110);
        assert!(after.render_evaluate_scripts_max_us >= 11);
        assert!(after.render_event_loop_max_us >= 22);
        assert!(after.render_paint_max_us >= 33);
        assert!(after.render_readback_max_us >= 44);
        assert!(after.render_frame_max_us >= 110);
    }

    #[test]
    fn gpu_render_frame_metrics_do_not_count_cpu_readback() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .expect("telemetry tests should not poison lock");
        let before = servo_telemetry_snapshot();

        record_servo_gpu_render_frame(11, 22, 33, 110);

        let after = servo_telemetry_snapshot();
        assert_eq!(
            after.render_cpu_frames_total,
            before.render_cpu_frames_total
        );
        assert_eq!(
            after.render_cached_frames_total,
            before.render_cached_frames_total
        );
        assert_eq!(
            after.render_readback_total_us,
            before.render_readback_total_us
        );
        assert!(
            after.render_evaluate_scripts_total_us >= before.render_evaluate_scripts_total_us + 11
        );
        assert!(after.render_event_loop_total_us >= before.render_event_loop_total_us + 22);
        assert!(after.render_paint_total_us >= before.render_paint_total_us + 33);
        assert!(after.render_frame_total_us >= before.render_frame_total_us + 110);
    }

    #[cfg(feature = "servo-gpu-import")]
    #[test]
    fn gpu_import_metrics_accumulate_timings_and_fallback_reason() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .expect("telemetry tests should not poison lock");
        let before = servo_telemetry_snapshot();

        record_servo_gpu_import_frame(10, 20, 40);
        record_servo_gpu_import_failure(ServoGpuImportFallbackReason::MissingGlFunction, true);

        let after = servo_telemetry_snapshot();
        assert!(after.render_gpu_frames_total > before.render_gpu_frames_total);
        assert!(after.render_gpu_import_failures_total > before.render_gpu_import_failures_total);
        assert!(after.render_gpu_import_fallbacks_total > before.render_gpu_import_fallbacks_total);
        assert_eq!(
            after.render_gpu_import_fallback_reason,
            Some("missing_gl_function")
        );
        assert!(
            after.render_gpu_import_blit_total_us >= before.render_gpu_import_blit_total_us + 10
        );
        assert!(
            after.render_gpu_import_sync_total_us >= before.render_gpu_import_sync_total_us + 20
        );
        assert!(after.render_gpu_import_total_us >= before.render_gpu_import_total_us + 40);
        assert!(after.render_gpu_import_blit_max_us >= 10);
        assert!(after.render_gpu_import_sync_max_us >= 20);
        assert!(after.render_gpu_import_max_us >= 40);
    }

    #[cfg(feature = "servo-gpu-import")]
    #[test]
    fn gpu_import_slot_exhaustion_reason_roundtrips() {
        assert_eq!(
            ServoGpuImportFallbackReason::from_u64(12),
            Some(ServoGpuImportFallbackReason::ImportSlotsExhausted)
        );
        assert_eq!(
            ServoGpuImportFallbackReason::ImportSlotsExhausted.as_str(),
            "import_slots_exhausted"
        );
    }
}
