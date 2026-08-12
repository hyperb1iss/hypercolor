use std::cell::RefCell;
use std::fmt;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGRequestScreenCaptureAccess,
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
use objc2_foundation::{NSError, NSNumber, NSObject, NSObjectProtocol, NSString, NSValue};
use objc2_screen_capture_kit::{
    SCCaptureResolutionType, SCContentFilter, SCContentSharingPicker,
    SCContentSharingPickerConfiguration, SCContentSharingPickerMode,
    SCContentSharingPickerObserver, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamErrorCode, SCStreamErrorDomain, SCStreamFrameInfoBoundingRect,
    SCStreamFrameInfoContentRect, SCStreamFrameInfoContentScale, SCStreamFrameInfoDirtyRects,
    SCStreamFrameInfoDisplayTime, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoScreenRect,
    SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use crate::diagnostics::CallbackCounters;
use crate::{
    MACOS_STREAM_QUEUE_DEPTH, MacosAttachment, MacosCaptureCallbackDiagnostics,
    MacosCaptureColorimetry, MacosCaptureError, MacosCapturePixelFormat, MacosCaptureSurface,
    MacosChromaLocation, MacosColorPrimaries, MacosColorRange, MacosFrameDecoder, MacosFrameEvent,
    MacosFrameMailbox, MacosFrameStatus, MacosPixelExtent, MacosPixelRect, MacosPointRect,
    MacosProtectedSourceState, MacosRawCapturePlane, MacosRawCaptureSample, MacosRawCompleteFrame,
    MacosRawFrameAttachments, MacosScale, MacosStreamRequest, MacosTransferFunction,
    MacosYuvMatrix,
};

#[derive(Debug)]
struct SessionShared {
    mailbox: MacosFrameMailbox,
    status: Mutex<MacosProtectedSourceState>,
    counters: CallbackCounters,
    current_epoch: AtomicU64,
}

impl SessionShared {
    fn new(status: MacosProtectedSourceState) -> Self {
        Self {
            mailbox: MacosFrameMailbox::new(),
            status: Mutex::new(status),
            counters: CallbackCounters::default(),
            current_epoch: AtomicU64::new(0),
        }
    }

    fn status(&self) -> MacosProtectedSourceState {
        *lock(&self.status)
    }

    fn set_status(&self, status: MacosProtectedSourceState) {
        *lock(&self.status) = status;
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
}

#[derive(Debug)]
struct CaptureOutputIvars {
    decoder: Mutex<MacosFrameDecoder>,
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
            let result = if output_type == SCStreamOutputType::Screen {
                let mut decoder = lock(&self.ivars().decoder);
                decode_sample(&mut decoder, sample_buffer, self.ivars().cursor_composed)
            } else {
                Err(MacosCaptureError::UnexpectedStreamOutputType(output_type.0))
            };
            match result {
                Ok(MacosFrameEvent::Frame(frame)) => {
                    let active = self.ivars().shared.current_epoch() == self.ivars().epoch
                        || self
                            .ivars()
                            .streams
                            .upgrade()
                            .is_some_and(|streams| streams.activate(self.ivars().epoch));
                    if active {
                        self.ivars().shared.publish(MacosFrameEvent::Frame(frame));
                    }
                }
                Ok(event) if self.ivars().shared.current_epoch() == self.ivars().epoch => {
                    self.ivars().shared.publish(event);
                }
                Ok(_) => {}
                Err(error) => self.ivars().shared.counters.record_drop(&error),
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
        shared: Arc<SessionShared>,
        streams: Weak<StreamSlot>,
        cursor_composed: bool,
        display_filter: bool,
    ) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CaptureOutputIvars {
            decoder: Mutex::new(MacosFrameDecoder::new(epoch)),
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

struct NativeStream {
    stream: Retained<SCStream>,
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
    ) -> Result<Self, MacosCaptureError> {
        let (configuration, display_filter) = stream_configuration(filter, request)?;
        let output = CaptureOutput::new(
            epoch,
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
            _output: output,
            _queue: queue,
        })
    }

    fn stop(&self) {
        // SAFETY: Stopping an owned SCStream without a completion callback is
        // valid and retains no borrowed Rust state.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
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
}

struct StreamSlot {
    state: Mutex<StreamState>,
    shared: Arc<SessionShared>,
}

impl StreamSlot {
    fn new(shared: Arc<SessionShared>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(StreamState::default()),
            shared,
        })
    }

    fn stage_candidate(
        self: &Arc<Self>,
        filter: &SCContentFilter,
        request: MacosStreamRequest,
        epoch: u64,
    ) -> Result<(), MacosCaptureError> {
        let candidate = NativeStream::prepare(
            filter,
            request,
            epoch,
            Arc::clone(&self.shared),
            Arc::downgrade(self),
        )?;
        let stream = candidate.stream.clone();
        let replaced = lock(&self.state).candidate.replace(candidate);
        if let Some(replaced) = replaced {
            replaced.stop();
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
                .take_if(|candidate| candidate._output.ivars().epoch == epoch)
            else {
                return false;
            };
            let previous = state.current.replace(candidate);
            self.shared.activate_epoch(epoch);
            previous
        };
        if let Some(previous) = previous {
            previous.stop();
        }
        true
    }

    fn remove(&self, epoch: u64) -> StreamRole {
        let mut state = lock(&self.state);
        if state
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate._output.ivars().epoch == epoch)
        {
            state.candidate.take();
            return StreamRole::Candidate;
        }
        if state
            .current
            .as_ref()
            .is_some_and(|current| current._output.ivars().epoch == epoch)
        {
            state.current.take();
            self.shared.activate_epoch(0);
            return StreamRole::Current;
        }
        StreamRole::Stale
    }

    fn has_current(&self) -> bool {
        lock(&self.state).current.is_some()
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
            (state.current.take(), state.candidate.take())
        };
        self.shared.activate_epoch(0);
        if let Some(candidate) = candidate {
            candidate.stop();
        }
        if let Some(current) = current {
            current.stop();
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
    let role = streams
        .upgrade()
        .map_or(StreamRole::Stale, |streams| streams.remove(epoch));
    match role {
        StreamRole::Candidate
            if streams
                .upgrade()
                .is_some_and(|streams| streams.has_current()) =>
        {
            shared.set_status(MacosProtectedSourceState::Live);
        }
        StreamRole::Candidate | StreamRole::Current => {
            shared.set_status(classify_stream_error(error));
        }
        StreamRole::Stale => return,
    }
    shared.publish_error(native_error("ScreenCaptureKit stream", error));
}

struct PickerObserverIvars {
    shared: Arc<SessionShared>,
    streams: Arc<StreamSlot>,
    request: MacosStreamRequest,
    next_epoch: RefCell<u64>,
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
            if !self.ivars().streams.has_current() {
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
            self.install_filter(filter);
        }

        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
        fn contentSharingPickerStartDidFailWithError(&self, error: &NSError) {
            if !self.ivars().streams.has_current() {
                self.ivars()
                    .shared
                    .set_status(MacosProtectedSourceState::Failed);
            }
            self.ivars()
                .shared
                .publish_error(native_error("ScreenCaptureKit picker", error));
        }
    }
);

impl PickerObserver {
    fn new(
        mtm: MainThreadMarker,
        request: MacosStreamRequest,
        shared: Arc<SessionShared>,
    ) -> Retained<Self> {
        let streams = StreamSlot::new(Arc::clone(&shared));
        let this = mtm.alloc::<Self>().set_ivars(PickerObserverIvars {
            shared,
            streams,
            request,
            next_epoch: RefCell::new(1),
        });
        // SAFETY: NSObject has no additional initialization requirements for
        // this main-thread observer subclass.
        unsafe { msg_send![super(this), init] }
    }

    fn install_filter(&self, filter: &SCContentFilter) {
        let epoch = *self.ivars().next_epoch.borrow();
        let result = epoch
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)
            .and_then(|next_epoch| {
                *self.ivars().next_epoch.borrow_mut() = next_epoch;
                self.ivars()
                    .streams
                    .stage_candidate(filter, self.ivars().request, epoch)
            });
        if let Err(error) = result {
            let status = if self.ivars().streams.has_current() {
                MacosProtectedSourceState::Live
            } else {
                MacosProtectedSourceState::Failed
            };
            self.ivars().shared.set_status(status);
            self.ivars().shared.publish_error(error);
        }
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

    fn stop(&self) {
        self.ivars().streams.stop();
    }
}

pub struct MacosScreenCaptureSession {
    picker: Retained<SCContentSharingPicker>,
    observer: Retained<PickerObserver>,
    shared: Arc<SessionShared>,
}

impl MacosScreenCaptureSession {
    pub fn new(request: MacosStreamRequest) -> Result<Self, MacosCaptureError> {
        request.cadence.timescale()?;
        let mtm = MainThreadMarker::new().ok_or(MacosCaptureError::NotMainThread)?;
        let status = if CGPreflightScreenCaptureAccess() {
            MacosProtectedSourceState::NeedsSelection
        } else {
            MacosProtectedSourceState::NeedsUserAction
        };
        let shared = Arc::new(SessionShared::new(status));
        let observer = PickerObserver::new(mtm, request, Arc::clone(&shared));
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
        Ok(Self {
            picker,
            observer,
            shared,
        })
    }

    pub fn screen_authorized() -> bool {
        CGPreflightScreenCaptureAccess()
    }

    pub fn request_authorization(&self) -> MacosProtectedSourceState {
        let status = if CGRequestScreenCaptureAccess() {
            MacosProtectedSourceState::NeedsSelection
        } else {
            MacosProtectedSourceState::PermissionDenied
        };
        self.shared.set_status(status);
        status
    }

    pub fn present_picker(&self) -> Result<(), MacosCaptureError> {
        if !CGPreflightScreenCaptureAccess() {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsUserAction);
            return Err(MacosCaptureError::ScreenCapturePermissionRequired);
        }
        self.observer.present(&self.picker);
        Ok(())
    }

    pub fn status(&self) -> MacosProtectedSourceState {
        self.shared.status()
    }

    pub fn mailbox(&self) -> MacosFrameMailbox {
        self.shared.mailbox.clone()
    }

    pub fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        self.shared.diagnostics()
    }

    pub fn stop(&self) {
        self.observer.stop();
        self.shared.set_status(MacosProtectedSourceState::ReadyIdle);
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

impl Drop for MacosScreenCaptureSession {
    fn drop(&mut self) {
        self.observer.stop();
        // SAFETY: MainThreadOnly ownership prevents this session from moving
        // to another thread while registered with the picker.
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
) -> Result<(Retained<SCStreamConfiguration>, bool), MacosCaptureError> {
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
    Ok((configuration, display_filter))
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

pub(crate) fn decode_sample(
    decoder: &mut MacosFrameDecoder,
    sample: &CMSampleBuffer,
    cursor_composed: bool,
) -> Result<MacosFrameEvent, MacosCaptureError> {
    // SAFETY: ScreenCaptureKit supplied a live CMSampleBuffer reference for
    // the duration of this callback.
    if !unsafe { sample.is_valid() } {
        return Err(MacosCaptureError::InvalidSampleBuffer);
    }
    // SAFETY: The same callback lifetime makes the sample reference valid.
    if !unsafe { sample.data_is_ready() } {
        return Err(MacosCaptureError::SampleDataNotReady);
    }

    let attachments = FrameAttachments::from_sample(sample)?;
    let raw_attachments = attachments.decode();
    let status = match raw_attachments.status {
        MacosAttachment::Value(status) => MacosFrameStatus::try_from(status)?,
        MacosAttachment::Missing => return Err(MacosCaptureError::MissingAttachment("status")),
        MacosAttachment::Malformed => {
            return Err(MacosCaptureError::MalformedAttachment("status"));
        }
    };
    if status != MacosFrameStatus::Complete {
        return decoder.decode(MacosRawCaptureSample {
            frame: None,
            attachments: raw_attachments,
        });
    }

    // SAFETY: The valid, ready sample is retained by the callback while Core
    // Media returns a retained image-buffer owner.
    let pixel_buffer =
        unsafe { sample.image_buffer() }.ok_or(MacosCaptureError::MissingFramePayload)?;
    let frame = decode_complete_frame(pixel_buffer, cursor_composed)?;
    decoder.decode(MacosRawCaptureSample {
        frame: Some(frame),
        attachments: raw_attachments,
    })
}

fn decode_complete_frame(
    pixel_buffer: CFRetained<CVPixelBuffer>,
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
    let surface = MacosCaptureSurface::from_pixel_buffer(pixel_buffer)?;

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
