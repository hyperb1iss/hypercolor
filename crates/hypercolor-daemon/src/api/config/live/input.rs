use std::sync::Arc;

use tracing::{info, warn};

use crate::app_state::AppState;

/// Apply host-input config changes live.
///
/// Enable/disable adds or removes the interaction source on the running
/// input manager. Activation converges on the next frame through the
/// uncached interaction demand reconcile, so a source added while an
/// interactive effect is already running starts capturing immediately.
pub(in crate::api::config) async fn apply_input_config_change(
    state: &Arc<AppState>,
    key: Option<&str>,
) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };

    let input = manager.get().input.clone();
    let route_snapshot = state.interaction_routing.snapshot();
    let route_changed = route_snapshot.daemon_policy != input.daemon_route
        || route_snapshot.preview_policy != input.preview_route;
    if route_changed {
        state.interaction_routing.publish_policies(
            route_snapshot
                .config_generation
                .checked_add(1)
                .expect("interaction route config generation exhausted"),
            input.daemon_route,
            input.preview_route,
        );
    }
    if matches!(key, Some("input.daemon_route" | "input.preview_route")) {
        return route_changed;
    }

    let mut input_manager = state.input_manager().lock().await;
    // Only the host hardware source is consent-gated; the browser injection
    // source is always registered and must survive enable/disable toggles.
    let had_source = input_manager.has_host_capture_source();
    let replacement = crate::startup::services::build_interaction_source(&input);

    // Rebuild on any change so keyboard/mouse toggles apply, not just enable
    // and disable.
    input_manager.remove_host_capture_sources();
    let Some(mut source) = replacement else {
        if had_source {
            info!("Disabled host input capture live");
        }
        return had_source || route_changed;
    };

    if let Err(error) = source.start() {
        warn!(%error, "Failed to start live host input source");
        return had_source || route_changed;
    }
    input_manager.add_source(source);
    info!("Applied live host input capture config");
    true
}
