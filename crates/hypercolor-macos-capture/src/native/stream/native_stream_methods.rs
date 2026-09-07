use super::{
    AnyThread, Arc, AtomicBool, CaptureOutput, CompletionFence, CompletionWitness, DefinedClass,
    DispatchQueue, DispatchQueueAttr, InterruptedRestage, InterruptedRestagePlan,
    LatestSampleWorker, MacosCaptureError, MacosFrameDecoder, MacosStreamDeliveryValidator,
    MacosStreamRequest, NSError, NativeLifecycle, NativeSelectionFilter, NativeStream,
    NativeStreamControl, Ordering, PoolReservationFactory, ProtocolObject, RcBlock,
    RetainedNativeSample, SCStream, SCStreamDelegate, SCStreamOutput, SCStreamOutputType,
    SessionShared, StreamSlot, Weak, classify_stream_error, conservative_pool_quote, decode_sample,
    invoke_stream_start, native_error, publish_decoded_result, stream_configuration,
};

impl NativeStreamControl {
    pub(super) fn enqueue_start(
        &self,
        epoch: u64,
        streams: Weak<StreamSlot>,
        shared: Arc<SessionShared>,
        completion: CompletionWitness,
    ) {
        let control = self.clone();
        self.queue.exec_async(move || {
            control.invoke_start(epoch, streams, shared, completion);
        });
    }

    pub(super) fn enqueue_stop(&self, shared: Arc<SessionShared>, completion: CompletionWitness) {
        let control = self.clone();
        self.queue.exec_async(move || {
            control.invoke_stop(shared, completion);
        });
    }

    pub(super) fn invoke_start(
        &self,
        epoch: u64,
        streams: Weak<StreamSlot>,
        shared: Arc<SessionShared>,
        completion: CompletionWitness,
    ) {
        if !streams
            .upgrade()
            .is_some_and(|streams| streams.accepts_epoch(epoch))
        {
            return;
        }
        self.start_invoked.store(true, Ordering::Release);
        invoke_stream_start(&self.stream, epoch, streams, shared, completion);
    }

    pub(super) fn invoke_stop(&self, shared: Arc<SessionShared>, completion: CompletionWitness) {
        if !self.start_invoked.load(Ordering::Acquire) {
            let _ = completion.complete();
            return;
        }
        let callback = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: ScreenCaptureKit supplies either null or a live
            // NSError for the duration of this callback.
            if let Some(error) = unsafe { error.as_ref() } {
                shared
                    .record_retirement_error(&native_error("stop ScreenCaptureKit stream", error));
            }
            let _ = completion.complete();
        });
        // SAFETY: ScreenCaptureKit copies the completion block. The
        // retirement registry retains the stream until it invokes or
        // destroys that block and the decode worker has retired.
        unsafe {
            self.stream
                .stopCaptureWithCompletionHandler(Some(&callback));
        }
    }
}

impl NativeStream {
    pub(super) fn prepare(
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
        let control_queue = DispatchQueue::new(
            "tech.hyperbliss.hypercolor.screen-capture-control",
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
            control: NativeStreamControl {
                stream: stream.clone(),
                queue: control_queue,
                start_invoked: Arc::new(AtomicBool::new(false)),
            },
            stream,
            filter: selection_filter.filter,
            selection: selection_filter.selection,
            source_id: selection_filter.source_id,
            request,
            reserve_pool: Arc::clone(reserve_pool),
            worker,
            start_completion: CompletionFence::new(),
            _output: output,
            _output_queue: queue,
        })
    }

    pub(super) fn epoch(&self) -> u64 {
        self._output.ivars().epoch
    }

    pub(super) fn finish_worker_retirement(&mut self) -> Result<(), MacosCaptureError> {
        self.worker.close();
        self.worker
            .join()
            .map_err(|_| MacosCaptureError::CaptureWorkerPanicked)
    }

    pub(super) fn interruption_restage(&self, selection_revision: u64) -> InterruptedRestagePlan {
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
