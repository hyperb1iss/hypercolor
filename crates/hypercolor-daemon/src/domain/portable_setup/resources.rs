//! Installed resource availability never opens effect or media payloads.

use std::collections::HashSet;

use hypercolor_types::asset::AssetId;
use hypercolor_types::effect::EffectId;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::library::{PlaylistItemTarget, PresetId};
use hypercolor_types::portable_setup::{MissingSetupResources, PortableSetupV1};

use super::PortableSetupService;

impl PortableSetupService {
    /// Report references requiring local installation without reading their files.
    pub async fn missing_resources(&self, manifest: &PortableSetupV1) -> MissingSetupResources {
        let entries = self.state.domains.effects.portable_sources().await;
        let effects = entries.iter().map(|entry| entry.metadata.id).collect();
        let presets = entries
            .iter()
            .flat_map(|entry| entry.metadata.presets.iter().map(|preset| preset.id))
            .collect();
        let media = self
            .state
            .asset_library
            .read()
            .await
            .records()
            .iter()
            .map(|record| record.id)
            .collect();
        missing_resources(manifest, &effects, &media, &presets)
    }
}

pub(super) fn missing_resources(
    manifest: &PortableSetupV1,
    installed_effects: &HashSet<EffectId>,
    installed_media: &HashSet<AssetId>,
    installed_presets: &HashSet<PresetId>,
) -> MissingSetupResources {
    let mut effects = HashSet::new();
    let mut media = HashSet::new();
    let mut presets = HashSet::new();
    for scene in manifest.scenes.values() {
        for zone in &scene.zones {
            for layer in &zone.layers {
                match &layer.source {
                    LayerSource::Effect {
                        effect_id,
                        preset_id,
                        ..
                    } => {
                        effects.insert(*effect_id);
                        presets.extend(*preset_id);
                    }
                    LayerSource::Media { asset_id, .. } => {
                        media.insert(*asset_id);
                    }
                    _ => {}
                }
            }
        }
    }
    effects.extend(manifest.presets.values().map(|preset| preset.effect_id));
    effects.extend(manifest.display_faces.values().map(|face| face.effect_id));
    for playlist in manifest.playlists.values() {
        for item in &playlist.items {
            match item.target {
                PlaylistItemTarget::Effect { effect_id } => {
                    effects.insert(effect_id);
                }
                PlaylistItemTarget::Preset { preset_id } => {
                    presets.insert(preset_id);
                }
            }
        }
    }
    let mut result = MissingSetupResources {
        presets: presets
            .into_iter()
            .filter(|id| !manifest.presets.contains_key(&id.0) && !installed_presets.contains(id))
            .collect(),
        effects: effects
            .into_iter()
            .filter(|id| !installed_effects.contains(id))
            .collect(),
        media: media
            .into_iter()
            .filter(|id| !installed_media.contains(id))
            .collect(),
    };
    result.effects.sort_by_key(|id| id.0);
    result.media.sort_by_key(|id| id.0);
    result.presets.sort_by_key(|id| id.0);
    result
}
