//! Lighting-state feed for display faces (`engine.lighting`).
//!
//! Assembles a [`LightingState`] per frame from the scene snapshot and the
//! previous frame's sampled zone colors. Effect names refresh only when the
//! render groups change; dominant colors refresh at 2 Hz from quantized LED
//! output so the set stays stable while effects animate.

use std::sync::Arc;

use hypercolor_core::effect::EffectRegistry;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::lighting::LightingState;
use hypercolor_types::scene::Zone;

const DOMINANT_REFRESH_INTERVAL_MS: u64 = 500;
const DOMINANT_COLOR_COUNT: usize = 3;
const MAX_LED_SAMPLES: usize = 512;
/// Below this r+g+b sum a sample reads as "off" and is skipped.
const NEAR_BLACK_THRESHOLD: u32 = 24;

#[derive(Default)]
pub(crate) struct LightingFeedState {
    effect_names: Vec<String>,
    effect_names_key: Option<(u64, u64)>,
    dominant_colors: Vec<[u8; 3]>,
    last_dominant_sample_ms: Option<u64>,
    current: Option<Arc<LightingState>>,
}

impl LightingFeedState {
    /// Assemble this frame's lighting state, reusing the cached `Arc` while
    /// nothing changed so downstream change detection stays cheap.
    pub(crate) fn lighting_for_frame(
        &mut self,
        scene_name: Option<&str>,
        groups: &[Zone],
        groups_revision: u64,
        registry: &EffectRegistry,
    ) -> Arc<LightingState> {
        let key = (groups_revision, registry.generation());
        if self.effect_names_key != Some(key) {
            self.effect_names = effect_names_for_groups(groups, registry);
            self.effect_names_key = Some(key);
        }

        if let Some(current) = self.current.as_ref()
            && current.scene_name.as_deref() == scene_name
            && current.effect_names == self.effect_names
            && current.dominant_colors == self.dominant_colors
        {
            return Arc::clone(current);
        }

        let state = Arc::new(LightingState {
            scene_name: scene_name.map(str::to_owned),
            effect_names: self.effect_names.clone(),
            dominant_colors: self.dominant_colors.clone(),
        });
        self.current = Some(Arc::clone(&state));
        state
    }

    /// Feed the frame's sampled zone colors; recomputes dominant colors at
    /// most every [`DOMINANT_REFRESH_INTERVAL_MS`].
    pub(crate) fn observe_zones(&mut self, zones: &[ZoneColors], elapsed_ms: u64) {
        if zones.is_empty() {
            return;
        }
        if self
            .last_dominant_sample_ms
            .is_some_and(|last| elapsed_ms.saturating_sub(last) < DOMINANT_REFRESH_INTERVAL_MS)
        {
            return;
        }

        self.last_dominant_sample_ms = Some(elapsed_ms);
        self.dominant_colors = dominant_colors(zones);
    }
}

fn effect_names_for_groups(groups: &[Zone], registry: &EffectRegistry) -> Vec<String> {
    let mut names = Vec::new();
    let push_effect = |effect_id: &hypercolor_types::effect::EffectId, names: &mut Vec<String>| {
        let Some(entry) = registry.get(effect_id) else {
            return;
        };
        let name = entry.metadata.name.clone();
        if !names.contains(&name) {
            names.push(name);
        }
    };
    for group in groups {
        if !group.enabled {
            continue;
        }
        for layer in &group.layers {
            if !layer.enabled {
                continue;
            }
            if let LayerSource::Effect { effect_id, .. } = &layer.source {
                push_effect(effect_id, &mut names);
            }
        }
        // Default-face overlay zones carry their effect on the legacy
        // zone-level field with no layer stack.
        if group.layers.is_empty()
            && let Some(effect_id) = group.effect_id.as_ref()
        {
            push_effect(effect_id, &mut names);
        }
    }
    names
}

/// Pick up to three dominant colors by bucketing LED samples at 4 bits per
/// channel and averaging each of the most-populated buckets.
fn dominant_colors(zones: &[ZoneColors]) -> Vec<[u8; 3]> {
    let total_leds: usize = zones.iter().map(|zone| zone.colors.len()).sum();
    if total_leds == 0 {
        return Vec::new();
    }
    let stride = total_leds.div_ceil(MAX_LED_SAMPLES).max(1);

    struct Bucket {
        count: u32,
        sum: [u32; 3],
    }
    let mut buckets: std::collections::HashMap<u16, Bucket> = std::collections::HashMap::new();
    for (index, [r, g, b]) in zones
        .iter()
        .flat_map(|zone| zone.colors.iter().copied())
        .enumerate()
    {
        if index % stride != 0 {
            continue;
        }
        if u32::from(r) + u32::from(g) + u32::from(b) < NEAR_BLACK_THRESHOLD {
            continue;
        }
        let key = (u16::from(r >> 4) << 8) | (u16::from(g >> 4) << 4) | u16::from(b >> 4);
        let bucket = buckets.entry(key).or_insert(Bucket {
            count: 0,
            sum: [0; 3],
        });
        bucket.count += 1;
        bucket.sum[0] += u32::from(r);
        bucket.sum[1] += u32::from(g);
        bucket.sum[2] += u32::from(b);
    }

    let mut ranked: Vec<(u16, Bucket)> = buckets.into_iter().collect();
    ranked.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(DOMINANT_COLOR_COUNT)
        .map(|(_, bucket)| {
            let count = bucket.count.max(1);
            [
                u8::try_from(bucket.sum[0] / count).unwrap_or(u8::MAX),
                u8::try_from(bucket.sum[1] / count).unwrap_or(u8::MAX),
                u8::try_from(bucket.sum[2] / count).unwrap_or(u8::MAX),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(colors: &[[u8; 3]]) -> ZoneColors {
        ZoneColors {
            zone_id: "test:zone".to_owned(),
            colors: colors.to_vec(),
        }
    }

    #[test]
    fn dominant_colors_rank_by_population_and_skip_black() {
        let zones = vec![zone(&[
            [225, 53, 255],
            [226, 52, 254],
            [128, 255, 234],
            [0, 0, 0],
            [2, 3, 4],
        ])];

        let colors = dominant_colors(&zones);

        assert_eq!(colors.len(), 2);
        assert_eq!(colors[0], [225, 52, 254]);
        assert_eq!(colors[1], [128, 255, 234]);
    }

    #[test]
    fn dominant_colors_cap_at_three() {
        let zones = vec![zone(&[
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [255, 255, 0],
        ])];

        assert_eq!(dominant_colors(&zones).len(), 3);
    }

    #[test]
    fn observe_zones_throttles_to_refresh_interval() {
        let mut feed = LightingFeedState::default();
        feed.observe_zones(&[zone(&[[255, 0, 0]])], 0);
        assert_eq!(feed.dominant_colors, vec![[255, 0, 0]]);

        feed.observe_zones(&[zone(&[[0, 255, 0]])], DOMINANT_REFRESH_INTERVAL_MS - 1);
        assert_eq!(feed.dominant_colors, vec![[255, 0, 0]]);

        feed.observe_zones(&[zone(&[[0, 255, 0]])], DOMINANT_REFRESH_INTERVAL_MS);
        assert_eq!(feed.dominant_colors, vec![[0, 255, 0]]);
    }

    #[test]
    fn lighting_for_frame_reuses_arc_when_unchanged() {
        let mut feed = LightingFeedState::default();
        let registry = EffectRegistry::new(Vec::new());

        let first = feed.lighting_for_frame(Some("Studio"), &[], 1, &registry);
        let second = feed.lighting_for_frame(Some("Studio"), &[], 1, &registry);

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.scene_name.as_deref(), Some("Studio"));

        let renamed = feed.lighting_for_frame(Some("Stage"), &[], 1, &registry);
        assert!(!Arc::ptr_eq(&first, &renamed));
    }
}
