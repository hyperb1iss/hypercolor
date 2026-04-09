use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow};

use hypercolor_types::audio::AudioData;
use hypercolor_types::canvas::Canvas;
use hypercolor_types::effect::{ControlValue, EffectId, EffectMetadata};
use hypercolor_types::scene::{RenderGroup, RenderGroupId};

use super::factory::create_renderer_for_metadata;
use super::registry::EffectRegistry;
use super::traits::{EffectRenderer, FrameInput, prepare_target_canvas};
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

            let needs_rebuild = self
                .slots
                .get(&group.id)
                .is_none_or(|slot| slot.effect_id != effect_id);
            if needs_rebuild {
                let metadata = lookup_effect_metadata(registry, effect_id)?;
                let slot = EffectSlot::build(metadata, &group.controls)?;
                self.slots.insert(group.id, slot);
                continue;
            }

            if let Some(slot) = self.slots.get_mut(&group.id) {
                slot.sync_controls(&group.controls);
            }
        }

        Ok(())
    }

    pub fn render_group_into(
        &mut self,
        group: &RenderGroup,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
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
            group.layout.canvas_width,
            group.layout.canvas_height,
            target,
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
    metadata: EffectMetadata,
    renderer: Box<dyn EffectRenderer>,
    controls: HashMap<String, ControlValue>,
    elapsed_secs: f32,
    frame_number: u64,
}

impl EffectSlot {
    fn build(
        metadata: EffectMetadata,
        group_controls: &HashMap<String, ControlValue>,
    ) -> Result<Self> {
        let mut renderer = create_renderer_for_metadata(&metadata)?;
        renderer.init(&metadata)?;

        let mut slot = Self {
            effect_id: metadata.id,
            metadata,
            renderer,
            controls: HashMap::new(),
            elapsed_secs: 0.0,
            frame_number: 0,
        };
        slot.sync_controls(group_controls);
        Ok(slot)
    }

    fn sync_controls(&mut self, group_controls: &HashMap<String, ControlValue>) {
        let mut desired = HashMap::new();

        for definition in &self.metadata.controls {
            let value = group_controls
                .get(definition.control_id())
                .cloned()
                .unwrap_or_else(|| definition.default_value.clone());
            desired.insert(definition.control_id().to_owned(), value);
        }

        for (name, value) in group_controls {
            desired.entry(name.clone()).or_insert_with(|| value.clone());
        }

        for (name, value) in &desired {
            if self.controls.get(name) != Some(value) {
                self.renderer.set_control(name, value);
            }
        }

        self.controls = desired;
    }

    fn render_into(
        &mut self,
        delta_secs: f32,
        audio: &AudioData,
        interaction: &InteractionData,
        screen: Option<&ScreenData>,
        canvas_width: u32,
        canvas_height: u32,
        target: &mut Canvas,
    ) -> Result<()> {
        self.elapsed_secs += delta_secs;
        let input = FrameInput {
            time_secs: self.elapsed_secs,
            delta_secs,
            frame_number: self.frame_number,
            audio,
            interaction,
            screen,
            canvas_width,
            canvas_height,
        };
        self.renderer.render_into(&input, target)?;
        self.frame_number = self.frame_number.wrapping_add(1);
        Ok(())
    }
}

fn lookup_effect_metadata(
    registry: &EffectRegistry,
    effect_id: EffectId,
) -> Result<EffectMetadata> {
    registry
        .get(&effect_id)
        .map(|entry| entry.metadata.clone())
        .ok_or_else(|| anyhow!("effect '{effect_id}' is not registered"))
}
