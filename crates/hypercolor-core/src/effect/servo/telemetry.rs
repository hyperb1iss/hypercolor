use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::worker_client::ServoProducerRole;

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
    pub renderer_loads_total: u64,
    pub renderer_load_failures_total: u64,
    pub renderer_load_wait_total_us: u64,
    pub renderer_load_wait_max_us: u64,
    pub detached_destroys_total: u64,
    pub detached_destroy_failures_total: u64,
    pub destroy_wait_total_us: u64,
    pub destroy_wait_max_us: u64,
    pub render_requests_total: u64,
    pub render_queue_wait_total_us: u64,
    pub render_queue_wait_max_us: u64,
    pub render_scene_requests_total: u64,
    pub render_scene_queue_wait_total_us: u64,
    pub render_scene_queue_wait_max_us: u64,
    pub render_display_requests_total: u64,
    pub render_display_queue_wait_total_us: u64,
    pub render_display_queue_wait_max_us: u64,
    pub render_queue_depth: u64,
    pub render_queue_depth_max: u64,
    pub render_superseded_total: u64,
    pub render_pending_age_max_us: u64,
    pub render_cpu_frames_total: u64,
    pub render_cached_frames_total: u64,
    pub render_gpu_frames_total: u64,
    pub render_gpu_import_failures_total: u64,
    pub render_gpu_import_fallbacks_total: u64,
    pub render_gpu_import_fallback_reason: Option<&'static str>,
    pub render_gpu_import_windows_sync_mode: Option<&'static str>,
    pub render_gpu_import_stale_frame_total: u64,
    pub render_gpu_import_adapter_mismatch_total: u64,
    pub render_gpu_import_slot_count: u64,
    pub render_gpu_import_pending_slots: u64,
    pub render_gpu_import_pending_slots_max: u64,
    pub render_gpu_import_completed_slots: u64,
    pub render_gpu_import_available_slots: u64,
    pub render_gpu_import_available_slots_min: u64,
    pub render_gpu_import_oldest_pending_age_max_us: u64,
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
static SERVO_RENDERER_LOADS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDERER_LOAD_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDERER_LOAD_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDERER_LOAD_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_DETACHED_DESTROYS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_DETACHED_DESTROY_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_DESTROY_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_DESTROY_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_SCENE_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_SCENE_QUEUE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_SCENE_QUEUE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_DISPLAY_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_DISPLAY_QUEUE_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_DISPLAY_QUEUE_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_DEPTH: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_QUEUE_DEPTH_MAX: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_SUPERSEDED_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_PENDING_AGE_MAX_US: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_CPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_CACHED_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_FRAMES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_FALLBACKS_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_FALLBACK_REASON: AtomicU64 = AtomicU64::new(0);
// These classify import failures before CPU-fallback filtering.
static SERVO_RENDER_GPU_IMPORT_STALE_FRAME_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_ADAPTER_MISMATCH_TOTAL: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_SLOT_COUNT: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_PENDING_SLOTS: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_PENDING_SLOTS_MAX: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_COMPLETED_SLOTS: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_AVAILABLE_SLOTS: AtomicU64 = AtomicU64::new(0);
static SERVO_RENDER_GPU_IMPORT_AVAILABLE_SLOTS_MIN: AtomicU64 = AtomicU64::new(u64::MAX);
static SERVO_RENDER_GPU_IMPORT_OLDEST_PENDING_AGE_MAX_US: AtomicU64 = AtomicU64::new(0);
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

pub(super) fn record_servo_renderer_load(wait: Duration, success: bool) {
    record_wait(
        wait,
        &SERVO_RENDERER_LOADS_TOTAL,
        &SERVO_RENDERER_LOAD_WAIT_TOTAL_US,
        &SERVO_RENDERER_LOAD_WAIT_MAX_US,
    );
    if !success {
        let _ = SERVO_RENDERER_LOAD_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_detached_destroy(success: bool) {
    let _ = SERVO_DETACHED_DESTROYS_TOTAL.fetch_add(1, Ordering::Relaxed);
    if !success {
        let _ = SERVO_DETACHED_DESTROY_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn record_servo_destroy_wait(wait: Duration) {
    record_duration(
        wait,
        &SERVO_DESTROY_WAIT_TOTAL_US,
        &SERVO_DESTROY_WAIT_MAX_US,
    );
}

pub(super) fn record_servo_render_queue_wait(producer_role: ServoProducerRole, wait: Duration) {
    record_wait(
        wait,
        &SERVO_RENDER_REQUESTS_TOTAL,
        &SERVO_RENDER_QUEUE_WAIT_TOTAL_US,
        &SERVO_RENDER_QUEUE_WAIT_MAX_US,
    );
    match producer_role {
        ServoProducerRole::SceneHtml => record_wait(
            wait,
            &SERVO_RENDER_SCENE_REQUESTS_TOTAL,
            &SERVO_RENDER_SCENE_QUEUE_WAIT_TOTAL_US,
            &SERVO_RENDER_SCENE_QUEUE_WAIT_MAX_US,
        ),
        ServoProducerRole::DisplayFaceHtml => record_wait(
            wait,
            &SERVO_RENDER_DISPLAY_REQUESTS_TOTAL,
            &SERVO_RENDER_DISPLAY_QUEUE_WAIT_TOTAL_US,
            &SERVO_RENDER_DISPLAY_QUEUE_WAIT_MAX_US,
        ),
    }
}

pub(super) fn record_servo_render_queue_depth(depth: usize) {
    let depth = u64::try_from(depth).unwrap_or(u64::MAX);
    SERVO_RENDER_QUEUE_DEPTH.store(depth, Ordering::Relaxed);
    let _ = SERVO_RENDER_QUEUE_DEPTH_MAX.fetch_max(depth, Ordering::Relaxed);
}

pub(super) fn record_servo_render_superseded() {
    let _ = SERVO_RENDER_SUPERSEDED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_servo_pending_render_age(age: Duration) {
    let age_us = u64::try_from(age.as_micros()).unwrap_or(u64::MAX);
    let _ = SERVO_RENDER_PENDING_AGE_MAX_US.fetch_max(age_us, Ordering::Relaxed);
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
    match reason {
        ServoGpuImportFallbackReason::WindowsImportStaleFrame => {
            let _ = SERVO_RENDER_GPU_IMPORT_STALE_FRAME_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        ServoGpuImportFallbackReason::AdapterLuidMismatch => {
            let _ = SERVO_RENDER_GPU_IMPORT_ADAPTER_MISMATCH_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
    if fell_back_to_cpu {
        let _ = SERVO_RENDER_GPU_IMPORT_FALLBACKS_TOTAL.fetch_add(1, Ordering::Relaxed);
        SERVO_RENDER_GPU_IMPORT_FALLBACK_REASON.store(reason.as_u64(), Ordering::Relaxed);
    }
}

#[cfg(feature = "servo-gpu-import")]
pub(super) fn record_servo_gpu_import_slot_state(
    slot_count: usize,
    pending_slots: usize,
    completed_slots: usize,
    available_slots: usize,
    oldest_pending_age_ms: Option<u64>,
) {
    let slot_count = u64::try_from(slot_count).unwrap_or(u64::MAX);
    let pending_slots = u64::try_from(pending_slots).unwrap_or(u64::MAX);
    let completed_slots = u64::try_from(completed_slots).unwrap_or(u64::MAX);
    let available_slots = u64::try_from(available_slots).unwrap_or(u64::MAX);
    SERVO_RENDER_GPU_IMPORT_SLOT_COUNT.store(slot_count, Ordering::Relaxed);
    SERVO_RENDER_GPU_IMPORT_PENDING_SLOTS.store(pending_slots, Ordering::Relaxed);
    let _ = SERVO_RENDER_GPU_IMPORT_PENDING_SLOTS_MAX.fetch_max(pending_slots, Ordering::Relaxed);
    SERVO_RENDER_GPU_IMPORT_COMPLETED_SLOTS.store(completed_slots, Ordering::Relaxed);
    SERVO_RENDER_GPU_IMPORT_AVAILABLE_SLOTS.store(available_slots, Ordering::Relaxed);
    let _ =
        SERVO_RENDER_GPU_IMPORT_AVAILABLE_SLOTS_MIN.fetch_min(available_slots, Ordering::Relaxed);
    if let Some(oldest_pending_age_ms) = oldest_pending_age_ms {
        let oldest_pending_age_us = oldest_pending_age_ms.saturating_mul(1_000);
        let _ = SERVO_RENDER_GPU_IMPORT_OLDEST_PENDING_AGE_MAX_US
            .fetch_max(oldest_pending_age_us, Ordering::Relaxed);
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
    MissingWgpuMetalDevice,
    MissingMacosServoSurface,
    IosurfacePixelFormatMismatch,
    MetalTextureCreateFailed,
    Other,
    MissingVulkanExternalMemoryWin32,
    MissingWindowsAngleContext,
    D3d11DeviceCreateFailed,
    D3d11SharedTextureCreateFailed,
    D3d11SharedHandleCreateFailed,
    AngleClientBufferSurfaceFailed,
    AdapterLuidMismatch,
    VulkanD3d11ImportFailed,
    WindowsImportStaleFrame,
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
            Self::MissingWgpuMetalDevice => 13,
            Self::MissingMacosServoSurface => 14,
            Self::IosurfacePixelFormatMismatch => 15,
            Self::MetalTextureCreateFailed => 16,
            Self::Other => 17,
            Self::MissingVulkanExternalMemoryWin32 => 18,
            Self::MissingWindowsAngleContext => 19,
            Self::D3d11DeviceCreateFailed => 20,
            Self::D3d11SharedTextureCreateFailed => 21,
            Self::D3d11SharedHandleCreateFailed => 22,
            Self::AngleClientBufferSurfaceFailed => 23,
            Self::AdapterLuidMismatch => 24,
            Self::VulkanD3d11ImportFailed => 25,
            Self::WindowsImportStaleFrame => 26,
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
            13 => Some(Self::MissingWgpuMetalDevice),
            14 => Some(Self::MissingMacosServoSurface),
            15 => Some(Self::IosurfacePixelFormatMismatch),
            16 => Some(Self::MetalTextureCreateFailed),
            17 => Some(Self::Other),
            18 => Some(Self::MissingVulkanExternalMemoryWin32),
            19 => Some(Self::MissingWindowsAngleContext),
            20 => Some(Self::D3d11DeviceCreateFailed),
            21 => Some(Self::D3d11SharedTextureCreateFailed),
            22 => Some(Self::D3d11SharedHandleCreateFailed),
            23 => Some(Self::AngleClientBufferSurfaceFailed),
            24 => Some(Self::AdapterLuidMismatch),
            25 => Some(Self::VulkanD3d11ImportFailed),
            26 => Some(Self::WindowsImportStaleFrame),
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
            Self::MissingWgpuMetalDevice => "missing_wgpu_metal_device",
            Self::MissingMacosServoSurface => "missing_macos_servo_surface",
            Self::IosurfacePixelFormatMismatch => "iosurface_pixel_format_mismatch",
            Self::MetalTextureCreateFailed => "metal_texture_create_failed",
            Self::Other => "other",
            Self::MissingVulkanExternalMemoryWin32 => "missing_vulkan_external_memory_win32",
            Self::MissingWindowsAngleContext => "missing_windows_angle_context",
            Self::D3d11DeviceCreateFailed => "d3d11_device_create_failed",
            Self::D3d11SharedTextureCreateFailed => "d3d11_shared_texture_create_failed",
            Self::D3d11SharedHandleCreateFailed => "d3d11_shared_handle_create_failed",
            Self::AngleClientBufferSurfaceFailed => "angle_client_buffer_surface_failed",
            Self::AdapterLuidMismatch => "adapter_luid_mismatch",
            Self::VulkanD3d11ImportFailed => "vulkan_d3d11_import_failed",
            Self::WindowsImportStaleFrame => "windows_import_stale_frame",
        }
    }
}

#[must_use]
pub fn servo_telemetry_snapshot() -> ServoTelemetrySnapshot {
    let available_slots_min = SERVO_RENDER_GPU_IMPORT_AVAILABLE_SLOTS_MIN.load(Ordering::Relaxed);
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
        renderer_loads_total: SERVO_RENDERER_LOADS_TOTAL.load(Ordering::Relaxed),
        renderer_load_failures_total: SERVO_RENDERER_LOAD_FAILURES_TOTAL.load(Ordering::Relaxed),
        renderer_load_wait_total_us: SERVO_RENDERER_LOAD_WAIT_TOTAL_US.load(Ordering::Relaxed),
        renderer_load_wait_max_us: SERVO_RENDERER_LOAD_WAIT_MAX_US.load(Ordering::Relaxed),
        detached_destroys_total: SERVO_DETACHED_DESTROYS_TOTAL.load(Ordering::Relaxed),
        detached_destroy_failures_total: SERVO_DETACHED_DESTROY_FAILURES_TOTAL
            .load(Ordering::Relaxed),
        destroy_wait_total_us: SERVO_DESTROY_WAIT_TOTAL_US.load(Ordering::Relaxed),
        destroy_wait_max_us: SERVO_DESTROY_WAIT_MAX_US.load(Ordering::Relaxed),
        render_requests_total: SERVO_RENDER_REQUESTS_TOTAL.load(Ordering::Relaxed),
        render_queue_wait_total_us: SERVO_RENDER_QUEUE_WAIT_TOTAL_US.load(Ordering::Relaxed),
        render_queue_wait_max_us: SERVO_RENDER_QUEUE_WAIT_MAX_US.load(Ordering::Relaxed),
        render_scene_requests_total: SERVO_RENDER_SCENE_REQUESTS_TOTAL.load(Ordering::Relaxed),
        render_scene_queue_wait_total_us: SERVO_RENDER_SCENE_QUEUE_WAIT_TOTAL_US
            .load(Ordering::Relaxed),
        render_scene_queue_wait_max_us: SERVO_RENDER_SCENE_QUEUE_WAIT_MAX_US
            .load(Ordering::Relaxed),
        render_display_requests_total: SERVO_RENDER_DISPLAY_REQUESTS_TOTAL.load(Ordering::Relaxed),
        render_display_queue_wait_total_us: SERVO_RENDER_DISPLAY_QUEUE_WAIT_TOTAL_US
            .load(Ordering::Relaxed),
        render_display_queue_wait_max_us: SERVO_RENDER_DISPLAY_QUEUE_WAIT_MAX_US
            .load(Ordering::Relaxed),
        render_queue_depth: SERVO_RENDER_QUEUE_DEPTH.load(Ordering::Relaxed),
        render_queue_depth_max: SERVO_RENDER_QUEUE_DEPTH_MAX.load(Ordering::Relaxed),
        render_superseded_total: SERVO_RENDER_SUPERSEDED_TOTAL.load(Ordering::Relaxed),
        render_pending_age_max_us: SERVO_RENDER_PENDING_AGE_MAX_US.load(Ordering::Relaxed),
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
        render_gpu_import_windows_sync_mode: windows_gpu_import_sync_mode(),
        render_gpu_import_stale_frame_total: SERVO_RENDER_GPU_IMPORT_STALE_FRAME_TOTAL
            .load(Ordering::Relaxed),
        render_gpu_import_adapter_mismatch_total: SERVO_RENDER_GPU_IMPORT_ADAPTER_MISMATCH_TOTAL
            .load(Ordering::Relaxed),
        render_gpu_import_slot_count: SERVO_RENDER_GPU_IMPORT_SLOT_COUNT.load(Ordering::Relaxed),
        render_gpu_import_pending_slots: SERVO_RENDER_GPU_IMPORT_PENDING_SLOTS
            .load(Ordering::Relaxed),
        render_gpu_import_pending_slots_max: SERVO_RENDER_GPU_IMPORT_PENDING_SLOTS_MAX
            .load(Ordering::Relaxed),
        render_gpu_import_completed_slots: SERVO_RENDER_GPU_IMPORT_COMPLETED_SLOTS
            .load(Ordering::Relaxed),
        render_gpu_import_available_slots: SERVO_RENDER_GPU_IMPORT_AVAILABLE_SLOTS
            .load(Ordering::Relaxed),
        render_gpu_import_available_slots_min: if available_slots_min == u64::MAX {
            0
        } else {
            available_slots_min
        },
        render_gpu_import_oldest_pending_age_max_us:
            SERVO_RENDER_GPU_IMPORT_OLDEST_PENDING_AGE_MAX_US.load(Ordering::Relaxed),
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

#[cfg(all(feature = "servo-gpu-import", target_os = "windows"))]
const fn windows_gpu_import_sync_mode() -> Option<&'static str> {
    Some(hypercolor_windows_gpu_interop::WINDOWS_SERVO_GPU_IMPORT_SYNC_MODE)
}

#[cfg(not(all(feature = "servo-gpu-import", target_os = "windows")))]
const fn windows_gpu_import_sync_mode() -> Option<&'static str> {
    None
}

fn record_wait(wait: Duration, count: &AtomicU64, total_us: &AtomicU64, max_us: &AtomicU64) {
    let wait_us = u64::try_from(wait.as_micros()).unwrap_or(u64::MAX);
    let _ = count.fetch_add(1, Ordering::Relaxed);
    let _ = total_us.fetch_add(wait_us, Ordering::Relaxed);
    let _ = max_us.fetch_max(wait_us, Ordering::Relaxed);
}

fn record_duration(duration: Duration, total_us: &AtomicU64, max_us: &AtomicU64) {
    let duration_us = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
    let _ = total_us.fetch_add(duration_us, Ordering::Relaxed);
    let _ = max_us.fetch_max(duration_us, Ordering::Relaxed);
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

    #[test]
    fn servo_queue_lifecycle_metrics_accumulate() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .expect("telemetry tests should not poison lock");
        let before = servo_telemetry_snapshot();

        record_servo_renderer_load(Duration::from_micros(15), false);
        record_servo_destroy_wait(Duration::from_micros(25));
        record_servo_render_queue_wait(ServoProducerRole::SceneHtml, Duration::from_micros(45));
        record_servo_render_queue_wait(
            ServoProducerRole::DisplayFaceHtml,
            Duration::from_micros(55),
        );
        record_servo_render_queue_depth(3);
        record_servo_render_superseded();
        record_servo_pending_render_age(Duration::from_micros(35));

        let after = servo_telemetry_snapshot();
        assert!(after.renderer_loads_total > before.renderer_loads_total);
        assert!(after.renderer_load_failures_total > before.renderer_load_failures_total);
        assert!(after.renderer_load_wait_total_us >= before.renderer_load_wait_total_us + 15);
        assert!(after.renderer_load_wait_max_us >= 15);
        assert!(after.destroy_wait_total_us >= before.destroy_wait_total_us + 25);
        assert!(after.destroy_wait_max_us >= 25);
        assert!(after.render_requests_total >= before.render_requests_total + 2);
        assert!(after.render_queue_wait_total_us >= before.render_queue_wait_total_us + 100);
        assert!(after.render_scene_requests_total > before.render_scene_requests_total);
        assert!(
            after.render_scene_queue_wait_total_us >= before.render_scene_queue_wait_total_us + 45
        );
        assert!(after.render_scene_queue_wait_max_us >= 45);
        assert!(after.render_display_requests_total > before.render_display_requests_total);
        assert!(
            after.render_display_queue_wait_total_us
                >= before.render_display_queue_wait_total_us + 55
        );
        assert!(after.render_display_queue_wait_max_us >= 55);
        assert!(after.render_queue_depth_max >= 3);
        assert!(after.render_queue_depth <= after.render_queue_depth_max);
        assert!(after.render_superseded_total > before.render_superseded_total);
        assert!(after.render_pending_age_max_us >= 35);
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
        record_servo_gpu_import_failure(
            ServoGpuImportFallbackReason::WindowsImportStaleFrame,
            false,
        );
        record_servo_gpu_import_failure(ServoGpuImportFallbackReason::AdapterLuidMismatch, false);
        record_servo_gpu_import_slot_state(8, 3, 5, 2, Some(17));

        let after = servo_telemetry_snapshot();
        assert!(after.render_gpu_frames_total > before.render_gpu_frames_total);
        assert!(after.render_gpu_import_failures_total > before.render_gpu_import_failures_total);
        assert!(after.render_gpu_import_fallbacks_total > before.render_gpu_import_fallbacks_total);
        assert!(
            after.render_gpu_import_stale_frame_total > before.render_gpu_import_stale_frame_total
        );
        assert!(
            after.render_gpu_import_adapter_mismatch_total
                > before.render_gpu_import_adapter_mismatch_total
        );
        assert_eq!(after.render_gpu_import_slot_count, 8);
        assert_eq!(after.render_gpu_import_pending_slots, 3);
        assert!(after.render_gpu_import_pending_slots_max >= 3);
        assert_eq!(after.render_gpu_import_completed_slots, 5);
        assert_eq!(after.render_gpu_import_available_slots, 2);
        assert!(after.render_gpu_import_available_slots_min <= 2);
        assert!(after.render_gpu_import_oldest_pending_age_max_us >= 17_000);
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
