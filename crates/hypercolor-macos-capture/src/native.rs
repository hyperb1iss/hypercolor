use std::fmt;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained, MainThreadBound};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFUUID, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGDirectDisplayID, CGMainDisplayID, CGPreflightScreenCaptureAccess,
    CGRectMakeWithDictionaryRepresentation, CGRequestScreenCaptureAccess,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVBuffer, CVPixelBuffer, CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetDataSize, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
    CVPixelBufferGetWidthOfPlane, kCVImageBufferChromaLocation_Center,
    kCVImageBufferChromaLocation_Left, kCVImageBufferChromaLocation_TopLeft,
    kCVImageBufferChromaLocationTopFieldKey, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferColorPrimaries_ITU_R_2020, kCVImageBufferColorPrimaries_P3_D65,
    kCVImageBufferColorPrimariesKey, kCVImageBufferTransferFunction_ITU_R_709_2,
    kCVImageBufferTransferFunction_ITU_R_2020, kCVImageBufferTransferFunction_ITU_R_2100_HLG,
    kCVImageBufferTransferFunction_Linear, kCVImageBufferTransferFunction_SMPTE_ST_2084_PQ,
    kCVImageBufferTransferFunction_sRGB, kCVImageBufferTransferFunctionKey,
    kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
    kCVImageBufferYCbCrMatrix_ITU_R_2020, kCVImageBufferYCbCrMatrixKey,
};
use objc2_foundation::{NSArray, NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSValue};
use objc2_io_surface::IOSurfaceRef;
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCContentSharingPicker,
    SCContentSharingPickerConfiguration, SCContentSharingPickerMode,
    SCContentSharingPickerObserver, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamErrorCode, SCStreamErrorDomain, SCStreamFrameInfoBoundingRect,
    SCStreamFrameInfoContentRect, SCStreamFrameInfoContentScale, SCStreamFrameInfoDirtyRects,
    SCStreamFrameInfoDisplayTime, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoScreenRect,
    SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType, SCWindow,
};

use crate::diagnostics::CallbackCounters;
use crate::worker::{LatestSampleInput, LatestSampleWorker, SamplePublishOutcome};
use crate::{
    MACOS_STREAM_QUEUE_DEPTH, MacosAttachment, MacosCaptureCallbackDiagnostics,
    MacosCaptureColorimetry, MacosCaptureContentStyle, MacosCaptureError, MacosCapturePixelFormat,
    MacosCaptureSelection, MacosCaptureSelector, MacosCaptureSurface, MacosChromaLocation,
    MacosColorPrimaries, MacosColorRange, MacosFrameDecoder, MacosFrameEvent, MacosFrameMailbox,
    MacosFrameStatus, MacosPixelExtent, MacosPixelRect, MacosPointRect, MacosProtectedSourceState,
    MacosRawCapturePlane, MacosRawCaptureSample, MacosRawCompleteFrame, MacosRawFrameAttachments,
    MacosScale, MacosStreamRequest, MacosTransferFunction, MacosYuvMatrix,
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
    selection: Mutex<MacosCaptureSelection>,
    selector: Mutex<MacosCaptureSelector>,
    counters: CallbackCounters,
    capture_active: AtomicBool,
    current_epoch: AtomicU64,
    resolution_epoch: AtomicU64,
}

impl SessionShared {
    fn new(status: MacosProtectedSourceState, selector: MacosCaptureSelector) -> Self {
        Self {
            mailbox: MacosFrameMailbox::new(),
            status: Mutex::new(status),
            selection: Mutex::new(MacosCaptureSelection::None),
            selector: Mutex::new(selector),
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
        lock(&self.selection).clone()
    }

    fn set_selection(&self, selection: MacosCaptureSelection) {
        *lock(&self.selection) = selection;
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
    result: Result<MacosFrameEvent, MacosCaptureError>,
    epoch: u64,
    streams: &Weak<StreamSlot>,
    shared: &SessionShared,
) {
    match result {
        Ok(MacosFrameEvent::Frame(frame)) => {
            let active = shared.current_epoch() == epoch
                || streams
                    .upgrade()
                    .is_some_and(|streams| streams.activate(epoch));
            if active {
                shared.publish(MacosFrameEvent::Frame(frame));
            }
        }
        Ok(event) if shared.current_epoch() == epoch => shared.publish(event),
        Ok(_) => {}
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
                    handle_pool_admission_error(
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

struct NativeStream {
    stream: Retained<SCStream>,
    filter: NativeFilter,
    selection: MacosCaptureSelection,
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
        let (configuration, display_filter, extent, pixel_format) =
            stream_configuration(filter, request)?;
        let quote = conservative_pool_quote(extent, pixel_format)?;
        let pool = reserve_pool(quote.per_surface_bytes, quote.stream_metadata_bytes)?;
        let selection = selection_from_filter(filter)?;
        // SAFETY: The picker callback supplies a live filter. Retaining it
        // preserves the immutable selection through stream retirement.
        let retained_filter = unsafe {
            Retained::retain(ptr::from_ref(filter).cast_mut())
                .ok_or(MacosCaptureError::RetainNativeFilterFailed)?
        };
        let mut decoder = MacosFrameDecoder::new(epoch);
        let worker_shared = Arc::clone(&shared);
        let worker_streams = streams.clone();
        let worker = LatestSampleWorker::spawn(
            "hypercolor-macos-screen-capture",
            move |sample: Result<RetainedNativeSample, MacosCaptureError>| {
                sample.and_then(|sample| decode_sample(&mut decoder, sample))
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
        let replaced = lock(&self.state).candidate.replace(candidate);
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

    fn activate(&self, epoch: u64) -> bool {
        let previous = {
            let mut state = lock(&self.state);
            let Some(candidate) = state
                .candidate
                .take_if(|candidate| candidate.epoch() == epoch)
            else {
                return false;
            };
            let previous = state.current.replace(candidate);
            state.selected_filter = state.current.as_ref().map(|current| current.filter.clone());
            if let Some(current) = &state.current {
                self.shared.set_selection(current.selection.clone());
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
        lock(&self.state).selected_filter = Some(NativeFilter(filter));
        self.shared.set_selection(selection);
        Ok(())
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
            if state.current.is_none()
                && state.selected_filter.is_none()
                && let Some(candidate) = state.candidate.as_ref()
            {
                state.selected_filter = Some(candidate.filter.clone());
            }
            (state.current.take(), state.candidate.take())
        };
        self.shared.activate_epoch(0);
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

fn handle_pool_admission_error(
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
        .name("hypercolor-macos-screen-resource-stop".to_owned())
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
        let reserve_pool = Arc::new(move |surface_bytes, metadata_bytes| {
            let observer = reserve_pool(surface_bytes, metadata_bytes)?;
            Ok(Arc::new(observer) as PoolObservation)
        }) as PoolReservationFactory;
        dispatch2::run_on_main(move |mtm| Self::new_on_main(request, selector, reserve_pool, mtm))
    }

    fn new_on_main(
        request: MacosStreamRequest,
        selector: MacosCaptureSelector,
        reserve_pool: PoolReservationFactory,
        mtm: MainThreadMarker,
    ) -> Result<Self, MacosCaptureError> {
        let authorized = CGPreflightScreenCaptureAccess();
        let status = if authorized {
            MacosProtectedSourceState::NeedsSelection
        } else {
            MacosProtectedSourceState::NeedsUserAction
        };
        let shared = Arc::new(SessionShared::new(status, selector));
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

fn stream_configuration(
    filter: &SCContentFilter,
    request: MacosStreamRequest,
) -> Result<
    (
        Retained<SCStreamConfiguration>,
        bool,
        MacosPixelExtent,
        MacosCapturePixelFormat,
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
    // SAFETY: Every setter receives validated point or pixel units, and the
    // configuration is retained by the caller before stream creation.
    let configuration = unsafe {
        let configuration = SCStreamConfiguration::new();
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
        configuration.setPixelFormat(0x4247_5241);
        configuration
    };
    Ok((
        configuration,
        display_filter,
        extent,
        MacosCapturePixelFormat::Bgra8,
    ))
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
    sample: RetainedNativeSample,
) -> Result<MacosFrameEvent, MacosCaptureError> {
    let status = match sample.attachments.status {
        MacosAttachment::Value(status) => MacosFrameStatus::try_from(status)?,
        MacosAttachment::Missing => return Err(MacosCaptureError::MissingAttachment("status")),
        MacosAttachment::Malformed => {
            return Err(MacosCaptureError::MalformedAttachment("status"));
        }
    };
    if status != MacosFrameStatus::Complete {
        return decoder.decode(MacosRawCaptureSample {
            frame: None,
            attachments: sample.attachments,
        });
    }

    let pixel_buffer = sample
        .pixel_buffer
        .ok_or(MacosCaptureError::MissingFramePayload)?;
    let frame = decode_complete_frame(
        pixel_buffer,
        sample.admission_lifetime,
        sample.cursor_composed,
    )?;
    decoder.decode(MacosRawCaptureSample {
        frame: Some(frame),
        attachments: sample.attachments,
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
    let surface = MacosCaptureSurface::from_pixel_buffer(pixel_buffer, admission_lifetime)?;

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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{
        MacosCaptureError, MacosCapturePixelFormat, MacosPixelExtent, PoolBackingLifetime,
        PoolObservation, conservative_pool_quote, with_admitted_surface,
    };

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
