use std::sync::Arc;

use anyhow::Context;
use tracing::{info, warn};

use hypercolor_core::input::AudioReconfigurationConflict;
use hypercolor_types::audio::AudioPipelineConfig;

use crate::app_state::AppState;

pub(super) async fn apply_audio_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    info!(
        key = key.unwrap_or("<all>"),
        "Applying live audio config change"
    );

    match reconfigure_input_manager(state).await {
        Ok(()) => true,
        Err(error) => {
            warn!(
                key = key.unwrap_or("<all>"),
                %error,
                "Failed to apply live audio config; change will take effect after daemon restart"
            );
            false
        }
    }
}

async fn reconfigure_input_manager(state: &Arc<AppState>) -> anyhow::Result<()> {
    let Some(manager) = state.config_manager.as_ref() else {
        return Ok(());
    };

    let mut conflict_count = 0_u64;
    loop {
        let latest_config = Arc::clone(&manager.get());
        let capture_active = current_live_audio_capture_demand(state).await;
        let audio_device = latest_config.audio.device.clone();
        let audio_name = format!("AudioInput({audio_device})");
        let effective_config = AudioPipelineConfig::from(&latest_config.audio);
        let (plan, previous_sources) = {
            let input_manager = state.input_manager().lock().await;
            (
                input_manager.plan_audio_runtime_config(
                    latest_config.audio.enabled,
                    &effective_config,
                    &audio_name,
                    capture_active,
                )?,
                input_manager.source_names(),
            )
        };
        let replacement_sources = if latest_config.audio.enabled {
            vec![audio_name]
        } else {
            Vec::new()
        };

        info!(
            audio_enabled = latest_config.audio.enabled,
            audio_device = %audio_device,
            capture_active,
            conflict_count,
            previous_sources = ?previous_sources,
            replacement_sources = ?replacement_sources,
            "Applying targeted live audio config change"
        );

        let mut prepared = tokio::task::spawn_blocking(move || plan.prepare())
            .await
            .context("audio reconfiguration preparation task failed")??;
        if !manager.is_current(&latest_config) {
            anyhow::bail!("audio config changed while live reconfiguration was prepared");
        }
        let mut input_manager = state.input_manager().lock().await;
        match input_manager.commit_audio_runtime_config(&mut prepared) {
            Ok(retirement) => {
                let sources = input_manager.source_names();
                drop(input_manager);
                drop(prepared);
                retirement.retire();
                info!(
                    audio_device = %audio_device,
                    conflict_count,
                    sources = ?sources,
                    "Live audio config change applied"
                );
                return Ok(());
            }
            Err(error)
                if error
                    .downcast_ref::<AudioReconfigurationConflict>()
                    .is_some()
                    && manager.is_current(&latest_config) =>
            {
                conflict_count = conflict_count.saturating_add(1);
                drop(input_manager);
                drop(prepared);
            }
            Err(error) => {
                drop(input_manager);
                drop(prepared);
                return Err(error);
            }
        }
    }
}

async fn current_live_audio_capture_demand(state: &Arc<AppState>) -> bool {
    let power_state = state.output_power.snapshot();
    if power_state.sleeping() {
        return false;
    }

    let active_effect_ids = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager
            .resolved_zones()
            .iter()
            .filter(|zone| zone.enabled)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .collect::<Vec<_>>()
    };
    if active_effect_ids.is_empty() {
        return false;
    }

    state
        .domains
        .effects
        .any_audio_reactive(active_effect_ids)
        .await
}
