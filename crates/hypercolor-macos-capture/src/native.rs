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

mod lifecycle;
mod transactions;

use lifecycle::{CompletionFence, CompletionWitness, NativeLifecycle};
pub use transactions::{
    MacosNativeTransactionError, MacosNativeTransactionPhase, MacosStreamDiagnosticTransaction,
    MacosStreamRequestTransaction,
};
use transactions::{
    TransactionCompleter, TransactionIdentity, TransactionSettlement,
    stream_diagnostic_transaction, stream_request_transaction,
};

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

struct RetainedNativeSample {
    attachments: MacosRawFrameAttachments,
    pixel_buffer: CFRetained<CVPixelBuffer>,
    admission_lifetime: PoolBackingLifetime,
    cursor_composed: bool,
}

enum RetainedNativeDelivery<T = RetainedNativeSample> {
    Complete(T),
    Lifecycle(MacosFrameStatus),
}

fn route_retained_delivery<T>(
    delivery: RetainedNativeDelivery<T>,
    complete: impl FnOnce(T),
    lifecycle: impl FnOnce(MacosFrameStatus),
) {
    match delivery {
        RetainedNativeDelivery::Complete(sample) => complete(sample),
        RetainedNativeDelivery::Lifecycle(status) => lifecycle(status),
    }
}

struct DecodedSample {
    event: MacosFrameEvent,
    confirmed_delivery: Option<MacosValidatedStreamDelivery>,
}

// SAFETY: The retained Core Video pixel buffer is reference-counted and the
// decode worker only reads its immutable descriptor metadata.
unsafe impl Send for RetainedNativeSample {}

fn retain_sample(
    sample: &CMSampleBuffer,
    cursor_composed: bool,
    pool: &PoolObservation,
) -> Result<RetainedNativeDelivery, MacosCaptureError> {
    // SAFETY: ScreenCaptureKit supplied a live CMSampleBuffer reference for
    // the duration of this callback.
    if !unsafe { sample.is_valid() } {
        return Err(MacosCaptureError::InvalidSampleBuffer);
    }
    // SAFETY: The same callback lifetime makes the sample reference valid.
    if !unsafe { sample.data_is_ready() } {
        return Err(MacosCaptureError::SampleDataNotReady);
    }
    let attachments = FrameAttachments::from_sample(sample)?.decode();
    let status = match attachments.status.clone() {
        MacosAttachment::Value(status) => MacosFrameStatus::try_from(status)?,
        MacosAttachment::Missing => return Err(MacosCaptureError::MissingAttachment("status")),
        MacosAttachment::Malformed => {
            return Err(MacosCaptureError::MalformedAttachment("status"));
        }
    };
    if status != MacosFrameStatus::Complete {
        return Ok(RetainedNativeDelivery::Lifecycle(status));
    }
    let pixel_buffer = borrowed_pixel_buffer(sample)?;
    let storage_extent = extent(
        CVPixelBufferGetWidth(pixel_buffer),
        CVPixelBufferGetHeight(pixel_buffer),
    )?;
    let pixel_format_fourcc = CVPixelBufferGetPixelFormatType(pixel_buffer);
    let pixel_format = MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?;
    let planes = planes(pixel_buffer, storage_extent)?;
    let (iosurface_id, allocation_bytes) = borrowed_surface_identity(pixel_buffer)?;
    crate::frame::validate_capture_planes(storage_extent, pixel_format, planes, allocation_bytes)?;
    with_admitted_surface(pool, iosurface_id, allocation_bytes, |admission_lifetime| {
        // SAFETY: admission succeeded while the callback still owns the
        // borrowed image buffer, so this takes the retained owner handed off.
        let pixel_buffer = unsafe { CFRetained::retain(NonNull::from(pixel_buffer)) };
        RetainedNativeDelivery::Complete(RetainedNativeSample {
            attachments,
            pixel_buffer,
            admission_lifetime,
            cursor_composed,
        })
    })
}

fn with_admitted_surface<T>(
    pool: &PoolObservation,
    iosurface_id: u32,
    allocation_bytes: u64,
    retain: impl FnOnce(PoolBackingLifetime) -> T,
) -> Result<T, MacosCaptureError> {
    let admission_lifetime = pool(iosurface_id, allocation_bytes)?;
    Ok(retain(admission_lifetime))
}

fn borrowed_pixel_buffer(sample: &CMSampleBuffer) -> Result<&CVPixelBuffer, MacosCaptureError> {
    #[link(name = "CoreMedia", kind = "framework")]
    unsafe extern "C-unwind" {
        #[link_name = "CMSampleBufferGetImageBuffer"]
        fn sample_buffer_get_image_buffer(
            sample: &CMSampleBuffer,
        ) -> Option<NonNull<CVPixelBuffer>>;
    }

    // SAFETY: the sample is valid and ready, and ScreenCaptureKit keeps the
    // borrowed image buffer alive for this callback invocation.
    unsafe { sample_buffer_get_image_buffer(sample).map(|pixel_buffer| pixel_buffer.as_ref()) }
        .ok_or(MacosCaptureError::MissingFramePayload)
}

fn borrowed_surface_identity(
    pixel_buffer: &CVPixelBuffer,
) -> Result<(u32, u64), MacosCaptureError> {
    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C-unwind" {
        #[link_name = "CVPixelBufferGetIOSurface"]
        fn pixel_buffer_get_io_surface(
            pixel_buffer: Option<&CVPixelBuffer>,
        ) -> Option<NonNull<IOSurfaceRef>>;
    }

    // SAFETY: the borrowed pixel buffer remains live for this callback, and
    // Core Video returns its non-owning IOSurface reference.
    let surface =
        unsafe { pixel_buffer_get_io_surface(Some(pixel_buffer)).map(|surface| surface.as_ref()) }
            .ok_or(MacosCaptureError::MissingIoSurface)?;
    let iosurface_id = surface.id();
    let allocation_bytes =
        u64::try_from(surface.alloc_size()).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    if iosurface_id == 0 || allocation_bytes == 0 {
        return Err(MacosCaptureError::InvalidSurface);
    }
    Ok((iosurface_id, allocation_bytes))
}

fn publish_decoded_result(
    result: Result<DecodedSample, MacosCaptureError>,
    publication: SamplePublication,
    epoch: u64,
    streams: &Weak<StreamSlot>,
    shared: &Arc<SessionShared>,
) {
    let _timing = shared.counters.observe_publication();
    match result {
        Ok(sample) => {
            if let Some(streams) = streams.upgrade() {
                streams.publish_decoded_sample(epoch, sample, &publication);
            }
        }
        Err(error @ MacosCaptureError::StreamDeliveryRejected(_)) => {
            handle_fatal_stream_error(streams, epoch, Arc::clone(shared), error);
        }
        Err(error) => shared.counters.record_drop(&error),
    }
}

struct CaptureOutputIvars {
    samples: LatestSampleInput<RetainedNativeSample>,
    pool: PoolObservation,
    shared: Arc<SessionShared>,
    streams: Weak<StreamSlot>,
    epoch: u64,
    cursor_composed: bool,
    display_filter: bool,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HypercolorScreenCaptureOutput"]
    #[ivars = CaptureOutputIvars]
    struct CaptureOutput;

    unsafe impl NSObjectProtocol for CaptureOutput {}

    unsafe impl SCStreamOutput for CaptureOutput {
        #[allow(non_snake_case)]
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_didOutputSampleBuffer_ofType(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            let _callback_timing = self.ivars().shared.counters.observe_callback();
            self.ivars().shared.counters.record_received();
            if self
                .ivars()
                .streams
                .upgrade()
                .is_none_or(|streams| !streams.accepts_epoch(self.ivars().epoch))
            {
                return;
            }
            let delivery = if output_type == SCStreamOutputType::Screen {
                let _retain_timing = self.ivars().shared.counters.observe_retain();
                retain_sample(
                    sample_buffer,
                    self.ivars().cursor_composed,
                    &self.ivars().pool,
                )
            } else {
                Err(MacosCaptureError::UnexpectedStreamOutputType(output_type.0))
            };
            let delivery = match delivery {
                Err(error @ MacosCaptureError::ScreenResourceExhausted { .. }) => {
                    handle_fatal_stream_error(
                        &self.ivars().streams,
                        self.ivars().epoch,
                        Arc::clone(&self.ivars().shared),
                        error,
                    );
                    return;
                }
                Err(error) => {
                    self.ivars().shared.counters.record_drop(&error);
                    return;
                }
                Ok(delivery) => delivery,
            };
            route_retained_delivery(
                delivery,
                |sample| {
                    let _enqueue_timing = self.ivars().shared.counters.observe_enqueue();
                    if self.ivars().samples.publish(sample) == SamplePublishOutcome::Superseded {
                        self.ivars()
                            .shared
                            .counters
                            .record_native_sample_superseded();
                    }
                },
                |status| {
                    if let Some(streams) = self.ivars().streams.upgrade() {
                        route_stream_lifecycle(
                            &self.ivars().samples,
                            &streams,
                            self.ivars().epoch,
                            status,
                        );
                    }
                },
            );
        }
    }

    unsafe impl SCStreamDelegate for CaptureOutput {
        #[allow(non_snake_case)]
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_didStopWithError(&self, _stream: &SCStream, error: &NSError) {
            handle_stream_error(
                &self.ivars().streams,
                self.ivars().epoch,
                &self.ivars().shared,
                error,
            );
        }

        #[allow(non_snake_case)]
        #[unsafe(method(streamDidBecomeActive:))]
        fn streamDidBecomeActive(&self, _stream: &SCStream) {
            if let Some(streams) = self.ivars().streams.upgrade() {
                route_stream_activity(
                    &self.ivars().samples,
                    &streams,
                    self.ivars().epoch,
                    true,
                    self.ivars().display_filter,
                );
            }
        }

        #[allow(non_snake_case)]
        #[unsafe(method(streamDidBecomeInactive:))]
        fn streamDidBecomeInactive(&self, _stream: &SCStream) {
            if let Some(streams) = self.ivars().streams.upgrade() {
                route_stream_activity(
                    &self.ivars().samples,
                    &streams,
                    self.ivars().epoch,
                    false,
                    self.ivars().display_filter,
                );
            }
        }
    }
);

fn route_stream_lifecycle<T>(
    samples: &LatestSampleInput<T>,
    streams: &StreamSlot,
    epoch: u64,
    status: MacosFrameStatus,
) {
    if matches!(
        status,
        MacosFrameStatus::Suspended | MacosFrameStatus::Stopped
    ) {
        samples.invalidate_if(|| streams.publish_stream_lifecycle(epoch, status));
    } else {
        samples.synchronize_if(|| streams.publish_stream_lifecycle(epoch, status));
    }
}

fn route_stream_activity<T>(
    samples: &LatestSampleInput<T>,
    streams: &StreamSlot,
    epoch: u64,
    active: bool,
    display_filter: bool,
) {
    if active {
        samples.synchronize_if(|| streams.record_stream_activity(epoch, true, display_filter));
    } else {
        samples.invalidate_if(|| streams.record_stream_activity(epoch, false, display_filter));
    }
}

impl CaptureOutput {
    fn new(
        epoch: u64,
        samples: LatestSampleInput<RetainedNativeSample>,
        pool: PoolObservation,
        shared: Arc<SessionShared>,
        streams: Weak<StreamSlot>,
        cursor_composed: bool,
        display_filter: bool,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CaptureOutputIvars {
            samples,
            pool,
            shared,
            streams,
            epoch,
            cursor_composed,
            display_filter,
        });
        // SAFETY: NSObject has no additional initialization requirements for
        // this callback subclass.
        unsafe { msg_send![super(this), init] }
    }
}

#[derive(Clone)]
enum NativeFilter {
    System(Retained<SCContentFilter>),
    #[cfg(test)]
    Fixture(u64),
}

// SAFETY: SCContentFilter is immutable after picker delivery and remains in
// the process that owns every consuming SCStream. Rust never mutates it.
unsafe impl Send for NativeFilter {}

impl NativeFilter {
    fn system(&self) -> &SCContentFilter {
        match self {
            Self::System(filter) => filter,
            #[cfg(test)]
            Self::Fixture(_) => panic!("fixture selection has no native filter"),
        }
    }
}

#[derive(Clone)]
struct NativeSelectionFilter {
    filter: NativeFilter,
    selection: MacosCaptureSelection,
    source_id: Arc<str>,
}

impl NativeSelectionFilter {
    fn retain(filter: &SCContentFilter) -> Result<Self, MacosCaptureError> {
        let selection = selection_from_filter(filter)?;
        let source_id = selection_source_id(filter, &selection);
        // SAFETY: The picker or configured-source callback supplies a live
        // immutable filter, and each owner stays process-local.
        let filter = unsafe {
            Retained::retain(ptr::from_ref(filter).cast_mut())
                .ok_or(MacosCaptureError::RetainNativeFilterFailed)?
        };
        Ok(Self {
            filter: NativeFilter::System(filter),
            selection,
            source_id,
        })
    }

    #[cfg(test)]
    fn fixture(id: u64) -> Self {
        let source_id: Arc<str> = Arc::from(format!("fixture:{id}"));
        Self {
            filter: NativeFilter::Fixture(id),
            selection: MacosCaptureSelection::Display {
                source_id: Arc::clone(&source_id),
            },
            source_id,
        }
    }

    #[cfg(test)]
    fn fixture_id(&self) -> u64 {
        match &self.filter {
            NativeFilter::Fixture(id) => *id,
            NativeFilter::System(_) => panic!("native filter has no fixture identity"),
        }
    }
}

#[derive(Clone)]
enum ScreenshotFilterHandle {
    Native(NativeFilter),
    #[cfg(test)]
    Fixture(u64),
}

#[derive(Clone)]
struct ScreenshotTransactionSnapshot {
    filter: ScreenshotFilterHandle,
    source_id: Arc<str>,
    generation: u64,
    selection_revision: u64,
    capability: MacosScreenshotReferenceCapability,
}

type ScreenshotCompletion =
    Box<dyn FnOnce(Result<MacosScreenshotReferenceSet, MacosCaptureError>) + Send>;
type ScreenshotImageCompletion =
    Box<dyn FnOnce(Result<MacosScreenshotReferenceImage, MacosCaptureError>) + Send>;

trait ScreenshotCaptureBackend: Send + Sync {
    fn capture(
        &self,
        filter: ScreenshotFilterHandle,
        dynamic_range: MacosCaptureDynamicRange,
        cursor_composed: bool,
        completion: ScreenshotImageCompletion,
    ) -> Result<(), MacosCaptureError>;
}

trait ScreenshotIdentityFence: Send + Sync {
    fn matches(&self, source_id: &str, generation: u64, selection_revision: u64) -> bool;
}

struct NativeScreenshotCaptureBackend;

impl ScreenshotCaptureBackend for NativeScreenshotCaptureBackend {
    fn capture(
        &self,
        filter: ScreenshotFilterHandle,
        dynamic_range: MacosCaptureDynamicRange,
        cursor_composed: bool,
        completion: ScreenshotImageCompletion,
    ) -> Result<(), MacosCaptureError> {
        #[cfg(not(test))]
        let ScreenshotFilterHandle::Native(filter) = filter;
        #[cfg(test)]
        let filter = match filter {
            ScreenshotFilterHandle::Native(filter) => filter,
            ScreenshotFilterHandle::Fixture(_) => {
                return Err(MacosCaptureError::TahoePlatformDefect(
                    "native screenshot filter",
                ));
            }
        };
        let configuration_class = AnyClass::get(c"SCScreenshotConfiguration").ok_or(
            MacosCaptureError::TahoePlatformDefect("SCScreenshotConfiguration"),
        )?;
        let manager_class = AnyClass::get(c"SCScreenshotManager").ok_or(
            MacosCaptureError::TahoePlatformDefect("SCScreenshotManager"),
        )?;
        for (class, selector, capability) in [
            (
                configuration_class,
                sel!(setShowsCursor:),
                "SCScreenshotConfiguration.setShowsCursor",
            ),
            (
                configuration_class,
                sel!(setDisplayIntent:),
                "SCScreenshotConfiguration.setDisplayIntent",
            ),
            (
                configuration_class,
                sel!(setDynamicRange:),
                "SCScreenshotConfiguration.setDynamicRange",
            ),
        ] {
            if !class.responds_to(selector) {
                return Err(MacosCaptureError::TahoePlatformDefect(capability));
            }
        }
        if !manager_class.metaclass().responds_to(sel!(
            captureScreenshotWithFilter:configuration:completionHandler:
        )) {
            return Err(MacosCaptureError::TahoePlatformDefect(
                "SCScreenshotManager.captureScreenshot",
            ));
        }
        // SAFETY: the runtime probes above establish the Tahoe class and each
        // selector before the dynamically dispatched configuration calls.
        let configuration: Retained<AnyObject> = unsafe { msg_send![configuration_class, new] };
        let range_value = match dynamic_range {
            MacosCaptureDynamicRange::Sdr => 0_isize,
            MacosCaptureDynamicRange::Hdr => 1_isize,
        };
        // SAFETY: values match the SDK-declared BOOL and NSInteger properties.
        unsafe {
            let _: () = msg_send![&*configuration, setShowsCursor: cursor_composed];
            let _: () = msg_send![&*configuration, setDisplayIntent: 0_isize];
            let _: () = msg_send![&*configuration, setDynamicRange: range_value];
        }
        let completion = Arc::new(Mutex::new(Some(completion)));
        let completion_slot = Arc::clone(&completion);
        let retained_filter = filter.clone();
        let callback = RcBlock::new(move |output: *mut AnyObject, error: *mut NSError| {
            let Some(completion) = lock(&completion_slot).take() else {
                return;
            };
            // SAFETY: ScreenCaptureKit supplies callback objects for this
            // invocation. The selected CGImage is retained before return.
            let result = if let Some(error) = unsafe { error.as_ref() } {
                Err(native_error("capture Tahoe screenshot", error))
            } else if let Some(output) = unsafe { output.as_ref() } {
                // SAFETY: the live Objective-C output supports the NSObject
                // protocol query for its Tahoe image selector.
                unsafe {
                    let selector = match dynamic_range {
                        MacosCaptureDynamicRange::Sdr => sel!(sdrImage),
                        MacosCaptureDynamicRange::Hdr => sel!(hdrImage),
                    };
                    let responds: bool = msg_send![output, respondsToSelector: selector];
                    if !responds {
                        Err(MacosCaptureError::TahoePlatformDefect(
                            "SCScreenshotOutput image selector",
                        ))
                    } else {
                        let image: Option<Retained<CGImage>> = match dynamic_range {
                            MacosCaptureDynamicRange::Sdr => msg_send![output, sdrImage],
                            MacosCaptureDynamicRange::Hdr => msg_send![output, hdrImage],
                        };
                        image
                            .ok_or(MacosCaptureError::MissingScreenshotImage(dynamic_range))
                            .and_then(|image| {
                                MacosScreenshotReferenceImage::from_native(image, dynamic_range)
                            })
                    }
                }
            } else {
                Err(MacosCaptureError::TahoePlatformDefect("SCScreenshotOutput"))
            };
            drop(retained_filter.clone());
            completion(result);
        });
        // SAFETY: the runtime probe establishes this class selector. The API
        // copies the block and retains the filter and configuration while the
        // asynchronous capture is pending.
        unsafe {
            let _: () = msg_send![
                manager_class,
                captureScreenshotWithFilter: filter.system(),
                configuration: &*configuration,
                completionHandler: &*callback
            ];
        }
        Ok(())
    }
}

fn execute_screenshot_transaction(
    snapshot: ScreenshotTransactionSnapshot,
    fence: Arc<dyn ScreenshotIdentityFence>,
    backend: Arc<dyn ScreenshotCaptureBackend>,
    cursor_composed: bool,
    completion: ScreenshotCompletion,
) -> Result<(), MacosCaptureError> {
    if matches!(
        snapshot.capability,
        MacosScreenshotReferenceCapability::PendingFirstFrame
    ) {
        return Err(MacosCaptureError::ScreenshotCapabilityPending);
    }
    let completion = Arc::new(Mutex::new(Some(completion)));
    let first_filter = snapshot.filter.clone();
    let second_filter = snapshot.filter.clone();
    let first_source_id = Arc::clone(&snapshot.source_id);
    let first_fence = Arc::clone(&fence);
    let second_backend = Arc::clone(&backend);
    let capability = snapshot.capability.clone();
    let generation = snapshot.generation;
    let selection_revision = snapshot.selection_revision;
    let first_completion = Arc::clone(&completion);
    backend.capture(
        first_filter,
        MacosCaptureDynamicRange::Sdr,
        cursor_composed,
        Box::new(move |sdr| {
            if !first_fence.matches(&first_source_id, generation, selection_revision) {
                finish_screenshot(
                    &first_completion,
                    Err(MacosCaptureError::ScreenshotSelectionChanged),
                );
                return;
            }
            let sdr = match sdr {
                Ok(sdr) => sdr,
                Err(error) => {
                    finish_screenshot(&first_completion, Err(error));
                    return;
                }
            };
            match capability {
                MacosScreenshotReferenceCapability::PendingFirstFrame => {
                    finish_screenshot(
                        &first_completion,
                        Err(MacosCaptureError::ScreenshotCapabilityPending),
                    );
                }
                MacosScreenshotReferenceCapability::SdrOnly { .. } => {
                    finish_screenshot(
                        &first_completion,
                        Ok(MacosScreenshotReferenceSet::Sdr { image: sdr }),
                    );
                }
                MacosScreenshotReferenceCapability::PairedSdrHdr { .. } => {
                    let second_source_id = Arc::clone(&first_source_id);
                    let second_fence = Arc::clone(&first_fence);
                    let second_completion = Arc::clone(&first_completion);
                    let start_completion = Arc::clone(&first_completion);
                    let start = second_backend.capture(
                        second_filter,
                        MacosCaptureDynamicRange::Hdr,
                        cursor_composed,
                        Box::new(move |hdr| {
                            if !second_fence.matches(
                                &second_source_id,
                                generation,
                                selection_revision,
                            ) {
                                finish_screenshot(
                                    &second_completion,
                                    Err(MacosCaptureError::ScreenshotSelectionChanged),
                                );
                                return;
                            }
                            match hdr {
                                Ok(hdr) => finish_screenshot(
                                    &second_completion,
                                    Ok(MacosScreenshotReferenceSet::Paired { sdr, hdr }),
                                ),
                                Err(error) => finish_screenshot(&second_completion, Err(error)),
                            }
                        }),
                    );
                    if let Err(error) = start {
                        finish_screenshot(&start_completion, Err(error));
                    }
                }
            }
        }),
    )
}

fn finish_screenshot(
    completion: &Arc<Mutex<Option<ScreenshotCompletion>>>,
    result: Result<MacosScreenshotReferenceSet, MacosCaptureError>,
) {
    if let Some(completion) = lock(completion).take() {
        completion(result);
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

struct PickerObserverIvars {
    shared: Arc<SessionShared>,
    streams: Arc<StreamSlot>,
    reserve_pool: PoolReservationFactory,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HypercolorContentSharingPickerObserver"]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerObserverIvars]
    struct PickerObserver;

    unsafe impl NSObjectProtocol for PickerObserver {}

    unsafe impl SCContentSharingPickerObserver for PickerObserver {
        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPicker:didCancelForStream:))]
        fn contentSharingPicker_didCancelForStream(
            &self,
            _picker: &SCContentSharingPicker,
            _stream: Option<&SCStream>,
        ) {
            let Some(resolution) = self.ivars().shared.picker_resolution() else {
                return;
            };
            let settlement = self.ivars().streams.cancel_source_transaction(&resolution);
            self.ivars().streams.finalize_picker_cancel(&resolution);
            if let Some(settlement) = settlement {
                settlement.publish();
            }
        }

        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPicker:didUpdateWithFilter:forStream:))]
        fn contentSharingPicker_didUpdateWithFilter_forStream(
            &self,
            _picker: &SCContentSharingPicker,
            filter: &SCContentFilter,
            _stream: Option<&SCStream>,
        ) {
            let Some(resolution) = self.ivars().shared.picker_resolution() else {
                return;
            };
            let settlement = self.ivars().streams.claim_source_transaction(&resolution);
            accept_filter(
                &self.ivars().streams,
                &self.ivars().shared,
                self.ivars().streams.request(),
                &self.ivars().reserve_pool,
                filter,
                true,
                ClaimedSourceResolution {
                    resolution,
                    settlement,
                },
            );
        }

        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
        fn contentSharingPickerStartDidFailWithError(&self, error: &NSError) {
            let Some(resolution) = self.ivars().shared.picker_resolution() else {
                return;
            };
            let settlement = self.ivars().streams.claim_source_transaction(&resolution);
            let error = native_error("ScreenCaptureKit picker", error);
            self.ivars()
                .streams
                .finalize_picker_failure(&resolution, error);
            if let Some(settlement) = settlement {
                settlement.publish();
            }
        }
    }
);

impl PickerObserver {
    fn new(
        mtm: MainThreadMarker,
        request: MacosStreamRequest,
        shared: Arc<SessionShared>,
        reserve_pool: PoolReservationFactory,
    ) -> Result<Retained<Self>, MacosCaptureError> {
        let streams = StreamSlot::new(Arc::clone(&shared), request)?;
        let this = mtm.alloc::<Self>().set_ivars(PickerObserverIvars {
            shared,
            streams,
            reserve_pool,
        });
        // SAFETY: NSObject has no additional initialization requirements for
        // this main-thread observer subclass.
        Ok(unsafe { msg_send![super(this), init] })
    }

    fn request(&self) -> MacosStreamRequest {
        self.ivars().streams.request()
    }

    fn set_request(
        &self,
        request: MacosStreamRequest,
    ) -> Result<MacosStreamRequestTransaction, MacosCaptureError> {
        self.ivars()
            .streams
            .set_request(request, &self.ivars().reserve_pool)
    }

    fn present(&self, picker: &SCContentSharingPicker) {
        if let Some(stream) = self.ivars().streams.current_stream() {
            // SAFETY: The stream is owned by this observer for the duration of
            // picker presentation.
            unsafe { picker.presentPickerForStream(&stream) };
        } else {
            // SAFETY: The public session action is an explicit local request
            // to present Apple's system picker.
            unsafe { picker.present() };
        }
    }

    fn set_active(&self, active: bool) {
        if !active {
            if !self.ivars().streams.set_capture_active(false) {
                return;
            }
            let status = if self.ivars().streams.has_selection() {
                MacosProtectedSourceState::ReadyIdle
            } else {
                MacosProtectedSourceState::NeedsSelection
            };
            self.ivars().shared.set_status(status);
            return;
        }
        match self.ivars().streams.begin_capture_activation() {
            Ok(CaptureActivation::Unchanged) => {}
            Ok(CaptureActivation::NeedsSelection) => self
                .ivars()
                .shared
                .set_status(MacosProtectedSourceState::NeedsSelection),
            Ok(CaptureActivation::Candidate {
                reservation,
                request,
            }) => {
                if let Err(failure) = self.ivars().streams.prepare_and_start_candidate(
                    *reservation,
                    request,
                    &self.ivars().reserve_pool,
                ) {
                    self.ivars()
                        .streams
                        .finalize_candidate_preparation_failure(failure, None);
                }
            }
            Err(error) => self.ivars().shared.counters.record_drop(&error),
        }
    }

    fn stop(&self) {
        self.ivars().streams.set_capture_active(false);
    }
}

fn accept_filter(
    streams: &Arc<StreamSlot>,
    shared: &Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: &PoolReservationFactory,
    filter: &SCContentFilter,
    picker: bool,
    claimed: ClaimedSourceResolution,
) {
    let ClaimedSourceResolution {
        resolution,
        settlement,
    } = claimed;
    let commit = || {
        let diagnostic = matches!(resolution, SourceResolution::Diagnostic(_));
        let selection_filter = match NativeSelectionFilter::retain(filter) {
            Ok(selection_filter) => selection_filter,
            Err(error) => {
                streams.finalize_resolution_error(&resolution, picker, error);
                return;
            }
        };
        let epoch = match streams.allocate_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                streams.finalize_resolution_error(&resolution, picker, error);
                return;
            }
        };
        match streams.accept_selection_filter(
            selection_filter,
            request,
            epoch,
            resolution.clone(),
            picker,
        ) {
            Ok(FilterAcceptance::Stale) => {}
            Ok(FilterAcceptance::Stored(replaced)) => {
                if let Some(replaced) = replaced {
                    streams.stop_stream(replaced);
                }
                if diagnostic {
                    shared
                        .record_stream_diagnostic_result(epoch, MacosProtectedSourceState::Failed);
                }
            }
            Ok(FilterAcceptance::Candidate {
                reservation,
                request,
            }) => match streams.prepare_and_start_candidate(*reservation, request, reserve_pool) {
                Ok(true) => {}
                Ok(false) => {
                    shared
                        .record_stream_diagnostic_result(epoch, MacosProtectedSourceState::Failed);
                }
                Err(failure) => {
                    streams.finalize_candidate_preparation_failure(failure, Some(&resolution));
                }
            },
            Err(error) => {
                streams.finalize_resolution_error(&resolution, false, error);
            }
        }
    };
    commit();
    if let Some(settlement) = settlement {
        settlement.publish();
    }
}

struct MainThreadSession {
    picker: Retained<SCContentSharingPicker>,
    observer: Retained<PickerObserver>,
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

fn resolve_display_selector(
    streams: Arc<StreamSlot>,
    shared: Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: PoolReservationFactory,
    selector: MacosCaptureSelector,
    resolution: SourceResolution,
) -> Result<(), MacosCaptureError> {
    let completion = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            if !source_resolution_is_current(&streams, &shared, &resolution) {
                return;
            }
            let settlement = streams.claim_source_transaction(&resolution);
            // SAFETY: ScreenCaptureKit supplies callback objects for the
            // duration of this invocation. Derived owners are retained before
            // the callback returns.
            let result = unsafe {
                if let Some(error) = error.as_ref() {
                    Err(native_error("enumerate ScreenCaptureKit content", error))
                } else {
                    content
                        .as_ref()
                        .ok_or(MacosCaptureError::MissingShareableContent)
                        .and_then(|content| display_filter(content, &selector))
                }
            };
            if !source_resolution_is_current(&streams, &shared, &resolution) {
                if let Some(settlement) = settlement {
                    settlement.publish();
                }
                return;
            }
            match result {
                Ok(filter) => {
                    accept_filter(
                        &streams,
                        &shared,
                        request,
                        &reserve_pool,
                        &filter,
                        false,
                        ClaimedSourceResolution {
                            resolution: resolution.clone(),
                            settlement,
                        },
                    );
                }
                Err(error) => {
                    streams.finalize_resolution_error(&resolution, false, error);
                    if let Some(settlement) = settlement {
                        settlement.publish();
                    }
                }
            }
        },
    );
    // SAFETY: ScreenCaptureKit copies the completion block for asynchronous
    // use. The block owns every Rust value captured by the callback.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&completion) };
    Ok(())
}

fn source_resolution_is_current(
    streams: &StreamSlot,
    shared: &SessionShared,
    resolution: &SourceResolution,
) -> bool {
    shared.source_resolution_is_current(resolution)
        && match resolution {
            SourceResolution::General(_) => true,
            SourceResolution::Diagnostic(diagnostic) => {
                streams.selection_revision() == diagnostic.attempt.selection_revision
            }
        }
}

fn display_filter(
    content: &SCShareableContent,
    selector: &MacosCaptureSelector,
) -> Result<Retained<SCContentFilter>, MacosCaptureError> {
    // SAFETY: Shareable content owns an immutable display snapshot. The
    // returned array and each selected display are retained locally.
    let displays = unsafe { content.displays() };
    // SAFETY: The same immutable shareable-content snapshot retains its
    // returned window array and each member.
    let excluded_windows = unsafe { content.windows() }
        .to_vec()
        .into_iter()
        .filter(|window| {
            // SAFETY: Every retained SCWindow and owning application belongs
            // to this immutable shareable-content snapshot.
            unsafe {
                window.owningApplication().is_some_and(|application| {
                    is_hypercolor_ui_bundle_identifier(&application.bundleIdentifier().to_string())
                })
            }
        })
        .collect::<Vec<_>>();
    let excluded = NSArray::<SCWindow>::from_retained_slice(&excluded_windows);
    let primary_display = CGMainDisplayID();
    let mut primary_uuid_error = None;
    for display in displays.to_vec() {
        // SAFETY: The retained SCDisplay remains live for this query.
        let display_id = unsafe { display.displayID() };
        let source_id = match display_source_id(display_id) {
            Ok(source_id) => source_id,
            Err(error) if display_id == primary_display => {
                primary_uuid_error = Some(error);
                continue;
            }
            Err(_) => continue,
        };
        if selector.matches_display(&source_id, display_id == primary_display) {
            // SAFETY: The filter retains the selected display and the stable
            // Hypercolor window identities from this content snapshot.
            return Ok(unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            });
        }
    }
    if matches!(
        selector,
        MacosCaptureSelector::Auto | MacosCaptureSelector::PrimaryDisplay
    ) && let Some(error) = primary_uuid_error
    {
        return Err(error);
    }
    Err(MacosCaptureError::DisplaySourceUnavailable(
        selector.configured_source().to_owned(),
    ))
}

fn selection_from_filter(
    filter: &SCContentFilter,
) -> Result<MacosCaptureSelection, MacosCaptureError> {
    // SAFETY: Picker-delivered filters are immutable and retain every array
    // member for the duration of this metadata query.
    unsafe {
        let displays = filter.includedDisplays();
        let windows = filter.includedWindows();
        let applications = filter.includedApplications();
        if displays.is_empty() && windows.is_empty() && applications.is_empty() {
            return Ok(MacosCaptureSelection::None);
        }
        if windows.is_empty() && applications.is_empty() && displays.len() == 1 {
            let display = displays
                .firstObject()
                .ok_or(MacosCaptureError::DisplayUuidUnavailable(0))?;
            let display_id = display.displayID();
            let source_id = display_source_id(display_id)?;
            return Ok(MacosCaptureSelection::Display {
                source_id: Arc::from(source_id),
            });
        }
        let content_style = if !windows.is_empty() && !applications.is_empty() {
            MacosCaptureContentStyle::Mixed
        } else if windows.len() > 1 {
            MacosCaptureContentStyle::MultipleWindows
        } else if !windows.is_empty() {
            MacosCaptureContentStyle::Window
        } else if applications.len() > 1 {
            MacosCaptureContentStyle::MultipleApplications
        } else {
            MacosCaptureContentStyle::Application
        };
        Ok(MacosCaptureSelection::SessionScoped { content_style })
    }
}

fn selection_source_id(filter: &SCContentFilter, selection: &MacosCaptureSelection) -> Arc<str> {
    match selection {
        MacosCaptureSelection::Display { source_id } => Arc::clone(source_id),
        MacosCaptureSelection::SessionScoped { content_style } => {
            // SAFETY: The retained filter owns immutable selected-content
            // arrays and their members for the duration of this query.
            let (window_ids, application_ids) = unsafe {
                (
                    filter
                        .includedWindows()
                        .to_vec()
                        .into_iter()
                        .map(|window| window.windowID())
                        .collect::<Vec<_>>(),
                    filter
                        .includedApplications()
                        .to_vec()
                        .into_iter()
                        .map(|application| application.bundleIdentifier().to_string())
                        .collect::<Vec<_>>(),
                )
            };
            session_selection_source_id(*content_style, window_ids, application_ids)
        }
        MacosCaptureSelection::None => Arc::from("macos:session"),
    }
}

fn session_selection_source_id(
    content_style: MacosCaptureContentStyle,
    mut window_ids: Vec<u32>,
    mut application_ids: Vec<String>,
) -> Arc<str> {
    window_ids.sort_unstable();
    window_ids.dedup();
    application_ids.sort_unstable();
    application_ids.dedup();
    let mut source_id = format!("macos:session:{}", content_style_name(content_style));
    for window_id in window_ids {
        source_id.push_str(&format!(":w{window_id}"));
    }
    for application_id in application_ids {
        source_id.push_str(&format!(":a{}:{application_id}", application_id.len()));
    }
    Arc::from(source_id)
}

const fn content_style_name(content_style: MacosCaptureContentStyle) -> &'static str {
    match content_style {
        MacosCaptureContentStyle::Window => "window",
        MacosCaptureContentStyle::MultipleWindows => "multiple-windows",
        MacosCaptureContentStyle::Application => "application",
        MacosCaptureContentStyle::MultipleApplications => "multiple-applications",
        MacosCaptureContentStyle::Mixed => "mixed",
    }
}

fn display_source_id(display_id: CGDirectDisplayID) -> Result<String, MacosCaptureError> {
    let uuid =
        display_uuid(display_id).ok_or(MacosCaptureError::DisplayUuidUnavailable(display_id))?;
    let uuid = CFUUID::new_string(None, Some(&uuid))
        .ok_or(MacosCaptureError::DisplayUuidUnavailable(display_id))?
        .to_string()
        .to_ascii_lowercase();
    Ok(format!("display:{uuid}"))
}

fn display_uuid(display_id: CGDirectDisplayID) -> Option<CFRetained<CFUUID>> {
    #[link(name = "ColorSync", kind = "framework")]
    unsafe extern "C-unwind" {
        fn CGDisplayCreateUUIDFromDisplayID(display: CGDirectDisplayID) -> Option<NonNull<CFUUID>>;
    }
    // SAFETY: Core Graphics returns a nullable create-rule CFUUID reference.
    // CFRetained assumes the owning +1 reference and balances it on drop.
    unsafe { CGDisplayCreateUUIDFromDisplayID(display_id).map(|uuid| CFRetained::from_raw(uuid)) }
}

impl fmt::Debug for MacosScreenCaptureSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosScreenCaptureSession")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Drop for MainThreadSession {
    fn drop(&mut self) {
        self.observer.stop();
        // SAFETY: MainThreadBound runs this destructor on the main thread, and
        // the observer remains retained through its removal.
        unsafe {
            let protocol: &ProtocolObject<dyn SCContentSharingPickerObserver> =
                ProtocolObject::from_ref(&*self.observer);
            self.picker.removeObserver(protocol);
            self.picker.setActive(false);
        }
    }
}

fn native_capture_capabilities() -> Result<MacosCaptureCapabilities, MacosCaptureError> {
    let screenshot_configuration = AnyClass::get(c"SCScreenshotConfiguration");
    let screenshot_manager = AnyClass::get(c"SCScreenshotManager");
    let probes = MacosTahoeRuntimeProbes {
        content_tone_mapping_info_symbol: capability(
            crate::screenshot::tahoe_reference_output_symbols_present(),
        ),
        screenshot_configuration_class: capability(screenshot_configuration.is_some()),
        screenshot_dynamic_range_selector: capability(
            screenshot_configuration.is_some_and(|class| class.responds_to(sel!(setDynamicRange:))),
        ),
        screenshot_capture_selector: capability(screenshot_manager.is_some_and(|class| {
            class.metaclass().responds_to(sel!(
                captureScreenshotWithFilter:configuration:completionHandler:
            ))
        })),
    };
    capture_capabilities_from_probes(
        sysctl_i32(c"hw.optional.arm64", "hw.optional.arm64"),
        sysctl_i32(c"sysctl.proc_translated", "sysctl.proc_translated"),
        probes,
    )
}

const fn capability(present: bool) -> MacosRuntimeCapability {
    if present {
        MacosRuntimeCapability::Present
    } else {
        MacosRuntimeCapability::Absent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SysctlI32Value {
    Present(i32),
    Missing,
}

fn capture_capabilities_from_probes(
    arm64: Result<SysctlI32Value, MacosCaptureError>,
    translated: Result<SysctlI32Value, MacosCaptureError>,
    tahoe: MacosTahoeRuntimeProbes,
) -> Result<MacosCaptureCapabilities, MacosCaptureError> {
    let arm64 = arm64?;
    let translated_process = matches!(translated?, SysctlI32Value::Present(1));
    let host_architecture = if matches!(arm64, SysctlI32Value::Present(1)) || translated_process {
        MacosHostArchitecture::AppleSilicon
    } else {
        MacosHostArchitecture::Intel
    };
    Ok(MacosCaptureCapabilities::from_runtime(
        host_architecture,
        translated_process,
        tahoe,
    ))
}

fn sysctl_i32(name: &CStr, failure: &'static str) -> Result<SysctlI32Value, MacosCaptureError> {
    #[link(name = "System", kind = "dylib")]
    unsafe extern "C-unwind" {
        fn sysctlbyname(
            name: *const c_char,
            old_value: *mut c_void,
            old_length: *mut usize,
            new_value: *mut c_void,
            new_length: usize,
        ) -> i32;
    }

    let mut value = 0_i32;
    let mut length = std::mem::size_of::<i32>();
    // SAFETY: Both output pointers reference initialized writable storage, the
    // name is nul-terminated, and this query performs no mutation.
    let status = unsafe {
        sysctlbyname(
            name.as_ptr(),
            ptr::from_mut(&mut value).cast(),
            &mut length,
            ptr::null_mut(),
            0,
        )
    };
    if status == 0 && length == std::mem::size_of::<i32>() {
        Ok(SysctlI32Value::Present(value))
    } else if status != 0 {
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            Ok(SysctlI32Value::Missing)
        } else {
            Err(MacosCaptureError::CapabilityProbeFailed(failure))
        }
    } else {
        Err(MacosCaptureError::CapabilityProbeFailed("sysctl size"))
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

fn decode_sample(
    decoder: &mut MacosFrameDecoder,
    delivery_validator: &mut MacosStreamDeliveryValidator,
    sample: RetainedNativeSample,
) -> Result<DecodedSample, MacosCaptureError> {
    let awaiting_first_delivery = matches!(
        delivery_validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    );
    let frame = decode_complete_frame(
        sample.pixel_buffer,
        Some(sample.admission_lifetime),
        sample.cursor_composed,
    )
    .map_err(|error| classify_delivery_error(delivery_validator, error))?;
    let event = decoder
        .decode(MacosRawCaptureSample {
            frame: Some(frame),
            attachments: sample.attachments,
        })
        .map_err(|error| classify_delivery_error(delivery_validator, error))?;
    let confirmed_delivery = if awaiting_first_delivery {
        let MacosFrameEvent::Frame(frame) = &event else {
            return Err(classify_delivery_error(
                delivery_validator,
                MacosCaptureError::MissingFramePayload,
            ));
        };
        Some(
            delivery_validator
                .observe_first_complete(frame.surface.delivery_metadata())
                .map_err(MacosCaptureError::StreamDeliveryRejected)?,
        )
    } else {
        None
    };
    Ok(DecodedSample {
        event,
        confirmed_delivery,
    })
}

fn classify_delivery_error(
    validator: &mut MacosStreamDeliveryValidator,
    error: MacosCaptureError,
) -> MacosCaptureError {
    if matches!(
        validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    ) {
        return reject_first_delivery(validator, error);
    }
    match error {
        MacosCaptureError::StreamDeliveryRejected(rejection) => {
            MacosCaptureError::FrameDeliveryDropped(rejection)
        }
        error => error,
    }
}

fn reject_first_delivery(
    validator: &mut MacosStreamDeliveryValidator,
    error: MacosCaptureError,
) -> MacosCaptureError {
    if !matches!(
        validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    ) {
        return error;
    }
    let rejection = match &error {
        MacosCaptureError::MissingFramePayload => {
            Some(MacosStreamDeliveryRejection::MissingFirstCompleteFrame)
        }
        MacosCaptureError::UnsupportedPixelFormat(_) => {
            Some(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("pixel_format"))
        }
        MacosCaptureError::MissingColorAttachment(field)
        | MacosCaptureError::UnsupportedColorAttachment(field)
        | MacosCaptureError::MalformedLuminanceAttachment(field) => {
            Some(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata(field))
        }
        MacosCaptureError::ColorMetadataMismatch | MacosCaptureError::MissingYuvColorMetadata => {
            Some(MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("colorimetry"))
        }
        MacosCaptureError::StreamDeliveryRejected(rejection) => Some(*rejection),
        _ => None,
    };
    rejection.map_or(error, |rejection| {
        validator.reject_delivery(rejection);
        MacosCaptureError::StreamDeliveryRejected(rejection)
    })
}

fn decode_complete_frame(
    pixel_buffer: CFRetained<CVPixelBuffer>,
    admission_lifetime: Option<PoolBackingLifetime>,
    cursor_composed: bool,
) -> Result<MacosRawCompleteFrame, MacosCaptureError> {
    let storage_extent = extent(
        CVPixelBufferGetWidth(&pixel_buffer),
        CVPixelBufferGetHeight(&pixel_buffer),
    )?;
    let pixel_format_fourcc = CVPixelBufferGetPixelFormatType(&pixel_buffer);
    let pixel_format = MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?;
    let planes = planes(&pixel_buffer, storage_extent)?;
    let color = colorimetry(&pixel_buffer, pixel_format_fourcc, pixel_format)?;
    let (source_reference_white_nits, content_headroom) = hdr_luminance_metadata(&pixel_buffer)?;
    let delivery_metadata = MacosDeliveredFrameMetadata::new(
        pixel_format,
        color,
        source_reference_white_nits,
        content_headroom,
    )?;
    let surface = MacosCaptureSurface::from_pixel_buffer_with_delivery_metadata(
        pixel_buffer,
        admission_lifetime,
        Some(delivery_metadata),
    )?;

    Ok(MacosRawCompleteFrame {
        storage_extent,
        planes,
        pixel_format_fourcc,
        color,
        cursor_composed,
        surface,
    })
}

fn planes(
    pixel_buffer: &CVPixelBuffer,
    storage_extent: MacosPixelExtent,
) -> Result<Vec<MacosRawCapturePlane>, MacosCaptureError> {
    let plane_count = CVPixelBufferGetPlaneCount(pixel_buffer);
    if plane_count == 0 {
        return Ok(vec![MacosRawCapturePlane {
            index: 0,
            extent: storage_extent,
            bytes_per_row: CVPixelBufferGetBytesPerRow(pixel_buffer),
            length_bytes: u64::try_from(CVPixelBufferGetDataSize(pixel_buffer))
                .map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
        }]);
    }

    (0..plane_count)
        .map(|index| {
            let extent = extent(
                CVPixelBufferGetWidthOfPlane(pixel_buffer, index),
                CVPixelBufferGetHeightOfPlane(pixel_buffer, index),
            )?;
            let bytes_per_row = CVPixelBufferGetBytesPerRowOfPlane(pixel_buffer, index);
            let length_bytes = u64::try_from(bytes_per_row)
                .ok()
                .and_then(|stride| stride.checked_mul(u64::from(extent.height)))
                .ok_or(MacosCaptureError::ArithmeticOverflow)?;
            Ok(MacosRawCapturePlane {
                index: u32::try_from(index).map_err(|_| MacosCaptureError::ArithmeticOverflow)?,
                extent,
                bytes_per_row,
                length_bytes,
            })
        })
        .collect()
}

fn extent(width: usize, height: usize) -> Result<MacosPixelExtent, MacosCaptureError> {
    let width = u32::try_from(width).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    let height = u32::try_from(height).map_err(|_| MacosCaptureError::ArithmeticOverflow)?;
    Ok(MacosPixelExtent::new(width, height)?)
}

fn colorimetry(
    pixel_buffer: &CVBuffer,
    fourcc: u32,
    format: MacosCapturePixelFormat,
) -> Result<MacosCaptureColorimetry, MacosCaptureError> {
    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (primaries_key, rec709, display_p3, rec2020) = unsafe {
        (
            kCVImageBufferColorPrimariesKey,
            kCVImageBufferColorPrimaries_ITU_R_709_2,
            kCVImageBufferColorPrimaries_P3_D65,
            kCVImageBufferColorPrimaries_ITU_R_2020,
        )
    };
    let primaries_value = color_attachment(pixel_buffer, primaries_key, "color_primaries")?;
    let primaries = match &*primaries_value {
        value if value == rec709 => MacosColorPrimaries::Srgb,
        value if value == display_p3 => MacosColorPrimaries::DisplayP3,
        value if value == rec2020 => MacosColorPrimaries::Rec2020,
        _ => {
            return Err(MacosCaptureError::UnsupportedColorAttachment(
                "color_primaries",
            ));
        }
    };

    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (transfer_key, srgb, rec709, rec2020, linear, pq, hlg) = unsafe {
        (
            kCVImageBufferTransferFunctionKey,
            kCVImageBufferTransferFunction_sRGB,
            kCVImageBufferTransferFunction_ITU_R_709_2,
            kCVImageBufferTransferFunction_ITU_R_2020,
            kCVImageBufferTransferFunction_Linear,
            kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ,
            kCVImageBufferTransferFunction_ITU_R_2100_HLG,
        )
    };
    let transfer_value = color_attachment(pixel_buffer, transfer_key, "transfer_function")?;
    let transfer = match &*transfer_value {
        value if value == srgb => MacosTransferFunction::Srgb,
        value if value == rec709 => MacosTransferFunction::Rec709,
        value if value == rec2020 => MacosTransferFunction::Rec2020,
        value if value == linear => MacosTransferFunction::Linear,
        value if value == pq => MacosTransferFunction::Pq,
        value if value == hlg => MacosTransferFunction::Hlg,
        _ => {
            return Err(MacosCaptureError::UnsupportedColorAttachment(
                "transfer_function",
            ));
        }
    };

    let range = match fourcc {
        0x3432_3076 | 0x7834_3434 => MacosColorRange::Video,
        _ => MacosColorRange::Full,
    };
    let is_rgb = matches!(
        format,
        MacosCapturePixelFormat::Bgra8
            | MacosCapturePixelFormat::Argb2101010
            | MacosCapturePixelFormat::Rgba16Float
    );
    let (matrix, chroma_location) = if is_rgb {
        (None, None)
    } else {
        (
            Some(yuv_matrix(pixel_buffer)?),
            Some(chroma_location(pixel_buffer)?),
        )
    };

    Ok(MacosCaptureColorimetry {
        primaries,
        transfer,
        matrix,
        range,
        chroma_location,
    })
}

fn hdr_luminance_metadata(
    pixel_buffer: &CVPixelBuffer,
) -> Result<(Option<f32>, Option<f32>), MacosCaptureError> {
    let content_headroom = content_headroom(pixel_buffer)?;
    let content_peak_nits = content_peak_nits(pixel_buffer)?;
    let source_reference_white_nits = content_peak_nits
        .zip(content_headroom)
        .map(|(peak, headroom)| peak / headroom)
        .filter(|reference| reference.is_finite() && *reference > 0.0);
    Ok((source_reference_white_nits, content_headroom))
}

fn content_headroom(pixel_buffer: &CVPixelBuffer) -> Result<Option<f32>, MacosCaptureError> {
    let surface =
        CVPixelBufferGetIOSurface(Some(pixel_buffer)).ok_or(MacosCaptureError::MissingIoSurface)?;
    // SAFETY: This is a process-lifetime IOSurface key available at the macOS
    // 15.2 deployment floor.
    let value = surface.value(unsafe { kIOSurfaceContentHeadroom });
    let Some(value) = value else {
        return Ok(None);
    };
    let headroom = value
        .downcast_ref::<CFNumber>()
        .and_then(CFNumber::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite() && *value >= 1.0)
        .ok_or(MacosCaptureError::MalformedLuminanceAttachment(
            "content_headroom",
        ))?;
    Ok(Some(headroom))
}

fn content_peak_nits(pixel_buffer: &CVBuffer) -> Result<Option<f32>, MacosCaptureError> {
    // SAFETY: This is a process-lifetime Core Video key, and a null mode
    // pointer explicitly requests no attachment-mode output.
    let value =
        unsafe { pixel_buffer.attachment(kCVImageBufferContentLightLevelInfoKey, ptr::null_mut()) };
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes = cf_data_bytes(&value).ok_or(MacosCaptureError::MalformedLuminanceAttachment(
        "content_light_level_info",
    ))?;
    if bytes.len() != 4 {
        return Err(MacosCaptureError::MalformedLuminanceAttachment(
            "content_light_level_info",
        ));
    }
    let max_content_light_level = u16::from_be_bytes([bytes[0], bytes[1]]);
    Ok((max_content_light_level != 0).then_some(f32::from(max_content_light_level)))
}

fn cf_data_bytes(value: &CFType) -> Option<&[u8]> {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C-unwind" {
        fn CFDataGetTypeID() -> usize;
        fn CFDataGetLength(data: *const c_void) -> isize;
        fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    }

    // SAFETY: The CFType is live for the returned borrow, and the type ID is
    // checked before calling CFData accessors.
    unsafe {
        if CFGetTypeID(Some(value)) != CFDataGetTypeID() {
            return None;
        }
        let data = ptr::from_ref(value).cast::<c_void>();
        let length = usize::try_from(CFDataGetLength(data)).ok()?;
        let bytes = CFDataGetBytePtr(data);
        if bytes.is_null() && length != 0 {
            return None;
        }
        Some(std::slice::from_raw_parts(bytes, length))
    }
}

fn yuv_matrix(pixel_buffer: &CVBuffer) -> Result<MacosYuvMatrix, MacosCaptureError> {
    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (matrix_key, bt601, bt709, bt2020) = unsafe {
        (
            kCVImageBufferYCbCrMatrixKey,
            kCVImageBufferYCbCrMatrix_ITU_R_601_4,
            kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            kCVImageBufferYCbCrMatrix_ITU_R_2020,
        )
    };
    let value = color_attachment(pixel_buffer, matrix_key, "ycbcr_matrix")?;
    match &*value {
        value if value == bt601 => Ok(MacosYuvMatrix::Bt601),
        value if value == bt709 => Ok(MacosYuvMatrix::Bt709),
        value if value == bt2020 => Ok(MacosYuvMatrix::Bt2020),
        _ => Err(MacosCaptureError::UnsupportedColorAttachment(
            "ycbcr_matrix",
        )),
    }
}

fn chroma_location(pixel_buffer: &CVBuffer) -> Result<MacosChromaLocation, MacosCaptureError> {
    // SAFETY: These Core Video constants are process-lifetime immutable CFString
    // references supplied by the linked framework.
    let (location_key, left, center, top_left) = unsafe {
        (
            kCVImageBufferChromaLocationTopFieldKey,
            kCVImageBufferChromaLocation_Left,
            kCVImageBufferChromaLocation_Center,
            kCVImageBufferChromaLocation_TopLeft,
        )
    };
    let value = color_attachment(pixel_buffer, location_key, "chroma_location")?;
    match &*value {
        value if value == left => Ok(MacosChromaLocation::Left),
        value if value == center => Ok(MacosChromaLocation::Center),
        value if value == top_left => Ok(MacosChromaLocation::TopLeft),
        _ => Err(MacosCaptureError::UnsupportedColorAttachment(
            "chroma_location",
        )),
    }
}

fn color_attachment(
    pixel_buffer: &CVBuffer,
    key: &CFString,
    name: &'static str,
) -> Result<CFRetained<CFString>, MacosCaptureError> {
    // SAFETY: A null mode pointer explicitly requests no attachment-mode
    // output, and the retained result survives the pixel-buffer query.
    let value = unsafe { pixel_buffer.attachment(key, ptr::null_mut()) }
        .ok_or(MacosCaptureError::MissingColorAttachment(name))?;
    value
        .downcast::<CFString>()
        .map_err(|_| MacosCaptureError::UnsupportedColorAttachment(name))
}

struct FrameAttachments(CFRetained<CFDictionary<CFString, CFType>>);

impl FrameAttachments {
    fn from_sample(sample: &CMSampleBuffer) -> Result<Self, MacosCaptureError> {
        // SAFETY: The sample reference is valid for this callback. Passing
        // false prevents Core Media from mutating it to create attachments.
        let attachments = unsafe { sample.sample_attachments_array(false) }
            .ok_or(MacosCaptureError::MissingFrameAttachments)?;
        if attachments.len() != 1 {
            return Err(MacosCaptureError::MalformedAttachment("frame_info"));
        }
        // SAFETY: Core Media documents this as an array of CF attachment
        // dictionaries. The element is still type-checked before use.
        let attachments = unsafe { attachments.cast_unchecked::<CFType>() };
        let dictionary = attachments
            .get(0)
            .and_then(|value| value.downcast::<CFDictionary>().ok())
            .ok_or(MacosCaptureError::MalformedAttachment("frame_info"))?;
        // SAFETY: ScreenCaptureKit frame dictionaries use NSString keys and
        // Core Foundation object values. Both are toll-free bridge types.
        let dictionary =
            unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(dictionary) };
        Ok(Self(dictionary))
    }

    fn decode(&self) -> MacosRawFrameAttachments {
        // SAFETY: ScreenCaptureKit exports process-lifetime immutable NSString
        // constants for every frame-info dictionary key.
        let (status, display_time, scale, content_scale, content, dirty, screen, bounding) = unsafe {
            (
                SCStreamFrameInfoStatus,
                SCStreamFrameInfoDisplayTime,
                SCStreamFrameInfoScaleFactor,
                SCStreamFrameInfoContentScale,
                SCStreamFrameInfoContentRect,
                SCStreamFrameInfoDirtyRects,
                SCStreamFrameInfoScreenRect,
                SCStreamFrameInfoBoundingRect,
            )
        };
        MacosRawFrameAttachments {
            status: self.number_i64(status),
            display_time: self.number_u64(display_time),
            display_scale_factor: self.number_f64(scale),
            content_scale: self.number_f64(content_scale),
            content_rect: self.point_rect(content),
            dirty_rects: self.pixel_rects(dirty),
            screen_rect: self.point_rect(screen),
            bounding_rect: self.point_rect(bounding),
        }
    }

    fn value(&self, key: &NSString) -> Option<CFRetained<CFType>> {
        self.0.get(cf_string(key))
    }

    fn number_i64(&self, key: &NSString) -> MacosAttachment<i64> {
        self.convert(key, |value| value.downcast_ref::<CFNumber>()?.as_i64())
    }

    fn number_u64(&self, key: &NSString) -> MacosAttachment<u64> {
        self.convert(key, |value| {
            value
                .downcast_ref::<CFNumber>()?
                .as_i64()
                .and_then(|number| u64::try_from(number).ok())
        })
    }

    fn number_f64(&self, key: &NSString) -> MacosAttachment<f64> {
        self.convert(key, |value| value.downcast_ref::<CFNumber>()?.as_f64())
    }

    fn point_rect(&self, key: &NSString) -> MacosAttachment<MacosPointRect> {
        self.convert(key, point_rect)
    }

    fn pixel_rects(&self, key: &NSString) -> MacosAttachment<Vec<MacosPixelRect>> {
        self.convert(key, |value| {
            let array = value.downcast_ref::<CFArray>()?;
            // SAFETY: ScreenCaptureKit documents dirtyRects as an NSArray of
            // NSValue objects. Every element is checked before conversion.
            let array = unsafe { array.cast_unchecked::<CFType>() };
            array.iter().map(|rect| pixel_rect(&rect)).collect()
        })
    }

    fn convert<T>(
        &self,
        key: &NSString,
        convert: impl FnOnce(&CFType) -> Option<T>,
    ) -> MacosAttachment<T> {
        match self.value(key) {
            None => MacosAttachment::Missing,
            Some(value) => {
                convert(&value).map_or(MacosAttachment::Malformed, MacosAttachment::Value)
            }
        }
    }
}

fn cf_string(value: &NSString) -> &CFString {
    // SAFETY: NSString and CFString are toll-free bridged immutable string
    // representations on macOS.
    unsafe { &*(ptr::from_ref(value).cast::<CFString>()) }
}

fn point_rect(value: &CFType) -> Option<MacosPointRect> {
    let dictionary = value.downcast_ref::<CFDictionary>()?;
    let mut rect = CGRect::ZERO;
    // SAFETY: The output points to initialized CGRect storage, and the input
    // was type-checked as a CFDictionary.
    if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(dictionary), &mut rect) } {
        return None;
    }
    MacosPointRect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
    .ok()
}

fn pixel_rect(value: &CFType) -> Option<MacosPixelRect> {
    let object = <CFType as AsRef<AnyObject>>::as_ref(value);
    let rect = object.downcast_ref::<NSValue>()?.get_rect()?;
    let x = exact_i64(rect.origin.x)?;
    let y = exact_i64(rect.origin.y)?;
    let width = exact_u32(rect.size.width)?;
    let height = exact_u32(rect.size.height)?;
    MacosPixelRect::new(x, y, width, height).ok()
}

fn exact_i64(value: f64) -> Option<i64> {
    if !value.is_finite()
        || value.fract() != 0.0
        || value < i64::MIN as f64
        || value > i64::MAX as f64
    {
        return None;
    }
    Some(value as i64)
}

fn exact_u32(value: f64) -> Option<u32> {
    if !value.is_finite() || value.fract() != 0.0 || value <= 0.0 || value > f64::from(u32::MAX) {
        return None;
    }
    Some(value as u32)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    use super::{
        CandidateReservation, CandidateStage, InterruptedRestage, InterruptionRecoveryPhase,
        MacosCaptureColorimetry, MacosCaptureDynamicRange, MacosCaptureError,
        MacosCapturePixelFormat, MacosColorPrimaries, MacosColorRange, MacosConfiguredStream,
        MacosDeliveredFrameMetadata, MacosFrameEvent, MacosFrameStatus, MacosHostArchitecture,
        MacosNativeTransactionError, MacosNativeTransactionPhase, MacosPixelExtent,
        MacosProtectedSourceState, MacosRuntimeCapability, MacosStreamDeliveryRejection,
        MacosStreamDeliveryState, MacosStreamDeliveryValidator, MacosStreamPreset,
        MacosStreamRequestTransaction, MacosTahoeCapabilities, MacosTahoeRuntimeProbes,
        MacosTransferFunction, MacosValidatedStreamDelivery, NativeSelectionFilter, NativeStream,
        PendingStreamRequest, PoolBackingLifetime, PoolObservation, SCCaptureDynamicRange,
        SCStreamConfiguration, SCStreamConfigurationPreset, ScreenshotCaptureBackend,
        ScreenshotFilterHandle, ScreenshotIdentityFence, ScreenshotImageCompletion,
        ScreenshotTransactionSnapshot, SessionShared, SourceResolution, StreamSlot, SysctlI32Value,
        capture_capabilities_from_probes, capture_dynamic_range, classify_delivery_error,
        color_range_from_fourcc, conservative_pool_quote, execute_screenshot_transaction,
        is_hypercolor_ui_bundle_identifier, route_retained_delivery, route_stream_activity,
        route_stream_lifecycle, session_selection_source_id, stream_request_transaction,
        with_admitted_surface,
    };
    use crate::worker::{LatestSampleWorker, SamplePublishOutcome};
    use crate::{
        MacosCaptureCadence, MacosScreenshotReferenceCapability, MacosScreenshotReferenceImage,
        MacosScreenshotReferenceSet, MacosStreamRequest,
    };

    struct FixtureScreenshotCall {
        filter_id: u64,
        dynamic_range: MacosCaptureDynamicRange,
        completion: ScreenshotImageCompletion,
    }

    #[derive(Default)]
    struct FixtureScreenshotBackend {
        calls: Mutex<VecDeque<FixtureScreenshotCall>>,
    }

    impl FixtureScreenshotBackend {
        fn calls(&self) -> Vec<(u64, MacosCaptureDynamicRange)> {
            super::lock(&self.calls)
                .iter()
                .map(|call| (call.filter_id, call.dynamic_range))
                .collect()
        }

        fn complete_next(&self, result: Result<MacosScreenshotReferenceImage, MacosCaptureError>) {
            let call = super::lock(&self.calls)
                .pop_front()
                .expect("fixture callback should be pending");
            (call.completion)(result);
        }
    }

    impl ScreenshotCaptureBackend for FixtureScreenshotBackend {
        fn capture(
            &self,
            filter: ScreenshotFilterHandle,
            dynamic_range: MacosCaptureDynamicRange,
            _cursor_composed: bool,
            completion: ScreenshotImageCompletion,
        ) -> Result<(), MacosCaptureError> {
            let ScreenshotFilterHandle::Fixture(filter_id) = filter else {
                panic!("fixture backend requires a fixture filter");
            };
            super::lock(&self.calls).push_back(FixtureScreenshotCall {
                filter_id,
                dynamic_range,
                completion,
            });
            Ok(())
        }
    }

    struct FixtureScreenshotFence {
        identity: Mutex<(Arc<str>, u64, u64)>,
    }

    impl ScreenshotIdentityFence for FixtureScreenshotFence {
        fn matches(&self, source_id: &str, generation: u64, revision: u64) -> bool {
            let identity = super::lock(&self.identity);
            identity.0.as_ref() == source_id && identity.1 == generation && identity.2 == revision
        }
    }

    fn screenshot_fixture(
        capability: MacosScreenshotReferenceCapability,
    ) -> (
        ScreenshotTransactionSnapshot,
        Arc<FixtureScreenshotFence>,
        Arc<FixtureScreenshotBackend>,
    ) {
        let (source_id, generation) = match &capability {
            MacosScreenshotReferenceCapability::PendingFirstFrame => (Arc::from("pending"), 0),
            MacosScreenshotReferenceCapability::SdrOnly {
                source_id,
                generation,
            }
            | MacosScreenshotReferenceCapability::PairedSdrHdr {
                source_id,
                generation,
            } => (Arc::clone(source_id), *generation),
        };
        let selection_revision = 11;
        (
            ScreenshotTransactionSnapshot {
                filter: ScreenshotFilterHandle::Fixture(7),
                source_id: Arc::clone(&source_id),
                generation,
                selection_revision,
                capability,
            },
            Arc::new(FixtureScreenshotFence {
                identity: Mutex::new((source_id, generation, selection_revision)),
            }),
            Arc::new(FixtureScreenshotBackend::default()),
        )
    }

    const ABSENT_TAHOE_PROBES: MacosTahoeRuntimeProbes = MacosTahoeRuntimeProbes {
        content_tone_mapping_info_symbol: MacosRuntimeCapability::Absent,
        screenshot_configuration_class: MacosRuntimeCapability::Absent,
        screenshot_dynamic_range_selector: MacosRuntimeCapability::Absent,
        screenshot_capture_selector: MacosRuntimeCapability::Absent,
    };

    #[test]
    fn hypercolor_ui_exclusion_matches_only_the_stable_app_bundle() {
        assert!(is_hypercolor_ui_bundle_identifier(
            "tech.hyperbliss.hypercolor"
        ));
        assert!(!is_hypercolor_ui_bundle_identifier(
            "tech.hyperbliss.hypercolor.daemon"
        ));
        assert!(!is_hypercolor_ui_bundle_identifier(
            "com.example.hypercolor"
        ));
    }

    #[test]
    fn stream_selection_revision_advances_monotonically_across_lifecycles() {
        let shared = Arc::new(SessionShared::new(
            MacosProtectedSourceState::ReadyIdle,
            super::MacosCaptureSelector::Auto,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        ));
        let streams = StreamSlot::new(shared, MacosStreamRequest::default())
            .expect("fixture native lifecycle starts");
        assert_eq!(streams.selection_revision(), 0);

        assert!(streams.set_capture_active(true));
        assert!(streams.set_capture_active(false));
        assert_eq!(streams.selection_revision(), 1);

        assert!(streams.set_capture_active(true));
        assert!(streams.set_capture_active(false));
        assert_eq!(streams.selection_revision(), 2);
    }

    #[test]
    fn incomplete_native_delivery_never_enters_the_latest_frame_slot() {
        let latest_frame_slot_called = AtomicBool::new(false);
        let lifecycle_called = AtomicBool::new(false);

        route_retained_delivery(
            super::RetainedNativeDelivery::<()>::Lifecycle(MacosFrameStatus::Idle),
            |_| latest_frame_slot_called.store(true, Ordering::Release),
            |status| {
                assert_eq!(status, MacosFrameStatus::Idle);
                lifecycle_called.store(true, Ordering::Release);
            },
        );

        assert!(!latest_frame_slot_called.load(Ordering::Acquire));
        assert!(lifecycle_called.load(Ordering::Acquire));
    }

    fn stream_slot_fixture(current_epoch: u64, selection_revision: u64) -> Arc<StreamSlot> {
        let shared = Arc::new(SessionShared::new(
            MacosProtectedSourceState::Live,
            super::MacosCaptureSelector::Auto,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        ));
        shared.set_capture_active(true);
        shared.activate_epoch(current_epoch);
        let streams = StreamSlot::new(shared, MacosStreamRequest::default())
            .expect("fixture native lifecycle starts");
        {
            let mut state = super::lock(&streams.state);
            state.selection_revision = selection_revision;
            state.selected_filter = Some(NativeSelectionFilter::fixture(1));
            state.fixture_current_epoch = (current_epoch != 0).then_some(current_epoch);
        }
        streams
    }

    fn reserve_selection_candidate_fixture(
        streams: &StreamSlot,
        epoch: u64,
        request: MacosStreamRequest,
        selection_id: u64,
    ) -> Result<Option<(CandidateStage, Option<NativeStream>)>, MacosCaptureError> {
        streams
            .reserve_candidate_stage(
                epoch,
                request,
                Some(NativeSelectionFilter::fixture(selection_id)),
                None,
                None,
            )
            .map(|reservation| {
                reservation.map(|reservation| {
                    StreamSlot::finish_replaced_candidate(reservation.replaced_settlement);
                    (reservation.stage, reservation.replaced)
                })
            })
    }

    fn reserve_request_candidate_fixture(
        streams: &StreamSlot,
        epoch: u64,
        request: MacosStreamRequest,
        pending: PendingStreamRequest,
    ) -> Result<Option<(CandidateStage, Option<NativeStream>)>, MacosCaptureError> {
        streams
            .reserve_candidate_stage(epoch, request, None, None, Some(pending))
            .map(|reservation| {
                reservation.map(|reservation| {
                    StreamSlot::finish_replaced_candidate(reservation.replaced_settlement);
                    (reservation.stage, reservation.replaced)
                })
            })
    }

    fn pending_request(
        epoch: u64,
        request: MacosStreamRequest,
    ) -> (PendingStreamRequest, MacosStreamRequestTransaction) {
        let (transaction, completion) = stream_request_transaction(epoch);
        (
            PendingStreamRequest {
                epoch,
                request,
                completion,
            },
            transaction,
        )
    }

    fn selection_filter_ids(streams: &StreamSlot) -> (Option<u64>, Option<(u64, u64)>) {
        let state = super::lock(&streams.state);
        (
            state
                .selected_filter
                .as_ref()
                .map(NativeSelectionFilter::fixture_id),
            state
                .pending_selection
                .as_ref()
                .map(|pending| (pending.epoch, pending.selection_filter.fixture_id())),
        )
    }

    fn sdr_delivery_fixture() -> MacosValidatedStreamDelivery {
        let configured = MacosConfiguredStream {
            requested_dynamic_range: MacosCaptureDynamicRange::Sdr,
            requested_preset: MacosStreamPreset::SdrDefault,
            configured_dynamic_range: MacosCaptureDynamicRange::Sdr,
            configured_pixel_format: MacosCapturePixelFormat::Bgra8,
            configured_color_range: MacosColorRange::Full,
        };
        let delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Bgra8,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            None,
            None,
        )
        .expect("fixture delivery metadata should be valid");
        MacosValidatedStreamDelivery {
            configured,
            delivered,
        }
    }

    #[test]
    fn current_publication_holds_lifecycle_until_publish_precedes_deactivation() {
        let streams = stream_slot_fixture(41, 9);
        let (publishing_tx, publishing_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let publishing_streams = Arc::clone(&streams);
        let publisher = thread::spawn(move || {
            publishing_streams.publish_decoded_event_with(41, false, None, || {
                publishing_tx
                    .send(())
                    .expect("publication should be observable");
                release_rx.recv().expect("publication should resume");
                publishing_streams
                    .shared
                    .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
            })
        });
        publishing_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("current publication should hold the lifecycle gate");
        assert!(streams.state.try_lock().is_ok());

        let (deactivation_started_tx, deactivation_started_rx) = mpsc::channel();
        let (deactivated_tx, deactivated_rx) = mpsc::channel();
        let deactivating_streams = Arc::clone(&streams);
        let deactivator = thread::spawn(move || {
            deactivation_started_tx
                .send(())
                .expect("deactivation attempt should be observable");
            let changed = deactivating_streams.set_capture_active(false);
            deactivating_streams
                .shared
                .set_status(MacosProtectedSourceState::ReadyIdle);
            deactivated_tx
                .send(changed)
                .expect("deactivation should be observable");
        });
        deactivation_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation should reach the lifecycle gate");
        assert_eq!(
            deactivated_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(streams.shared.current_epoch(), 41);

        release_tx.send(()).expect("publication should be released");
        assert!(publisher.join().expect("publisher thread should join"));
        assert!(
            deactivated_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should follow publication")
        );
        deactivator.join().expect("deactivation thread should join");
        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::ReadyIdle
        );
    }

    #[test]
    fn candidate_first_frame_publish_holds_lifecycle_until_deactivation() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 2)
                .expect("candidate reservation should succeed")
                .expect("active capture should admit the candidate");
        assert!(streams.start_candidate_fixture(stage));
        let (publishing_tx, publishing_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let publishing_streams = Arc::clone(&streams);
        let publisher = thread::spawn(move || {
            publishing_streams.publish_decoded_event_with(
                42,
                true,
                Some(sdr_delivery_fixture()),
                || {
                    publishing_tx
                        .send(())
                        .expect("first-frame publication should be observable");
                    release_rx.recv().expect("publication should resume");
                    publishing_streams
                        .shared
                        .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
                },
            )
        });
        publishing_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("candidate activation should reach publication under the lifecycle gate");
        assert!(streams.state.try_lock().is_ok());
        assert_eq!(streams.shared.current_epoch(), 42);
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));

        let (deactivation_started_tx, deactivation_started_rx) = mpsc::channel();
        let (deactivated_tx, deactivated_rx) = mpsc::channel();
        let deactivating_streams = Arc::clone(&streams);
        let deactivator = thread::spawn(move || {
            deactivation_started_tx
                .send(())
                .expect("deactivation attempt should be observable");
            let changed = deactivating_streams.set_capture_active(false);
            deactivating_streams
                .shared
                .set_status(MacosProtectedSourceState::ReadyIdle);
            deactivated_tx
                .send(changed)
                .expect("deactivation should be observable");
        });
        deactivation_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation should reach the lifecycle gate");
        assert_eq!(
            deactivated_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        release_tx.send(()).expect("publication should be released");
        assert!(publisher.join().expect("publisher thread should join"));
        assert!(
            deactivated_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should follow first-frame publication")
        );
        deactivator.join().expect("deactivation thread should join");
        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::ReadyIdle
        );
    }

    #[test]
    fn stale_picker_resolution_cannot_mutate_filter_acceptance() {
        let streams = stream_slot_fixture(41, 9);
        let stale = streams
            .begin_resolution()
            .expect("picker resolution should begin");
        streams.shared.enable_picker_callbacks(stale);
        let picker_resolution = streams
            .shared
            .picker_resolution()
            .expect("picker update should retain its exact resolution");
        let initial_revision = streams.selection_revision();
        let initial_selection = selection_filter_ids(&streams);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let accepting_streams = Arc::clone(&streams);
        let accepting = thread::spawn(move || {
            accepting_streams.accept_selection_filter_with_hooks(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                picker_resolution,
                true,
                (
                    || {
                        ready_tx
                            .send(())
                            .expect("retained picker filter should be observable");
                        release_rx
                            .recv()
                            .expect("picker filter acceptance should resume");
                    },
                    || panic!("stale picker filter must not be accepted"),
                ),
            )
        });
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("picker filter should pause before the lifecycle transition");

        let fresh = streams
            .begin_picker_resolution()
            .expect("newer picker resolution should begin");
        release_tx
            .send(())
            .expect("stale picker acceptance should resume");
        assert!(matches!(
            accepting.join().expect("acceptance thread should join"),
            Ok(super::FilterAcceptance::Stale)
        ));
        assert_eq!(streams.selection_revision(), initial_revision);
        assert_eq!(selection_filter_ids(&streams), initial_selection);

        let retry = streams
            .accept_selection_filter(
                NativeSelectionFilter::fixture(3),
                MacosStreamRequest::default(),
                43,
                fresh,
                true,
            )
            .expect("fresh resolution should be accepted");
        assert!(matches!(retry, super::FilterAcceptance::Candidate { .. }));
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((43, 3))));
    }

    fn install_live_successor(streams: &StreamSlot, epoch: u64) {
        let (stage, _) = reserve_selection_candidate_fixture(
            streams,
            epoch,
            MacosStreamRequest::default(),
            epoch,
        )
        .expect("successor reservation should succeed")
        .expect("active capture should admit the successor");
        assert!(streams.start_candidate_fixture(stage));
        assert!(streams.activate_candidate_fixture(epoch));
        assert!(streams.publish_decoded_event_with(epoch, false, None, || {
            streams
                .shared
                .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
        }));
    }

    fn assert_retired_error_cannot_overwrite_successor(fatal: bool) {
        let streams = stream_slot_fixture(41, 9);
        let error = MacosCaptureError::CaptureWorkerStartFailed(
            "retired injected stream failure".to_owned(),
        );
        let (retired_tx, retired_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let failing_streams = Arc::clone(&streams);
        let failing_shared = Arc::clone(&streams.shared);
        let finalizer = thread::spawn(move || {
            let after_retirement = || {
                retired_tx
                    .send(())
                    .expect("retired stream should be observable");
                release_rx.recv().expect("error finalization should resume");
            };
            if fatal {
                super::handle_owned_fatal_stream_error_with(
                    &failing_streams,
                    41,
                    failing_shared,
                    error,
                    after_retirement,
                );
            } else {
                super::handle_owned_stream_error_with(
                    &failing_streams,
                    41,
                    &failing_shared,
                    MacosProtectedSourceState::PermissionDenied,
                    error,
                    after_retirement,
                );
            }
        });
        retired_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old error should pause after retirement");
        assert_eq!(streams.shared.current_epoch(), 0);

        install_live_successor(&streams, 42);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        release_tx
            .send(())
            .expect("old error finalization should resume");
        finalizer.join().expect("error finalizer should join");

        assert_eq!(streams.shared.current_epoch(), 42);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle)))
        ));
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::RecoverableError(_)))
        ));
    }

    #[test]
    fn ordinary_error_finalization_cannot_overwrite_live_successor() {
        assert_retired_error_cannot_overwrite_successor(false);
    }

    #[test]
    fn fatal_error_finalization_cannot_overwrite_live_successor() {
        assert_retired_error_cannot_overwrite_successor(true);
    }

    #[test]
    fn duplicate_fatal_callbacks_invalidate_the_owned_epoch_once() {
        let streams = stream_slot_fixture(41, 9);
        let error = MacosCaptureError::CaptureWorkerStartFailed(
            "duplicate fatal callback fixture".to_owned(),
        );

        super::handle_owned_fatal_stream_error(
            &streams,
            41,
            Arc::clone(&streams.shared),
            error.clone(),
        );
        super::handle_owned_fatal_stream_error(&streams, 41, Arc::clone(&streams.shared), error);

        assert!(matches!(
            streams.shared.mailbox.take_latest_with_generation(),
            Some((_, 1, Err(MacosCaptureError::CaptureWorkerStartFailed(_))))
        ));
        assert!(!streams.shared.mailbox.has_pending());
    }

    #[test]
    fn retired_preparation_failure_cannot_overwrite_live_successor() {
        let streams = stream_slot_fixture(41, 9);
        let removal = streams.remove(41, None);
        assert_eq!(removal.role, super::StreamRole::Current);
        assert_eq!(streams.shared.current_epoch(), 0);
        let recovery = InterruptedRestage::interrupted(41, 9);
        let reservation = streams
            .reserve_candidate_stage(
                42,
                MacosStreamRequest::default(),
                Some(NativeSelectionFilter::fixture(1)),
                Some(recovery),
                None,
            )
            .expect("interrupted restage should reserve")
            .expect("active capture should admit interrupted restage");
        let CandidateReservation {
            stage,
            replaced,
            replaced_settlement,
            ..
        } = reservation;
        StreamSlot::finish_replaced_candidate(replaced_settlement);
        assert!(replaced.is_none());
        let failure = streams.fail_candidate_preparation_fixture(
            stage,
            MacosCaptureError::CaptureWorkerStartFailed(
                "retired interrupted restage failed to prepare".to_owned(),
            ),
        );
        let (paused_tx, paused_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finalized_tx, finalized_rx) = mpsc::channel();
        let failing_streams = Arc::clone(&streams);
        let finalizer = thread::spawn(move || {
            let finalized =
                failing_streams.finalize_candidate_preparation_failure_with(failure, None, || {
                    paused_tx
                        .send(())
                        .expect("post-retirement finalization pause should be observable");
                    release_rx
                        .recv()
                        .expect("preparation finalization should resume");
                });
            finalized_tx
                .send(finalized)
                .expect("preparation finalization result should be observable");
        });
        paused_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old preparation failure should pause after retirement");

        let (successor, _) =
            reserve_selection_candidate_fixture(&streams, 43, MacosStreamRequest::default(), 43)
                .expect("successor reservation should succeed")
                .expect("active capture should admit the successor");
        assert!(streams.start_candidate_fixture(successor));
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        release_tx
            .send(())
            .expect("old preparation finalization should resume");
        assert!(
            !finalized_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("stale preparation finalization should finish")
        );
        finalizer.join().expect("preparation finalizer should join");

        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started)))
        ));
        assert!(streams.activate_candidate_fixture(43));
        assert!(streams.publish_decoded_event_with(43, false, None, || {
            streams
                .shared
                .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
        }));
        assert_eq!(streams.shared.current_epoch(), 43);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle)))
        ));
    }

    #[test]
    fn preparation_failure_revision_rejects_request_only_aba() {
        let original = MacosStreamRequest::default();
        let request_a = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("first request-only candidate should be valid");
        let request_b = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(45), true)
            .expect("second request-only candidate should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;

        let (pending_a, completion_a) = pending_request(42, request_a);
        let (stage_a, _) = reserve_request_candidate_fixture(&streams, 42, request_a, pending_a)
            .expect("first request candidate should reserve")
            .expect("active capture should stage the first request candidate");
        assert_eq!(streams.selection_revision(), 9);
        let revision_a = stage_a.lifecycle_revision;
        let failure_a = streams.fail_candidate_preparation_fixture(
            stage_a,
            MacosCaptureError::CaptureWorkerStartFailed(
                "first request candidate failed to prepare".to_owned(),
            ),
        );
        assert!(failure_a.stage.lifecycle_revision > revision_a);
        let failure_a_revision = failure_a.stage.lifecycle_revision;
        assert_eq!(completion_a.try_recv(), Err(mpsc::TryRecvError::Empty));

        let (paused_tx, paused_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finalized_tx, finalized_rx) = mpsc::channel();
        let failing_streams = Arc::clone(&streams);
        let finalizer_a = thread::spawn(move || {
            let finalized = failing_streams.finalize_candidate_preparation_failure_with(
                failure_a,
                None,
                || {
                    paused_tx
                        .send(())
                        .expect("first finalizer pause should be observable");
                    release_rx.recv().expect("first finalizer should resume");
                },
            );
            finalized_tx
                .send(finalized)
                .expect("first finalizer result should be observable");
        });
        paused_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first finalizer should pause before lifecycle validation");
        assert_eq!(completion_a.try_recv(), Err(mpsc::TryRecvError::Empty));

        let (pending_b, completion_b) = pending_request(43, request_b);
        let (stage_b, _) = reserve_request_candidate_fixture(&streams, 43, request_b, pending_b)
            .expect("second request candidate should reserve")
            .expect("active capture should stage the second request candidate");
        assert_eq!(streams.selection_revision(), 9);
        assert!(stage_b.lifecycle_revision > failure_a_revision);
        let error_b = MacosCaptureError::CaptureWorkerStartFailed(
            "second request candidate failed to prepare".to_owned(),
        );
        let failure_b = streams.fail_candidate_preparation_fixture(stage_b, error_b.clone());
        assert!(failure_b.stage.lifecycle_revision > stage_b.lifecycle_revision);
        let failure_b_revision = failure_b.stage.lifecycle_revision;
        assert_eq!(completion_b.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(streams.finalize_candidate_preparation_failure(failure_b, None));
        assert!(
            completion_b
                .recv()
                .expect("second request should complete after finalization")
                .is_err()
        );
        assert!(super::lock(&streams.state).lifecycle_revision > failure_b_revision);
        assert_eq!(streams.selection_revision(), 9);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);

        release_tx
            .send(())
            .expect("first finalizer should resume after the ABA lifecycle");
        assert!(
            !finalized_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("stale first finalizer should finish")
        );
        finalizer_a.join().expect("first finalizer should join");
        assert!(
            completion_a
                .recv()
                .expect("first request should complete after stale finalization")
                .is_err()
        );

        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::RecoverableError(error))) if error.as_ref() == &error_b
        ));
    }

    #[test]
    fn current_inactive_epoch_rejects_queued_frame_publication() {
        let streams = stream_slot_fixture(41, 9);
        streams.record_stream_activity(41, false, false);
        let published = AtomicBool::new(false);

        assert!(!streams.publish_decoded_event_with(
            41,
            true,
            Some(sdr_delivery_fixture()),
            || {
                published.store(true, Ordering::Release);
                streams
                    .shared
                    .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
            },
        ));
        assert!(!published.load(Ordering::Acquire));
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::NeedsSelection
        );
    }

    #[test]
    fn terminal_lifecycle_generation_rejects_frames_until_stream_reactivation() {
        let streams = stream_slot_fixture(41, 9);
        assert!(streams.publish_stream_lifecycle(41, MacosFrameStatus::Suspended));
        let stale_published = AtomicBool::new(false);

        assert!(!streams.publish_decoded_event_with(41, true, None, || {
            stale_published.store(true, Ordering::Release);
        }));
        assert!(!stale_published.load(Ordering::Acquire));
        assert!(matches!(
            streams.shared.mailbox.take_latest_with_generation(),
            Some((
                _,
                1,
                Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Suspended))
            ))
        ));

        streams.record_stream_activity(41, true, false);
        let resumed_published = AtomicBool::new(false);
        assert!(streams.publish_decoded_event_with(41, true, None, || {
            resumed_published.store(true, Ordering::Release);
        }));
        assert!(resumed_published.load(Ordering::Acquire));
    }

    #[test]
    fn rejected_terminal_lifecycle_does_not_advance_decode_generation() {
        let streams = stream_slot_fixture(41, 9);
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-terminal-generation-test",
            |sample: ()| sample,
            |(), _publication| {},
        )
        .expect("generation worker should start");
        let samples = worker.input();
        let initial_generation = samples.generation();

        route_stream_lifecycle(&samples, &streams, 999, MacosFrameStatus::Suspended);
        assert_eq!(samples.generation(), initial_generation);

        route_stream_lifecycle(&samples, &streams, 41, MacosFrameStatus::Suspended);
        let terminal_generation = samples.generation();
        assert!(terminal_generation > initial_generation);

        route_stream_lifecycle(&samples, &streams, 41, MacosFrameStatus::Suspended);
        assert_eq!(samples.generation(), terminal_generation);

        worker.close();
        worker.join().expect("generation worker should join");
    }

    #[test]
    fn terminal_invalidation_crossing_cannot_publish_the_old_generation() {
        let streams = stream_slot_fixture(41, 9);
        let publish_streams = Arc::clone(&streams);
        let old_published = Arc::new(AtomicBool::new(false));
        let new_published = Arc::new(AtomicBool::new(false));
        let worker_old_published = Arc::clone(&old_published);
        let worker_new_published = Arc::clone(&new_published);
        let (publication_entered_tx, publication_entered_rx) = mpsc::sync_channel(1);
        let (release_publication_tx, release_publication_rx) = mpsc::sync_channel(1);
        let (publication_result_tx, publication_result_rx) = mpsc::sync_channel(2);
        let mut worker = LatestSampleWorker::spawn(
            "macos-capture-terminal-crossing-test",
            |sample| sample,
            move |sample, publication| {
                if sample == 1 {
                    publication_entered_tx
                        .send(())
                        .expect("old publication should hold the generation lock");
                    release_publication_rx
                        .recv()
                        .expect("old publication should resume");
                }
                let published = publish_streams.publish_decoded_event_if(
                    41,
                    true,
                    None,
                    || publication.is_current(),
                    || {
                        if sample == 1 {
                            worker_old_published.store(true, Ordering::Release);
                        } else {
                            worker_new_published.store(true, Ordering::Release);
                        }
                    },
                );
                publication_result_tx
                    .send((sample, published))
                    .expect("publication outcome should be observable");
            },
        )
        .expect("crossing worker should start");
        let samples = worker.input();

        assert_eq!(samples.publish(1), SamplePublishOutcome::Accepted);
        publication_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old decode should enter generation-locked publication");

        let terminal_samples = samples.clone();
        let terminal_streams = Arc::clone(&streams);
        let (invalidation_requested_tx, invalidation_requested_rx) = mpsc::sync_channel(1);
        let (terminal_done_tx, terminal_done_rx) = mpsc::sync_channel(1);
        let terminal = thread::spawn(move || {
            let accepted = terminal_samples.invalidate_if_observed(
                || {
                    invalidation_requested_tx
                        .send(())
                        .expect("terminal invalidation request should be observable");
                },
                || terminal_streams.publish_stream_lifecycle(41, MacosFrameStatus::Suspended),
            );
            terminal_done_tx
                .send(accepted)
                .expect("terminal outcome should be observable");
        });
        invalidation_requested_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("terminal callback should request invalidation");

        let active_samples = samples.clone();
        let active_streams = Arc::clone(&streams);
        let (active_started_tx, active_started_rx) = mpsc::sync_channel(1);
        let (active_done_tx, active_done_rx) = mpsc::sync_channel(1);
        let active = thread::spawn(move || {
            active_started_tx
                .send(())
                .expect("active callback start should be observable");
            route_stream_activity(&active_samples, &active_streams, 41, true, false);
            active_done_tx
                .send(())
                .expect("active callback completion should be observable");
        });
        active_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exact active callback should start after terminal invalidation");
        assert_eq!(
            active_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        release_publication_tx
            .send(())
            .expect("old generation publication should resume");

        assert_eq!(
            publication_result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("old publication should settle"),
            (1, false)
        );
        assert!(!old_published.load(Ordering::Acquire));
        assert!(
            terminal_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("terminal transition should settle")
        );
        terminal.join().expect("terminal callback should join");
        active_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exact active callback should follow terminal invalidation");
        active.join().expect("active callback should join");

        assert_eq!(samples.publish(2), SamplePublishOutcome::Accepted);
        assert_eq!(
            publication_result_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("new generation should publish"),
            (2, true)
        );
        assert!(new_published.load(Ordering::Acquire));

        worker.close();
        worker.join().expect("crossing worker should join");
    }

    #[test]
    fn candidate_terminal_lifecycle_blocks_first_frame_until_exact_reactivation() {
        for status in [MacosFrameStatus::Suspended, MacosFrameStatus::Stopped] {
            let streams = stream_slot_fixture(41, 9);
            let (stage, _) = reserve_selection_candidate_fixture(
                &streams,
                42,
                MacosStreamRequest::default(),
                42,
            )
            .expect("candidate reservation should succeed")
            .expect("active capture should admit the candidate");
            assert!(streams.start_candidate_fixture(stage));
            let revision = super::lock(&streams.state).lifecycle_revision;

            assert!(!streams.publish_stream_lifecycle(999, status));
            assert_eq!(super::lock(&streams.state).lifecycle_revision, revision);
            assert!(streams.publish_stream_lifecycle(42, status));
            let terminal_revision = super::lock(&streams.state).lifecycle_revision;
            assert!(terminal_revision > revision);
            assert!(!streams.publish_stream_lifecycle(42, status));
            assert_eq!(
                super::lock(&streams.state).lifecycle_revision,
                terminal_revision
            );
            assert!(!streams.shared.mailbox.has_pending());

            let stale_published = AtomicBool::new(false);
            assert!(!streams.publish_decoded_event_with(
                42,
                true,
                Some(sdr_delivery_fixture()),
                || stale_published.store(true, Ordering::Release),
            ));
            assert!(!stale_published.load(Ordering::Acquire));
            assert_eq!(streams.shared.current_epoch(), 41);
            assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));

            streams.record_stream_activity(42, true, false);
            let resumed_published = AtomicBool::new(false);
            assert!(streams.publish_decoded_event_with(
                42,
                true,
                Some(sdr_delivery_fixture()),
                || resumed_published.store(true, Ordering::Release),
            ));
            assert!(resumed_published.load(Ordering::Acquire));
            assert_eq!(streams.shared.current_epoch(), 42);
        }
    }

    #[test]
    fn candidate_inactive_epoch_rejects_first_frame_activation() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("candidate reservation should succeed")
            .expect("active capture should admit the candidate");
        assert!(streams.start_candidate_fixture(stage));
        streams.record_stream_activity(42, false, false);
        let published = AtomicBool::new(false);

        assert!(!streams.publish_decoded_event_with(
            42,
            true,
            Some(sdr_delivery_fixture()),
            || published.store(true, Ordering::Release),
        ));
        assert!(!published.load(Ordering::Acquire));
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        let state = super::lock(&streams.state);
        assert_eq!(state.candidate_epoch, Some(42));
        assert_eq!(
            state.pending_request.as_ref().map(|request| request.epoch),
            Some(42)
        );
    }

    #[test]
    fn selection_stage_adopting_a_pending_request_keeps_deadline_authority() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = MacosStreamRequest::default();
        let (pending, transaction) = pending_request(42, next);
        reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request reservation should succeed")
            .expect("active capture should admit the candidate");

        reserve_selection_candidate_fixture(&streams, 43, next, 43)
            .expect("selection reservation should succeed")
            .expect("the selection stage should adopt the in-flight request");

        let armed = streams
            .arm_candidate_deadline(
                43,
                MacosNativeTransactionPhase::StreamStart,
                Duration::from_secs(5),
            )
            .expect("deadline arming should not error");
        assert!(
            armed,
            "the adopted transaction must answer to the stage that owns it now"
        );
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        let state = super::lock(&streams.state);
        assert_eq!(
            state
                .candidate_completion
                .as_ref()
                .map(|completion| completion.identity().generation),
            Some(43)
        );
        assert_eq!(
            state.pending_request.as_ref().map(|request| request.epoch),
            Some(43)
        );
    }

    #[test]
    fn cancelling_an_adopted_request_tears_down_the_adopting_stage() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = MacosStreamRequest::default();
        let (pending, transaction) = pending_request(42, next);
        reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request reservation should succeed")
            .expect("active capture should admit the candidate");
        let (stage, _) = reserve_selection_candidate_fixture(&streams, 43, next, 43)
            .expect("selection reservation should succeed")
            .expect("the selection stage should adopt the in-flight request");
        assert!(streams.start_candidate_fixture(stage));
        let cancel_streams = Arc::clone(&streams);
        {
            let state = super::lock(&streams.state);
            state
                .candidate_completion
                .as_ref()
                .expect("candidate completion is installed")
                .set_cancel(move |generation| {
                    cancel_streams.cancel_candidate_transaction(generation);
                });
        }

        assert!(transaction.cancel());

        let state = super::lock(&streams.state);
        assert_eq!(state.candidate_epoch, None);
        assert!(state.candidate_completion.is_none());
        assert!(state.pending_request.is_none());
    }

    #[test]
    fn display_current_inactive_callback_does_not_block_publication() {
        let streams = stream_slot_fixture(41, 9);
        streams.record_stream_activity(41, false, true);
        let published = AtomicBool::new(false);

        assert!(streams.publish_decoded_event_with(41, true, None, || {
            published.store(true, Ordering::Release);
            streams
                .shared
                .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
        }));
        assert!(published.load(Ordering::Acquire));
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
    }

    #[test]
    fn display_candidate_inactive_callback_does_not_strand_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request should be valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("candidate reservation should succeed")
            .expect("active capture should admit the candidate");
        assert!(streams.start_candidate_fixture(stage));
        streams.record_stream_activity(42, false, true);

        assert!(
            streams.publish_decoded_event_with(42, true, Some(sdr_delivery_fixture()), || {
                streams
                    .shared
                    .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));
            },)
        );
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.shared.current_epoch(), 42);
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
    }

    fn assert_latest_lifecycle(streams: &StreamSlot, expected: MacosFrameStatus) {
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(actual))) if actual == expected
        ));
    }

    #[test]
    fn stale_picker_cancel_cannot_overwrite_successor_starting() {
        let streams = stream_slot_fixture(0, 0);
        super::lock(&streams.state).selected_filter = None;
        let stale = streams
            .begin_picker_resolution()
            .expect("first picker resolution should begin");
        let fresh = streams
            .begin_picker_resolution()
            .expect("successor picker resolution should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started));

        assert!(!streams.finalize_picker_cancel(&stale));
        assert_eq!(streams.shared.picker_resolution(), Some(fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Started);
    }

    #[test]
    fn stale_picker_failure_cannot_overwrite_successor_live() {
        let streams = stream_slot_fixture(41, 9);
        let stale = streams
            .begin_picker_resolution()
            .expect("first picker resolution should begin");
        let fresh = streams
            .begin_picker_resolution()
            .expect("successor picker resolution should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));

        assert!(!streams.finalize_picker_failure(
            &stale,
            MacosCaptureError::CaptureWorkerStartFailed("stale picker failure".to_owned()),
        ));
        assert_eq!(streams.shared.picker_resolution(), Some(fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Idle);
    }

    #[test]
    fn stale_filter_error_cannot_overwrite_successor_starting() {
        let streams = stream_slot_fixture(0, 0);
        super::lock(&streams.state).selected_filter = None;
        let stale = streams
            .begin_picker_resolution()
            .expect("picker filter resolution should begin");
        let fresh = streams
            .begin_picker_resolution()
            .expect("successor filter resolution should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Started));

        assert!(!streams.finalize_resolution_error(
            &stale,
            true,
            MacosCaptureError::RetainNativeFilterFailed,
        ));
        assert_eq!(streams.shared.picker_resolution(), Some(fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Started);
    }

    #[test]
    fn stale_enumeration_error_cannot_overwrite_successor_live() {
        let streams = stream_slot_fixture(41, 9);
        let stale = streams
            .begin_resolution()
            .expect("first enumeration should begin");
        let fresh = streams
            .begin_resolution()
            .expect("successor enumeration should begin");
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Idle));

        assert!(!streams.finalize_resolution_error(
            &stale,
            false,
            MacosCaptureError::MissingShareableContent,
        ));
        assert!(streams.shared.source_resolution_is_current(&fresh));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
        assert_latest_lifecycle(&streams, MacosFrameStatus::Idle);
    }

    #[test]
    fn diagnostic_selector_remains_primary_across_concurrent_set_selector() {
        let streams = stream_slot_fixture(0, 7);
        let (diagnostic, completion) = streams
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic resolution should begin");
        let resolution = SourceResolution::Diagnostic(diagnostic.clone());
        assert_eq!(
            resolution.selector(),
            &super::MacosCaptureSelector::PrimaryDisplay
        );

        let lifecycle = super::lock(&streams.lifecycle_start);
        let (started_tx, started_rx) = mpsc::channel();
        let mutating_streams = Arc::clone(&streams);
        let mutation = thread::spawn(move || {
            started_tx
                .send(())
                .expect("selector mutation should be observable");
            mutating_streams
                .set_selector_and_begin_resolution(super::MacosCaptureSelector::Auto)
                .expect("selector mutation should begin its own resolution")
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("selector mutation should reach the lifecycle gate");
        assert_eq!(
            streams.shared.selector(),
            super::MacosCaptureSelector::PrimaryDisplay
        );
        drop(lifecycle);
        let successor = mutation.join().expect("selector mutation should join");

        assert_eq!(streams.shared.selector(), super::MacosCaptureSelector::Auto);
        assert_eq!(successor.selector(), &super::MacosCaptureSelector::Auto);
        assert_eq!(
            resolution.selector(),
            &super::MacosCaptureSelector::PrimaryDisplay
        );
        assert_eq!(completion.recv(), Ok(MacosProtectedSourceState::Failed));
    }

    #[test]
    fn diagnostic_setup_fences_old_filter_acceptance_and_new_picker_resolution() {
        let streams = stream_slot_fixture(41, 9);
        let stale_resolution = streams
            .begin_picker_resolution()
            .expect("old picker resolution should begin");
        let (setup_tx, setup_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let setup_streams = Arc::clone(&streams);
        let setup = thread::spawn(move || {
            setup_streams.setup_restart_diagnostic_with(true, || {
                setup_tx
                    .send(())
                    .expect("installed diagnostic setup should be observable");
                release_rx.recv().expect("diagnostic setup should resume");
            })
        });
        setup_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("diagnostic should pause while holding the lifecycle gate");

        assert_eq!(streams.shared.picker_resolution(), None);
        assert_eq!(
            streams.shared.selector(),
            super::MacosCaptureSelector::PrimaryDisplay
        );
        assert!(streams.shared.capture_active());
        assert_eq!(
            streams.shared.selection(),
            super::MacosCaptureSelection::None
        );
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Starting);

        let (filter_started_tx, filter_started_rx) = mpsc::channel();
        let (filter_done_tx, filter_done_rx) = mpsc::channel();
        let filter_streams = Arc::clone(&streams);
        let stale_filter_resolution = stale_resolution.clone();
        let stale_filter = thread::spawn(move || {
            filter_started_tx
                .send(())
                .expect("old filter acceptance should be observable");
            let result = filter_streams.accept_selection_filter(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                stale_filter_resolution,
                true,
            );
            filter_done_tx
                .send(result)
                .expect("old filter result should be observable");
        });
        let (picker_started_tx, picker_started_rx) = mpsc::channel();
        let (picker_done_tx, picker_done_rx) = mpsc::channel();
        let picker_streams = Arc::clone(&streams);
        let new_picker = thread::spawn(move || {
            picker_started_tx
                .send(())
                .expect("new picker resolution should be observable");
            let resolution = picker_streams.begin_picker_resolution();
            picker_done_tx
                .send(resolution)
                .expect("new picker result should be observable");
        });
        filter_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("old filter should reach the lifecycle gate");
        picker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("new picker should reach the lifecycle gate");
        assert!(matches!(
            filter_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            picker_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        release_tx
            .send(())
            .expect("diagnostic setup should release the lifecycle gate");
        let (diagnostic, completion) = setup
            .join()
            .expect("diagnostic setup thread should join")
            .expect("diagnostic setup should succeed");
        assert!(matches!(
            filter_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("old filter should finish after diagnostic setup"),
            Ok(super::FilterAcceptance::Stale)
        ));
        let picker_resolution = picker_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("new picker should finish after diagnostic setup")
            .expect("new picker resolution should succeed");
        stale_filter.join().expect("old filter thread should join");
        new_picker.join().expect("new picker thread should join");

        assert!(!streams.shared.diagnostic_resolution_is_current(&diagnostic));
        assert_eq!(
            streams.shared.picker_resolution(),
            Some(picker_resolution.clone())
        );
        assert!(
            streams
                .shared
                .source_resolution_is_current(&picker_resolution)
        );
        assert_eq!(completion.recv(), Ok(MacosProtectedSourceState::Failed));
        assert_eq!(selection_filter_ids(&streams), (None, None));
    }

    #[test]
    fn inactive_filter_acceptance_precedes_crossing_activation_atomically() {
        let streams = stream_slot_fixture(41, 9);
        assert!(streams.set_capture_active(false));
        streams.next_epoch.store(43, Ordering::Release);
        let resolution = streams
            .begin_resolution()
            .expect("filter resolution should begin");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let accepting_streams = Arc::clone(&streams);
        let accepting = thread::spawn(move || {
            accepting_streams.accept_selection_filter_with(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                resolution,
                false,
                || {
                    accepted_tx
                        .send(())
                        .expect("filter acceptance should be observable");
                    release_rx.recv().expect("filter acceptance should resume");
                },
            )
        });
        accepted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("filter acceptance should hold the lifecycle gate");

        let (activation_tx, activation_rx) = mpsc::channel();
        let activation_streams = Arc::clone(&streams);
        let activation = thread::spawn(move || {
            activation_tx
                .send(activation_streams.begin_capture_activation())
                .expect("activation result should be observable");
        });
        assert!(matches!(
            activation_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        release_tx
            .send(())
            .expect("filter acceptance should be released");
        assert!(matches!(
            accepting.join().expect("acceptance thread should join"),
            Ok(super::FilterAcceptance::Stored(None))
        ));
        let activation_result = activation_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("activation should finish after filter acceptance")
            .expect("activation should reserve the accepted filter");
        activation.join().expect("activation thread should join");
        let super::CaptureActivation::Candidate { reservation, .. } = activation_result else {
            panic!("activation should stage the accepted filter");
        };
        let CandidateReservation {
            stage,
            selection_filter,
            replaced_settlement,
            ..
        } = *reservation;
        StreamSlot::finish_replaced_candidate(replaced_settlement);
        assert_eq!(selection_filter.fixture_id(), 2);
        assert!(streams.start_candidate_fixture(stage));
        assert!(streams.activate_candidate_fixture(stage.epoch));
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
    }

    #[test]
    fn active_filter_acceptance_precedes_crossing_deactivation_atomically() {
        let streams = stream_slot_fixture(41, 9);
        let resolution = streams
            .begin_resolution()
            .expect("filter resolution should begin");
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let accepting_streams = Arc::clone(&streams);
        let accepting = thread::spawn(move || {
            accepting_streams.accept_selection_filter_with(
                NativeSelectionFilter::fixture(2),
                MacosStreamRequest::default(),
                42,
                resolution,
                false,
                || {
                    accepted_tx
                        .send(())
                        .expect("filter acceptance should be observable");
                    release_rx.recv().expect("filter acceptance should resume");
                },
            )
        });
        accepted_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("filter acceptance should hold the lifecycle gate");

        let (deactivation_tx, deactivation_rx) = mpsc::channel();
        let deactivation_streams = Arc::clone(&streams);
        let deactivation = thread::spawn(move || {
            deactivation_tx
                .send(deactivation_streams.set_capture_active(false))
                .expect("deactivation result should be observable");
        });
        assert_eq!(
            deactivation_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );

        release_tx
            .send(())
            .expect("filter acceptance should be released");
        let acceptance = accepting
            .join()
            .expect("acceptance thread should join")
            .expect("active acceptance should reserve a candidate");
        assert!(
            deactivation_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should finish after filter acceptance")
        );
        deactivation
            .join()
            .expect("deactivation thread should join");
        let super::FilterAcceptance::Candidate { reservation, .. } = acceptance else {
            panic!("active acceptance should stage the delivered filter");
        };
        let CandidateReservation {
            stage,
            replaced_settlement,
            ..
        } = *reservation;
        StreamSlot::finish_replaced_candidate(replaced_settlement);
        assert!(!streams.start_candidate_fixture(stage));
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
        assert!(!streams.shared.capture_active());
    }

    #[test]
    fn candidate_activation_requires_its_pending_selection_revision() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 2)
                .expect("candidate reservation succeeds")
                .expect("active capture admits a candidate");
        assert!(streams.start_candidate_fixture(stage));
        super::lock(&streams.state).selection_revision += 1;

        assert!(!streams.activate_candidate_fixture(42));
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((42, 2))));
        assert_eq!(streams.shared.current_epoch(), 41);
    }

    #[test]
    fn interrupted_restage_transitions_once_from_interrupted_to_live() {
        let recovery = InterruptedRestage::interrupted(41, 9);
        assert_eq!(recovery.phase(), InterruptionRecoveryPhase::Interrupted);
        assert!(recovery.can_schedule(true, 0, 9));

        let recovery = recovery
            .schedule(42)
            .expect("the next session epoch should schedule one recovery restage");
        assert_eq!(
            recovery.phase(),
            InterruptionRecoveryPhase::Starting { epoch: 42 }
        );
        assert_eq!(
            recovery.complete(42),
            Some(InterruptionRecoveryPhase::Live { epoch: 42 })
        );
        assert_eq!(recovery.complete(43), None);
        assert_eq!(recovery.schedule(43), None);
    }

    #[test]
    fn interrupted_restage_cancels_when_capture_demand_reaches_zero() {
        let recovery = InterruptedRestage::interrupted(41, 9);

        assert!(!recovery.can_schedule(false, 0, 9));
    }

    #[test]
    fn interrupted_restage_rejects_newer_selection_and_session_epochs() {
        let recovery = InterruptedRestage::interrupted(41, 9);

        assert!(!recovery.can_schedule(true, 0, 10));
        assert!(!recovery.can_schedule(true, 42, 9));
    }

    #[test]
    fn stream_slot_start_fixture_discards_a_candidate_after_demand_stops() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, replaced) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("production slot reserves a candidate")
                .expect("active demand admits a candidate");
        assert!(replaced.is_none());

        assert!(streams.set_capture_active(false));

        assert!(!streams.start_candidate_fixture(stage));
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
    }

    #[test]
    fn stream_slot_start_fixture_rejects_a_repick_before_the_old_start_runs() {
        let streams = stream_slot_fixture(41, 9);
        let (stale, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("first candidate reserves")
                .expect("first candidate stages");
        let (current, _) =
            reserve_selection_candidate_fixture(&streams, 43, MacosStreamRequest::default(), 43)
                .expect("replacement candidate reserves")
                .expect("replacement candidate stages");

        assert!(!streams.start_candidate_fixture(stale));
        assert!(streams.start_candidate_fixture(current));
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(43));
    }

    #[test]
    fn candidate_start_gate_blocks_deactivation_until_native_start_is_invoked() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("candidate reservation succeeds")
                .expect("active capture admits a candidate");
        let invoked = Arc::new(AtomicBool::new(false));
        let (installed_tx, installed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let starter_streams = Arc::clone(&streams);
        let starter_invoked = Arc::clone(&invoked);
        let starter = thread::spawn(move || {
            starter_streams.start_candidate_fixture_with(stage, move || {
                installed_tx
                    .send(())
                    .expect("installed candidate should be observable");
                release_rx
                    .recv()
                    .expect("native start invocation should be released");
                starter_invoked.store(true, Ordering::Release);
            })
        });
        installed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("candidate should install before the injected invocation pauses");

        let (deactivate_started_tx, deactivate_started_rx) = mpsc::channel();
        let (deactivate_done_tx, deactivate_done_rx) = mpsc::channel();
        let deactivate_streams = Arc::clone(&streams);
        let deactivate_invoked = Arc::clone(&invoked);
        let deactivate = thread::spawn(move || {
            deactivate_started_tx
                .send(())
                .expect("deactivation attempt should be observable");
            let changed = deactivate_streams.set_capture_active(false);
            deactivate_done_tx
                .send((changed, deactivate_invoked.load(Ordering::Acquire)))
                .expect("deactivation result should be observable");
        });
        deactivate_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("deactivation should reach the lifecycle gate");
        assert_eq!(
            deactivate_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));

        release_tx
            .send(())
            .expect("native start invocation should resume");
        assert!(starter.join().expect("starter thread should join"));
        assert_eq!(
            deactivate_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("deactivation should finish after invocation"),
            (true, true)
        );
        deactivate.join().expect("deactivation thread should join");
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
    }

    #[test]
    fn candidate_start_gate_blocks_repick_until_native_start_is_invoked() {
        let streams = stream_slot_fixture(41, 9);
        let (stage, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("candidate reservation succeeds")
                .expect("active capture admits a candidate");
        let invoked = Arc::new(AtomicBool::new(false));
        let (installed_tx, installed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let starter_streams = Arc::clone(&streams);
        let starter_invoked = Arc::clone(&invoked);
        let starter = thread::spawn(move || {
            starter_streams.start_candidate_fixture_with(stage, move || {
                installed_tx
                    .send(())
                    .expect("installed candidate should be observable");
                release_rx
                    .recv()
                    .expect("native start invocation should be released");
                starter_invoked.store(true, Ordering::Release);
            })
        });
        installed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("candidate should install before the injected invocation pauses");

        let (repick_started_tx, repick_started_rx) = mpsc::channel();
        let (repick_done_tx, repick_done_rx) = mpsc::channel();
        let repick_streams = Arc::clone(&streams);
        let repick_invoked = Arc::clone(&invoked);
        let repick = thread::spawn(move || {
            repick_started_tx
                .send(())
                .expect("repick attempt should be observable");
            let (replacement, retired) = reserve_selection_candidate_fixture(
                &repick_streams,
                43,
                MacosStreamRequest::default(),
                43,
            )
            .expect("repick reservation succeeds")
            .expect("active capture admits the repick");
            assert!(retired.is_none());
            repick_done_tx
                .send((replacement.epoch, repick_invoked.load(Ordering::Acquire)))
                .expect("repick result should be observable");
        });
        repick_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("repick should reach the lifecycle gate");
        assert_eq!(
            repick_done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));

        release_tx
            .send(())
            .expect("native start invocation should resume");
        assert!(starter.join().expect("starter thread should join"));
        assert_eq!(
            repick_done_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("repick should finish after invocation"),
            (43, true)
        );
        repick.join().expect("repick thread should join");
        assert_eq!(super::lock(&streams.state).staging_epoch, Some(43));
    }

    #[test]
    fn stale_async_start_failure_cannot_retire_the_successor_candidate() {
        let streams = stream_slot_fixture(41, 9);
        let (diagnostic, diagnostic_completion) = streams
            .shared
            .begin_restart_diagnostic(true, 9)
            .expect("diagnostic attempt begins");
        streams.shared.record_filter_enumerated(&diagnostic, 42);
        let (callback_blocked_tx, callback_blocked_rx) = mpsc::channel();
        let (release_callback_tx, release_callback_rx) = mpsc::channel();
        streams.lifecycle_callbacks.exec_async(move || {
            callback_blocked_tx
                .send(())
                .expect("blocked lifecycle callback should be observable");
            release_callback_rx
                .recv()
                .expect("lifecycle callback should be released");
        });
        callback_blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle callback queue should pause");
        let (stale, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("stale candidate reservation succeeds")
                .expect("active capture admits the stale candidate");
        let failure_streams = Arc::clone(&streams);
        let failure_shared = Arc::clone(&streams.shared);
        assert!(streams.start_candidate_fixture_with(stale, move || {
            super::dispatch_owned_stream_error(
                failure_streams,
                42,
                failure_shared,
                MacosProtectedSourceState::PermissionDenied,
                MacosCaptureError::CaptureWorkerStartFailed(
                    "stale injected start failure".to_owned(),
                ),
            );
        }));
        assert!(streams.set_capture_active(false));
        assert!(streams.set_capture_active(true));

        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("successor request is valid");
        let (pending, completion) = pending_request(43, next);
        let (successor, _) = reserve_request_candidate_fixture(&streams, 43, next, pending)
            .expect("successor reservation succeeds")
            .expect("reactivated capture admits the successor");
        assert!(streams.start_candidate_fixture(successor));

        release_callback_tx
            .send(())
            .expect("stale start completion should resume");
        streams.drain_lifecycle_callbacks();
        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 42);
        streams.drain_lifecycle_callbacks();

        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(43));
        assert_eq!(streams.request(), next);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(
            diagnostic_completion.try_recv(),
            Ok(MacosProtectedSourceState::Failed)
        );
        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(completion.recv(), Ok(Ok(())));
        streams
            .shared
            .fail_restart_diagnostic_attempt(diagnostic.attempt);
        assert_eq!(
            diagnostic_completion.try_recv(),
            Ok(MacosProtectedSourceState::Failed)
        );
    }

    #[test]
    fn deactivation_retires_diagnostic_before_queued_candidate_completion() {
        let streams = stream_slot_fixture(41, 9);
        let (diagnostic, diagnostic_completion) = streams
            .shared
            .begin_restart_diagnostic(true, 9)
            .expect("diagnostic attempt should begin");
        streams.shared.record_filter_enumerated(&diagnostic, 42);

        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        streams.lifecycle_callbacks.exec_async(move || {
            blocked_tx
                .send(())
                .expect("queued completion pause should be observable");
            release_rx.recv().expect("queued completion should resume");
        });
        blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle queue should pause before candidate completion");

        let (candidate, _) =
            reserve_selection_candidate_fixture(&streams, 42, MacosStreamRequest::default(), 42)
                .expect("diagnostic candidate should reserve")
                .expect("active capture should admit the diagnostic candidate");
        let callback_streams = Arc::clone(&streams);
        let callback_shared = Arc::clone(&streams.shared);
        assert!(streams.start_candidate_fixture_with(candidate, move || {
            super::dispatch_owned_stream_error(
                callback_streams,
                42,
                callback_shared,
                MacosProtectedSourceState::PermissionDenied,
                MacosCaptureError::CaptureWorkerStartFailed(
                    "queued diagnostic candidate completion".to_owned(),
                ),
            );
        }));

        assert!(streams.set_capture_active(false));
        assert_eq!(
            diagnostic_completion
                .recv()
                .expect("deactivation should terminally complete the diagnostic"),
            MacosProtectedSourceState::Failed
        );
        streams
            .shared
            .publish(MacosFrameEvent::Lifecycle(MacosFrameStatus::Stopped));

        release_tx
            .send(())
            .expect("stale candidate completion should resume");
        streams.drain_lifecycle_callbacks();
        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 42);
        streams.drain_lifecycle_callbacks();

        assert!(!streams.shared.capture_active());
        assert_eq!(streams.shared.current_epoch(), 0);
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
        assert!(matches!(
            streams.shared.mailbox.take_latest(),
            Some(Ok(MacosFrameEvent::Lifecycle(MacosFrameStatus::Stopped)))
        ));
    }

    fn assert_failure_before_activation_rejects_the_candidate(
        dispatch_failure: impl FnOnce(&Arc<StreamSlot>, Arc<SessionShared>, MacosCaptureError),
    ) {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("candidate request is valid");
        let streams = stream_slot_fixture(41, 9);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("candidate reservation succeeds")
            .expect("active capture admits the candidate");
        assert!(streams.start_candidate_fixture(stage));

        let (callback_blocked_tx, callback_blocked_rx) = mpsc::channel();
        let (release_callback_tx, release_callback_rx) = mpsc::channel();
        streams.lifecycle_callbacks.exec_async(move || {
            callback_blocked_tx
                .send(())
                .expect("blocked lifecycle callback should be observable");
            release_callback_rx
                .recv()
                .expect("lifecycle callback should be released");
        });
        callback_blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle callback queue should pause");

        dispatch_failure(
            &streams,
            Arc::clone(&streams.shared),
            MacosCaptureError::CaptureWorkerStartFailed(
                "injected candidate failure before activation".to_owned(),
            ),
        );

        assert!(!streams.accepts_epoch(42));
        assert!(!streams.activate_candidate_fixture(42));
        assert_eq!(streams.committed_request(), original);
        assert_eq!(streams.shared.current_epoch(), 41);
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));

        release_callback_tx
            .send(())
            .expect("queued teardown should resume");
        streams.drain_lifecycle_callbacks();

        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(streams.shared.current_epoch(), 41);
        assert!(matches!(completion.recv(), Ok(Err(_))));
    }

    #[test]
    fn start_failure_before_activation_rejects_the_exact_candidate_synchronously() {
        assert_failure_before_activation_rejects_the_candidate(|streams, shared, error| {
            super::dispatch_owned_stream_error(
                Arc::clone(streams),
                42,
                shared,
                MacosProtectedSourceState::PermissionDenied,
                error,
            );
        });
    }

    #[test]
    fn fatal_failure_before_activation_rejects_the_exact_candidate_synchronously() {
        assert_failure_before_activation_rejects_the_candidate(|streams, shared, error| {
            super::handle_fatal_stream_error(&Arc::downgrade(streams), 42, shared, error);
        });
    }

    #[test]
    fn stream_slot_start_fixture_never_regresses_a_newer_live_session_to_interrupted() {
        let streams = stream_slot_fixture(0, 9);
        let recovery = InterruptedRestage::interrupted(41, 9);

        assert!(recovery.can_begin(&super::lock(&streams.state), &streams.shared));
        streams.shared.activate_epoch(43);

        assert!(!recovery.can_begin(&super::lock(&streams.state), &streams.shared));
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Live);
    }

    #[test]
    fn pending_selection_request_after_repick_avoids_the_current_filter() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("request is valid");
        let streams = stream_slot_fixture(41, 9);
        let (repick, _) = reserve_selection_candidate_fixture(&streams, 42, original, 2)
            .expect("repick reserves")
            .expect("active capture stages the repick");
        assert!(streams.start_candidate_fixture(repick));
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((42, 2))));
        assert_eq!(streams.shared.current_epoch(), 41);

        streams.next_epoch.store(43, Ordering::Release);
        let (transaction, replaced) = streams
            .begin_request_candidate_fixture(next)
            .expect("request restages the repick selection");
        assert!(replaced.is_none());
        assert_eq!(transaction.generation(), 43);
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((43, 2))));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(!streams.activate_candidate_fixture(42));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(streams.shared.current_epoch(), 41);

        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(transaction.recv(), Ok(Ok(())));
        assert_eq!(selection_filter_ids(&streams), (Some(2), None));
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.current_epoch(), 43);
    }

    #[test]
    fn pending_selection_request_after_first_candidate_keeps_the_only_filter() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("request is valid");
        let streams = stream_slot_fixture(0, 3);
        super::lock(&streams.state).selected_filter = None;
        let (first, _) = reserve_selection_candidate_fixture(&streams, 42, original, 7)
            .expect("first selection reserves")
            .expect("active capture stages the first selection");
        assert!(streams.start_candidate_fixture(first));
        assert_eq!(selection_filter_ids(&streams), (None, Some((42, 7))));

        streams.next_epoch.store(43, Ordering::Release);
        let (transaction, replaced) = streams
            .begin_request_candidate_fixture(next)
            .expect("request restages the only selection");
        assert!(replaced.is_none());
        assert_eq!(selection_filter_ids(&streams), (None, Some((43, 7))));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(!streams.activate_candidate_fixture(42));

        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(transaction.recv(), Ok(Ok(())));
        assert_eq!(selection_filter_ids(&streams), (Some(7), None));
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.current_epoch(), 43);
    }

    #[test]
    fn pending_selection_request_fences_async_preinstall_ordering() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("request is valid");
        let streams = stream_slot_fixture(41, 9);
        let (uninstalled, _) = reserve_selection_candidate_fixture(&streams, 42, original, 8)
            .expect("async selection reserves")
            .expect("active capture stages the async selection");
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((42, 8))));

        streams.next_epoch.store(43, Ordering::Release);
        let (transaction, replaced) = streams
            .begin_request_candidate_fixture(next)
            .expect("request supersedes the pre-install stage");
        assert!(replaced.is_none());
        assert_eq!(selection_filter_ids(&streams), (Some(1), Some((43, 8))));
        assert!(!streams.start_candidate_fixture(uninstalled));
        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));

        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(transaction.recv(), Ok(Ok(())));
        assert_eq!(selection_filter_ids(&streams), (Some(8), None));
        assert_eq!(streams.committed_request(), next);
    }

    #[test]
    fn stream_slot_request_restage_commits_only_at_candidate_activation() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(9, next);

        let (stage, replaced) = reserve_request_candidate_fixture(&streams, 9, next, pending)
            .expect("request restage should reserve")
            .expect("active request should stage a candidate");
        assert!(replaced.is_none());
        assert_eq!(streams.request(), next);
        assert_eq!(super::lock(&streams.state).request, original);
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );

        assert!(streams.start_candidate_fixture(stage));
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        assert!(streams.activate_candidate_fixture(stage.epoch));
        assert_eq!(completion.recv(), Ok(Ok(())));

        let state = super::lock(&streams.state);
        assert_eq!(state.request, next);
        assert!(state.pending_request.is_none());
        assert_eq!(state.candidate_epoch, None);
    }

    #[test]
    fn picker_replacement_retargets_the_pending_request_transaction() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));

        let (picker_stage, replaced) = reserve_selection_candidate_fixture(&streams, 43, next, 43)
            .expect("picker replacement reserves with the authoritative request")
            .expect("active capture stages the picker replacement");
        assert!(replaced.is_none());
        assert_eq!(picker_stage.request.map(|request| request.epoch), Some(43));
        assert_eq!(streams.request(), next);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));

        assert!(!streams.fail_candidate_fixture(
            42,
            MacosCaptureError::CaptureWorkerStartFailed(
                "stale replaced candidate failed".to_owned(),
            )
        ));
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(streams.start_candidate_fixture(picker_stage));
        assert!(streams.activate_candidate_fixture(43));
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.committed_request(), next);
    }

    #[test]
    fn stale_resolution_snapshot_cannot_displace_the_pending_request_transaction() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));
        let selection_revision = streams.selection_revision();

        let error = match reserve_selection_candidate_fixture(&streams, 43, original, 43) {
            Ok(_) => panic!("stale resolution snapshot must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("authoritative stream request"));
        assert_eq!(streams.selection_revision(), selection_revision);
        assert_eq!(super::lock(&streams.state).candidate_epoch, Some(42));
        assert_eq!(streams.request(), next);
        assert_eq!(streams.committed_request(), original);
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));

        let (retry, _) = reserve_selection_candidate_fixture(&streams, 44, next, 44)
            .expect("resolution retries with the authoritative pending request")
            .expect("retry stages a replacement candidate");
        assert!(!streams.fail_candidate_fixture(
            42,
            MacosCaptureError::CaptureWorkerStartFailed(
                "stale request candidate failed".to_owned(),
            )
        ));
        assert_eq!(completion.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(streams.start_candidate_fixture(retry));
        assert!(streams.activate_candidate_fixture(44));
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.committed_request(), next);
    }

    #[test]
    fn stale_resolution_snapshot_after_request_commit_cannot_replace_the_committed_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));
        assert!(streams.activate_candidate_fixture(42));
        assert_eq!(completion.recv(), Ok(Ok(())));

        let selection_revision = streams.selection_revision();
        let current_epoch = streams.shared.current_epoch();
        let error = match reserve_selection_candidate_fixture(&streams, 43, original, 43) {
            Ok(_) => panic!("post-commit stale resolution snapshot must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("authoritative stream request"));
        assert_eq!(streams.selection_revision(), selection_revision);
        assert_eq!(streams.shared.current_epoch(), current_epoch);
        {
            let state = super::lock(&streams.state);
            assert_eq!(state.request, next);
            assert!(state.pending_request.is_none());
            assert_eq!(state.staging_epoch, None);
            assert_eq!(state.candidate_epoch, None);
        }

        let (retry, replaced) = reserve_selection_candidate_fixture(&streams, 44, next, 44)
            .expect("resolution retries with the committed request")
            .expect("retry stages a replacement candidate");
        assert!(replaced.is_none());
        assert!(streams.start_candidate_fixture(retry));
        assert!(streams.activate_candidate_fixture(44));
        assert_eq!(streams.committed_request(), next);
        assert_eq!(streams.shared.current_epoch(), 44);
    }

    #[test]
    fn stale_resolution_snapshot_after_request_rollback_cannot_replace_the_committed_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("pending request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(42, next);
        let (request_stage, _) = reserve_request_candidate_fixture(&streams, 42, next, pending)
            .expect("request candidate reserves")
            .expect("active capture stages the request candidate");
        assert!(streams.start_candidate_fixture(request_stage));
        let failure =
            MacosCaptureError::CaptureWorkerStartFailed("fixture request failure".to_owned());
        assert!(streams.fail_candidate_fixture(42, failure.clone()));
        assert_eq!(completion.recv(), Ok(Err(failure)));

        let selection_revision = streams.selection_revision();
        let current_epoch = streams.shared.current_epoch();
        let error = match reserve_selection_candidate_fixture(&streams, 43, next, 43) {
            Ok(_) => panic!("post-rollback stale resolution snapshot must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("authoritative stream request"));
        assert_eq!(streams.selection_revision(), selection_revision);
        assert_eq!(streams.shared.current_epoch(), current_epoch);
        {
            let state = super::lock(&streams.state);
            assert_eq!(state.request, original);
            assert!(state.pending_request.is_none());
            assert_eq!(state.staging_epoch, None);
            assert_eq!(state.candidate_epoch, None);
        }

        let (retry, replaced) = reserve_selection_candidate_fixture(&streams, 44, original, 44)
            .expect("resolution retries with the rolled-back committed request")
            .expect("retry stages a replacement candidate");
        assert!(replaced.is_none());
        assert!(streams.start_candidate_fixture(retry));
        assert!(streams.activate_candidate_fixture(44));
        assert_eq!(streams.committed_request(), original);
        assert_eq!(streams.shared.current_epoch(), 44);
    }

    #[test]
    fn stream_slot_request_restage_failure_rolls_back_pending_request() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
            .expect("fixture HDR request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(12, next);

        let (stage, replaced) = reserve_request_candidate_fixture(&streams, 12, next, pending)
            .expect("request restage should reserve")
            .expect("active request should stage a candidate");
        assert!(replaced.is_none());
        assert!(streams.start_candidate_fixture(stage));
        let error = MacosCaptureError::CaptureWorkerStartFailed("fixture async failure".to_owned());
        assert!(streams.fail_candidate_fixture(stage.epoch, error.clone()));
        assert_eq!(completion.recv(), Ok(Err(error)));

        let state = super::lock(&streams.state);
        assert_eq!(state.request, original);
        assert!(state.pending_request.is_none());
        assert_eq!(state.staging_epoch, None);
        assert_eq!(state.candidate_epoch, None);
    }

    #[test]
    fn missing_start_completion_times_out_without_retiring_the_current_stream() {
        let original = MacosStreamRequest::default();
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        streams.next_epoch.store(12, Ordering::Release);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(next)
            .expect("candidate transaction starts");
        let deadline = transaction
            .current_deadline()
            .expect("start transaction has a deadline");

        streams
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);

        assert_eq!(
            transaction.wait(),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::StreamStart,
                generation: 12,
            })
        );
        let state = super::lock(&streams.state);
        assert_eq!(StreamSlot::current_epoch(&state), Some(7));
        assert_eq!(state.candidate_epoch, None);
        assert_eq!(state.request, original);
    }

    #[test]
    fn missing_source_callback_times_out_and_fences_the_exact_resolution() {
        let streams = stream_slot_fixture(7, 3);
        let resolution = streams
            .begin_resolution()
            .expect("general source resolution starts");
        let completion = super::lock(&streams.source_transaction)
            .as_ref()
            .expect("source transaction is installed")
            .completion
            .clone();
        let deadline = completion
            .current_deadline()
            .expect("general source resolution is bounded");

        streams
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);

        assert!(!completion.is_open());
        assert!(!streams.shared.source_resolution_is_current(&resolution));
        assert!(super::lock(&streams.source_transaction).is_none());
        assert_eq!(streams.shared.current_epoch(), 7);
    }

    #[test]
    fn picker_selection_has_cancellation_without_a_wall_clock_deadline() {
        let streams = stream_slot_fixture(7, 3);
        let resolution = streams
            .begin_picker_resolution()
            .expect("picker resolution starts");
        let completion = super::lock(&streams.source_transaction)
            .as_ref()
            .expect("picker transaction is installed")
            .completion
            .clone();

        assert_eq!(completion.current_deadline(), None);
        assert!(completion.is_open());
        let settlement = streams.cancel_source_transaction(&resolution);
        settlement
            .expect("picker cancellation claims the source transaction")
            .publish();

        assert!(!completion.is_open());
        assert!(super::lock(&streams.source_transaction).is_none());
    }

    #[test]
    fn source_success_remains_unpublished_until_resolution_commit() {
        let streams = stream_slot_fixture(7, 3);
        let resolution = streams
            .begin_picker_resolution()
            .expect("picker resolution starts");
        let completion = super::lock(&streams.source_transaction)
            .as_ref()
            .expect("source transaction is installed")
            .completion
            .clone();

        let settlement = streams
            .claim_source_transaction(&resolution)
            .expect("source callback claims success");
        assert_eq!(completion.outcome(), None);
        assert!(super::lock(&streams.source_transaction).is_none());

        streams
            .shared
            .set_status(MacosProtectedSourceState::ReadyIdle);
        assert_eq!(completion.outcome(), None);
        settlement.publish();

        assert_eq!(completion.outcome(), Some(Ok(())));
        assert_eq!(
            streams.shared.status(),
            MacosProtectedSourceState::ReadyIdle
        );
    }

    #[test]
    fn missing_first_complete_frame_rearms_and_times_out_the_candidate() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        streams.next_epoch.store(12, Ordering::Release);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(next)
            .expect("candidate transaction starts");

        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 12);
        streams.drain_lifecycle_callbacks();
        let deadline = transaction
            .current_deadline()
            .expect("first frame transaction has a deadline");
        streams
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);

        assert_eq!(
            transaction.wait(),
            Err(MacosNativeTransactionError::TimedOut {
                phase: MacosNativeTransactionPhase::FirstCompleteFrame,
                generation: 12,
            })
        );
        assert_eq!(streams.shared.current_epoch(), 7);
        assert_eq!(super::lock(&streams.state).candidate_epoch, None);
    }

    #[test]
    fn observed_start_callback_retires_start_deadline_before_lifecycle_queue_delivery() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        streams.next_epoch.store(12, Ordering::Release);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(next)
            .expect("candidate transaction starts");
        let stale_start_deadline = transaction
            .current_deadline()
            .expect("start transaction has a deadline");
        let (blocked_tx, blocked_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        streams.lifecycle_callbacks.exec_async(move || {
            blocked_tx
                .send(())
                .expect("lifecycle queue block is observable");
            release_rx
                .recv()
                .expect("lifecycle queue block is released");
        });
        blocked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("lifecycle queue is blocked");

        super::dispatch_stream_start_success(&Arc::downgrade(&streams), 12);
        streams
            .native_lifecycle
            .deadlines()
            .expire_through(stale_start_deadline);

        assert_eq!(transaction.try_recv(), Err(mpsc::TryRecvError::Empty));
        release_tx.send(()).expect("lifecycle queue should resume");
        streams.drain_lifecycle_callbacks();
        assert!(streams.activate_candidate_fixture(12));
        assert_eq!(transaction.wait(), Ok(()));
    }

    #[test]
    fn first_frame_and_timeout_commit_exactly_one_candidate_result() {
        let next = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let winner = stream_slot_fixture(7, 3);
        winner.next_epoch.store(12, Ordering::Release);
        let (committed, _) = winner
            .begin_request_candidate_fixture(next)
            .expect("winning candidate starts");
        let stale_deadline = committed
            .current_deadline()
            .expect("winning candidate has a deadline");
        assert!(winner.activate_candidate_fixture(12));
        winner
            .native_lifecycle
            .deadlines()
            .expire_through(stale_deadline);
        assert_eq!(committed.wait(), Ok(()));
        assert_eq!(winner.shared.current_epoch(), 12);

        let timed_out = stream_slot_fixture(7, 3);
        timed_out.next_epoch.store(12, Ordering::Release);
        let (rejected, _) = timed_out
            .begin_request_candidate_fixture(next)
            .expect("losing candidate starts");
        let deadline = rejected
            .current_deadline()
            .expect("losing candidate has a deadline");
        timed_out
            .native_lifecycle
            .deadlines()
            .expire_through(deadline);
        assert!(!timed_out.activate_candidate_fixture(12));
        assert!(matches!(
            rejected.wait(),
            Err(MacosNativeTransactionError::TimedOut { .. })
        ));
        assert_eq!(timed_out.shared.current_epoch(), 7);
    }

    #[test]
    fn claimed_cancellation_retires_only_the_candidate_before_publishing() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let cancel_selected = Arc::new(std::sync::Barrier::new(2));
        let selected = Arc::clone(&cancel_selected);
        let resume_cancel = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::clone(&resume_cancel);
        let cancel_streams = Arc::clone(&streams);
        completion.set_cancel(move |generation| {
            selected.wait();
            resume.wait();
            cancel_streams.cancel_candidate_transaction(generation);
        });
        let cancel = thread::spawn(move || transaction.cancel());
        cancel_selected.wait();

        assert_eq!(completion.current_deadline(), None);
        assert!(!completion.has_deadline_ticket());
        assert_eq!(completion.outcome(), None);
        assert!(!streams.activate_candidate_fixture(epoch));
        assert_eq!(streams.shared.current_epoch(), 7);

        resume_cancel.wait();
        assert!(cancel.join().expect("cancellation attempt exits"));
        assert!(matches!(
            completion.outcome(),
            Some(Err(MacosNativeTransactionError::Cancelled { .. }))
        ));

        let state = super::lock(&streams.state);
        assert_eq!(state.fixture_current_epoch, Some(7));
        assert_eq!(state.fixture_candidate_epoch, None);
        assert_eq!(state.candidate_epoch, None);
        assert!(state.candidate_completion.is_none());
        assert!(state.pending_request.is_none());
        assert_eq!(state.request, MacosStreamRequest::default());
        drop(state);
        assert_eq!(streams.shared.current_epoch(), 7);
    }

    #[test]
    fn successful_claim_wakes_only_after_current_and_first_publication_commit() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let published = Arc::new(AtomicBool::new(false));
        let observed_publication = Arc::clone(&published);
        let observer_streams = Arc::clone(&streams);
        let (observed_tx, observed_rx) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            let result = transaction.wait();
            let state = super::lock(&observer_streams.state);
            observed_tx
                .send((
                    result,
                    observer_streams.shared.current_epoch(),
                    state.fixture_current_epoch,
                    state.pending_selection.is_none(),
                    state.request,
                    observed_publication.load(Ordering::Acquire),
                ))
                .expect("waiter observation is delivered");
        });
        let claim_reached = Arc::new(std::sync::Barrier::new(2));
        let claimed = Arc::clone(&claim_reached);
        let resume_commit = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::clone(&resume_commit);
        let publish_streams = Arc::clone(&streams);
        let publish_flag = Arc::clone(&published);
        let publisher = thread::spawn(move || {
            publish_streams.publish_decoded_event_with_claim_hook(
                epoch,
                sdr_delivery_fixture(),
                move || {
                    claimed.wait();
                    resume.wait();
                },
                move || publish_flag.store(true, Ordering::Release),
            )
        });
        claim_reached.wait();

        assert_eq!(completion.current_deadline(), None);
        assert!(!completion.has_deadline_ticket());
        assert_eq!(completion.outcome(), None);
        assert_eq!(observed_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert!(!completion.cancel());

        resume_commit.wait();
        assert!(publisher.join().expect("first publication exits"));
        let observation = observed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("successful transaction wakes after publication");
        waiter.join().expect("request waiter exits");

        assert_eq!(observation.0, Ok(()));
        assert_eq!(observation.1, epoch);
        assert_eq!(observation.2, Some(epoch));
        assert!(observation.3);
        assert_eq!(observation.4, request);
        assert!(observation.5);
        assert!(!completion.is_open());
    }

    #[test]
    fn panic_after_success_claim_cleans_candidate_before_failure_publication() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            streams.publish_decoded_event_with_claim_hook(
                epoch,
                sdr_delivery_fixture(),
                || panic!("abort after reserving transaction success"),
                || panic!("publication must not run after claim abort"),
            )
        }));

        assert!(unwind.is_err());
        assert!(matches!(
            transaction.wait(),
            Err(MacosNativeTransactionError::Cancelled { .. })
        ));
        let state = super::lock(&streams.state);
        assert_eq!(state.fixture_current_epoch, Some(7));
        assert_eq!(state.fixture_candidate_epoch, None);
        assert_eq!(state.candidate_epoch, None);
        assert!(state.candidate_completion.is_none());
        assert!(state.pending_request.is_none());
        assert_eq!(state.request, MacosStreamRequest::default());
        drop(state);
        assert_eq!(streams.shared.current_epoch(), 7);
        assert!(!completion.has_deadline_ticket());
    }

    #[test]
    fn panic_before_first_publication_restores_prior_current_before_failure_wakes() {
        let request = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        let (transaction, _) = streams
            .begin_request_candidate_fixture(request)
            .expect("request candidate starts");
        let epoch = transaction.generation();
        let completion = super::lock(&streams.state)
            .candidate_completion
            .as_ref()
            .cloned()
            .expect("candidate completion is installed");
        let previous_status = streams.shared.status();
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            streams.publish_decoded_event_with(epoch, true, Some(sdr_delivery_fixture()), || {
                panic!("abort before first publication commits")
            })
        }));

        assert!(unwind.is_err());
        assert!(matches!(
            transaction.wait(),
            Err(MacosNativeTransactionError::Cancelled { .. })
        ));
        let state = super::lock(&streams.state);
        assert_eq!(state.fixture_current_epoch, Some(7));
        assert_eq!(state.fixture_candidate_epoch, None);
        assert_eq!(state.candidate_epoch, None);
        assert_eq!(state.request, MacosStreamRequest::default());
        assert!(state.pending_selection.is_none());
        assert!(state.pending_request.is_none());
        drop(state);
        assert_eq!(streams.shared.current_epoch(), 7);
        assert_eq!(streams.shared.status(), previous_status);
        assert!(!completion.has_deadline_ticket());
    }

    #[test]
    fn stream_slot_serializes_request_transactions_while_a_candidate_is_pending() {
        let original = MacosStreamRequest::default();
        let first = MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
            .expect("first fixture request is valid");
        let second = MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
            .expect("second fixture request is valid");
        let streams = stream_slot_fixture(7, 3);
        super::lock(&streams.state).request = original;
        let (pending, completion) = pending_request(12, first);
        let (stage, _) = reserve_request_candidate_fixture(&streams, 12, first, pending)
            .expect("first request reserves")
            .expect("first request stages");
        assert!(streams.start_candidate_fixture(stage));
        let reserve_pool: super::PoolReservationFactory =
            Arc::new(|_, _| -> Result<PoolObservation, MacosCaptureError> {
                unreachable!("serialized request never prepares another native stream")
            });

        let error = match streams.set_request(second, &reserve_pool) {
            Ok(_) => panic!("a second request cannot overtake the pending transaction"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("still pending"));
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        assert!(streams.activate_candidate_fixture(stage.epoch));
        assert_eq!(completion.recv(), Ok(Ok(())));
        assert_eq!(streams.committed_request(), first);
    }

    #[test]
    fn repeated_activate_deactivate_cancels_every_pending_transaction() {
        let streams = stream_slot_fixture(7, 3);
        let requests = [
            MacosStreamRequest::new(MacosCaptureCadence::FramesPerSecond(30), false)
                .expect("first fixture request is valid"),
            MacosStreamRequest::new_hdr(MacosCaptureCadence::NativeRefresh, true)
                .expect("second fixture request is valid"),
        ];

        for request in requests {
            streams
                .begin_picker_resolution()
                .expect("picker resolution begins");
            let (transaction, _) = streams
                .begin_request_candidate_fixture(request)
                .expect("request candidate starts");
            assert!(transaction.current_deadline().is_some());

            assert!(streams.set_capture_active(false));
            assert!(transaction.current_deadline().is_none());
            assert!(matches!(
                transaction.wait(),
                Err(MacosNativeTransactionError::Cancelled { .. })
            ));
            assert!(super::lock(&streams.source_transaction).is_none());
            let state = super::lock(&streams.state);
            assert!(state.pending_request.is_none());
            assert!(state.candidate_completion.is_none());
            assert_eq!(state.candidate_epoch, None);
            assert_eq!(state.staging_epoch, None);
            drop(state);

            assert!(!streams.set_capture_active(false));
            assert!(streams.set_capture_active(true));
        }
    }

    #[test]
    fn timed_out_old_stop_error_cannot_degrade_a_live_successor() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Live,
            super::MacosCaptureSelector::Auto,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        shared.activate_epoch(41);
        shared.record_retirement_error(&MacosCaptureError::StreamStopCompletionLost);

        shared.activate_epoch(42);
        shared.set_status(MacosProtectedSourceState::Live);
        shared.record_retirement_error(&MacosCaptureError::CaptureWorkerStartFailed(
            "late stop callback failed".to_owned(),
        ));

        assert_eq!(shared.current_epoch(), 42);
        assert_eq!(shared.status(), MacosProtectedSourceState::Live);
        assert!(!shared.mailbox.has_pending());
        assert_eq!(shared.diagnostics().total_dropped(), 2);
    }

    #[test]
    fn restart_diagnostic_requires_grant_enumeration_and_stream_permission_failure() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (resolution, completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic attempt begins");
        shared.record_filter_enumerated(&resolution, 42);

        assert_eq!(
            shared.record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(
            completion.recv(),
            Ok(MacosProtectedSourceState::NeedsProcessRestart)
        );
    }

    #[test]
    fn restart_diagnostic_requires_its_exact_resolution_provenance() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (stale, stale_completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("first diagnostic begins");
        let (fresh, fresh_completion) = shared
            .begin_restart_diagnostic(true, 8)
            .expect("second diagnostic supersedes it");
        assert_eq!(
            stale_completion.recv(),
            Ok(MacosProtectedSourceState::Failed)
        );

        shared.record_non_stream_diagnostic_failure(&stale, MacosProtectedSourceState::Failed);
        assert_eq!(
            fresh_completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        shared.record_filter_enumerated(&fresh, 43);
        assert_eq!(
            shared.record_stream_diagnostic_result(43, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(
            fresh_completion.recv(),
            Ok(MacosProtectedSourceState::NeedsProcessRestart)
        );
    }

    #[test]
    fn claimed_diagnostic_cancellation_cannot_be_overwritten_by_stream_success() {
        let streams = stream_slot_fixture(0, 7);
        let (resolution, transaction) = streams
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic transaction begins");
        streams.shared.record_filter_enumerated(&resolution, 42);
        let completion = streams
            .shared
            .restart_diagnostic_completion(resolution.attempt)
            .expect("diagnostic completion remains active");
        let cancel_selected = Arc::new(std::sync::Barrier::new(2));
        let selected = Arc::clone(&cancel_selected);
        let resume_cancel = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::clone(&resume_cancel);
        let cancel_streams = Arc::clone(&streams);
        let attempt = resolution.attempt;
        completion.set_cancel(move |_| {
            selected.wait();
            resume.wait();
            cancel_streams.finish_restart_diagnostic(attempt);
        });
        let cancellation = thread::spawn(move || transaction.cancel());
        cancel_selected.wait();

        assert_eq!(completion.current_deadline(), None);
        assert_eq!(completion.outcome(), None);
        assert_eq!(
            streams
                .shared
                .record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied,),
            MacosProtectedSourceState::PermissionDenied
        );
        assert!(
            streams
                .shared
                .restart_diagnostic_completion(resolution.attempt)
                .is_some()
        );

        resume_cancel.wait();
        assert!(cancellation.join().expect("diagnostic cancellation exits"));
        assert!(matches!(
            completion.outcome(),
            Some(Err(MacosNativeTransactionError::Cancelled { .. }))
        ));
        assert!(
            streams
                .shared
                .restart_diagnostic_completion(resolution.attempt)
                .is_none()
        );
        assert_eq!(streams.shared.status(), MacosProtectedSourceState::Failed);
        assert!(!completion.has_deadline_ticket());
    }

    #[test]
    fn ordinary_resolution_supersedes_the_diagnostic_without_stranding_its_receiver() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (diagnostic, completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic begins");

        let ordinary = shared
            .begin_resolution()
            .expect("ordinary resolution begins");

        assert!(shared.resolution_is_current(ordinary));
        assert_eq!(completion.recv(), Ok(MacosProtectedSourceState::Failed));
        shared.record_filter_enumerated(&diagnostic, 42);
        assert_eq!(
            shared.record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::PermissionDenied
        );
    }

    #[test]
    fn primary_display_diagnostic_clears_picker_identity_before_enumeration() {
        let shared = Arc::new(SessionShared::new(
            MacosProtectedSourceState::ReadyIdle,
            super::MacosCaptureSelector::SessionScoped,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        ));
        shared.set_unconfirmed_selection(super::MacosCaptureSelection::SessionScoped {
            content_style: super::MacosCaptureContentStyle::Window,
        });
        let streams = StreamSlot::new(Arc::clone(&shared), MacosStreamRequest::default())
            .expect("fixture native lifecycle starts");
        {
            let mut state = super::lock(&streams.state);
            state.staging_epoch = Some(8);
            state.pending_request = Some(pending_request(8, MacosStreamRequest::default()).0);
        }

        streams
            .clear_selection()
            .expect("diagnostic reset clears prior selection state");
        shared.set_selector(super::MacosCaptureSelector::PrimaryDisplay);

        let state = super::lock(&streams.state);
        assert!(state.selected_filter.is_none());
        assert_eq!(state.staging_epoch, None);
        assert!(state.pending_request.is_none());
        assert_eq!(shared.selection(), super::MacosCaptureSelection::None);
        assert_eq!(
            shared.selector(),
            super::MacosCaptureSelector::PrimaryDisplay
        );
    }

    #[test]
    fn old_stream_completion_cannot_satisfy_primary_display_diagnostic() {
        let shared = SessionShared::new(
            MacosProtectedSourceState::Starting,
            super::MacosCaptureSelector::PrimaryDisplay,
            MacosTahoeCapabilities::from_probes(ABSENT_TAHOE_PROBES),
        );
        let (resolution, completion) = shared
            .begin_restart_diagnostic(true, 7)
            .expect("diagnostic begins");
        shared.record_filter_enumerated(&resolution, 42);

        assert_eq!(
            shared.record_stream_diagnostic_result(41, MacosProtectedSourceState::ReadyIdle),
            MacosProtectedSourceState::ReadyIdle
        );
        assert_eq!(
            completion.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        );
        assert_eq!(
            shared.record_stream_diagnostic_result(42, MacosProtectedSourceState::PermissionDenied),
            MacosProtectedSourceState::NeedsProcessRestart
        );
        assert_eq!(
            completion.recv(),
            Ok(MacosProtectedSourceState::NeedsProcessRestart)
        );
    }

    #[test]
    fn missing_arm64_and_translation_sysctls_resolve_native_intel_sdr() {
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Missing),
            Ok(SysctlI32Value::Missing),
            ABSENT_TAHOE_PROBES,
        )
        .expect("missing Apple Silicon sysctls identify a native Intel host");

        assert_eq!(capabilities.host_architecture, MacosHostArchitecture::Intel);
        assert!(!capabilities.translated_process);
        assert_eq!(
            capabilities.validate_dynamic_range(MacosCaptureDynamicRange::Sdr),
            Ok(())
        );
        assert_eq!(
            capabilities.validate_dynamic_range(MacosCaptureDynamicRange::Hdr),
            Err(MacosStreamDeliveryRejection::UnsupportedIntelHdr)
        );
    }

    #[test]
    fn translated_process_resolves_the_native_apple_silicon_host() {
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Missing),
            Ok(SysctlI32Value::Present(1)),
            ABSENT_TAHOE_PROBES,
        )
        .expect("translation is direct evidence of an Apple Silicon host");

        assert_eq!(
            capabilities.host_architecture,
            MacosHostArchitecture::AppleSilicon
        );
        assert!(capabilities.translated_process);
        assert_eq!(
            capabilities.validate_dynamic_range(MacosCaptureDynamicRange::Hdr),
            Ok(())
        );
    }

    #[test]
    fn nonmissing_sysctl_failures_remain_typed() {
        assert_eq!(
            capture_capabilities_from_probes(
                Err(MacosCaptureError::CapabilityProbeFailed(
                    "hw.optional.arm64"
                )),
                Ok(SysctlI32Value::Missing),
                ABSENT_TAHOE_PROBES,
            ),
            Err(MacosCaptureError::CapabilityProbeFailed(
                "hw.optional.arm64"
            ))
        );
    }

    #[test]
    fn partial_tahoe_runtime_surfaces_fail_closed_per_capability() {
        let screenshot_only = MacosTahoeRuntimeProbes {
            screenshot_configuration_class: MacosRuntimeCapability::Present,
            screenshot_dynamic_range_selector: MacosRuntimeCapability::Present,
            screenshot_capture_selector: MacosRuntimeCapability::Present,
            ..ABSENT_TAHOE_PROBES
        };
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Present(1)),
            Ok(SysctlI32Value::Missing),
            screenshot_only,
        )
        .expect("independent Tahoe capability probes should not disable capture");

        assert_eq!(
            capabilities.tahoe.content_tone_mapping_info,
            MacosRuntimeCapability::Absent
        );
        assert_eq!(
            capabilities.tahoe.screenshot_api,
            MacosRuntimeCapability::Present
        );

        let incomplete_screenshot = MacosTahoeRuntimeProbes {
            screenshot_configuration_class: MacosRuntimeCapability::Present,
            ..ABSENT_TAHOE_PROBES
        };
        let capabilities = capture_capabilities_from_probes(
            Ok(SysctlI32Value::Present(1)),
            Ok(SysctlI32Value::Missing),
            incomplete_screenshot,
        )
        .expect("an incomplete diagnostic surface should not disable streaming");
        assert_eq!(
            capabilities.tahoe.screenshot_api,
            MacosRuntimeCapability::Absent
        );
    }

    #[test]
    fn malformed_delivery_metadata_is_fatal_only_before_confirmation() {
        let configured = MacosConfiguredStream {
            requested_dynamic_range: MacosCaptureDynamicRange::Sdr,
            requested_preset: MacosStreamPreset::SdrDefault,
            configured_dynamic_range: MacosCaptureDynamicRange::Sdr,
            configured_pixel_format: MacosCapturePixelFormat::Bgra8,
            configured_color_range: MacosColorRange::Full,
        };
        let rejection =
            MacosStreamDeliveryRejection::MissingOrInvalidDeliveryMetadata("dynamic_range");
        let mut awaiting = MacosStreamDeliveryValidator::new(configured);
        assert_eq!(
            classify_delivery_error(
                &mut awaiting,
                MacosCaptureError::StreamDeliveryRejected(rejection),
            ),
            MacosCaptureError::StreamDeliveryRejected(rejection)
        );
        assert_eq!(
            awaiting.state(),
            &MacosStreamDeliveryState::Rejected(rejection)
        );

        let delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Bgra8,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            None,
            None,
        )
        .expect("valid SDR delivery");
        let mut confirmed = MacosStreamDeliveryValidator::new(configured);
        confirmed
            .observe_first_complete(Some(delivered))
            .expect("matching delivery should confirm the stream");

        assert_eq!(
            classify_delivery_error(
                &mut confirmed,
                MacosCaptureError::StreamDeliveryRejected(rejection),
            ),
            MacosCaptureError::FrameDeliveryDropped(rejection)
        );
        assert!(matches!(
            confirmed.state(),
            MacosStreamDeliveryState::Confirmed(_)
        ));
    }

    #[test]
    fn session_selection_identity_is_canonical_and_membership_exact() {
        let window_ids = vec![41, 7, 41];
        let application_ids = vec![
            "tech.hyperbliss.zeta".to_owned(),
            "tech.hyperbliss.alpha".to_owned(),
            "tech.hyperbliss.zeta".to_owned(),
        ];

        assert_eq!(
            session_selection_source_id(
                super::MacosCaptureContentStyle::Mixed,
                window_ids,
                application_ids,
            )
            .as_ref(),
            "macos:session:mixed:w7:w41:a21:tech.hyperbliss.alpha:a20:tech.hyperbliss.zeta"
        );
    }

    #[test]
    fn repick_preserves_the_live_record_until_replacement_confirms() {
        let tahoe = MacosTahoeCapabilities {
            content_tone_mapping_info: MacosRuntimeCapability::Present,
            screenshot_api: MacosRuntimeCapability::Present,
        };
        let shared = SessionShared::new(
            MacosProtectedSourceState::Live,
            super::MacosCaptureSelector::Auto,
            tahoe,
        );
        let configured = MacosConfiguredStream {
            requested_dynamic_range: MacosCaptureDynamicRange::Sdr,
            requested_preset: MacosStreamPreset::SdrDefault,
            configured_dynamic_range: MacosCaptureDynamicRange::Sdr,
            configured_pixel_format: MacosCapturePixelFormat::Bgra8,
            configured_color_range: MacosColorRange::Full,
        };
        let delivered = MacosDeliveredFrameMetadata::new(
            MacosCapturePixelFormat::Bgra8,
            MacosCaptureColorimetry {
                primaries: MacosColorPrimaries::Srgb,
                transfer: MacosTransferFunction::Srgb,
                matrix: None,
                range: MacosColorRange::Full,
                chroma_location: None,
            },
            None,
            None,
        )
        .expect("valid SDR delivery");
        let delivery = MacosValidatedStreamDelivery {
            configured,
            delivered,
        };
        shared.confirm_selection(
            super::MacosCaptureSelection::Display {
                source_id: Arc::from("display:a"),
            },
            Arc::from("display:a"),
            1,
            delivery,
        );

        shared
            .begin_resolution()
            .expect("repick resolution should begin");
        assert!(shared.tahoe_selection_for("display:a", 1).is_some());

        shared.confirm_selection(
            super::MacosCaptureSelection::Display {
                source_id: Arc::from("display:b"),
            },
            Arc::from("display:b"),
            2,
            delivery,
        );
        assert_eq!(shared.tahoe_selection_for("display:a", 1), None);
        assert!(shared.tahoe_selection_for("display:b", 2).is_some());

        shared.clear_tahoe_selection();
        assert_eq!(shared.tahoe_selection_for("display:b", 2), None);
    }

    #[test]
    fn pending_screenshot_capability_dispatches_no_native_call() {
        let (snapshot, fence, backend) =
            screenshot_fixture(MacosScreenshotReferenceCapability::PendingFirstFrame);
        let result = execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(|_| panic!("pending capability must not complete asynchronously")),
        );

        assert_eq!(result, Err(MacosCaptureError::ScreenshotCapabilityPending));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn sdr_screenshot_dispatches_one_configuration() {
        let capability = MacosScreenshotReferenceCapability::SdrOnly {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("SDR transaction should start");
        assert_eq!(backend.calls(), vec![(7, MacosCaptureDynamicRange::Sdr)]);

        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        assert!(matches!(
            result_rx.recv().expect("SDR result should arrive"),
            Ok(MacosScreenshotReferenceSet::Sdr { .. })
        ));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn paired_screenshot_dispatches_exactly_two_ranges_on_one_filter() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("paired transaction should start");

        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        assert_eq!(backend.calls(), vec![(7, MacosCaptureDynamicRange::Hdr)]);
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Hdr,
            2,
        )));
        assert!(matches!(
            result_rx.recv().expect("paired result should arrive"),
            Ok(MacosScreenshotReferenceSet::Paired { .. })
        ));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn paired_screenshot_partial_failure_publishes_no_partial_set() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            fence,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("paired transaction should start");
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        backend.complete_next(Err(MacosCaptureError::NativeOperation {
            operation: "fixture HDR screenshot",
            code: 9,
            message: "redacted".to_owned(),
        }));

        assert!(matches!(
            result_rx.recv().expect("failure should arrive"),
            Err(MacosCaptureError::NativeOperation { code: 9, .. })
        ));
    }

    #[test]
    fn repick_between_paired_callbacks_rejects_the_complete_pair() {
        let capability = MacosScreenshotReferenceCapability::PairedSdrHdr {
            source_id: Arc::from("display:a"),
            generation: 4,
        };
        let (snapshot, fence, backend) = screenshot_fixture(capability);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        execute_screenshot_transaction(
            snapshot,
            Arc::clone(&fence) as Arc<dyn ScreenshotIdentityFence>,
            Arc::clone(&backend) as Arc<dyn ScreenshotCaptureBackend>,
            false,
            Box::new(move |result| result_tx.send(result).expect("receiver remains live")),
        )
        .expect("paired transaction should start");
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Sdr,
            1,
        )));
        super::lock(&fence.identity).2 = 12;
        backend.complete_next(Ok(MacosScreenshotReferenceImage::new_fixture(
            MacosCaptureDynamicRange::Hdr,
            2,
        )));

        assert!(matches!(
            result_rx.recv().expect("fence failure should arrive"),
            Err(MacosCaptureError::ScreenshotSelectionChanged)
        ));
    }

    #[test]
    fn canonical_hdr_preset_resolves_to_a_valid_hdr_configuration() {
        // SAFETY: The deployment floor includes this pure configuration
        // constructor, which does not start capture or request TCC access.
        let configuration = unsafe {
            SCStreamConfiguration::streamConfigurationWithPreset(
                SCStreamConfigurationPreset::CaptureHDRStreamCanonicalDisplay,
            )
        };
        // SAFETY: Both values are initialized scalar configuration properties.
        let configured = unsafe {
            let fourcc = configuration.pixelFormat();
            assert_eq!(
                configuration.captureDynamicRange(),
                SCCaptureDynamicRange::HDRCanonicalDisplay
            );
            MacosConfiguredStream {
                requested_dynamic_range: MacosCaptureDynamicRange::Hdr,
                requested_preset: MacosStreamPreset::CaptureHdrStreamCanonicalDisplay,
                configured_dynamic_range: capture_dynamic_range(
                    configuration.captureDynamicRange(),
                )
                .expect("preset dynamic range should decode"),
                configured_pixel_format: MacosCapturePixelFormat::from_fourcc(fourcc)
                    .expect("preset pixel format should be supported"),
                configured_color_range: color_range_from_fourcc(fourcc),
            }
        };
        configured
            .validate()
            .expect("canonical HDR preset should resolve to an accepted stream format");
    }

    #[test]
    fn conservative_bgra_pool_quote_covers_aligned_native_storage() {
        let extent = MacosPixelExtent::new(3_840, 2_160).expect("4K extent is valid");
        let quote = conservative_pool_quote(extent, MacosCapturePixelFormat::Bgra8)
            .expect("4K quote should fit");
        assert!(quote.per_surface_bytes >= 3_840 * 2_160 * 4);
        assert_eq!(quote.per_surface_bytes % (16 * 1024), 0);
        assert!(quote.stream_metadata_bytes > 0);
    }

    #[test]
    fn hdr_pool_quotes_cover_rgba16f_and_multiplane_storage() {
        let extent = MacosPixelExtent::new(3_840, 2_160).expect("4K extent is valid");
        let rgba = conservative_pool_quote(extent, MacosCapturePixelFormat::Rgba16Float)
            .expect("RGBA16F quote should fit");
        let yuv = conservative_pool_quote(extent, MacosCapturePixelFormat::Yuv420VideoRange)
            .expect("YUV quote should fit");
        assert!(rgba.per_surface_bytes >= 3_840 * 2_160 * 8);
        assert!(yuv.per_surface_bytes >= 3_840 * 2_160 * 3 / 2);
        assert_eq!(rgba.per_surface_bytes % (16 * 1024), 0);
        assert_eq!(yuv.per_surface_bytes % (16 * 1024), 0);
    }

    #[test]
    fn rejected_surface_never_reaches_the_retain_operation() {
        let pool = Arc::new(|_, _| -> Result<PoolBackingLifetime, MacosCaptureError> {
            Err(MacosCaptureError::ScreenResourceExhausted {
                requested_bytes: 128,
                available_bytes: 64,
            })
        }) as PoolObservation;
        let retained = AtomicBool::new(false);

        assert!(matches!(
            with_admitted_surface(&pool, 7, 128, |_| retained.store(true, Ordering::Release)),
            Err(MacosCaptureError::ScreenResourceExhausted { .. })
        ));
        assert!(!retained.load(Ordering::Acquire));
    }
}
