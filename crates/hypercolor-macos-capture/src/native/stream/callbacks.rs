use super::{
    Arc, CompletionWitness, MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT, MacosCaptureError,
    MacosNativeTransactionError, MacosNativeTransactionPhase, MacosProtectedSourceState, NSError,
    RcBlock, SCStream, ScreenshotIdentityFence, SessionShared, StreamRole, StreamSlot, Weak,
    classify_stream_error, lock, native_error,
};

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

pub(super) fn invoke_stream_start(
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

pub(super) fn dispatch_stream_start_success(streams: &Weak<StreamSlot>, epoch: u64) {
    let Some(streams) = streams.upgrade() else {
        return;
    };
    match streams.arm_candidate_deadline(
        epoch,
        MacosNativeTransactionPhase::FirstCompleteFrame,
        MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT,
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

pub(super) fn handle_stream_error(
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

pub(super) fn dispatch_owned_stream_error(
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

pub(super) fn handle_owned_stream_error(
    streams: &Arc<StreamSlot>,
    epoch: u64,
    shared: &SessionShared,
    state: MacosProtectedSourceState,
    error: MacosCaptureError,
) {
    handle_owned_stream_error_with(streams, epoch, shared, state, error, || {});
}

pub(super) fn handle_owned_stream_error_with(
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

pub(super) fn handle_fatal_stream_error(
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

pub(super) fn handle_owned_fatal_stream_error(
    streams: &Arc<StreamSlot>,
    epoch: u64,
    shared: Arc<SessionShared>,
    error: MacosCaptureError,
) {
    handle_owned_fatal_stream_error_with(streams, epoch, shared, error, || {});
}

pub(super) fn handle_owned_fatal_stream_error_with(
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
