use super::super::{
    Arc, CaptureActivityHandle, CaptureSessionAuthority, CaptureSourceId, Duration, Instant,
    MacosCaptureBackend, MacosCaptureControl, MacosCaptureFrame, MacosExactDelivery,
    MacosExactPublicationShared, MacosExactRuntime, MacosPublicationSource,
    MacosScreenRuntimeTelemetry, NonZeroU64, Ordering, PlatformGpuApi, PlatformGpuSurface,
    PreparedWorker, ResourceState, ScreenBranchPayload, ScreenGpuSurfacePayload,
    ScreenNativeWorkPayload, ScreenPublicationColorimetry, ScreenPublicationHealth,
    ScreenPublicationHubError, ScreenPublicationMetadata, SourceSessionSlot, TopologyState, anyhow,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::super::{InputData, MacosCapturePixelFormat, analyze_screen_frame};
#[cfg(feature = "macos-capture-fixtures")]
use super::fixtures::{
    legacy_cpu_capture_frame, native_cpu_capture_frame, publish_macos_cpu_exact,
    publish_macos_scalar_exact,
};
use super::model::bind_current_macos_exact_runtime;
use super::resolution::macos_native_descriptor_is_identity;

#[allow(clippy::too_many_arguments)]
pub(in crate::input::screen::macos) fn publish_frame(
    prepared: &mut PreparedWorker,
    frame: Arc<MacosCaptureFrame>,
    source_id: CaptureSourceId,
    topology: &mut TopologyState,
    resources: &mut ResourceState,
    activity: &CaptureActivityHandle<MacosCaptureBackend>,
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
    exact.replace_current_source(
        CaptureSessionAuthority::new(worker_generation),
        Some(source.clone()),
    );
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
    #[cfg(feature = "macos-capture-fixtures")]
    if exact_delivery.cpu {
        let capture =
            native_cpu_capture_frame(&frame, captured_at, fresh_until, &source, source_id.clone())?;
        if Instant::now() > fresh_until {
            telemetry.stale_frames.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        publish_macos_scalar_exact(&frame, &capture, &source, exact, exact_runtimes, telemetry)?;
    }
    #[cfg(feature = "macos-capture-fixtures")]
    {
        // The fixture-only CPU reference analysis runs alongside exact
        // deliveries so parity tests can compare the native lane against
        // the reference ScreenData for the same frame.
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
        if !activity.is_current_epoch(&worker_generation) {
            return Ok(());
        }
        if let Some(status) = status_session.load() {
            status.record_sample(captured_at, fresh_until, 1)?;
        }
        exact.publish_fixture_reference(worker_generation, data);
        telemetry.record_converted_publication(captured_at);
    }
    #[cfg(not(feature = "macos-capture-fixtures"))]
    {
        let _ = (prepared, activity, worker_generation);
        if exact_delivery.native
            && let Some(status) = status_session.load()
        {
            status.record_sample(captured_at, fresh_until, 1)?;
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "macos-capture-fixtures"))]
pub(in crate::input::screen::macos) fn publish_macos_native_exact(
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

pub(in crate::input::screen::macos) fn publish_macos_native_exact_with_telemetry(
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
        #[cfg(feature = "macos-capture-fixtures")]
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
