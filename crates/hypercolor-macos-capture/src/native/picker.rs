use super::*;

#[derive(Clone)]
pub(super) enum NativeFilter {
    System(Retained<SCContentFilter>),
    #[cfg(test)]
    Fixture(u64),
}

// SAFETY: SCContentFilter is immutable after picker delivery and remains in
// the process that owns every consuming SCStream. Rust never mutates it.
unsafe impl Send for NativeFilter {}

impl NativeFilter {
    pub(super) fn system(&self) -> &SCContentFilter {
        match self {
            Self::System(filter) => filter,
            #[cfg(test)]
            Self::Fixture(_) => panic!("fixture selection has no native filter"),
        }
    }
}

#[derive(Clone)]
pub(super) struct NativeSelectionFilter {
    pub(super) filter: NativeFilter,
    pub(super) selection: MacosCaptureSelection,
    pub(super) source_id: Arc<str>,
}

impl NativeSelectionFilter {
    fn retain(filter: &SCContentFilter) -> Result<Self, MacosCaptureError> {
        let selection = selection_from_filter(filter)?;
        let source_id = selection_source_id(filter, &selection);
        // SAFETY: The picker or configured-source callback supplies a live
        // immutable filter, and each owner stays process-local.
        let filter = unsafe {
            Retained::retain(ptr::from_ref(filter).cast_mut())
                .ok_or(MacosCaptureError::RetainNativeFilterFailed)?
        };
        Ok(Self {
            filter: NativeFilter::System(filter),
            selection,
            source_id,
        })
    }

    #[cfg(test)]
    pub(super) fn fixture(id: u64) -> Self {
        let source_id: Arc<str> = Arc::from(format!("fixture:{id}"));
        Self {
            filter: NativeFilter::Fixture(id),
            selection: MacosCaptureSelection::Display {
                source_id: Arc::clone(&source_id),
            },
            source_id,
        }
    }

    #[cfg(test)]
    pub(super) fn fixture_id(&self) -> u64 {
        match &self.filter {
            NativeFilter::Fixture(id) => *id,
            NativeFilter::System(_) => panic!("native filter has no fixture identity"),
        }
    }
}

pub(super) struct PickerObserverIvars {
    pub(super) shared: Arc<SessionShared>,
    pub(super) streams: Arc<StreamSlot>,
    pub(super) reserve_pool: PoolReservationFactory,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "HypercolorContentSharingPickerObserver"]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerObserverIvars]
    pub(super) struct PickerObserver;

    unsafe impl NSObjectProtocol for PickerObserver {}

    unsafe impl SCContentSharingPickerObserver for PickerObserver {
        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPicker:didCancelForStream:))]
        fn contentSharingPicker_didCancelForStream(
            &self,
            _picker: &SCContentSharingPicker,
            _stream: Option<&SCStream>,
        ) {
            let Some(resolution) = self.ivars().shared.picker_resolution() else {
                return;
            };
            let settlement = self.ivars().streams.cancel_source_transaction(&resolution);
            self.ivars().streams.finalize_picker_cancel(&resolution);
            if let Some(settlement) = settlement {
                settlement.publish();
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
            let Some(resolution) = self.ivars().shared.picker_resolution() else {
                return;
            };
            let settlement = self.ivars().streams.claim_source_transaction(&resolution);
            accept_filter(
                &self.ivars().streams,
                &self.ivars().shared,
                self.ivars().streams.request(),
                &self.ivars().reserve_pool,
                filter,
                true,
                ClaimedSourceResolution {
                    resolution,
                    settlement,
                },
            );
        }

        #[allow(non_snake_case)]
        #[unsafe(method(contentSharingPickerStartDidFailWithError:))]
        fn contentSharingPickerStartDidFailWithError(&self, error: &NSError) {
            let Some(resolution) = self.ivars().shared.picker_resolution() else {
                return;
            };
            let settlement = self.ivars().streams.claim_source_transaction(&resolution);
            let error = native_error("ScreenCaptureKit picker", error);
            self.ivars()
                .streams
                .finalize_picker_failure(&resolution, error);
            if let Some(settlement) = settlement {
                settlement.publish();
            }
        }
    }
);

impl PickerObserver {
    pub(super) fn new(
        mtm: MainThreadMarker,
        request: MacosStreamRequest,
        shared: Arc<SessionShared>,
        reserve_pool: PoolReservationFactory,
    ) -> Result<Retained<Self>, MacosCaptureError> {
        let streams = StreamSlot::new(Arc::clone(&shared), request)?;
        let this = mtm.alloc::<Self>().set_ivars(PickerObserverIvars {
            shared,
            streams,
            reserve_pool,
        });
        // SAFETY: NSObject has no additional initialization requirements for
        // this main-thread observer subclass.
        Ok(unsafe { msg_send![super(this), init] })
    }

    pub(super) fn request(&self) -> MacosStreamRequest {
        self.ivars().streams.request()
    }

    pub(super) fn set_request(
        &self,
        request: MacosStreamRequest,
    ) -> Result<MacosStreamRequestTransaction, MacosCaptureError> {
        self.ivars()
            .streams
            .set_request(request, &self.ivars().reserve_pool)
    }

    pub(super) fn present(&self, picker: &SCContentSharingPicker) {
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

    pub(super) fn set_active(&self, active: bool) {
        if !active {
            if !self.ivars().streams.set_capture_active(false) {
                return;
            }
            let status = if self.ivars().streams.has_selection() {
                MacosProtectedSourceState::ReadyIdle
            } else {
                MacosProtectedSourceState::NeedsSelection
            };
            self.ivars().shared.set_status(status);
            return;
        }
        match self.ivars().streams.begin_capture_activation() {
            Ok(CaptureActivation::Unchanged) => {}
            Ok(CaptureActivation::NeedsSelection) => self
                .ivars()
                .shared
                .set_status(MacosProtectedSourceState::NeedsSelection),
            Ok(CaptureActivation::Candidate {
                reservation,
                request,
            }) => {
                if let Err(failure) = self.ivars().streams.prepare_and_start_candidate(
                    *reservation,
                    request,
                    &self.ivars().reserve_pool,
                ) {
                    self.ivars()
                        .streams
                        .finalize_candidate_preparation_failure(failure, None);
                }
            }
            Err(error) => self.ivars().shared.counters.record_drop(&error),
        }
    }

    pub(super) fn stop(&self) {
        self.ivars().streams.set_capture_active(false);
    }
}

fn accept_filter(
    streams: &Arc<StreamSlot>,
    shared: &Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: &PoolReservationFactory,
    filter: &SCContentFilter,
    picker: bool,
    claimed: ClaimedSourceResolution,
) {
    let ClaimedSourceResolution {
        resolution,
        settlement,
    } = claimed;
    let commit = || {
        let diagnostic = matches!(resolution, SourceResolution::Diagnostic(_));
        let selection_filter = match NativeSelectionFilter::retain(filter) {
            Ok(selection_filter) => selection_filter,
            Err(error) => {
                streams.finalize_resolution_error(&resolution, picker, error);
                return;
            }
        };
        let epoch = match streams.allocate_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                streams.finalize_resolution_error(&resolution, picker, error);
                return;
            }
        };
        match streams.accept_selection_filter(
            selection_filter,
            request,
            epoch,
            resolution.clone(),
            picker,
        ) {
            Ok(FilterAcceptance::Stale) => {}
            Ok(FilterAcceptance::Stored(replaced)) => {
                if let Some(replaced) = replaced {
                    streams.stop_stream(replaced);
                }
                if diagnostic {
                    shared
                        .record_stream_diagnostic_result(epoch, MacosProtectedSourceState::Failed);
                }
            }
            Ok(FilterAcceptance::Candidate {
                reservation,
                request,
            }) => match streams.prepare_and_start_candidate(*reservation, request, reserve_pool) {
                Ok(true) => {}
                Ok(false) => {
                    shared
                        .record_stream_diagnostic_result(epoch, MacosProtectedSourceState::Failed);
                }
                Err(failure) => {
                    streams.finalize_candidate_preparation_failure(failure, Some(&resolution));
                }
            },
            Err(error) => {
                streams.finalize_resolution_error(&resolution, false, error);
            }
        }
    };
    commit();
    if let Some(settlement) = settlement {
        settlement.publish();
    }
}

pub(super) struct MainThreadSession {
    pub(super) picker: Retained<SCContentSharingPicker>,
    pub(super) observer: Retained<PickerObserver>,
}

pub(super) fn resolve_display_selector(
    streams: Arc<StreamSlot>,
    shared: Arc<SessionShared>,
    request: MacosStreamRequest,
    reserve_pool: PoolReservationFactory,
    selector: MacosCaptureSelector,
    resolution: SourceResolution,
) -> Result<(), MacosCaptureError> {
    let completion = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            if !source_resolution_is_current(&streams, &shared, &resolution) {
                return;
            }
            let settlement = streams.claim_source_transaction(&resolution);
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
            if !source_resolution_is_current(&streams, &shared, &resolution) {
                if let Some(settlement) = settlement {
                    settlement.publish();
                }
                return;
            }
            match result {
                Ok(filter) => {
                    accept_filter(
                        &streams,
                        &shared,
                        request,
                        &reserve_pool,
                        &filter,
                        false,
                        ClaimedSourceResolution {
                            resolution: resolution.clone(),
                            settlement,
                        },
                    );
                }
                Err(error) => {
                    streams.finalize_resolution_error(&resolution, false, error);
                    if let Some(settlement) = settlement {
                        settlement.publish();
                    }
                }
            }
        },
    );
    // SAFETY: ScreenCaptureKit copies the completion block for asynchronous
    // use. The block owns every Rust value captured by the callback.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&completion) };
    Ok(())
}

fn source_resolution_is_current(
    streams: &StreamSlot,
    shared: &SessionShared,
    resolution: &SourceResolution,
) -> bool {
    shared.source_resolution_is_current(resolution)
        && match resolution {
            SourceResolution::General(_) => true,
            SourceResolution::Diagnostic(diagnostic) => {
                streams.selection_revision() == diagnostic.attempt.selection_revision
            }
        }
}

fn display_filter(
    content: &SCShareableContent,
    selector: &MacosCaptureSelector,
) -> Result<Retained<SCContentFilter>, MacosCaptureError> {
    // SAFETY: Shareable content owns an immutable display snapshot. The
    // returned array and each selected display are retained locally.
    let displays = unsafe { content.displays() };
    // SAFETY: The same immutable shareable-content snapshot retains its
    // returned window array and each member.
    let excluded_windows = unsafe { content.windows() }
        .to_vec()
        .into_iter()
        .filter(|window| {
            // SAFETY: Every retained SCWindow and owning application belongs
            // to this immutable shareable-content snapshot.
            unsafe {
                window.owningApplication().is_some_and(|application| {
                    is_hypercolor_ui_bundle_identifier(&application.bundleIdentifier().to_string())
                })
            }
        })
        .collect::<Vec<_>>();
    let excluded = NSArray::<SCWindow>::from_retained_slice(&excluded_windows);
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
            // SAFETY: The filter retains the selected display and the stable
            // Hypercolor window identities from this content snapshot.
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

pub(super) fn session_selection_source_id(
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
