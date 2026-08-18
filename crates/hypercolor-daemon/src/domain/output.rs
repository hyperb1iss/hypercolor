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

use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode, OutputResource};
use hypercolor_types::event::HypercolorEvent;

use crate::api::AppState;
use crate::domain::DomainError;
use crate::session::{
    OutputOverride, OutputPowerState, clear_output_override, current_global_brightness,
    set_global_brightness, set_manual_pause,
};

/// Which outputs a released pause has to reconnect on resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputReconnectScope {
    All,
    Network,
}

/// Read the live output resource.
pub fn get_output(state: &AppState) -> OutputResource {
    let power = *state.power_state.borrow();
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
    state: &AppState,
    request: OutputPatchRequest,
) -> Result<OutputResource, DomainError> {
    let OutputPatchRequest { power, brightness } = request;
    if power.is_none() && brightness.is_none() {
        return Err(DomainError::validation(
            "output patch must set power, brightness, or both",
        ));
    }

    if let Some(brightness) = brightness {
        set_brightness(state, brightness).await?;
    }
    if let Some(power) = power {
        set_power(state, power).await;
    }

    Ok(get_output(state))
}

/// Set global brightness, persisting it and mirroring it into live
/// power state.
pub async fn set_brightness(state: &AppState, brightness: f32) -> Result<(), DomainError> {
    if !(0.0..=1.0).contains(&brightness) {
        return Err(DomainError::validation_field(
            "brightness",
            "brightness must be between 0.0 and 1.0",
        ));
    }

    let previous = brightness_percent(current_global_brightness(&state.power_state));

    {
        let mut settings = state.device_settings.write().await;
        settings.set_global_brightness(brightness);
        settings.save().map_err(|error| {
            DomainError::Internal(anyhow::anyhow!(
                "Failed to persist global brightness: {error}"
            ))
        })?;
    }
    state
        .event_bus
        .publish(HypercolorEvent::DeviceSettingsChanged { key: None });

    set_global_brightness(&state.power_state, brightness);
    state.event_bus.publish(HypercolorEvent::BrightnessChanged {
        old: previous,
        new_value: brightness_percent(brightness),
    });

    crate::api::save_runtime_session_snapshot(state).await;
    Ok(())
}

/// Drive global output power to the requested mode.
pub async fn set_power(state: &AppState, requested: OutputPowerMode) {
    let _transition_guard = state.output_power_transition.lock().await;
    let previous = *state.power_state.borrow();
    match requested {
        OutputPowerMode::Paused => {
            let static_color = [0, 0, 0];
            set_manual_pause(&state.power_state, &state.event_bus, true, static_color);
            schedule_released_output_reconnect(state, previous);
            crate::api::effects::publish_static_output_snapshot(state, static_color).await;
            state.performance.write().await.clear_frame_timings();
            state.render_loop.write().await.pause();
        }
        OutputPowerMode::Running => {
            clear_output_override(&state.power_state, &state.event_bus);
            state.render_loop.write().await.resume();
            schedule_released_output_reconnect(state, previous);
        }
    }

    crate::api::save_runtime_session_snapshot(state).await;
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
/// [`OutputPowerState::reported_paused`] answers a different question
/// ("did the user latch a pause?") and keeps returning `false` for a
/// stop, because a stop publishes no `Paused` event. The WS hello and
/// the MCP status surfaces read that one. §3 of the REST matrix names
/// the split and the wave that collapses it.
///
/// [`OutputPowerState::reported_paused`]: crate::session::OutputPowerState::reported_paused
fn observed_power(power: OutputPowerState) -> OutputPowerMode {
    if power.sleeping() {
        OutputPowerMode::Paused
    } else {
        OutputPowerMode::Running
    }
}

fn schedule_released_output_reconnect(state: &AppState, previous: OutputPowerState) {
    match released_output_reconnect_scope(previous) {
        Some(OutputReconnectScope::All) => {
            crate::api::effects::schedule_all_output_reconnect(state);
        }
        Some(OutputReconnectScope::Network) => {
            crate::api::effects::schedule_network_output_reconnect(state);
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
