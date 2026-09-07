use super::{
    Arc, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess, DefinedClass,
    HYPERCOLOR_UI_BUNDLE_IDENTIFIER, MacosCaptureCallbackDiagnostics, MacosCaptureCapabilities,
    MacosCaptureError, MacosCaptureSelection, MacosCaptureSelector, MacosFrameMailbox,
    MacosNativeTransactionError, MacosProtectedSourceState, MacosScreenCaptureSession,
    MacosScreenshotReferenceCapability, MacosScreenshotReferenceCapture,
    MacosScreenshotReferenceSet, MacosStreamDiagnosticTransaction, MacosStreamRequest,
    MacosStreamRequestTransaction, MacosTahoeSelectionCapabilities, MainThreadBound,
    MainThreadMarker, MainThreadSession, NSArray, NSNumber, NSString,
    NativeScreenshotCaptureBackend, PickerObserver, PoolBackingLifetime, PoolObservation,
    PoolReservationFactory, ProtocolObject, Retained, SCContentSharingPicker,
    SCContentSharingPickerConfiguration, SCContentSharingPickerMode,
    SCContentSharingPickerObserver, ScreenshotIdentityFence, SessionShared, SourceResolution,
    execute_screenshot_transaction, fmt, native_capture_capabilities, resolve_display_selector,
};

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

impl fmt::Debug for MacosScreenCaptureSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MacosScreenCaptureSession")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}
