use super::{
    Arc, AtomicTimingHistogram, CapabilityActionDisposition, CapabilityActionIdentity,
    CaptureColorSpace, CaptureDynamicRange, CaptureTransferFunction, Duration, Instant,
    MacosCapabilityOwner, MacosCaptureCallbackDiagnostics, MacosCapturePixelFormat,
    MacosFrameDropReason, MacosHostArchitecture, MacosScreenRuntimeTelemetry,
    MacosScreenTahoeSelectionStatus, MacosScreenTahoeStatus, MacosSourceTimingStatus,
    NativeCaptureCapabilities, NativeProtectedSourceState, NativeTahoeSelectionCapabilities,
    Ordering, PUBLICATION_PATH_NATIVE, PUBLICATION_PATH_NATIVE_UNAVAILABLE,
    PUBLICATION_PATH_UNKNOWN, PlatformGpuSurfaceTimingSink, ScreenNativeExecutionUnavailableReason,
    ScreenPublicationExecutorFallbackReason, ScreenRendererExecutionState, SourceIssue,
    TIMING_BUCKET_COUNT, TIMING_BUCKET_WIDTH_NS, lock,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::{PUBLICATION_PATH_CPU, PUBLICATION_PATH_CPU_FALLBACK};

impl AtomicTimingHistogram {
    pub(super) fn record(&self, elapsed: Duration) {
        self.record_with_hook(elapsed, || {});
    }

    pub(super) fn record_with_hook(&self, elapsed: Duration, before_complete: impl FnOnce()) {
        let generation = self.begin_write();
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let bucket = usize::try_from(nanos / TIMING_BUCKET_WIDTH_NS)
            .unwrap_or(usize::MAX)
            .min(TIMING_BUCKET_COUNT);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        let _ = self
            .total_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(total.saturating_add(nanos))
            });
        self.max_ns.fetch_max(nanos, Ordering::Relaxed);
        before_complete();
        self.sample_count.fetch_add(1, Ordering::Relaxed);
        self.generation
            .store(generation.wrapping_add(1), Ordering::Release);
    }

    pub(super) fn begin_write(&self) -> u64 {
        let mut generation = self.generation.load(Ordering::Relaxed);
        loop {
            if generation & 1 == 1 {
                std::hint::spin_loop();
                generation = self.generation.load(Ordering::Acquire);
                continue;
            }
            let started = generation.wrapping_add(1);
            match self.generation.compare_exchange_weak(
                generation,
                started,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return started,
                Err(observed) => generation = observed,
            }
        }
    }

    pub(super) fn percentile_upper_bound_ns(
        &self,
        percentile: u64,
        sample_count: u64,
        maximum: u64,
    ) -> u64 {
        if sample_count == 0 {
            return 0;
        }
        let rank = sample_count.saturating_mul(percentile).saturating_add(99) / 100;
        let mut observed = 0_u64;
        for (index, count) in self.buckets.iter().enumerate() {
            observed = observed.saturating_add(count.load(Ordering::Relaxed));
            if observed >= rank {
                if index == TIMING_BUCKET_COUNT {
                    return maximum;
                }
                return u64::try_from(index.saturating_add(1))
                    .unwrap_or(u64::MAX)
                    .saturating_mul(TIMING_BUCKET_WIDTH_NS)
                    .min(maximum);
            }
        }
        maximum
    }

    pub(super) fn snapshot(&self) -> MacosSourceTimingStatus {
        self.snapshot_with_hooks(|| {}, || {})
    }

    pub(super) fn snapshot_with_hooks(
        &self,
        mut retrying: impl FnMut(),
        mut after_p95: impl FnMut(),
    ) -> MacosSourceTimingStatus {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            if generation & 1 == 1 {
                retrying();
                std::hint::spin_loop();
                continue;
            }
            let sample_count = self.sample_count.load(Ordering::Relaxed);
            let total_ns = self.total_ns.load(Ordering::Relaxed);
            let max_ns = self.max_ns.load(Ordering::Relaxed);
            let p95_ns = self.percentile_upper_bound_ns(95, sample_count, max_ns);
            after_p95();
            let p99_ns = self.percentile_upper_bound_ns(99, sample_count, max_ns);
            std::sync::atomic::fence(Ordering::Acquire);
            if self.generation.load(Ordering::Relaxed) == generation {
                return MacosSourceTimingStatus {
                    sample_count,
                    total_ns,
                    max_ns,
                    p95_ns,
                    p99_ns,
                };
            }
            retrying();
        }
    }
}

impl PlatformGpuSurfaceTimingSink for MacosScreenRuntimeTelemetry {
    fn record_import(&self, elapsed: Duration) {
        self.native_import_timing.record(elapsed);
    }

    fn record_native_reduction_submission(&self, elapsed: Duration) {
        self.native_reduction_submit_timing.record(elapsed);
    }
}

impl MacosScreenRuntimeTelemetry {
    pub(super) fn renderer_authoritative() -> Self {
        Self {
            renderer_authoritative: true,
            ..Self::default()
        }
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(super) fn set_cpu(&self) {
        if self.renderer_authoritative {
            return;
        }
        self.publication_path
            .store(PUBLICATION_PATH_CPU, Ordering::Release);
        *lock(&self.fallback_reason) = None;
    }

    pub(super) fn set_native(
        &self,
        required: bool,
        target_id: super::ScreenNativeExecutionTargetId,
    ) {
        if self.renderer_authoritative && !required {
            return;
        }
        let renderer_target = self
            .renderer_authoritative
            .then(|| lock(&self.renderer_target));
        if renderer_target
            .as_deref()
            .is_some_and(|current| *current != Some(target_id))
        {
            return;
        }
        self.publication_path
            .store(PUBLICATION_PATH_NATIVE, Ordering::Release);
        *lock(&self.fallback_reason) = None;
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(super) fn set_cpu_fallback(&self, reason: &'static str) {
        if self.renderer_authoritative {
            return;
        }
        self.publication_path
            .store(PUBLICATION_PATH_CPU_FALLBACK, Ordering::Release);
        *lock(&self.fallback_reason) = Some(Arc::from(reason));
    }

    pub(super) fn set_native_unavailable(
        &self,
        reason: ScreenPublicationExecutorFallbackReason,
        target_id: super::ScreenNativeExecutionTargetId,
    ) {
        if self.renderer_authoritative {
            let renderer_target = lock(&self.renderer_target);
            if *renderer_target != Some(target_id) {
                return;
            }
            self.publication_path
                .store(PUBLICATION_PATH_NATIVE_UNAVAILABLE, Ordering::Release);
            *lock(&self.fallback_reason) = Some(Arc::from(native_unavailable_reason(reason)));
            return;
        }
        self.publication_path
            .store(PUBLICATION_PATH_NATIVE_UNAVAILABLE, Ordering::Release);
        *lock(&self.fallback_reason) = Some(Arc::from(native_unavailable_reason(reason)));
    }

    pub(super) fn set_renderer_execution_state(&self, state: ScreenRendererExecutionState) {
        if !self.renderer_authoritative {
            return;
        }
        let mut renderer_target = lock(&self.renderer_target);
        let (path, reason) = match state {
            ScreenRendererExecutionState::Inactive => (PUBLICATION_PATH_UNKNOWN, None),
            ScreenRendererExecutionState::NativeReady(target_id) => {
                *renderer_target = Some(target_id);
                (PUBLICATION_PATH_NATIVE, None)
            }
            ScreenRendererExecutionState::NativeUnavailable(reason) => (
                PUBLICATION_PATH_NATIVE_UNAVAILABLE,
                Some(Arc::from(native_execution_unavailable_reason(reason))),
            ),
        };
        if !matches!(state, ScreenRendererExecutionState::NativeReady(_)) {
            *renderer_target = None;
        }
        self.publication_path.store(path, Ordering::Release);
        *lock(&self.fallback_reason) = reason;
    }

    pub(super) fn publication_path(&self) -> Option<Arc<str>> {
        match self.publication_path.load(Ordering::Acquire) {
            #[cfg(feature = "macos-capture-fixtures")]
            PUBLICATION_PATH_CPU => Some(Arc::from("cpu")),
            PUBLICATION_PATH_NATIVE => Some(Arc::from("native")),
            #[cfg(feature = "macos-capture-fixtures")]
            PUBLICATION_PATH_CPU_FALLBACK => Some(Arc::from("cpu_fallback")),
            PUBLICATION_PATH_NATIVE_UNAVAILABLE => Some(Arc::from("native_unavailable")),
            PUBLICATION_PATH_UNKNOWN => None,
            _ => None,
        }
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(super) fn record_cpu_reduction(&self, elapsed: Duration) {
        self.cpu_reduction_timing.record(elapsed);
    }

    pub(super) fn record_native_publication(&self, captured_at: Instant) {
        self.capture_to_native_publication_timing
            .record(Instant::now().saturating_duration_since(captured_at));
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(super) fn record_converted_publication(&self, captured_at: Instant) {
        self.capture_to_converted_publication_timing
            .record(Instant::now().saturating_duration_since(captured_at));
    }
}

const fn native_unavailable_reason(
    reason: ScreenPublicationExecutorFallbackReason,
) -> &'static str {
    match reason {
        ScreenPublicationExecutorFallbackReason::CpuSource => "cpu_source",
        ScreenPublicationExecutorFallbackReason::PlatformApiMismatch => "platform_api_mismatch",
        ScreenPublicationExecutorFallbackReason::MissingPhysicalGpuDevice => {
            "missing_physical_gpu_device"
        }
        ScreenPublicationExecutorFallbackReason::PhysicalGpuDeviceMismatch => {
            "physical_gpu_device_mismatch"
        }
        ScreenPublicationExecutorFallbackReason::TargetDimensionLimitExceeded => {
            "target_dimension_limit_exceeded"
        }
        ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported => {
            "native_color_contract_unsupported"
        }
    }
}

const fn native_execution_unavailable_reason(
    reason: ScreenNativeExecutionUnavailableReason,
) -> &'static str {
    match reason {
        ScreenNativeExecutionUnavailableReason::MissingTarget => "missing_target",
        ScreenNativeExecutionUnavailableReason::Executor(reason) => {
            native_unavailable_reason(reason)
        }
    }
}

pub(super) fn protected_screen_action_issue(
    state: NativeProtectedSourceState,
) -> Option<SourceIssue> {
    match state {
        NativeProtectedSourceState::NeedsUserAction => Some(
            SourceIssue::new(
                "authorization_required",
                "Screen Recording authorization is required",
                true,
            )
            .with_remediation("Authorize Screen Recording"),
        ),
        NativeProtectedSourceState::PermissionDenied => Some(
            SourceIssue::new(
                "authorization_denied",
                "Screen Recording authorization was denied",
                true,
            )
            .with_remediation("Authorize Screen Recording"),
        ),
        NativeProtectedSourceState::Revoked => Some(
            SourceIssue::new(
                "authorization_revoked",
                "Screen Recording authorization was revoked",
                true,
            )
            .with_remediation("Authorize Screen Recording"),
        ),
        NativeProtectedSourceState::NeedsProcessRestart => Some(
            SourceIssue::new(
                "process_restart_required",
                "Screen Recording authorization requires a process restart",
                true,
            )
            .with_remediation("Restart the active Hypercolor process"),
        ),
        _ => None,
    }
}

pub(super) fn protected_action_identity(
    owner: MacosCapabilityOwner,
    presentation_required: bool,
) -> CapabilityActionIdentity {
    let requires_ui = matches!(
        owner,
        MacosCapabilityOwner::App | MacosCapabilityOwner::Broker
    ) || presentation_required
        && matches!(
            owner,
            MacosCapabilityOwner::LaunchdService | MacosCapabilityOwner::HomebrewService
        );
    CapabilityActionIdentity::new(
        owner.as_str(),
        if requires_ui {
            CapabilityActionDisposition::RequiresUi
        } else {
            CapabilityActionDisposition::Local
        },
    )
}

pub(super) fn map_tahoe_selection_capabilities(
    capabilities: NativeTahoeSelectionCapabilities,
) -> MacosScreenTahoeSelectionStatus {
    MacosScreenTahoeSelectionStatus {
        source_id: capabilities.source_id,
        capture_session_generation: capabilities.capture_session_generation,
        hdr_capture: capabilities.hdr_capture,
        dual_range_screenshots: capabilities.dual_range_screenshots,
    }
}

pub(super) fn map_tahoe_capabilities(
    capabilities: NativeCaptureCapabilities,
    metal4: bool,
) -> MacosScreenTahoeStatus {
    MacosScreenTahoeStatus {
        host_architecture: capabilities.host_architecture,
        translated_process: capabilities.translated_process,
        content_tone_mapping_info: capabilities.tahoe.content_tone_mapping_info.is_present(),
        metal4,
    }
}

pub(super) const fn executable_architecture() -> MacosHostArchitecture {
    #[cfg(target_arch = "aarch64")]
    {
        MacosHostArchitecture::AppleSilicon
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        MacosHostArchitecture::Intel
    }
}

pub(super) const fn nonzero_telemetry(value: u64) -> Option<u64> {
    if value == 0 { None } else { Some(value) }
}

pub(super) const fn timing_status(
    sample_count: u64,
    total_ns: u64,
    max_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
) -> MacosSourceTimingStatus {
    MacosSourceTimingStatus {
        sample_count,
        total_ns,
        max_ns,
        p95_ns,
        p99_ns,
    }
}

pub(super) const fn pixel_format_name(format: MacosCapturePixelFormat) -> &'static str {
    match format {
        MacosCapturePixelFormat::Bgra8 => "bgra8",
        MacosCapturePixelFormat::Argb2101010 => "argb2101010",
        MacosCapturePixelFormat::Rgba16Float => "rgba16_float",
        MacosCapturePixelFormat::Yuv420VideoRange => "yuv420_video_range",
        MacosCapturePixelFormat::Yuv420FullRange => "yuv420_full_range",
        MacosCapturePixelFormat::Yuv44410BiPlanar => "yuv44410_biplanar",
    }
}

pub(super) const fn dynamic_range_name(range: CaptureDynamicRange) -> &'static str {
    match range {
        CaptureDynamicRange::Standard => "standard",
        CaptureDynamicRange::High => "high",
    }
}

pub(super) const fn color_space_name(space: CaptureColorSpace) -> &'static str {
    match space {
        CaptureColorSpace::Srgb => "srgb",
        CaptureColorSpace::DisplayP3 => "display_p3",
        CaptureColorSpace::Rec2020 => "rec2020",
        CaptureColorSpace::Unknown => "unknown",
    }
}

pub(super) const fn transfer_function_name(function: CaptureTransferFunction) -> &'static str {
    match function {
        CaptureTransferFunction::Srgb => "srgb",
        CaptureTransferFunction::Linear => "linear",
        CaptureTransferFunction::Rec709 => "rec709",
        CaptureTransferFunction::Rec2020 => "rec2020",
        CaptureTransferFunction::Pq => "pq",
        CaptureTransferFunction::Hlg => "hlg",
        CaptureTransferFunction::Unknown => "unknown",
    }
}

pub(super) fn frame_drop_counters(
    diagnostics: &MacosCaptureCallbackDiagnostics,
) -> Arc<[(Arc<str>, u64)]> {
    MacosFrameDropReason::ALL
        .into_iter()
        .map(|reason| {
            let name = match reason {
                MacosFrameDropReason::InvalidSample => "invalid_sample",
                MacosFrameDropReason::DataNotReady => "data_not_ready",
                MacosFrameDropReason::UnexpectedOutput => "unexpected_output",
                MacosFrameDropReason::Attachment => "attachment",
                MacosFrameDropReason::UnsupportedFormat => "unsupported_format",
                MacosFrameDropReason::ColorMetadata => "color_metadata",
                MacosFrameDropReason::Surface => "surface",
                MacosFrameDropReason::Validation => "validation",
                MacosFrameDropReason::Resource => "resource",
            };
            (Arc::from(name), diagnostics.dropped(reason))
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(all(test, not(feature = "macos-capture-fixtures")))]
mod production_tests {
    use std::num::NonZeroU64;

    use super::*;
    use crate::input::screen::ScreenNativeExecutionTargetId;

    #[test]
    fn production_publication_telemetry_has_only_native_states() {
        let telemetry = MacosScreenRuntimeTelemetry::renderer_authoritative();
        assert_eq!(telemetry.publication_path(), None);

        let target_id = ScreenNativeExecutionTargetId::new(NonZeroU64::MIN);
        telemetry
            .set_renderer_execution_state(ScreenRendererExecutionState::NativeReady(target_id));
        assert_eq!(telemetry.publication_path().as_deref(), Some("native"));

        telemetry.set_renderer_execution_state(ScreenRendererExecutionState::NativeUnavailable(
            ScreenNativeExecutionUnavailableReason::MissingTarget,
        ));
        assert_eq!(
            telemetry.publication_path().as_deref(),
            Some("native_unavailable")
        );
    }
}
