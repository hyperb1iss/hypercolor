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

use capabilities::*;
use frame_decode::*;
use mailbox::*;
use picker::*;
use reference::*;

type PoolBackingLifetime = Arc<dyn Send + Sync>;
type PoolObservation =
    Arc<dyn Fn(u32, u64) -> Result<PoolBackingLifetime, MacosCaptureError> + Send + Sync>;
type PoolReservationFactory =
    Arc<dyn Fn(u64, u64) -> Result<PoolObservation, MacosCaptureError> + Send + Sync>;

const MACOS_IOSURFACE_ROW_ALIGNMENT: u64 = 256;
const MACOS_IOSURFACE_ALLOCATION_ALIGNMENT: u64 = 16 * 1024;
const HYPERCOLOR_UI_BUNDLE_IDENTIFIER: &str = "tech.hyperbliss.hypercolor";
const MACOS_NATIVE_SOURCE_TIMEOUT: Duration = Duration::from_secs(5);
const MACOS_NATIVE_START_TIMEOUT: Duration = Duration::from_secs(5);
const MACOS_NATIVE_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
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

impl SessionShared {
    fn new(
        status: MacosProtectedSourceState,
        selector: MacosCaptureSelector,
        tahoe: MacosTahoeCapabilities,
    ) -> Self {
        Self {
            mailbox: MacosFrameMailbox::new(),
            status: Mutex::new(status),
            selection: Mutex::new(SessionSelectionState::default()),
            selector: Mutex::new(selector),
            tahoe,
            counters: CallbackCounters::default(),
            capture_active: AtomicBool::new(false),
            picker_resolution: Mutex::new(None),
            current_epoch: AtomicU64::new(0),
            resolution_epoch: AtomicU64::new(0),
            restart_diagnostic: Mutex::new(PostAuthorizationStreamDiagnosticState::default()),
        }
    }

    fn status(&self) -> MacosProtectedSourceState {
        *lock(&self.status)
    }

    fn set_status(&self, status: MacosProtectedSourceState) {
        *lock(&self.status) = status;
    }

    fn begin_restart_diagnostic(
        &self,
        authorization_granted: bool,
        selection_revision: u64,
    ) -> Result<
        (
            PostAuthorizationStreamDiagnosticResolution,
            MacosStreamDiagnosticTransaction,
        ),
        MacosCaptureError,
    > {
        let (attempt, superseded, transaction) = {
            let mut state = lock(&self.restart_diagnostic);
            let attempt_id = state
                .next_attempt_id
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            state.next_attempt_id = attempt_id;
            let attempt = PostAuthorizationStreamDiagnosticAttempt {
                attempt_id,
                selection_revision,
            };
            let resolution_epoch = self.allocate_resolution_epoch()?;
            let (transaction, completion) = stream_diagnostic_transaction(attempt_id);
            let superseded = state.active.as_ref().and_then(|active| {
                active
                    .completion
                    .claim(Ok(MacosProtectedSourceState::Failed))
            });
            state.active = Some(PostAuthorizationStreamDiagnostic {
                attempt,
                authorization_granted,
                resolution_epoch: Some(resolution_epoch),
                stream_epoch: None,
                completion,
            });
            (
                PostAuthorizationStreamDiagnosticResolution {
                    attempt,
                    resolution_epoch,
                    selector: MacosCaptureSelector::PrimaryDisplay,
                },
                superseded,
                transaction,
            )
        };
        if let Some(superseded) = superseded {
            superseded.publish();
        }
        if !authorization_granted {
            self.complete_restart_diagnostic_attempt(
                attempt.attempt,
                MacosProtectedSourceState::PermissionDenied,
            );
        }
        Ok((attempt, transaction))
    }

    fn diagnostic_resolution_is_current(
        &self,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
    ) -> bool {
        self.resolution_is_current(resolution.resolution_epoch)
            && lock(&self.restart_diagnostic)
                .active
                .as_ref()
                .is_some_and(|active| {
                    active.attempt == resolution.attempt
                        && active.resolution_epoch == Some(resolution.resolution_epoch)
                })
    }

    fn record_filter_enumerated(
        &self,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
        stream_epoch: u64,
    ) {
        let mut state = lock(&self.restart_diagnostic);
        if let Some(active) = state.active.as_mut()
            && active.attempt == resolution.attempt
            && active.authorization_granted
            && active.resolution_epoch == Some(resolution.resolution_epoch)
        {
            active.stream_epoch = Some(stream_epoch);
        }
    }

    fn record_non_stream_diagnostic_failure(
        &self,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
        state: MacosProtectedSourceState,
    ) {
        if self.diagnostic_resolution_is_current(resolution) {
            self.complete_restart_diagnostic_attempt(
                resolution.attempt,
                if state == MacosProtectedSourceState::PermissionDenied {
                    state
                } else {
                    MacosProtectedSourceState::Failed
                },
            );
        }
    }

    fn fail_restart_diagnostic_attempt(&self, attempt: PostAuthorizationStreamDiagnosticAttempt) {
        self.complete_restart_diagnostic_attempt(attempt, MacosProtectedSourceState::Failed);
    }

    fn claim_restart_diagnostic_completion(
        &self,
        outcome: MacosProtectedSourceState,
    ) -> Option<TransactionSettlement<MacosProtectedSourceState>> {
        let mut state = lock(&self.restart_diagnostic);
        let settlement = state.active.as_ref()?.completion.claim(Ok(outcome))?;
        state.active = None;
        Some(settlement)
    }

    fn restart_diagnostic_completion(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
    ) -> Option<TransactionCompleter<MacosProtectedSourceState>> {
        lock(&self.restart_diagnostic)
            .active
            .as_ref()
            .filter(|active| active.attempt == attempt)
            .map(|active| active.completion.clone())
    }

    fn take_restart_diagnostic_attempt(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
    ) -> Option<TransactionCompleter<MacosProtectedSourceState>> {
        let mut state = lock(&self.restart_diagnostic);
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.attempt == attempt)
        {
            state.active.take().map(|active| active.completion)
        } else {
            None
        }
    }

    fn record_stream_diagnostic_result(
        &self,
        stream_epoch: u64,
        state: MacosProtectedSourceState,
    ) -> MacosProtectedSourceState {
        let settlement = {
            let mut diagnostic = lock(&self.restart_diagnostic);
            let Some(active) = diagnostic
                .active
                .as_ref()
                .filter(|active| active.stream_epoch == Some(stream_epoch))
            else {
                return state;
            };
            let state = if active.authorization_granted
                && state == MacosProtectedSourceState::PermissionDenied
            {
                MacosProtectedSourceState::NeedsProcessRestart
            } else {
                state
            };
            let settlement = active.completion.claim(Ok(state));
            if settlement.is_some() {
                diagnostic.active = None;
            }
            settlement.map(|settlement| (settlement, state))
        };
        if let Some((settlement, state)) = settlement {
            settlement.publish();
            state
        } else {
            state
        }
    }

    fn complete_restart_diagnostic_attempt(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
        outcome: MacosProtectedSourceState,
    ) {
        let settlement = {
            let mut diagnostic = lock(&self.restart_diagnostic);
            if diagnostic
                .active
                .as_ref()
                .is_some_and(|active| active.attempt == attempt)
            {
                let settlement = diagnostic
                    .active
                    .as_ref()
                    .and_then(|active| active.completion.claim(Ok(outcome)));
                if settlement.is_some() {
                    diagnostic.active = None;
                }
                settlement
            } else {
                None
            }
        };
        if let Some(settlement) = settlement {
            settlement.publish();
        }
    }

    fn selection(&self) -> MacosCaptureSelection {
        lock(&self.selection).selection.clone()
    }

    fn set_unconfirmed_selection(&self, selection: MacosCaptureSelection) {
        let mut state = lock(&self.selection);
        state.selection = selection;
        state.tahoe.clear();
    }

    fn confirm_selection(
        &self,
        selection: MacosCaptureSelection,
        source_id: Arc<str>,
        epoch: u64,
        delivery: MacosValidatedStreamDelivery,
    ) {
        let mut state = lock(&self.selection);
        state.selection = selection;
        state.tahoe.confirm(source_id, epoch, delivery, self.tahoe);
    }

    fn clear_tahoe_selection(&self) {
        lock(&self.selection).tahoe.clear();
    }

    fn tahoe_selection_for(
        &self,
        source_id: &str,
        epoch: u64,
    ) -> Option<MacosTahoeSelectionCapabilities> {
        lock(&self.selection).tahoe.current_for(source_id, epoch)
    }

    fn selector(&self) -> MacosCaptureSelector {
        lock(&self.selector).clone()
    }

    fn set_selector(&self, selector: MacosCaptureSelector) {
        *lock(&self.selector) = selector;
    }

    fn capture_active(&self) -> bool {
        self.capture_active.load(Ordering::Acquire)
    }

    fn enable_picker_callbacks(&self, resolution: SourceResolution) {
        *lock(&self.picker_resolution) = Some(resolution);
    }

    fn disable_picker_callbacks(&self) {
        lock(&self.picker_resolution).take();
    }

    fn picker_resolution(&self) -> Option<SourceResolution> {
        lock(&self.picker_resolution).clone()
    }

    fn consume_picker_resolution(&self, resolution: &SourceResolution) -> bool {
        let mut picker = lock(&self.picker_resolution);
        if picker.as_ref() == Some(resolution) {
            picker.take();
            true
        } else {
            false
        }
    }

    fn set_capture_active(&self, active: bool) -> bool {
        self.capture_active.swap(active, Ordering::AcqRel)
    }

    fn begin_resolution(&self) -> Result<u64, MacosCaptureError> {
        let superseded = {
            let mut state = lock(&self.restart_diagnostic);
            let settlement = state.active.as_ref().and_then(|active| {
                active
                    .completion
                    .claim(Ok(MacosProtectedSourceState::Failed))
            });
            state.active = None;
            settlement
        };
        if let Some(superseded) = superseded {
            superseded.publish();
        }
        self.allocate_resolution_epoch()
    }

    fn allocate_resolution_epoch(&self) -> Result<u64, MacosCaptureError> {
        self.resolution_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map(|epoch| epoch + 1)
            .map_err(|_| MacosCaptureError::SequenceExhausted)
    }

    fn resolution_is_current(&self, epoch: u64) -> bool {
        self.resolution_epoch.load(Ordering::Acquire) == epoch
    }

    fn source_resolution_is_current(&self, resolution: &SourceResolution) -> bool {
        match resolution {
            SourceResolution::General(resolution) => {
                self.resolution_is_current(resolution.resolution_epoch)
            }
            SourceResolution::Diagnostic(resolution) => {
                self.diagnostic_resolution_is_current(resolution)
            }
        }
    }

    fn current_epoch(&self) -> u64 {
        self.current_epoch.load(Ordering::Acquire)
    }

    fn activate_epoch(&self, epoch: u64) {
        self.current_epoch.store(epoch, Ordering::Release);
    }

    fn publish(&self, event: MacosFrameEvent) {
        let status = match &event {
            MacosFrameEvent::Frame(_) => {
                self.counters.record_published();
                MacosProtectedSourceState::Live
            }
            MacosFrameEvent::Lifecycle(MacosFrameStatus::Started) => {
                self.counters.record_lifecycle();
                MacosProtectedSourceState::Starting
            }
            MacosFrameEvent::Lifecycle(MacosFrameStatus::Suspended)
            | MacosFrameEvent::Lifecycle(MacosFrameStatus::Stopped) => {
                self.counters.record_lifecycle();
                MacosProtectedSourceState::Interrupted
            }
            MacosFrameEvent::Lifecycle(_) => {
                self.counters.record_lifecycle();
                MacosProtectedSourceState::Live
            }
            MacosFrameEvent::RecoverableError(_) => self.status(),
        };
        self.set_status(status);
        self.mailbox.publish(Ok(event));
    }

    fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        self.counters.snapshot(self.mailbox.superseded_count())
    }

    fn publish_error(&self, error: MacosCaptureError) {
        self.mailbox.publish(Err(error));
    }

    fn publish_recoverable_error(&self, error: MacosCaptureError) {
        self.mailbox
            .publish(Ok(MacosFrameEvent::RecoverableError(Box::new(error))));
    }

    fn record_retirement_error(&self, error: &MacosCaptureError) {
        self.counters.record_drop(error);
    }
}

struct NativeStream {
    stream: Retained<SCStream>,
    filter: NativeFilter,
    selection: MacosCaptureSelection,
    source_id: Arc<str>,
    request: MacosStreamRequest,
    reserve_pool: PoolReservationFactory,
    worker: LatestSampleWorker<RetainedNativeSample>,
    start_completion: CompletionFence,
    _output: Retained<CaptureOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: ScreenCaptureKit owns callback execution across its queues, and all
// Rust access to this owner is serialized through StreamSlot. NativeStream is
// moved between owners but never exposes concurrent mutable Objective-C state.
unsafe impl Send for NativeStream {}

impl NativeStream {
    fn prepare(
        selection_filter: NativeSelectionFilter,
        request: MacosStreamRequest,
        epoch: u64,
        shared: Arc<SessionShared>,
        streams: Weak<StreamSlot>,
        reserve_pool: &PoolReservationFactory,
        native_lifecycle: &NativeLifecycle,
    ) -> Result<Self, MacosCaptureError> {
        let filter = selection_filter.filter.system();
        let (configuration, display_filter, extent, configured_stream) =
            stream_configuration(filter, request)?;
        let quote = conservative_pool_quote(extent, configured_stream.configured_pixel_format)?;
        let pool = reserve_pool(quote.per_surface_bytes, quote.stream_metadata_bytes)?;
        let mut decoder = MacosFrameDecoder::new(epoch);
        let mut delivery_validator = MacosStreamDeliveryValidator::new(configured_stream);
        delivery_validator.validate_configuration()?;
        let decode_shared = Arc::clone(&shared);
        let worker_shared = Arc::clone(&shared);
        let worker_streams = streams.clone();
        let worker = LatestSampleWorker::spawn(
            "hypercolor-macos-screen-capture",
            move |sample: RetainedNativeSample| {
                let _timing = decode_shared.counters.observe_conversion();
                decode_sample(&mut decoder, &mut delivery_validator, sample)
            },
            move |result, publication| {
                publish_decoded_result(result, publication, epoch, &worker_streams, &worker_shared);
            },
        )
        .map_err(|error| MacosCaptureError::CaptureWorkerStartFailed(error.to_string()))?;
        let samples = worker.input();
        let output = CaptureOutput::new(
            epoch,
            samples,
            pool,
            shared,
            streams,
            request.cursor_composed,
            display_filter,
        );
        let setup_shared = Arc::clone(&output.ivars().shared);
        let delegate: &ProtocolObject<dyn SCStreamDelegate> = ProtocolObject::from_ref(&*output);
        // SAFETY: The filter, configuration, and delegate remain retained by
        // the returned stream and NativeStream owner.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                filter,
                &configuration,
                Some(delegate),
            )
        };
        let queue = DispatchQueue::new(
            "tech.hyperbliss.hypercolor.screen-capture",
            DispatchQueueAttr::SERIAL,
        );
        let protocol: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
        // SAFETY: The protocol object and serial queue outlive their stream
        // registration through the NativeStream owner.
        let output_result = unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                protocol,
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        };
        if let Err(error) = output_result {
            setup_shared.record_stream_diagnostic_result(epoch, classify_stream_error(&error));
            let error = native_error("add ScreenCaptureKit output", &error);
            let completion = CompletionFence::new();
            drop(completion.witness());
            let retirement_shared = Arc::clone(&setup_shared);
            native_lifecycle.retire_without_native_stop(worker, completion, move |worker| {
                worker.close();
                if worker.join().is_err() {
                    retirement_shared
                        .counters
                        .record_drop(&MacosCaptureError::CaptureWorkerPanicked);
                }
            });
            return Err(error);
        }
        Ok(Self {
            stream,
            filter: selection_filter.filter,
            selection: selection_filter.selection,
            source_id: selection_filter.source_id,
            request,
            reserve_pool: Arc::clone(reserve_pool),
            worker,
            start_completion: CompletionFence::new(),
            _output: output,
            _queue: queue,
        })
    }

    fn epoch(&self) -> u64 {
        self._output.ivars().epoch
    }

    fn finish_worker_retirement(&mut self) -> Result<(), MacosCaptureError> {
        self.worker.close();
        self.worker
            .join()
            .map_err(|_| MacosCaptureError::CaptureWorkerPanicked)
    }

    fn interruption_restage(&self, selection_revision: u64) -> InterruptedRestagePlan {
        InterruptedRestagePlan {
            recovery: InterruptedRestage::interrupted(self.epoch(), selection_revision),
            selection_filter: NativeSelectionFilter {
                filter: self.filter.clone(),
                selection: self.selection.clone(),
                source_id: Arc::clone(&self.source_id),
            },
            request: self.request,
            reserve_pool: Arc::clone(&self.reserve_pool),
        }
    }
}

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

impl StreamSlot {
    fn new(
        shared: Arc<SessionShared>,
        request: MacosStreamRequest,
    ) -> Result<Arc<Self>, MacosCaptureError> {
        let native_lifecycle = NativeLifecycle::start().map_err(|error| {
            MacosCaptureError::CaptureWorkerStartFailed(format!(
                "start macOS native transaction scheduler: {error}"
            ))
        })?;
        Ok(Arc::new(Self {
            lifecycle_start: Mutex::new(()),
            rejected_epochs: Mutex::new(Vec::new()),
            state: Mutex::new(StreamState {
                request,
                ..StreamState::default()
            }),
            source_transaction: Mutex::new(None),
            lifecycle_callbacks: DispatchQueue::new(
                "tech.hyperbliss.hypercolor.screen-capture-lifecycle",
                DispatchQueueAttr::SERIAL,
            ),
            native_lifecycle,
            shared,
            next_epoch: AtomicU64::new(1),
        }))
    }

    fn allocate_epoch(&self) -> Result<u64, MacosCaptureError> {
        self.next_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map_err(|_| MacosCaptureError::SequenceExhausted)
    }

    fn install_candidate_completion(
        state: &mut StreamState,
        epoch: u64,
        request: Option<&PendingStreamRequest>,
    ) -> Option<TransactionSettlement<()>> {
        let completion = request.map_or_else(
            || {
                TransactionCompleter::new(TransactionIdentity {
                    generation: epoch,
                    phase: MacosNativeTransactionPhase::StreamStart,
                })
            },
            |request| {
                // An adopted in-flight request must follow the stage it now
                // belongs to: every deadline arm, cancel, and claim filters
                // on the live cell generation, and a cell left keyed to the
                // superseded epoch would miss all of them, insta-cancelling
                // the fresh candidate and stranding the core waiter.
                let completion = request.completion.clone();
                // A refused rekey means the cell was already claimed (a
                // timeout won the race); installing it anyway is safe
                // because the stage's own arm declines claimed cells and
                // aborts the stage.
                let _ = completion.rekey_generation(epoch);
                completion
            },
        );
        let replaced = state.candidate_completion.as_ref().and_then(|replaced| {
            (!replaced.shares_cell(&completion)).then(|| {
                let identity = replaced.identity();
                replaced.claim(Err(MacosNativeTransactionError::Cancelled {
                    phase: identity.phase,
                    generation: identity.generation,
                }))
            })
        });
        state.candidate_completion = Some(completion);
        replaced.flatten()
    }

    fn cancel_candidate_completion(state: &mut StreamState) -> Option<TransactionSettlement<()>> {
        let settlement = state.candidate_completion.as_ref().and_then(|completion| {
            let identity = completion.identity();
            completion.claim(Err(MacosNativeTransactionError::Cancelled {
                phase: identity.phase,
                generation: identity.generation,
            }))
        });
        state.candidate_completion = None;
        settlement
    }

    fn finish_replaced_candidate(settlement: Option<TransactionSettlement<()>>) {
        if let Some(settlement) = settlement {
            settlement.publish();
        }
    }

    fn arm_candidate_deadline(
        self: &Arc<Self>,
        epoch: u64,
        phase: MacosNativeTransactionPhase,
        timeout: Duration,
    ) -> Result<bool, MacosCaptureError> {
        let completion = {
            let state = lock(&self.state);
            state
                .candidate_completion
                .as_ref()
                .filter(|completion| completion.identity().generation == epoch)
                .cloned()
        };
        let Some(completion) = completion else {
            return Ok(false);
        };
        let streams = Arc::downgrade(self);
        completion
            .arm_for_generation(
                self.native_lifecycle.deadlines(),
                Instant::now() + timeout,
                epoch,
                phase,
                move || {
                    if let Some(streams) = streams.upgrade() {
                        streams.timeout_candidate(epoch, phase);
                    }
                },
            )
            .map_err(|error| {
                MacosCaptureError::CaptureWorkerStartFailed(format!(
                    "schedule macOS {phase} deadline: {error}"
                ))
            })
    }

    fn timeout_candidate(&self, epoch: u64, phase: MacosNativeTransactionPhase) {
        let stream = {
            let _lifecycle = lock(&self.lifecycle_start);
            let mut state = lock(&self.state);
            let completion = state
                .candidate_completion
                .as_ref()
                .filter(|completion| {
                    let identity = completion.identity();
                    identity.generation == epoch && identity.phase == phase
                })
                .cloned();
            if state.candidate_epoch != Some(epoch) || completion.is_none() {
                return;
            }
            state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
            state.candidate_epoch = None;
            Self::forget_epoch_activity(&mut state, epoch);
            #[cfg(test)]
            {
                state
                    .fixture_candidate_epoch
                    .take_if(|candidate| *candidate == epoch);
            }
            state
                .pending_selection
                .take_if(|pending| pending.epoch == epoch);
            state
                .pending_request
                .take_if(|request| request.epoch == epoch);
            state.candidate_completion = None;
            if state
                .pending_interruption
                .is_some_and(|recovery| recovery.matches(epoch))
            {
                state.pending_interruption = None;
            }
            state.candidate.take()
        };
        if let Some(stream) = stream {
            self.stop_stream(stream);
        }
        let error = MacosCaptureError::CaptureWorkerStartFailed(format!(
            "macOS {phase} transaction {epoch} timed out"
        ));
        if self.shared.current_epoch() == 0 {
            self.shared.set_status(MacosProtectedSourceState::Failed);
            self.shared.publish_error(error);
        } else {
            self.shared.set_status(MacosProtectedSourceState::Live);
            self.shared.publish_recoverable_error(error);
        }
    }

    fn begin_resolution(self: &Arc<Self>) -> Result<SourceResolution, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let resolution_epoch = self.shared.begin_resolution()?;
        let resolution = SourceResolution::General(GeneralSourceResolution {
            resolution_epoch,
            selector: self.shared.selector(),
        });
        self.install_source_transaction(resolution.clone(), Some(MACOS_NATIVE_SOURCE_TIMEOUT))?;
        Ok(resolution)
    }

    fn begin_picker_resolution(self: &Arc<Self>) -> Result<SourceResolution, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let resolution_epoch = self.shared.begin_resolution()?;
        let resolution = SourceResolution::General(GeneralSourceResolution {
            resolution_epoch,
            selector: self.shared.selector(),
        });
        self.install_source_transaction(resolution.clone(), None)?;
        self.shared.enable_picker_callbacks(resolution.clone());
        Ok(resolution)
    }

    fn set_selector(&self, selector: MacosCaptureSelector) {
        let _lifecycle = lock(&self.lifecycle_start);
        let source_settlement = self.cancel_source_transaction_locked();
        self.shared.disable_picker_callbacks();
        self.shared.set_selector(selector);
        if let Some(settlement) = source_settlement {
            settlement.publish();
        }
    }

    fn set_selector_and_begin_resolution(
        self: &Arc<Self>,
        selector: MacosCaptureSelector,
    ) -> Result<SourceResolution, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        self.shared.set_selector(selector.clone());
        let resolution_epoch = self.shared.begin_resolution()?;
        let resolution = SourceResolution::General(GeneralSourceResolution {
            resolution_epoch,
            selector,
        });
        self.install_source_transaction(resolution.clone(), Some(MACOS_NATIVE_SOURCE_TIMEOUT))?;
        Ok(resolution)
    }

    #[cfg(test)]
    fn begin_restart_diagnostic(
        self: &Arc<Self>,
        authorization_granted: bool,
        selection_revision: u64,
    ) -> Result<
        (
            PostAuthorizationStreamDiagnosticResolution,
            MacosStreamDiagnosticTransaction,
        ),
        MacosCaptureError,
    > {
        let _lifecycle = lock(&self.lifecycle_start);
        self.shared
            .set_selector(MacosCaptureSelector::PrimaryDisplay);
        let (resolution, transaction) = self
            .shared
            .begin_restart_diagnostic(authorization_granted, selection_revision)?;
        self.arm_restart_diagnostic(&resolution)?;
        Ok((resolution, transaction))
    }

    fn reset_for_restart_diagnostic_locked(
        &self,
        state: &mut StreamState,
    ) -> Result<RestartDiagnosticReset, MacosCaptureError> {
        let selection_revision = state
            .selection_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        let lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        self.shared.set_capture_active(false);
        let current = state.current.take();
        let candidate = state.candidate.take();
        #[cfg(test)]
        {
            state.fixture_current_epoch = None;
            state.fixture_candidate_epoch = None;
        }
        state.selection_revision = selection_revision;
        state.lifecycle_revision = lifecycle_revision;
        state.selected_filter = None;
        state.pending_selection = None;
        state.pending_interruption = None;
        let candidate_settlement = Self::cancel_candidate_completion(state);
        state.pending_request = None;
        state.staging_epoch = None;
        state.candidate_epoch = None;
        state.inactive_epochs.clear();
        state.terminal_epochs.clear();
        self.shared.activate_epoch(0);
        self.shared.clear_tahoe_selection();
        self.shared
            .set_unconfirmed_selection(MacosCaptureSelection::None);
        self.shared.set_capture_active(true);
        Ok(RestartDiagnosticReset {
            current,
            candidate,
            candidate_settlement,
        })
    }

    fn setup_restart_diagnostic(
        self: &Arc<Self>,
        authorization_granted: bool,
    ) -> Result<
        (
            PostAuthorizationStreamDiagnosticResolution,
            MacosStreamDiagnosticTransaction,
        ),
        MacosCaptureError,
    > {
        self.setup_restart_diagnostic_with(authorization_granted, || {})
    }

    fn setup_restart_diagnostic_with(
        self: &Arc<Self>,
        authorization_granted: bool,
        setup_installed: impl FnOnce(),
    ) -> Result<
        (
            PostAuthorizationStreamDiagnosticResolution,
            MacosStreamDiagnosticTransaction,
        ),
        MacosCaptureError,
    > {
        let (diagnostic, current, candidate, candidate_settlement, source_settlement) = {
            let _lifecycle = lock(&self.lifecycle_start);
            self.shared.disable_picker_callbacks();
            let source_settlement = self.cancel_source_transaction_locked();
            let mut state = lock(&self.state);
            let RestartDiagnosticReset {
                current,
                candidate,
                candidate_settlement,
            } = self.reset_for_restart_diagnostic_locked(&mut state)?;
            self.shared
                .set_selector(MacosCaptureSelector::PrimaryDisplay);
            let selection_revision = state.selection_revision;
            let diagnostic = self
                .shared
                .begin_restart_diagnostic(authorization_granted, selection_revision);
            if diagnostic.is_ok() {
                self.shared.set_status(MacosProtectedSourceState::Starting);
            }
            drop(state);
            setup_installed();
            (
                diagnostic,
                current,
                candidate,
                candidate_settlement,
                source_settlement,
            )
        };
        if let Some(candidate) = candidate {
            self.stop_stream(candidate);
        }
        if let Some(current) = current {
            self.stop_stream(current);
        }
        if let Some(settlement) = source_settlement {
            settlement.publish();
        }
        Self::finish_replaced_candidate(candidate_settlement);
        let (resolution, transaction) = diagnostic?;
        self.arm_restart_diagnostic(&resolution)?;
        Ok((resolution, transaction))
    }

    fn arm_restart_diagnostic(
        self: &Arc<Self>,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
    ) -> Result<(), MacosCaptureError> {
        let Some(completion) = self
            .shared
            .restart_diagnostic_completion(resolution.attempt)
        else {
            return Ok(());
        };
        let cancel_streams = Arc::downgrade(self);
        let attempt = resolution.attempt;
        completion.set_cancel(move |_| {
            if let Some(streams) = cancel_streams.upgrade() {
                streams.finish_restart_diagnostic(attempt);
            }
        });
        let timeout_streams = Arc::downgrade(self);
        let result = completion.arm(
            self.native_lifecycle.deadlines(),
            Instant::now() + MACOS_NATIVE_SOURCE_TIMEOUT,
            move || {
                if let Some(streams) = timeout_streams.upgrade() {
                    streams.finish_restart_diagnostic(attempt);
                }
            },
        );
        if let Err(source) = result {
            let error = MacosCaptureError::CaptureWorkerStartFailed(format!(
                "schedule macOS source resolution deadline: {source}"
            ));
            let settlement = {
                let mut state = lock(&self.shared.restart_diagnostic);
                let settlement = state
                    .active
                    .as_ref()
                    .filter(|active| active.attempt == attempt)
                    .and_then(|active| {
                        active
                            .completion
                            .claim(Err(MacosNativeTransactionError::Capture(error.clone())))
                    });
                if settlement.is_some() {
                    state.active = None;
                }
                settlement
            };
            self.shared.set_status(MacosProtectedSourceState::Failed);
            if let Some(settlement) = settlement {
                settlement.publish();
            }
            return Err(error);
        }
        Ok(())
    }

    fn finish_restart_diagnostic(&self, attempt: PostAuthorizationStreamDiagnosticAttempt) {
        let _lifecycle = lock(&self.lifecycle_start);
        if self
            .shared
            .take_restart_diagnostic_attempt(attempt)
            .is_none()
        {
            return;
        }
        let _ = self.shared.begin_resolution();
        self.shared.set_status(MacosProtectedSourceState::Failed);
    }

    fn install_source_transaction(
        self: &Arc<Self>,
        resolution: SourceResolution,
        timeout: Option<Duration>,
    ) -> Result<(), MacosCaptureError> {
        let generation = match &resolution {
            SourceResolution::General(resolution) => resolution.resolution_epoch,
            SourceResolution::Diagnostic(resolution) => resolution.resolution_epoch,
        };
        let completion = TransactionCompleter::new(TransactionIdentity {
            generation,
            phase: MacosNativeTransactionPhase::SourceResolution,
        });
        let replaced = {
            let mut state = lock(&self.source_transaction);
            let settlement = state.as_ref().and_then(|replaced| {
                let identity = replaced.completion.identity();
                replaced
                    .completion
                    .claim(Err(MacosNativeTransactionError::Cancelled {
                        phase: identity.phase,
                        generation: identity.generation,
                    }))
            });
            *state = Some(SourceTransaction {
                resolution: resolution.clone(),
                completion: completion.clone(),
            });
            settlement
        };
        let Some(timeout) = timeout else {
            if let Some(settlement) = replaced {
                settlement.publish();
            }
            return Ok(());
        };
        let streams = Arc::downgrade(self);
        let result = completion.arm(
            self.native_lifecycle.deadlines(),
            Instant::now() + timeout,
            move || {
                if let Some(streams) = streams.upgrade() {
                    streams.timeout_source_resolution(resolution.clone());
                }
            },
        );
        if let Err(source) = result {
            let error = MacosCaptureError::CaptureWorkerStartFailed(format!(
                "schedule macOS source resolution deadline: {source}"
            ));
            let settlement = {
                let mut state = lock(&self.source_transaction);
                let settlement = state
                    .as_ref()
                    .filter(|transaction| transaction.completion.shares_cell(&completion))
                    .and_then(|transaction| {
                        transaction
                            .completion
                            .claim(Err(MacosNativeTransactionError::Capture(error.clone())))
                    });
                if settlement.is_some() {
                    state.take();
                }
                settlement
            };
            if let Some(settlement) = settlement {
                settlement.publish();
            }
            if let Some(settlement) = replaced {
                settlement.publish();
            }
            return Err(error);
        }
        if let Some(settlement) = replaced {
            settlement.publish();
        }
        Ok(())
    }

    fn claim_source_transaction(
        &self,
        resolution: &SourceResolution,
    ) -> Option<TransactionSettlement<()>> {
        let _lifecycle = lock(&self.lifecycle_start);
        {
            let mut state = lock(&self.source_transaction);
            let settlement = state
                .as_ref()
                .filter(|transaction| transaction.resolution == *resolution)
                .and_then(|transaction| transaction.completion.claim(Ok(())));
            if settlement.is_some() {
                state.take();
            }
            settlement
        }
    }

    fn cancel_source_transaction(
        &self,
        resolution: &SourceResolution,
    ) -> Option<TransactionSettlement<()>> {
        let _lifecycle = lock(&self.lifecycle_start);
        {
            let mut state = lock(&self.source_transaction);
            let settlement = state
                .as_ref()
                .filter(|transaction| transaction.resolution == *resolution)
                .and_then(|transaction| {
                    let identity = transaction.completion.identity();
                    transaction
                        .completion
                        .claim(Err(MacosNativeTransactionError::Cancelled {
                            phase: identity.phase,
                            generation: identity.generation,
                        }))
                });
            if settlement.is_some() {
                state.take();
            }
            settlement
        }
    }

    fn cancel_source_transaction_locked(&self) -> Option<TransactionSettlement<()>> {
        {
            let mut state = lock(&self.source_transaction);
            let settlement = state.as_ref().and_then(|transaction| {
                let identity = transaction.completion.identity();
                transaction
                    .completion
                    .claim(Err(MacosNativeTransactionError::Cancelled {
                        phase: identity.phase,
                        generation: identity.generation,
                    }))
            });
            state.take();
            settlement
        }
    }

    fn timeout_source_resolution(&self, resolution: SourceResolution) {
        let _lifecycle = lock(&self.lifecycle_start);
        let transaction = lock(&self.source_transaction)
            .take_if(|transaction| transaction.resolution == resolution);
        let Some(transaction) = transaction else {
            return;
        };
        let generation = transaction.completion.identity().generation;
        let _ = self.shared.allocate_resolution_epoch();
        self.shared.consume_picker_resolution(&resolution);
        let state = lock(&self.state);
        let preserve_current = Self::current_epoch(&state).is_some();
        let preserve_selection =
            state.pending_selection.is_some() || state.selected_filter.is_some();
        drop(state);
        let error = MacosCaptureError::CaptureWorkerStartFailed(format!(
            "macOS source resolution transaction {} timed out",
            generation
        ));
        if preserve_current || preserve_selection {
            self.shared.publish_recoverable_error(error);
        } else {
            self.shared.set_status(MacosProtectedSourceState::Failed);
            self.shared.publish_error(error);
        }
    }

    fn reserve_selection_candidate_locked(
        &self,
        state: &mut StreamState,
        epoch: u64,
        candidate_request: MacosStreamRequest,
        selection_filter: NativeSelectionFilter,
    ) -> Result<CandidateReservation, MacosCaptureError> {
        let authoritative_request = state
            .pending_request
            .as_ref()
            .map_or(state.request, |pending| pending.request);
        if candidate_request != authoritative_request {
            return Err(MacosCaptureError::CaptureWorkerStartFailed(
                "candidate request snapshot does not match the authoritative stream request"
                    .to_owned(),
            ));
        }
        let selection_revision = state
            .selection_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        let lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.selection_revision = selection_revision;
        state.lifecycle_revision = lifecycle_revision;
        state.pending_interruption = None;
        let request = state.pending_request.take().map(|mut pending| {
            pending.epoch = epoch;
            pending
        });
        let request_identity = request.as_ref().map(PendingStreamRequest::identity);
        let replaced_settlement =
            Self::install_candidate_completion(state, epoch, request.as_ref());
        state.pending_request = request;
        state.pending_selection = Some(PendingSelectionFilter {
            epoch,
            selection_revision: state.selection_revision,
            selection_filter: selection_filter.clone(),
        });
        let stage = CandidateStage {
            epoch,
            selection_revision: state.selection_revision,
            lifecycle_revision,
            predecessor_epoch: Self::current_epoch(state),
            recovery_current_epoch: None,
            recovery: None,
            request: request_identity,
        };
        state.staging_epoch = Some(epoch);
        if let Some(replaced_epoch) = state.candidate_epoch {
            Self::forget_epoch_activity(state, replaced_epoch);
        }
        state.candidate_epoch = None;
        Ok(CandidateReservation {
            stage,
            selection_filter,
            replaced: state.candidate.take(),
            replaced_settlement,
        })
    }

    fn accept_selection_filter_with(
        &self,
        selection_filter: NativeSelectionFilter,
        candidate_request: MacosStreamRequest,
        epoch: u64,
        resolution: SourceResolution,
        picker: bool,
        accepted: impl FnOnce(),
    ) -> Result<FilterAcceptance, MacosCaptureError> {
        self.accept_selection_filter_with_hooks(
            selection_filter,
            candidate_request,
            epoch,
            resolution,
            picker,
            (|| {}, accepted),
        )
    }

    fn accept_selection_filter_with_hooks(
        &self,
        selection_filter: NativeSelectionFilter,
        candidate_request: MacosStreamRequest,
        epoch: u64,
        resolution: SourceResolution,
        picker: bool,
        hooks: (impl FnOnce(), impl FnOnce()),
    ) -> Result<FilterAcceptance, MacosCaptureError> {
        let (before_transition, accepted) = hooks;
        before_transition();
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if !self.resolution_is_current(&state, &resolution) {
            return Ok(FilterAcceptance::Stale);
        }
        if picker && !self.shared.consume_picker_resolution(&resolution) {
            return Ok(FilterAcceptance::Stale);
        }
        let (acceptance, stored_selection) = if self.shared.capture_active() {
            let request = state
                .pending_request
                .as_ref()
                .map_or(state.request, |pending| pending.request);
            let reservation = self.reserve_selection_candidate_locked(
                &mut state,
                epoch,
                candidate_request,
                selection_filter,
            )?;
            (
                FilterAcceptance::Candidate {
                    reservation: Box::new(reservation),
                    request,
                },
                None,
            )
        } else {
            let selection_revision = state
                .selection_revision
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            let lifecycle_revision = state
                .lifecycle_revision
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            state.selection_revision = selection_revision;
            state.lifecycle_revision = lifecycle_revision;
            state.pending_interruption = None;
            state.staging_epoch = None;
            state.pending_selection = None;
            state.inactive_epochs.clear();
            state.terminal_epochs.clear();
            state.candidate_epoch = None;
            let selection = selection_filter.selection.clone();
            state.selected_filter = Some(selection_filter);
            let replaced = state.candidate.take();
            (FilterAcceptance::Stored(replaced), Some(selection))
        };
        if let SourceResolution::Diagnostic(diagnostic) = &resolution {
            self.shared.record_filter_enumerated(diagnostic, epoch);
        }
        drop(state);
        if let Some(selection) = stored_selection {
            self.shared.set_unconfirmed_selection(selection);
            self.shared.set_status(MacosProtectedSourceState::ReadyIdle);
        }
        accepted();
        Ok(acceptance)
    }

    fn accept_selection_filter(
        &self,
        selection_filter: NativeSelectionFilter,
        candidate_request: MacosStreamRequest,
        epoch: u64,
        resolution: SourceResolution,
        picker: bool,
    ) -> Result<FilterAcceptance, MacosCaptureError> {
        self.accept_selection_filter_with(
            selection_filter,
            candidate_request,
            epoch,
            resolution,
            picker,
            || {},
        )
    }

    fn resolution_is_current(&self, state: &StreamState, resolution: &SourceResolution) -> bool {
        self.shared.source_resolution_is_current(resolution)
            && match resolution {
                SourceResolution::General(_) => true,
                SourceResolution::Diagnostic(diagnostic) => {
                    state.selection_revision == diagnostic.attempt.selection_revision
                }
            }
    }

    fn finalize_picker_cancel(&self, resolution: &SourceResolution) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        if !self.resolution_is_current(&state, resolution)
            || !self.shared.consume_picker_resolution(resolution)
        {
            return false;
        }
        let needs_selection = Self::current_epoch(&state).is_none()
            && state.pending_selection.is_none()
            && state.selected_filter.is_none();
        drop(state);
        if needs_selection {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsSelection);
        }
        true
    }

    fn finalize_session_scoped_resolution(&self, resolution: &SourceResolution) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        if !self.resolution_is_current(&state, resolution) {
            return false;
        }
        drop(state);
        self.shared
            .set_status(MacosProtectedSourceState::NeedsSelection);
        true
    }

    fn finalize_resolution_error(
        &self,
        resolution: &SourceResolution,
        consume_picker: bool,
        error: MacosCaptureError,
    ) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        if !self.resolution_is_current(&state, resolution)
            || (consume_picker && !self.shared.consume_picker_resolution(resolution))
        {
            return false;
        }
        if let SourceResolution::Diagnostic(diagnostic) = resolution {
            self.shared.record_non_stream_diagnostic_failure(
                diagnostic,
                MacosProtectedSourceState::Failed,
            );
        }
        let preserve_current = Self::current_epoch(&state).is_some();
        let preserve_selection =
            state.pending_selection.is_some() || state.selected_filter.is_some();
        let preserve_status =
            preserve_current || state.candidate_epoch.is_some() || state.staging_epoch.is_some();
        let status = (!preserve_status).then_some({
            if preserve_selection {
                MacosProtectedSourceState::ReadyIdle
            } else if matches!(error, MacosCaptureError::DisplaySourceUnavailable(_)) {
                MacosProtectedSourceState::NeedsSelection
            } else {
                MacosProtectedSourceState::Failed
            }
        });
        drop(state);
        if let Some(status) = status {
            self.shared.set_status(status);
        }
        if preserve_current || preserve_selection {
            self.shared.publish_recoverable_error(error);
        } else {
            self.shared.publish_error(error);
        }
        true
    }

    fn finalize_picker_failure(
        &self,
        resolution: &SourceResolution,
        error: MacosCaptureError,
    ) -> bool {
        self.finalize_resolution_error(resolution, true, error)
    }

    fn finalize_candidate_preparation_failure(
        &self,
        failure: CandidatePreparationFailure,
        resolution: Option<&SourceResolution>,
    ) -> bool {
        self.finalize_candidate_preparation_failure_with(failure, resolution, || {})
    }

    fn finalize_candidate_preparation_failure_with(
        &self,
        mut failure: CandidatePreparationFailure,
        resolution: Option<&SourceResolution>,
        before_finalization: impl FnOnce(),
    ) -> bool {
        before_finalization();
        let finalized = (|| {
            let _lifecycle = lock(&self.lifecycle_start);
            let mut state = lock(&self.state);
            if resolution.is_some_and(|resolution| !self.resolution_is_current(&state, resolution))
            {
                return false;
            }
            let failed_stage_cleared = state.staging_epoch != Some(failure.stage.epoch)
                && state.candidate_epoch != Some(failure.stage.epoch)
                && state
                    .pending_selection
                    .as_ref()
                    .is_none_or(|pending| pending.epoch != failure.stage.epoch);
            let lifecycle_matches = failed_stage_cleared
                && state.selection_revision == failure.stage.selection_revision
                && state.lifecycle_revision == failure.stage.lifecycle_revision
                && Self::current_epoch(&state) == failure.stage.predecessor_epoch
                && state.staging_epoch.is_none()
                && state.candidate_epoch.is_none();
            if !lifecycle_matches {
                return false;
            }
            let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
                return false;
            };
            if let Some(SourceResolution::Diagnostic(diagnostic)) = resolution {
                self.shared.record_non_stream_diagnostic_failure(
                    diagnostic,
                    MacosProtectedSourceState::Failed,
                );
            }
            let current_epoch = Self::current_epoch(&state);
            let current_inactive =
                current_epoch.is_some_and(|epoch| state.inactive_epochs.contains(&epoch));
            let preserve_current = current_epoch.is_some();
            let preserve_selection = state.selected_filter.is_some();
            let status = if preserve_current {
                if current_inactive {
                    MacosProtectedSourceState::NeedsSelection
                } else {
                    MacosProtectedSourceState::Live
                }
            } else if preserve_selection {
                MacosProtectedSourceState::ReadyIdle
            } else if matches!(
                &failure.error,
                MacosCaptureError::DisplaySourceUnavailable(_)
            ) {
                MacosProtectedSourceState::NeedsSelection
            } else {
                MacosProtectedSourceState::Failed
            };
            state.lifecycle_revision = lifecycle_revision;
            drop(state);
            self.shared.set_status(status);
            if preserve_current || preserve_selection {
                self.shared.publish_recoverable_error(failure.error.clone());
            } else {
                self.shared.publish_error(failure.error.clone());
            }
            true
        })();
        if let Some(settlement) = failure.settlement.take() {
            (*settlement).publish();
        }
        finalized
    }

    fn stage_candidate_with_selection(
        self: &Arc<Self>,
        selection_filter: Option<NativeSelectionFilter>,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
        epoch: u64,
        recovery: Option<InterruptedRestage>,
        request_transaction: Option<PendingStreamRequest>,
    ) -> Result<bool, CandidatePreparationFailure> {
        let failure_stage = {
            let state = lock(&self.state);
            CandidateStageIdentity {
                epoch,
                selection_revision: recovery.map_or(state.selection_revision, |recovery| {
                    recovery.selection_revision
                }),
                lifecycle_revision: state.lifecycle_revision,
                predecessor_epoch: recovery
                    .is_none()
                    .then(|| Self::current_epoch(&state))
                    .flatten(),
            }
        };
        let Some(reservation) = self
            .reserve_candidate_stage(
                epoch,
                request,
                selection_filter,
                recovery,
                request_transaction,
            )
            .map_err(|error| CandidatePreparationFailure {
                stage: failure_stage,
                error,
                settlement: None,
            })?
        else {
            return Ok(false);
        };
        self.prepare_and_start_candidate(reservation, request, reserve_pool)
    }

    fn prepare_and_start_candidate(
        self: &Arc<Self>,
        reservation: CandidateReservation,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
    ) -> Result<bool, CandidatePreparationFailure> {
        let CandidateReservation {
            stage,
            selection_filter,
            replaced,
            replaced_settlement,
        } = reservation;
        if let Some(replaced) = replaced {
            self.stop_stream(replaced);
        }
        Self::finish_replaced_candidate(replaced_settlement);
        let candidate = match NativeStream::prepare(
            selection_filter,
            request,
            stage.epoch,
            Arc::clone(&self.shared),
            Arc::downgrade(self),
            reserve_pool,
            &self.native_lifecycle,
        ) {
            Ok(candidate) => candidate,
            Err(error) => {
                let (identity, settlement) =
                    self.cancel_candidate_stage(stage, Some(error.clone()));
                return Err(CandidatePreparationFailure::new(
                    identity, error, settlement,
                ));
            }
        };
        self.start_candidate_stage(candidate, stage)
    }

    fn reserve_candidate_stage(
        &self,
        epoch: u64,
        candidate_request: MacosStreamRequest,
        candidate_selection: Option<NativeSelectionFilter>,
        recovery: Option<InterruptedRestage>,
        request_transaction: Option<PendingStreamRequest>,
    ) -> Result<Option<CandidateReservation>, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if !self.shared.capture_active() {
            if let Some(request) = request_transaction {
                let Some(settlement) = request.completion.claim(Ok(())) else {
                    return Ok(None);
                };
                state.request = request.request;
                state.pending_request = None;
                drop(state);
                settlement.publish();
            }
            return Ok(None);
        }
        let selection_replacement = request_transaction.is_none() && recovery.is_none();
        match (request_transaction.as_ref(), state.pending_request.as_ref()) {
            (Some(request), Some(_)) => {
                return Err(MacosCaptureError::CaptureWorkerStartFailed(format!(
                    "stream request transaction {} cannot replace another pending request",
                    request.epoch
                )));
            }
            (None, pending) => {
                let authoritative_request =
                    pending.map_or(state.request, |pending| pending.request);
                if candidate_request != authoritative_request {
                    return Err(MacosCaptureError::CaptureWorkerStartFailed(
                        "candidate request snapshot does not match the authoritative stream request"
                            .to_owned(),
                    ));
                }
            }
            _ => {}
        }
        let selection_filter = candidate_selection
            .or_else(|| {
                state
                    .pending_selection
                    .as_ref()
                    .map(|pending| pending.selection_filter.clone())
            })
            .or_else(|| state.selected_filter.clone());
        let Some(selection_filter) = selection_filter else {
            let Some(request) = request_transaction else {
                return Err(MacosCaptureError::CaptureWorkerStartFailed(
                    "candidate has no authoritative selection filter".to_owned(),
                ));
            };
            let Some(settlement) = request.completion.claim(Ok(())) else {
                return Ok(None);
            };
            state.request = request.request;
            state.pending_request = None;
            drop(state);
            settlement.publish();
            return Ok(None);
        };
        let lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        let current_epoch = self.shared.current_epoch();
        let recovery = match recovery {
            Some(recovery) => {
                if !recovery.can_begin(&state, &self.shared) {
                    return Ok(None);
                }
                let recovery = recovery
                    .schedule(epoch)
                    .expect("interrupted recovery schedules exactly one later epoch");
                state.pending_interruption = Some(recovery);
                self.shared
                    .set_status(MacosProtectedSourceState::Interrupted);
                Some(recovery)
            }
            None => {
                if selection_replacement {
                    state.selection_revision = state
                        .selection_revision
                        .checked_add(1)
                        .ok_or(MacosCaptureError::SequenceExhausted)?;
                }
                state.pending_interruption = None;
                None
            }
        };
        state.lifecycle_revision = lifecycle_revision;
        let request = request_transaction.or_else(|| {
            state.pending_request.take().map(|mut pending| {
                pending.epoch = epoch;
                pending
            })
        });
        let request_identity = request.as_ref().map(PendingStreamRequest::identity);
        let replaced_settlement =
            Self::install_candidate_completion(&mut state, epoch, request.as_ref());
        state.pending_request = request;
        state.pending_selection = Some(PendingSelectionFilter {
            epoch,
            selection_revision: state.selection_revision,
            selection_filter: selection_filter.clone(),
        });
        let stage = CandidateStage {
            epoch,
            selection_revision: state.selection_revision,
            lifecycle_revision,
            predecessor_epoch: Self::current_epoch(&state),
            recovery_current_epoch: recovery.map(|_| current_epoch),
            recovery,
            request: request_identity,
        };
        state.staging_epoch = Some(epoch);
        if let Some(replaced_epoch) = state.candidate_epoch {
            Self::forget_epoch_activity(&mut state, replaced_epoch);
        }
        state.candidate_epoch = None;
        Ok(Some(CandidateReservation {
            stage,
            selection_filter,
            replaced: state.candidate.take(),
            replaced_settlement,
        }))
    }

    fn cancel_candidate_stage(
        &self,
        stage: CandidateStage,
        error: Option<MacosCaptureError>,
    ) -> (CandidateStageIdentity, Option<TransactionSettlement<()>>) {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        let mut identity = stage.identity();
        let current = state.lifecycle_revision == stage.lifecycle_revision
            && state.staging_epoch == Some(stage.epoch);
        let settlement = current.then(|| {
            error.as_ref().and_then(|error| {
                state
                    .candidate_completion
                    .as_ref()
                    .filter(|completion| completion.identity().generation == stage.epoch)
                    .and_then(|completion| {
                        completion.claim(Err(MacosNativeTransactionError::Capture(error.clone())))
                    })
            })
        });
        if current {
            state.staging_epoch = None;
            state
                .pending_selection
                .take_if(|pending| pending.epoch == stage.epoch);
            if stage
                .recovery
                .is_some_and(|recovery| state.pending_interruption == Some(recovery))
            {
                state.pending_interruption = None;
            }
            if let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) {
                state.lifecycle_revision = lifecycle_revision;
                identity.lifecycle_revision = lifecycle_revision;
            }
        }
        if current {
            state.candidate_completion = None;
        }
        if current
            && stage.request.is_some_and(|request| {
                state
                    .pending_request
                    .as_ref()
                    .is_some_and(|pending| pending.identity() == request)
            })
        {
            state.pending_request = None;
        }
        drop(state);
        (identity, settlement.flatten())
    }

    #[cfg(test)]
    fn fail_candidate_preparation_fixture(
        &self,
        stage: CandidateStage,
        error: MacosCaptureError,
    ) -> CandidatePreparationFailure {
        let (identity, settlement) = self.cancel_candidate_stage(stage, Some(error.clone()));
        CandidatePreparationFailure::new(identity, error, settlement)
    }

    fn start_candidate_stage(
        self: &Arc<Self>,
        candidate: NativeStream,
        stage: CandidateStage,
    ) -> Result<bool, CandidatePreparationFailure> {
        match self.arm_candidate_deadline(
            stage.epoch,
            MacosNativeTransactionPhase::StreamStart,
            MACOS_NATIVE_START_TIMEOUT,
        ) {
            Ok(true) => {}
            Ok(false) => {
                let error = MacosCaptureError::CaptureWorkerStartFailed(
                    "stream request candidate was superseded before start".to_owned(),
                );
                let (_, settlement) = self.cancel_candidate_stage(stage, Some(error));
                self.retire_unstarted_stream(candidate);
                if let Some(settlement) = settlement {
                    settlement.publish();
                }
                return Ok(false);
            }
            Err(error) => {
                let (identity, settlement) =
                    self.cancel_candidate_stage(stage, Some(error.clone()));
                self.retire_unstarted_stream(candidate);
                return Err(CandidatePreparationFailure::new(
                    identity, error, settlement,
                ));
            }
        }
        let stream = candidate.stream.clone();
        let start_completion = candidate.start_completion.witness();
        let mut candidate = Some(candidate);
        let started = self.invoke_candidate_start(
            stage,
            |state| state.candidate = candidate.take(),
            || {
                start_stream(
                    &stream,
                    stage.epoch,
                    Arc::downgrade(self),
                    Arc::clone(&self.shared),
                    start_completion,
                );
            },
        );
        if !started {
            let error = MacosCaptureError::CaptureWorkerStartFailed(
                "stream request candidate was superseded before start".to_owned(),
            );
            let (_, settlement) = self.cancel_candidate_stage(stage, Some(error));
            self.retire_unstarted_stream(candidate.expect("uninstalled candidate remains owned"));
            if let Some(settlement) = settlement {
                settlement.publish();
            }
            return Ok(false);
        }
        Ok(true)
    }

    fn invoke_candidate_start(
        &self,
        stage: CandidateStage,
        install: impl FnOnce(&mut StreamState),
        invoke_start: impl FnOnce(),
    ) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        {
            let mut state = lock(&self.state);
            if !stage.begin(&mut state, &self.shared) {
                return false;
            }
            install(&mut state);
            self.shared.set_status(MacosProtectedSourceState::Starting);
        }
        invoke_start();
        true
    }

    #[cfg(test)]
    fn start_candidate_fixture(&self, stage: CandidateStage) -> bool {
        self.start_candidate_fixture_with(stage, || {})
    }

    #[cfg(test)]
    fn start_candidate_fixture_with(
        &self,
        stage: CandidateStage,
        invoke_start: impl FnOnce(),
    ) -> bool {
        self.invoke_candidate_start(
            stage,
            |state| state.fixture_candidate_epoch = Some(stage.epoch),
            invoke_start,
        )
    }

    #[cfg(test)]
    fn activate_candidate_fixture(&self, epoch: u64) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let rejected = lock(&self.rejected_epochs);
        let mut state = lock(&self.state);
        if !Self::candidate_is_activatable(&state, &rejected, epoch) {
            return false;
        }
        let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
            return false;
        };
        let Some(completion) = state.candidate_completion.as_ref().cloned() else {
            return false;
        };
        let Some(settlement) = completion.claim(Ok(())) else {
            return false;
        };
        state.lifecycle_revision = lifecycle_revision;
        state.candidate_epoch = None;
        state.fixture_candidate_epoch = None;
        state.fixture_current_epoch = Some(epoch);
        Self::commit_pending_selection(&mut state, epoch);
        state.candidate_completion = None;
        Self::commit_pending_request(&mut state, epoch);
        self.shared.activate_epoch(epoch);
        drop(state);
        settlement.publish();
        true
    }

    #[cfg(test)]
    fn fail_candidate_fixture(&self, epoch: u64, error: MacosCaptureError) -> bool {
        let removal = self.remove(epoch, Some(MacosNativeTransactionError::Capture(error)));
        if removal.role != StreamRole::Candidate {
            return false;
        }
        if let Some(settlement) = removal.request_settlement {
            settlement.publish();
        }
        true
    }

    #[cfg(test)]
    fn drain_lifecycle_callbacks(&self) {
        self.lifecycle_callbacks.exec_sync(|| {});
    }

    fn current_is_epoch(state: &StreamState, epoch: u64) -> bool {
        let current = state.current.as_ref().map(NativeStream::epoch);
        #[cfg(test)]
        {
            current.or(state.fixture_current_epoch) == Some(epoch)
        }
        #[cfg(not(test))]
        {
            current == Some(epoch)
        }
    }

    fn current_epoch(state: &StreamState) -> Option<u64> {
        let current = state.current.as_ref().map(NativeStream::epoch);
        #[cfg(test)]
        {
            current.or(state.fixture_current_epoch)
        }
        #[cfg(not(test))]
        {
            current
        }
    }

    fn tracks_epoch(state: &StreamState, epoch: u64) -> bool {
        Self::current_is_epoch(state, epoch)
            || state.candidate_epoch == Some(epoch)
            || state
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.epoch() == epoch)
    }

    fn forget_epoch_activity(state: &mut StreamState, epoch: u64) {
        state.inactive_epochs.retain(|inactive| *inactive != epoch);
        state.terminal_epochs.retain(|terminal| *terminal != epoch);
    }

    fn record_stream_activity(&self, epoch: u64, active: bool, display_filter: bool) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if !Self::tracks_epoch(&state, epoch) {
            return false;
        }
        if display_filter {
            return false;
        }
        let changed = if active {
            let changed =
                state.inactive_epochs.contains(&epoch) || state.terminal_epochs.contains(&epoch);
            Self::forget_epoch_activity(&mut state, epoch);
            changed
        } else if !state.inactive_epochs.contains(&epoch) {
            state.inactive_epochs.push(epoch);
            true
        } else {
            false
        };
        let current = Self::current_is_epoch(&state, epoch);
        drop(state);
        if current {
            self.shared.set_status(if active {
                MacosProtectedSourceState::Live
            } else {
                MacosProtectedSourceState::NeedsSelection
            });
        }
        changed
    }

    fn activate_candidate_for_publication(
        &self,
        state: &mut StreamState,
        rejected: &[u64],
        epoch: u64,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        after_claim: impl FnOnce(),
    ) -> Result<Option<PublicationSideEffects>, Box<CandidateActivationAbort>> {
        if !Self::candidate_is_activatable(state, rejected, epoch) {
            return Ok(None);
        }
        let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
            return Ok(None);
        };
        let Some(confirmed_delivery) = confirmed_delivery else {
            return Ok(None);
        };
        #[cfg(not(test))]
        if state.candidate.is_none() {
            return Ok(None);
        }
        #[cfg(test)]
        let fixture_candidate = state.fixture_candidate_epoch == Some(epoch);
        #[cfg(test)]
        if state.candidate.is_none() && !fixture_candidate {
            return Ok(None);
        }
        let Some(request_completion) = state.candidate_completion.as_ref().cloned() else {
            return Ok(None);
        };
        let previous_epoch = Self::current_epoch(state);
        let previous_status = self.shared.status();
        let previous_selection = self.shared.selection();
        let previous_request = state.request;
        let previous_selected_filter = state.selected_filter.clone();
        let previous_inactive_epochs = state.inactive_epochs.clone();
        let previous_terminal_epochs = state.terminal_epochs.clone();
        let confirmed_selection = state.candidate.as_ref().map(|candidate| {
            (
                candidate.selection.clone(),
                Arc::clone(&candidate.source_id),
            )
        });
        let Some(request_settlement) = request_completion.claim(Ok(())) else {
            return Ok(None);
        };
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(after_claim)) {
            let removal = Self::remove_candidate_locked(state, epoch, None)
                .expect("claimed candidate remains tracked until activation commits");
            return Err(Box::new(CandidateActivationAbort {
                payload,
                stream: removal.stream,
                request_settlement,
            }));
        }
        let candidate = state.candidate.take();
        #[cfg(test)]
        state
            .fixture_candidate_epoch
            .take_if(|candidate| *candidate == epoch);
        state.lifecycle_revision = lifecycle_revision;
        state.candidate_epoch = None;
        let previous = candidate.and_then(|candidate| state.current.replace(candidate));
        #[cfg(test)]
        if fixture_candidate {
            state.fixture_current_epoch = Some(epoch);
        }
        if let Some(previous_epoch) = previous_epoch {
            Self::forget_epoch_activity(state, previous_epoch);
        }
        Self::commit_pending_selection(state, epoch);
        state.candidate_completion = None;
        Self::commit_pending_request(state, epoch);
        let recovered = state
            .pending_interruption
            .take_if(|recovery| recovery.matches(epoch))
            .is_some();
        if let Some((selection, source_id)) = confirmed_selection {
            self.shared
                .confirm_selection(selection, source_id, epoch, confirmed_delivery);
        }
        self.shared.activate_epoch(epoch);
        if recovered {
            self.shared.set_status(MacosProtectedSourceState::Live);
        }
        Ok(Some(PublicationSideEffects {
            candidate: Some(CandidatePublication {
                previous,
                previous_epoch,
                previous_status,
                previous_selection,
                previous_request,
                previous_selected_filter,
                previous_inactive_epochs,
                previous_terminal_epochs,
                request_settlement,
            }),
        }))
    }

    fn rollback_candidate_publication(
        &self,
        epoch: u64,
        candidate: &mut CandidatePublication,
    ) -> Option<NativeStream> {
        let mut state = lock(&self.state);
        let failed = Self::current_is_epoch(&state, epoch)
            .then(|| state.current.take())
            .flatten();
        state.current = candidate.previous.take();
        state.request = candidate.previous_request;
        state.selected_filter = candidate.previous_selected_filter.take();
        state.pending_selection = None;
        state.pending_request = None;
        state.pending_interruption = None;
        state.candidate_completion = None;
        state.inactive_epochs = std::mem::take(&mut candidate.previous_inactive_epochs);
        state.terminal_epochs = std::mem::take(&mut candidate.previous_terminal_epochs);
        #[cfg(test)]
        {
            state.fixture_current_epoch = candidate.previous_epoch;
            state.fixture_candidate_epoch = None;
        }
        self.shared
            .activate_epoch(candidate.previous_epoch.unwrap_or_default());
        self.shared
            .set_unconfirmed_selection(candidate.previous_selection.clone());
        self.shared.set_status(candidate.previous_status);
        failed
    }

    fn publish_decoded_sample(
        &self,
        epoch: u64,
        sample: DecodedSample,
        publication: &SamplePublication,
    ) -> bool {
        let is_frame = matches!(&sample.event, MacosFrameEvent::Frame(_));
        self.publish_decoded_event_if(
            epoch,
            is_frame,
            sample.confirmed_delivery,
            || publication.is_current(),
            || self.shared.publish(sample.event),
        )
    }

    fn publish_stream_lifecycle(&self, epoch: u64, status: MacosFrameStatus) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let rejected = lock(&self.rejected_epochs);
        let mut state = lock(&self.state);
        if !self.shared.capture_active()
            || rejected.contains(&epoch)
            || !Self::tracks_epoch(&state, epoch)
        {
            return false;
        }
        let current = Self::current_is_epoch(&state, epoch);
        if matches!(
            status,
            MacosFrameStatus::Suspended | MacosFrameStatus::Stopped
        ) {
            if state.terminal_epochs.contains(&epoch) {
                return false;
            }
            let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
                return false;
            };
            state.lifecycle_revision = lifecycle_revision;
            if !state.inactive_epochs.contains(&epoch) {
                state.inactive_epochs.push(epoch);
            }
            state.terminal_epochs.push(epoch);
            drop(state);
            if current {
                self.shared.publish(MacosFrameEvent::Lifecycle(status));
            }
            return true;
        }
        if !current || state.inactive_epochs.contains(&epoch) {
            return false;
        }
        drop(state);
        self.shared.publish(MacosFrameEvent::Lifecycle(status));
        true
    }

    #[cfg(test)]
    fn publish_decoded_event_with(
        &self,
        epoch: u64,
        is_frame: bool,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        publish: impl FnOnce(),
    ) -> bool {
        self.publish_decoded_event_if(epoch, is_frame, confirmed_delivery, || true, publish)
    }

    fn publish_decoded_event_if(
        &self,
        epoch: u64,
        is_frame: bool,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        publication_is_current: impl FnOnce() -> bool,
        publish: impl FnOnce(),
    ) -> bool {
        self.publish_decoded_event_if_with_claim_hook(
            epoch,
            is_frame,
            confirmed_delivery,
            publication_is_current,
            || {},
            publish,
        )
    }

    #[cfg(test)]
    fn publish_decoded_event_with_claim_hook(
        &self,
        epoch: u64,
        confirmed_delivery: MacosValidatedStreamDelivery,
        after_claim: impl FnOnce(),
        publish: impl FnOnce(),
    ) -> bool {
        self.publish_decoded_event_if_with_claim_hook(
            epoch,
            true,
            Some(confirmed_delivery),
            || true,
            after_claim,
            publish,
        )
    }

    fn publish_decoded_event_if_with_claim_hook(
        &self,
        epoch: u64,
        is_frame: bool,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        publication_is_current: impl FnOnce() -> bool,
        after_candidate_claim: impl FnOnce(),
        publish: impl FnOnce(),
    ) -> bool {
        let lifecycle = lock(&self.lifecycle_start);
        if !publication_is_current() {
            return false;
        }
        let rejected = lock(&self.rejected_epochs);
        let mut state = lock(&self.state);
        if !self.shared.capture_active()
            || rejected.contains(&epoch)
            || state.inactive_epochs.contains(&epoch)
        {
            return false;
        }
        let side_effects = if Self::current_is_epoch(&state, epoch) {
            PublicationSideEffects::default()
        } else if is_frame {
            match self.activate_candidate_for_publication(
                &mut state,
                &rejected,
                epoch,
                confirmed_delivery,
                after_candidate_claim,
            ) {
                Ok(Some(side_effects)) => side_effects,
                Ok(None) => return false,
                Err(abort) => {
                    let CandidateActivationAbort {
                        payload,
                        stream,
                        request_settlement,
                    } = *abort;
                    if let Some(stream) = stream {
                        self.stop_stream(stream);
                    }
                    drop(request_settlement);
                    drop(state);
                    drop(rejected);
                    drop(lifecycle);
                    std::panic::resume_unwind(payload);
                }
            }
        } else {
            return false;
        };
        drop(state);
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(publish)) {
            let mut side_effects = side_effects;
            if let Some(mut candidate) = side_effects.candidate.take() {
                if let Some(stream) = self.rollback_candidate_publication(epoch, &mut candidate) {
                    self.stop_stream(stream);
                }
                drop(candidate.request_settlement);
            }
            drop(rejected);
            drop(lifecycle);
            std::panic::resume_unwind(payload);
        }
        let previous = if let Some(candidate) = side_effects.candidate {
            candidate.request_settlement.publish();
            candidate.previous
        } else {
            None
        };
        drop(rejected);
        drop(lifecycle);
        if let Some(previous) = previous {
            self.stop_stream(previous);
        }
        true
    }

    fn commit_pending_request(state: &mut StreamState, epoch: u64) {
        if let Some(request) = state
            .pending_request
            .take_if(|request| request.epoch == epoch)
        {
            state.request = request.request;
        }
    }

    fn commit_pending_selection(state: &mut StreamState, epoch: u64) {
        if let Some(pending) = state
            .pending_selection
            .take_if(|pending| pending.epoch == epoch)
        {
            state.selected_filter = Some(pending.selection_filter);
        }
    }

    fn candidate_is_activatable(state: &StreamState, rejected: &[u64], epoch: u64) -> bool {
        !rejected.contains(&epoch)
            && !state.inactive_epochs.contains(&epoch)
            && state.candidate_epoch == Some(epoch)
            && state.pending_selection.as_ref().is_some_and(|pending| {
                pending.epoch == epoch && pending.selection_revision == state.selection_revision
            })
    }

    fn remove(
        &self,
        epoch: u64,
        request_error: Option<MacosNativeTransactionError>,
    ) -> StreamRemoval {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if let Some(removal) =
            Self::remove_candidate_locked(&mut state, epoch, request_error.as_ref())
        {
            return removal;
        }
        if Self::current_is_epoch(&state, epoch) {
            state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
            let current = state.current.take();
            Self::forget_epoch_activity(&mut state, epoch);
            #[cfg(test)]
            {
                state
                    .fixture_current_epoch
                    .take_if(|current| *current == epoch);
            }
            self.shared.activate_epoch(0);
            self.shared.clear_tahoe_selection();
            return StreamRemoval {
                role: StreamRole::Current,
                stream: current,
                selection_revision: state.selection_revision,
                request_settlement: None,
            };
        }
        StreamRemoval {
            role: StreamRole::Stale,
            stream: None,
            selection_revision: state.selection_revision,
            request_settlement: None,
        }
    }

    fn remove_candidate_locked(
        state: &mut StreamState,
        epoch: u64,
        request_error: Option<&MacosNativeTransactionError>,
    ) -> Option<StreamRemoval> {
        if state.candidate_epoch != Some(epoch) {
            return None;
        }
        let request_settlement = request_error.and_then(|error| {
            state
                .candidate_completion
                .as_ref()
                .filter(|completion| completion.identity().generation == epoch)
                .and_then(|completion| completion.claim(Err(error.clone())))
        });
        state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
        state.candidate_epoch = None;
        Self::forget_epoch_activity(state, epoch);
        #[cfg(test)]
        {
            state
                .fixture_candidate_epoch
                .take_if(|candidate| *candidate == epoch);
        }
        state
            .pending_selection
            .take_if(|pending| pending.epoch == epoch);
        state.candidate_completion = None;
        state
            .pending_request
            .take_if(|request| request.epoch == epoch);
        if state
            .pending_interruption
            .is_some_and(|recovery| recovery.matches(epoch))
        {
            state.pending_interruption = None;
        }
        Some(StreamRemoval {
            role: StreamRole::Candidate,
            stream: state.candidate.take(),
            selection_revision: state.selection_revision,
            request_settlement,
        })
    }

    fn cancel_candidate_transaction(&self, epoch: u64) {
        let removal = {
            let _lifecycle = lock(&self.lifecycle_start);
            let mut state = lock(&self.state);
            let Some(removal) = Self::remove_candidate_locked(&mut state, epoch, None) else {
                return;
            };
            removal
        };
        if let Some(stream) = removal.stream {
            self.stop_stream(stream);
        }
        self.shared.set_status(if self.shared.current_epoch() == 0 {
            MacosProtectedSourceState::ReadyIdle
        } else {
            MacosProtectedSourceState::Live
        });
    }

    fn accepts_epoch(&self, epoch: u64) -> bool {
        let rejected = lock(&self.rejected_epochs);
        if rejected.contains(&epoch) {
            return false;
        }
        let state = lock(&self.state);
        !state.inactive_epochs.contains(&epoch)
            && (state
                .current
                .as_ref()
                .is_some_and(|stream| stream.epoch() == epoch)
                || state
                    .candidate
                    .as_ref()
                    .is_some_and(|stream| stream.epoch() == epoch))
    }

    fn record_stream_start_success(&self, epoch: u64) {
        let _lifecycle = lock(&self.lifecycle_start);
        let rejected = lock(&self.rejected_epochs);
        if rejected.contains(&epoch) {
            return;
        }
        let tracked = {
            let state = lock(&self.state);
            state.candidate_epoch == Some(epoch)
                || state
                    .current
                    .as_ref()
                    .is_some_and(|stream| stream.epoch() == epoch)
        };
        if tracked {
            self.shared
                .record_stream_diagnostic_result(epoch, MacosProtectedSourceState::ReadyIdle);
        }
    }

    fn reject_epoch(&self, epoch: u64) {
        let mut rejected = lock(&self.rejected_epochs);
        if !rejected.contains(&epoch) {
            rejected.push(epoch);
        }
    }

    fn clear_rejected_epoch(&self, epoch: u64) {
        lock(&self.rejected_epochs).retain(|rejected| *rejected != epoch);
    }

    fn selection_revision(&self) -> u64 {
        lock(&self.state).selection_revision
    }

    fn has_newer_lifecycle(&self, selection_revision: u64) -> bool {
        let state = lock(&self.state);
        state.selection_revision != selection_revision
            || Self::current_epoch(&state).is_some()
            || state.candidate_epoch.is_some()
            || state.staging_epoch.is_some()
    }

    fn finalize_stream_error(
        &self,
        role: StreamRole,
        selection_revision: u64,
        terminal_state: MacosProtectedSourceState,
        error: MacosCaptureError,
    ) {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        let current_epoch = Self::current_epoch(&state);
        let preserve_current = role == StreamRole::Candidate && current_epoch.is_some();
        let superseded_candidate = role == StreamRole::Candidate
            && (state.selection_revision != selection_revision
                || state.candidate_epoch.is_some()
                || state.staging_epoch.is_some());
        let superseded_current = role == StreamRole::Current
            && (!self.shared.capture_active()
                || state.selection_revision != selection_revision
                || current_epoch.is_some()
                || state.candidate_epoch.is_some()
                || state.staging_epoch.is_some());
        let current_inactive =
            current_epoch.is_some_and(|epoch| state.inactive_epochs.contains(&epoch));
        drop(state);
        if superseded_candidate || superseded_current {
            self.shared.publish_recoverable_error(error);
        } else if preserve_current {
            self.shared.set_status(if current_inactive {
                MacosProtectedSourceState::NeedsSelection
            } else {
                MacosProtectedSourceState::Live
            });
            self.shared.publish_recoverable_error(error);
        } else if role != StreamRole::Stale {
            self.shared.set_status(terminal_state);
            self.shared.publish_error(error);
        }
    }

    fn active_identity(&self) -> Option<(Arc<str>, u64)> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|current| (Arc::clone(&current.source_id), current.epoch()))
    }

    fn has_selection(&self) -> bool {
        let state = lock(&self.state);
        state.pending_selection.is_some() || state.selected_filter.is_some()
    }

    #[cfg(test)]
    fn clear_selection(&self) -> Result<(), MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        state.selection_revision = state
            .selection_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.selected_filter = None;
        state.pending_selection = None;
        state.pending_interruption = None;
        let candidate_settlement = Self::cancel_candidate_completion(&mut state);
        state.pending_request = None;
        state.staging_epoch = None;
        state.candidate_epoch = None;
        state.inactive_epochs.clear();
        state.terminal_epochs.clear();
        drop(state);
        Self::finish_replaced_candidate(candidate_settlement);
        self.shared
            .set_unconfirmed_selection(MacosCaptureSelection::None);
        Ok(())
    }

    fn screenshot_capability(
        &self,
    ) -> Result<MacosScreenshotReferenceCapability, MacosCaptureError> {
        let state = lock(&self.state);
        let Some(current) = state.current.as_ref() else {
            return Ok(MacosScreenshotReferenceCapability::PendingFirstFrame);
        };
        self.capability_for_current(current)
    }

    fn screenshot_snapshot(&self) -> Result<ScreenshotTransactionSnapshot, MacosCaptureError> {
        let state = lock(&self.state);
        let current = state
            .current
            .as_ref()
            .ok_or(MacosCaptureError::ScreenshotCapabilityPending)?;
        let capability = self.capability_for_current(current)?;
        Ok(ScreenshotTransactionSnapshot {
            filter: ScreenshotFilterHandle::Native(current.filter.clone()),
            source_id: Arc::clone(&current.source_id),
            generation: current.epoch(),
            selection_revision: state.selection_revision,
            capability,
        })
    }

    fn capability_for_current(
        &self,
        current: &NativeStream,
    ) -> Result<MacosScreenshotReferenceCapability, MacosCaptureError> {
        if !self.shared.tahoe.screenshot_api.is_present() {
            return Err(MacosCaptureError::TahoePlatformDefect(
                "Tahoe screenshot API",
            ));
        }
        if !self.shared.tahoe.content_tone_mapping_info.is_present() {
            return Err(MacosCaptureError::TahoePlatformDefect(
                "Core Graphics Tahoe tone mapping",
            ));
        }
        crate::screenshot::require_tahoe_reference_output_symbols()?;
        let capability = self
            .shared
            .tahoe_selection_for(&current.source_id, current.epoch())
            .ok_or(MacosCaptureError::ScreenshotCapabilityPending)?;
        if capability.hdr_capture {
            if !capability.dual_range_screenshots {
                return Err(MacosCaptureError::TahoePlatformDefect(
                    "paired SDR and HDR screenshots",
                ));
            }
            Ok(MacosScreenshotReferenceCapability::PairedSdrHdr {
                source_id: capability.source_id,
                generation: capability.capture_session_generation,
            })
        } else {
            Ok(MacosScreenshotReferenceCapability::SdrOnly {
                source_id: capability.source_id,
                generation: capability.capture_session_generation,
            })
        }
    }

    fn request(&self) -> MacosStreamRequest {
        let state = lock(&self.state);
        state
            .pending_request
            .as_ref()
            .map_or(state.request, |pending| pending.request)
    }

    fn committed_request(&self) -> MacosStreamRequest {
        lock(&self.state).request
    }

    fn set_request(
        self: &Arc<Self>,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
    ) -> Result<MacosStreamRequestTransaction, MacosCaptureError> {
        let (transaction, reservation) = self.begin_request_candidate(request)?;
        if let Some(reservation) = reservation
            && let Err(failure) =
                self.prepare_and_start_candidate(reservation, request, reserve_pool)
        {
            let error = failure.error.clone();
            self.finalize_candidate_preparation_failure(failure, None);
            return Err(error);
        }
        Ok(transaction)
    }

    fn begin_request_candidate(
        self: &Arc<Self>,
        request: MacosStreamRequest,
    ) -> Result<(MacosStreamRequestTransaction, Option<CandidateReservation>), MacosCaptureError>
    {
        {
            let _lifecycle = lock(&self.lifecycle_start);
            let state = lock(&self.state);
            if state.pending_request.is_some() {
                return Err(MacosCaptureError::CaptureWorkerStartFailed(
                    "another stream request transaction is still pending".to_owned(),
                ));
            }
            if state.request == request {
                let generation = self.shared.current_epoch();
                let (transaction, completion) = stream_request_transaction(generation);
                if let Some(settlement) = completion.claim(Ok(())) {
                    settlement.publish();
                }
                return Ok((transaction, None));
            }
        }
        let epoch = self.allocate_epoch()?;
        let (transaction, completion) = stream_request_transaction(epoch);
        let reservation = self.reserve_candidate_stage(
            epoch,
            request,
            None,
            None,
            Some(PendingStreamRequest {
                epoch,
                request,
                completion: completion.clone(),
            }),
        )?;
        let cancel_streams = Arc::downgrade(self);
        completion.set_cancel(move |generation| {
            if let Some(streams) = cancel_streams.upgrade() {
                streams.cancel_candidate_transaction(generation);
            }
        });
        Ok((transaction, reservation))
    }

    #[cfg(test)]
    fn begin_request_candidate_fixture(
        self: &Arc<Self>,
        request: MacosStreamRequest,
    ) -> Result<(MacosStreamRequestTransaction, Option<NativeStream>), MacosCaptureError> {
        let (transaction, reservation) = self.begin_request_candidate(request)?;
        let Some(reservation) = reservation else {
            return Ok((transaction, None));
        };
        let CandidateReservation {
            stage,
            replaced,
            replaced_settlement,
            ..
        } = reservation;
        Self::finish_replaced_candidate(replaced_settlement);
        if !self.arm_candidate_deadline(
            stage.epoch,
            MacosNativeTransactionPhase::StreamStart,
            MACOS_NATIVE_START_TIMEOUT,
        )? {
            return Err(MacosCaptureError::CaptureWorkerStartFailed(
                "fixture request deadline was superseded before start".to_owned(),
            ));
        }
        if !self.start_candidate_fixture(stage) {
            return Err(MacosCaptureError::CaptureWorkerStartFailed(
                "fixture request candidate was superseded before start".to_owned(),
            ));
        }
        Ok((transaction, replaced))
    }

    fn current_stream(&self) -> Option<Retained<SCStream>> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|current| current.stream.clone())
    }

    fn stage_interrupted_recovery(
        self: &Arc<Self>,
        plan: InterruptedRestagePlan,
    ) -> Result<bool, CandidatePreparationFailure> {
        let lifecycle_revision = lock(&self.state).lifecycle_revision;
        let epoch = self
            .allocate_epoch()
            .map_err(|error| CandidatePreparationFailure {
                stage: CandidateStageIdentity {
                    epoch: plan.recovery.interrupted_epoch,
                    selection_revision: plan.recovery.selection_revision,
                    lifecycle_revision,
                    predecessor_epoch: None,
                },
                error,
                settlement: None,
            })?;
        self.stage_candidate_with_selection(
            Some(plan.selection_filter),
            plan.request,
            &plan.reserve_pool,
            epoch,
            Some(plan.recovery),
            None,
        )
    }

    fn begin_capture_activation(&self) -> Result<CaptureActivation, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if self.shared.capture_active() {
            return Ok(CaptureActivation::Unchanged);
        }
        let Some(selection_filter) = state.selected_filter.clone() else {
            self.shared.set_capture_active(true);
            return Ok(CaptureActivation::NeedsSelection);
        };
        let epoch = self.allocate_epoch()?;
        let request = state
            .pending_request
            .as_ref()
            .map_or(state.request, |pending| pending.request);
        let reservation =
            self.reserve_selection_candidate_locked(&mut state, epoch, request, selection_filter)?;
        self.shared.set_capture_active(true);
        Ok(CaptureActivation::Candidate {
            reservation: Box::new(reservation),
            request,
        })
    }

    fn set_capture_active(&self, active: bool) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        if self.shared.set_capture_active(active) == active {
            return false;
        }
        if active {
            return true;
        }
        self.shared.disable_picker_callbacks();
        let source_settlement = self.cancel_source_transaction_locked();
        let (current, candidate, selection, candidate_settlement, diagnostic_settlement) = {
            let mut state = lock(&self.state);
            let diagnostic_settlement = self
                .shared
                .claim_restart_diagnostic_completion(MacosProtectedSourceState::Failed);
            state.selection_revision = state.selection_revision.saturating_add(1);
            state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
            state.pending_interruption = None;
            let candidate_settlement = Self::cancel_candidate_completion(&mut state);
            state.staging_epoch = None;
            state.pending_request = None;
            state.candidate_epoch = None;
            state.inactive_epochs.clear();
            state.terminal_epochs.clear();
            #[cfg(test)]
            {
                state.fixture_candidate_epoch = None;
                state.fixture_current_epoch = None;
            }
            let mut selection = state.pending_selection.take().map(|pending| {
                let selection = pending.selection_filter.selection.clone();
                state.selected_filter = Some(pending.selection_filter);
                selection
            });
            if state.current.is_none()
                && state.selected_filter.is_none()
                && let Some(candidate) = state.candidate.as_ref()
            {
                let selection_filter = NativeSelectionFilter {
                    filter: candidate.filter.clone(),
                    selection: candidate.selection.clone(),
                    source_id: Arc::clone(&candidate.source_id),
                };
                selection = Some(selection_filter.selection.clone());
                state.selected_filter = Some(selection_filter);
            }
            (
                state.current.take(),
                state.candidate.take(),
                selection,
                candidate_settlement,
                diagnostic_settlement,
            )
        };
        self.shared.activate_epoch(0);
        self.shared.clear_tahoe_selection();
        if let Some(selection) = selection {
            self.shared.set_unconfirmed_selection(selection);
        }
        drop(_lifecycle);
        if let Some(candidate) = candidate {
            self.stop_stream(candidate);
        }
        if let Some(current) = current {
            self.stop_stream(current);
        }
        Self::finish_replaced_candidate(candidate_settlement);
        if let Some(settlement) = source_settlement {
            settlement.publish();
        }
        if let Some(settlement) = diagnostic_settlement {
            settlement.publish();
        }
        true
    }

    fn stop_stream(&self, stream: NativeStream) {
        let start_completion = stream.start_completion.clone();
        let shared = Arc::clone(&self.shared);
        let stop_shared = Arc::clone(&shared);
        let timeout_shared = Arc::clone(&shared);
        self.native_lifecycle.retire(
            stream,
            start_completion,
            Instant::now() + MACOS_NATIVE_STOP_TIMEOUT,
            move |stream, stop_completion| {
                stream.worker.close();
                let completion = RcBlock::new(move |error: *mut NSError| {
                    // SAFETY: ScreenCaptureKit supplies either null or a live
                    // NSError for the duration of this callback.
                    if let Some(error) = unsafe { error.as_ref() } {
                        stop_shared.record_retirement_error(&native_error(
                            "stop ScreenCaptureKit stream",
                            error,
                        ));
                    }
                    let _ = stop_completion.complete();
                });
                // SAFETY: ScreenCaptureKit copies the completion block. The
                // retirement registry retains the stream until it invokes or
                // destroys that block and the decode worker has retired.
                unsafe {
                    stream
                        .stream
                        .stopCaptureWithCompletionHandler(Some(&completion));
                }
                if let Err(error) = stream.finish_worker_retirement() {
                    shared.record_retirement_error(&error);
                }
            },
            move || {
                timeout_shared
                    .record_retirement_error(&MacosCaptureError::StreamStopCompletionLost);
            },
        );
    }

    fn retire_stream_after_native_error(&self, stream: NativeStream) {
        let start_completion = stream.start_completion.clone();
        let shared = Arc::clone(&self.shared);
        self.native_lifecycle
            .retire_without_native_stop(stream, start_completion, move |stream| {
                if let Err(error) = stream.finish_worker_retirement() {
                    shared.counters.record_drop(&error);
                }
            });
    }

    fn retire_unstarted_stream(&self, stream: NativeStream) {
        let start_completion = stream.start_completion.clone();
        drop(start_completion.witness());
        let shared = Arc::clone(&self.shared);
        self.native_lifecycle
            .retire_without_native_stop(stream, start_completion, move |stream| {
                if let Err(error) = stream.finish_worker_retirement() {
                    shared.counters.record_drop(&error);
                }
            });
    }
}

impl ScreenshotIdentityFence for StreamSlot {
    fn matches(&self, source_id: &str, generation: u64, selection_revision: u64) -> bool {
        let state = lock(&self.state);
        state.selection_revision == selection_revision
            && state.current.as_ref().is_some_and(|current| {
                current.epoch() == generation && current.source_id.as_ref() == source_id
            })
            && self
                .shared
                .tahoe_selection_for(source_id, generation)
                .is_some()
    }
}

fn start_stream(
    stream: &SCStream,
    epoch: u64,
    streams: Weak<StreamSlot>,
    shared: Arc<SessionShared>,
    start_completion: CompletionWitness,
) {
    let completion = RcBlock::new(move |error: *mut NSError| {
        let _ = start_completion.complete();
        // SAFETY: ScreenCaptureKit supplies either null or a live NSError for
        // the duration of this completion invocation.
        if let Some(error) = unsafe { error.as_ref() } {
            handle_stream_error(&streams, epoch, &shared, error);
        } else {
            dispatch_stream_start_success(&streams, epoch);
        }
    });
    // SAFETY: ScreenCaptureKit copies the heap block for asynchronous use, and
    // the stream remains retained by StreamSlot until activation or failure.
    unsafe { stream.startCaptureWithCompletionHandler(Some(&completion)) };
}

fn dispatch_stream_start_success(streams: &Weak<StreamSlot>, epoch: u64) {
    let Some(streams) = streams.upgrade() else {
        return;
    };
    match streams.arm_candidate_deadline(
        epoch,
        MacosNativeTransactionPhase::FirstCompleteFrame,
        MACOS_NATIVE_FIRST_FRAME_TIMEOUT,
    ) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            let shared = Arc::clone(&streams.shared);
            dispatch_owned_stream_error(
                streams,
                epoch,
                shared,
                MacosProtectedSourceState::Failed,
                error,
            );
            return;
        }
    }
    let callbacks = streams.lifecycle_callbacks.clone();
    callbacks.exec_async(move || streams.record_stream_start_success(epoch));
}

fn handle_stream_error(
    streams: &Weak<StreamSlot>,
    epoch: u64,
    shared: &Arc<SessionShared>,
    error: &NSError,
) {
    let Some(streams) = streams.upgrade() else {
        return;
    };
    let state = classify_stream_error(error);
    let error = native_error("ScreenCaptureKit stream", error);
    let shared = Arc::clone(shared);
    dispatch_owned_stream_error(streams, epoch, shared, state, error);
}

fn dispatch_owned_stream_error(
    streams: Arc<StreamSlot>,
    epoch: u64,
    shared: Arc<SessionShared>,
    state: MacosProtectedSourceState,
    error: MacosCaptureError,
) {
    streams.reject_epoch(epoch);
    let callbacks = streams.lifecycle_callbacks.clone();
    callbacks.exec_async(move || {
        handle_owned_stream_error(&streams, epoch, &shared, state, error);
    });
}

fn handle_owned_stream_error(
    streams: &Arc<StreamSlot>,
    epoch: u64,
    shared: &SessionShared,
    state: MacosProtectedSourceState,
    error: MacosCaptureError,
) {
    handle_owned_stream_error_with(streams, epoch, shared, state, error, || {});
}

fn handle_owned_stream_error_with(
    streams: &Arc<StreamSlot>,
    epoch: u64,
    shared: &SessionShared,
    state: MacosProtectedSourceState,
    error: MacosCaptureError,
    after_retirement: impl FnOnce(),
) {
    let mut removal = streams.remove(
        epoch,
        Some(MacosNativeTransactionError::Capture(error.clone())),
    );
    streams.clear_rejected_epoch(epoch);
    let state = if removal.role == StreamRole::Stale {
        state
    } else {
        shared.record_stream_diagnostic_result(epoch, state)
    };
    let role = removal.role;
    let selection_revision = removal.selection_revision;
    let recovery = (removal.role == StreamRole::Current
        && state == MacosProtectedSourceState::Interrupted)
        .then(|| {
            removal
                .stream
                .as_ref()
                .map(|stream| stream.interruption_restage(removal.selection_revision))
        })
        .flatten();
    if let Some(retired) = removal.stream {
        streams.retire_stream_after_native_error(retired);
    }
    after_retirement();
    if let Some(recovery) = recovery {
        let stream_error = error;
        match streams.stage_interrupted_recovery(recovery) {
            Ok(true) => shared.publish_recoverable_error(stream_error),
            Ok(false) => {
                if !shared.capture_active() || streams.has_newer_lifecycle(selection_revision) {
                    shared.publish_recoverable_error(stream_error);
                }
            }
            Err(stage_error) => {
                shared.counters.record_drop(&stream_error);
                streams.finalize_candidate_preparation_failure(stage_error, None);
            }
        }
        if let Some(settlement) = removal.request_settlement.take() {
            settlement.publish();
        }
        return;
    }
    streams.finalize_stream_error(role, selection_revision, state, error);
    if let Some(settlement) = removal.request_settlement.take() {
        settlement.publish();
    }
}

fn handle_fatal_stream_error(
    streams: &Weak<StreamSlot>,
    epoch: u64,
    shared: Arc<SessionShared>,
    error: MacosCaptureError,
) {
    shared.counters.record_drop(&error);
    let Some(streams) = streams.upgrade() else {
        return;
    };
    streams.reject_epoch(epoch);
    let callbacks = streams.lifecycle_callbacks.clone();
    callbacks.exec_async(move || {
        handle_owned_fatal_stream_error(&streams, epoch, shared, error);
    });
}

fn handle_owned_fatal_stream_error(
    streams: &Arc<StreamSlot>,
    epoch: u64,
    shared: Arc<SessionShared>,
    error: MacosCaptureError,
) {
    handle_owned_fatal_stream_error_with(streams, epoch, shared, error, || {});
}

fn handle_owned_fatal_stream_error_with(
    streams: &Arc<StreamSlot>,
    epoch: u64,
    shared: Arc<SessionShared>,
    error: MacosCaptureError,
    after_retirement: impl FnOnce(),
) {
    let mut removal = streams.remove(
        epoch,
        Some(MacosNativeTransactionError::Capture(error.clone())),
    );
    streams.clear_rejected_epoch(epoch);
    if removal.role != StreamRole::Stale {
        shared.record_stream_diagnostic_result(epoch, MacosProtectedSourceState::Failed);
    }
    let role = removal.role;
    let selection_revision = removal.selection_revision;
    if let Some(retired) = removal.stream {
        streams.stop_stream(retired);
    }
    after_retirement();
    streams.finalize_stream_error(
        role,
        selection_revision,
        MacosProtectedSourceState::Failed,
        error,
    );
    if let Some(settlement) = removal.request_settlement.take() {
        settlement.publish();
    }
}

pub struct MacosScreenCaptureSession {
    main: MainThreadBound<MainThreadSession>,
    shared: Arc<SessionShared>,
    streams: Arc<StreamSlot>,
    capabilities: MacosCaptureCapabilities,
}

impl MacosScreenCaptureSession {
    pub fn capabilities() -> Result<MacosCaptureCapabilities, MacosCaptureError> {
        native_capture_capabilities()
    }

    pub fn new(
        request: MacosStreamRequest,
        selector: MacosCaptureSelector,
    ) -> Result<Self, MacosCaptureError> {
        Self::new_with_pool_admission(request, selector, |_, _| {
            Ok(|_, _| Ok(Arc::new(()) as PoolBackingLifetime))
        })
    }

    pub fn new_with_pool_admission<F, A>(
        request: MacosStreamRequest,
        selector: MacosCaptureSelector,
        reserve_pool: F,
    ) -> Result<Self, MacosCaptureError>
    where
        F: Fn(u64, u64) -> Result<A, MacosCaptureError> + Send + Sync + 'static,
        A: Fn(u32, u64) -> Result<Arc<dyn Send + Sync>, MacosCaptureError> + Send + Sync + 'static,
    {
        request.cadence.timescale()?;
        let capabilities = native_capture_capabilities()?;
        capabilities.validate_dynamic_range(request.dynamic_range)?;
        let reserve_pool = Arc::new(move |surface_bytes, metadata_bytes| {
            let observer = reserve_pool(surface_bytes, metadata_bytes)?;
            Ok(Arc::new(observer) as PoolObservation)
        }) as PoolReservationFactory;
        dispatch2::run_on_main(move |mtm| {
            Self::new_on_main(request, selector, capabilities, reserve_pool, mtm)
        })
    }

    fn new_on_main(
        request: MacosStreamRequest,
        selector: MacosCaptureSelector,
        capabilities: MacosCaptureCapabilities,
        reserve_pool: PoolReservationFactory,
        mtm: MainThreadMarker,
    ) -> Result<Self, MacosCaptureError> {
        let authorized = CGPreflightScreenCaptureAccess();
        let status = if authorized {
            MacosProtectedSourceState::NeedsSelection
        } else {
            MacosProtectedSourceState::NeedsUserAction
        };
        let shared = Arc::new(SessionShared::new(status, selector, capabilities.tahoe));
        let observer = PickerObserver::new(mtm, request, Arc::clone(&shared), reserve_pool)?;
        let streams = Arc::clone(&observer.ivars().streams);
        // SAFETY: These are main-thread ScreenCaptureKit setup calls. The
        // observer remains retained by this session until it is removed.
        let picker = unsafe {
            let picker = SCContentSharingPicker::sharedPicker();
            let configuration: Retained<SCContentSharingPickerConfiguration> =
                SCContentSharingPickerConfiguration::new();
            configuration.setAllowedPickerModes(
                SCContentSharingPickerMode::SingleWindow
                    | SCContentSharingPickerMode::MultipleWindows
                    | SCContentSharingPickerMode::SingleApplication
                    | SCContentSharingPickerMode::MultipleApplications
                    | SCContentSharingPickerMode::SingleDisplay,
            );
            configuration.setAllowsChangingSelectedContent(true);
            let excluded_bundle_ids = NSArray::from_retained_slice(&[NSString::from_str(
                HYPERCOLOR_UI_BUNDLE_IDENTIFIER,
            )]);
            configuration.setExcludedBundleIDs(&excluded_bundle_ids);
            picker.setDefaultConfiguration(&configuration);
            picker.setMaximumStreamCount(Some(&NSNumber::new_i32(2)));
            let protocol: &ProtocolObject<dyn SCContentSharingPickerObserver> =
                ProtocolObject::from_ref(&*observer);
            picker.addObserver(protocol);
            picker.setActive(true);
            picker
        };
        let session = Self {
            main: MainThreadBound::new(MainThreadSession { picker, observer }, mtm),
            shared,
            streams,
            capabilities,
        };
        if authorized {
            session.resolve_configured_source()?;
        }
        Ok(session)
    }

    pub fn screen_authorized() -> bool {
        CGPreflightScreenCaptureAccess()
    }

    pub fn request_authorization(&self) -> MacosProtectedSourceState {
        if CGRequestScreenCaptureAccess() {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsSelection);
            if let Err(error) = self.resolve_configured_source() {
                self.shared.counters.record_drop(&error);
            }
        } else {
            self.shared
                .set_status(MacosProtectedSourceState::PermissionDenied);
        }
        self.shared.status()
    }

    pub fn present_picker(&self) -> Result<(), MacosCaptureError> {
        if !CGPreflightScreenCaptureAccess() {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsUserAction);
            return Err(MacosCaptureError::ScreenCapturePermissionRequired);
        }
        self.streams.begin_picker_resolution()?;
        self.main
            .get_on_main(|main| main.observer.present(&main.picker));
        Ok(())
    }

    pub fn status(&self) -> MacosProtectedSourceState {
        self.shared.status()
    }

    pub fn begin_post_authorization_stream_diagnostic(
        &self,
    ) -> Result<MacosStreamDiagnosticTransaction, MacosCaptureError> {
        if !CGPreflightScreenCaptureAccess() {
            return Err(MacosCaptureError::ScreenCapturePermissionRequired);
        }
        let (resolution, completion_rx) = self.streams.setup_restart_diagnostic(true)?;
        if let Err(error) = self.resolve_configured_source_with_resolution(
            SourceResolution::Diagnostic(resolution.clone()),
        ) {
            self.shared
                .fail_restart_diagnostic_attempt(resolution.attempt);
            self.streams.finalize_resolution_error(
                &SourceResolution::Diagnostic(resolution),
                false,
                error,
            );
        }
        Ok(completion_rx)
    }

    pub fn selection(&self) -> MacosCaptureSelection {
        self.shared.selection()
    }

    pub fn selection_revision(&self) -> u64 {
        self.streams.selection_revision()
    }

    pub fn tahoe_selection_capabilities(&self) -> Option<MacosTahoeSelectionCapabilities> {
        let (source_id, epoch) = self.streams.active_identity()?;
        self.shared.tahoe_selection_for(&source_id, epoch)
    }

    pub fn screenshot_reference_capability(
        &self,
    ) -> Result<MacosScreenshotReferenceCapability, MacosCaptureError> {
        self.streams.screenshot_capability()
    }

    pub fn capture_screenshot_reference<F>(&self, completion: F) -> Result<(), MacosCaptureError>
    where
        F: FnOnce(Result<MacosScreenshotReferenceSet, MacosCaptureError>) + Send + 'static,
    {
        self.capture_screenshot_reference_with_identity(move |result| {
            completion(result.map(MacosScreenshotReferenceCapture::into_references));
        })
    }

    pub fn capture_screenshot_reference_with_identity<F>(
        &self,
        completion: F,
    ) -> Result<(), MacosCaptureError>
    where
        F: FnOnce(Result<MacosScreenshotReferenceCapture, MacosCaptureError>) + Send + 'static,
    {
        let snapshot = self.streams.screenshot_snapshot()?;
        let source_id = Arc::clone(&snapshot.source_id);
        let generation = snapshot.generation;
        execute_screenshot_transaction(
            snapshot,
            Arc::clone(&self.streams) as Arc<dyn ScreenshotIdentityFence>,
            Arc::new(NativeScreenshotCaptureBackend),
            self.main
                .get_on_main(|main| main.observer.request().cursor_composed),
            Box::new(move |result| {
                completion(result.map(|references| {
                    MacosScreenshotReferenceCapture::new(source_id, generation, references)
                }));
            }),
        )
    }

    pub fn mailbox(&self) -> MacosFrameMailbox {
        self.shared.mailbox.clone()
    }

    pub fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        self.shared.diagnostics()
    }

    pub fn stop(&self) {
        self.set_capture_active(false);
    }

    pub fn set_capture_active(&self, active: bool) {
        self.main
            .get_on_main(|main| main.observer.set_active(active));
    }

    pub fn set_selector(&self, selector: MacosCaptureSelector) -> Result<(), MacosCaptureError> {
        if CGPreflightScreenCaptureAccess() {
            let resolution = self.streams.set_selector_and_begin_resolution(selector)?;
            self.resolve_configured_source_with_resolution(resolution)
        } else {
            self.streams.set_selector(selector);
            self.shared
                .set_status(MacosProtectedSourceState::NeedsUserAction);
            Ok(())
        }
    }

    pub fn set_stream_request(
        &self,
        request: MacosStreamRequest,
    ) -> Result<(), MacosNativeTransactionError> {
        self.begin_stream_request(request)?.wait()
    }

    pub fn begin_stream_request(
        &self,
        request: MacosStreamRequest,
    ) -> Result<MacosStreamRequestTransaction, MacosCaptureError> {
        request.cadence.timescale()?;
        self.capabilities
            .validate_dynamic_range(request.dynamic_range)?;
        self.main
            .get_on_main(|main| main.observer.set_request(request))
    }

    pub fn stream_request(&self) -> MacosStreamRequest {
        self.streams.committed_request()
    }

    fn resolve_configured_source(&self) -> Result<(), MacosCaptureError> {
        let resolution = self.streams.begin_resolution()?;
        self.resolve_configured_source_with_resolution(resolution)
    }

    fn resolve_configured_source_with_resolution(
        &self,
        resolution: SourceResolution,
    ) -> Result<(), MacosCaptureError> {
        let selector = resolution.selector().clone();
        if selector == MacosCaptureSelector::SessionScoped {
            let settlement = self.streams.claim_source_transaction(&resolution);
            self.streams.finalize_session_scoped_resolution(&resolution);
            if let Some(settlement) = settlement {
                settlement.publish();
            }
            return Ok(());
        }
        resolve_display_selector(
            Arc::clone(&self.streams),
            Arc::clone(&self.shared),
            self.main.get_on_main(|main| main.observer.request()),
            self.main
                .get_on_main(|main| Arc::clone(&main.observer.ivars().reserve_pool)),
            selector,
            resolution,
        )
    }
}

impl fmt::Debug for MacosScreenCaptureSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosScreenCaptureSession")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

fn stream_configuration(
    filter: &SCContentFilter,
    request: MacosStreamRequest,
) -> Result<
    (
        Retained<SCStreamConfiguration>,
        bool,
        MacosPixelExtent,
        MacosConfiguredStream,
    ),
    MacosCaptureError,
> {
    // SAFETY: Picker callbacks supply a live SCContentFilter for the duration
    // of configuration, and returned collection values are retained.
    let (content_rect, point_pixel_scale, display_filter) = unsafe {
        (
            filter.contentRect(),
            f64::from(filter.pointPixelScale()),
            !filter.includedDisplays().is_empty(),
        )
    };
    let point_rect = MacosPointRect::new(
        content_rect.origin.x,
        content_rect.origin.y,
        content_rect.size.width,
        content_rect.size.height,
    )?;
    let scale = MacosScale::display(point_pixel_scale)?;
    let pixel_rect = point_rect.to_pixel_rect(scale)?;
    let extent = MacosPixelExtent::new(pixel_rect.width, pixel_rect.height)?;
    let cadence_timescale = request.cadence.timescale()?;
    // SAFETY: Both constructors use a positive timescale. FramesPerSecond is
    // validated before conversion, while the native-refresh sentinel is zero
    // duration at the canonical unit timescale.
    let minimum_frame_interval = unsafe {
        cadence_timescale.map_or_else(|| CMTime::new(0, 1), |timescale| CMTime::new(1, timescale))
    };
    // SAFETY: The deployment floor includes the HDR preset API. Every setter
    // receives validated point or pixel units, and the caller retains the
    // configuration through stream creation.
    let configuration = unsafe {
        let configuration = match request.preset() {
            MacosStreamPreset::SdrDefault => SCStreamConfiguration::new(),
            MacosStreamPreset::CaptureHdrStreamCanonicalDisplay => {
                SCStreamConfiguration::streamConfigurationWithPreset(
                    SCStreamConfigurationPreset::CaptureHDRStreamCanonicalDisplay,
                )
            }
        };
        configuration.setCapturesAudio(false);
        configuration.setCaptureMicrophone(false);
        configuration.setCaptureResolution(SCCaptureResolutionType::Best);
        configuration.setWidth(extent.width as usize);
        configuration.setHeight(extent.height as usize);
        configuration.setSourceRect(content_rect);
        configuration.setDestinationRect(CGRect::new(
            CGPoint::ZERO,
            CGSize::new(f64::from(extent.width), f64::from(extent.height)),
        ));
        configuration.setPreservesAspectRatio(true);
        configuration.setScalesToFit(false);
        configuration.setMinimumFrameInterval(minimum_frame_interval);
        configuration.setShowsCursor(request.cursor_composed);
        configuration.setShowMouseClicks(false);
        configuration.setStreamName(Some(&NSString::from_str("Hypercolor")));
        configuration.setQueueDepth(MACOS_STREAM_QUEUE_DEPTH as isize);
        if request.dynamic_range == MacosCaptureDynamicRange::Sdr {
            configuration.setCaptureDynamicRange(SCCaptureDynamicRange::SDR);
            configuration.setPixelFormat(0x4247_5241);
        }
        configuration
    };
    // SAFETY: The retained configuration exposes scalar values initialized by
    // its constructor and the setters above.
    let configured_stream = unsafe {
        let pixel_format_fourcc = configuration.pixelFormat();
        MacosConfiguredStream {
            requested_dynamic_range: request.dynamic_range,
            requested_preset: request.preset(),
            configured_dynamic_range: capture_dynamic_range(configuration.captureDynamicRange())?,
            configured_pixel_format: MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?,
            configured_color_range: color_range_from_fourcc(pixel_format_fourcc),
        }
    };
    configured_stream.validate()?;
    Ok((configuration, display_filter, extent, configured_stream))
}

const fn color_range_from_fourcc(fourcc: u32) -> MacosColorRange {
    match fourcc {
        0x3432_3076 | 0x7834_3434 => MacosColorRange::Video,
        _ => MacosColorRange::Full,
    }
}

fn capture_dynamic_range(
    value: SCCaptureDynamicRange,
) -> Result<MacosCaptureDynamicRange, MacosCaptureError> {
    match value {
        SCCaptureDynamicRange::SDR => Ok(MacosCaptureDynamicRange::Sdr),
        SCCaptureDynamicRange::HDRLocalDisplay | SCCaptureDynamicRange::HDRCanonicalDisplay => {
            Ok(MacosCaptureDynamicRange::Hdr)
        }
        _ => Err(MacosCaptureError::UnsupportedConfiguredDynamicRange(
            value.0,
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MacosStreamPoolQuote {
    per_surface_bytes: u64,
    stream_metadata_bytes: u64,
}

fn conservative_pool_quote(
    extent: MacosPixelExtent,
    format: MacosCapturePixelFormat,
) -> Result<MacosStreamPoolQuote, MacosCaptureError> {
    let plane_bytes = match format {
        MacosCapturePixelFormat::Bgra8 | MacosCapturePixelFormat::Argb2101010 => {
            conservative_plane_bytes(extent, 4)?
        }
        MacosCapturePixelFormat::Rgba16Float => conservative_plane_bytes(extent, 8)?,
        MacosCapturePixelFormat::Yuv420VideoRange | MacosCapturePixelFormat::Yuv420FullRange => {
            let chroma = MacosPixelExtent {
                width: extent.width.div_ceil(2),
                height: extent.height.div_ceil(2),
            };
            conservative_plane_bytes(extent, 1)?
                .checked_add(conservative_plane_bytes(chroma, 2)?)
                .ok_or(MacosCaptureError::ArithmeticOverflow)?
        }
        MacosCapturePixelFormat::Yuv44410BiPlanar => conservative_plane_bytes(extent, 2)?
            .checked_add(conservative_plane_bytes(extent, 4)?)
            .ok_or(MacosCaptureError::ArithmeticOverflow)?,
    };
    let per_surface_bytes = align_up(plane_bytes, MACOS_IOSURFACE_ALLOCATION_ALIGNMENT)?;
    let stream_metadata_bytes = [
        std::mem::size_of::<NativeStream>(),
        std::mem::size_of::<CaptureOutputIvars>(),
        std::mem::size_of::<RetainedNativeSample>() * MACOS_STREAM_QUEUE_DEPTH,
    ]
    .into_iter()
    .try_fold(0_u64, |total, bytes| {
        total.checked_add(u64::try_from(bytes).ok()?)
    })
    .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    Ok(MacosStreamPoolQuote {
        per_surface_bytes,
        stream_metadata_bytes,
    })
}

fn conservative_plane_bytes(
    extent: MacosPixelExtent,
    bytes_per_pixel: u64,
) -> Result<u64, MacosCaptureError> {
    let row_bytes = u64::from(extent.width)
        .checked_mul(bytes_per_pixel)
        .ok_or(MacosCaptureError::ArithmeticOverflow)?;
    align_up(row_bytes, MACOS_IOSURFACE_ROW_ALIGNMENT)?
        .checked_mul(u64::from(extent.height))
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

fn align_up(value: u64, alignment: u64) -> Result<u64, MacosCaptureError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or(MacosCaptureError::ArithmeticOverflow)
}

fn classify_stream_error(error: &NSError) -> MacosProtectedSourceState {
    // SAFETY: ScreenCaptureKit and Foundation expose retained immutable error
    // domain strings for the lifetime of this callback.
    let is_stream_error = error
        .domain()
        .isEqualToString(unsafe { SCStreamErrorDomain });
    if !is_stream_error {
        return MacosProtectedSourceState::Failed;
    }
    match SCStreamErrorCode(error.code()) {
        SCStreamErrorCode::UserDeclined => MacosProtectedSourceState::PermissionDenied,
        SCStreamErrorCode::NoCaptureSource => MacosProtectedSourceState::NeedsSelection,
        SCStreamErrorCode::FailedApplicationConnectionInterrupted
        | SCStreamErrorCode::SystemStoppedStream => MacosProtectedSourceState::Interrupted,
        SCStreamErrorCode::UserStopped => MacosProtectedSourceState::ReadyIdle,
        _ => MacosProtectedSourceState::Failed,
    }
}

fn native_error(operation: &'static str, error: &NSError) -> MacosCaptureError {
    MacosCaptureError::NativeOperation {
        operation,
        code: error.code(),
        message: error.localizedDescription().to_string(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

include!("tests.rs");
