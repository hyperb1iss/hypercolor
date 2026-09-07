//! REST adapter for daemon diagnostics.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use axum::{Extension, Json};

use crate::api::capture::protected_control_rejection;
use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::domain::diagnostics::default_safe_checks;

pub use hypercolor_types::api::diagnose::DiagnoseRequest;
pub(crate) use hypercolor_types::api::diagnose::DiagnoseResponse;

/// `POST /api/v1/diagnose` runs lightweight daemon diagnostics.
pub(crate) async fn run_diagnostics(
    State(state): State<Arc<AppState>>,
    Extension(auth_context): Extension<RequestAuthContext>,
    body: Option<Json<DiagnoseRequest>>,
) -> Response {
    let requested = body
        .as_ref()
        .and_then(|request| request.checks.as_ref())
        .cloned()
        .unwrap_or_else(default_safe_checks);

    if requested.iter().any(|check| check == "macos_screen_parity")
        && let Some(rejection) = protected_control_rejection(auth_context)
    {
        return rejection;
    }

    let include_system = body
        .as_ref()
        .and_then(|request| request.system)
        .unwrap_or(false);
    envelope::ok(
        state
            .domains
            .diagnostics
            .collect(&requested, include_system)
            .await,
    )
}
