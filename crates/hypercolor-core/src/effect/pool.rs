mod control_sync;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::control::{
    ControlDeltaBatch, ControlId, ControlSet, ControlValue, SetRevision,
};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::effect::{ControlBinding, EffectId, EffectMetadata};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Zone, ZoneId};
use hypercolor_types::sensor::SystemSnapshot;
#[cfg(feature = "servo")]
use hypercolor_types::viewport::FitMode;
use tokio::sync::RwLock;

use super::factory::create_renderer_for_metadata;
use super::registry::{EffectEntry, EffectRegistry};
use super::traits::{
    EffectRenderOutput, EffectRenderer, FrameDataSources, FrameInput, prepare_target_canvas,
};
use crate::asset::AssetLibrary;
use crate::input::{InteractionData, ScreenData};

use self::control_sync::{ActiveBindingState, apply_sensor_bindings, canonical_control_set};

pub struct EffectPool {
    slots: HashMap<EffectSlotKey, EffectSlot>,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
    generation: u64,
}

/// Effect-pool changes with allocation and control validation completed.
pub struct PreparedEffectPoolReconcile {
    slots: HashMap<EffectSlotKey, EffectSlot>,
    reused_keys: Vec<EffectSlotKey>,
    control_updates: Vec<PreparedControlUpdate>,
    source_generation: u64,
}

struct PreparedControlUpdate {
    key: EffectSlotKey,
    state: PreparedLayerState,
}

struct PreparedLayerState {
    controls: ControlSet,
    control_bindings: HashMap<String, ControlBinding>,
    changed_bindings: Vec<String>,
    changes: Vec<(ControlId, ControlValue)>,
    revision: SetRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectSlotKey {
    pub zone_id: ZoneId,
    pub layer_id: SceneLayerId,
}

#[derive(Debug, Clone, PartialEq)]
struct LayerEffectSource {
    effect_id: EffectId,
    controls: HashMap<String, ControlValue>,
    control_bindings: HashMap<String, ControlBinding>,
}

impl EffectSlotKey {
    #[must_use]
    pub const fn new(zone_id: ZoneId, layer_id: SceneLayerId) -> Self {
        Self { zone_id, layer_id }
    }
}

impl EffectPool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            asset_library: None,
            generation: 0,
        }
    }

    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Provide the asset library used by asset-backed effect renderers.
    pub fn set_asset_library(&mut self, asset_library: Arc<RwLock<AssetLibrary>>) {
        let next_generation = self.next_generation();
        self.asset_library = Some(asset_library);
        self.generation = next_generation;
    }

    pub fn reconcile(
        &mut self,
        zones: &[Zone],
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
    ) -> Result<()> {
        let prepared = self.prepare_reconcile(zones, registry, display_descriptors)?;
        self.commit_reconcile(prepared)
    }

    /// Construct every replacement renderer before changing the live pool.
    ///
    /// # Errors
    ///
    /// Returns an error if an effect is unavailable or its replacement renderer
    /// cannot be initialized. The live pool is unchanged on failure.
    pub fn prepare_reconcile(
        &self,
        zones: &[Zone],
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
    ) -> Result<PreparedEffectPoolReconcile> {
        let desired_layers = desired_effect_layers(zones);
        let mut slots = HashMap::new();
        slots.try_reserve(desired_layers.len())?;
        let mut reused_keys = Vec::new();
        reused_keys.try_reserve(desired_layers.len())?;
        let mut control_updates = Vec::new();
        control_updates.try_reserve(desired_layers.len())?;

        for (zone, layer) in desired_layers {
            let Some(source) = layer_effect_source(&layer) else {
                continue;
            };
            let key = EffectSlotKey::new(zone.id, layer.id);

            let entry = lookup_effect_entry(registry, source.effect_id)?;

            let display_descriptor = zone
                .display_target
                .as_ref()
                .and_then(|_| display_descriptors.get(&zone.id));
            let needs_replacement = self.slots.get(&key).is_none_or(|slot| {
                slot.needs_rebuild(
                    source.effect_id,
                    entry,
                    display_descriptor,
                    zone.layout.canvas_width,
                    zone.layout.canvas_height,
                )
            });
            if needs_replacement {
                let slot = EffectSlot::build(
                    entry,
                    zone,
                    source,
                    self.asset_library.as_ref(),
                    display_descriptor.cloned(),
                )?;
                slots.insert(key, slot);
            } else {
                let slot = self
                    .slots
                    .get(&key)
                    .expect("replacement check requires an existing effect slot");
                let revision = SetRevision::new(zone.controls_version);
                if slot.controls.set_revision() != revision
                    || slot.control_bindings != source.control_bindings
                {
                    control_updates.push(PreparedControlUpdate {
                        key,
                        state: slot.prepare_layer_state(source, revision)?,
                    });
                }
                reused_keys.push(key);
            }
        }

        Ok(PreparedEffectPoolReconcile {
            slots,
            reused_keys,
            control_updates,
            source_generation: self.generation,
        })
    }

    /// Commit a previously prepared reconciliation.
    ///
    /// The prepared value is tied to the pool state that produced it. Commit
    /// validates that invariant before changing any live renderer or slot.
    ///
    /// # Errors
    ///
    /// Returns an error when a live renderer rejects both an incremental
    /// update and the authoritative snapshot replay used to recover it.
    pub fn commit_reconcile(&mut self, prepared: PreparedEffectPoolReconcile) -> Result<()> {
        let PreparedEffectPoolReconcile {
            mut slots,
            reused_keys,
            control_updates,
            source_generation,
        } = prepared;
        assert_eq!(
            self.generation, source_generation,
            "prepared effect pool must commit against its source generation"
        );
        let next_generation = self.next_generation();
        assert!(
            reused_keys.iter().all(|key| self.slots.contains_key(key))
                && control_updates
                    .iter()
                    .all(|update| self.slots.contains_key(&update.key)),
            "prepared effect pool must commit against its source state"
        );
        for update in control_updates {
            let slot = self
                .slots
                .get_mut(&update.key)
                .expect("prepared effect slot must remain live until commit");
            slot.commit_prepared_layer_state(update.state)?;
        }
        let mut live_slots = std::mem::take(&mut self.slots);
        for key in reused_keys {
            let slot = live_slots
                .remove(&key)
                .expect("prepared effect slot was validated before commit");
            slots.insert(key, slot);
        }
        self.slots = slots;
        self.generation = next_generation;
        Ok(())
    }

    pub fn clear(&mut self) {
        if !self.slots.is_empty() {
            let next_generation = self.next_generation();
            self.slots.clear();
            self.generation = next_generation;
        }
    }

    pub fn remove_zone(&mut self, zone_id: ZoneId) {
        if self.slots.keys().any(|key| key.zone_id == zone_id) {
            let next_generation = self.next_generation();
            self.slots.retain(|key, _| key.zone_id != zone_id);
            self.generation = next_generation;
        }
    }

    fn next_generation(&self) -> u64 {
        self.generation
            .checked_add(1)
            .expect("effect pool generation overflowed")
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input plus a mutable target canvas"
    )]
    pub fn render_zone_into(
        &mut self,
        zone: &Zone,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
        target: &mut Canvas,
    ) -> Result<()> {
        let Some(layer) = single_enabled_effect_layer(zone)? else {
            target.clear();
            return Ok(());
        };
        self.render_layer_into(
            zone,
            &layer,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            target,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input plus a mutable target canvas"
    )]
    pub fn render_layer_into(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
        target: &mut Canvas,
    ) -> Result<()> {
        prepare_target_canvas(target, zone.layout.canvas_width, zone.layout.canvas_height);

        if !zone.enabled || !layer.enabled || layer_effect_source(layer).is_none() {
            target.clear();
            return Ok(());
        }

        let key = EffectSlotKey::new(zone.id, layer.id);
        let slot = self.slots.get_mut(&key).ok_or_else(|| {
            anyhow!(
                "zone '{}' layer '{}' is not reconciled before advancing",
                zone.name,
                layer.id
            )
        })?;
        slot.render_into(
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
            target,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input for output-capable renderers"
    )]
    pub fn render_zone_output(
        &mut self,
        zone: &Zone,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
    ) -> Result<EffectRenderOutput> {
        let Some(layer) = single_enabled_effect_layer(zone)? else {
            return Ok(EffectRenderOutput::Cpu(Canvas::new(
                zone.layout.canvas_width,
                zone.layout.canvas_height,
            )));
        };
        self.render_layer_output(
            zone,
            &layer,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            sources,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input for output-capable renderers"
    )]
    pub fn render_layer_output(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
    ) -> Result<EffectRenderOutput> {
        if !zone.enabled || !layer.enabled || layer_effect_source(layer).is_none() {
            return Ok(EffectRenderOutput::Cpu(Canvas::new(
                zone.layout.canvas_width,
                zone.layout.canvas_height,
            )));
        }

        let key = EffectSlotKey::new(zone.id, layer.id);
        let slot = self.slots.get_mut(&key).ok_or_else(|| {
            anyhow!(
                "zone '{}' layer '{}' is not reconciled before rendering",
                zone.name,
                layer.id
            )
        })?;
        slot.render_output(
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "advancing output-capable renderers needs the full frame input"
    )]
    pub fn advance_layer_output(
        &mut self,
        zone: &Zone,
        layer: &SceneLayer,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
    ) -> Result<()> {
        if !zone.enabled || !layer.enabled || layer_effect_source(layer).is_none() {
            return Ok(());
        }

        let key = EffectSlotKey::new(zone.id, layer.id);
        let slot = self.slots.get_mut(&key).ok_or_else(|| {
            anyhow!(
                "zone '{}' layer '{}' is not reconciled before rendering",
                zone.name,
                layer.id
            )
        })?;
        slot.advance_output(
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )
    }
}

impl Default for EffectPool {
    fn default() -> Self {
        Self::new()
    }
}

struct EffectSlot {
    effect_id: EffectId,
    registry_metadata: EffectMetadata,
    registry_source_path: PathBuf,
    registry_modified: SystemTime,
    metadata: EffectMetadata,
    display_descriptor: Option<DisplayDescriptor>,
    canvas_width: u32,
    canvas_height: u32,
    renderer: Box<dyn EffectRenderer>,
    controls: ControlSet,
    control_bindings: HashMap<String, ControlBinding>,
    controls_initialized: bool,
    binding_state: HashMap<String, ActiveBindingState>,
    resolution_seq: u64,
    elapsed: Duration,
    frame_number: u64,
}

impl EffectSlot {
    fn build(
        entry: &EffectEntry,
        zone: &Zone,
        layer_source: LayerEffectSource,
        asset_library: Option<&Arc<RwLock<AssetLibrary>>>,
        display_descriptor: Option<DisplayDescriptor>,
    ) -> Result<Self> {
        let mut renderer = create_renderer_for_metadata(&entry.metadata)?;
        if let Some(asset_library) = asset_library {
            renderer.bind_asset_library(Arc::clone(asset_library));
        }
        if display_descriptor.is_some() {
            renderer.set_display_descriptor(display_descriptor.clone());
        }
        renderer.init_with_canvas_size(
            &entry.metadata,
            zone.layout.canvas_width,
            zone.layout.canvas_height,
        )?;

        let mut slot = Self {
            effect_id: entry.metadata.id,
            registry_metadata: entry.metadata.clone(),
            registry_source_path: entry.source_path.clone(),
            registry_modified: entry.modified,
            metadata: entry.metadata.clone(),
            display_descriptor,
            canvas_width: zone.layout.canvas_width,
            canvas_height: zone.layout.canvas_height,
            renderer,
            controls: ControlSet::new(SetRevision::new(zone.controls_version)),
            control_bindings: HashMap::new(),
            controls_initialized: false,
            binding_state: HashMap::new(),
            resolution_seq: 0,
            elapsed: Duration::ZERO,
            frame_number: 0,
        };
        slot.sync_layer_state(layer_source, SetRevision::new(zone.controls_version))?;
        Ok(slot)
    }

    fn needs_rebuild(
        &self,
        effect_id: EffectId,
        entry: &EffectEntry,
        display_descriptor: Option<&DisplayDescriptor>,
        canvas_width: u32,
        canvas_height: u32,
    ) -> bool {
        self.effect_id != effect_id
            || self.registry_metadata != entry.metadata
            || self.registry_source_path != entry.source_path
            || self.registry_modified != entry.modified
            || self.display_descriptor.as_ref() != display_descriptor
            || self.canvas_width != canvas_width
            || self.canvas_height != canvas_height
    }

    fn sync_layer_state(&mut self, source: LayerEffectSource, revision: SetRevision) -> Result<()> {
        let prepared = self.prepare_layer_state(source, revision)?;
        self.commit_prepared_layer_state(prepared)
    }

    fn prepare_layer_state(
        &self,
        source: LayerEffectSource,
        revision: SetRevision,
    ) -> Result<PreparedLayerState> {
        let controls = canonical_control_set(revision, &self.metadata, &source)?;
        let changed_bindings = self
            .control_bindings
            .keys()
            .chain(source.control_bindings.keys())
            .filter(|control_id| {
                self.control_bindings.get(*control_id) != source.control_bindings.get(*control_id)
            })
            .cloned()
            .collect::<std::collections::HashSet<_>>();

        let changes = if self.controls_initialized {
            controls
                .iter()
                .filter_map(|(control_id, authored_value)| {
                    let control_id_text = control_id.as_str();
                    let binding_changed = changed_bindings.contains(control_id_text);
                    let previous_value = self
                        .binding_state
                        .get(control_id_text)
                        .filter(|_| self.control_bindings.contains_key(control_id_text))
                        .map_or_else(
                            || self.controls.get(control_id_text),
                            |state| Some(&state.control_value),
                        );
                    let desired_value = self
                        .binding_state
                        .get(control_id_text)
                        .filter(|_| {
                            !binding_changed
                                && source.control_bindings.contains_key(control_id_text)
                        })
                        .map_or(authored_value, |state| &state.control_value);
                    (previous_value != Some(desired_value))
                        .then(|| (control_id.clone(), desired_value.clone()))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut changed_bindings = changed_bindings.into_iter().collect::<Vec<_>>();
        changed_bindings.sort_unstable();

        Ok(PreparedLayerState {
            controls,
            control_bindings: source.control_bindings,
            changed_bindings,
            changes,
            revision,
        })
    }

    fn commit_prepared_layer_state(&mut self, prepared: PreparedLayerState) -> Result<()> {
        if self.controls_initialized {
            if !prepared.changes.is_empty() {
                let batch = ControlDeltaBatch::new(prepared.revision, 0, &prepared.changes);
                if let Err(delta_error) = self.renderer.apply_controls(&batch) {
                    self.controls_initialized = false;
                    let snapshot = self.resolved_prepared_snapshot(&prepared)?;
                    self.renderer
                        .initialize_controls(&snapshot)
                        .with_context(|| {
                            format!(
                                "renderer rejected control delta ({delta_error}) and snapshot replay"
                            )
                        })?;
                }
            }
        } else {
            let snapshot = self.resolved_prepared_snapshot(&prepared)?;
            self.renderer
                .initialize_controls(&snapshot)
                .context("renderer rejected authoritative control snapshot")?;
        }

        for control_id in prepared.changed_bindings {
            self.binding_state.remove(&control_id);
        }
        self.controls = prepared.controls;
        self.control_bindings = prepared.control_bindings;
        self.controls_initialized = true;
        self.resolution_seq = 0;
        Ok(())
    }

    fn resolved_prepared_snapshot(&self, prepared: &PreparedLayerState) -> Result<ControlSet> {
        let mut snapshot = prepared.controls.clone();
        for (control_id, state) in &self.binding_state {
            if prepared.control_bindings.contains_key(control_id)
                && !prepared.changed_bindings.contains(control_id)
            {
                snapshot.insert(
                    ControlId::from(control_id.as_str()),
                    state.control_value.clone(),
                )?;
            }
        }
        Ok(snapshot)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input plus a mutable target canvas"
    )]
    fn render_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
        canvas_width: u32,
        canvas_height: u32,
        target: &mut Canvas,
    ) -> Result<()> {
        let time_secs = self.advance_elapsed(delta_secs);
        apply_sensor_bindings(
            self.renderer.as_mut(),
            &self.metadata,
            &self.control_bindings,
            &self.controls,
            &mut self.binding_state,
            &mut self.resolution_seq,
            &mut self.controls_initialized,
            sensors,
        )?;
        let input = FrameInput {
            time_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            canvas_width,
            canvas_height,
        };
        self.renderer.render_into(&input, target)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input for output-capable renderers"
    )]
    fn render_output(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<EffectRenderOutput> {
        let time_secs = self.advance_elapsed(delta_secs);
        apply_sensor_bindings(
            self.renderer.as_mut(),
            &self.metadata,
            &self.control_bindings,
            &self.controls,
            &mut self.binding_state,
            &mut self.resolution_seq,
            &mut self.controls_initialized,
            sensors,
        )?;
        let input = FrameInput {
            time_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            canvas_width,
            canvas_height,
        };
        let output = self.renderer.render_output(&input)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(output)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "advancing output-capable renderers needs the full frame input"
    )]
    fn advance_output(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        sources: FrameDataSources<'_>,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<()> {
        let time_secs = self.advance_elapsed(delta_secs);
        apply_sensor_bindings(
            self.renderer.as_mut(),
            &self.metadata,
            &self.control_bindings,
            &self.controls,
            &mut self.binding_state,
            &mut self.resolution_seq,
            &mut self.controls_initialized,
            sensors,
        )?;
        let input = FrameInput {
            time_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            sources,
            canvas_width,
            canvas_height,
        };
        self.renderer.advance_output(&input)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(())
    }

    fn advance_elapsed(&mut self, delta_secs: f32) -> f64 {
        let delta = Duration::try_from_secs_f32(delta_secs).unwrap_or_default();
        self.elapsed = self.elapsed.saturating_add(delta);
        self.elapsed.as_secs_f64()
    }
}

impl Drop for EffectSlot {
    fn drop(&mut self) {
        self.renderer.destroy();
    }
}

fn lookup_effect_entry(registry: &EffectRegistry, effect_id: EffectId) -> Result<&EffectEntry> {
    registry
        .get(&effect_id)
        .ok_or_else(|| anyhow!("effect '{effect_id}' is not registered"))
}

fn desired_effect_layers(zones: &[Zone]) -> Vec<(&Zone, SceneLayer)> {
    zones
        .iter()
        .filter(|zone| zone.enabled)
        .flat_map(|zone| {
            zone.layers
                .clone()
                .into_iter()
                .filter(|layer| layer.enabled && layer_effect_source(layer).is_some())
                .map(move |layer| (zone, layer))
        })
        .collect()
}

fn single_enabled_effect_layer(zone: &Zone) -> Result<Option<SceneLayer>> {
    if !zone.enabled {
        return Ok(None);
    }

    let mut layers = zone
        .layers
        .clone()
        .into_iter()
        .filter(|layer| layer.enabled && layer_effect_source(layer).is_some());
    let Some(layer) = layers.next() else {
        return Ok(None);
    };
    if layers.next().is_some() {
        return Err(anyhow!(
            "zone '{}' has multiple enabled effect layers; render layers explicitly",
            zone.name
        ));
    }
    Ok(Some(layer))
}

fn layer_effect_source(layer: &SceneLayer) -> Option<LayerEffectSource> {
    match &layer.source {
        LayerSource::Effect {
            effect_id,
            controls,
            control_bindings,
            ..
        } => Some(LayerEffectSource {
            effect_id: *effect_id,
            controls: controls.clone(),
            control_bindings: control_bindings.clone(),
        }),
        #[cfg(feature = "servo")]
        LayerSource::WebViewport {
            url,
            viewport,
            render,
        } => Some(LayerEffectSource {
            effect_id: crate::effect::builtin::builtin_effect_stable_id("web_viewport"),
            controls: web_viewport_controls(url, *viewport, *render),
            control_bindings: HashMap::new(),
        }),
        LayerSource::Media { .. }
        | LayerSource::ScreenRegion { .. }
        | LayerSource::ColorFill { .. } => None,
        #[cfg(not(feature = "servo"))]
        LayerSource::WebViewport { .. } => None,
    }
}

#[cfg(feature = "servo")]
fn web_viewport_controls(
    url: &str,
    viewport: hypercolor_types::viewport::ViewportRect,
    render: hypercolor_types::layer::WebViewportRender,
) -> HashMap<String, ControlValue> {
    HashMap::from([
        ("url".to_owned(), ControlValue::Text(url.to_owned())),
        ("viewport".to_owned(), ControlValue::rect(viewport)),
        (
            "fit_mode".to_owned(),
            ControlValue::Enum(fit_mode_control_value(FitMode::Cover).to_owned()),
        ),
        (
            "refresh_interval".to_owned(),
            ControlValue::Float(match render {
                hypercolor_types::layer::WebViewportRender::Live => 0.0,
                hypercolor_types::layer::WebViewportRender::Snapshot => 300.0,
            }),
        ),
    ])
}

#[cfg(feature = "servo")]
const fn fit_mode_control_value(fit: FitMode) -> &'static str {
    match fit {
        FitMode::Contain => "Contain",
        FitMode::Cover => "Cover",
        FitMode::Stretch => "Stretch",
        FitMode::Tile | FitMode::Mirror => "Cover",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use anyhow::Result;

    #[cfg(feature = "servo")]
    use super::layer_effect_source;
    use super::{EffectPool, EffectSlot, EffectSlotKey};
    use crate::effect::builtin::register_builtin_effects;
    use crate::effect::registry::EffectRegistry;
    use crate::effect::traits::{EffectRenderer, FrameDataSources, FrameInput};
    use crate::input::InteractionData;
    use hypercolor_types::audio::AudioData;
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::control::{
        ControlDeltaBatch, ControlId, ControlSet, ControlValue, SetRevision,
    };
    use hypercolor_types::effect::{
        ControlBinding, ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId,
        EffectMetadata, EffectSource,
    };
    #[cfg(feature = "servo")]
    use hypercolor_types::layer::{BlendMode, LayerAdjust, LayerTransform};
    use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
    use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
        StripDirection,
    };

    struct DestroySpyRenderer {
        destroyed: Arc<AtomicBool>,
    }

    struct AdvanceSpyRenderer {
        advanced: Arc<AtomicU64>,
    }

    struct ControlLifecycleSpyRenderer {
        initialized: Arc<Mutex<Vec<ControlSet>>>,
        applied: SharedControlBatchLog,
        fail_initialize: Arc<AtomicBool>,
        fail_delta: Arc<AtomicBool>,
    }

    type RecordedControlBatch = Vec<(String, ControlValue)>;
    type SharedControlBatchLog = Arc<Mutex<Vec<RecordedControlBatch>>>;

    #[derive(Default)]
    struct ControlSpyRenderer {
        applied: Vec<(String, ControlValue)>,
    }

    impl DestroySpyRenderer {
        fn new(destroyed: Arc<AtomicBool>) -> Self {
            Self { destroyed }
        }
    }

    impl EffectRenderer for DestroySpyRenderer {
        fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
            Ok(())
        }

        fn render_into(&mut self, _input: &FrameInput<'_>, _target: &mut Canvas) -> Result<()> {
            Ok(())
        }

        fn apply_controls(&mut self, _batch: &ControlDeltaBatch<'_>) -> Result<()> {
            Ok(())
        }

        fn destroy(&mut self) {
            self.destroyed.store(true, Ordering::SeqCst);
        }
    }

    impl EffectRenderer for AdvanceSpyRenderer {
        fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
            Ok(())
        }

        fn render_into(&mut self, _input: &FrameInput<'_>, _target: &mut Canvas) -> Result<()> {
            Ok(())
        }

        fn advance_output(&mut self, input: &FrameInput<'_>) -> Result<()> {
            assert_eq!(input.canvas_width, 32);
            assert_eq!(input.canvas_height, 16);
            self.advanced.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn apply_controls(&mut self, _batch: &ControlDeltaBatch<'_>) -> Result<()> {
            Ok(())
        }

        fn destroy(&mut self) {}
    }

    impl EffectRenderer for ControlSpyRenderer {
        fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
            Ok(())
        }

        fn render_into(&mut self, _input: &FrameInput<'_>, _target: &mut Canvas) -> Result<()> {
            Ok(())
        }

        fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> Result<()> {
            self.applied.extend(
                batch
                    .changes
                    .iter()
                    .map(|(control_id, value)| (control_id.to_string(), value.clone())),
            );
            Ok(())
        }

        fn destroy(&mut self) {}
    }

    impl EffectRenderer for ControlLifecycleSpyRenderer {
        fn init(&mut self, _metadata: &EffectMetadata) -> Result<()> {
            Ok(())
        }

        fn render_into(&mut self, _input: &FrameInput<'_>, _target: &mut Canvas) -> Result<()> {
            Ok(())
        }

        fn initialize_controls(&mut self, controls: &ControlSet) -> Result<()> {
            self.initialized
                .lock()
                .expect("control initialization log should be available")
                .push(controls.clone());
            if self.fail_initialize.swap(false, Ordering::SeqCst) {
                anyhow::bail!("injected snapshot rejection");
            }
            Ok(())
        }

        fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> Result<()> {
            self.applied
                .lock()
                .expect("control delta log should be available")
                .push(
                    batch
                        .changes
                        .iter()
                        .map(|(control_id, value)| (control_id.to_string(), value.clone()))
                        .collect(),
                );
            if self.fail_delta.swap(false, Ordering::SeqCst) {
                anyhow::bail!("injected delta rejection");
            }
            Ok(())
        }

        fn destroy(&mut self) {}
    }

    fn sample_layout() -> SpatialLayout {
        SpatialLayout {
            id: "pool-drop-test".into(),
            name: "Pool Drop Test".into(),
            description: None,
            canvas_width: 32,
            canvas_height: 16,
            zones: vec![Output {
                id: "desk:main".into(),
                name: "Desk".into(),
                device_id: "mock:device".into(),
                zone_name: None,
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                display_order: 0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: 1,
                    direction: StripDirection::LeftToRight,
                },
                led_positions: Vec::new(),
                led_mapping: None,
                sampling_mode: Some(SamplingMode::Bilinear),
                edge_behavior: Some(EdgeBehavior::Clamp),
                shape: None,
                shape_preset: None,
                attachment: None,
                brightness: None,
            }],
            default_sampling_mode: SamplingMode::Bilinear,
            default_edge_behavior: EdgeBehavior::Clamp,
            version: 1,
        }
    }

    fn spy_metadata(effect_id: EffectId) -> EffectMetadata {
        EffectMetadata {
            id: effect_id,
            name: "Destroy Spy".into(),
            author: "hypercolor-test".into(),
            version: "0.1.0".into(),
            description: "Destroy spy effect".into(),
            category: EffectCategory::Utility,
            tags: vec!["test".into()],
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            input_reactive: false,
            source: EffectSource::Native {
                path: "mock/destroy-spy.wgsl".into(),
            },
            license: Some("Apache-2.0".into()),
        }
    }

    fn number_control(id: &str, default_value: f64) -> ControlDefinition {
        ControlDefinition {
            id: id.into(),
            name: id.into(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(default_value),
            min: Some(0.0),
            max: Some(10.0),
            step: Some(1.0),
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }
    }

    fn lifecycle_slot(
        metadata: EffectMetadata,
        controls: ControlSet,
        initialized: Arc<Mutex<Vec<ControlSet>>>,
        applied: SharedControlBatchLog,
        fail_initialize: Arc<AtomicBool>,
        fail_delta: Arc<AtomicBool>,
    ) -> EffectSlot {
        EffectSlot {
            effect_id: metadata.id,
            registry_metadata: metadata.clone(),
            registry_source_path: PathBuf::from("mock/control-lifecycle-spy.wgsl"),
            registry_modified: SystemTime::UNIX_EPOCH,
            metadata,
            display_descriptor: None,
            canvas_width: 1,
            canvas_height: 1,
            renderer: Box::new(ControlLifecycleSpyRenderer {
                initialized,
                applied,
                fail_initialize,
                fail_delta,
            }),
            controls,
            control_bindings: HashMap::new(),
            controls_initialized: true,
            binding_state: HashMap::new(),
            resolution_seq: 0,
            elapsed: Duration::ZERO,
            frame_number: 0,
        }
    }

    fn spy_slot(effect_id: EffectId, destroyed: Arc<AtomicBool>) -> EffectSlot {
        let registry_metadata = spy_metadata(effect_id);
        EffectSlot {
            effect_id,
            registry_metadata: registry_metadata.clone(),
            registry_source_path: PathBuf::from("mock/destroy-spy.wgsl"),
            registry_modified: SystemTime::UNIX_EPOCH,
            metadata: registry_metadata,
            display_descriptor: None,
            canvas_width: 1,
            canvas_height: 1,
            renderer: Box::new(DestroySpyRenderer::new(destroyed)),
            controls: ControlSet::new(SetRevision::default()),
            control_bindings: HashMap::new(),
            controls_initialized: true,
            binding_state: HashMap::new(),
            resolution_seq: 0,
            elapsed: Duration::ZERO,
            frame_number: 0,
        }
    }

    fn advance_spy_slot(effect_id: EffectId, advanced: Arc<AtomicU64>) -> EffectSlot {
        let registry_metadata = spy_metadata(effect_id);
        EffectSlot {
            effect_id,
            registry_metadata: registry_metadata.clone(),
            registry_source_path: PathBuf::from("mock/advance-spy.wgsl"),
            registry_modified: SystemTime::UNIX_EPOCH,
            metadata: registry_metadata,
            display_descriptor: None,
            canvas_width: 1,
            canvas_height: 1,
            renderer: Box::new(AdvanceSpyRenderer { advanced }),
            controls: ControlSet::new(SetRevision::default()),
            control_bindings: HashMap::new(),
            controls_initialized: true,
            binding_state: HashMap::new(),
            resolution_seq: 0,
            elapsed: Duration::ZERO,
            frame_number: 0,
        }
    }

    fn registry_with_builtins() -> EffectRegistry {
        let mut registry = EffectRegistry::new(Vec::new());
        register_builtin_effects(&mut registry);
        registry
    }

    fn builtin_effect_id(registry: &EffectRegistry, stem: &str) -> EffectId {
        registry
            .iter()
            .find_map(|(id, entry)| {
                (entry.metadata.source.source_stem() == Some(stem)).then_some(*id)
            })
            .expect("builtin effect should be registered")
    }

    fn render_zone(id: ZoneId, effect_id: EffectId) -> Zone {
        Zone {
            id,
            name: "Desk".into(),
            description: None,
            layers: vec![SceneLayer::from_effect(
                SceneLayerId::new(),
                effect_id,
                HashMap::new(),
                HashMap::new(),
                None,
            )],
            layout: sample_layout(),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: ZoneRole::Custom,
            controls_version: 0,
            layers_version: 0,
        }
    }

    #[test]
    fn dropping_effect_slot_calls_destroy() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let slot = spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed));

        drop(slot);

        assert!(destroyed.load(Ordering::SeqCst));
    }

    #[test]
    fn effect_slot_clock_advances_after_long_uptime() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let mut slot = spy_slot(EffectId::new(uuid::Uuid::now_v7()), destroyed);
        slot.elapsed = Duration::from_hours(60 * 24);
        let before = slot.elapsed.as_secs_f64();

        let after = slot.advance_elapsed(1.0 / 60.0);

        assert!(after > before);
        assert_eq!(after, slot.elapsed.as_secs_f64());
    }

    #[test]
    fn authored_delta_updates_the_derived_cache_without_snapshot_replay() {
        let effect_id = EffectId::new(uuid::Uuid::now_v7());
        let mut metadata = spy_metadata(effect_id);
        metadata.controls.push(ControlDefinition {
            id: "speed".into(),
            name: "Speed".into(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(1.0),
            min: Some(0.0),
            max: Some(10.0),
            step: Some(1.0),
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        });
        let initialized = Arc::new(Mutex::new(Vec::new()));
        let applied = Arc::new(Mutex::new(Vec::new()));
        let mut slot = EffectSlot {
            effect_id,
            registry_metadata: metadata.clone(),
            registry_source_path: PathBuf::from("mock/control-lifecycle-spy.wgsl"),
            registry_modified: SystemTime::UNIX_EPOCH,
            metadata,
            display_descriptor: None,
            canvas_width: 1,
            canvas_height: 1,
            renderer: Box::new(ControlLifecycleSpyRenderer {
                initialized: Arc::clone(&initialized),
                applied: Arc::clone(&applied),
                fail_initialize: Arc::new(AtomicBool::new(false)),
                fail_delta: Arc::new(AtomicBool::new(false)),
            }),
            controls: ControlSet::try_from_entries(
                SetRevision::default(),
                [(ControlId::from("speed"), ControlValue::Float(1.0))],
            )
            .expect("valid initial controls"),
            control_bindings: HashMap::new(),
            controls_initialized: true,
            binding_state: HashMap::new(),
            resolution_seq: 0,
            elapsed: Duration::ZERO,
            frame_number: 0,
        };
        let source = super::LayerEffectSource {
            effect_id,
            controls: HashMap::from([("speed".into(), ControlValue::Float(2.0))]),
            control_bindings: HashMap::new(),
        };

        slot.sync_layer_state(source, SetRevision::new(1))
            .expect("snapshot recovery should update the renderer");

        assert_eq!(
            initialized
                .lock()
                .expect("control initialization log should be available")
                .len(),
            0
        );
        assert_eq!(
            applied
                .lock()
                .expect("control delta log should be available")
                .as_slice(),
            &[vec![("speed".into(), ControlValue::Float(2.0))]]
        );
        assert_eq!(slot.controls.set_revision(), SetRevision::new(1));
        assert_eq!(slot.controls.get("speed"), Some(&ControlValue::Float(2.0)));
    }

    #[test]
    fn rejected_authored_delta_replays_snapshot_before_committing_authority() {
        let effect_id = EffectId::new(uuid::Uuid::now_v7());
        let mut metadata = spy_metadata(effect_id);
        metadata.controls.push(number_control("speed", 1.0));
        let initialized = Arc::new(Mutex::new(Vec::new()));
        let applied = Arc::new(Mutex::new(Vec::new()));
        let mut slot = lifecycle_slot(
            metadata,
            ControlSet::try_from_entries(
                SetRevision::default(),
                [(ControlId::from("speed"), ControlValue::Float(1.0))],
            )
            .expect("valid initial controls"),
            Arc::clone(&initialized),
            Arc::clone(&applied),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(true)),
        );

        slot.sync_layer_state(
            super::LayerEffectSource {
                effect_id,
                controls: HashMap::from([("speed".into(), ControlValue::Float(2.0))]),
                control_bindings: HashMap::new(),
            },
            SetRevision::new(1),
        )
        .expect("snapshot replay should recover the rejected delta");

        let snapshots = initialized
            .lock()
            .expect("control initialization log should be available");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].set_revision(), SetRevision::new(1));
        assert_eq!(snapshots[0].get("speed"), Some(&ControlValue::Float(2.0)));
        assert_eq!(slot.controls.set_revision(), SetRevision::new(1));
        assert_eq!(slot.controls.get("speed"), Some(&ControlValue::Float(2.0)));
        assert!(slot.controls_initialized);
    }

    #[test]
    fn double_renderer_rejection_keeps_old_authority_and_invalidates_slot() {
        let effect_id = EffectId::new(uuid::Uuid::now_v7());
        let mut metadata = spy_metadata(effect_id);
        metadata.controls.push(number_control("speed", 1.0));
        let initialized = Arc::new(Mutex::new(Vec::new()));
        let fail_initialize = Arc::new(AtomicBool::new(true));
        let mut slot = lifecycle_slot(
            metadata,
            ControlSet::try_from_entries(
                SetRevision::default(),
                [(ControlId::from("speed"), ControlValue::Float(1.0))],
            )
            .expect("valid initial controls"),
            Arc::clone(&initialized),
            Arc::new(Mutex::new(Vec::new())),
            Arc::clone(&fail_initialize),
            Arc::new(AtomicBool::new(true)),
        );
        let source = super::LayerEffectSource {
            effect_id,
            controls: HashMap::from([("speed".into(), ControlValue::Float(2.0))]),
            control_bindings: HashMap::new(),
        };

        let error = slot
            .sync_layer_state(source.clone(), SetRevision::new(1))
            .expect_err("double renderer rejection must fail the commit");

        assert!(error.to_string().contains("snapshot replay"));
        assert_eq!(slot.controls.set_revision(), SetRevision::default());
        assert_eq!(slot.controls.get("speed"), Some(&ControlValue::Float(1.0)));
        assert!(!slot.controls_initialized);

        slot.sync_layer_state(source, SetRevision::new(1))
            .expect("the invalid slot should accept a later authoritative replay");
        assert_eq!(slot.controls.set_revision(), SetRevision::new(1));
        assert_eq!(slot.controls.get("speed"), Some(&ControlValue::Float(2.0)));
        assert!(slot.controls_initialized);
        assert_eq!(
            initialized
                .lock()
                .expect("control initialization log should be available")
                .len(),
            2
        );
        assert!(!fail_initialize.load(Ordering::SeqCst));
    }

    #[test]
    fn rejected_sensor_delta_replays_resolved_snapshot() {
        let effect_id = EffectId::new(uuid::Uuid::now_v7());
        let binding = ControlBinding {
            sensor: "cpu_temp".into(),
            sensor_min: 30.0,
            sensor_max: 100.0,
            target_min: 0.0,
            target_max: 10.0,
            deadband: 0.0,
            smoothing: 0.0,
        };
        let mut metadata = spy_metadata(effect_id);
        let mut definition = number_control("speed", 5.0);
        definition.binding = Some(binding.clone());
        metadata.controls.push(definition);
        let controls = ControlSet::try_from_entries(
            SetRevision::default(),
            [(ControlId::from("speed"), ControlValue::Float(5.0))],
        )
        .expect("valid controls");
        let initialized = Arc::new(Mutex::new(Vec::new()));
        let mut renderer = ControlLifecycleSpyRenderer {
            initialized: Arc::clone(&initialized),
            applied: Arc::new(Mutex::new(Vec::new())),
            fail_initialize: Arc::new(AtomicBool::new(false)),
            fail_delta: Arc::new(AtomicBool::new(true)),
        };
        let mut binding_state = HashMap::new();
        let mut resolution_seq = 0;
        let mut controls_initialized = true;

        super::apply_sensor_bindings(
            &mut renderer,
            &metadata,
            &HashMap::from([("speed".into(), binding)]),
            &controls,
            &mut binding_state,
            &mut resolution_seq,
            &mut controls_initialized,
            &hypercolor_types::sensor::SystemSnapshot {
                cpu_temp_celsius: Some(58.0),
                ..hypercolor_types::sensor::SystemSnapshot::empty()
            },
        )
        .expect("snapshot replay should recover the rejected sensor delta");

        let snapshots = initialized
            .lock()
            .expect("control initialization log should be available");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].get("speed"), Some(&ControlValue::Float(4.0)));
        assert_eq!(resolution_seq, 1);
        assert!(controls_initialized);
    }

    #[test]
    fn authored_delta_preserves_active_sensor_resolution() {
        let effect_id = EffectId::new(uuid::Uuid::now_v7());
        let binding = ControlBinding {
            sensor: "cpu_temp".into(),
            sensor_min: 30.0,
            sensor_max: 100.0,
            target_min: 0.0,
            target_max: 10.0,
            deadband: 0.0,
            smoothing: 0.0,
        };
        let mut metadata = spy_metadata(effect_id);
        metadata.controls.extend([
            ControlDefinition {
                id: "speed".into(),
                name: "Speed".into(),
                kind: ControlKind::Number,
                control_type: ControlType::Slider,
                default_value: ControlValue::Float(5.0),
                min: Some(0.0),
                max: Some(10.0),
                step: Some(1.0),
                labels: Vec::new(),
                group: None,
                tooltip: None,
                aspect_lock: None,
                preview_source: None,
                binding: Some(binding.clone()),
            },
            ControlDefinition {
                id: "brightness".into(),
                name: "Brightness".into(),
                kind: ControlKind::Number,
                control_type: ControlType::Slider,
                default_value: ControlValue::Float(1.0),
                min: Some(0.0),
                max: Some(10.0),
                step: Some(1.0),
                labels: Vec::new(),
                group: None,
                tooltip: None,
                aspect_lock: None,
                preview_source: None,
                binding: None,
            },
        ]);
        let initialized = Arc::new(Mutex::new(Vec::new()));
        let applied = Arc::new(Mutex::new(Vec::new()));
        let mut slot = EffectSlot {
            effect_id,
            registry_metadata: metadata.clone(),
            registry_source_path: PathBuf::from("mock/control-lifecycle-spy.wgsl"),
            registry_modified: SystemTime::UNIX_EPOCH,
            metadata,
            display_descriptor: None,
            canvas_width: 1,
            canvas_height: 1,
            renderer: Box::new(ControlLifecycleSpyRenderer {
                initialized: Arc::clone(&initialized),
                applied: Arc::clone(&applied),
                fail_initialize: Arc::new(AtomicBool::new(false)),
                fail_delta: Arc::new(AtomicBool::new(false)),
            }),
            controls: ControlSet::try_from_entries(
                SetRevision::default(),
                [
                    (ControlId::from("speed"), ControlValue::Float(5.0)),
                    (ControlId::from("brightness"), ControlValue::Float(1.0)),
                ],
            )
            .expect("valid initial controls"),
            control_bindings: HashMap::from([("speed".into(), binding.clone())]),
            controls_initialized: true,
            binding_state: HashMap::from([(
                "speed".into(),
                super::ActiveBindingState {
                    sensor_value: Some(58.0),
                    control_value: ControlValue::Float(4.0),
                },
            )]),
            resolution_seq: 1,
            elapsed: Duration::ZERO,
            frame_number: 0,
        };
        let source = super::LayerEffectSource {
            effect_id,
            controls: HashMap::from([
                ("speed".into(), ControlValue::Float(5.0)),
                ("brightness".into(), ControlValue::Float(2.0)),
            ]),
            control_bindings: HashMap::from([("speed".into(), binding)]),
        };

        slot.sync_layer_state(source, SetRevision::new(1))
            .expect("snapshot recovery should update the renderer");

        assert_eq!(
            slot.binding_state.get("speed"),
            Some(&super::ActiveBindingState {
                sensor_value: Some(58.0),
                control_value: ControlValue::Float(4.0),
            })
        );
        assert_eq!(slot.resolution_seq, 0);
        assert_eq!(
            initialized
                .lock()
                .expect("control initialization log should be available")
                .len(),
            0
        );
        assert_eq!(
            applied
                .lock()
                .expect("control delta log should be available")
                .as_slice(),
            &[vec![("brightness".into(), ControlValue::Float(2.0))]]
        );

        let sensors = hypercolor_types::sensor::SystemSnapshot {
            cpu_temp_celsius: Some(58.0),
            ..hypercolor_types::sensor::SystemSnapshot::empty()
        };
        super::apply_sensor_bindings(
            slot.renderer.as_mut(),
            &slot.metadata,
            &slot.control_bindings,
            &slot.controls,
            &mut slot.binding_state,
            &mut slot.resolution_seq,
            &mut slot.controls_initialized,
            &sensors,
        )
        .expect("unchanged sensor value should remain resolved");

        assert_eq!(slot.resolution_seq, 0);
        assert_eq!(
            applied
                .lock()
                .expect("control delta log should be available")
                .len(),
            1
        );
    }

    #[test]
    fn sensor_bindings_apply_and_restore_the_authored_control() {
        let mut metadata = spy_metadata(EffectId::new(uuid::Uuid::now_v7()));
        metadata.controls.push(ControlDefinition {
            id: "speed".into(),
            name: "Speed".into(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(5.0),
            min: Some(0.0),
            max: Some(10.0),
            step: Some(1.0),
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: Some(ControlBinding {
                sensor: "cpu_temp".into(),
                sensor_min: 30.0,
                sensor_max: 100.0,
                target_min: 0.0,
                target_max: 10.0,
                deadband: 0.0,
                smoothing: 0.0,
            }),
        });
        let controls = ControlSet::try_from_entries(
            SetRevision::default(),
            [(ControlId::from("speed"), ControlValue::Float(5.0))],
        )
        .expect("valid controls");
        let bindings = HashMap::from([(
            "speed".into(),
            metadata.controls[0]
                .binding
                .clone()
                .expect("sensor binding"),
        )]);
        let mut binding_state = HashMap::new();
        let mut resolution_seq = 0;
        let mut controls_initialized = true;
        let mut renderer = ControlSpyRenderer::default();
        let live_sensors = hypercolor_types::sensor::SystemSnapshot {
            cpu_temp_celsius: Some(58.0),
            ..hypercolor_types::sensor::SystemSnapshot::empty()
        };

        super::apply_sensor_bindings(
            &mut renderer,
            &metadata,
            &bindings,
            &controls,
            &mut binding_state,
            &mut resolution_seq,
            &mut controls_initialized,
            &live_sensors,
        )
        .expect("first sensor delivery");
        assert_eq!(resolution_seq, 1);
        assert_eq!(
            renderer.applied.last(),
            Some(&("speed".into(), ControlValue::Float(4.0)))
        );

        super::apply_sensor_bindings(
            &mut renderer,
            &metadata,
            &bindings,
            &controls,
            &mut binding_state,
            &mut resolution_seq,
            &mut controls_initialized,
            &live_sensors,
        )
        .expect("unchanged sensor delivery");
        assert_eq!(resolution_seq, 1);
        assert_eq!(renderer.applied.len(), 1);

        super::apply_sensor_bindings(
            &mut renderer,
            &metadata,
            &bindings,
            &controls,
            &mut binding_state,
            &mut resolution_seq,
            &mut controls_initialized,
            &hypercolor_types::sensor::SystemSnapshot::empty(),
        )
        .expect("authored value restoration");
        assert_eq!(resolution_seq, 2);
        assert_eq!(
            renderer.applied.last(),
            Some(&("speed".into(), ControlValue::Float(5.0)))
        );
    }

    #[test]
    fn reconcile_pruning_destroys_removed_slot() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let zone_id = ZoneId::new();
        let layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(zone_id, layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        pool.reconcile(&[], &EffectRegistry::new(Vec::new()), &HashMap::new())
            .expect("prune should succeed");

        assert!(destroyed.load(Ordering::SeqCst));
        assert!(pool.slots.is_empty());
    }

    #[test]
    fn clear_destroys_slots() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let zone_id = ZoneId::new();
        let layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(zone_id, layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        pool.clear();

        assert!(destroyed.load(Ordering::SeqCst));
        assert!(pool.slots.is_empty());
    }

    #[test]
    fn remove_zone_destroys_matching_slots_only() {
        let removed = Arc::new(AtomicBool::new(false));
        let kept = Arc::new(AtomicBool::new(false));
        let removed_zone_id = ZoneId::new();
        let kept_zone_id = ZoneId::new();
        let kept_layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(removed_zone_id, SceneLayerId::new()),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&removed)),
        );
        pool.slots.insert(
            EffectSlotKey::new(kept_zone_id, kept_layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&kept)),
        );

        pool.remove_zone(removed_zone_id);

        assert!(removed.load(Ordering::SeqCst));
        assert!(!kept.load(Ordering::SeqCst));
        assert_eq!(pool.slots.len(), 1);
        assert!(
            pool.slots
                .contains_key(&EffectSlotKey::new(kept_zone_id, kept_layer_id))
        );
    }

    #[test]
    fn advance_layer_output_ticks_renderer_without_rendering_canvas() {
        let advanced = Arc::new(AtomicU64::new(0));
        let effect_id = EffectId::new(uuid::Uuid::now_v7());
        let zone_id = ZoneId::new();
        let zone = render_zone(zone_id, effect_id);
        let layer = zone
            .layers
            .clone()
            .into_iter()
            .next()
            .expect("effect zone should expose its authored layer");
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(zone_id, layer.id),
            advance_spy_slot(effect_id, Arc::clone(&advanced)),
        );
        let audio = AudioData::silence();
        let interaction = InteractionData::default();
        let sensors = hypercolor_types::sensor::SystemSnapshot::empty();

        pool.advance_layer_output(
            &zone,
            &layer,
            1.0 / 60.0,
            &audio,
            &interaction,
            None,
            &sensors,
            FrameDataSources::default(),
        )
        .expect("advance should succeed");

        assert_eq!(advanced.load(Ordering::SeqCst), 1);
        assert_eq!(
            pool.slots
                .get(&EffectSlotKey::new(zone_id, layer.id))
                .expect("slot should remain")
                .frame_number,
            1
        );
    }

    #[test]
    fn reconcile_replacement_destroys_old_slot() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let zone_id = ZoneId::new();
        let layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(zone_id, layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        let registry = registry_with_builtins();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let zone = render_zone(zone_id, solid_id);

        pool.reconcile(&[zone], &registry, &HashMap::new())
            .expect("replacement should succeed");

        assert!(destroyed.load(Ordering::SeqCst));
        assert_eq!(pool.slots.len(), 1);
    }

    #[test]
    fn reconcile_applies_control_deltas_without_rebuilding_the_slot() {
        let registry = registry_with_builtins();
        let effect_id = builtin_effect_id(&registry, "solid_color");
        let zone_id = ZoneId::new();
        let mut zone = render_zone(zone_id, effect_id);
        let layer_id = zone.layers[0].id;
        let key = EffectSlotKey::new(zone_id, layer_id);
        let mut pool = EffectPool::new();
        pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
            .expect("initial reconcile");
        pool.slots.get_mut(&key).expect("effect slot").frame_number = 41;

        let LayerSource::Effect { controls, .. } = &mut zone.layers[0].source else {
            panic!("test layer should be an effect");
        };
        controls.insert("brightness".into(), ControlValue::Float(0.25));
        zone.controls_version = 1;
        pool.reconcile(std::slice::from_ref(&zone), &registry, &HashMap::new())
            .expect("control delta reconcile");

        let slot = pool.slots.get(&key).expect("reused effect slot");
        assert_eq!(slot.frame_number, 41);
        assert_eq!(slot.controls.set_revision(), SetRevision::new(1));
        assert_eq!(
            slot.controls.get("brightness"),
            Some(&ControlValue::Float(0.25))
        );
    }

    #[cfg(feature = "servo")]
    #[test]
    fn web_viewport_layer_maps_to_builtin_effect_controls() {
        let layer = SceneLayer {
            id: SceneLayerId::new(),
            name: Some("Web".into()),
            source: LayerSource::WebViewport {
                url: "localhost:9430".into(),
                viewport: hypercolor_types::viewport::ViewportRect::new(0.1, 0.2, 0.3, 0.4),
                render: hypercolor_types::layer::WebViewportRender::Snapshot,
            },
            blend: BlendMode::Replace,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        };

        let source = layer_effect_source(&layer).expect("web viewport should map to effect");

        assert_eq!(
            source.effect_id,
            crate::effect::builtin::builtin_effect_stable_id("web_viewport")
        );
        assert_eq!(
            source.controls.get("url"),
            Some(&hypercolor_types::control::ControlValue::Text(
                "localhost:9430".into()
            ))
        );
        assert_eq!(
            source.controls.get("refresh_interval"),
            Some(&hypercolor_types::control::ControlValue::Float(300.0))
        );
    }
}
