use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::MacosCaptureError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum MacosFrameDropReason {
    InvalidSample = 0,
    DataNotReady = 1,
    UnexpectedOutput = 2,
    Attachment = 3,
    UnsupportedFormat = 4,
    ColorMetadata = 5,
    Surface = 6,
    Validation = 7,
    Resource = 8,
}

impl MacosFrameDropReason {
    pub const ALL: [Self; 9] = [
        Self::InvalidSample,
        Self::DataNotReady,
        Self::UnexpectedOutput,
        Self::Attachment,
        Self::UnsupportedFormat,
        Self::ColorMetadata,
        Self::Surface,
        Self::Validation,
        Self::Resource,
    ];

    pub(crate) const fn from_error(error: &MacosCaptureError) -> Self {
        match error {
            MacosCaptureError::InvalidSampleBuffer => Self::InvalidSample,
            MacosCaptureError::SampleDataNotReady => Self::DataNotReady,
            MacosCaptureError::UnexpectedStreamOutputType(_) => Self::UnexpectedOutput,
            MacosCaptureError::MissingFrameAttachments
            | MacosCaptureError::MissingAttachment(_)
            | MacosCaptureError::MalformedAttachment(_)
            | MacosCaptureError::UnknownFrameStatus(_) => Self::Attachment,
            MacosCaptureError::UnsupportedPixelFormat(_)
            | MacosCaptureError::UnsupportedConfiguredDynamicRange(_) => Self::UnsupportedFormat,
            MacosCaptureError::ColorMetadataMismatch
            | MacosCaptureError::MissingYuvColorMetadata
            | MacosCaptureError::MissingColorAttachment(_)
            | MacosCaptureError::UnsupportedColorAttachment(_)
            | MacosCaptureError::MalformedLuminanceAttachment(_) => Self::ColorMetadata,
            MacosCaptureError::MissingFramePayload
            | MacosCaptureError::InvalidSurface
            | MacosCaptureError::MissingIoSurface
            | MacosCaptureError::NativeSurfaceUnavailable => Self::Surface,
            MacosCaptureError::InvalidCadence(_)
            | MacosCaptureError::NotMainThread
            | MacosCaptureError::ScreenCapturePermissionRequired
            | MacosCaptureError::InvalidSourceSelector(_)
            | MacosCaptureError::NativeOperation { .. }
            | MacosCaptureError::RetainNativeFilterFailed
            | MacosCaptureError::CaptureWorkerStartFailed(_)
            | MacosCaptureError::CaptureWorkerPanicked
            | MacosCaptureError::StreamStopCompletionLost
            | MacosCaptureError::DisplayUuidUnavailable(_)
            | MacosCaptureError::DisplaySourceUnavailable(_)
            | MacosCaptureError::MissingShareableContent
            | MacosCaptureError::PlaneCount { .. }
            | MacosCaptureError::InvalidPlaneIndex { .. }
            | MacosCaptureError::InvalidPlaneExtent { .. }
            | MacosCaptureError::StrideTooSmall { .. }
            | MacosCaptureError::PlaneLengthTooSmall { .. }
            | MacosCaptureError::ArithmeticOverflow
            | MacosCaptureError::AllocationTooSmall { .. }
            | MacosCaptureError::GeometryOutsideStorage(_)
            | MacosCaptureError::CpuMappingUnavailable
            | MacosCaptureError::CpuPlaneLayoutMismatch
            | MacosCaptureError::PixelBufferLockFailed(_)
            | MacosCaptureError::PixelBufferUnlockFailed(_)
            | MacosCaptureError::FixturePlaneCount { .. }
            | MacosCaptureError::FixturePlaneLength { .. }
            | MacosCaptureError::PixelBufferFixtureCreateFailed(_)
            | MacosCaptureError::MissingCpuPlaneAddress(_)
            | MacosCaptureError::UnsupportedCpuPixelFormat(_)
            | MacosCaptureError::CpuPixelOutsideStorage { .. }
            | MacosCaptureError::InvalidCpuDestinationStride { .. }
            | MacosCaptureError::CpuDestinationTooSmall { .. }
            | MacosCaptureError::SequenceExhausted
            | MacosCaptureError::StreamDeliveryRejected(_)
            | MacosCaptureError::FrameDeliveryDropped(_)
            | MacosCaptureError::CapabilityProbeFailed(_)
            | MacosCaptureError::TahoePlatformDefect(_)
            | MacosCaptureError::ScreenshotCapabilityPending
            | MacosCaptureError::ScreenshotSelectionChanged
            | MacosCaptureError::MissingScreenshotImage(_)
            | MacosCaptureError::ScreenshotMetadataOutOfRange(_)
            | MacosCaptureError::MissingScreenshotColorSpace
            | MacosCaptureError::ScreenshotReferenceTooLarge { .. }
            | MacosCaptureError::ScreenshotReferenceContextFailed
            | MacosCaptureError::ScreenshotToneMappingOptionsFailed
            | MacosCaptureError::ScreenshotOutputUrlFailed
            | MacosCaptureError::ScreenshotEncoderCreateFailed
            | MacosCaptureError::ScreenshotEncodeFailed
            | MacosCaptureError::Geometry(_) => Self::Validation,
            MacosCaptureError::ScreenResourceExhausted { .. } => Self::Resource,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosCaptureCallbackDiagnostics {
    pub frames_received: u64,
    pub frames_published: u64,
    pub lifecycle_events: u64,
    pub superseded_deliveries: u64,
    pub malformed_frames: u64,
    pub callback_total_ns: u64,
    pub callback_max_ns: u64,
    pub retain_total_ns: u64,
    pub retain_max_ns: u64,
    pub conversion_total_ns: u64,
    pub conversion_max_ns: u64,
    pub publication_total_ns: u64,
    pub publication_max_ns: u64,
    dropped: [u64; MacosFrameDropReason::ALL.len()],
}

impl MacosCaptureCallbackDiagnostics {
    pub const fn dropped(self, reason: MacosFrameDropReason) -> u64 {
        self.dropped[reason as usize]
    }

    pub fn total_dropped(self) -> u64 {
        self.dropped.into_iter().sum()
    }
}

#[derive(Debug, Default)]
pub(crate) struct CallbackCounters {
    frames_received: AtomicU64,
    frames_published: AtomicU64,
    lifecycle_events: AtomicU64,
    native_samples_superseded: AtomicU64,
    malformed_frames: AtomicU64,
    callback_timing: TimingCounters,
    retain_timing: TimingCounters,
    conversion_timing: TimingCounters,
    publication_timing: TimingCounters,
    dropped: [AtomicU64; MacosFrameDropReason::ALL.len()],
}

#[derive(Debug, Default)]
struct TimingCounters {
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}

impl TimingCounters {
    fn record(&self, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let _ = self
            .total_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(total.saturating_add(nanos))
            });
        self.max_ns.fetch_max(nanos, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, u64) {
        (
            self.total_ns.load(Ordering::Relaxed),
            self.max_ns.load(Ordering::Relaxed),
        )
    }
}

pub(crate) struct TimingObservation<'a> {
    counters: &'a TimingCounters,
    started: Instant,
}

impl Drop for TimingObservation<'_> {
    fn drop(&mut self) {
        self.counters.record(self.started.elapsed());
    }
}

impl CallbackCounters {
    pub(crate) fn observe_callback(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.callback_timing,
            started: Instant::now(),
        }
    }

    pub(crate) fn observe_retain(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.retain_timing,
            started: Instant::now(),
        }
    }

    pub(crate) fn observe_conversion(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.conversion_timing,
            started: Instant::now(),
        }
    }

    pub(crate) fn observe_publication(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.publication_timing,
            started: Instant::now(),
        }
    }

    pub(crate) fn record_received(&self) {
        self.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_published(&self) {
        self.frames_published.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_lifecycle(&self) {
        self.lifecycle_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_native_sample_superseded(&self) {
        self.native_samples_superseded
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_drop(&self, error: &MacosCaptureError) {
        if matches!(
            error,
            MacosCaptureError::MalformedAttachment(_)
                | MacosCaptureError::MalformedLuminanceAttachment(_)
        ) {
            self.malformed_frames.fetch_add(1, Ordering::Relaxed);
        }
        self.dropped[MacosFrameDropReason::from_error(error) as usize]
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self, superseded_deliveries: u64) -> MacosCaptureCallbackDiagnostics {
        let (callback_total_ns, callback_max_ns) = self.callback_timing.snapshot();
        let (retain_total_ns, retain_max_ns) = self.retain_timing.snapshot();
        let (conversion_total_ns, conversion_max_ns) = self.conversion_timing.snapshot();
        let (publication_total_ns, publication_max_ns) = self.publication_timing.snapshot();
        MacosCaptureCallbackDiagnostics {
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_published: self.frames_published.load(Ordering::Relaxed),
            lifecycle_events: self.lifecycle_events.load(Ordering::Relaxed),
            superseded_deliveries: superseded_deliveries
                .saturating_add(self.native_samples_superseded.load(Ordering::Relaxed)),
            malformed_frames: self.malformed_frames.load(Ordering::Relaxed),
            callback_total_ns,
            callback_max_ns,
            retain_total_ns,
            retain_max_ns,
            conversion_total_ns,
            conversion_max_ns,
            publication_total_ns,
            publication_max_ns,
            dropped: std::array::from_fn(|index| self.dropped[index].load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::MacosStreamDeliveryRejection;

    use super::{CallbackCounters, MacosCaptureError, MacosFrameDropReason};

    #[test]
    fn resource_exhaustion_has_a_distinct_drop_reason() {
        assert_eq!(
            MacosFrameDropReason::from_error(&MacosCaptureError::ScreenResourceExhausted {
                requested_bytes: 64,
                available_bytes: 32,
            }),
            MacosFrameDropReason::Resource
        );
    }

    #[test]
    fn dropped_delivery_metadata_increments_the_validation_counter() {
        let counters = CallbackCounters::default();
        counters.record_drop(&MacosCaptureError::FrameDeliveryDropped(
            MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("colorimetry"),
        ));

        let diagnostics = counters.snapshot(0);
        assert_eq!(diagnostics.total_dropped(), 1);
        assert_eq!(diagnostics.dropped(MacosFrameDropReason::Validation), 1);
    }

    #[test]
    fn malformed_frames_remain_distinct_from_the_bounded_drop_reason() {
        let counters = CallbackCounters::default();
        counters.record_drop(&MacosCaptureError::MalformedAttachment("status"));

        let diagnostics = counters.snapshot(0);
        assert_eq!(diagnostics.malformed_frames, 1);
        assert_eq!(diagnostics.dropped(MacosFrameDropReason::Attachment), 1);
    }

    #[test]
    fn timing_counters_saturate_totals_and_retain_the_maximum() {
        let timing = super::TimingCounters::default();
        timing.record(Duration::from_nanos(40));
        timing.record(Duration::from_nanos(70));

        assert_eq!(timing.snapshot(), (110, 70));
    }
}
