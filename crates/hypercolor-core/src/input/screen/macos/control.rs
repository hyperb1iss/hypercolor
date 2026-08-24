use super::{
    CaptureConfig, Context, MacosCaptureCadence, MacosStreamRequest, NativeCaptureCapabilities,
    ScreenCaptureCadence, ScreenCaptureDemand, ScreenCursorPolicy, StreamRequest,
};
use super::{
    Instant, MacosCaptureCallbackDiagnostics, MacosCaptureControl, MacosCaptureSelection,
    MacosFrameMailbox, MacosScreenAuthorizationState, MacosScreenCaptureSession,
    MacosScreenshotReferenceCapture, NativeCaptureControl, NativeProtectedSourceState,
    NativeTahoeSelectionCapabilities, mpsc,
};

impl StreamRequest {
    #[cfg(feature = "macos-capture-fixtures")]
    pub(super) fn completed(generation: u64, result: anyhow::Result<()>) -> Self {
        Self {
            generation,
            completion: Box::new(|| result),
        }
    }

    pub(super) fn wait(self) -> anyhow::Result<()> {
        (self.completion)().with_context(|| {
            format!(
                "macOS stream request generation {} did not commit",
                self.generation
            )
        })
    }
}

impl MacosCaptureControl for NativeCaptureControl {
    fn mailbox(&self) -> MacosFrameMailbox {
        self.session.mailbox()
    }

    fn set_active(&self, active: bool) {
        self.session.set_capture_active(active);
    }

    fn present_picker(&self) -> anyhow::Result<()> {
        self.session.present_picker().map_err(anyhow::Error::from)
    }

    fn request_authorization(&self) -> NativeProtectedSourceState {
        self.session.request_authorization()
    }

    fn status(&self) -> NativeProtectedSourceState {
        self.session.status()
    }

    fn selection(&self) -> MacosCaptureSelection {
        self.session.selection()
    }

    fn selection_revision(&self) -> u64 {
        self.session.selection_revision()
    }

    fn begin_stream_request(&self, request: MacosStreamRequest) -> anyhow::Result<StreamRequest> {
        let transaction = self.session.begin_stream_request(request)?;
        let generation = transaction.generation();
        Ok(StreamRequest {
            generation,
            completion: Box::new(move || transaction.wait().map_err(anyhow::Error::from)),
        })
    }

    fn tahoe_selection_capabilities(&self) -> Option<NativeTahoeSelectionCapabilities> {
        self.session.tahoe_selection_capabilities()
    }

    fn host_capabilities(&self) -> NativeCaptureCapabilities {
        self.host_capabilities
    }

    fn authorization(&self) -> MacosScreenAuthorizationState {
        if MacosScreenCaptureSession::screen_authorized() {
            MacosScreenAuthorizationState::Authorized
        } else if self.session.status() == NativeProtectedSourceState::PermissionDenied {
            MacosScreenAuthorizationState::Denied
        } else {
            MacosScreenAuthorizationState::NotDetermined
        }
    }

    fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        self.session.diagnostics()
    }

    fn captured_at(&self, display_time: u64) -> anyhow::Result<Instant> {
        self.clock
            .timestamp(display_time)
            .map_err(anyhow::Error::from)
    }

    fn capture_screenshot_reference(
        &self,
    ) -> anyhow::Result<
        mpsc::Receiver<
            Result<MacosScreenshotReferenceCapture, hypercolor_macos_capture::MacosCaptureError>,
        >,
    > {
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        self.session
            .capture_screenshot_reference_with_identity(move |result| {
                let _ = result_tx.send(result);
            })?;
        Ok(result_rx)
    }
}

pub(super) fn production_stream_request(
    config: &CaptureConfig,
    demand: ScreenCaptureDemand,
    capabilities: NativeCaptureCapabilities,
) -> anyhow::Result<MacosStreamRequest> {
    let cadence = demand
        .cadence()
        .unwrap_or(ScreenCaptureCadence::Configured)
        .resolve(config.acquisition_cadence);
    let cadence = match cadence {
        ScreenCaptureCadence::Configured => MacosCaptureCadence::FramesPerSecond(config.target_fps),
        ScreenCaptureCadence::NativeRefresh => MacosCaptureCadence::NativeRefresh,
        ScreenCaptureCadence::FramesPerSecond(frames_per_second) => {
            MacosCaptureCadence::FramesPerSecond(frames_per_second.get())
        }
    };
    let cursor_composed = matches!(demand.cursor(), Some(ScreenCursorPolicy::Include));
    Ok(MacosStreamRequest::for_capabilities(
        cadence,
        cursor_composed,
        capabilities,
    )?)
}
