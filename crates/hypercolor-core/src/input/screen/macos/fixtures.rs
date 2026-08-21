use super::{
    Arc, AtomicBool, AtomicU64, CaptureConfig, Instant, MacosCaptureCallbackDiagnostics,
    MacosCaptureControl, MacosCaptureFrame, MacosCaptureSelection, MacosFrameEvent,
    MacosFrameMailbox, MacosHostArchitecture, MacosRuntimeCapability,
    MacosScreenAuthorizationState, MacosScreenCaptureInput, MacosScreenRuntimeTelemetry,
    MacosStreamRequest, MacosTahoeRuntimeProbes, Mutex, NativeCaptureCapabilities,
    NativeProtectedSourceState, NativeTahoeSelectionCapabilities, Ordering,
    ScreenByteAdmissionCoordinator, ScreenComputeCapacityPolicy, StreamRequest, anyhow, lock, mpsc,
};

#[cfg(feature = "macos-capture-fixtures")]
pub(super) struct FixtureControl {
    mailbox: MacosFrameMailbox,
    active: AtomicBool,
    pub(super) active_transitions: AtomicU64,
    status: Mutex<NativeProtectedSourceState>,
    selection: Mutex<MacosCaptureSelection>,
    selection_revision: AtomicU64,
    stream_request: Mutex<MacosStreamRequest>,
    pending_stream_request: Mutex<Option<FixturePendingStreamRequest>>,
    stream_request_transitions: AtomicU64,
    reject_next_stream_request: AtomicBool,
    defer_next_stream_request: AtomicBool,
    tahoe_selection: Mutex<Option<NativeTahoeSelectionCapabilities>>,
    host_capabilities: Mutex<NativeCaptureCapabilities>,
    captured_at: Mutex<Option<Instant>>,
    diagnostics: Mutex<MacosCaptureCallbackDiagnostics>,
}

#[cfg(feature = "macos-capture-fixtures")]
struct FixturePendingStreamRequest {
    generation: u64,
    request: MacosStreamRequest,
    completion: mpsc::SyncSender<anyhow::Result<()>>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl Default for FixtureControl {
    fn default() -> Self {
        Self {
            mailbox: MacosFrameMailbox::default(),
            active: AtomicBool::new(false),
            active_transitions: AtomicU64::new(0),
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            selection: Mutex::new(MacosCaptureSelection::None),
            selection_revision: AtomicU64::new(0),
            stream_request: Mutex::new(MacosStreamRequest::default()),
            pending_stream_request: Mutex::new(None),
            stream_request_transitions: AtomicU64::new(0),
            reject_next_stream_request: AtomicBool::new(false),
            defer_next_stream_request: AtomicBool::new(false),
            tahoe_selection: Mutex::new(None),
            host_capabilities: Mutex::new(NativeCaptureCapabilities::from_runtime(
                MacosHostArchitecture::AppleSilicon,
                true,
                MacosTahoeRuntimeProbes {
                    content_tone_mapping_info_symbol: MacosRuntimeCapability::Present,
                    screenshot_configuration_class: MacosRuntimeCapability::Present,
                    screenshot_dynamic_range_selector: MacosRuntimeCapability::Present,
                    screenshot_capture_selector: MacosRuntimeCapability::Present,
                },
            )),
            captured_at: Mutex::new(None),
            diagnostics: Mutex::new(MacosCaptureCallbackDiagnostics::default()),
        }
    }
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosCaptureControl for FixtureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.mailbox.clone()
    }

    fn set_active(&self, active: bool) {
        let previous = self.active.swap(active, Ordering::AcqRel);
        if previous == active {
            return;
        }
        self.active_transitions.fetch_add(1, Ordering::AcqRel);
        if !active {
            *lock(&self.tahoe_selection) = None;
            self.selection_revision.fetch_add(1, Ordering::AcqRel);
        }
        *lock(&self.status) = if active {
            NativeProtectedSourceState::Starting
        } else {
            NativeProtectedSourceState::ReadyIdle
        };
    }

    fn present_picker(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn request_authorization(&self) -> NativeProtectedSourceState {
        *lock(&self.status) = NativeProtectedSourceState::NeedsSelection;
        NativeProtectedSourceState::NeedsSelection
    }

    fn status(&self) -> NativeProtectedSourceState {
        *lock(&self.status)
    }

    fn selection(&self) -> MacosCaptureSelection {
        lock(&self.selection).clone()
    }

    fn selection_revision(&self) -> u64 {
        self.selection_revision.load(Ordering::Acquire)
    }

    fn begin_stream_request(&self, request: MacosStreamRequest) -> anyhow::Result<StreamRequest> {
        if self
            .reject_next_stream_request
            .swap(false, Ordering::AcqRel)
        {
            anyhow::bail!("fixture rejected macOS stream request");
        }
        let generation = self
            .stream_request_transitions
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        if self.defer_next_stream_request.swap(false, Ordering::AcqRel) {
            let (completion, receiver) = mpsc::sync_channel(1);
            *lock(&self.pending_stream_request) = Some(FixturePendingStreamRequest {
                generation,
                request,
                completion,
            });
            return Ok(StreamRequest {
                generation,
                completion: Box::new(move || {
                    receiver
                        .recv()
                        .map_err(|_| anyhow!("fixture stream request completion was lost"))?
                }),
            });
        }
        *lock(&self.stream_request) = request;
        Ok(StreamRequest::completed(generation, Ok(())))
    }

    fn tahoe_selection_capabilities(&self) -> Option<NativeTahoeSelectionCapabilities> {
        lock(&self.tahoe_selection).clone()
    }

    fn host_capabilities(&self) -> NativeCaptureCapabilities {
        *lock(&self.host_capabilities)
    }

    fn authorization(&self) -> MacosScreenAuthorizationState {
        match self.status() {
            NativeProtectedSourceState::PermissionDenied | NativeProtectedSourceState::Revoked => {
                MacosScreenAuthorizationState::Denied
            }
            NativeProtectedSourceState::NeedsUserAction => {
                MacosScreenAuthorizationState::NotDetermined
            }
            NativeProtectedSourceState::Disabled => MacosScreenAuthorizationState::Unknown,
            _ => MacosScreenAuthorizationState::Authorized,
        }
    }

    fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        *lock(&self.diagnostics)
    }

    fn captured_at(&self, _display_time: u64) -> anyhow::Result<Instant> {
        Ok(lock(&self.captured_at).take().unwrap_or_else(Instant::now))
    }
}

#[cfg(feature = "macos-capture-fixtures")]
pub struct MacosScreenCaptureFixture {
    pub(super) control: Arc<FixtureControl>,
}

#[cfg(feature = "macos-capture-fixtures")]
impl MacosScreenCaptureFixture {
    pub fn source(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> (MacosScreenCaptureInput, Self) {
        Self::source_with_compute_capacity_policy(
            config,
            admission,
            ScreenComputeCapacityPolicy::UNBOUNDED,
        )
    }

    pub fn source_with_compute_capacity_policy(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> (MacosScreenCaptureInput, Self) {
        let control = Arc::new(FixtureControl {
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            ..FixtureControl::default()
        });
        let source = MacosScreenCaptureInput::with_control_and_telemetry(
            config,
            admission,
            compute_capacity_policy,
            control.clone(),
            Arc::new(MacosScreenRuntimeTelemetry::default()),
            "screen_capture_kit_fixture",
        );
        (source, Self { control })
    }

    #[cfg(test)]
    pub(super) fn renderer_authoritative_source(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> (MacosScreenCaptureInput, Self) {
        let control = Arc::new(FixtureControl {
            status: Mutex::new(NativeProtectedSourceState::ReadyIdle),
            ..FixtureControl::default()
        });
        let source = MacosScreenCaptureInput::with_control_and_telemetry(
            config,
            admission,
            ScreenComputeCapacityPolicy::UNBOUNDED,
            control.clone(),
            Arc::new(MacosScreenRuntimeTelemetry::renderer_authoritative()),
            "screen_capture_kit_native",
        );
        (source, Self { control })
    }

    pub fn publish(&self, frame: MacosCaptureFrame) {
        *lock(&self.control.status) = NativeProtectedSourceState::Live;
        let mut diagnostics = lock(&self.control.diagnostics);
        diagnostics.frames_received = diagnostics.frames_received.saturating_add(1);
        diagnostics.frames_published = diagnostics.frames_published.saturating_add(1);
        drop(diagnostics);
        self.control
            .mailbox
            .publish(Ok(MacosFrameEvent::Frame(Box::new(frame))));
    }

    pub fn publish_at(&self, frame: MacosCaptureFrame, captured_at: Instant) {
        *lock(&self.control.captured_at) = Some(captured_at);
        self.publish(frame);
    }

    pub fn publish_recoverable_error(&self, error: hypercolor_macos_capture::MacosCaptureError) {
        self.control
            .mailbox
            .publish(Ok(MacosFrameEvent::RecoverableError(Box::new(error))));
    }

    pub fn is_active(&self) -> bool {
        self.control.active.load(Ordering::Acquire)
    }

    pub fn set_selection(&self, selection: MacosCaptureSelection) {
        *lock(&self.control.tahoe_selection) = None;
        let mut current = lock(&self.control.selection);
        if *current != selection {
            *current = selection;
            self.control
                .selection_revision
                .fetch_add(1, Ordering::AcqRel);
        }
    }

    pub fn selection_revision(&self) -> u64 {
        self.control.selection_revision.load(Ordering::Acquire)
    }

    pub fn stream_request(&self) -> MacosStreamRequest {
        *lock(&self.control.stream_request)
    }

    pub fn stream_request_transitions(&self) -> u64 {
        self.control
            .stream_request_transitions
            .load(Ordering::Acquire)
    }

    pub fn active_transitions(&self) -> u64 {
        self.control.active_transitions.load(Ordering::Acquire)
    }

    pub fn reject_next_stream_request(&self) {
        self.control
            .reject_next_stream_request
            .store(true, Ordering::Release);
    }

    pub fn defer_next_stream_request(&self) {
        self.control
            .defer_next_stream_request
            .store(true, Ordering::Release);
    }

    pub fn pending_stream_request(&self) -> Option<MacosStreamRequest> {
        lock(&self.control.pending_stream_request)
            .as_ref()
            .map(|pending| pending.request)
    }

    pub fn commit_pending_stream_request(&self) {
        let pending = lock(&self.control.pending_stream_request)
            .take()
            .expect("fixture stream request should be pending");
        *lock(&self.control.stream_request) = pending.request;
        let _ = pending.completion.send(Ok(()));
    }

    pub fn fail_pending_stream_request(&self) {
        let pending = lock(&self.control.pending_stream_request)
            .take()
            .expect("fixture stream request should be pending");
        let _ = pending.completion.send(Err(anyhow!(
            "fixture stream request generation {} failed asynchronously",
            pending.generation
        )));
    }

    pub fn set_tahoe_selection_capabilities(
        &self,
        capabilities: Option<NativeTahoeSelectionCapabilities>,
    ) {
        *lock(&self.control.tahoe_selection) = capabilities;
    }

    pub fn set_host_capabilities(&self, capabilities: NativeCaptureCapabilities) {
        *lock(&self.control.host_capabilities) = capabilities;
    }
}
