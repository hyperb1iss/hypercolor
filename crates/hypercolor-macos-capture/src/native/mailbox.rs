use super::*;

pub(super) struct RetainedNativeSample {
    pub(super) attachments: MacosRawFrameAttachments,
    pub(super) pixel_buffer: CFRetained<CVPixelBuffer>,
    pub(super) admission_lifetime: PoolBackingLifetime,
    pub(super) cursor_composed: bool,
}

pub(super) enum RetainedNativeDelivery<T = RetainedNativeSample> {
    Complete(T),
    Lifecycle(MacosFrameStatus),
}

pub(super) fn route_retained_delivery<T>(
    delivery: RetainedNativeDelivery<T>,
    complete: impl FnOnce(T),
    lifecycle: impl FnOnce(MacosFrameStatus),
) {
    match delivery {
        RetainedNativeDelivery::Complete(sample) => complete(sample),
        RetainedNativeDelivery::Lifecycle(status) => lifecycle(status),
    }
}

pub(super) struct DecodedSample {
    pub(super) event: MacosFrameEvent,
    pub(super) confirmed_delivery: Option<MacosValidatedStreamDelivery>,
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

pub(super) fn with_admitted_surface<T>(
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

pub(super) fn publish_decoded_result(
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

pub(super) struct CaptureOutputIvars {
    pub(super) samples: LatestSampleInput<RetainedNativeSample>,
    pub(super) pool: PoolObservation,
    pub(super) shared: Arc<SessionShared>,
    pub(super) streams: Weak<StreamSlot>,
    pub(super) epoch: u64,
    pub(super) cursor_composed: bool,
    pub(super) display_filter: bool,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HypercolorScreenCaptureOutput"]
    #[ivars = CaptureOutputIvars]
    pub(super) struct CaptureOutput;

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

pub(super) fn route_stream_lifecycle<T>(
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

pub(super) fn route_stream_activity<T>(
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
    pub(super) fn new(
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
