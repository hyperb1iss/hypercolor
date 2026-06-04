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
}

pub(super) fn render_layer_effect_error(
    group: &Zone,
    layer: &SceneLayer,
    registry: &EffectRegistry,
    error: Error,
) -> ZoneEffectError {
    let effect_id = match &layer.source {
        LayerSource::Effect { effect_id, .. } => effect_id.to_string(),
        LayerSource::WebViewport { url, .. } => format!("web_viewport:{url}"),
        _ => "unknown".to_owned(),
    };
    let effect_name = group
        .effective_layers()
        .into_iter()
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
        group_id: group.id,
        group_name: group.name.clone(),
        error: error.to_string(),
    }
}
