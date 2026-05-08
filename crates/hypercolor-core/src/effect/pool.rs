use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Result, anyhow};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlValue, EffectId, EffectMetadata,
};
use hypercolor_types::scene::{RenderGroup, RenderGroupId};
use hypercolor_types::sensor::SystemSnapshot;

use super::factory::create_renderer_for_metadata;
use super::registry::{EffectEntry, EffectRegistry};
use super::traits::{EffectRenderOutput, EffectRenderer, FrameInput, prepare_target_canvas};
use crate::input::{InteractionData, ScreenData};

pub struct EffectPool {
    slots: HashMap<RenderGroupId, EffectSlot>,
}

impl EffectPool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn reconcile(&mut self, groups: &[RenderGroup], registry: &EffectRegistry) -> Result<()> {
        let desired_ids = groups.iter().map(|group| group.id).collect::<HashSet<_>>();
        self.slots
            .retain(|group_id, _| desired_ids.contains(group_id));

        for group in groups {
            let Some(effect_id) = group.effect_id else {
                self.slots.remove(&group.id);
                continue;
            };

            let entry = lookup_effect_entry(registry, effect_id)?;
            let resolved_effect_id = registry.resolve_id(&effect_id).unwrap_or(effect_id);

            let needs_rebuild = self
                .slots
                .get(&group.id)
                .is_none_or(|slot| slot.needs_rebuild(resolved_effect_id, entry));
            if needs_rebuild {
                let slot = EffectSlot::build(entry, group)?;
                self.slots.insert(group.id, slot);
                continue;
            }

            if let Some(slot) = self.slots.get_mut(&group.id) {
                slot.sync_group_state(group);
            }
        }

        Ok(())
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }

    pub fn render_group_into(
        &mut self,
        group: &RenderGroup,
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

        if !group.enabled || group.effect_id.is_none() {
            target.clear();
            return Ok(());
        }

        let slot = self.slots.get_mut(&group.id).ok_or_else(|| {
            anyhow!(
                "render group '{}' is not reconciled before rendering",
                group.name
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
        group: &RenderGroup,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        sensors: &SystemSnapshot,
    ) -> Result<EffectRenderOutput> {
        if !group.enabled || group.effect_id.is_none() {
            return Ok(EffectRenderOutput::Cpu(Canvas::new(
                group.layout.canvas_width,
                group.layout.canvas_height,
            )));
        }

        let slot = self.slots.get_mut(&group.id).ok_or_else(|| {
            anyhow!(
                "render group '{}' is not reconciled before rendering",
                group.name
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
    renderer: Box<dyn EffectRenderer>,
    controls: HashMap<String, ControlValue>,
    binding_state: HashMap<String, ActiveBindingState>,
    elapsed_secs: f32,
    frame_number: u64,
}

impl EffectSlot {
    fn build(entry: &EffectEntry, group: &RenderGroup) -> Result<Self> {
        let mut renderer = create_renderer_for_metadata(&entry.metadata)?;
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
            renderer,
            controls: HashMap::new(),
            binding_state: HashMap::new(),
            elapsed_secs: 0.0,
            frame_number: 0,
        };
        slot.sync_group_state(group);
        Ok(slot)
    }

    fn needs_rebuild(&self, effect_id: EffectId, entry: &EffectEntry) -> bool {
        self.effect_id != effect_id
            || self.registry_metadata != entry.metadata
            || self.registry_source_path != entry.source_path
            || self.registry_modified != entry.modified
    }

    fn sync_group_state(&mut self, group: &RenderGroup) {
        let mut desired = HashMap::new();

        for definition in &mut self.metadata.controls {
            let next_binding = group.control_bindings.get(definition.control_id()).cloned();
            if definition.binding != next_binding {
                definition.binding = next_binding;
                self.binding_state.remove(definition.control_id());
            }
            let value = group
                .controls
                .get(definition.control_id())
                .cloned()
                .unwrap_or_else(|| definition.default_value.clone());
            desired.insert(definition.control_id().to_owned(), value);
        }

        for (name, value) in &group.controls {
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

    use super::{EffectPool, EffectSlot};
    use crate::effect::builtin::register_builtin_effects;
    use crate::effect::registry::EffectRegistry;
    use crate::effect::traits::{EffectRenderer, FrameInput};
    use hypercolor_types::canvas::Canvas;
    use hypercolor_types::effect::{EffectCategory, EffectId, EffectMetadata, EffectSource};
    use hypercolor_types::scene::{RenderGroup, RenderGroupId, RenderGroupRole};
    use hypercolor_types::spatial::{
        DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
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
            zones: vec![DeviceZone {
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

    fn render_group(id: RenderGroupId, effect_id: EffectId) -> RenderGroup {
        RenderGroup {
            id,
            name: "Desk".into(),
            description: None,
            effect_id: Some(effect_id),
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layout: sample_layout(),
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: RenderGroupRole::Custom,
            controls_version: 0,
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
        let group_id = RenderGroupId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            group_id,
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        pool.reconcile(&[], &EffectRegistry::new(Vec::new()))
            .expect("prune should succeed");

        assert!(destroyed.load(Ordering::SeqCst));
        assert!(pool.slots.is_empty());
    }

    #[test]
    fn clear_destroys_slots() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let group_id = RenderGroupId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            group_id,
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        pool.clear();

        assert!(destroyed.load(Ordering::SeqCst));
        assert!(pool.slots.is_empty());
    }

    #[test]
    fn reconcile_replacement_destroys_old_slot() {
        let destroyed = Arc::new(AtomicBool::new(false));
        let group_id = RenderGroupId::new();
        let mut pool = EffectPool::new();
        pool.slots.insert(
            group_id,
            spy_slot(EffectId::new(uuid::Uuid::now_v7()), Arc::clone(&destroyed)),
        );

        let registry = registry_with_builtins();
        let solid_id = builtin_effect_id(&registry, "solid_color");
        let group = render_group(group_id, solid_id);

        pool.reconcile(&[group], &registry)
            .expect("replacement should succeed");

        assert!(destroyed.load(Ordering::SeqCst));
        assert_eq!(pool.slots.len(), 1);
    }
}
