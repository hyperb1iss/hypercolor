use super::{
    Arc, AtomicU64, DispatchQueue, DispatchQueueAttr, Duration, GeneralSourceResolution, Instant,
    MACOS_NATIVE_SOURCE_TIMEOUT, MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT, MacosCaptureError,
    MacosCaptureSelection, MacosCaptureSelector, MacosNativeTransactionError,
    MacosNativeTransactionPhase, MacosProtectedSourceState, MacosStreamDiagnosticTransaction,
    MacosStreamRequest, Mutex, NativeLifecycle, Ordering, PendingStreamRequest,
    PostAuthorizationStreamDiagnosticAttempt, PostAuthorizationStreamDiagnosticResolution,
    RestartDiagnosticReset, SessionShared, SourceResolution, SourceTransaction, StreamSlot,
    StreamState, TransactionCompleter, TransactionIdentity, TransactionSettlement, lock,
};

impl StreamSlot {
    pub(super) fn new(
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

    pub(super) fn allocate_epoch(&self) -> Result<u64, MacosCaptureError> {
        self.next_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map_err(|_| MacosCaptureError::SequenceExhausted)
    }

    pub(super) fn install_candidate_completion(
        state: &mut StreamState,
        epoch: u64,
        request: Option<&PendingStreamRequest>,
    ) -> Option<TransactionSettlement<()>> {
        let completion = request.map_or_else(
            || {
                TransactionCompleter::new(
                    TransactionIdentity {
                        generation: epoch,
                        phase: MacosNativeTransactionPhase::StreamStart,
                    },
                    Some(Instant::now() + MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT),
                )
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

    pub(super) fn cancel_candidate_completion(
        state: &mut StreamState,
    ) -> Option<TransactionSettlement<()>> {
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

    pub(super) fn finish_replaced_candidate(settlement: Option<TransactionSettlement<()>>) {
        if let Some(settlement) = settlement {
            settlement.publish();
        }
    }

    pub(super) fn arm_candidate_deadline(
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

    pub(super) fn timeout_candidate(&self, epoch: u64, phase: MacosNativeTransactionPhase) {
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

    pub(super) fn begin_resolution(
        self: &Arc<Self>,
    ) -> Result<SourceResolution, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let resolution_epoch = self.shared.begin_resolution()?;
        let resolution = SourceResolution::General(GeneralSourceResolution {
            resolution_epoch,
            selector: self.shared.selector(),
        });
        self.install_source_transaction(resolution.clone(), Some(MACOS_NATIVE_SOURCE_TIMEOUT))?;
        Ok(resolution)
    }

    pub(super) fn begin_picker_resolution(
        self: &Arc<Self>,
    ) -> Result<SourceResolution, MacosCaptureError> {
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

    pub(super) fn set_selector(&self, selector: MacosCaptureSelector) {
        let _lifecycle = lock(&self.lifecycle_start);
        let source_settlement = self.cancel_source_transaction_locked();
        self.shared.disable_picker_callbacks();
        self.shared.set_selector(selector);
        if let Some(settlement) = source_settlement {
            settlement.publish();
        }
    }

    pub(super) fn set_selector_and_begin_resolution(
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
    pub(super) fn begin_restart_diagnostic(
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

    pub(super) fn reset_for_restart_diagnostic_locked(
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

    pub(super) fn setup_restart_diagnostic(
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

    pub(super) fn setup_restart_diagnostic_with(
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

    pub(super) fn arm_restart_diagnostic(
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

    pub(super) fn finish_restart_diagnostic(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
    ) {
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

    pub(super) fn install_source_transaction(
        self: &Arc<Self>,
        resolution: SourceResolution,
        timeout: Option<Duration>,
    ) -> Result<(), MacosCaptureError> {
        let generation = match &resolution {
            SourceResolution::General(resolution) => resolution.resolution_epoch,
            SourceResolution::Diagnostic(resolution) => resolution.resolution_epoch,
        };
        let completion = TransactionCompleter::new(
            TransactionIdentity {
                generation,
                phase: MacosNativeTransactionPhase::SourceResolution,
            },
            None,
        );
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

    pub(super) fn claim_source_transaction(
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

    pub(super) fn cancel_source_transaction(
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

    pub(super) fn cancel_source_transaction_locked(&self) -> Option<TransactionSettlement<()>> {
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

    pub(super) fn timeout_source_resolution(&self, resolution: SourceResolution) {
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
}
