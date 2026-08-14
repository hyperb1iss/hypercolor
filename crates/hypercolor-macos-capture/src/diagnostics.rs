#[cfg(any(target_os = "macos", test))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(any(target_os = "macos", test))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", test))]
use crate::MacosCaptureError;

#[cfg(any(target_os = "macos", test))]
const TIMING_BUCKET_WIDTH_NS: u64 = 100_000;
#[cfg(any(target_os = "macos", test))]
const TIMING_BUCKET_COUNT: usize = 4096;

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

    #[cfg(any(target_os = "macos", test))]
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
    pub callback_sample_count: u64,
    pub callback_total_ns: u64,
    pub callback_max_ns: u64,
    pub callback_p95_ns: u64,
    pub callback_p99_ns: u64,
    pub retain_sample_count: u64,
    pub retain_total_ns: u64,
    pub retain_max_ns: u64,
    pub retain_p95_ns: u64,
    pub retain_p99_ns: u64,
    pub enqueue_sample_count: u64,
    pub enqueue_total_ns: u64,
    pub enqueue_max_ns: u64,
    pub enqueue_p95_ns: u64,
    pub enqueue_p99_ns: u64,
    pub conversion_sample_count: u64,
    pub conversion_total_ns: u64,
    pub conversion_max_ns: u64,
    pub conversion_p95_ns: u64,
    pub conversion_p99_ns: u64,
    pub publication_sample_count: u64,
    pub publication_total_ns: u64,
    pub publication_max_ns: u64,
    pub publication_p95_ns: u64,
    pub publication_p99_ns: u64,
    dropped: [u64; MacosFrameDropReason::ALL.len()],
}

impl MacosCaptureCallbackDiagnostics {
    pub const fn dropped(&self, reason: MacosFrameDropReason) -> u64 {
        self.dropped[reason as usize]
    }

    pub fn total_dropped(&self) -> u64 {
        self.dropped.iter().sum()
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Default)]
pub(crate) struct CallbackCounters {
    frames_received: AtomicU64,
    frames_published: AtomicU64,
    lifecycle_events: AtomicU64,
    native_samples_superseded: AtomicU64,
    malformed_frames: AtomicU64,
    callback_timing: TimingCounters,
    retain_timing: TimingCounters,
    enqueue_timing: TimingCounters,
    conversion_timing: TimingCounters,
    publication_timing: TimingCounters,
    dropped: [AtomicU64; MacosFrameDropReason::ALL.len()],
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct TimingCounters {
    buckets: Box<[AtomicU64]>,
    generation: AtomicU64,
    sample_count: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}

#[cfg(any(target_os = "macos", test))]
impl Default for TimingCounters {
    fn default() -> Self {
        Self {
            buckets: (0..=TIMING_BUCKET_COUNT)
                .map(|_| AtomicU64::new(0))
                .collect(),
            generation: AtomicU64::new(0),
            sample_count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            max_ns: AtomicU64::new(0),
        }
    }
}

#[cfg(any(target_os = "macos", test))]
impl TimingCounters {
    fn record(&self, elapsed: Duration) {
        self.record_with_hook(elapsed, || {});
    }

    fn record_with_hook(&self, elapsed: Duration, before_complete: impl FnOnce()) {
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

    fn begin_write(&self) -> u64 {
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

    fn percentile_upper_bound_ns(&self, percentile: u64, sample_count: u64, maximum: u64) -> u64 {
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

    fn snapshot(&self) -> (u64, u64, u64, u64, u64) {
        self.snapshot_with_hooks(|| {}, || {})
    }

    fn snapshot_with_hooks(
        &self,
        mut retrying: impl FnMut(),
        mut after_p95: impl FnMut(),
    ) -> (u64, u64, u64, u64, u64) {
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
                return (sample_count, total_ns, max_ns, p95_ns, p99_ns);
            }
            retrying();
        }
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) struct TimingObservation<'a> {
    counters: &'a TimingCounters,
    started: Instant,
}

#[cfg(any(target_os = "macos", test))]
impl Drop for TimingObservation<'_> {
    fn drop(&mut self) {
        self.counters.record(self.started.elapsed());
    }
}

#[cfg(any(target_os = "macos", test))]
impl CallbackCounters {
    #[cfg(target_os = "macos")]
    pub(crate) fn observe_callback(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.callback_timing,
            started: Instant::now(),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn observe_retain(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.retain_timing,
            started: Instant::now(),
        }
    }

    pub(crate) fn observe_enqueue(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.enqueue_timing,
            started: Instant::now(),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn observe_conversion(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.conversion_timing,
            started: Instant::now(),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn observe_publication(&self) -> TimingObservation<'_> {
        TimingObservation {
            counters: &self.publication_timing,
            started: Instant::now(),
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn record_received(&self) {
        self.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn record_published(&self) {
        self.frames_published.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn record_lifecycle(&self) {
        self.lifecycle_events.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(target_os = "macos")]
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
        let (
            callback_sample_count,
            callback_total_ns,
            callback_max_ns,
            callback_p95_ns,
            callback_p99_ns,
        ) = self.callback_timing.snapshot();
        let (retain_sample_count, retain_total_ns, retain_max_ns, retain_p95_ns, retain_p99_ns) =
            self.retain_timing.snapshot();
        let (
            enqueue_sample_count,
            enqueue_total_ns,
            enqueue_max_ns,
            enqueue_p95_ns,
            enqueue_p99_ns,
        ) = self.enqueue_timing.snapshot();
        let (
            conversion_sample_count,
            conversion_total_ns,
            conversion_max_ns,
            conversion_p95_ns,
            conversion_p99_ns,
        ) = self.conversion_timing.snapshot();
        let (
            publication_sample_count,
            publication_total_ns,
            publication_max_ns,
            publication_p95_ns,
            publication_p99_ns,
        ) = self.publication_timing.snapshot();
        MacosCaptureCallbackDiagnostics {
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_published: self.frames_published.load(Ordering::Relaxed),
            lifecycle_events: self.lifecycle_events.load(Ordering::Relaxed),
            superseded_deliveries: superseded_deliveries
                .saturating_add(self.native_samples_superseded.load(Ordering::Relaxed)),
            malformed_frames: self.malformed_frames.load(Ordering::Relaxed),
            callback_sample_count,
            callback_total_ns,
            callback_max_ns,
            callback_p95_ns,
            callback_p99_ns,
            retain_sample_count,
            retain_total_ns,
            retain_max_ns,
            retain_p95_ns,
            retain_p99_ns,
            enqueue_sample_count,
            enqueue_total_ns,
            enqueue_max_ns,
            enqueue_p95_ns,
            enqueue_p99_ns,
            conversion_sample_count,
            conversion_total_ns,
            conversion_max_ns,
            conversion_p95_ns,
            conversion_p99_ns,
            publication_sample_count,
            publication_total_ns,
            publication_max_ns,
            publication_p95_ns,
            publication_p99_ns,
            dropped: std::array::from_fn(|index| self.dropped[index].load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};
    use std::thread;
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

        assert_eq!(timing.snapshot(), (2, 110, 70, 70, 70));
    }

    #[test]
    fn timing_percentiles_are_bounded_by_the_exact_maximum() {
        let timing = super::TimingCounters::default();
        timing.record(Duration::from_nanos(1));

        assert_eq!(timing.snapshot(), (1, 1, 1, 1, 1));
    }

    #[test]
    fn timing_snapshot_retries_when_population_changes_between_percentiles() {
        let timing = super::TimingCounters::default();
        timing.record(Duration::from_nanos(40));
        let mut injected = false;

        let snapshot = timing.snapshot_with_hooks(
            || {},
            || {
                if !injected {
                    timing.record(Duration::from_nanos(70));
                    injected = true;
                }
            },
        );

        assert_eq!(snapshot, (2, 110, 70, 70, 70));
    }

    #[test]
    fn timing_snapshot_waits_for_an_in_progress_observation() {
        let timing = Arc::new(super::TimingCounters::default());
        let (writer_started_tx, writer_started_rx) = mpsc::channel();
        let (release_writer_tx, release_writer_rx) = mpsc::channel();
        let writer_timing = Arc::clone(&timing);
        let writer = thread::spawn(move || {
            writer_timing.record_with_hook(Duration::from_nanos(70), || {
                writer_started_tx
                    .send(())
                    .expect("writer-start signal is received");
                release_writer_rx
                    .recv()
                    .expect("writer release signal is sent");
            });
        });
        writer_started_rx
            .recv()
            .expect("writer reaches its incomplete population");

        let (retry_tx, retry_rx) = mpsc::channel();
        let (snapshot_tx, snapshot_rx) = mpsc::channel();
        let snapshot_timing = Arc::clone(&timing);
        let snapshot = thread::spawn(move || {
            let mut signaled = false;
            let value = snapshot_timing.snapshot_with_hooks(
                || {
                    if !signaled {
                        retry_tx.send(()).expect("retry signal is received");
                        signaled = true;
                    }
                },
                || {},
            );
            snapshot_tx
                .send(value)
                .expect("snapshot result is received");
        });
        retry_rx
            .recv()
            .expect("snapshot observes the in-progress population");
        assert!(matches!(
            snapshot_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        release_writer_tx
            .send(())
            .expect("writer is released after the retry");
        writer.join().expect("writer thread completes");
        snapshot.join().expect("snapshot thread completes");
        assert_eq!(
            snapshot_rx.recv().expect("coherent snapshot is published"),
            (1, 70, 70, 70, 70)
        );
    }

    #[test]
    fn enqueue_observation_is_reported_separately_from_callback_work() {
        let counters = CallbackCounters::default();
        {
            let _observation = counters.observe_enqueue();
        }

        let diagnostics = counters.snapshot(0);
        assert_eq!(diagnostics.enqueue_sample_count, 1);
        assert_eq!(diagnostics.callback_sample_count, 0);
        assert!(diagnostics.enqueue_p99_ns <= diagnostics.enqueue_max_ns);
    }
}
