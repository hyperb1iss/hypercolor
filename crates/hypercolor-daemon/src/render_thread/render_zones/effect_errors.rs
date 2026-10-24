use anyhow::Error;
use hypercolor_core::effect::EffectRegistry;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::Zone;

use super::ZoneRuntime;
use super::model::ZoneEffectError;

impl ZoneRuntime {
    pub(crate) fn note_effect_error(&mut self, error: &ZoneEffectError) -> Option<ZoneEffectError> {
        self.recovered_effect_error = None;
        if self.last_effect_error.as_ref() == Some(error) {
            return None;
        }

        self.last_effect_error = Some(error.clone());
        Some(error.clone())
    }

    pub(super) fn clear_effect_error(&mut self) {
        if let Some(error) = self.last_effect_error.take() {
            self.recovered_effect_error = Some(error);
        }
    }

    pub(crate) fn take_recovered_effect_error(&mut self) -> Option<ZoneEffectError> {
        self.recovered_effect_error.take()
    }

    /// Forget the effect error state without reporting a recovery, for a
    /// scene that no longer renders any zone. The next failure, even the
    /// same one, publishes again.
    pub(crate) fn forget_effect_error(&mut self) {
        self.last_effect_error = None;
        self.recovered_effect_error = None;
    }

    /// Count a frame whose scene zones failed to render, `retained` saying
    /// whether it published the last good frame instead. Returns whether the
    /// failure is new and should be logged: the first since the scene last
    /// rendered, or one whose error or outcome differs from the one logged.
    /// A failure that persists frame after frame logs once.
    pub(crate) fn note_scene_render_failure(&mut self, error: &Error, retained: bool) -> bool {
        self.scene_render_failure
            .note((format!("{error:#}"), retained))
    }

    /// End the logged scene render failure, if one is open, returning how
    /// many frames failed since it was logged.
    pub(crate) fn clear_scene_render_failure(&mut self) -> Option<u64> {
        self.scene_render_failure.clear()
    }
}

pub(super) fn render_layer_effect_error(
    zone: &Zone,
    layer: &SceneLayer,
    registry: &EffectRegistry,
    error: Error,
) -> ZoneEffectError {
    let effect_id = match &layer.source {
        LayerSource::Effect { effect_id, .. } => effect_id.to_string(),
        LayerSource::WebViewport { url, .. } => format!("web_viewport:{url}"),
        _ => "unknown".to_owned(),
    };
    let effect_name = zone
        .layers
        .iter()
        .find(|candidate| candidate.id == layer.id)
        .and_then(|layer| match layer.source {
            LayerSource::Effect { effect_id, .. } => Some(effect_id),
            _ => None,
        })
        .and_then(|effect_id| {
            registry
                .get(&effect_id)
                .map(|entry| entry.metadata.name.clone())
        })
        .unwrap_or_else(|| effect_id.clone());

    ZoneEffectError {
        effect_id,
        effect_name,
        zone_id: zone.id,
        zone_name: zone.name.clone(),
        error: error.to_string(),
    }
}
