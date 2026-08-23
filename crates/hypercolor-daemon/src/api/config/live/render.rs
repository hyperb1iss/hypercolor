use std::sync::Arc;

use tracing::{info, warn};

use hypercolor_core::engine::FpsTier;

use super::write_covers;
use crate::app_state::AppState;

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
    match state
        .domains
        .layout
        .resize_active_canvas(width, height)
        .await
    {
        Ok(applied) => applied,
        Err(error) => {
            warn!(%error, width, height, "Rejected live canvas dimension config");
            false
        }
    }
}
