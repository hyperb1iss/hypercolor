use std::sync::Arc;

use tracing::{info, warn};

use hypercolor_core::input::{ManagedSourceKey, ManagedSourceRole, SourceSwapTarget};

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

    let input_manager = state.input_manager();
    // Only the host hardware source is consent-gated; the browser injection
    // source is always registered and must survive enable/disable toggles.
    let had_source = input_manager.has_host_capture_source();
    let mut replacement = crate::startup::services::build_interaction_source(&input);

    // Rebuild on any change so keyboard/mouse toggles apply, not just enable
    // and disable.
    if let Some(source) = replacement.as_mut()
        && let Err(error) = source.start()
    {
        warn!(%error, "Failed to prepare live host input source");
        return route_changed;
    }
    let enabling = replacement.is_some();
    let target = if enabling {
        SourceSwapTarget::Present { running: true }
    } else {
        SourceSwapTarget::Absent
    };
    let Ok(plan) = input_manager.plan_source_swap(ManagedSourceKey::Interaction, target) else {
        warn!("Failed to plan live host input source swap");
        return route_changed;
    };
    let mut replacement = replacement.map(ManagedSourceRole::interaction);
    let Ok(mut prepared) = plan.prepare(&mut replacement) else {
        if let Some(source) = replacement.as_mut() {
            source.source_mut().stop();
        }
        warn!("Failed to prepare live host input source swap");
        return route_changed;
    };
    let retirement = match input_manager.commit_source_swap(&mut prepared) {
        Ok(retirement) => retirement,
        Err(error) => {
            prepared.discard();
            warn!(%error, "Failed to commit live host input source swap");
            return route_changed;
        }
    };
    prepared.discard();
    retirement.retire();
    if !enabling && had_source {
        info!("Disabled host input capture live");
        return true;
    }
    info!("Applied live host input capture config");
    true
}
