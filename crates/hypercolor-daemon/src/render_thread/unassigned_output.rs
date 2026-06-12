use std::sync::Arc;

use hypercolor_core::device::BackendManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_core::types::canvas::Canvas;
use hypercolor_types::event::ZoneColors;
use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId};
use hypercolor_types::spatial::{Output, SpatialLayout};

use super::pipeline_runtime::{
    CachedUnassignedOutput, UnassignedOutputCache, UnassignedOutputCacheKey,
};
use super::producer_queue::ProducerFrame;

pub(crate) struct UnassignedOutputPlanner<'a> {
    manager: &'a BackendManager,
    cache: &'a mut UnassignedOutputCache,
}

impl<'a> UnassignedOutputPlanner<'a> {
    pub(crate) fn new(manager: &'a BackendManager, cache: &'a mut UnassignedOutputCache) -> Self {
        Self { manager, cache }
    }

    pub(crate) fn plan(
        &mut self,
        layout: Arc<SpatialLayout>,
        behavior: &UnassignedBehavior,
        groups: &[Zone],
        zone_canvases: &[(ZoneId, ProducerFrame)],
    ) -> UnassignedOutputPlan {
        if matches!(behavior, UnassignedBehavior::Hold) {
            return UnassignedOutputPlan {
                layout,
                appended_zones: UnassignedOutputZones::None,
            };
        }

        let cached = self.cached_unassigned_outputs(layout);
        if cached.zones.is_empty() {
            return UnassignedOutputPlan {
                layout: Arc::clone(&cached.source_layout),
                appended_zones: UnassignedOutputZones::None,
            };
        }

        let appended_zones = match behavior {
            UnassignedBehavior::Off => {
                UnassignedOutputZones::Cached(Arc::clone(&cached.black_zones))
            }
            UnassignedBehavior::Hold => UnassignedOutputZones::None,
            UnassignedBehavior::Fallback(group_id) => {
                let zones = fallback_zone_colors(*group_id, groups, zone_canvases, &cached.zones)
                    .unwrap_or_else(|| cached.black_zones.iter().cloned().collect());
                UnassignedOutputZones::Owned(zones)
            }
        };

        UnassignedOutputPlan {
            layout: Arc::new(layout_with_unassigned_zones(
                cached.source_layout.as_ref(),
                &cached.zones,
            )),
            appended_zones,
        }
    }

    fn cached_unassigned_outputs(&mut self, layout: Arc<SpatialLayout>) -> CachedUnassignedOutput {
        let key = UnassignedOutputCacheKey::new(&layout, self.manager.routing_mapping_generation());
        if let Some(cached) = self.cache.get(key) {
            return cached;
        }

        let unassigned_zones = self.manager.unassigned_output_zones(layout.as_ref());
        let black_zones = black_zone_colors(&unassigned_zones).into();
        self.cache.store(
            key,
            CachedUnassignedOutput {
                source_layout: layout,
                zones: unassigned_zones.into(),
                black_zones,
            },
        )
    }
}

pub(crate) struct UnassignedOutputPlan {
    layout: Arc<SpatialLayout>,
    appended_zones: UnassignedOutputZones,
}

impl UnassignedOutputPlan {
    pub(crate) fn layout(&self) -> &SpatialLayout {
        self.layout.as_ref()
    }

    pub(crate) fn zones_for(&self, base: &[ZoneColors]) -> Vec<ZoneColors> {
        if self.appended_zones.is_empty() {
            return base.to_vec();
        }

        let mut zones = Vec::with_capacity(base.len().saturating_add(self.appended_zones.len()));
        zones.extend_from_slice(base);
        zones.extend_from_slice(self.appended_zones.as_slice());
        zones
    }
}

enum UnassignedOutputZones {
    None,
    Cached(Arc<[ZoneColors]>),
    Owned(Vec<ZoneColors>),
}

impl UnassignedOutputZones {
    fn is_empty(&self) -> bool {
        match self {
            Self::None => true,
            Self::Cached(zones) => zones.is_empty(),
            Self::Owned(zones) => zones.is_empty(),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Cached(zones) => zones.len(),
            Self::Owned(zones) => zones.len(),
        }
    }

    fn as_slice(&self) -> &[ZoneColors] {
        match self {
            Self::None => &[],
            Self::Cached(zones) => zones,
            Self::Owned(zones) => zones,
        }
    }
}

fn layout_with_unassigned_zones(
    layout: &SpatialLayout,
    unassigned_zones: &[Output],
) -> SpatialLayout {
    let mut zones = Vec::with_capacity(layout.zones.len().saturating_add(unassigned_zones.len()));
    zones.extend_from_slice(&layout.zones);
    zones.extend_from_slice(unassigned_zones);

    SpatialLayout {
        id: layout.id.clone(),
        name: layout.name.clone(),
        description: layout.description.clone(),
        canvas_width: layout.canvas_width,
        canvas_height: layout.canvas_height,
        zones,
        default_sampling_mode: layout.default_sampling_mode.clone(),
        default_edge_behavior: layout.default_edge_behavior,
        spaces: layout.spaces.clone(),
        version: layout.version,
    }
}

fn black_zone_colors(zones: &[Output]) -> Vec<ZoneColors> {
    zones
        .iter()
        .map(|zone| ZoneColors {
            zone_id: zone.id.clone(),
            colors: vec![[0, 0, 0]; usize::try_from(zone.topology.led_count()).unwrap_or_default()],
        })
        .collect()
}

fn fallback_zone_colors(
    fallback_group_id: ZoneId,
    groups: &[Zone],
    zone_canvases: &[(ZoneId, ProducerFrame)],
    unassigned_zones: &[Output],
) -> Option<Vec<ZoneColors>> {
    let fallback_group = groups
        .iter()
        .find(|group| group.id == fallback_group_id && group.display_target.is_none())?;
    let fallback_canvas = zone_canvases
        .iter()
        .find(|(group_id, _)| *group_id == fallback_group_id)
        .and_then(|(_, frame)| producer_frame_canvas(frame))?;
    let mut fallback_layout = fallback_group.layout.clone();
    fallback_layout.zones = unassigned_zones.to_vec();
    fallback_layout.canvas_width = fallback_canvas.width();
    fallback_layout.canvas_height = fallback_canvas.height();

    Some(SpatialEngine::new(fallback_layout).sample(&fallback_canvas))
}

#[cfg_attr(
    not(any(feature = "wgpu", feature = "servo-gpu-import")),
    expect(
        clippy::unnecessary_wraps,
        reason = "the return type stays feature-stable because GPU frames cannot be materialized on the CPU"
    )
)]
fn producer_frame_canvas(frame: &ProducerFrame) -> Option<Canvas> {
    match frame {
        ProducerFrame::Canvas(canvas) => Some(canvas.clone()),
        ProducerFrame::Surface(surface) => Some(Canvas::from_published_surface(surface)),
        #[cfg(feature = "servo-gpu-import")]
        ProducerFrame::Gpu(_) => {
            frame.record_cpu_materialization_blocked();
            None
        }
        #[cfg(feature = "wgpu")]
        ProducerFrame::GpuTexture(_) => {
            frame.record_cpu_materialization_blocked();
            None
        }
    }
}

pub(crate) fn unassigned_behavior_generation(behavior: &UnassignedBehavior) -> u64 {
    match behavior {
        UnassignedBehavior::Off => 0,
        UnassignedBehavior::Hold => 1,
        UnassignedBehavior::Fallback(group_id) => {
            let raw = group_id.0.as_u128();
            2 ^ ((raw >> 64) as u64) ^ (raw as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use hypercolor_core::device::{BackendManager, SegmentRange};
    use hypercolor_core::types::canvas::Canvas;
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::scene::{UnassignedBehavior, Zone, ZoneId, ZoneRole};
    use hypercolor_types::spatial::{
        EdgeBehavior, LedTopology, NormalizedPosition, Output, SamplingMode, SpatialLayout,
    };

    use super::UnassignedOutputPlanner;
    use crate::render_thread::pipeline_runtime::UnassignedOutputCache;
    use crate::render_thread::producer_queue::ProducerFrame;

    fn sample_layout(zone_ids: &[&str]) -> SpatialLayout {
        SpatialLayout {
            id: "layout".to_owned(),
            name: "layout".to_owned(),
            description: None,
            canvas_width: 1,
            canvas_height: 1,
            zones: zone_ids
                .iter()
                .map(|zone_id| Output {
                    id: (*zone_id).to_owned(),
                    name: (*zone_id).to_owned(),
                    device_id: "device".to_owned(),
                    zone_name: None,
                    position: NormalizedPosition::new(0.5, 0.5),
                    size: NormalizedPosition::new(1.0, 1.0),
                    rotation: 0.0,
                    scale: 1.0,
                    display_order: 0,
                    orientation: None,
                    topology: LedTopology::Point,
                    led_positions: vec![NormalizedPosition::new(0.5, 0.5)],
                    led_mapping: None,
                    sampling_mode: Some(SamplingMode::Nearest),
                    edge_behavior: Some(EdgeBehavior::Clamp),
                    shape: None,
                    shape_preset: None,
                    attachment: None,
                    brightness: None,
                })
                .collect(),
            default_sampling_mode: SamplingMode::Nearest,
            default_edge_behavior: EdgeBehavior::Clamp,
            spaces: None,
            version: 1,
        }
    }

    fn render_group(id: ZoneId, layout: SpatialLayout) -> Zone {
        Zone {
            id,
            name: "fallback".to_owned(),
            description: None,
            effect_id: None,
            controls: HashMap::new(),
            control_bindings: HashMap::new(),
            preset_id: None,
            layers: Vec::new(),
            layout,
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
    fn unassigned_output_plan_appends_black_zones_for_off_behavior() {
        let mut manager = BackendManager::new();
        let device_id = DeviceId::new();
        manager.map_device_with_segment(
            "usb:unassigned",
            "mock",
            device_id,
            Some(SegmentRange::new(0, 2)),
        );

        let mut cache = UnassignedOutputCache::default();
        let plan = UnassignedOutputPlanner::new(&manager, &mut cache).plan(
            Arc::new(sample_layout(&[])),
            &UnassignedBehavior::Off,
            &[],
            &[],
        );
        let zones = plan.zones_for(&[]);

        assert_eq!(plan.layout().zones.len(), 1);
        assert_eq!(plan.layout().zones[0].device_id, "usb:unassigned");
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].colors, vec![[0, 0, 0]; 2]);
    }

    #[test]
    fn unassigned_output_cache_reuses_zone_metadata_for_stable_mapping() {
        let mut manager = BackendManager::new();
        let device_id = DeviceId::new();
        manager.map_device_with_segment(
            "usb:unassigned",
            "mock",
            device_id,
            Some(SegmentRange::new(0, 2)),
        );

        let layout = Arc::new(sample_layout(&[]));
        let mut cache = UnassignedOutputCache::default();
        let first = UnassignedOutputPlanner::new(&manager, &mut cache)
            .cached_unassigned_outputs(Arc::clone(&layout));
        let second =
            UnassignedOutputPlanner::new(&manager, &mut cache).cached_unassigned_outputs(layout);

        assert!(Arc::ptr_eq(&first.zones, &second.zones));
        assert!(Arc::ptr_eq(&first.black_zones, &second.black_zones));
    }

    #[test]
    fn unassigned_output_plan_samples_fallback_group_canvas() {
        let mut manager = BackendManager::new();
        let device_id = DeviceId::new();
        manager.map_device_with_segment(
            "usb:unassigned",
            "mock",
            device_id,
            Some(SegmentRange::new(0, 2)),
        );

        let group_id = ZoneId::new();
        let fallback_canvas = Canvas::from_rgba(&[255, 0, 0, 255], 1, 1);
        let groups = vec![render_group(group_id, sample_layout(&[]))];
        let zone_canvases = vec![(group_id, ProducerFrame::Canvas(fallback_canvas))];
        let mut cache = UnassignedOutputCache::default();
        let plan = UnassignedOutputPlanner::new(&manager, &mut cache).plan(
            Arc::new(sample_layout(&[])),
            &UnassignedBehavior::Fallback(group_id),
            &groups,
            &zone_canvases,
        );
        let zones = plan.zones_for(&[]);

        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].colors, vec![[255, 0, 0]; 2]);
    }
}
