use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use hypercolor_core::bus::{CanvasFrame, DisplayZoneFrame};
use hypercolor_core::device::BackendManager;
use hypercolor_core::input::screen::PixelExtent;
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::event::FrameTiming;
use hypercolor_types::event::{FrameData, HypercolorEvent, Severity};
use hypercolor_types::session::OffOutputBehavior;

use super::frame_composer::{ComposeRequest, compose_frame};
use super::frame_io::{FramePublicationRequest, FramePublicationSurfaces, publish_frame_updates};
use super::frame_metrics::{ActiveFrameMetricsInput, summarize_active_frame};
use super::frame_policy::{FrameExecution, SkipDecision};
use super::frame_reporting::{FrameCompletionReport, report_active_frame_completion};
use super::frame_sampling::{LedSamplingOutcome, resolve_led_sampling};
use super::frame_throttle::{maybe_idle_throttle, maybe_sleep_throttle};
use super::pipeline_runtime::PreparedCanvasResize;
use super::pipeline_runtime::{
    FrameLoopState, OutputFrameSource, OutputReuseKey, PendingSamplingWork, PipelineRuntime,
    PreparedLayoutActivation, RenderCaches, SceneSnapshotState,
};
use super::scene_snapshot::{
    FrameSceneSnapshot, apply_zone_layout_previews, build_frame_scene_snapshot,
    display_descriptors_for_zones, refresh_effect_scene_snapshot, scene_dependency_key,
    snapshot_display_zone_target_metadata,
};
use super::sparkleflinger::ComposedFrameSet;
use super::unassigned_output::{UnassignedOutputPlanner, unassigned_behavior_generation};
use super::{RenderThreadState, micros_between, micros_u32, u64_to_u32};
use crate::discovery::handle_async_write_failures;
use crate::performance::OutputFrameSourceKind;
use crate::scene_transactions::{
    LayoutActivationDecision, LayoutTransactionRejection, SceneTransaction,
};

#[expect(
    clippy::too_many_lines,
    reason = "frame execution intentionally keeps the full pipeline in one ordered async function"
)]
/// Retire queued scene transactions and commit any prepared layout.
///
/// The queue is the synchronisation boundary between the API and the
/// renderer, so it has to be serviced on every tick, not only on ticks that
/// draw. A paused render loop still accepts layout edits, and a caller
/// blocked on one of these transactions holds the layout update lock while
/// it waits.
pub(crate) async fn service_scene_transactions(
    state: &RenderThreadState,
    scene: &mut SceneSnapshotState,
    frame_loop: &mut FrameLoopState,
    render: &mut RenderCaches,
) {
    if let Some(prepared) = render.pending_layout_activation.take() {
        match prepared.activation.decision() {
            LayoutActivationDecision::Pending => {
                render.pending_layout_activation = Some(prepared);
            }
            LayoutActivationDecision::Abort => {
                prepared.activation.complete(Ok(()));
            }
            LayoutActivationDecision::Commit => {
                let PreparedLayoutActivation {
                    spatial_engine,
                    expected_layout,
                    active_scene_id,
                    source_resolved_zones_revision,
                    prepared_resize,
                    sampling_preparation,
                    prepared_zones,
                    prepared_projected_scene,
                    activation,
                    width,
                    height,
                } = prepared;
                let completion = activation.clone();
                let publication = state
                    .scene_manager
                    .publish_layout_activation(
                        &state.spatial_engine,
                        spatial_engine,
                        &expected_layout,
                        active_scene_id,
                        source_resolved_zones_revision,
                        |spatial_engine| {
                            render
                                .render_zone_runtime
                                .commit_reconcile(prepared_zones)
                                .map_err(|error| LayoutTransactionRejection::PreparationFailed {
                                    message: error.to_string(),
                                })?;
                            if let Some(prepared_resize) = prepared_resize {
                                render.commit_canvas_resize(prepared_resize);
                                state.canvas_dims.set(width, height);
                                frame_loop.throttle.reset_for_canvas_resize();
                                info!(width, height, "Applied live canvas resize");
                            } else if let Some(sampling_preparation) = sampling_preparation {
                                render.commit_spatial_sampling_plan(sampling_preparation);
                            }
                            render
                                .sparkleflinger
                                .apply_projected_scene_resources(prepared_projected_scene);
                            scene.render_state.replace_spatial_engine(spatial_engine);
                            Ok(())
                        },
                    )
                    .await;
                completion.complete(publication);
            }
        }
    }

    for transaction in state.scene_transactions.drain() {
        match transaction {
            SceneTransaction::SetScreenCaptureConfigured(configured) => {
                scene.render_state.set_screen_capture_configured(configured);
            }
            SceneTransaction::PrepareLayout(transaction) => {
                if transaction.is_cancelled() {
                    continue;
                }
                if render.pending_layout_activation.is_some() {
                    transaction.reject(LayoutTransactionRejection::PreparationFailed {
                        message: "another layout preparation is already pending".to_owned(),
                    });
                    continue;
                }
                let spatial_engine = transaction.spatial_engine().clone();
                let expected_layout = transaction.expected_layout().clone();
                let layout = spatial_engine.layout();
                let width = layout.canvas_width;
                let height = layout.canvas_height;
                let needs_resize =
                    state.canvas_dims.width() != width || state.canvas_dims.height() != height;
                let candidate_zones = transaction.resolved_zones();
                let active_scene_id = transaction.active_scene_id();
                let source_resolved_zones_revision = transaction.source_resolved_zones_revision();
                let resolved_zones_revision = transaction.resolved_zones_revision();
                let unassigned_behavior = transaction.unassigned_behavior().clone();
                let preparation = async {
                    let (zone_layout_preview_generation, candidate_zones) =
                        if let Some(scene_id) = active_scene_id {
                            let (generation, overrides) = state
                                .zone_layout_previews
                                .scene_overrides_with_generation(scene_id)
                                .await;
                            (
                                generation,
                                apply_zone_layout_previews(candidate_zones, &overrides),
                            )
                        } else {
                            (state.zone_layout_previews.generation(), candidate_zones)
                        };
                    let (display_target_fps, display_output_routes) =
                        snapshot_display_zone_target_metadata(
                            &state.device_registry,
                            &mut scene.snapshot_cache,
                            resolved_zones_revision,
                            candidate_zones.as_ref(),
                            state.face_fps_cap,
                        )
                        .await;
                    let display_descriptors =
                        display_descriptors_for_zones(&display_target_fps, &display_output_routes);
                    let registry = state.effect_registry.read().await;
                    let dependency_key = scene_dependency_key(
                        resolved_zones_revision,
                        registry.generation(),
                        state.device_registry.generation(),
                        zone_layout_preview_generation,
                        &unassigned_behavior,
                    );
                    let (mut prepared_resize, sampling_preparation) = if needs_resize {
                        let off_color = state.power_state.borrow().effective_off_output_color();
                        (
                            Some(render.prepare_canvas_resize(
                                width,
                                height,
                                off_color,
                                &spatial_engine,
                            )?),
                            None,
                        )
                    } else {
                        (
                            None,
                            Some(render.prepare_spatial_sampling_plan(&spatial_engine)?),
                        )
                    };
                    let mut prepared_zones = render
                        .render_zone_runtime
                        .prepare_reconcile_for_scene_dimensions(
                            candidate_zones.as_ref(),
                            active_scene_id,
                            dependency_key,
                            &registry,
                            &display_descriptors,
                            Some(&spatial_engine),
                            width,
                            height,
                        )?;
                    #[cfg(feature = "wgpu")]
                    let gpu_projection_admitted = prepared_resize.as_ref().map_or_else(
                        || render.sparkleflinger.supports_gpu_output_frames(),
                        PreparedCanvasResize::render_gpu_output_admitted,
                    );
                    #[cfg(not(feature = "wgpu"))]
                    let gpu_projection_admitted = false;
                    let (scene_width, scene_height) = prepared_zones.scene_dimensions();
                    let prepared_projected_scene =
                        render.sparkleflinger.prepare_projected_scene_resources(
                            prepared_zones.projected_zone_texture_requirements(),
                            gpu_projection_admitted,
                            scene_width,
                            scene_height,
                            prepared_resize
                                .as_ref()
                                .map(PreparedCanvasResize::render_preparation),
                        );
                    let gpu_projection_admitted =
                        prepared_projected_scene.gpu_projection_admitted();
                    if !gpu_projection_admitted && let Some(prepared_resize) = &mut prepared_resize
                    {
                        prepared_resize.prepare_scene_cpu_backing()?;
                    }
                    prepared_zones.resolve_scene_backing(
                        &render.render_zone_runtime,
                        gpu_projection_admitted,
                        prepared_resize.is_some(),
                    )?;
                    Ok::<_, anyhow::Error>((
                        prepared_resize,
                        sampling_preparation,
                        prepared_zones,
                        prepared_projected_scene,
                    ))
                }
                .await;
                let (
                    prepared_resize,
                    sampling_preparation,
                    prepared_zones,
                    prepared_projected_scene,
                ) = match preparation {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        warn!(
                            %error,
                            requested_width = width,
                            requested_height = height,
                            active_width = state.canvas_dims.width(),
                            active_height = state.canvas_dims.height(),
                            "Rejected live layout because render resources could not be prepared"
                        );
                        transaction.reject(LayoutTransactionRejection::PreparationFailed {
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                render.pending_layout_activation = Some(PreparedLayoutActivation {
                    spatial_engine,
                    expected_layout,
                    active_scene_id,
                    source_resolved_zones_revision,
                    prepared_resize,
                    sampling_preparation,
                    prepared_zones,
                    prepared_projected_scene,
                    activation: transaction.activation(),
                    width,
                    height,
                });
                transaction.accept();
            }
        }
    }
}

pub(crate) async fn execute_frame(
    state: &RenderThreadState,
    runtime: &mut PipelineRuntime,
    scheduled_start: Instant,
    skip_decision: SkipDecision,
) -> FrameExecution {
    let scene = &mut runtime.scene;
    let frame_loop = &mut runtime.frame_loop;
    let render = &mut runtime.render;
    let frame_start = Instant::now();
    let frame_tick = frame_loop.clock.advance(frame_start);
    let delta_secs = frame_tick.delta_secs;
    let frame_interval = frame_tick.frame_interval;
    let frame_interval_us = frame_tick.frame_interval_us;
    let wake_late_us = micros_u32(frame_start.saturating_duration_since(scheduled_start));
    let reused_inputs = matches!(
        skip_decision,
        SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas
    );
    let reused_canvas = matches!(skip_decision, SkipDecision::ReuseCanvas);

    if state.scene_transactions.has_pending() || render.pending_layout_activation.is_some() {
        // Layout preparation owns large staged GPU resources. Keep its future
        // off the hot render future unless the control plane has work queued.
        Box::pin(service_scene_transactions(state, scene, frame_loop, render)).await;
    }
    if render.pending_layout_activation.is_some() {
        let mut render_loop = state.render_loop.write().await;
        return runtime
            .frame_policy
            .complete_throttled_frame(&mut render_loop, Duration::ZERO);
    }
    let mut scene_snapshot = build_frame_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &mut scene.render_state,
        delta_secs,
    )
    .await;
    refresh_effect_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &scene.render_state,
        &mut scene_snapshot,
    )
    .await;
    let output_power = scene_snapshot.output_power;
    publish_input_demands(
        frame_loop,
        scene_snapshot.effect_demand,
        state,
        render.sparkleflinger.screen_native_execution_target(),
    );
    let mut screen_input_active = frame_loop.has_screen_input_demand();
    scene_snapshot.effect_demand.screen_capture_active = screen_input_active;
    if !screen_input_active {
        let _ = render.screen_queue.clear_latest();
        render.sparkleflinger.release_native_screen_caches();
    }
    let scene_snapshot_done_us = micros_u32(frame_start.elapsed());
    if output_power.sleeping() {
        if output_power.effective_off_output_behavior() == OffOutputBehavior::Static
            && !frame_loop.throttle.sleep_black_pushed()
        {
            force_static_sleep_snapshot(
                state,
                &scene_snapshot,
                output_power.effective_off_output_color(),
                None,
            )
            .await;
            frame_loop.throttle.note_sleep_frame_published();
            let mut render_loop = state.render_loop.write().await;
            return runtime
                .frame_policy
                .sleep_throttle_execution(&mut render_loop);
        }
        let sleep_render_surfaces =
            render.render_surface_snapshot(state.published_canvas_receiver_count());
        if let Some(frame) = maybe_sleep_throttle(
            state,
            &mut runtime.frame_policy,
            &scene_snapshot,
            frame_start,
            scene_snapshot_done_us,
            sleep_render_surfaces,
            &mut render.output_artifacts,
            &mut frame_loop.throttle,
            &mut frame_loop.publication_cadence,
        )
        .await
        {
            return frame;
        }
    } else {
        frame_loop.throttle.clear_sleep();
    }

    if refresh_effect_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &scene.render_state,
        &mut scene_snapshot,
    )
    .await
    {
        let refreshed_demand = scene_snapshot.effect_demand;
        publish_input_demands(
            frame_loop,
            refreshed_demand,
            state,
            render.sparkleflinger.screen_native_execution_target(),
        );
        screen_input_active = frame_loop.has_screen_input_demand();
    }
    scene_snapshot.effect_demand.screen_capture_active = screen_input_active;
    if !screen_input_active {
        let _ = render.screen_queue.clear_latest();
        render.sparkleflinger.release_native_screen_caches();
    }

    if let Some(frame) = maybe_idle_throttle(
        state,
        &mut runtime.frame_policy,
        scene_snapshot.effect_demand.effect_running || frame_loop.has_active_input_demand(),
        scene_snapshot.effect_demand.screen_capture_active,
        &mut frame_loop.throttle,
    )
    .await
    {
        return frame;
    }

    let input_start = Instant::now();
    let screen_plan_generation = frame_loop.inputs.observe_screen_plan(
        state,
        render.sparkleflinger.screen_native_execution_target(),
    );
    let inputs = frame_loop
        .inputs
        .inputs_for_frame(state, skip_decision, delta_secs);
    frame_loop
        .publication_cadence
        .observe_audio_publication(inputs.audio_was_published);
    inputs.lighting = {
        let registry = state.effect_registry.read().await;
        Some(frame_loop.lighting_feed.lighting_for_frame(
            scene_snapshot.scene_runtime.active_scene_name.as_deref(),
            scene_snapshot.scene_runtime.resolved_zones.as_ref(),
            scene_snapshot.scene_runtime.resolved_zones_revision,
            &registry,
        ))
    };
    let input_snapshot_done_at = Instant::now();
    let input_us = micros_between(input_start, input_snapshot_done_at);
    let input_done_us = micros_between(frame_start, input_snapshot_done_at);
    super::frame_composer::synchronize_screen_plan_generation(
        &mut render.sparkleflinger,
        &mut render.screen_queue,
        screen_plan_generation.get(),
    );
    #[cfg(all(target_os = "macos", feature = "wgpu", feature = "screen-capture"))]
    if let Some(render_device) = state.render_gpu_device.as_ref() {
        render.service_macos_screen_parity(
            render_device,
            inputs.screen_publication.as_ref(),
            inputs.screen_descriptor.as_ref(),
            &scene_snapshot.spatial_engine,
        );
    }
    let deferred_sample_start = Instant::now();
    let PendingSamplingWork {
        completed: completed_deferred_sampling,
        stale: stale_deferred_sampling,
    } = {
        let mut sampling = render.sampling_runtime();
        sampling.prepare_pending_work(
            "Retired GPU spatial sampling finalize failed; dropping stale deferred sample result",
            "Deferred GPU spatial sampling finalize failed; dropping deferred sample result",
        )
    };
    let deferred_sample_done_at = Instant::now();
    let deferred_sample_us = micros_between(deferred_sample_start, deferred_sample_done_at);
    let canvas_preview_due = frame_loop.publication_cadence.canvas_preview_due(
        scene_snapshot.elapsed_ms,
        state.preview_canvas_receiver_count(),
        state.preview_runtime.tracked_canvas_receiver_count(),
        state.preview_runtime.tracked_canvas_demand().max_fps,
    );
    let screen_canvas_preview_due = frame_loop.publication_cadence.screen_canvas_preview_due(
        scene_snapshot.elapsed_ms,
        state.event_bus.screen_canvas_receiver_count(),
        state.preview_runtime.screen_canvas_receiver_count(),
        state.preview_runtime.screen_canvas_demand().max_fps,
    );
    let _ = refresh_effect_scene_snapshot(
        state,
        &mut scene.snapshot_cache,
        &scene.render_state,
        &mut scene_snapshot,
    )
    .await;
    scene_snapshot.effect_demand.screen_capture_active = screen_input_active;

    let render_started_at = Instant::now();
    let render_started_us = micros_between(frame_start, render_started_at);
    let mut render_stage = compose_frame(ComposeRequest {
        state,
        compose: render.compose_runtime(),
        scene_snapshot: &scene_snapshot,
        publish_canvas_preview: canvas_preview_due,
        publish_screen_canvas_preview: screen_canvas_preview_due,
        skip_decision,
        inputs,
        frame_delta: frame_interval,
    })
    .await;
    let render_us = render_stage.total_us;

    let preview_advance_start = Instant::now();
    {
        let mut preview = render.preview_runtime();
        preview.advance_gpu_preview(&mut render_stage);
    }
    let sample_start = Instant::now();
    let preview_advance_us = micros_between(preview_advance_start, sample_start);
    let LedSamplingOutcome {
        layout,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        refresh_reused_frame_metadata,
        reuses_published_frame,
        zone_shape_signature,
    } = {
        let mut sampling = render.sampling_runtime();
        resolve_led_sampling(
            state,
            &mut sampling,
            &scene_snapshot,
            &mut render_stage,
            completed_deferred_sampling,
            stale_deferred_sampling,
        )
    };

    let sample_done_at = Instant::now();
    let sample_done_us = micros_between(frame_start, sample_done_at);
    let sample_us = micros_between(sample_start, sample_done_at);
    let sample_dispatch_us = render_stage.sampled_us;
    frame_loop
        .lighting_feed
        .observe_zones(render.output_artifacts.zones(), scene_snapshot.elapsed_ms);
    let push_start = Instant::now();
    let global_brightness = output_power.effective_brightness();
    let global_brightness_bits = global_brightness.to_bits();
    let (write_stats, async_failures, output_frame_source, output_reuse_key) = {
        let mut manager = state.backend_manager.lock().await;
        let latest_output_power = *state.power_state.borrow();
        if should_switch_to_late_sleep_frame(output_power, latest_output_power) {
            scene_snapshot.output_power = latest_output_power;
            force_static_sleep_snapshot(
                state,
                &scene_snapshot,
                latest_output_power.effective_off_output_color(),
                Some(&mut *manager),
            )
            .await;
            frame_loop.throttle.note_sleep_frame_published();
            let mut render_loop = state.render_loop.write().await;
            return runtime
                .frame_policy
                .sleep_throttle_execution(&mut render_loop);
        }
        let mut unassigned_output_planner = UnassignedOutputPlanner::new(
            &manager,
            &mut render.output_artifacts.unassigned_output_cache,
        );
        let unassigned_output_plan = match unassigned_output_planner.plan(
            Arc::clone(&layout),
            &scene_snapshot.scene_runtime.unassigned_behavior,
            scene_snapshot.scene_runtime.resolved_zones.as_ref(),
            &render_stage.zone_canvases,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                warn!(%error, "Unassigned fallback sampling plan was rejected; using black output");
                unassigned_output_planner
                    .plan(
                        Arc::clone(&layout),
                        &hypercolor_types::scene::UnassignedBehavior::Off,
                        scene_snapshot.scene_runtime.resolved_zones.as_ref(),
                        &render_stage.zone_canvases,
                    )
                    .expect("black unassigned output does not construct a sampling plan")
            }
        };
        let device_brightness_generation = manager.output_brightness_generation();
        let routing_signature = manager.routed_output_signature(unassigned_output_plan.layout());
        let output_reuse_key = OutputReuseKey::new(
            global_brightness_bits,
            device_brightness_generation,
            routing_signature,
            zone_shape_signature,
            unassigned_behavior_generation(&scene_snapshot.scene_runtime.unassigned_behavior),
        );
        let output_reuse_decision = frame_loop.output_reuse.decide_frame_source(
            reuses_published_frame,
            output_reuse_key,
            || manager.can_reuse_routed_frame_outputs(unassigned_output_plan.layout()),
        );
        let output_frame_source = output_reuse_decision.source();
        let output_reuse_key = output_reuse_decision.key();
        let write_stats = match output_frame_source {
            OutputFrameSource::RoutedReuse => {
                manager.reuse_routed_frame_outputs(unassigned_output_plan.layout())
            }
            OutputFrameSource::PublishedFrame => {
                let published_frame = state.event_bus.frame_lane().borrow();
                let zones = unassigned_output_plan.zones_for(&published_frame.zones);
                manager.write_frame_with_brightness(
                    &zones,
                    unassigned_output_plan.layout(),
                    global_brightness,
                    None,
                )
            }
            OutputFrameSource::CurrentFrame => {
                let zones = unassigned_output_plan.zones_for(render.output_artifacts.zones());
                manager.write_frame_with_brightness(
                    &zones,
                    unassigned_output_plan.layout(),
                    global_brightness,
                    None,
                )
            }
        };
        frame_loop
            .output_reuse
            .record_decision(output_reuse_decision);
        let async_failures = manager.async_write_failures();
        (
            write_stats,
            async_failures,
            output_frame_source,
            output_reuse_key,
        )
    };
    let output_done_at = Instant::now();
    let push_us = micros_between(push_start, output_done_at);
    let output_done_us = micros_between(frame_start, output_done_at);

    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures);
    }
    {
        let mut sampling = render.sampling_runtime();
        sampling.finish_retired(
            "Retired GPU spatial sampling late finalize failed; dropping stale deferred sample result",
        );
    }

    let postprocess_start = Instant::now();
    let cpu_readback_skipped = matches!(
        render_stage.composed_frame.backend,
        crate::performance::CompositorBackendKind::Gpu
    ) && render_stage.composed_frame.sampling_canvas.is_none();
    {
        let mut preview = render.preview_runtime();
        preview.finalize_gpu_preview(&mut render_stage);
    }
    let postprocess_us = micros_between(postprocess_start, Instant::now());

    let frame_num_u32 = u64_to_u32(scene_snapshot.frame_token);
    let timing_total_us = micros_u32(frame_start.elapsed());
    let ComposedFrameSet {
        sampling_canvas,
        sampling_surface,
        preview_surface,
        bypassed: _,
        backend: compositor_backend,
        gpu_readback_failed,
        compositor_acceleration_downgraded,
    } = render_stage.composed_frame;
    if compositor_acceleration_downgraded {
        state.event_bus.publish(HypercolorEvent::Error {
            code: "compositor_acceleration_downgraded".to_owned(),
            message: "GPU producer composition failed; preserving GPU residency".to_owned(),
            severity: Severity::Warning,
        });
    }
    super::screen_canvas::publish_screen_zones(
        state,
        inputs.screen_data.as_ref(),
        frame_num_u32,
        u64_to_u32(scene_snapshot.elapsed_ms),
    );
    let publish_stats = publish_frame_updates(
        state,
        &mut frame_loop.publication_cadence,
        FramePublicationRequest {
            recycled_frame: render.output_artifacts.frame_mut(),
            audio: &inputs.audio,
            surfaces: FramePublicationSurfaces {
                canvas: sampling_canvas,
                frame_surface: sampling_surface,
                preview_surface,
                screen_capture_surface: inputs
                    .screen_data
                    .as_ref()
                    .and_then(|data| data.canvas_downscale.clone()),
                web_viewport_preview_surface: render_stage.web_viewport_preview,
                effect_running: scene_snapshot.effect_demand.effect_running,
                screen_capture_active: scene_snapshot.effect_demand.screen_capture_active,
            },
            scene_id: scene_snapshot.scene_runtime.active_scene_id,
            display_zone_frames: &render_stage.display_zone_frames,
            zone_canvases: &render_stage.zone_canvases,
            active_display_zone_ids: &render_stage.active_display_zone_ids,
            frame_number: frame_num_u32,
            elapsed_ms: scene_snapshot.elapsed_ms,
            reuse_existing_frame: reuses_published_frame,
            refresh_existing_frame_metadata: refresh_reused_frame_metadata,
            timing: FrameTiming {
                producer_us: render_stage.producer_us,
                composition_us: render_stage.composition_us,
                render_us,
                sample_us,
                push_us,
                total_us: timing_total_us,
                budget_us: scene_snapshot.budget_us,
            },
        },
    );
    let latest_output_power = *state.power_state.borrow();
    if should_switch_to_late_sleep_frame(output_power, latest_output_power) {
        scene_snapshot.output_power = latest_output_power;
        force_static_sleep_snapshot(
            state,
            &scene_snapshot,
            latest_output_power.effective_off_output_color(),
            None,
        )
        .await;
        frame_loop.throttle.note_sleep_frame_published();
        let mut render_loop = state.render_loop.write().await;
        return runtime
            .frame_policy
            .sleep_throttle_execution(&mut render_loop);
    }
    let publish_us = publish_stats.elapsed_us;
    let publish_done_at = Instant::now();
    let publish_done_us = micros_between(frame_start, publish_done_at);
    let render_surfaces = render.render_surface_snapshot(state.published_canvas_receiver_count());
    let total_us = publish_done_us;
    let known_stage_us = input_us
        .saturating_add(deferred_sample_us)
        .saturating_add(render_us)
        .saturating_add(preview_advance_us)
        .saturating_add(sample_us)
        .saturating_add(push_us)
        .saturating_add(postprocess_us)
        .saturating_add(publish_us);
    let overhead_us = total_us.saturating_sub(known_stage_us);
    let jitter_us = frame_interval_us.abs_diff(scene_snapshot.budget_us);
    let output_errors = u32::try_from(write_stats.errors.len()).unwrap_or(u32::MAX);
    let frame_summary = summarize_active_frame(ActiveFrameMetricsInput {
        scene_snapshot: &scene_snapshot,
        render_surfaces: &render_surfaces,
        publish_stats: &publish_stats,
        producer_full_frame_copy: render_stage.producer_full_frame_copy,
        input_us,
        deferred_sample_us,
        render_started_us,
        producer_us: render_stage.producer_us,
        producer_render_us: render_stage.producer_render_us,
        producer_scene_compose_us: render_stage.producer_scene_compose_us,
        composition_us: render_stage.composition_us,
        producer_done_us: render_stage.producer_done_us,
        composition_done_us: render_stage.composition_done_us,
        render_us,
        preview_advance_us,
        sample_us,
        sample_dispatch_us,
        push_us,
        postprocess_us,
        total_us,
        wake_late_us,
        jitter_us,
        overhead_us,
        reused_inputs,
        reused_canvas,
        gpu_zone_sampling,
        gpu_sample_deferred,
        gpu_sample_stale,
        gpu_sample_retry_hit,
        gpu_sample_queue_saturated,
        gpu_sample_wait_blocked,
        gpu_sample_cpu_fallback,
        cpu_readback_skipped,
        gpu_readback_failed,
        compositor_backend,
        output_frame_source: output_frame_source_kind(output_frame_source),
        output_reuses_published_frame: reuses_published_frame,
        output_brightness_bits: output_reuse_key.output_brightness_bits,
        output_brightness_generation: output_reuse_key.device_output_brightness_generation,
        output_routing_signature: output_reuse_key.routing_signature,
        output_zone_shape_signature: output_reuse_key.zone_shape_signature,
        output_unassigned_behavior_generation: output_reuse_key.unassigned_behavior_generation,
        devices_written: u32::try_from(write_stats.devices_written).unwrap_or(u32::MAX),
        total_leds: u32::try_from(write_stats.total_leds).unwrap_or(u32::MAX),
        output_errors,
        logical_layer_count: render_stage.logical_layer_count,
        render_zone_count: render_stage.render_zone_count,
        scene_active: render_stage.scene_active,
        scene_transition_active: render_stage.scene_transition_active,
        effect_retained: render_stage.effect_retained,
        screen_retained: render_stage.screen_retained,
        composition_bypassed: render_stage.composition_bypassed,
        preview_surface_pressure: render_stage.preview_surface_pressure,
        scene_canvas_forced_surface: render_stage.scene_canvas_forced_surface,
        scene_snapshot_done_us,
        input_done_us,
        sample_done_us,
        output_done_us,
        publish_done_us,
    });
    let frame_metrics = frame_summary.metrics;

    {
        let mut performance = state.performance.write().await;
        performance.record_frame(&frame_metrics);
    }

    let completion_report =
        FrameCompletionReport::new(frame_interval_us, &frame_metrics, &write_stats);
    report_active_frame_completion(&completion_report, &write_stats.errors);

    let (next_wake, next_skip_decision) = {
        let mut rl = state.render_loop.write().await;
        runtime
            .frame_policy
            .set_configured_max_tier(state.configured_max_fps_tier.get());
        let execution = runtime
            .frame_policy
            .complete_render_frame(&mut rl, frame_summary.admission);
        (execution.next_wake, execution.next_skip_decision)
    };

    if should_record_idle_black_frame(
        scene_snapshot.effect_demand.effect_running,
        scene_snapshot.effect_demand.screen_capture_active,
        reuses_published_frame,
    ) {
        frame_loop.throttle.note_idle_frame_without_effect();
    }

    FrameExecution {
        next_wake,
        next_skip_decision,
    }
}

fn publish_input_demands(
    frame_loop: &mut super::pipeline_runtime::FrameLoopState,
    effect_demand: super::scene_snapshot::EffectDemand,
    state: &RenderThreadState,
    screen_target: Option<&hypercolor_core::input::screen::ScreenNativeExecutionTarget>,
) {
    let screen_extent = PixelExtent::new(state.canvas_dims.width(), state.canvas_dims.height())
        .expect("render canvas dimensions are non-empty");
    frame_loop.publish_input_demands(
        effect_demand,
        state.configured_max_fps_tier.get().fps(),
        screen_extent,
        screen_target,
    );
}

fn should_record_idle_black_frame(
    effect_running: bool,
    screen_capture_active: bool,
    reuses_published_frame: bool,
) -> bool {
    !effect_running && !screen_capture_active && !reuses_published_frame
}

async fn force_static_sleep_snapshot(
    state: &RenderThreadState,
    scene_snapshot: &FrameSceneSnapshot,
    color: [u8; 3],
    backend_manager: Option<&mut BackendManager>,
) {
    let layout = scene_snapshot.spatial_engine.layout();
    let mut canvas = Canvas::new(layout.canvas_width, layout.canvas_height);
    canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
    let zones = scene_snapshot.spatial_engine.sample(&canvas);

    let (write_stats, async_failures) = if let Some(manager) = backend_manager {
        let write_stats = manager.write_frame(&zones, layout.as_ref());
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    } else {
        let mut manager = state.backend_manager.lock().await;
        let write_stats = manager.write_frame(&zones, layout.as_ref());
        let async_failures = manager.async_write_failures();
        (write_stats, async_failures)
    };
    if let Some(runtime) = &state.discovery_runtime {
        handle_async_write_failures(runtime, async_failures);
    }
    if !write_stats.errors.is_empty() {
        warn!(
            error_count = write_stats.errors.len(),
            "Forced static sleep snapshot encountered output errors"
        );
    }

    let frame_number = state
        .event_bus
        .frame_receiver()
        .borrow()
        .frame_number
        .saturating_add(1);
    let elapsed_ms = u64_to_u32(scene_snapshot.elapsed_ms);
    let canvas_frame = CanvasFrame::from_canvas(&canvas, frame_number, elapsed_ms);
    let zone_frame = DisplayZoneFrame::Canvas(canvas_frame.clone());
    let (_, display_zone_targets) = state.event_bus.display_zone_targets_snapshot();
    for zone_id in display_zone_targets.keys().copied() {
        state
            .event_bus
            .zone_canvas_sender(zone_id)
            .send_replace(zone_frame.clone());
    }
    state
        .event_bus
        .frame_lane()
        .send_replace(FrameData::new(zones, frame_number, elapsed_ms));
    state
        .event_bus
        .scene_canvas_lane()
        .send_replace(canvas_frame.clone());
    state.event_bus.canvas_lane().send_replace(canvas_frame);
    state
        .preview_runtime
        .note_canvas_frame(frame_number, elapsed_ms);
}

fn should_switch_to_late_sleep_frame(
    frame_output_power: crate::output_power::OutputPowerState,
    latest_output_power: crate::output_power::OutputPowerState,
) -> bool {
    !frame_output_power.sleeping() && latest_output_power.sleeping()
}

const fn output_frame_source_kind(source: OutputFrameSource) -> OutputFrameSourceKind {
    match source {
        OutputFrameSource::CurrentFrame => OutputFrameSourceKind::CurrentFrame,
        OutputFrameSource::PublishedFrame => OutputFrameSourceKind::PublishedFrame,
        OutputFrameSource::RoutedReuse => OutputFrameSourceKind::RoutedReuse,
    }
}

#[cfg(test)]
mod tests {
    use crate::output_power::OutputPowerState;
    use crate::performance::CompositorBackendKind;
    use crate::render_thread::frame_composer::RenderStageStats;
    use crate::render_thread::frame_sampling::LedSamplingStrategy;
    use crate::render_thread::frame_sampling::{
        blend_scene_zone_frames, build_transition_layout,
        can_hold_published_frame_for_deferred_sampling, can_hold_zone_colors_for_deferred_sampling,
        can_reuse_published_frame_for_deferred_sampling,
    };
    use crate::render_thread::pipeline_runtime::SceneTransitionKey;
    use crate::render_thread::pipeline_runtime::needs_gpu_preview_advance;
    use crate::render_thread::sparkleflinger::ComposedFrameSet;
    use hypercolor_core::spatial::SpatialEngine;
    use hypercolor_types::canvas::{Canvas, PublishedSurface};
    use hypercolor_types::event::{FrameData, ZoneColors};
    use hypercolor_types::scene::{ColorInterpolation, SceneId};
    use hypercolor_types::session::OffOutputBehavior;
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    };

    fn render_stage(
        backend: CompositorBackendKind,
        preview_requested: bool,
        preview_surface_present: bool,
    ) -> RenderStageStats {
        let mut composed_frame = ComposedFrameSet {
            sampling_canvas: None,
            sampling_surface: None,
            preview_surface: None,
            bypassed: false,
            backend,
            gpu_readback_failed: false,
            compositor_acceleration_downgraded: false,
        };
        if preview_surface_present {
            composed_frame.preview_surface =
                Some(PublishedSurface::from_owned_canvas(Canvas::new(1, 1), 0, 0));
        }
        RenderStageStats {
            composed_frame,
            preview_requested,
            web_viewport_preview: None,
            producer_full_frame_copy: crate::performance::FullFrameCopyMetrics::default(),
            display_zone_frames: Vec::new(),
            zone_canvases: Vec::new(),
            active_display_zone_ids: Vec::new(),
            led_sampling_strategy: LedSamplingStrategy::SparkleFlinger(SpatialEngine::new(
                sample_layout(&[]),
            )),
            producer_render_us: 0,
            producer_scene_compose_us: 0,
            sampled_us: 0,
            producer_us: 0,
            producer_done_us: 0,
            composition_us: 0,
            composition_done_us: 0,
            total_us: 0,
            logical_layer_count: 0,
            render_zone_count: 0,
            scene_active: false,
            scene_transition_active: false,
            effect_retained: false,
            screen_retained: false,
            composition_bypassed: false,
            preview_surface_pressure: false,
            scene_canvas_forced_surface: false,
        }
    }

    #[test]
    fn gpu_preview_advances_only_when_requested() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, false, false);
        assert!(!needs_gpu_preview_advance(&render_stage));
    }

    #[test]
    fn gpu_preview_does_not_advance_when_surface_is_ready() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, true, true);
        assert!(!needs_gpu_preview_advance(&render_stage));
    }

    #[test]
    fn gpu_preview_advances_when_requested_and_unresolved() {
        let render_stage = render_stage(CompositorBackendKind::Gpu, true, false);
        assert!(needs_gpu_preview_advance(&render_stage));
    }

    #[test]
    fn idle_black_frame_is_recorded_only_after_current_idle_output() {
        assert!(super::should_record_idle_black_frame(false, false, false));
        assert!(!super::should_record_idle_black_frame(false, false, true));
        assert!(!super::should_record_idle_black_frame(false, true, false));
        assert!(!super::should_record_idle_black_frame(true, false, false));
    }

    #[test]
    fn late_sleep_frame_takes_over_admitted_running_frame() {
        let running = OutputPowerState::default();
        let sleeping = OutputPowerState {
            session_sleeping: true,
            session_brightness: 0.0,
            off_output_behavior: OffOutputBehavior::Static,
            off_output_color: [0, 0, 0],
            ..OutputPowerState::default()
        };

        assert!(super::should_switch_to_late_sleep_frame(running, sleeping));
        assert!(!super::should_switch_to_late_sleep_frame(
            sleeping, sleeping
        ));
        assert!(!super::should_switch_to_late_sleep_frame(running, running));
    }

    fn sample_layout(zone_ids: &[&str]) -> SpatialLayout {
        SpatialLayout {
            id: "layout".to_owned(),
            name: "layout".to_owned(),
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: zone_ids
                .iter()
                .map(|zone_id| Output {
                    id: (*zone_id).to_owned(),
                    name: (*zone_id).to_owned(),
                    device_id: "device".to_owned(),
                    zone_name: None,
                    position: NormalizedPosition::new(0.5, 0.5),
                    size: NormalizedPosition::new(1.0, 1.0),
                    rotation: 0.0,
                    scale: 1.0,
                    display_order: 0,
                    orientation: None,
                    topology: LedTopology::Point,
                    led_positions: vec![NormalizedPosition::new(0.5, 0.5)],
                    led_mapping: None,
                    sampling_mode: Some(SamplingMode::Nearest),
                    edge_behavior: Some(EdgeBehavior::Clamp),
                    shape: None,
                    shape_preset: None,
                    attachment: None,
                    brightness: None,
                })
                .collect(),
            default_sampling_mode: SamplingMode::Nearest,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    fn published_frame(zone_ids: &[&str]) -> FrameData {
        FrameData::new(
            zone_ids
                .iter()
                .map(|zone_id| ZoneColors {
                    zone_id: (*zone_id).to_owned(),
                    colors: vec![[0, 0, 0]],
                })
                .collect(),
            1,
            1,
        )
    }

    fn layout_with_display_zone() -> SpatialLayout {
        let mut layout = sample_layout(&["left", "display", "right"]);
        layout.zones[1].zone_name = Some("Display".to_owned());
        layout
    }

    fn zone(zone_id: &str, colors: &[[u8; 3]]) -> ZoneColors {
        ZoneColors {
            zone_id: zone_id.to_owned(),
            colors: colors.to_vec(),
        }
    }

    #[test]
    fn gpu_zone_sampling_reuses_retained_frame_only_when_layout_matches_without_backlog() {
        let mut render_stage = render_stage(CompositorBackendKind::Gpu, false, false);
        render_stage.screen_retained = true;
        let layout = sample_layout(&["left", "right"]);
        let retained_frame = published_frame(&["left", "right"]);

        assert!(can_reuse_published_frame_for_deferred_sampling(
            &render_stage,
            &layout,
            &retained_frame
        ));
        assert!(!can_reuse_published_frame_for_deferred_sampling(
            &render_stage,
            &layout,
            &published_frame(&["left", "other"])
        ));
        render_stage.screen_retained = false;
        assert!(!can_reuse_published_frame_for_deferred_sampling(
            &render_stage,
            &layout,
            &retained_frame
        ));
    }

    #[test]
    fn gpu_zone_sampling_can_hold_previous_frame_when_layout_matches_without_backlog() {
        let layout = sample_layout(&["left", "right"]);
        let retained_frame = published_frame(&["left", "right"]);

        assert!(can_hold_published_frame_for_deferred_sampling(
            &layout,
            &retained_frame
        ));
        assert!(!can_hold_published_frame_for_deferred_sampling(
            &layout,
            &published_frame(&["left", "other"])
        ));
    }

    #[test]
    fn completed_deferred_zone_colors_can_drive_matching_layout() {
        let layout = sample_layout(&["left", "right"]);
        let zones = vec![zone("left", &[[255, 0, 0]]), zone("right", &[[0, 0, 255]])];

        assert!(can_hold_zone_colors_for_deferred_sampling(&layout, &zones));
        assert!(!can_hold_zone_colors_for_deferred_sampling(
            &layout,
            &[zone("left", &[[255, 0, 0]])]
        ));
    }

    #[test]
    fn gpu_zone_sampling_ignores_display_only_zones_when_reusing_published_frame() {
        let layout = layout_with_display_zone();

        assert!(can_hold_published_frame_for_deferred_sampling(
            &layout,
            &published_frame(&["left", "right"])
        ));
        assert!(!can_hold_published_frame_for_deferred_sampling(
            &layout,
            &published_frame(&["left", "display", "right"])
        ));
    }

    #[test]
    fn transition_layout_keeps_current_order_and_appends_base_only_zones() {
        let base_layout = sample_layout(&["left", "legacy"]);
        let current_layout = sample_layout(&["left", "right"]);
        let transition_layout = build_transition_layout(
            &base_layout,
            &current_layout,
            SceneTransitionKey {
                epoch: 1,
                from_scene: SceneId::new(),
                to_scene: SceneId::new(),
            },
        );

        let zone_ids = transition_layout
            .zones
            .iter()
            .map(|zone| zone.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(zone_ids, vec!["left", "right", "legacy"]);
    }

    #[test]
    fn zone_transition_blend_unions_shared_and_missing_zone_outputs() {
        let transition_layout = sample_layout(&["shared", "entering", "leaving"]);
        let from = vec![
            zone("shared", &[[255, 0, 0]]),
            zone("leaving", &[[0, 255, 0]]),
        ];
        let to = vec![
            zone("shared", &[[0, 0, 255]]),
            zone("entering", &[[255, 255, 255]]),
        ];
        let mut blended = Vec::new();

        blend_scene_zone_frames(
            &from,
            &to,
            &transition_layout,
            0.5,
            &ColorInterpolation::Srgb,
            &mut blended,
        );

        assert_eq!(blended.len(), 3);
        assert_eq!(blended[0].zone_id, "shared");
        assert_eq!(blended[1].zone_id, "entering");
        assert_eq!(blended[2].zone_id, "leaving");
        assert_ne!(blended[0].colors[0], [255, 0, 0]);
        assert_ne!(blended[0].colors[0], [0, 0, 255]);
        assert!(blended[1].colors[0][0] > 0);
        assert!(blended[1].colors[0][1] > 0);
        assert!(blended[1].colors[0][2] > 0);
        assert!(blended[2].colors[0][1] > 0);
        assert_eq!(blended[2].colors[0][0], 0);
        assert_eq!(blended[2].colors[0][2], 0);
    }
}
