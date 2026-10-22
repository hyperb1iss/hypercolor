//! GPU compositor telemetry counters.

use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::time::Duration;

use hypercolor_types::api::system::GpuTransferStatus;
use hypercolor_types::canvas::BYTES_PER_PIXEL;

/// Completed GPU mappings consumed by CPU readers, classified by their purpose.
pub(crate) enum GpuReadbackKind {
    Preview,
    Display,
    Led,
    SamplingCanvas,
}

#[derive(Default)]
struct ReadbackCounter {
    count: AtomicU64,
    bytes: AtomicU64,
}

static PREVIEW_READBACK: ReadbackCounter = ReadbackCounter {
    count: AtomicU64::new(0),
    bytes: AtomicU64::new(0),
};
static DISPLAY_READBACK: ReadbackCounter = ReadbackCounter {
    count: AtomicU64::new(0),
    bytes: AtomicU64::new(0),
};
static LED_READBACK: ReadbackCounter = ReadbackCounter {
    count: AtomicU64::new(0),
    bytes: AtomicU64::new(0),
};
static SAMPLING_CANVAS_READBACK: ReadbackCounter = ReadbackCounter {
    count: AtomicU64::new(0),
    bytes: AtomicU64::new(0),
};

pub(crate) fn record_gpu_readback(kind: GpuReadbackKind, bytes: u64) {
    let counter = match kind {
        GpuReadbackKind::Preview => &PREVIEW_READBACK,
        GpuReadbackKind::Display => &DISPLAY_READBACK,
        GpuReadbackKind::Led => &LED_READBACK,
        GpuReadbackKind::SamplingCanvas => &SAMPLING_CANVAS_READBACK,
    };
    counter.bytes.fetch_add(bytes, Ordering::Relaxed);
    counter.count.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn gpu_transfer_telemetry_snapshot() -> GpuTransferStatus {
    GpuTransferStatus {
        preview_readback_count: PREVIEW_READBACK.count.load(Ordering::Relaxed),
        preview_readback_bytes: PREVIEW_READBACK.bytes.load(Ordering::Relaxed),
        display_readback_count: DISPLAY_READBACK.count.load(Ordering::Relaxed),
        display_readback_bytes: DISPLAY_READBACK.bytes.load(Ordering::Relaxed),
        led_readback_count: LED_READBACK.count.load(Ordering::Relaxed),
        led_readback_bytes: LED_READBACK.bytes.load(Ordering::Relaxed),
        sampling_canvas_readback_count: SAMPLING_CANVAS_READBACK.count.load(Ordering::Relaxed),
        sampling_canvas_readback_bytes: SAMPLING_CANVAS_READBACK.bytes.load(Ordering::Relaxed),
    }
}

static GPU_SOURCE_UPLOAD_SKIPPED_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_MEDIA_TEXTURE_ALLOCATIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_MEDIA_TEXTURE_UPLOAD_BYTES_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_RGBA_ATTEMPTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_YUV_ATTEMPTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_SUCCESSES_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_MISSES_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_LATCHES_TOTAL: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_BLOCKING_WAIT_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_BLOCKING_WAIT_MAX_US: AtomicU64 = AtomicU64::new(0);
static GPU_DISPLAY_FINALIZE_SURFACE_REALLOCS_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GpuSparkleFlingerTelemetrySnapshot {
    pub(crate) source_upload_skipped_total: u64,
    pub(crate) media_texture_allocations_total: u64,
    pub(crate) media_texture_upload_bytes_total: u64,
    pub(crate) display_finalize_rgba_attempts_total: u64,
    pub(crate) display_finalize_yuv_attempts_total: u64,
    pub(crate) display_finalize_successes_total: u64,
    pub(crate) display_finalize_misses_total: u64,
    pub(crate) display_finalize_latches_total: u64,
    pub(crate) display_finalize_blocking_wait_total_us: u64,
    pub(crate) display_finalize_blocking_wait_max_us: u64,
    pub(crate) display_finalize_surface_reallocs_total: u64,
}

pub(crate) fn gpu_sparkleflinger_telemetry_snapshot() -> GpuSparkleFlingerTelemetrySnapshot {
    GpuSparkleFlingerTelemetrySnapshot {
        source_upload_skipped_total: GPU_SOURCE_UPLOAD_SKIPPED_TOTAL.load(Ordering::Relaxed),
        media_texture_allocations_total: GPU_MEDIA_TEXTURE_ALLOCATIONS_TOTAL
            .load(Ordering::Relaxed),
        media_texture_upload_bytes_total: GPU_MEDIA_TEXTURE_UPLOAD_BYTES_TOTAL
            .load(Ordering::Relaxed),
        display_finalize_rgba_attempts_total: GPU_DISPLAY_FINALIZE_RGBA_ATTEMPTS_TOTAL
            .load(Ordering::Relaxed),
        display_finalize_yuv_attempts_total: GPU_DISPLAY_FINALIZE_YUV_ATTEMPTS_TOTAL
            .load(Ordering::Relaxed),
        display_finalize_successes_total: GPU_DISPLAY_FINALIZE_SUCCESSES_TOTAL
            .load(Ordering::Relaxed),
        display_finalize_misses_total: GPU_DISPLAY_FINALIZE_MISSES_TOTAL.load(Ordering::Relaxed),
        display_finalize_latches_total: GPU_DISPLAY_FINALIZE_LATCHES_TOTAL.load(Ordering::Relaxed),
        display_finalize_blocking_wait_total_us: GPU_DISPLAY_FINALIZE_BLOCKING_WAIT_TOTAL_US
            .load(Ordering::Relaxed),
        display_finalize_blocking_wait_max_us: GPU_DISPLAY_FINALIZE_BLOCKING_WAIT_MAX_US
            .load(Ordering::Relaxed),
        display_finalize_surface_reallocs_total: GPU_DISPLAY_FINALIZE_SURFACE_REALLOCS_TOTAL
            .load(Ordering::Relaxed),
    }
}

pub(super) fn record_gpu_source_upload_skipped() {
    let _ = GPU_SOURCE_UPLOAD_SKIPPED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_gpu_media_texture_allocation() {
    let _ = GPU_MEDIA_TEXTURE_ALLOCATIONS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_gpu_media_texture_upload(width: u32, height: u32) {
    let bytes = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(u64::try_from(BYTES_PER_PIXEL).unwrap_or(u64::MAX));
    let _ = GPU_MEDIA_TEXTURE_UPLOAD_BYTES_TOTAL.fetch_add(bytes, Ordering::Relaxed);
}

pub(super) fn record_gpu_display_finalize_attempt(yuv: bool) {
    let counter = if yuv {
        &GPU_DISPLAY_FINALIZE_YUV_ATTEMPTS_TOTAL
    } else {
        &GPU_DISPLAY_FINALIZE_RGBA_ATTEMPTS_TOTAL
    };
    let _ = counter.fetch_add(1, Ordering::Relaxed);
}

pub(super) fn record_gpu_display_finalize_result(success: bool) {
    let counter = if success {
        &GPU_DISPLAY_FINALIZE_SUCCESSES_TOTAL
    } else {
        &GPU_DISPLAY_FINALIZE_MISSES_TOTAL
    };
    let _ = counter.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_gpu_display_finalize_latch() {
    let _ = GPU_DISPLAY_FINALIZE_LATCHES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) fn record_gpu_display_finalize_blocking_wait(wait: Duration) {
    let wait_us = u64::try_from(wait.as_micros()).unwrap_or(u64::MAX);
    let _ = GPU_DISPLAY_FINALIZE_BLOCKING_WAIT_TOTAL_US.fetch_add(wait_us, Ordering::Relaxed);
    let _ = GPU_DISPLAY_FINALIZE_BLOCKING_WAIT_MAX_US.fetch_max(wait_us, Ordering::Relaxed);
}

pub(super) fn record_gpu_display_finalize_surface_realloc() {
    let _ = GPU_DISPLAY_FINALIZE_SURFACE_REALLOCS_TOTAL.fetch_add(1, Ordering::Relaxed);
}
