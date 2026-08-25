use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Error, Result, anyhow};
use hypercolor_gpu_frame::servo::{ServoGpuFrameImporter, ServoGpuImportFailure};
use hypercolor_gpu_frame::{GpuFrameImportError, GpuFrameImportFallbackReason};
use tracing::debug;

use super::telemetry::{record_servo_gpu_import_frame, record_servo_gpu_import_slot_state};
use super::worker_client::ServoSessionId;
use crate::effect::servo_bootstrap::ServoRenderingContextHandle;
use crate::effect::traits::ImportedEffectFrame;

const SERVO_GPU_IMPORT_TRANSIENT_RETRY: Duration = Duration::from_millis(250);

#[derive(Debug, thiserror::Error)]
#[error(
    "Servo GPU framebuffer import is temporarily unavailable: {reason} ({detail}); retry in {retry_ms}ms"
)]
pub(super) struct ServoFrameUnavailable {
    reason: &'static str,
    detail: String,
    retry_ms: u64,
}

impl ServoFrameUnavailable {
    pub(super) const fn new(reason: &'static str, detail: String, retry_ms: u64) -> Self {
        Self {
            reason,
            detail,
            retry_ms,
        }
    }

    pub(super) const fn reason(&self) -> &'static str {
        self.reason
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }

    pub(super) const fn retry_ms(&self) -> u64 {
        self.retry_ms
    }
}

pub(super) struct ServoGpuImportBackend {
    importer: Option<Box<dyn ServoGpuFrameImporter>>,
    retry_after: Option<Instant>,
    transient_failures: u32,
    last_frame: Option<ImportedEffectFrame>,
}

/// Neutral session context logged next to a platform import failure.
pub(super) struct ServoGpuImportSessionContext<'a> {
    pub(super) session_id: ServoSessionId,
    pub(super) loaded_html_path: Option<&'a Path>,
    pub(super) loaded_at: Option<Instant>,
    pub(super) renders_since_load: u64,
}

impl ServoGpuImportBackend {
    pub(super) fn new(handle: &mut ServoRenderingContextHandle) -> Self {
        Self {
            importer: handle.gpu_importer.take(),
            retry_after: None,
            transient_failures: 0,
            last_frame: None,
        }
    }

    pub(super) fn warm_if_available(&mut self, width: u32, height: u32) {
        if !super::servo_gpu_import_should_attempt() {
            return;
        }
        let Ok(device) = super::gpu_import::servo_gpu_import_device() else {
            return;
        };
        let Some(importer) = self.importer.as_mut() else {
            return;
        };
        self.last_frame = None;
        if let Err(error) = importer.warm(device, width, height) {
            debug!(%error, "Servo GPU import pool warmup skipped");
        }
    }

    pub(super) fn import_frame(
        &mut self,
        context: ServoGpuImportSessionContext<'_>,
        width: u32,
        height: u32,
    ) -> Result<ImportedEffectFrame> {
        let device = super::gpu_import::servo_gpu_import_device()?;
        let importer = self
            .importer
            .as_mut()
            .ok_or_else(|| anyhow!("Servo GPU import context is unavailable for this session"))?;
        let result = importer.import_frame(device, width, height);
        if let Some(slot_state) = importer.slot_state() {
            record_servo_gpu_import_slot_state(
                slot_state.slot_count,
                slot_state.pending_slots,
                slot_state.completed_slots,
                slot_state.available_slots,
                slot_state.oldest_pending_age_ms,
            );
        }
        match result {
            Ok(frame) => Ok(frame),
            Err(ServoGpuImportFailure { error, diagnostics }) => {
                let loaded_html_path = context
                    .loaded_html_path
                    .map_or_else(String::new, |path| path.display().to_string());
                debug!(
                    %error,
                    ?context.session_id,
                    width,
                    height,
                    loaded_html_path,
                    page_age_ms = context
                        .loaded_at
                        .map(|loaded_at| duration_millis_u64(loaded_at.elapsed())),
                    renders_since_load = context.renders_since_load,
                    diagnostics = diagnostics.as_deref().unwrap_or_default(),
                    "Servo GPU import failed"
                );
                Err(error)
            }
        }
    }

    pub(super) fn clear_importer(&mut self) {
        if let Some(importer) = self.importer.as_mut() {
            importer.clear();
        }
        self.last_frame = None;
    }

    pub(super) fn reset_retry_state(&mut self) {
        self.retry_after = None;
        self.transient_failures = 0;
        self.last_frame = None;
    }

    pub(super) fn retry_delay(&self, now: Instant) -> Option<Duration> {
        self.retry_after
            .and_then(|retry_after| retry_after.checked_duration_since(now))
    }

    pub(super) fn schedule_transient_retry(&mut self) -> u64 {
        self.retry_after = Some(Instant::now() + SERVO_GPU_IMPORT_TRANSIENT_RETRY);
        self.transient_failures = self.transient_failures.saturating_add(1);
        duration_millis_u64(SERVO_GPU_IMPORT_TRANSIENT_RETRY)
    }

    pub(super) fn transient_failures(&self) -> u32 {
        self.transient_failures
    }

    pub(super) fn note_success(&mut self) {
        self.retry_after = None;
        self.transient_failures = 0;
    }

    pub(super) fn cached_frame(&self) -> Option<&ImportedEffectFrame> {
        self.last_frame.as_ref()
    }

    pub(super) fn store_frame(&mut self, frame: ImportedEffectFrame) {
        self.last_frame = Some(frame);
    }

    pub(super) fn clear_cached_frame(&mut self) {
        self.last_frame = None;
    }

    /// Log what the platform knows about the native surface behind this
    /// session's rendering context, when it exposes one.
    pub(super) fn trace_native_surface(&self, session_id: ServoSessionId) {
        let Some(summary) = self
            .importer
            .as_ref()
            .and_then(|importer| importer.native_surface_summary())
        else {
            return;
        };
        debug!(?session_id, summary, "Servo GPU import native surface");
    }
}

pub(super) fn record_imported_frame(frame: &ImportedEffectFrame) {
    record_servo_gpu_import_frame(
        frame
            .timings
            .blit_us
            .or(frame.timings.wrap_us)
            .unwrap_or_default(),
        frame.timings.sync_us.unwrap_or_default(),
        frame.timings.total_us,
    );
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

pub(super) fn failure_is_transient(reason: GpuFrameImportFallbackReason) -> bool {
    matches!(
        reason,
        GpuFrameImportFallbackReason::GlOperation
            | GpuFrameImportFallbackReason::GlFramebufferIncomplete
            | GpuFrameImportFallbackReason::ImportSlotsExhausted
            | GpuFrameImportFallbackReason::MissingMacosServoSurface
            | GpuFrameImportFallbackReason::WindowsImportStaleFrame
    )
}

pub(super) fn failure_should_clear_importer(reason: GpuFrameImportFallbackReason) -> bool {
    !failure_is_transient(reason)
}

pub(super) fn failure_detail(error: &Error) -> String {
    error.to_string()
}

pub(super) fn classify_failure(error: &Error) -> GpuFrameImportFallbackReason {
    for cause in error.chain() {
        if let Some(error) =
            cause.downcast_ref::<hypercolor_macos_gpu_interop::MacosGpuInteropError>()
        {
            return error.fallback_reason();
        }

        if let Some(error) =
            cause.downcast_ref::<hypercolor_windows_gpu_interop::WindowsGpuInteropError>()
        {
            return error.fallback_reason();
        }

        if let Some(error) =
            cause.downcast_ref::<hypercolor_linux_gpu_interop::LinuxGpuInteropError>()
        {
            return error.fallback_reason();
        }
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("not installed") || message.contains("device is not installed") {
        GpuFrameImportFallbackReason::DeviceUnavailable
    } else {
        GpuFrameImportFallbackReason::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_gpu_import_slot_exhaustion() {
        let error = anyhow::anyhow!(
            hypercolor_linux_gpu_interop::LinuxGpuInteropError::ImportSlotsExhausted {
                slot_count: 8
            }
        );

        assert_eq!(
            classify_failure(&error),
            GpuFrameImportFallbackReason::ImportSlotsExhausted
        );
    }

    #[test]
    fn transient_gpu_import_failures_skip_global_auto_backoff() {
        assert!(failure_is_transient(
            GpuFrameImportFallbackReason::GlOperation
        ));
        assert!(failure_is_transient(
            GpuFrameImportFallbackReason::GlFramebufferIncomplete
        ));
        assert!(failure_is_transient(
            GpuFrameImportFallbackReason::ImportSlotsExhausted
        ));
        assert!(!failure_is_transient(
            GpuFrameImportFallbackReason::MissingWgpuVulkanDevice
        ));
    }

    #[test]
    fn transient_gpu_import_failures_preserve_importer_state() {
        assert!(!failure_should_clear_importer(
            GpuFrameImportFallbackReason::GlOperation
        ));
        assert!(!failure_should_clear_importer(
            GpuFrameImportFallbackReason::GlFramebufferIncomplete
        ));
        assert!(!failure_should_clear_importer(
            GpuFrameImportFallbackReason::ImportSlotsExhausted
        ));
        assert!(failure_should_clear_importer(
            GpuFrameImportFallbackReason::GlResource
        ));
        assert!(failure_should_clear_importer(
            GpuFrameImportFallbackReason::MissingWgpuVulkanDevice
        ));
    }
}
