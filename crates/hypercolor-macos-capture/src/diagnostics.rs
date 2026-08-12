use std::sync::atomic::{AtomicU64, Ordering};

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
}

impl MacosFrameDropReason {
    pub const ALL: [Self; 8] = [
        Self::InvalidSample,
        Self::DataNotReady,
        Self::UnexpectedOutput,
        Self::Attachment,
        Self::UnsupportedFormat,
        Self::ColorMetadata,
        Self::Surface,
        Self::Validation,
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
            MacosCaptureError::UnsupportedPixelFormat(_) => Self::UnsupportedFormat,
            MacosCaptureError::ColorMetadataMismatch
            | MacosCaptureError::MissingYuvColorMetadata
            | MacosCaptureError::MissingColorAttachment(_)
            | MacosCaptureError::UnsupportedColorAttachment(_) => Self::ColorMetadata,
            MacosCaptureError::MissingFramePayload
            | MacosCaptureError::InvalidSurface
            | MacosCaptureError::MissingIoSurface => Self::Surface,
            MacosCaptureError::InvalidCadence(_)
            | MacosCaptureError::NotMainThread
            | MacosCaptureError::ScreenCapturePermissionRequired
            | MacosCaptureError::InvalidSourceSelector(_)
            | MacosCaptureError::NativeOperation { .. }
            | MacosCaptureError::RetainNativeFilterFailed
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
            | MacosCaptureError::MissingCpuPlaneAddress(_)
            | MacosCaptureError::UnsupportedCpuPixelFormat(_)
            | MacosCaptureError::UnsupportedCpuTransferFunction(_)
            | MacosCaptureError::InvalidCpuDestinationStride { .. }
            | MacosCaptureError::CpuDestinationTooSmall { .. }
            | MacosCaptureError::SequenceExhausted
            | MacosCaptureError::Geometry(_) => Self::Validation,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosCaptureCallbackDiagnostics {
    pub frames_received: u64,
    pub frames_published: u64,
    pub lifecycle_events: u64,
    pub superseded_deliveries: u64,
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
    dropped: [AtomicU64; MacosFrameDropReason::ALL.len()],
}

impl CallbackCounters {
    pub(crate) fn record_received(&self) {
        self.frames_received.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_published(&self) {
        self.frames_published.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_lifecycle(&self) {
        self.lifecycle_events.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_drop(&self, error: &MacosCaptureError) {
        self.dropped[MacosFrameDropReason::from_error(error) as usize]
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self, superseded_deliveries: u64) -> MacosCaptureCallbackDiagnostics {
        MacosCaptureCallbackDiagnostics {
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_published: self.frames_published.load(Ordering::Relaxed),
            lifecycle_events: self.lifecycle_events.load(Ordering::Relaxed),
            superseded_deliveries,
            dropped: std::array::from_fn(|index| self.dropped[index].load(Ordering::Relaxed)),
        }
    }
}
