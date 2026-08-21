use super::{
    Arc, CaptureColorSpace, CaptureColorimetry, CaptureCursor, CaptureCursorContent, CaptureDamage,
    CaptureDynamicRange, CaptureEpoch, CaptureFrame, CaptureFrameMetadata, CaptureLuminanceContext,
    CapturePixelFormat, CapturePositiveScalar, CaptureRotation, CaptureSourceId, CaptureStorage,
    CaptureTransferFunction, CpuCaptureStorage, CpuPublicationFanoutError, CpuReductionExecutor,
    CpuSamplingError, CpuScalarSource, DEFAULT_HDR_SOURCE_CONTENT_HEADROOM,
    DEFAULT_HDR_SOURCE_REFERENCE_WHITE_NITS, Duration, InputData, Instant, KnownCaptureColorimetry,
    LEGACY_ANALYSIS_MAX_HEIGHT, LEGACY_ANALYSIS_MAX_WIDTH, LedToneMapCalibration,
    MacosCaptureContentStyle, MacosCaptureControl, MacosCaptureDynamicRange, MacosCaptureFrame,
    MacosCapturePixelFormat, MacosCaptureSelection, MacosColorPrimaries, MacosCpuSourceView,
    MacosExactDelivery, MacosExactPublicationShared, MacosExactRuntime, MacosNativeTargetManifest,
    MacosOwnedSource, MacosPublication, MacosPublicationSource, MacosScreenRuntimeTelemetry,
    MacosTransferFunction, Mutex, NonZeroU32, NonZeroU64, NonZeroUsize, Ordering, PixelExtent,
    PixelRect, PlatformGpuApi, PlatformGpuSurface, PreparedLedToneMap, PreparedWorker,
    RawCaptureSurface, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ResolvedScreenPublicationDescriptor, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ResourceDescriptor, ResourceState, ScreenBackendResourceIdentity, ScreenBranchPayload,
    ScreenCaptureBackend, ScreenComputeCapacityPolicy, ScreenCursorCapabilities,
    ScreenExecutorColorCapabilities, ScreenGpuSurfacePayload, ScreenNativeWorkPayload,
    ScreenPhysicalGpuDeviceIdentity, ScreenPublicationColorimetry, ScreenPublicationError,
    ScreenPublicationExecutor, ScreenPublicationExecutorFallbackReason,
    ScreenPublicationExecutorRequest, ScreenPublicationHealth, ScreenPublicationHub,
    ScreenPublicationHubError, ScreenPublicationMetadata, ScreenResourceApi,
    ScreenSourceReflection, ScreenSourceSelector, ScreenWorkerBindingState, SourceScale,
    SourceSessionSlot, TopologyDescriptor, TopologyState, analyze_screen_frame, anyhow, lock,
    thread,
};

impl MacosNativeTargetManifest {
    pub(super) fn new(descriptor: &ResolvedScreenPublicationDescriptor) -> anyhow::Result<Self> {
        let resources = descriptor.physical().source().resources();
        let ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(metal_registry_id) = resources
            .physical_gpu_device()
            .ok_or_else(|| anyhow!("macOS native publication is missing Metal identity"))?
        else {
            return Err(anyhow!(
                "macOS native publication selected a non-Metal device"
            ));
        };
        if *metal_registry_id == 0
            || resources.device_generation() == 0
            || resources.resource_generation() == 0
        {
            return Err(anyhow!(
                "macOS native publication generations must be nonzero"
            ));
        }
        Ok(Self {
            capture_session_generation: resources.device_generation(),
            resource_generation: resources.resource_generation(),
            metal_registry_id: *metal_registry_id,
        })
    }

    /// Capture-session generation whose surfaces this target accepts.
    #[must_use]
    pub const fn capture_session_generation(&self) -> u64 {
        self.capture_session_generation
    }

    /// Storage-descriptor generation whose surfaces this target accepts.
    #[must_use]
    pub const fn resource_generation(&self) -> u64 {
        self.resource_generation
    }

    /// Physical Metal device registry identity.
    #[must_use]
    pub const fn metal_registry_id(&self) -> u64 {
        self.metal_registry_id
    }
}

impl MacosPublicationSource {
    pub(super) fn from_frame(
        source_id: CaptureSourceId,
        topology_generation: u64,
        resource_generation: u64,
        frame: &MacosCaptureFrame,
    ) -> anyhow::Result<Self> {
        let storage_extent =
            PixelExtent::new(frame.storage_extent.width, frame.storage_extent.height)?;
        let content = frame.geometry.content_rect_pixels;
        let content_x = u32::try_from(content.x)?;
        let content_y = u32::try_from(content.y)?;
        let content_rect = PixelRect::new(content_x, content_y, content.width, content.height)?;
        let crop = (content_x != 0
            || content_y != 0
            || content.width != storage_extent.width()
            || content.height != storage_extent.height())
        .then_some(content_rect);
        Ok(Self {
            epoch: CaptureEpoch {
                source_id,
                topology_generation,
                session_generation: frame.epoch,
            },
            geometry: super::super::CaptureGeometry::new(
                capture_origin(frame)?,
                storage_extent,
                storage_extent,
                CaptureRotation::Identity,
                crop,
                SourceScale::ONE,
            )?,
            logical_extent: content_rect.extent(),
            colorimetry: capture_colorimetry(frame)?,
            pixel_format: frame.pixel_format,
            resource_generation,
            allocation_bytes: frame.surface.allocation_bytes,
            display_scale_bits: frame.geometry.display_scale_factor.get().to_bits(),
            cursor_composed: frame.cursor_composed,
        })
    }

    pub(super) fn matches_selector(&self, selector: &ScreenSourceSelector) -> bool {
        match selector {
            ScreenSourceSelector::Configured | ScreenSourceSelector::Primary => true,
            ScreenSourceSelector::Exact(source_id) => source_id == &self.epoch.source_id,
        }
    }

    pub(super) fn cursor_capabilities(&self) -> ScreenCursorCapabilities {
        if self.cursor_composed {
            ScreenCursorCapabilities::composed_only()
        } else {
            ScreenCursorCapabilities::clean_only()
        }
    }

    pub(super) fn cpu_source(&self, selector: ScreenSourceSelector) -> ResolvedScreenSource {
        ResolvedScreenSource::new(
            selector,
            self.epoch.clone(),
            ResolvedScreenSourceConfig::new_with_cursor_capabilities(
                self.geometry,
                self.logical_extent,
                ScreenSourceReflection::None,
                capture_pixel_format(self.pixel_format),
                self.colorimetry,
                self.cursor_capabilities(),
                ScreenBackendResourceIdentity::new(
                    ScreenCaptureBackend::MacosScreenCaptureKit,
                    ScreenResourceApi::Cpu,
                    self.epoch.session_generation,
                    self.resource_generation,
                ),
            ),
        )
    }

    pub(super) fn gpu_source(
        &self,
        selector: ScreenSourceSelector,
        physical_gpu_device: ScreenPhysicalGpuDeviceIdentity,
    ) -> anyhow::Result<ResolvedScreenSource> {
        let ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(registry_id) = physical_gpu_device
        else {
            return Err(anyhow!("macOS capture requires a Metal execution target"));
        };
        if registry_id == 0 {
            return Err(anyhow!(
                "macOS capture received a zero Metal registry identity"
            ));
        }
        let pixel_format = capture_pixel_format(self.pixel_format);
        Ok(ResolvedScreenSource::new(
            selector,
            self.epoch.clone(),
            ResolvedScreenSourceConfig::new_with_cursor_capabilities(
                self.geometry,
                self.logical_extent,
                ScreenSourceReflection::None,
                pixel_format,
                self.colorimetry,
                self.cursor_capabilities(),
                ScreenBackendResourceIdentity::new_with_physical_gpu_device(
                    ScreenCaptureBackend::MacosScreenCaptureKit,
                    ScreenResourceApi::PlatformGpu(PlatformGpuApi::Metal),
                    ScreenPhysicalGpuDeviceIdentity::MetalRegistryId(registry_id),
                    self.epoch.session_generation,
                    self.resource_generation,
                ),
            ),
        ))
    }
}

impl MacosExactPublicationShared {
    #[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
    pub(super) fn with_compute_capacity_policy(policy: ScreenComputeCapacityPolicy) -> Self {
        Self {
            compute_capacity_policy: policy,
            ..Self::default()
        }
    }

    pub(super) fn advance_resolution_revision(&self) {
        self.resolution_revision
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |revision| {
                revision.checked_add(1)
            })
            .expect("macOS screen publication resolution revision exhausted");
    }

    pub(super) fn replace_source(&self, next: Option<MacosPublicationSource>) {
        let mut source = lock(&self.source);
        if *source == next {
            return;
        }
        tracing::debug!(
            shared = ?std::ptr::from_ref(self),
            installed = next.is_some(),
            "macOS exact publication source changed"
        );
        *source = next;
        self.advance_resolution_revision();
    }

    pub(super) fn source(&self) -> Option<MacosPublicationSource> {
        lock(&self.source).clone()
    }

    pub(super) fn hub(&self) -> Option<Arc<ScreenPublicationHub>> {
        lock(&self.hub).clone()
    }

    pub(super) fn owns_source(&self, source_id: &CaptureSourceId) -> bool {
        self.source()
            .is_some_and(|source| &source.epoch.source_id == source_id)
            || lock(&self.owned_sources)
                .iter()
                .any(|source| &source.source_id == source_id)
    }

    pub(super) fn register_owned_source(&self, source: MacosOwnedSource) {
        lock(&self.owned_sources).push(source);
    }

    pub(super) fn reap_owned_sources(&self) {
        let authority = self.hub().map(|hub| hub.committed_state());
        lock(&self.owned_sources).retain(|source| {
            authority
                .as_ref()
                .is_some_and(|authority| authority.owns_runtime_binding(&source.binding))
        });
    }

    pub(super) fn clear_owned_sources(&self) {
        lock(&self.owned_sources).clear();
    }

    pub(super) fn cpu_executor(&self) -> anyhow::Result<Arc<CpuReductionExecutor>> {
        let mut executor = lock(&self.cpu_executor);
        if let Some(executor) = executor.as_ref() {
            return Ok(Arc::clone(executor));
        }
        let prepared = Arc::new(CpuReductionExecutor::new(
            thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
            NonZeroU32::new(16).expect("CPU reduction tile height is nonzero"),
        )?);
        *executor = Some(Arc::clone(&prepared));
        Ok(prepared)
    }
}

impl MacosExactRuntime {
    pub(super) fn bind_if_current(&mut self, hub: &ScreenPublicationHub) -> anyhow::Result<()> {
        let authority = hub.committed_state();
        if !authority.owns_runtime_binding(&self.binding) {
            return Ok(());
        }
        match self.binding.state() {
            ScreenWorkerBindingState::Active | ScreenWorkerBindingState::Retired => {}
            ScreenWorkerBindingState::Prepared | ScreenWorkerBindingState::Armed => return Ok(()),
            ScreenWorkerBindingState::Aborted => {
                return Err(anyhow!("macOS exact runtime was aborted after commit"));
            }
        }
        for route in &mut self.native_routes {
            if route.publisher.is_none() {
                route.publisher =
                    Some(authority.publisher_for_runtime(&route.descriptor, &self.binding)?);
            }
        }
        if self.fanout.is_none()
            && let Some(candidate) = self.fanout_candidate.take()
        {
            self.fanout = Some(candidate.bind(&authority, &self.binding)?);
        }
        Ok(())
    }

    pub(super) fn is_bound(&self) -> bool {
        self.native_routes
            .iter()
            .all(|route| route.publisher.is_some())
            && self.fanout_candidate.is_none()
    }
}

pub(super) fn bind_current_macos_exact_runtime<'a>(
    runtimes: &'a mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    hub: &ScreenPublicationHub,
    captured_at: Instant,
) -> anyhow::Result<Option<&'a mut MacosExactRuntime>> {
    let authority = hub.committed_state();
    let Some(current_binding) = authority.runtime_binding(&source.epoch.source_id) else {
        return Ok(None);
    };
    let Some(current_index) = runtimes
        .iter_mut()
        .position(|runtime| runtime.source == *source && runtime.binding.is_same(current_binding))
    else {
        return Ok(None);
    };
    let should_inherit = runtimes[current_index].fanout.is_none()
        && runtimes[current_index].fanout_candidate.is_some();
    runtimes[current_index].bind_if_current(hub)?;
    if should_inherit
        && let Some(previous_index) =
            runtimes
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, runtime)| {
                    (index != current_index
                        && runtime.binding.source_id() == current_binding.source_id()
                        && runtime.fanout.is_some())
                    .then_some(index)
                })
    {
        let (current, previous) = if current_index < previous_index {
            let (before_previous, previous_and_after) = runtimes.split_at_mut(previous_index);
            (
                &mut before_previous[current_index],
                &mut previous_and_after[0],
            )
        } else {
            let (before_current, current_and_after) = runtimes.split_at_mut(current_index);
            (
                &mut current_and_after[0],
                &mut before_current[previous_index],
            )
        };
        if let (Some(current), Some(previous)) = (current.fanout.as_mut(), previous.fanout.as_mut())
        {
            current.inherit_tone_map_transition_from(previous, captured_at);
        }
    }
    Ok(runtimes[current_index]
        .is_bound()
        .then_some(&mut runtimes[current_index]))
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) fn resolve_macos_publication_branch(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    resolve_macos_publication_branch_with_telemetry(source, demand, &telemetry)
}

pub(super) fn resolve_macos_publication_branch_with_telemetry(
    source: &MacosPublicationSource,
    demand: &RegisteredScreenBranchDemand,
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
    let selector = demand.request().selector();
    if !source.matches_selector(selector) {
        return Ok(None);
    }
    let selector = selector.clone();
    let capabilities = CpuReductionExecutor::supported_color_capabilities();
    if matches!(
        demand.request().executor(),
        ScreenPublicationExecutorRequest::Cpu
    ) {
        telemetry.set_cpu();
        return Ok(Some(demand.resolve_with_color_capabilities(
            &source.cpu_source(selector),
            capabilities,
        )?));
    }

    let (target, native_required) = match demand.request().executor() {
        ScreenPublicationExecutorRequest::SourceNative(target) => (target, false),
        ScreenPublicationExecutorRequest::SourceNativeRequired(target) => (target, true),
        ScreenPublicationExecutorRequest::Cpu => {
            unreachable!("CPU publication requests returned above")
        }
    };
    if target.accepted_api() != &PlatformGpuApi::Metal {
        if native_required {
            let reason = ScreenPublicationExecutorFallbackReason::PlatformApiMismatch;
            telemetry.set_native_unavailable(reason, target.id());
            return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
        }
        telemetry.set_cpu_fallback("target_api_not_metal");
    } else if let Ok(native_source) =
        source.gpu_source(selector.clone(), target.physical_gpu_device().clone())
    {
        match demand.resolve_with_executor_capabilities(
            &native_source,
            ScreenExecutorColorCapabilities::new(capabilities, target.color_capabilities()),
        ) {
            Ok(resolved)
                if matches!(
                    resolved.descriptor().executor(),
                    ScreenPublicationExecutor::SourceNative(_)
                ) && MacosNativeTargetManifest::new(resolved.descriptor()).is_ok() =>
            {
                telemetry.set_native(native_required, target.id());
                return Ok(Some(resolved));
            }
            Err(ScreenPublicationError::RequiredNativeUnavailable(reason)) => {
                telemetry.set_native_unavailable(reason, target.id());
                return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
            }
            Ok(_) if native_required => {
                let reason =
                    ScreenPublicationExecutorFallbackReason::NativeColorContractUnsupported;
                telemetry.set_native_unavailable(reason, target.id());
                return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
            }
            Ok(_) => telemetry.set_cpu_fallback("native_contract_unavailable"),
            Err(error) if native_required => return Err(error.into()),
            Err(_) => telemetry.set_cpu_fallback("native_descriptor_incompatible"),
        }
    } else {
        if native_required {
            let reason = ScreenPublicationExecutorFallbackReason::PhysicalGpuDeviceMismatch;
            telemetry.set_native_unavailable(reason, target.id());
            return Err(ScreenPublicationError::RequiredNativeUnavailable(reason).into());
        }
        telemetry.set_cpu_fallback("metal_device_mismatch");
    }

    Ok(Some(demand.resolve_with_color_capabilities(
        &source.cpu_source(selector),
        capabilities,
    )?))
}

pub(super) fn macos_native_descriptor_is_identity(
    descriptor: &ResolvedScreenPublicationDescriptor,
) -> bool {
    descriptor.source_pixel_format() == CapturePixelFormat::Bgra8
        && descriptor.source().geometry().crop().is_none()
        && descriptor.geometry().output_extent() == descriptor.source().geometry().storage_extent()
        && descriptor.physical().reduction_extent()
            == descriptor.source().geometry().storage_extent()
        && descriptor.physical().target_pixel_format() == descriptor.source_pixel_format()
        && matches!(
            descriptor.physical().color_pipeline().transform(),
            super::super::ResolvedScreenColorTransform::PreserveEncodedSamples
        )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn publish_frame(
    prepared: &mut PreparedWorker,
    frame: Arc<MacosCaptureFrame>,
    source_id: CaptureSourceId,
    topology: &mut TopologyState,
    resources: &mut ResourceState,
    publication: &Mutex<MacosPublication>,
    exact: &MacosExactPublicationShared,
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
    exact_runtimes: &mut [MacosExactRuntime],
    worker_generation: u64,
    target_fps: u32,
    status_session: &SourceSessionSlot,
    control: &Arc<dyn MacosCaptureControl>,
) -> anyhow::Result<()> {
    // ScreenCaptureKit's display time is the frame's intended display
    // vsync, which runs slightly ahead of callback delivery, so the raw
    // conversion can land in the future. A future capture instant makes
    // every publication timeline read backwards (published_at precedes
    // captured_at) and kills the pump; a capture time can never postdate
    // the moment we hold the frame.
    let captured_at = control.captured_at(frame.display_time)?.min(Instant::now());
    let fresh_until = captured_at
        .checked_add(Duration::from_nanos(
            2_000_000_000_u64.div_ceil(u64::from(target_fps)),
        ))
        .ok_or_else(|| anyhow!("macOS capture freshness deadline overflow"))?;
    if Instant::now() > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    let topology_generation = topology.observe(&frame)?;
    let resource_generation = resources.observe(&frame)?;
    let source = MacosPublicationSource::from_frame(
        source_id.clone(),
        topology_generation,
        resource_generation,
        &frame,
    )?;
    exact.replace_source(Some(source.clone()));
    let exact_delivery = publish_macos_native_exact_with_telemetry(
        &frame,
        captured_at,
        fresh_until,
        &source,
        exact,
        exact_runtimes,
        telemetry,
    )?;
    if exact_delivery.stale {
        return Ok(());
    }
    if exact_delivery.cpu {
        let capture =
            native_cpu_capture_frame(&frame, captured_at, fresh_until, &source, source_id.clone())?;
        if Instant::now() > fresh_until {
            telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        publish_macos_scalar_exact(&frame, &capture, &source, exact, exact_runtimes, telemetry)?;
    }
    // The exact lane feeds native compositing only; HTML effects read the
    // analyzed ScreenData that flows through the legacy slot publication.
    // Treating the lanes as exclusive turns every HTML screen effect black
    // the moment an exact plan commits, so the legacy analysis always runs
    // alongside exact deliveries.
    let capture = legacy_cpu_capture_frame(
        prepared,
        &frame,
        captured_at,
        fresh_until,
        &source,
        source_id,
        topology_generation,
    )?;
    if Instant::now() > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        publish_macos_cpu_exact(&capture, &source, exact, exact_runtimes, telemetry)?;
    }
    let reduction_started = Instant::now();
    let snapshot = analyze_screen_frame(&mut prepared.analyzer, capture);
    telemetry.record_cpu_reduction(reduction_started.elapsed());
    let snapshot = snapshot?;
    if Instant::now() > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    if snapshot.geometry_frame().metadata().topology_generation != topology_generation {
        return Err(anyhow!("macOS analysis changed topology generation"));
    }
    let data = Arc::new(InputData::Screen(snapshot.data().clone()));
    if lock(publication).worker_generation != worker_generation {
        return Ok(());
    }
    if let Some(status) = status_session.load() {
        status.record_sample(captured_at, fresh_until, 1)?;
    }
    {
        let mut publication = lock(publication);
        if publication.worker_generation != worker_generation {
            return Ok(());
        }
        publication.latest = Some(data);
    }
    telemetry.record_converted_publication(captured_at);
    Ok(())
}

pub(super) fn legacy_analysis_decimation(extent: PixelExtent) -> u32 {
    extent
        .width()
        .div_ceil(LEGACY_ANALYSIS_MAX_WIDTH)
        .max(extent.height().div_ceil(LEGACY_ANALYSIS_MAX_HEIGHT))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn legacy_cpu_capture_frame(
    prepared: &mut PreparedWorker,
    frame: &MacosCaptureFrame,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    source_id: CaptureSourceId,
    topology_generation: u64,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let extent = source.geometry.storage_extent();
    let decimation = if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        // The Bgra8 plane also feeds the CPU-exact publication, which is
        // exact by contract, so it keeps every native pixel.
        1
    } else {
        legacy_analysis_decimation(extent)
    };
    let storage_extent = if decimation == 1 {
        extent
    } else {
        PixelExtent::new(
            extent.width().div_ceil(decimation),
            extent.height().div_ceil(decimation),
        )?
    };
    let row_stride = usize::try_from(storage_extent.width())
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| anyhow!("macOS capture row stride overflow"))?;
    let byte_len = row_stride
        .checked_mul(usize::try_from(storage_extent.height())?)
        .ok_or_else(|| anyhow!("macOS capture plane length overflow"))?;
    let mut plane = prepared.plane_pool.try_acquire(byte_len)?;
    plane.resize(byte_len, 0);
    let (pixel_format, colorimetry) = if frame.pixel_format == MacosCapturePixelFormat::Bgra8 {
        frame.copy_bgra8_to(&mut plane, row_stride)?;
        (CapturePixelFormat::Bgra8, source.colorimetry)
    } else {
        let calibration = LedToneMapCalibration::try_new(
            prepared.analyzer.config().target_led_white_x,
            prepared.analyzer.config().target_led_white_y,
            prepared.analyzer.config().target_led_reference_white_nits,
            prepared.analyzer.config().target_led_peak_nits,
            prepared.analyzer.config().exposure_ev,
        )?;
        let tone_map = PreparedLedToneMap::prepare(
            source.colorimetry.try_known()?,
            KnownCaptureColorimetry::SRGB,
            calibration,
        )?;
        frame.with_cpu_source(|samples| -> anyhow::Result<()> {
            for y in 0..storage_extent.height() {
                let source_y = y
                    .checked_mul(decimation)
                    .ok_or_else(|| anyhow!("macOS legacy sample row overflow"))?;
                let row_start = usize::try_from(y)?
                    .checked_mul(row_stride)
                    .ok_or_else(|| anyhow!("macOS legacy row offset overflow"))?;
                for x in 0..storage_extent.width() {
                    let source_x = x
                        .checked_mul(decimation)
                        .ok_or_else(|| anyhow!("macOS legacy sample column overflow"))?;
                    let pixel_start = usize::try_from(x)?
                        .checked_mul(4)
                        .and_then(|offset| row_start.checked_add(offset))
                        .ok_or_else(|| anyhow!("macOS legacy pixel offset overflow"))?;
                    let pixel_end = pixel_start
                        .checked_add(4)
                        .ok_or_else(|| anyhow!("macOS legacy pixel end overflow"))?;
                    let source_pixel = samples.sample_rgba32f(source_x, source_y)?;
                    plane[pixel_start..pixel_end].copy_from_slice(
                        &tone_map.encode(tone_map.decode_and_map_source(source_pixel)),
                    );
                }
            }
            Ok(())
        })??;
        (CapturePixelFormat::Rgba8, CaptureColorimetry::SRGB)
    };
    let geometry = if decimation == 1 {
        source.geometry
    } else {
        super::super::CaptureGeometry::new(
            source.geometry.origin(),
            source.geometry.native_extent(),
            storage_extent,
            source.geometry.rotation(),
            source.geometry.crop(),
            source.geometry.source_scale(),
        )?
    };
    let damage = if decimation == 1 {
        CaptureDamage::new(
            frame
                .damage
                .iter()
                .map(|rect| {
                    Ok(PixelRect::new(
                        u32::try_from(rect.x)?,
                        u32::try_from(rect.y)?,
                        rect.width,
                        rect.height,
                    )?)
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            Vec::new(),
        )
    } else {
        // Native damage rects no longer address the decimated pixels, and the
        // analyzer resamples the whole surface anyway.
        CaptureDamage::new(
            vec![PixelRect::new(
                0,
                0,
                storage_extent.width(),
                storage_extent.height(),
            )?],
            Vec::new(),
        )
    };
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    Ok(CaptureFrame::<RawCaptureSurface>::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry,
            colorimetry,
            cursor: CaptureCursor {
                visible: frame.cursor_composed,
                position: None,
                hotspot: None,
                shape_extent: None,
                shape_generation: None,
                content: if frame.cursor_composed {
                    CaptureCursorContent::Composed
                } else {
                    CaptureCursorContent::Hidden
                },
            },
        },
        CaptureStorage::Cpu(CpuCaptureStorage::from_owner(
            plane.freeze(),
            pixel_format,
            i64::try_from(row_stride)?,
            0,
        )),
        damage,
    )?)
}

pub(super) fn native_cpu_capture_frame(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    source_id: CaptureSourceId,
) -> anyhow::Result<CaptureFrame<RawCaptureSurface>> {
    let sequence = frame
        .sequence
        .checked_add(1)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let surface = PlatformGpuSurface::new(
        PlatformGpuApi::Metal,
        u64::from(frame.surface.iosurface_id),
        source.geometry.storage_extent(),
        capture_pixel_format(frame.pixel_format),
        Arc::clone(frame),
    )?;
    Ok(CaptureFrame::new(
        CaptureFrameMetadata {
            source_id,
            topology_generation: source.epoch.topology_generation,
            session_generation: frame.epoch,
            sequence,
            captured_at,
            fresh_until,
            geometry: source.geometry,
            colorimetry: source.colorimetry,
            cursor: CaptureCursor {
                visible: frame.cursor_composed,
                position: None,
                hotspot: None,
                shape_extent: None,
                shape_generation: None,
                content: if frame.cursor_composed {
                    CaptureCursorContent::Composed
                } else {
                    CaptureCursorContent::Hidden
                },
            },
        },
        CaptureStorage::Gpu(surface),
        CaptureDamage::new(
            frame
                .damage
                .iter()
                .map(|rect| {
                    Ok(PixelRect::new(
                        u32::try_from(rect.x)?,
                        u32::try_from(rect.y)?,
                        rect.width,
                        rect.height,
                    )?)
                })
                .collect::<anyhow::Result<Vec<_>>>()?,
            Vec::new(),
        ),
    )?)
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(super) fn publish_macos_native_exact(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
) -> anyhow::Result<(MacosExactDelivery, Arc<MacosScreenRuntimeTelemetry>)> {
    let telemetry = Arc::new(MacosScreenRuntimeTelemetry::default());
    let delivery = publish_macos_native_exact_with_telemetry(
        frame,
        captured_at,
        fresh_until,
        source,
        exact,
        runtimes,
        &telemetry,
    )?;
    Ok((delivery, telemetry))
}

pub(super) fn publish_macos_native_exact_with_telemetry(
    frame: &Arc<MacosCaptureFrame>,
    captured_at: Instant,
    fresh_until: Instant,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &Arc<MacosScreenRuntimeTelemetry>,
) -> anyhow::Result<MacosExactDelivery> {
    let Some(hub) = exact.hub() else {
        return Ok(MacosExactDelivery::default());
    };
    let Some(runtime) = bind_current_macos_exact_runtime(runtimes, source, &hub, captured_at)?
    else {
        return Ok(MacosExactDelivery::default());
    };
    let delivery = MacosExactDelivery {
        native: !runtime.native_routes.is_empty(),
        cpu: runtime.fanout.is_some(),
        stale: false,
    };
    let published_at = Instant::now();
    if published_at > fresh_until {
        telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
        return Ok(MacosExactDelivery {
            stale: true,
            ..delivery
        });
    }
    let native_sequence = frame
        .sequence
        .checked_add(1)
        .and_then(NonZeroU64::new)
        .ok_or_else(|| anyhow!("macOS capture sequence exhausted"))?;
    let mut native_published = false;
    for route in &mut runtime.native_routes {
        if published_at < route.next_publish_at
            || route
                .last_accepted_sequence
                .is_some_and(|accepted| frame.sequence <= accepted)
        {
            continue;
        }
        let publisher = route
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow!("macOS native route has no committed publisher"))?;
        let surface = PlatformGpuSurface::new(
            PlatformGpuApi::Metal,
            u64::from(frame.surface.iosurface_id),
            source.geometry.storage_extent(),
            route.descriptor.source_pixel_format(),
            Arc::clone(frame),
        )?
        .with_timing_sink(Arc::clone(telemetry));
        let surface = route
            .target
            .retain_on_surface_with_capture_allocation(surface, route.capture_lifetime.clone())?;
        let metadata = ScreenPublicationMetadata::try_new(
            source.epoch.clone(),
            publisher.plan_generation(),
            native_sequence,
            captured_at,
            published_at,
            fresh_until,
            ScreenPublicationHealth::Healthy,
        )?;
        let payload = if macos_native_descriptor_is_identity(&route.descriptor) {
            ScreenBranchPayload::GpuSurface(ScreenGpuSurfacePayload::new(
                ScreenPublicationColorimetry::new(
                    route.descriptor.physical().color_pipeline().output(),
                ),
                &surface,
            ))
        } else {
            ScreenBranchPayload::NativeWork(ScreenNativeWorkPayload::new(
                ScreenPublicationColorimetry::new(route.descriptor.source_colorimetry()),
                &surface,
            ))
        };
        match hub.publish(publisher, payload, &metadata) {
            Ok(_) => {
                native_published = true;
                telemetry
                    .publication_plan_generation
                    .store(publisher.plan_generation().get(), Ordering::Release);
                route.last_accepted_sequence = Some(frame.sequence);
                route.next_publish_at = route
                    .pacer
                    .advance_deadline(route.next_publish_at, published_at)?;
            }
            Err(ScreenPublicationHubError::PublicationPressure { .. }) => {}
            Err(error) => return Err(error.into()),
        }
    }
    if native_published {
        telemetry.record_native_publication(captured_at);
    }
    Ok(delivery)
}

pub(super) fn publish_macos_cpu_exact(
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)?
    else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        telemetry
            .publication_plan_generation
            .store(fanout.plan_generation().get(), Ordering::Release);
        let report = fanout.publish_due(
            &hub,
            Some(frame),
            Instant::now(),
            ScreenPublicationHealth::Healthy,
        )?;
        if report.published() > 0 {
            telemetry.record_converted_publication(frame.metadata().captured_at);
        }
    }
    Ok(())
}

pub(super) fn publish_macos_scalar_exact(
    native_frame: &MacosCaptureFrame,
    frame: &CaptureFrame<RawCaptureSurface>,
    source: &MacosPublicationSource,
    exact: &MacosExactPublicationShared,
    runtimes: &mut [MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) -> anyhow::Result<()> {
    let Some(hub) = exact.hub() else {
        return Ok(());
    };
    let Some(runtime) =
        bind_current_macos_exact_runtime(runtimes, source, &hub, frame.metadata().captured_at)?
    else {
        return Ok(());
    };
    if let Some(fanout) = runtime.fanout.as_mut() {
        let reduction_started = Instant::now();
        telemetry
            .publication_plan_generation
            .store(fanout.plan_generation().get(), Ordering::Release);
        let report = fanout.publish_due_scalar(
            &hub,
            frame,
            Instant::now(),
            ScreenPublicationHealth::Healthy,
            |execute| {
                native_frame
                    .with_cpu_source(|samples| execute(&samples))
                    .map_err(|error| {
                        CpuPublicationFanoutError::ScalarSourceAccessFailed(error.to_string())
                    })?
            },
        )?;
        if report.published() > 0 {
            telemetry.record_cpu_reduction(reduction_started.elapsed());
            telemetry.record_converted_publication(frame.metadata().captured_at);
        }
    }
    Ok(())
}

impl TopologyState {
    pub(super) fn observe(&mut self, frame: &MacosCaptureFrame) -> anyhow::Result<u64> {
        let descriptor = TopologyDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS topology generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

impl ResourceState {
    pub(super) fn observe(&mut self, frame: &MacosCaptureFrame) -> anyhow::Result<u64> {
        let descriptor = ResourceDescriptor::from_frame(frame);
        if self.descriptor.as_ref() != Some(&descriptor) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| anyhow!("macOS resource generation exhausted"))?;
            self.descriptor = Some(descriptor);
        }
        Ok(self.generation)
    }
}

impl ResourceDescriptor {
    pub(super) fn from_frame(frame: &MacosCaptureFrame) -> Self {
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            pixel_format: frame.pixel_format,
            planes: frame
                .planes
                .iter()
                .map(|plane| {
                    (
                        plane.index,
                        plane.extent.width,
                        plane.extent.height,
                        plane.bytes_per_row,
                        plane.length_bytes,
                    )
                })
                .collect(),
        }
    }
}

impl TopologyDescriptor {
    pub(super) fn from_frame(frame: &MacosCaptureFrame) -> Self {
        let content = frame.geometry.content_rect_pixels;
        Self {
            width: frame.storage_extent.width,
            height: frame.storage_extent.height,
            content: (content.x, content.y, content.width, content.height),
            scale_bits: frame.geometry.display_scale_factor.get().to_bits(),
            screen: frame.geometry.screen_rect_points.map(|rect| {
                (
                    rect.x.to_bits(),
                    rect.y.to_bits(),
                    rect.width.to_bits(),
                    rect.height.to_bits(),
                )
            }),
        }
    }
}

pub(super) fn capture_source_id(
    selection: MacosCaptureSelection,
) -> anyhow::Result<CaptureSourceId> {
    let source: Arc<str> = match selection {
        MacosCaptureSelection::Display { source_id } => source_id,
        MacosCaptureSelection::SessionScoped { content_style } => Arc::from(match content_style {
            MacosCaptureContentStyle::Window => "macos:session:window",
            MacosCaptureContentStyle::MultipleWindows => "macos:session:multiple-windows",
            MacosCaptureContentStyle::Application => "macos:session:application",
            MacosCaptureContentStyle::MultipleApplications => "macos:session:multiple-applications",
            MacosCaptureContentStyle::Mixed => "macos:session:mixed",
        }),
        MacosCaptureSelection::None => Arc::from("macos:session"),
    };
    Ok(CaptureSourceId::new(source)?)
}

pub(super) fn capture_colorimetry(frame: &MacosCaptureFrame) -> anyhow::Result<CaptureColorimetry> {
    let color = frame.color;
    let color_space = match color.primaries {
        MacosColorPrimaries::Srgb => CaptureColorSpace::Srgb,
        MacosColorPrimaries::DisplayP3 => CaptureColorSpace::DisplayP3,
        MacosColorPrimaries::Rec2020 => CaptureColorSpace::Rec2020,
    };
    let transfer_function = match color.transfer {
        MacosTransferFunction::Srgb => CaptureTransferFunction::Srgb,
        MacosTransferFunction::Rec709 => CaptureTransferFunction::Rec709,
        MacosTransferFunction::Rec2020 => CaptureTransferFunction::Rec2020,
        MacosTransferFunction::Linear => CaptureTransferFunction::Linear,
        MacosTransferFunction::Pq => CaptureTransferFunction::Pq,
        MacosTransferFunction::Hlg => CaptureTransferFunction::Hlg,
    };
    let delivered = frame.delivered_metadata();
    let dynamic_range = if matches!(
        color.transfer,
        MacosTransferFunction::Pq | MacosTransferFunction::Hlg
    ) || delivered
        .is_some_and(|metadata| metadata.dynamic_range == MacosCaptureDynamicRange::Hdr)
        || matches!(
            frame.pixel_format,
            MacosCapturePixelFormat::Argb2101010 | MacosCapturePixelFormat::Rgba16Float
        ) {
        CaptureDynamicRange::High
    } else {
        CaptureDynamicRange::Standard
    };
    let luminance = if dynamic_range == CaptureDynamicRange::High {
        let delivered = delivered
            .ok_or_else(|| anyhow!("macOS HDR capture is missing delivered luminance metadata"))?;
        if delivered.pixel_format != frame.pixel_format
            || delivered.color != frame.color
            || delivered.dynamic_range != MacosCaptureDynamicRange::Hdr
        {
            return Err(anyhow!(
                "macOS HDR delivered metadata contradicts the capture frame"
            ));
        }
        // ScreenCaptureKit only sometimes attaches ContentLightLevelInfo and
        // IOSurfaceContentHeadroom; spec 76 requires a source reference white
        // regardless and treats headroom as best-effort. When the OS stays
        // silent, anchor reference white at BT.2408 diffuse white (203 nits,
        // the same default as the target LED calibration, so unsignalled
        // content maps through at unity) and assume one stop of highlight
        // headroom, matching the output headroom the tone map reserves.
        let reference_white = delivered
            .source_reference_white_nits
            .unwrap_or(DEFAULT_HDR_SOURCE_REFERENCE_WHITE_NITS);
        // EDR headroom is dynamic: macOS reports it relative to the current
        // display brightness, and at high brightness it legitimately reaches
        // 1.0 (no room above SDR white right now). No real pixel can exceed
        // reference white in that state, so giving the tone map its default
        // rolloff space is visually identity while keeping the luminance
        // contract well-formed.
        let headroom = delivered
            .content_headroom
            .filter(|headroom| *headroom > 1.0)
            .unwrap_or(DEFAULT_HDR_SOURCE_CONTENT_HEADROOM);
        let reference_white = CapturePositiveScalar::try_new(reference_white)?;
        let peak = CapturePositiveScalar::try_new(reference_white.value() * headroom)?;
        Some(CaptureLuminanceContext::new(reference_white, peak)?)
    } else {
        None
    };
    Ok(CaptureColorimetry::new(
        color_space,
        transfer_function,
        Some(dynamic_range),
        luminance,
    )?)
}

pub(super) const fn capture_pixel_format(format: MacosCapturePixelFormat) -> CapturePixelFormat {
    match format {
        MacosCapturePixelFormat::Bgra8 => CapturePixelFormat::Bgra8,
        MacosCapturePixelFormat::Argb2101010 => CapturePixelFormat::Argb2101010,
        MacosCapturePixelFormat::Rgba16Float => CapturePixelFormat::Rgba16Float,
        MacosCapturePixelFormat::Yuv420VideoRange => CapturePixelFormat::Yuv420VideoRange,
        MacosCapturePixelFormat::Yuv420FullRange => CapturePixelFormat::Yuv420FullRange,
        MacosCapturePixelFormat::Yuv44410BiPlanar => CapturePixelFormat::Yuv44410BiPlanar,
    }
}

impl CpuScalarSource for MacosCpuSourceView<'_> {
    fn storage_extent(&self) -> PixelExtent {
        let extent = (*self).extent();
        PixelExtent::new(extent.width, extent.height)
            .expect("validated macOS CPU source has a non-empty extent")
    }

    fn pixel_format(&self) -> CapturePixelFormat {
        capture_pixel_format((*self).pixel_format())
    }

    fn sample_rgba32f(&self, x: u32, y: u32) -> Result<[f32; 4], CpuSamplingError> {
        (*self)
            .sample_rgba32f(x, y)
            .map_err(|_| CpuSamplingError::ScalarSourceReadFailed { x, y })
    }
}

pub(super) fn capture_origin(
    frame: &MacosCaptureFrame,
) -> anyhow::Result<super::super::PhysicalOrigin> {
    let rect = frame
        .geometry
        .screen_rect_points
        .unwrap_or(frame.geometry.content_rect_points);
    let scale = frame.geometry.display_scale_factor.get();
    Ok(super::super::PhysicalOrigin {
        x: scaled_coordinate(rect.x, scale)?,
        y: scaled_coordinate(rect.y, scale)?,
    })
}

pub(super) fn scaled_coordinate(value: f64, scale: f64) -> anyhow::Result<i32> {
    let value = (value * scale).floor();
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(anyhow!("macOS capture origin exceeds i32"));
    }
    Ok(value as i32)
}
