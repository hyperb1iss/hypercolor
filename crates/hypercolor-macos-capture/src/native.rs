use std::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

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
use crate::worker::{LatestSampleInput, LatestSampleWorker, SamplePublishOutcome};
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

type PoolBackingLifetime = Arc<dyn Send + Sync>;
type PoolObservation =
    Arc<dyn Fn(u32, u64) -> Result<PoolBackingLifetime, MacosCaptureError> + Send + Sync>;
type PoolReservationFactory =
    Arc<dyn Fn(u64, u64) -> Result<PoolObservation, MacosCaptureError> + Send + Sync>;

const MACOS_IOSURFACE_ROW_ALIGNMENT: u64 = 256;
const MACOS_IOSURFACE_ALLOCATION_ALIGNMENT: u64 = 16 * 1024;

#[derive(Debug)]
struct SessionShared {
    mailbox: MacosFrameMailbox,
    status: Mutex<MacosProtectedSourceState>,
    selection: Mutex<SessionSelectionState>,
    selector: Mutex<MacosCaptureSelector>,
    tahoe: MacosTahoeCapabilities,
    counters: CallbackCounters,
    capture_active: AtomicBool,
    current_epoch: AtomicU64,
    resolution_epoch: AtomicU64,
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
            current_epoch: AtomicU64::new(0),
            resolution_epoch: AtomicU64::new(0),
        }
    }

    fn status(&self) -> MacosProtectedSourceState {
        *lock(&self.status)
    }

    fn set_status(&self, status: MacosProtectedSourceState) {
        *lock(&self.status) = status;
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

    fn set_capture_active(&self, active: bool) -> bool {
        self.capture_active.swap(active, Ordering::AcqRel)
    }

    fn begin_resolution(&self) -> Result<u64, MacosCaptureError> {
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
}

struct RetainedNativeSample {
    attachments: MacosRawFrameAttachments,
    pixel_buffer: Option<CFRetained<CVPixelBuffer>>,
    admission_lifetime: Option<PoolBackingLifetime>,
    cursor_composed: bool,
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
) -> Result<RetainedNativeSample, MacosCaptureError> {
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
    let (pixel_buffer, admission_lifetime) = if status == MacosFrameStatus::Complete {
        let pixel_buffer = borrowed_pixel_buffer(sample)?;
        let storage_extent = extent(
            CVPixelBufferGetWidth(pixel_buffer),
            CVPixelBufferGetHeight(pixel_buffer),
        )?;
        let pixel_format_fourcc = CVPixelBufferGetPixelFormatType(pixel_buffer);
        let pixel_format = MacosCapturePixelFormat::from_fourcc(pixel_format_fourcc)?;
        let planes = planes(pixel_buffer, storage_extent)?;
        let (iosurface_id, allocation_bytes) = borrowed_surface_identity(pixel_buffer)?;
        crate::frame::validate_capture_planes(
            storage_extent,
            pixel_format,
            planes,
            allocation_bytes,
        )?;
        with_admitted_surface(pool, iosurface_id, allocation_bytes, |admission_lifetime| {
            // SAFETY: admission succeeded while the callback still owns the
            // borrowed image buffer, so this takes the retained owner handed off.
            let pixel_buffer = unsafe { CFRetained::retain(NonNull::from(pixel_buffer)) };
            (Some(pixel_buffer), Some(admission_lifetime))
        })?
    } else {
        (None, None)
    };
    Ok(RetainedNativeSample {
        attachments,
        pixel_buffer,
        admission_lifetime,
        cursor_composed,
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
    epoch: u64,
    streams: &Weak<StreamSlot>,
    shared: &Arc<SessionShared>,
) {
    let _timing = shared.counters.observe_publication();
    match result {
        Ok(DecodedSample {
            event: MacosFrameEvent::Frame(frame),
            confirmed_delivery,
        }) => {
            let active = shared.current_epoch() == epoch
                || streams
                    .upgrade()
                    .is_some_and(|streams| streams.activate(epoch, confirmed_delivery));
            if active {
                shared.publish(MacosFrameEvent::Frame(frame));
            }
        }
        Ok(DecodedSample { event, .. }) if shared.current_epoch() == epoch => {
            shared.publish(event);
        }
        Ok(_) => {}
        Err(error @ MacosCaptureError::StreamDeliveryRejected(_)) => {
            handle_fatal_stream_error(streams, epoch, Arc::clone(shared), error);
        }
        Err(error) => shared.counters.record_drop(&error),
    }
}

struct CaptureOutputIvars {
    samples: LatestSampleInput<Result<RetainedNativeSample, MacosCaptureError>>,
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
            let sample = if output_type == SCStreamOutputType::Screen {
                let _retain_timing = self.ivars().shared.counters.observe_retain();
                retain_sample(
                    sample_buffer,
                    self.ivars().cursor_composed,
                    &self.ivars().pool,
                )
            } else {
                Err(MacosCaptureError::UnexpectedStreamOutputType(output_type.0))
            };
            let sample = match sample {
                Err(error @ MacosCaptureError::ScreenResourceExhausted { .. }) => {
                    handle_fatal_stream_error(
                        &self.ivars().streams,
                        self.ivars().epoch,
                        Arc::clone(&self.ivars().shared),
                        error,
                    );
                    return;
                }
                sample => sample,
            };
            if self.ivars().samples.publish(sample) == SamplePublishOutcome::Superseded {
                self.ivars()
                    .shared
                    .counters
                    .record_native_sample_superseded();
            }
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
            if !self.ivars().display_filter
                && self.ivars().shared.current_epoch() == self.ivars().epoch
            {
                self.ivars()
                    .shared
                    .set_status(MacosProtectedSourceState::Live);
            }
        }

        #[allow(non_snake_case)]
        #[unsafe(method(streamDidBecomeInactive:))]
        fn streamDidBecomeInactive(&self, _stream: &SCStream) {
            if !self.ivars().display_filter
                && self.ivars().shared.current_epoch() == self.ivars().epoch
            {
                self.ivars()
                    .shared
                    .set_status(MacosProtectedSourceState::NeedsSelection);
            }
        }
    }
);

impl CaptureOutput {
    fn new(
        epoch: u64,
        samples: LatestSampleInput<Result<RetainedNativeSample, MacosCaptureError>>,
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
struct NativeFilter(Retained<SCContentFilter>);

// SAFETY: SCContentFilter is immutable after picker delivery and remains in
// the process that owns every consuming SCStream. Rust never mutates it.
unsafe impl Send for NativeFilter {}

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
                captureScreenshotWithFilter: &*filter.0,
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
    worker: LatestSampleWorker<Result<RetainedNativeSample, MacosCaptureError>>,
    _output: Retained<CaptureOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: ScreenCaptureKit owns callback execution across its queues, and all
// Rust access to this owner is serialized through StreamSlot. NativeStream is
// moved between owners but never exposes concurrent mutable Objective-C state.
unsafe impl Send for NativeStream {}

impl NativeStream {
    fn prepare(
        filter: &SCContentFilter,
        request: MacosStreamRequest,
        epoch: u64,
        shared: Arc<SessionShared>,
        streams: Weak<StreamSlot>,
        reserve_pool: &PoolReservationFactory,
    ) -> Result<Self, MacosCaptureError> {
        let (configuration, display_filter, extent, configured_stream) =
            stream_configuration(filter, request)?;
        let quote = conservative_pool_quote(extent, configured_stream.configured_pixel_format)?;
        let pool = reserve_pool(quote.per_surface_bytes, quote.stream_metadata_bytes)?;
        let selection = selection_from_filter(filter)?;
        let source_id = selection_source_id(filter, &selection);
        // SAFETY: The picker callback supplies a live filter. Retaining it
        // preserves the immutable selection through stream retirement.
        let retained_filter = unsafe {
            Retained::retain(ptr::from_ref(filter).cast_mut())
                .ok_or(MacosCaptureError::RetainNativeFilterFailed)?
        };
        let mut decoder = MacosFrameDecoder::new(epoch);
        let mut delivery_validator = MacosStreamDeliveryValidator::new(configured_stream);
        delivery_validator.validate_configuration()?;
        let decode_shared = Arc::clone(&shared);
        let worker_shared = Arc::clone(&shared);
        let worker_streams = streams.clone();
        let worker = LatestSampleWorker::spawn(
            "hypercolor-macos-screen-capture",
            move |sample: Result<RetainedNativeSample, MacosCaptureError>| {
                let _timing = decode_shared.counters.observe_conversion();
                match sample {
                    Ok(sample) => decode_sample(&mut decoder, &mut delivery_validator, sample),
                    Err(error) => Err(classify_delivery_error(&mut delivery_validator, error)),
                }
            },
            move |result| {
                publish_decoded_result(result, epoch, &worker_streams, &worker_shared);
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
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    protocol,
                    SCStreamOutputType::Screen,
                    Some(&queue),
                )
                .map_err(|error| native_error("add ScreenCaptureKit output", &error))?;
        }
        Ok(Self {
            stream,
            filter: NativeFilter(retained_filter),
            selection,
            source_id,
            worker,
            _output: output,
            _queue: queue,
        })
    }

    fn epoch(&self) -> u64 {
        self._output.ivars().epoch
    }

    fn stop(mut self) -> Result<(), MacosCaptureError> {
        self.worker.close();
        let worker_result = self
            .worker
            .join()
            .map_err(|_| MacosCaptureError::CaptureWorkerPanicked);
        let (completion_tx, completion_rx) = std::sync::mpsc::sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: ScreenCaptureKit supplies either null or a live NSError
            // for the duration of this completion invocation.
            let result = unsafe { error.as_ref() }.map_or(Ok(()), |error| {
                Err(native_error("stop ScreenCaptureKit stream", error))
            });
            let _ = completion_tx.send(result);
        });
        // SAFETY: ScreenCaptureKit copies the completion block and the stream
        // remains retained until the completion result is received.
        unsafe {
            self.stream
                .stopCaptureWithCompletionHandler(Some(&completion));
        }
        let stop_result = completion_rx
            .recv()
            .map_err(|_| MacosCaptureError::StreamStopCompletionLost)
            .and_then(std::convert::identity);
        stop_result.and(worker_result)
    }

    fn retire_after_native_stop(mut self) -> Result<(), MacosCaptureError> {
        self.worker.close();
        self.worker
            .join()
            .map_err(|_| MacosCaptureError::CaptureWorkerPanicked)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamRole {
    Current,
    Candidate,
    Stale,
}

#[derive(Default)]
struct StreamState {
    current: Option<NativeStream>,
    candidate: Option<NativeStream>,
    selected_filter: Option<NativeFilter>,
    selection_revision: u64,
}

struct StreamSlot {
    state: Mutex<StreamState>,
    shared: Arc<SessionShared>,
    next_epoch: AtomicU64,
}

impl StreamSlot {
    fn new(shared: Arc<SessionShared>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StreamState::default()),
            shared,
            next_epoch: AtomicU64::new(1),
        })
    }

    fn allocate_epoch(&self) -> Result<u64, MacosCaptureError> {
        self.next_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map_err(|_| MacosCaptureError::SequenceExhausted)
    }

    fn stage_candidate(
        self: &Arc<Self>,
        filter: &SCContentFilter,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
        epoch: u64,
    ) -> Result<(), MacosCaptureError> {
        let candidate = NativeStream::prepare(
            filter,
            request,
            epoch,
            Arc::clone(&self.shared),
            Arc::downgrade(self),
            reserve_pool,
        )?;
        let stream = candidate.stream.clone();
        let replaced = {
            let mut state = lock(&self.state);
            state.selection_revision = state
                .selection_revision
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            state.candidate.replace(candidate)
        };
        if let Some(replaced) = replaced {
            self.stop_stream(replaced);
        }
        self.shared.set_status(MacosProtectedSourceState::Starting);
        start_stream(
            &stream,
            epoch,
            Arc::downgrade(self),
            Arc::clone(&self.shared),
        );
        Ok(())
    }

    fn activate(
        &self,
        epoch: u64,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
    ) -> bool {
        let previous = {
            let mut state = lock(&self.state);
            let Some(candidate) = state
                .candidate
                .take_if(|candidate| candidate.epoch() == epoch)
            else {
                return false;
            };
            let Some(confirmed_delivery) = confirmed_delivery else {
                state.candidate = Some(candidate);
                return false;
            };
            let previous = state.current.replace(candidate);
            state.selected_filter = state.current.as_ref().map(|current| current.filter.clone());
            if let Some(current) = &state.current {
                self.shared.confirm_selection(
                    current.selection.clone(),
                    Arc::clone(&current.source_id),
                    epoch,
                    confirmed_delivery,
                );
            }
            self.shared.activate_epoch(epoch);
            previous
        };
        if let Some(previous) = previous {
            self.stop_stream(previous);
        }
        true
    }

    fn remove(&self, epoch: u64) -> (StreamRole, Option<NativeStream>) {
        let mut state = lock(&self.state);
        if state
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.epoch() == epoch)
        {
            return (StreamRole::Candidate, state.candidate.take());
        }
        if state
            .current
            .as_ref()
            .is_some_and(|current| current.epoch() == epoch)
        {
            let current = state.current.take();
            self.shared.activate_epoch(0);
            self.shared.clear_tahoe_selection();
            return (StreamRole::Current, current);
        }
        (StreamRole::Stale, None)
    }

    fn accepts_epoch(&self, epoch: u64) -> bool {
        let state = lock(&self.state);
        state
            .current
            .as_ref()
            .is_some_and(|stream| stream.epoch() == epoch)
            || state
                .candidate
                .as_ref()
                .is_some_and(|stream| stream.epoch() == epoch)
    }

    fn has_current(&self) -> bool {
        lock(&self.state).current.is_some()
    }

    fn active_identity(&self) -> Option<(Arc<str>, u64)> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|current| (Arc::clone(&current.source_id), current.epoch()))
    }

    fn has_selection(&self) -> bool {
        lock(&self.state).selected_filter.is_some()
    }

    fn store_selection(&self, filter: &SCContentFilter) -> Result<(), MacosCaptureError> {
        let selection = selection_from_filter(filter)?;
        // SAFETY: The picker callback supplies a live immutable filter. The
        // retained owner remains process-local and is never serialized.
        let filter = unsafe {
            Retained::retain(ptr::from_ref(filter).cast_mut())
                .ok_or(MacosCaptureError::RetainNativeFilterFailed)?
        };
        let mut state = lock(&self.state);
        state.selection_revision = state
            .selection_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.selected_filter = Some(NativeFilter(filter));
        drop(state);
        self.shared.set_unconfirmed_selection(selection);
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

    fn selected_filter(&self) -> Option<NativeFilter> {
        lock(&self.state).selected_filter.clone()
    }

    fn current_stream(&self) -> Option<Retained<SCStream>> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|current| current.stream.clone())
    }

    fn stop(&self) {
        let (current, candidate) = {
            let mut state = lock(&self.state);
            state.selection_revision = state.selection_revision.saturating_add(1);
            if state.current.is_none()
                && state.selected_filter.is_none()
                && let Some(candidate) = state.candidate.as_ref()
            {
                state.selected_filter = Some(candidate.filter.clone());
            }
            (state.current.take(), state.candidate.take())
        };
        self.shared.activate_epoch(0);
        self.shared.clear_tahoe_selection();
        if let Some(candidate) = candidate {
            self.stop_stream(candidate);
        }
        if let Some(current) = current {
            self.stop_stream(current);
        }
    }

    fn stop_stream(&self, stream: NativeStream) {
        if let Err(error) = stream.stop() {
            self.shared.publish_recoverable_error(error);
        }
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
) {
    let completion = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: ScreenCaptureKit supplies either null or a live NSError for
        // the duration of this completion invocation.
        if let Some(error) = unsafe { error.as_ref() } {
            handle_stream_error(&streams, epoch, &shared, error);
        }
    });
    // SAFETY: ScreenCaptureKit copies the heap block for asynchronous use, and
    // the stream remains retained by StreamSlot until activation or failure.
    unsafe { stream.startCaptureWithCompletionHandler(Some(&completion)) };
}

fn handle_stream_error(
    streams: &Weak<StreamSlot>,
    epoch: u64,
    shared: &SessionShared,
    error: &NSError,
) {
    let (role, retired) = streams
        .upgrade()
        .map_or((StreamRole::Stale, None), |streams| streams.remove(epoch));
    if let Some(retired) = retired
        && let Err(worker_error) = retired.retire_after_native_stop()
    {
        shared.counters.record_drop(&worker_error);
    }
    let preserve_current = match role {
        StreamRole::Candidate
            if streams
                .upgrade()
                .is_some_and(|streams| streams.has_current()) =>
        {
            shared.set_status(MacosProtectedSourceState::Live);
            true
        }
        StreamRole::Candidate | StreamRole::Current => {
            shared.set_status(classify_stream_error(error));
            false
        }
        StreamRole::Stale => return,
    };
    let error = native_error("ScreenCaptureKit stream", error);
    if preserve_current {
        shared.publish_recoverable_error(error);
    } else {
        shared.publish_error(error);
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
    let (role, retired) = streams.remove(epoch);
    let preserve_current = role == StreamRole::Candidate && streams.has_current();
    if preserve_current {
        shared.set_status(MacosProtectedSourceState::Live);
        shared.publish_recoverable_error(error);
    } else if role != StreamRole::Stale {
        shared.set_status(MacosProtectedSourceState::Failed);
        shared.publish_error(error);
    }
    let Some(retired) = retired else {
        return;
    };
    let stop_shared = Arc::clone(&shared);
    if let Err(spawn_error) = std::thread::Builder::new()
        .name("hypercolor-macos-screen-rejection-stop".to_owned())
        .spawn(move || {
            if let Err(error) = retired.stop() {
                stop_shared.publish_recoverable_error(error);
            }
        })
    {
        shared.publish_recoverable_error(MacosCaptureError::CaptureWorkerStartFailed(
            spawn_error.to_string(),
        ));
    }
}

struct PickerObserverIvars {
    shared: Arc<SessionShared>,
    streams: Arc<StreamSlot>,
    request: MacosStreamRequest,
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
            if !self.ivars().streams.has_selection() {
                self.ivars()
                    .shared
                    .set_status(MacosProtectedSourceState::NeedsSelection);
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
            if let Err(error) = self.ivars().shared.begin_resolution() {
                handle_filter_error(&self.ivars().streams, &self.ivars().shared, error);
                return;
            }
            accept_filter(
                &self.ivars().streams,
                &self.ivars().shared,
                self.ivars().request,
                &self.ivars().reserve_pool,
                filter,
            );
        }

        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
        fn contentSharingPickerStartDidFailWithError(&self, error: &NSError) {
            let preserve_current = self.ivars().streams.has_current();
            let preserve_selection = self.ivars().streams.has_selection();
            if !preserve_current && !preserve_selection {
                self.ivars()
                    .shared
                    .set_status(MacosProtectedSourceState::Failed);
            } else if !preserve_current {
                self.ivars()
                    .shared
                    .set_status(MacosProtectedSourceState::ReadyIdle);
            }
            let error = native_error("ScreenCaptureKit picker", error);
            if preserve_current || preserve_selection {
                self.ivars().shared.publish_recoverable_error(error);
            } else {
                self.ivars().shared.publish_error(error);
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
    ) -> Retained<Self> {
        let streams = StreamSlot::new(Arc::clone(&shared));
        let this = mtm.alloc::<Self>().set_ivars(PickerObserverIvars {
            shared,
            streams,
            request,
            reserve_pool,
        });
        // SAFETY: NSObject has no additional initialization requirements for
        // this main-thread observer subclass.
        unsafe { msg_send![super(this), init] }
    }

    fn install_filter(&self, filter: &SCContentFilter) {
        stage_filter(
            &self.ivars().streams,
            &self.ivars().shared,
            self.ivars().request,
            &self.ivars().reserve_pool,
            filter,
        );
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
        if self.ivars().shared.set_capture_active(active) == active {
            return;
        }
        if !active {
            self.ivars().streams.stop();
            let status = if self.ivars().streams.has_selection() {
                MacosProtectedSourceState::ReadyIdle
            } else {
                MacosProtectedSourceState::NeedsSelection
            };
            self.ivars().shared.set_status(status);
            return;
        }
        let Some(filter) = self.ivars().streams.selected_filter() else {
            self.ivars()
                .shared
                .set_status(MacosProtectedSourceState::NeedsSelection);
            return;
        };
        self.install_filter(&filter.0);
    }

    fn stop(&self) {
        self.ivars().shared.set_capture_active(false);
        self.ivars().streams.stop();
    }
}

fn accept_filter(
    streams: &Arc<StreamSlot>,
    shared: &Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: &PoolReservationFactory,
    filter: &SCContentFilter,
) {
    if shared.capture_active() {
        stage_filter(streams, shared, request, reserve_pool, filter);
    } else if let Err(error) = streams.store_selection(filter) {
        handle_filter_error(streams, shared, error);
    } else {
        shared.set_status(MacosProtectedSourceState::ReadyIdle);
    }
}

fn stage_filter(
    streams: &Arc<StreamSlot>,
    shared: &Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: &PoolReservationFactory,
    filter: &SCContentFilter,
) {
    let result = streams
        .allocate_epoch()
        .and_then(|epoch| streams.stage_candidate(filter, request, reserve_pool, epoch));
    if let Err(error) = result {
        handle_filter_error(streams, shared, error);
    }
}

fn handle_filter_error(streams: &StreamSlot, shared: &SessionShared, error: MacosCaptureError) {
    let preserve_current = streams.has_current();
    let preserve_selection = streams.has_selection();
    let status = if preserve_current {
        MacosProtectedSourceState::Live
    } else if preserve_selection {
        MacosProtectedSourceState::ReadyIdle
    } else if matches!(error, MacosCaptureError::DisplaySourceUnavailable(_)) {
        MacosProtectedSourceState::NeedsSelection
    } else {
        MacosProtectedSourceState::Failed
    };
    shared.set_status(status);
    if preserve_current || preserve_selection {
        shared.publish_recoverable_error(error);
    } else {
        shared.publish_error(error);
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
    request: MacosStreamRequest,
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
            Self::new_on_main(request, selector, capabilities.tahoe, reserve_pool, mtm)
        })
    }

    fn new_on_main(
        request: MacosStreamRequest,
        selector: MacosCaptureSelector,
        tahoe: MacosTahoeCapabilities,
        reserve_pool: PoolReservationFactory,
        mtm: MainThreadMarker,
    ) -> Result<Self, MacosCaptureError> {
        let authorized = CGPreflightScreenCaptureAccess();
        let status = if authorized {
            MacosProtectedSourceState::NeedsSelection
        } else {
            MacosProtectedSourceState::NeedsUserAction
        };
        let shared = Arc::new(SessionShared::new(status, selector, tahoe));
        let observer = PickerObserver::new(mtm, request, Arc::clone(&shared), reserve_pool);
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
            request,
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
                handle_filter_error(&self.streams, &self.shared, error);
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
        self.shared.begin_resolution()?;
        self.main
            .get_on_main(|main| main.observer.present(&main.picker));
        Ok(())
    }

    pub fn status(&self) -> MacosProtectedSourceState {
        self.shared.status()
    }

    pub fn selection(&self) -> MacosCaptureSelection {
        self.shared.selection()
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
            self.request.cursor_composed,
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
        self.shared.set_selector(selector);
        if CGPreflightScreenCaptureAccess() {
            self.resolve_configured_source()
        } else {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsUserAction);
            Ok(())
        }
    }

    fn resolve_configured_source(&self) -> Result<(), MacosCaptureError> {
        let selector = self.shared.selector();
        if selector == MacosCaptureSelector::SessionScoped {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsSelection);
            return Ok(());
        }
        resolve_display_selector(
            Arc::clone(&self.streams),
            Arc::clone(&self.shared),
            self.request,
            self.main
                .get_on_main(|main| Arc::clone(&main.observer.ivars().reserve_pool)),
            selector,
        )
    }
}

fn resolve_display_selector(
    streams: Arc<StreamSlot>,
    shared: Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: PoolReservationFactory,
    selector: MacosCaptureSelector,
) -> Result<(), MacosCaptureError> {
    let resolution_epoch = shared.begin_resolution()?;
    let completion = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            if !shared.resolution_is_current(resolution_epoch) {
                return;
            }
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
            if !shared.resolution_is_current(resolution_epoch) {
                return;
            }
            match result {
                Ok(filter) => {
                    accept_filter(&streams, &shared, request, &reserve_pool, &filter);
                }
                Err(error) => handle_filter_error(&streams, &shared, error),
            }
        },
    );
    // SAFETY: ScreenCaptureKit copies the completion block for asynchronous
    // use. The block owns every Rust value captured by the callback.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&completion) };
    Ok(())
}

fn display_filter(
    content: &SCShareableContent,
    selector: &MacosCaptureSelector,
) -> Result<Retained<SCContentFilter>, MacosCaptureError> {
    // SAFETY: Shareable content owns an immutable display snapshot. The
    // returned array and each selected display are retained locally.
    let displays = unsafe { content.displays() };
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
            let excluded = NSArray::<SCWindow>::from_slice(&[]);
            // SAFETY: The filter retains the selected display and the empty
            // exclusion list. The display comes from this content snapshot.
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
    let status = match sample.attachments.status {
        MacosAttachment::Value(status) => MacosFrameStatus::try_from(status)?,
        MacosAttachment::Missing => return Err(MacosCaptureError::MissingAttachment("status")),
        MacosAttachment::Malformed => {
            return Err(MacosCaptureError::MalformedAttachment("status"));
        }
    };
    if status != MacosFrameStatus::Complete {
        return decoder
            .decode(MacosRawCaptureSample {
                frame: None,
                attachments: sample.attachments,
            })
            .map(|event| DecodedSample {
                event,
                confirmed_delivery: None,
            });
    }

    let awaiting_first_delivery = matches!(
        delivery_validator.state(),
        MacosStreamDeliveryState::AwaitingFirstCompleteFrame(_)
    );
    let pixel_buffer = sample
        .pixel_buffer
        .ok_or(MacosCaptureError::MissingFramePayload)
        .map_err(|error| classify_delivery_error(delivery_validator, error))?;
    let frame = decode_complete_frame(
        pixel_buffer,
        sample.admission_lifetime,
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
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        MacosCaptureColorimetry, MacosCaptureDynamicRange, MacosCaptureError,
        MacosCapturePixelFormat, MacosColorPrimaries, MacosColorRange, MacosConfiguredStream,
        MacosDeliveredFrameMetadata, MacosHostArchitecture, MacosPixelExtent,
        MacosProtectedSourceState, MacosRuntimeCapability, MacosStreamDeliveryRejection,
        MacosStreamDeliveryState, MacosStreamDeliveryValidator, MacosStreamPreset,
        MacosTahoeCapabilities, MacosTahoeRuntimeProbes, MacosTransferFunction,
        MacosValidatedStreamDelivery, PoolBackingLifetime, PoolObservation, SCCaptureDynamicRange,
        SCStreamConfiguration, SCStreamConfigurationPreset, ScreenshotCaptureBackend,
        ScreenshotFilterHandle, ScreenshotIdentityFence, ScreenshotImageCompletion,
        ScreenshotTransactionSnapshot, SessionShared, SysctlI32Value,
        capture_capabilities_from_probes, capture_dynamic_range, classify_delivery_error,
        color_range_from_fourcc, conservative_pool_quote, execute_screenshot_transaction,
        session_selection_source_id, with_admitted_surface,
    };
    use crate::{
        MacosScreenshotReferenceCapability, MacosScreenshotReferenceImage,
        MacosScreenshotReferenceSet,
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
