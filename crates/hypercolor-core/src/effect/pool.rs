use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Result, anyhow};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::effect::{
    ControlBinding, ControlDefinition, ControlKind, ControlValue, EffectId, EffectMetadata,
};
use hypercolor_types::layer::{LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{Zone, ZoneId};
use hypercolor_types::sensor::SystemSnapshot;
#[cfg(feature = "servo")]
use hypercolor_types::viewport::FitMode;
use tokio::sync::RwLock;

use super::factory::create_renderer_for_metadata;
use super::registry::{EffectEntry, EffectRegistry};
use super::traits::{EffectRenderOutput, EffectRenderer, FrameInput, prepare_target_canvas};
use crate::asset::AssetLibrary;
use crate::input::{InteractionData, ScreenData};

pub struct EffectPool {
    slots: HashMap<EffectSlotKey, EffectSlot>,
    asset_library: Option<Arc<RwLock<AssetLibrary>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectSlotKey {
    pub group_id: ZoneId,
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
    pub const fn new(group_id: ZoneId, layer_id: SceneLayerId) -> Self {
        Self { group_id, layer_id }
    }
}

impl EffectPool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            asset_library: None,
        }
    }

    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Provide the asset library used by asset-backed effect renderers.
    pub fn set_asset_library(&mut self, asset_library: Arc<RwLock<AssetLibrary>>) {
        self.asset_library = Some(asset_library);
    }

    pub fn reconcile(
        &mut self,
        groups: &[Zone],
        registry: &EffectRegistry,
        display_descriptors: &HashMap<ZoneId, DisplayDescriptor>,
    ) -> Result<()> {
        let desired_keys = desired_effect_layers(groups)
            .into_iter()
            .map(|(group, layer)| EffectSlotKey::new(group.id, layer.id))
            .collect::<HashSet<_>>();
        self.slots.retain(|key, _| desired_keys.contains(key));

        for (group, layer) in desired_effect_layers(groups) {
            let Some(source) = layer_effect_source(&layer) else {
                continue;
            };
            let key = EffectSlotKey::new(group.id, layer.id);

            let entry = lookup_effect_entry(registry, source.effect_id)?;
            let resolved_effect_id = registry
                .resolve_id(&source.effect_id)
                .unwrap_or(source.effect_id);

            let display_descriptor = group
                .display_target
                .as_ref()
                .and_then(|_| display_descriptors.get(&group.id));
            let needs_rebuild = self.slots.get(&key).is_none_or(|slot| {
                slot.needs_rebuild(resolved_effect_id, entry, display_descriptor)
            });
            if needs_rebuild {
                let slot = EffectSlot::build(
                    entry,
                    group,
                    &layer,
                    self.asset_library.as_ref(),
                    display_descriptor.cloned(),
                )?;
                self.slots.insert(key, slot);
                continue;
            }

            if let Some(slot) = self.slots.get_mut(&key) {
                slot.sync_layer_state(&layer);
            }
        }

        Ok(())
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }

    pub fn remove_group(&mut self, group_id: ZoneId) {
        self.slots.retain(|key, _| key.group_id != group_id);
    }

    pub fn render_group_into(
        &mut self,
        group: &Zone,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        target: &mut Canvas,
    ) -> Result<()> {
        let Some(layer) = single_enabled_effect_layer(group)? else {
            target.clear();
            return Ok(());
        };
        self.render_layer_into(
            group,
            &layer,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            target,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input plus a mutable target canvas"
    )]
    pub fn render_layer_into(
        &mut self,
        group: &Zone,
        layer: &SceneLayer,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
        target: &mut Canvas,
    ) -> Result<()> {
        prepare_target_canvas(
            target,
            group.layout.canvas_width,
            group.layout.canvas_height,
        );

        if !group.enabled || !layer.enabled || layer_effect_source(layer).is_none() {
            target.clear();
            return Ok(());
        }

        let key = EffectSlotKey::new(group.id, layer.id);
        let slot = self.slots.get_mut(&key).ok_or_else(|| {
            anyhow!(
                "zone '{}' layer '{}' is not reconciled before rendering",
                group.name,
                layer.id
            )
        })?;
        slot.render_into(
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            group.layout.canvas_width,
            group.layout.canvas_height,
            target,
        )
    }

    pub fn render_group_output(
        &mut self,
        group: &Zone,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
    ) -> Result<EffectRenderOutput> {
        let Some(layer) = single_enabled_effect_layer(group)? else {
            return Ok(EffectRenderOutput::Cpu(Canvas::new(
                group.layout.canvas_width,
                group.layout.canvas_height,
            )));
        };
        self.render_layer_output(
            group,
            &layer,
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "rendering needs the full frame input for output-capable renderers"
    )]
    pub fn render_layer_output(
        &mut self,
        group: &Zone,
        layer: &SceneLayer,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
    ) -> Result<EffectRenderOutput> {
        if !group.enabled || !layer.enabled || layer_effect_source(layer).is_none() {
            return Ok(EffectRenderOutput::Cpu(Canvas::new(
                group.layout.canvas_width,
                group.layout.canvas_height,
            )));
        }

        let key = EffectSlotKey::new(group.id, layer.id);
        let slot = self.slots.get_mut(&key).ok_or_else(|| {
            anyhow!(
                "zone '{}' layer '{}' is not reconciled before rendering",
                group.name,
                layer.id
            )
        })?;
        slot.render_output(
            delta_secs,
            audio,
            interaction,
            screen,
            sensors,
            group.layout.canvas_width,
            group.layout.canvas_height,
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
    renderer: Box<dyn EffectRenderer>,
    controls: HashMap<String, ControlValue>,
    binding_state: HashMap<String, ActiveBindingState>,
    elapsed_secs: f32,
    frame_number: u64,
}

impl EffectSlot {
    fn build(
        entry: &EffectEntry,
        group: &Zone,
        layer: &SceneLayer,
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
            group.layout.canvas_width,
            group.layout.canvas_height,
        )?;

        let mut slot = Self {
            effect_id: entry.metadata.id,
            registry_metadata: entry.metadata.clone(),
            registry_source_path: entry.source_path.clone(),
            registry_modified: entry.modified,
            metadata: entry.metadata.clone(),
            display_descriptor,
            renderer,
            controls: HashMap::new(),
            binding_state: HashMap::new(),
            elapsed_secs: 0.0,
            frame_number: 0,
        };
        slot.sync_layer_state(layer);
        Ok(slot)
    }

    fn needs_rebuild(
        &self,
        effect_id: EffectId,
        entry: &EffectEntry,
        display_descriptor: Option<&DisplayDescriptor>,
    ) -> bool {
        self.effect_id != effect_id
            || self.registry_metadata != entry.metadata
            || self.registry_source_path != entry.source_path
            || self.registry_modified != entry.modified
            || self.display_descriptor.as_ref() != display_descriptor
    }

    fn sync_layer_state(&mut self, layer: &SceneLayer) {
        let mut desired = HashMap::new();
        let Some(source) = layer_effect_source(layer) else {
            self.controls.clear();
            self.binding_state.clear();
            return;
        };

        for definition in &mut self.metadata.controls {
            let next_binding = source
                .control_bindings
                .get(definition.control_id())
                .cloned();
            if definition.binding != next_binding {
                definition.binding = next_binding;
                self.binding_state.remove(definition.control_id());
            }
            let value = source
                .controls
                .get(definition.control_id())
                .cloned()
                .unwrap_or_else(|| definition.default_value.clone());
            desired.insert(definition.control_id().to_owned(), value);
        }

        for (name, value) in &source.controls {
            desired.entry(name.clone()).or_insert_with(|| value.clone());
        }

        for (name, value) in &desired {
            if self.controls.get(name) != Some(value) {
                self.renderer.set_control(name, value);
            }
        }

        self.controls = desired;
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
        canvas_width: u32,
        canvas_height: u32,
        target: &mut Canvas,
    ) -> Result<()> {
        self.elapsed_secs += delta_secs;
        apply_sensor_bindings(
            self.renderer.as_mut(),
            &self.metadata,
            &self.controls,
            &mut self.binding_state,
            sensors,
        );
        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
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
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<EffectRenderOutput> {
        self.elapsed_secs += delta_secs;
        apply_sensor_bindings(
            self.renderer.as_mut(),
            &self.metadata,
            &self.controls,
            &mut self.binding_state,
            sensors,
        );
        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            sensors,
            canvas_width,
            canvas_height,
        };
        let output = self.renderer.render_output(&input)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(output)
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

fn desired_effect_layers(groups: &[Zone]) -> Vec<(&Zone, SceneLayer)> {
    groups
        .iter()
        .filter(|group| group.enabled)
        .flat_map(|group| {
            group
                .effective_layers()
                .into_iter()
                .filter(|layer| layer.enabled && layer_effect_source(layer).is_some())
                .map(move |layer| (group, layer))
        })
        .collect()
}

fn single_enabled_effect_layer(group: &Zone) -> Result<Option<SceneLayer>> {
    if !group.enabled {
        return Ok(None);
    }

    let mut layers = group
        .effective_layers()
        .into_iter()
        .filter(|layer| layer.enabled && layer_effect_source(layer).is_some());
    let Some(layer) = layers.next() else {
        return Ok(None);
    };
    if layers.next().is_some() {
        return Err(anyhow!(
            "zone '{}' has multiple enabled effect layers; render layers explicitly",
            group.name
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
        ("viewport".to_owned(), ControlValue::Rect(viewport)),
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

#[derive(Debug, Clone, PartialEq)]
struct ActiveBindingState {
    sensor_value: Option<f32>,
    control_value: ControlValue,
}

fn apply_sensor_bindings(
    renderer: &mut dyn EffectRenderer,
    metadata: &EffectMetadata,
    controls: &HashMap<String, ControlValue>,
    binding_state: &mut HashMap<String, ActiveBindingState>,
    sensors: &SystemSnapshot,
) {
    for control in &metadata.controls {
        let control_id = control.control_id();
        let Some(binding) = control.binding.as_ref() else {
            if binding_state.remove(control_id).is_some()
                && let Some(base_value) = controls.get(control_id)
            {
                renderer.set_control(control_id, base_value);
            }
            continue;
        };

        let Some(base_value) = controls.get(control_id) else {
            continue;
        };

        let next_state = sensors
            .reading(&binding.sensor)
            .and_then(|reading| {
                evaluate_sensor_binding(
                    control,
                    reading.value,
                    binding.target_min,
                    binding.target_max,
                    binding.sensor_min,
                    binding.sensor_max,
                    binding.deadband,
                    binding.smoothing,
                    binding_state.get(control_id),
                )
                .map(|value| ActiveBindingState {
                    sensor_value: Some(reading.value),
                    control_value: value,
                })
            })
            .unwrap_or_else(|| ActiveBindingState {
                sensor_value: None,
                control_value: base_value.clone(),
            });

        if binding_state.get(control_id) != Some(&next_state) {
            renderer.set_control(control_id, &next_state.control_value);
        }
        binding_state.insert(control_id.to_owned(), next_state);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "binding evaluation works on normalized scalar ranges plus previous state"
)]
fn evaluate_sensor_binding(
    control: &ControlDefinition,
    sensor_value: f32,
    target_min: f32,
    target_max: f32,
    sensor_min: f32,
    sensor_max: f32,
    deadband: f32,
    smoothing: f32,
    previous: Option<&ActiveBindingState>,
) -> Option<ControlValue> {
    let source_span = sensor_max - sensor_min;
    if !source_span.is_finite()
        || source_span.abs() < f32::EPSILON
        || !target_min.is_finite()
        || !target_max.is_finite()
    {
        return None;
    }

    if let Some(previous) = previous
        && let Some(previous_sensor) = previous.sensor_value
        && (sensor_value - previous_sensor).abs() <= deadband
    {
        return Some(previous.control_value.clone());
    }

    let normalized = ((sensor_value - sensor_min) / source_span).clamp(0.0, 1.0);
    let mapped = target_min + normalized * (target_max - target_min);
    let smoothed = previous
        .and_then(|state| state.control_value.as_f32())
        .map_or(mapped, |previous_value| {
            let alpha = 1.0 - smoothing;
            previous_value + (mapped - previous_value) * alpha
        });

    match control.kind {
        ControlKind::Number | ControlKind::Hue | ControlKind::Area => {
            control.validate_value(&ControlValue::Float(smoothed)).ok()
        }
        ControlKind::Boolean => {
            let midpoint = target_min + (target_max - target_min) * 0.5;
            control
                .validate_value(&ControlValue::Boolean(smoothed >= midpoint))
                .ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::SystemTime;

    use anyhow::Result;

    #[cfg(feature = "servo")]
    use super::layer_effect_source;
    use super::{EffectPool, EffectSlot, EffectSlotKey};
    use crate::effect::builtin::register_builtin_effects;
    use crate::effect::registry::EffectRegistry;
    use crate::effect::traits::{EffectRenderer, FrameInput};
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
    use hypercolor_types::layer::SceneLayerId;
    #[cfg(feature = "servo")]
    use hypercolor_types::layer::{
        LayerAdjust, LayerBlendMode, LayerSource, LayerTransform, SceneLayer,
    };
    use hypercolor_types::scene::{Zone, ZoneId, ZoneRole};
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
        StripDirection,
    };

    struct DestroySpyRenderer {
        destroyed: Arc<AtomicBool>,
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

        fn set_control(&mut self, _name: &str, _value: &hypercolor_types::effect::ControlValue) {}

        fn destroy(&mut self) {
            self.destroyed.store(true, Ordering::SeqCst);
        }
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
            spaces: None,
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
            source: EffectSource::Native {
                path: "mock/destroy-spy.wgsl".into(),
            },
            license: Some("Apache-2.0".into()),
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
            renderer: Box::new(DestroySpyRenderer::new(destroyed)),
            controls: HashMap::new(),
            binding_state: HashMap::new(),
            elapsed_secs: 0.0,
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

    fn render_group(id: ZoneId, effect_id: EffectId) -> Zone {
        Zone {
            id,
            name: "Desk".into(),
            description: None,
            effect_id: Some(effect_id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layers: Vec::new(),
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
    fn reconcile_pruning_destroys_removed_slot() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let group_id = ZoneId::new();
        let layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(group_id, layer_id),
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
        let group_id = ZoneId::new();
        let layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(group_id, layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        pool.clear();

        assert!(destroyed.load(Ordering::SeqCst));
        assert!(pool.slots.is_empty());
    }

    #[test]
    fn remove_group_destroys_matching_slots_only() {
        let removed = Arc::new(AtomicBool::new(false));
        let kept = Arc::new(AtomicBool::new(false));
        let removed_group_id = ZoneId::new();
        let kept_group_id = ZoneId::new();
        let kept_layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(removed_group_id, SceneLayerId::new()),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&removed)),
        );
        pool.slots.insert(
            EffectSlotKey::new(kept_group_id, kept_layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&kept)),
        );

        pool.remove_group(removed_group_id);

        assert!(removed.load(Ordering::SeqCst));
        assert!(!kept.load(Ordering::SeqCst));
        assert_eq!(pool.slots.len(), 1);
        assert!(
            pool.slots
                .contains_key(&EffectSlotKey::new(kept_group_id, kept_layer_id))
        );
    }

    #[test]
    fn reconcile_replacement_destroys_old_slot() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let group_id = ZoneId::new();
        let layer_id = SceneLayerId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            EffectSlotKey::new(group_id, layer_id),
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        let registry = registry_with_builtins();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let group = render_group(group_id, solid_id);

        pool.reconcile(&[group], &registry, &HashMap::new())
            .expect("replacement should succeed");

        assert!(destroyed.load(Ordering::SeqCst));
        assert_eq!(pool.slots.len(), 1);
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
            blend: LayerBlendMode::Replace,
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
            Some(&hypercolor_types::effect::ControlValue::Text(
                "localhost:9430".into()
            ))
        );
        assert_eq!(
            source.controls.get("refresh_interval"),
            Some(&hypercolor_types::effect::ControlValue::Float(300.0))
        );
    }
}
