//! Global output power endpoints and transition orchestration.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::Response;
use hypercolor_types::api::output::{
    OutputPowerMode, OutputPowerResponse, OutputPowerStatus, SetOutputPowerRequest,
};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::session::OutputOverride;
use crate::session::{clear_output_override, set_manual_pause};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputReconnectScope {
    All,
    Network,
}

/// `GET /api/v1/output/power` - Read effective global output power.
#[utoipa::path(
    get,
    path = "/api/v1/output/power",
    responses(
        (status = 200, description = "Current global output power", body = crate::api::envelope::ApiResponse<OutputPowerResponse>)
    ),
    tag = "output"
)]
pub async fn get_output_power(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(output_power_response(state.as_ref()))
}

/// `PUT /api/v1/output/power` - Set desired global output power.
#[utoipa::path(
    put,
    path = "/api/v1/output/power",
    request_body = SetOutputPowerRequest,
    responses(
        (status = 200, description = "Updated global output power", body = crate::api::envelope::ApiResponse<OutputPowerResponse>)
    ),
    tag = "output"
)]
pub async fn put_output_power(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SetOutputPowerRequest>,
) -> Response {
    ApiResponse::ok(set_output_power(state.as_ref(), request.state).await)
}

pub(crate) async fn set_output_power(
    state: &AppState,
    requested: OutputPowerMode,
) -> OutputPowerResponse {
    let _transition_guard = state.output_power_transition.lock().await;
    let previous = *state.power_state.borrow();
    match requested {
        OutputPowerMode::Paused => {
            let static_color = [0, 0, 0];
            set_manual_pause(&state.power_state, &state.event_bus, true, static_color);
            schedule_released_output_reconnect(state, previous);
            super::effects::publish_static_output_snapshot(state, static_color).await;
            state.performance.write().await.clear_frame_timings();
            state.render_loop.write().await.pause();
        }
        OutputPowerMode::Running => {
            clear_output_override(&state.power_state, &state.event_bus);
            state.render_loop.write().await.resume();
            schedule_released_output_reconnect(state, previous);
        }
    }

    super::save_runtime_session_snapshot(state).await;
    output_power_response(state)
}

pub(crate) fn output_power_response(state: &AppState) -> OutputPowerResponse {
    let power = *state.power_state.borrow();
    let state = match power.output_override {
        OutputOverride::Stopped => OutputPowerStatus::Stopped,
        _ if power.sleeping() => OutputPowerStatus::Paused,
        _ => OutputPowerStatus::Running,
    };
    OutputPowerResponse { state }
}

fn schedule_released_output_reconnect(
    state: &AppState,
    previous: crate::session::OutputPowerState,
) {
    match released_output_reconnect_scope(previous) {
        Some(OutputReconnectScope::All) => super::effects::schedule_all_output_reconnect(state),
        Some(OutputReconnectScope::Network) => {
            super::effects::schedule_network_output_reconnect(state);
        }
        None => {}
    }
}

fn released_output_reconnect_scope(
    previous: crate::session::OutputPowerState,
) -> Option<OutputReconnectScope> {
    if previous.session_release_active() {
        Some(OutputReconnectScope::All)
    } else if previous.output_override == OutputOverride::Stopped {
        Some(OutputReconnectScope::Network)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::session::OffOutputBehavior;

    use super::{OutputReconnectScope, released_output_reconnect_scope};
    use crate::session::{OutputOverride, OutputPowerState};

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
}
