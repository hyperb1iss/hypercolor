use super::*;

#[derive(Clone)]
pub(super) enum ScreenshotFilterHandle {
    Native(NativeFilter),
    #[cfg(test)]
    Fixture(u64),
}

#[derive(Clone)]
pub(super) struct ScreenshotTransactionSnapshot {
    pub(super) filter: ScreenshotFilterHandle,
    pub(super) source_id: Arc<str>,
    pub(super) generation: u64,
    pub(super) selection_revision: u64,
    pub(super) capability: MacosScreenshotReferenceCapability,
}

pub(super) type ScreenshotCompletion =
    Box<dyn FnOnce(Result<MacosScreenshotReferenceSet, MacosCaptureError>) + Send>;
pub(super) type ScreenshotImageCompletion =
    Box<dyn FnOnce(Result<MacosScreenshotReferenceImage, MacosCaptureError>) + Send>;

pub(super) trait ScreenshotCaptureBackend: Send + Sync {
    fn capture(
        &self,
        filter: ScreenshotFilterHandle,
        dynamic_range: MacosCaptureDynamicRange,
        cursor_composed: bool,
        completion: ScreenshotImageCompletion,
    ) -> Result<(), MacosCaptureError>;
}

pub(super) trait ScreenshotIdentityFence: Send + Sync {
    fn matches(&self, source_id: &str, generation: u64, selection_revision: u64) -> bool;
}

pub(super) struct NativeScreenshotCaptureBackend;

impl ScreenshotCaptureBackend for NativeScreenshotCaptureBackend {
    fn capture(
        &self,
        filter: ScreenshotFilterHandle,
        dynamic_range: MacosCaptureDynamicRange,
        cursor_composed: bool,
        completion: ScreenshotImageCompletion,
    ) -> Result<(), MacosCaptureError> {
        #[cfg(not(test))]
        let ScreenshotFilterHandle::Native(filter) = filter;
        #[cfg(test)]
        let filter = match filter {
            ScreenshotFilterHandle::Native(filter) => filter,
            ScreenshotFilterHandle::Fixture(_) => {
                return Err(MacosCaptureError::TahoePlatformDefect(
                    "native screenshot filter",
                ));
            }
        };
        let configuration_class = AnyClass::get(c"SCScreenshotConfiguration").ok_or(
            MacosCaptureError::TahoePlatformDefect("SCScreenshotConfiguration"),
        )?;
        let manager_class = AnyClass::get(c"SCScreenshotManager").ok_or(
            MacosCaptureError::TahoePlatformDefect("SCScreenshotManager"),
        )?;
        for (class, selector, capability) in [
            (
                configuration_class,
                sel!(setShowsCursor:),
                "SCScreenshotConfiguration.setShowsCursor",
            ),
            (
                configuration_class,
                sel!(setDisplayIntent:),
                "SCScreenshotConfiguration.setDisplayIntent",
            ),
            (
                configuration_class,
                sel!(setDynamicRange:),
                "SCScreenshotConfiguration.setDynamicRange",
            ),
        ] {
            if !class.responds_to(selector) {
                return Err(MacosCaptureError::TahoePlatformDefect(capability));
            }
        }
        if !manager_class.metaclass().responds_to(sel!(
            captureScreenshotWithFilter:configuration:completionHandler:
        )) {
            return Err(MacosCaptureError::TahoePlatformDefect(
                "SCScreenshotManager.captureScreenshot",
            ));
        }
        // SAFETY: the runtime probes above establish the Tahoe class and each
        // selector before the dynamically dispatched configuration calls.
        let configuration: Retained<AnyObject> = unsafe { msg_send![configuration_class, new] };
        let range_value = match dynamic_range {
            MacosCaptureDynamicRange::Sdr => 0_isize,
            MacosCaptureDynamicRange::Hdr => 1_isize,
        };
        // SAFETY: values match the SDK-declared BOOL and NSInteger properties.
        unsafe {
            let _: () = msg_send![&*configuration, setShowsCursor: cursor_composed];
            let _: () = msg_send![&*configuration, setDisplayIntent: 0_isize];
            let _: () = msg_send![&*configuration, setDynamicRange: range_value];
        }
        let completion = Arc::new(Mutex::new(Some(completion)));
        let completion_slot = Arc::clone(&completion);
        let retained_filter = filter.clone();
        let callback = RcBlock::new(move |output: *mut AnyObject, error: *mut NSError| {
            let Some(completion) = lock(&completion_slot).take() else {
                return;
            };
            // SAFETY: ScreenCaptureKit supplies callback objects for this
            // invocation. The selected CGImage is retained before return.
            let result = if let Some(error) = unsafe { error.as_ref() } {
                Err(native_error("capture Tahoe screenshot", error))
            } else if let Some(output) = unsafe { output.as_ref() } {
                // SAFETY: the live Objective-C output supports the NSObject
                // protocol query for its Tahoe image selector.
                unsafe {
                    let selector = match dynamic_range {
                        MacosCaptureDynamicRange::Sdr => sel!(sdrImage),
                        MacosCaptureDynamicRange::Hdr => sel!(hdrImage),
                    };
                    let responds: bool = msg_send![output, respondsToSelector: selector];
                    if !responds {
                        Err(MacosCaptureError::TahoePlatformDefect(
                            "SCScreenshotOutput image selector",
                        ))
                    } else {
                        let image: Option<Retained<CGImage>> = match dynamic_range {
                            MacosCaptureDynamicRange::Sdr => msg_send![output, sdrImage],
                            MacosCaptureDynamicRange::Hdr => msg_send![output, hdrImage],
                        };
                        image
                            .ok_or(MacosCaptureError::MissingScreenshotImage(dynamic_range))
                            .and_then(|image| {
                                MacosScreenshotReferenceImage::from_native(image, dynamic_range)
                            })
                    }
                }
            } else {
                Err(MacosCaptureError::TahoePlatformDefect("SCScreenshotOutput"))
            };
            drop(retained_filter.clone());
            completion(result);
        });
        // SAFETY: the runtime probe establishes this class selector. The API
        // copies the block and retains the filter and configuration while the
        // asynchronous capture is pending.
        unsafe {
            let _: () = msg_send![
                manager_class,
                captureScreenshotWithFilter: filter.system(),
                configuration: &*configuration,
                completionHandler: &*callback
            ];
        }
        Ok(())
    }
}

pub(super) fn execute_screenshot_transaction(
    snapshot: ScreenshotTransactionSnapshot,
    fence: Arc<dyn ScreenshotIdentityFence>,
    backend: Arc<dyn ScreenshotCaptureBackend>,
    cursor_composed: bool,
    completion: ScreenshotCompletion,
) -> Result<(), MacosCaptureError> {
    if matches!(
        snapshot.capability,
        MacosScreenshotReferenceCapability::PendingFirstFrame
    ) {
        return Err(MacosCaptureError::ScreenshotCapabilityPending);
    }
    let completion = Arc::new(Mutex::new(Some(completion)));
    let first_filter = snapshot.filter.clone();
    let second_filter = snapshot.filter.clone();
    let first_source_id = Arc::clone(&snapshot.source_id);
    let first_fence = Arc::clone(&fence);
    let second_backend = Arc::clone(&backend);
    let capability = snapshot.capability.clone();
    let generation = snapshot.generation;
    let selection_revision = snapshot.selection_revision;
    let first_completion = Arc::clone(&completion);
    backend.capture(
        first_filter,
        MacosCaptureDynamicRange::Sdr,
        cursor_composed,
        Box::new(move |sdr| {
            if !first_fence.matches(&first_source_id, generation, selection_revision) {
                finish_screenshot(
                    &first_completion,
                    Err(MacosCaptureError::ScreenshotSelectionChanged),
                );
                return;
            }
            let sdr = match sdr {
                Ok(sdr) => sdr,
                Err(error) => {
                    finish_screenshot(&first_completion, Err(error));
                    return;
                }
            };
            match capability {
                MacosScreenshotReferenceCapability::PendingFirstFrame => {
                    finish_screenshot(
                        &first_completion,
                        Err(MacosCaptureError::ScreenshotCapabilityPending),
                    );
                }
                MacosScreenshotReferenceCapability::SdrOnly { .. } => {
                    finish_screenshot(
                        &first_completion,
                        Ok(MacosScreenshotReferenceSet::Sdr { image: sdr }),
                    );
                }
                MacosScreenshotReferenceCapability::PairedSdrHdr { .. } => {
                    let second_source_id = Arc::clone(&first_source_id);
                    let second_fence = Arc::clone(&first_fence);
                    let second_completion = Arc::clone(&first_completion);
                    let start_completion = Arc::clone(&first_completion);
                    let start = second_backend.capture(
                        second_filter,
                        MacosCaptureDynamicRange::Hdr,
                        cursor_composed,
                        Box::new(move |hdr| {
                            if !second_fence.matches(
                                &second_source_id,
                                generation,
                                selection_revision,
                            ) {
                                finish_screenshot(
                                    &second_completion,
                                    Err(MacosCaptureError::ScreenshotSelectionChanged),
                                );
                                return;
                            }
                            match hdr {
                                Ok(hdr) => finish_screenshot(
                                    &second_completion,
                                    Ok(MacosScreenshotReferenceSet::Paired { sdr, hdr }),
                                ),
                                Err(error) => finish_screenshot(&second_completion, Err(error)),
                            }
                        }),
                    );
                    if let Err(error) = start {
                        finish_screenshot(&start_completion, Err(error));
                    }
                }
            }
        }),
    )
}

fn finish_screenshot(
    completion: &Arc<Mutex<Option<ScreenshotCompletion>>>,
    result: Result<MacosScreenshotReferenceSet, MacosCaptureError>,
) {
    if let Some(completion) = lock(completion).take() {
        completion(result);
    }
}
