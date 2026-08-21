//! The one output service — power and brightness (Spec 78 §4).
//!
//! Global output has exactly two knobs and this module owns both. Power
//! is a latch over the session's output override plus the render loop;
//! brightness is a persisted device setting mirrored into live power
//! state. Every surface that used to carry its own copy (`/output/power`,
//! `/settings/brightness`, effect pause/resume, the MCP brightness and
//! power tools) reaches the same two functions here.
//!
//! Range validation lives in this service rather than in the type layer:
//! `OutputPatchRequest.brightness` is an `f32` on the wire, and a
//! `0.0..=1.0` bound is a domain rule that renders as a validation
//! error, not a parse failure.

use std::sync::Arc;
use std::time::Instant;

use hypercolor_core::bus::{CanvasFrame, HypercolorBus};
use hypercolor_core::device::BackendManager;
use hypercolor_core::engine::{RenderLoop, RenderLoopState};
use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode, OutputResource};
use hypercolor_types::canvas::{Canvas, Rgba};
use hypercolor_types::event::{FrameData, HypercolorEvent, ZoneColors};
use hypercolor_types::session::OffOutputBehavior;
use tokio::sync::{Mutex, RwLock, watch};

use crate::device_settings::DeviceSettingsStore;
use crate::domain::DomainError;
use crate::domain::context::{DeviceContext, RuntimeSessionService};
use crate::domain::spatial::SpatialService;
use crate::performance::PerformanceTracker;
use crate::preview_runtime::PreviewRuntime;
use crate::session::{
    OutputOverride, OutputPowerState, clear_output_override, current_global_brightness,
    set_global_brightness, set_manual_pause, set_output_stopped,
};

/// Owning authority for global output power, brightness, and quiescence.
#[derive(Clone)]
pub struct OutputContext {
    power: watch::Sender<OutputPowerState>,
    transition: Arc<Mutex<()>>,
    settings: Arc<RwLock<DeviceSettingsStore>>,
    event_bus: Arc<HypercolorBus>,
    runtime_session: RuntimeSessionService,
    performance: Arc<RwLock<PerformanceTracker>>,
    render_loop: Arc<RwLock<RenderLoop>>,
    spatial: SpatialService,
    backend_manager: Arc<Mutex<BackendManager>>,
    preview_runtime: Arc<PreviewRuntime>,
    devices: DeviceContext,
    start_time: Instant,
}

impl OutputContext {
    #[expect(
        clippy::too_many_arguments,
        reason = "the composition root supplies the complete output ownership boundary"
    )]
    pub(crate) fn new(
        power: watch::Sender<OutputPowerState>,
        transition: Arc<Mutex<()>>,
        settings: Arc<RwLock<DeviceSettingsStore>>,
        event_bus: Arc<HypercolorBus>,
        runtime_session: RuntimeSessionService,
        performance: Arc<RwLock<PerformanceTracker>>,
        render_loop: Arc<RwLock<RenderLoop>>,
        spatial: SpatialService,
        backend_manager: Arc<Mutex<BackendManager>>,
        preview_runtime: Arc<PreviewRuntime>,
        devices: DeviceContext,
        start_time: Instant,
    ) -> Self {
        Self {
            power,
            transition,
            settings,
            event_bus,
            runtime_session,
            performance,
            render_loop,
            spatial,
            backend_manager,
            preview_runtime,
            devices,
            start_time,
        }
    }

    /// Whether output is awake and the render loop is running.
    pub async fn is_running(&self) -> bool {
        !self.power.borrow().sleeping()
            && self.render_loop.read().await.state() != RenderLoopState::Paused
    }

    /// Bring output back to running before a freshly applied effect starts.
    pub async fn wake_for_effect_start(&self) -> bool {
        if self.is_running().await {
            return true;
        }
        set_power(self, OutputPowerMode::Running).await;
        self.is_running().await
    }

    /// Quiesce render and network output after the final effect stops.
    pub async fn quiesce_after_effect_stop(&self) -> usize {
        let _transition_guard = self.transition.lock().await;
        self.render_loop.write().await.pause();
        set_output_stopped(&self.power, &self.event_bus);
        let released = self.devices.release_renderable_network_devices().await;
        self.publish_static_snapshot([0, 0, 0]).await;
        self.performance.write().await.clear_frame_timings();
        released
    }

    /// Re-publish a held static frame after output topology changes.
    pub async fn reconcile_static_hold(&self) -> bool {
        let _transition_guard = self.transition.lock().await;
        let output_power = *self.power.borrow();
        if !output_power.sleeping()
            || output_power.effective_off_output_behavior() != OffOutputBehavior::Static
        {
            return false;
        }

        self.publish_static_snapshot(output_power.effective_off_output_color())
            .await;
        true
    }

    pub async fn publish_static_snapshot(&self, color: [u8; 3]) {
        let (layout, canvas, mut zones) = {
            let spatial = self.spatial.snapshot();
            let layout = spatial.layout();
            let Ok(mut canvas) = Canvas::try_new(layout.canvas_width, layout.canvas_height)
                .inspect_err(|error| {
                    tracing::warn!(%error, "Static output canvas allocation failed; preserving the last published output");
                })
            else {
                return;
            };
            canvas.fill(Rgba::new(color[0], color[1], color[2], 255));
            let Ok(zones) = spatial.try_sample(&canvas).inspect_err(|error| {
                tracing::warn!(%error, "Static output sampling failed; preserving the last published output");
            }) else {
                return;
            };
            (layout, canvas, zones)
        };
        let frame_number = self
            .event_bus
            .frame_receiver()
            .borrow()
            .frame_number
            .saturating_add(1);
        let elapsed_ms = u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX);

        let write_stats = {
            let mut backend_manager = self.backend_manager.lock().await;
            let unassigned_outputs = backend_manager.unassigned_output_zones(layout.as_ref());
            if unassigned_outputs.is_empty() {
                backend_manager.write_frame(&zones, layout.as_ref())
            } else {
                zones.extend(unassigned_outputs.iter().map(|output| ZoneColors {
                    zone_id: output.id.clone(),
                    colors: vec![
                        color;
                        usize::try_from(output.topology.led_count()).unwrap_or_default()
                    ],
                }));
                let mut static_layout = layout.as_ref().clone();
                static_layout.zones.extend(unassigned_outputs);
                backend_manager.write_frame(&zones, &static_layout)
            }
        };
        if !write_stats.errors.is_empty() {
            tracing::warn!(
                error_count = write_stats.errors.len(),
                "One-shot static frame encountered output errors while quiescing effect output"
            );
        }

        let canvas_frame = CanvasFrame::from_canvas(&canvas, frame_number, elapsed_ms);
        let group_frame = hypercolor_core::bus::DisplayGroupFrame::Canvas(canvas_frame.clone());
        let (_, display_group_targets) = self.event_bus.display_group_targets_snapshot();
        for group_id in display_group_targets.keys().copied() {
            self.event_bus
                .group_canvas_sender(group_id)
                .send_replace(group_frame.clone());
        }
        self.event_bus
            .frame_lane()
            .send_replace(FrameData::new(zones, frame_number, elapsed_ms));
        self.event_bus
            .scene_canvas_lane()
            .send_replace(canvas_frame.clone());
        self.event_bus.canvas_lane().send_replace(canvas_frame);
        self.preview_runtime
            .note_canvas_frame(frame_number, elapsed_ms);
    }
}

/// Which outputs a released pause has to reconnect on resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputReconnectScope {
    All,
    Network,
}

/// Read the live output resource.
pub fn get_output(ctx: &OutputContext) -> OutputResource {
    let power = *ctx.power.borrow();
    OutputResource {
        power: observed_power(power),
        brightness: power.global_brightness,
    }
}

/// Apply a partial output patch and return the resulting resource.
///
/// A patch naming neither field is rejected: `PATCH /output` with an
/// empty document is far more likely to be a client that dropped its
/// payload than a caller asking for a deliberate no-op, and a silent
/// 200 there is the same defect class as a silently discarded query
/// filter. Callers wanting a read use `GET /output`.
pub async fn patch_output(
    ctx: &OutputContext,
    request: OutputPatchRequest,
) -> Result<OutputResource, DomainError> {
    let OutputPatchRequest { power, brightness } = request;
    if power.is_none() && brightness.is_none() {
        return Err(DomainError::validation(
            "output patch must set power, brightness, or both",
        ));
    }

    if let Some(brightness) = brightness {
        set_brightness(ctx, brightness).await?;
    }
    if let Some(power) = power {
        set_power(ctx, power).await;
    }

    Ok(get_output(ctx))
}

/// Set global brightness, persisting it and mirroring it into live
/// power state.
pub async fn set_brightness(ctx: &OutputContext, brightness: f32) -> Result<(), DomainError> {
    if !(0.0..=1.0).contains(&brightness) {
        return Err(DomainError::validation_field(
            "brightness",
            "brightness must be between 0.0 and 1.0",
        ));
    }

    let previous = brightness_percent(current_global_brightness(&ctx.power));

    {
        let mut settings = ctx.settings.write().await;
        settings.set_global_brightness(brightness);
        settings.save().map_err(|error| {
            DomainError::Internal(anyhow::anyhow!(
                "Failed to persist global brightness: {error}"
            ))
        })?;
    }
    ctx.event_bus
        .publish(HypercolorEvent::DeviceSettingsChanged { key: None });

    set_global_brightness(&ctx.power, brightness);
    ctx.event_bus.publish(HypercolorEvent::BrightnessChanged {
        old: previous,
        new_value: brightness_percent(brightness),
    });

    ctx.runtime_session.save().await;
    Ok(())
}

/// Drive global output power to the requested mode.
pub async fn set_power(ctx: &OutputContext, requested: OutputPowerMode) {
    let _transition_guard = ctx.transition.lock().await;
    let previous = *ctx.power.borrow();
    match requested {
        OutputPowerMode::Paused => {
            let static_color = [0, 0, 0];
            set_manual_pause(&ctx.power, &ctx.event_bus, true, static_color);
            schedule_released_output_reconnect(ctx, previous);
            ctx.publish_static_snapshot(static_color).await;
            ctx.performance.write().await.clear_frame_timings();
            ctx.render_loop.write().await.pause();
        }
        OutputPowerMode::Running => {
            clear_output_override(&ctx.power, &ctx.event_bus);
            ctx.render_loop.write().await.resume();
            schedule_released_output_reconnect(ctx, previous);
        }
    }

    ctx.runtime_session.save().await;
}

/// Project internal power state onto the two observable modes.
///
/// A destructive stop and a session sleep both leave outputs dark, so
/// both read as `paused` — the resource reports whether output is
/// running, and the stop's extra consequences (released ownership,
/// cleared effect state) are observable on the effect surface. The read
/// has to round-trip: a caller that reads `running` and then patches
/// `running` must not be silently clearing a stop.
///
/// [`OutputPowerState::reported_paused`] is the same projection used by
/// the WS hello and MCP status surfaces. A stop publishes no `Paused`
/// event, so clients reconcile the power snapshot after an
/// `EffectStopped` lifecycle event.
///
/// [`OutputPowerState::reported_paused`]: crate::session::OutputPowerState::reported_paused
fn observed_power(power: OutputPowerState) -> OutputPowerMode {
    if power.sleeping() {
        OutputPowerMode::Paused
    } else {
        OutputPowerMode::Running
    }
}

fn schedule_released_output_reconnect(ctx: &OutputContext, previous: OutputPowerState) {
    match released_output_reconnect_scope(previous) {
        Some(OutputReconnectScope::All) => {
            ctx.devices.schedule_output_reconnect(false);
        }
        Some(OutputReconnectScope::Network) => {
            ctx.devices.schedule_output_reconnect(true);
        }
        None => {}
    }
}

fn released_output_reconnect_scope(previous: OutputPowerState) -> Option<OutputReconnectScope> {
    if previous.session_release_active() {
        Some(OutputReconnectScope::All)
    } else if previous.output_override == OutputOverride::Stopped {
        Some(OutputReconnectScope::Network)
    } else {
        None
    }
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to the unit interval before scaling to a percentage"
)]
pub(crate) fn brightness_percent(brightness: f32) -> u8 {
    let scaled = (brightness.clamp(0.0, 1.0) * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        scaled as u8
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::session::OffOutputBehavior;

    use super::{
        OutputReconnectScope, brightness_percent, observed_power, released_output_reconnect_scope,
    };
    use crate::session::{OutputOverride, OutputPowerState};
    use hypercolor_types::api::output::OutputPowerMode;

    #[test]
    fn reconnect_scope_tracks_the_outputs_each_release_path_owns() {
        let session_release = OutputPowerState {
            session_sleeping: true,
            off_output_behavior: OffOutputBehavior::Release,
            ..OutputPowerState::default()
        };
        assert_eq!(
            released_output_reconnect_scope(session_release),
            Some(OutputReconnectScope::All)
        );

        let stopped = OutputPowerState {
            output_override: OutputOverride::Stopped,
            ..OutputPowerState::default()
        };
        assert_eq!(
            released_output_reconnect_scope(stopped),
            Some(OutputReconnectScope::Network)
        );

        let paused = OutputPowerState {
            output_override: OutputOverride::Paused,
            ..session_release
        };
        assert_eq!(released_output_reconnect_scope(paused), None);
    }

    #[test]
    fn every_dark_state_observes_as_paused() {
        assert_eq!(
            observed_power(OutputPowerState::default()),
            OutputPowerMode::Running
        );
        for dark in [
            OutputPowerState {
                output_override: OutputOverride::Paused,
                ..OutputPowerState::default()
            },
            OutputPowerState {
                output_override: OutputOverride::Stopped,
                ..OutputPowerState::default()
            },
            OutputPowerState {
                session_sleeping: true,
                ..OutputPowerState::default()
            },
        ] {
            assert_eq!(observed_power(dark), OutputPowerMode::Paused);
        }
    }

    #[test]
    fn brightness_percent_saturates_at_both_ends() {
        assert_eq!(brightness_percent(-1.0), 0);
        assert_eq!(brightness_percent(0.0), 0);
        assert_eq!(brightness_percent(0.375), 38);
        assert_eq!(brightness_percent(1.0), 100);
        assert_eq!(brightness_percent(2.0), 100);
    }
}
