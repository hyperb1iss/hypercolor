use std::sync::Arc;

use tracing::{info, warn};

use hypercolor_core::engine::FpsTier;

use super::write_covers;
use crate::app_state::AppState;
use crate::scene_transactions::{PreparedLayoutUpdate, apply_prepared_layout_update_under_guard};

/// Apply render config changes live: FPS retune and canvas resize.
///
/// FPS changes go directly to the `RenderLoop`. Canvas dimension changes
/// are queued as an acknowledged layout transaction and take effect at the
/// next frame boundary without blocking the pipeline.
pub(super) async fn apply_render_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };

    let config = manager.get();
    let mut applied = false;

    if write_covers(key, "daemon.target_fps") {
        let tier = FpsTier::from_fps(config.daemon.target_fps);
        state.configured_max_fps_tier.set(tier);
        let mut loop_guard = state.render_loop.write().await;
        loop_guard.fps_controller_mut().set_max_tier(tier);
        loop_guard.set_tier(tier);
        info!(
            target_fps = config.daemon.target_fps,
            resolved_tier = %tier,
            "Applied live render FPS change"
        );
        applied = true;
    }

    if write_covers(key, "daemon.canvas_width") || write_covers(key, "daemon.canvas_height") {
        let resize_queued = sync_active_layout_canvas_size(
            state,
            config.daemon.canvas_width,
            config.daemon.canvas_height,
        )
        .await;
        info!(
            canvas_width = config.daemon.canvas_width,
            canvas_height = config.daemon.canvas_height,
            resize_queued,
            "Applied live canvas dimension config"
        );
        applied = true;
    }

    applied
}

pub(in crate::api::config) const fn canvas_dimensions_differ(
    current_width: u32,
    current_height: u32,
    next_width: u32,
    next_height: u32,
) -> bool {
    current_width != next_width || current_height != next_height
}

async fn sync_active_layout_canvas_size(state: &Arc<AppState>, width: u32, height: u32) -> bool {
    let state = Arc::clone(state);
    match tokio::spawn(sync_active_layout_canvas_size_workflow(
        state, width, height,
    ))
    .await
    {
        Ok(applied) => applied,
        Err(error) => {
            warn!(%error, width, height, "Live canvas dimension workflow failed");
            false
        }
    }
}

async fn sync_active_layout_canvas_size_workflow(
    state: Arc<AppState>,
    width: u32,
    height: u32,
) -> bool {
    #[cfg(feature = "persistence-test-hooks")]
    let mutation_reference = format!("{width}x{height}");
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::BeforeGuard,
            crate::api::layouts::LayoutMutationTestOperation::ConfigResize,
            &mutation_reference,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let updated_layout = {
        let spatial = state.spatial_engine.snapshot();
        let current = spatial.layout().as_ref().clone();
        if canvas_dimensions_differ(current.canvas_width, current.canvas_height, width, height) {
            let mut updated = current;
            updated.canvas_width = width;
            updated.canvas_height = height;
            Some(updated)
        } else {
            None
        }
    };

    let Some(updated_layout) = updated_layout else {
        return false;
    };

    let prepared = match PreparedLayoutUpdate::try_new(updated_layout.clone()) {
        Ok(prepared) => prepared,
        Err(error) => {
            warn!(%error, width, height, "Rejected live canvas dimension config");
            return false;
        }
    };
    if let Err(error) = apply_prepared_layout_update_under_guard(
        state.spatial_engine.clone(),
        state.scene_manager.clone(),
        state.scene_transactions.clone(),
        &guard,
        prepared,
    )
    .await
    {
        warn!(%error, width, height, "Rejected live canvas dimension config");
        return false;
    }

    let persisted_layout_updated = {
        let mut layouts = state.layouts.write().await;
        if let Some(saved_layout) = layouts.get_mut(&updated_layout.id) {
            saved_layout.canvas_width = width;
            saved_layout.canvas_height = height;
            true
        } else {
            false
        }
    };

    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::AfterMemoryMutation,
            crate::api::layouts::LayoutMutationTestOperation::ConfigResize,
            &mutation_reference,
        )
        .await;
    if persisted_layout_updated {
        crate::api::persist_layouts_best_effort(&state).await;
    }
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            crate::api::layouts::LayoutMutationTestPoint::AfterWorkflow,
            crate::api::layouts::LayoutMutationTestOperation::ConfigResize,
            &mutation_reference,
        )
        .await;
    drop(guard);

    true
}
