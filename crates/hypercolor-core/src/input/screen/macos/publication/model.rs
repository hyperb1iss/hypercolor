#[cfg(feature = "macos-capture-fixtures")]
use super::super::{Arc, CpuReductionExecutor, NonZeroU32, NonZeroUsize, lock, thread};
use super::super::{
    CaptureEpoch, CaptureRotation, CaptureSessionAuthority, CaptureSourceId, Instant,
    MacosCaptureFrame, MacosExactPublicationShared, MacosExactRuntime, MacosPublicationSource,
    PixelExtent, PixelRect, PlatformGpuApi, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenCommittedState,
    ScreenComputeCapacityPolicy, ScreenCursorCapabilities, ScreenPhysicalGpuDeviceIdentity,
    ScreenPublicationHub, ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector,
    SourceScale, anyhow,
};
use super::metadata::{capture_colorimetry, capture_origin, capture_pixel_format};
use crate::input::screen::adapter::bind_current_capture_exact_runtime;

impl MacosPublicationSource {
    pub(in crate::input::screen::macos) fn from_frame(
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
            geometry: super::super::super::CaptureGeometry::new(
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

    pub(in crate::input::screen::macos) fn matches_selector(
        &self,
        selector: &ScreenSourceSelector,
    ) -> bool {
        match selector {
            ScreenSourceSelector::Configured | ScreenSourceSelector::Primary => true,
            ScreenSourceSelector::Exact(source_id) => source_id == &self.epoch.source_id,
        }
    }

    pub(in crate::input::screen::macos) fn cursor_capabilities(&self) -> ScreenCursorCapabilities {
        if self.cursor_composed {
            ScreenCursorCapabilities::composed_only()
        } else {
            ScreenCursorCapabilities::clean_only()
        }
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(in crate::input::screen::macos) fn cpu_source(
        &self,
        selector: ScreenSourceSelector,
    ) -> ResolvedScreenSource {
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

    pub(in crate::input::screen::macos) fn gpu_source(
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
    pub(in crate::input::screen::macos) fn with_compute_capacity_policy(
        policy: ScreenComputeCapacityPolicy,
    ) -> Self {
        #[cfg(not(feature = "macos-capture-fixtures"))]
        let _ = policy;
        Self {
            #[cfg(feature = "macos-capture-fixtures")]
            compute_capacity_policy: policy,
            ..Self::default()
        }
    }

    pub(in crate::input::screen::macos) fn replace_current_source(
        &self,
        authority: CaptureSessionAuthority,
        next: Option<MacosPublicationSource>,
    ) {
        let installed = next.is_some();
        if self.common.replace_source_if_current(authority, next) {
            tracing::debug!(
                shared = ?std::ptr::from_ref(self),
                installed,
                "macOS exact publication source changed"
            );
        }
    }

    #[cfg(test)]
    pub(in crate::input::screen::macos) fn install_test_source(
        &self,
        next: Option<MacosPublicationSource>,
    ) {
        let authority = if let Some(authority) = self.current_authority() {
            authority
        } else {
            let reservation = self.reserve_authority().expect("test authority reserves");
            let authority = reservation.authority();
            drop(
                self.activate_reserved_authority(reservation)
                    .expect("test authority activates"),
            );
            authority
        };
        self.replace_current_source(authority, next);
    }

    #[cfg(test)]
    pub(in crate::input::screen::macos) fn register_test_owned_source(
        &self,
        source: Box<crate::input::screen::ExactBoxNode<super::super::MacosOwnedSource>>,
    ) -> bool {
        let authority = if let Some(authority) = self.current_authority() {
            authority
        } else {
            let reservation = self.reserve_authority().expect("test authority reserves");
            let authority = reservation.authority();
            drop(
                self.activate_reserved_authority(reservation)
                    .expect("test authority activates"),
            );
            authority
        };
        self.register_owned_source_if_current(authority, source)
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(in crate::input::screen::macos) fn cpu_executor(
        &self,
    ) -> anyhow::Result<Arc<CpuReductionExecutor>> {
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
    pub(in crate::input::screen::macos) fn bind_routes(
        &mut self,
        authority: &ScreenCommittedState,
    ) -> anyhow::Result<bool> {
        let was_bound = self.is_bound();
        for route in &mut self.native_routes {
            if route.publisher.is_none() {
                route.publisher =
                    Some(authority.publisher_for_runtime(&route.descriptor, &self.binding)?);
            }
        }
        #[cfg(feature = "macos-capture-fixtures")]
        if self.fanout.is_none()
            && let Some(candidate) = self.fanout_candidate.take()
        {
            self.fanout = Some(candidate.bind(authority, &self.binding)?);
        }
        Ok(!was_bound && self.is_bound())
    }

    pub(in crate::input::screen::macos) fn is_bound(&self) -> bool {
        let native_bound = self
            .native_routes
            .iter()
            .all(|route| route.publisher.is_some());
        #[cfg(feature = "macos-capture-fixtures")]
        {
            native_bound && self.fanout_candidate.is_none()
        }
        #[cfg(not(feature = "macos-capture-fixtures"))]
        {
            native_bound
        }
    }
}

pub(in crate::input::screen::macos) fn bind_current_macos_exact_runtime<'a>(
    runtimes: &'a mut [MacosExactRuntime],
    source: &MacosPublicationSource,
    hub: &ScreenPublicationHub,
    captured_at: Instant,
) -> anyhow::Result<Option<&'a mut MacosExactRuntime>> {
    #[cfg(not(feature = "macos-capture-fixtures"))]
    let _ = captured_at;
    bind_current_capture_exact_runtime(runtimes, source, hub, |runtimes, current_binding| {
        #[cfg(not(feature = "macos-capture-fixtures"))]
        let _ = (runtimes, current_binding);
        #[cfg(feature = "macos-capture-fixtures")]
        if let Some(current_index) = runtimes.iter().position(|runtime| {
            runtime.source == *source && runtime.binding.is_same(current_binding)
        }) && let Some(previous_index) =
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
            if let (Some(current), Some(previous)) =
                (current.fanout.as_mut(), previous.fanout.as_mut())
            {
                current.inherit_tone_map_transition_from(previous, captured_at);
            }
        }
        Ok(())
    })
}
