use std::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained, MainThreadBound};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFGetTypeID, CFNumber, CFRetained, CFString, CFType, CFUUID, CGPoint,
    CGRect, CGSize,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGImage, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRectMakeWithDictionaryRepresentation, CGRequestScreenCaptureAccess,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVBuffer, CVPixelBuffer, CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetDataSize, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetIOSurface, CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount,
    CVPixelBufferGetWidth, CVPixelBufferGetWidthOfPlane, kCVImageBufferChromaLocation_Center,
    kCVImageBufferChromaLocation_Left, kCVImageBufferChromaLocation_TopLeft,
    kCVImageBufferChromaLocationTopFieldKey, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferColorPrimaries_ITU_R_2020, kCVImageBufferColorPrimaries_P3_D65,
    kCVImageBufferColorPrimariesKey, kCVImageBufferContentLightLevelInfoKey,
    kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferTransferFunction_ITU_R_2020,
    kCVImageBufferTransferFunction_ITU_R_2100_HLG, kCVImageBufferTransferFunction_Linear,
    kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ, kCVImageBufferTransferFunction_sRGB,
    kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
    kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_2020,
    kCVImageBufferYCbCrMatrixKey,
};
use objc2_foundation::{NSArray, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSValue};
use objc2_io_surface::{IOSurfaceRef, kIOSurfaceContentHeadroom};
use objc2_screen_capture_kit::{
    SCCaptureDynamicRange, SCCaptureResolutionType, SCContentFilter, SCContentSharingPicker,
    SCContentSharingPickerConfiguration, SCContentSharingPickerMode,
    SCContentSharingPickerObserver, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamConfigurationPreset, SCStreamDelegate, SCStreamErrorCode, SCStreamErrorDomain,
    SCStreamFrameInfoBoundingRect, SCStreamFrameInfoContentRect, SCStreamFrameInfoContentScale,
    SCStreamFrameInfoDirtyRects, SCStreamFrameInfoDisplayTime, SCStreamFrameInfoScaleFactor,
    SCStreamFrameInfoScreenRect, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
    SCWindow,
};

use crate::diagnostics::CallbackCounters;
use crate::stream_contract::MacosTahoeSelectionCapabilityState;
use crate::worker::{
    LatestSampleInput, LatestSampleWorker, SamplePublication, SamplePublishOutcome,
};
use crate::{
    MACOS_STREAM_QUEUE_DEPTH, MacosAttachment, MacosCaptureCallbackDiagnostics,
    MacosCaptureCapabilities, MacosCaptureColorimetry, MacosCaptureContentStyle,
    MacosCaptureDynamicRange, MacosCaptureError, MacosCapturePixelFormat, MacosCaptureSelection,
    MacosCaptureSelector, MacosCaptureSurface, MacosChromaLocation, MacosColorPrimaries,
    MacosColorRange, MacosConfiguredStream, MacosDeliveredFrameMetadata, MacosFrameDecoder,
    MacosFrameEvent, MacosFrameMailbox, MacosFrameStatus, MacosHostArchitecture, MacosPixelExtent,
    MacosPixelRect, MacosPointRect, MacosProtectedSourceState, MacosRawCapturePlane,
    MacosRawCaptureSample, MacosRawCompleteFrame, MacosRawFrameAttachments, MacosRuntimeCapability,
    MacosScale, MacosScreenshotReferenceCapability, MacosScreenshotReferenceCapture,
    MacosScreenshotReferenceImage, MacosScreenshotReferenceSet, MacosStreamDeliveryRejection,
    MacosStreamDeliveryState, MacosStreamDeliveryValidator, MacosStreamPreset, MacosStreamRequest,
    MacosTahoeCapabilities, MacosTahoeRuntimeProbes, MacosTahoeSelectionCapabilities,
    MacosTransferFunction, MacosValidatedStreamDelivery, MacosYuvMatrix,
};

use super::lifecycle::{CompletionFence, CompletionWitness, NativeLifecycle};
use super::transactions::{
    MacosNativeTransactionError, MacosNativeTransactionPhase, MacosStreamDiagnosticTransaction,
    MacosStreamRequestTransaction, TransactionCompleter, TransactionIdentity,
    TransactionSettlement, stream_diagnostic_transaction, stream_request_transaction,
};

#[path = "capabilities.rs"]
mod capabilities;
#[cfg(test)]
#[path = "fixtures.rs"]
mod fixtures;
#[path = "frame_decode.rs"]
mod frame_decode;
#[path = "mailbox.rs"]
mod mailbox;
#[path = "picker.rs"]
mod picker;
#[path = "reference.rs"]
mod reference;

use capabilities::native_capture_capabilities;
#[cfg(test)]
use capabilities::{SysctlI32Value, capture_capabilities_from_probes};
use frame_decode::{FrameAttachments, decode_sample, extent, planes};
#[cfg(test)]
use frame_decode::{chroma_location, classify_delivery_error, pixel_rect_from_cg};
use mailbox::{
    CaptureOutput, CaptureOutputIvars, DecodedSample, RetainedNativeSample, publish_decoded_result,
};
#[cfg(test)]
use mailbox::{
    RetainedNativeDelivery, route_retained_delivery, route_stream_activity, route_stream_lifecycle,
    with_admitted_surface,
};
#[cfg(test)]
use picker::session_selection_source_id;
use picker::{
    MainThreadSession, NativeFilter, NativeSelectionFilter, PickerObserver,
    resolve_display_selector,
};
use reference::{
    NativeScreenshotCaptureBackend, ScreenshotFilterHandle, ScreenshotIdentityFence,
    ScreenshotTransactionSnapshot, execute_screenshot_transaction,
};
#[cfg(test)]
use reference::{ScreenshotCaptureBackend, ScreenshotImageCompletion};

type PoolBackingLifetime = Arc<dyn Send + Sync>;
type PoolObservation =
    Arc<dyn Fn(u32, u64) -> Result<PoolBackingLifetime, MacosCaptureError> + Send + Sync>;
type PoolReservationFactory =
    Arc<dyn Fn(u64, u64) -> Result<PoolObservation, MacosCaptureError> + Send + Sync>;

const MACOS_IOSURFACE_ROW_ALIGNMENT: u64 = 256;
const MACOS_IOSURFACE_ALLOCATION_ALIGNMENT: u64 = 16 * 1024;
const HYPERCOLOR_UI_BUNDLE_IDENTIFIER: &str = "tech.hyperbliss.hypercolor";
const MACOS_NATIVE_SOURCE_TIMEOUT: Duration = Duration::from_secs(5);
const MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(5);
const MACOS_NATIVE_STOP_TIMEOUT: Duration = Duration::from_secs(2);

fn is_hypercolor_ui_bundle_identifier(bundle_identifier: &str) -> bool {
    bundle_identifier == HYPERCOLOR_UI_BUNDLE_IDENTIFIER
}

#[derive(Debug)]
struct SessionShared {
    mailbox: MacosFrameMailbox,
    status: Mutex<MacosProtectedSourceState>,
    selection: Mutex<SessionSelectionState>,
    selector: Mutex<MacosCaptureSelector>,
    tahoe: MacosTahoeCapabilities,
    counters: CallbackCounters,
    capture_active: AtomicBool,
    picker_resolution: Mutex<Option<SourceResolution>>,
    current_epoch: AtomicU64,
    resolution_epoch: AtomicU64,
    restart_diagnostic: Mutex<PostAuthorizationStreamDiagnosticState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PostAuthorizationStreamDiagnosticAttempt {
    attempt_id: u64,
    selection_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PostAuthorizationStreamDiagnosticResolution {
    attempt: PostAuthorizationStreamDiagnosticAttempt,
    resolution_epoch: u64,
    selector: MacosCaptureSelector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GeneralSourceResolution {
    resolution_epoch: u64,
    selector: MacosCaptureSelector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SourceResolution {
    General(GeneralSourceResolution),
    Diagnostic(PostAuthorizationStreamDiagnosticResolution),
}

struct SourceTransaction {
    resolution: SourceResolution,
    completion: TransactionCompleter<()>,
}

impl SourceResolution {
    fn selector(&self) -> &MacosCaptureSelector {
        match self {
            Self::General(resolution) => &resolution.selector,
            Self::Diagnostic(resolution) => &resolution.selector,
        }
    }
}

#[derive(Debug)]
struct PostAuthorizationStreamDiagnostic {
    attempt: PostAuthorizationStreamDiagnosticAttempt,
    authorization_granted: bool,
    resolution_epoch: Option<u64>,
    stream_epoch: Option<u64>,
    completion: TransactionCompleter<MacosProtectedSourceState>,
}

#[derive(Debug, Default)]
struct PostAuthorizationStreamDiagnosticState {
    next_attempt_id: u64,
    active: Option<PostAuthorizationStreamDiagnostic>,
}

#[derive(Debug, Default)]
struct SessionSelectionState {
    selection: MacosCaptureSelection,
    tahoe: MacosTahoeSelectionCapabilityState,
}

struct NativeStream {
    stream: Retained<SCStream>,
    control: NativeStreamControl,
    filter: NativeFilter,
    selection: MacosCaptureSelection,
    source_id: Arc<str>,
    request: MacosStreamRequest,
    reserve_pool: PoolReservationFactory,
    worker: LatestSampleWorker<RetainedNativeSample>,
    start_completion: CompletionFence,
    _output: Retained<CaptureOutput>,
    _output_queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: ScreenCaptureKit owns callback execution across its queues, and all
// Rust access to this owner is serialized through StreamSlot. NativeStream is
// moved between owners but never exposes concurrent mutable Objective-C state.
unsafe impl Send for NativeStream {}

#[derive(Clone)]
struct NativeStreamControl {
    stream: Retained<SCStream>,
    queue: DispatchRetained<DispatchQueue>,
    start_invoked: Arc<AtomicBool>,
}

// SAFETY: Every command carrying the retained SCStream executes on this
// control's private serial queue. Rust never invokes mutable native lifecycle
// operations concurrently through the wrapper.
unsafe impl Send for NativeStreamControl {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamRole {
    Current,
    Candidate,
    Stale,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterruptionRecoveryPhase {
    Interrupted,
    Starting { epoch: u64 },
    Live { epoch: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct InterruptedRestage {
    interrupted_epoch: u64,
    selection_revision: u64,
    restage_epoch: Option<u64>,
}

impl InterruptedRestage {
    const fn interrupted(interrupted_epoch: u64, selection_revision: u64) -> Self {
        Self {
            interrupted_epoch,
            selection_revision,
            restage_epoch: None,
        }
    }

    #[cfg(test)]
    const fn phase(self) -> InterruptionRecoveryPhase {
        match self.restage_epoch {
            Some(epoch) => InterruptionRecoveryPhase::Starting { epoch },
            None => InterruptionRecoveryPhase::Interrupted,
        }
    }

    const fn can_schedule(
        self,
        capture_active: bool,
        active_epoch: u64,
        selection_revision: u64,
    ) -> bool {
        self.restage_epoch.is_none()
            && capture_active
            && active_epoch == 0
            && self.selection_revision == selection_revision
    }

    fn can_begin(self, state: &StreamState, shared: &SessionShared) -> bool {
        self.can_schedule(
            shared.capture_active(),
            shared.current_epoch(),
            state.selection_revision,
        ) && state.current.is_none()
            && state.candidate_epoch.is_none()
            && state.staging_epoch.is_none()
    }

    const fn schedule(mut self, epoch: u64) -> Option<Self> {
        if self.restage_epoch.is_some() || epoch <= self.interrupted_epoch {
            return None;
        }
        self.restage_epoch = Some(epoch);
        Some(self)
    }

    fn matches(self, epoch: u64) -> bool {
        self.restage_epoch == Some(epoch)
    }

    #[cfg(test)]
    fn complete(self, epoch: u64) -> Option<InterruptionRecoveryPhase> {
        self.matches(epoch)
            .then_some(InterruptionRecoveryPhase::Live { epoch })
    }
}

#[derive(Clone)]
struct InterruptedRestagePlan {
    recovery: InterruptedRestage,
    selection_filter: NativeSelectionFilter,
    request: MacosStreamRequest,
    reserve_pool: PoolReservationFactory,
}

#[derive(Clone)]
struct PendingSelectionFilter {
    epoch: u64,
    selection_revision: u64,
    selection_filter: NativeSelectionFilter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CandidateStage {
    epoch: u64,
    selection_revision: u64,
    lifecycle_revision: u64,
    predecessor_epoch: Option<u64>,
    recovery_current_epoch: Option<u64>,
    recovery: Option<InterruptedRestage>,
    request: Option<PendingStreamRequestIdentity>,
}

impl CandidateStage {
    const fn identity(self) -> CandidateStageIdentity {
        CandidateStageIdentity {
            epoch: self.epoch,
            selection_revision: self.selection_revision,
            lifecycle_revision: self.lifecycle_revision,
            predecessor_epoch: self.predecessor_epoch,
        }
    }

    fn is_current(self, state: &StreamState, shared: &SessionShared) -> bool {
        shared.capture_active()
            && state.staging_epoch == Some(self.epoch)
            && state.selection_revision == self.selection_revision
            && state.lifecycle_revision == self.lifecycle_revision
            && state.candidate_epoch.is_none()
            && state.pending_selection.as_ref().is_some_and(|pending| {
                pending.epoch == self.epoch && pending.selection_revision == self.selection_revision
            })
            && self.request.is_none_or(|request| {
                state
                    .pending_request
                    .as_ref()
                    .is_some_and(|pending| pending.identity() == request)
            })
            && self
                .recovery_current_epoch
                .is_none_or(|current_epoch| shared.current_epoch() == current_epoch)
            && self.recovery.is_none_or(|recovery| {
                state.current.is_none()
                    && state.pending_interruption == Some(recovery)
                    && recovery.matches(self.epoch)
            })
    }

    fn begin(self, state: &mut StreamState, shared: &SessionShared) -> bool {
        if !self.is_current(state, shared) {
            return false;
        }
        state.candidate_epoch = Some(self.epoch);
        state.staging_epoch = None;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CandidateStageIdentity {
    epoch: u64,
    selection_revision: u64,
    lifecycle_revision: u64,
    predecessor_epoch: Option<u64>,
}

struct CandidatePreparationFailure {
    stage: CandidateStageIdentity,
    error: MacosCaptureError,
    settlement: Option<Box<TransactionSettlement<()>>>,
}

impl CandidatePreparationFailure {
    fn new(
        stage: CandidateStageIdentity,
        error: MacosCaptureError,
        settlement: Option<TransactionSettlement<()>>,
    ) -> Self {
        Self {
            stage,
            error,
            settlement: settlement.map(Box::new),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingStreamRequestIdentity {
    epoch: u64,
    request: MacosStreamRequest,
}

#[derive(Debug)]
struct PendingStreamRequest {
    epoch: u64,
    request: MacosStreamRequest,
    completion: TransactionCompleter<()>,
}

impl PendingStreamRequest {
    const fn identity(&self) -> PendingStreamRequestIdentity {
        PendingStreamRequestIdentity {
            epoch: self.epoch,
            request: self.request,
        }
    }
}

struct StreamRemoval {
    role: StreamRole,
    stream: Option<NativeStream>,
    selection_revision: u64,
    request_settlement: Option<TransactionSettlement<()>>,
}

struct CandidatePublication {
    previous: Option<NativeStream>,
    previous_epoch: Option<u64>,
    previous_status: MacosProtectedSourceState,
    previous_selection: MacosCaptureSelection,
    previous_request: MacosStreamRequest,
    previous_selected_filter: Option<NativeSelectionFilter>,
    previous_inactive_epochs: Vec<u64>,
    previous_terminal_epochs: Vec<u64>,
    request_settlement: TransactionSettlement<()>,
}

#[derive(Default)]
struct PublicationSideEffects {
    candidate: Option<CandidatePublication>,
}

struct CandidateActivationAbort {
    payload: Box<dyn std::any::Any + Send>,
    stream: Option<NativeStream>,
    request_settlement: TransactionSettlement<()>,
}

struct RestartDiagnosticReset {
    current: Option<NativeStream>,
    candidate: Option<NativeStream>,
    candidate_settlement: Option<TransactionSettlement<()>>,
}

struct ClaimedSourceResolution {
    resolution: SourceResolution,
    settlement: Option<TransactionSettlement<()>>,
}

struct CandidateReservation {
    stage: CandidateStage,
    selection_filter: NativeSelectionFilter,
    replaced: Option<NativeStream>,
    replaced_settlement: Option<TransactionSettlement<()>>,
}

enum FilterAcceptance {
    Stale,
    Stored(Option<NativeStream>),
    Candidate {
        reservation: Box<CandidateReservation>,
        request: MacosStreamRequest,
    },
}

enum CaptureActivation {
    Unchanged,
    NeedsSelection,
    Candidate {
        reservation: Box<CandidateReservation>,
        request: MacosStreamRequest,
    },
}

#[derive(Default)]
struct StreamState {
    current: Option<NativeStream>,
    candidate: Option<NativeStream>,
    candidate_epoch: Option<u64>,
    selected_filter: Option<NativeSelectionFilter>,
    pending_selection: Option<PendingSelectionFilter>,
    selection_revision: u64,
    lifecycle_revision: u64,
    pending_interruption: Option<InterruptedRestage>,
    staging_epoch: Option<u64>,
    request: MacosStreamRequest,
    pending_request: Option<PendingStreamRequest>,
    candidate_completion: Option<TransactionCompleter<()>>,
    inactive_epochs: Vec<u64>,
    terminal_epochs: Vec<u64>,
    #[cfg(test)]
    fixture_current_epoch: Option<u64>,
    #[cfg(test)]
    fixture_candidate_epoch: Option<u64>,
}

struct StreamSlot {
    // When more than one is required, lock lifecycle_start, rejected_epochs,
    // then state. Native start runs with only the lifecycle gate retained.
    lifecycle_start: Mutex<()>,
    rejected_epochs: Mutex<Vec<u64>>,
    state: Mutex<StreamState>,
    source_transaction: Mutex<Option<SourceTransaction>>,
    lifecycle_callbacks: DispatchRetained<DispatchQueue>,
    native_lifecycle: NativeLifecycle,
    shared: Arc<SessionShared>,
    next_epoch: AtomicU64,
}

mod callbacks;
mod configuration;
mod native_stream_methods;
mod session_methods;
mod session_shared_methods;
mod slot_candidate;
mod slot_control;
mod slot_publication;
mod slot_selection;
mod slot_transactions;

#[cfg(test)]
use callbacks::{
    dispatch_owned_stream_error, dispatch_stream_start_success, handle_owned_fatal_stream_error,
    handle_owned_fatal_stream_error_with, handle_owned_stream_error_with,
};
use callbacks::{handle_fatal_stream_error, handle_stream_error, invoke_stream_start};
#[cfg(test)]
use configuration::{capture_dynamic_range, color_range_from_fourcc};
use configuration::{
    classify_stream_error, conservative_pool_quote, native_error, stream_configuration,
};

pub struct MacosScreenCaptureSession {
    main: MainThreadBound<MainThreadSession>,
    shared: Arc<SessionShared>,
    streams: Arc<StreamSlot>,
    capabilities: MacosCaptureCapabilities,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MacosStreamPoolQuote {
    per_surface_bytes: u64,
    stream_metadata_bytes: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

include!("tests.rs");
