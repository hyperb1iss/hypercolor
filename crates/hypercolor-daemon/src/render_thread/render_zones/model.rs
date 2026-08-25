use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_core::bus::{DisplayZoneFrame, DisplayZoneOutputRoute, DisplayZoneTarget};
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_core::effect::{EffectRegistry, FrameDataSources, InputSourceAvailability};
use hypercolor_core::input::{InteractionData, ScreenBranchPublication};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::audio::AudioData;
#[cfg(test)]
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::event::LayerHealth;
use hypercolor_types::lighting::LightingState;
use hypercolor_types::media::MediaState;
use hypercolor_types::net::NetStats;
use hypercolor_types::scene::{DisplayFaceTarget, SceneId, Zone, ZoneId};
use hypercolor_types::sensor::SystemSnapshot;

use super::super::frame_sampling::{LedSamplingStrategy, RetainedLedSamplingStrategy};
use super::super::producer_queue::ProducerFrame;
use super::super::scene_dependency::SceneDependencyKey;
use crate::performance::FullFrameCopyMetrics;

#[derive(Clone)]
pub(crate) struct PendingDisplayZoneFrame {
    pub frame: ProducerFrame,
    pub display_target: DisplayFaceTarget,
    pub(crate) empty_direct_shell: bool,
}

#[cfg(test)]
impl PendingDisplayZoneFrame {
    pub(super) fn surface_for_test(&self) -> &PublishedSurface {
        match &self.frame {
            ProducerFrame::Surface(surface) => surface,
            ProducerFrame::Canvas(_) => panic!("direct zone test expected a published surface"),
            ProducerFrame::ScreenPublication(_) => {
                panic!("direct zone test expected a published surface")
            }
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => panic!("direct zone test expected a CPU surface"),
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(_) => panic!("direct zone test expected a CPU surface"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DisplayZoneCanvasFrame {
    pub frame: DisplayZoneFrame,
    pub display_target: DisplayZoneTarget,
}

pub(crate) struct ZoneResult {
    pub scene_frame: ProducerFrame,
    pub display_zone_frames: Vec<(ZoneId, PendingDisplayZoneFrame)>,
    pub zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub active_display_zone_ids: Vec<ZoneId>,
    pub led_sampling_strategy: LedSamplingStrategy,
    pub producer_full_frame_copy: FullFrameCopyMetrics,
    pub render_us: u32,
    pub sample_us: u32,
    pub scene_compose_us: u32,
    pub logical_layer_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("GPU scene projection failed without a complete CPU replay or retained scene")]
pub(crate) struct GpuProjectionReplayUnavailable;

#[derive(Clone, Copy)]
pub(crate) struct ZoneFrameInputs<'a> {
    pub(crate) delta_secs: f32,
    pub(crate) audio: &'a AudioData,
    pub(crate) interaction: &'a InteractionData,
    pub(crate) screen: Option<&'a Arc<ScreenBranchPublication>>,
    pub(crate) sensors: &'a SystemSnapshot,
    pub(crate) input_availability: InputSourceAvailability,
    pub(crate) media: Option<&'a MediaState>,
    pub(crate) net: Option<&'a NetStats>,
    pub(crate) lighting: Option<&'a LightingState>,
}

impl<'a> ZoneFrameInputs<'a> {
    pub(crate) fn sources(&self) -> FrameDataSources<'a> {
        FrameDataSources {
            input_availability: self.input_availability,
            media: self.media,
            net: self.net,
            lighting: self.lighting,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RenderSceneContext<'a> {
    pub(crate) zones: &'a [Zone],
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) dependency_key: SceneDependencyKey,
    pub(crate) elapsed_ms: u64,
    pub(crate) display_zone_target_fps: &'a HashMap<ZoneId, u32>,
    pub(crate) display_zone_descriptors: &'a HashMap<ZoneId, DisplayDescriptor>,
    pub(crate) registry: &'a EffectRegistry,
    pub(crate) authoritative_spatial_engine: Option<&'a SpatialEngine>,
    pub(crate) inputs: ZoneFrameInputs<'a>,
}

#[derive(Clone, Copy)]
pub(super) struct ZoneFrameContext<'a> {
    pub(super) active_scene_id: Option<SceneId>,
    pub(super) elapsed_ms: u64,
    pub(super) registry: &'a EffectRegistry,
    pub(super) inputs: ZoneFrameInputs<'a>,
}

impl<'a> RenderSceneContext<'a> {
    pub(super) fn zone_context(&self) -> ZoneFrameContext<'a> {
        ZoneFrameContext {
            active_scene_id: self.active_scene_id,
            elapsed_ms: self.elapsed_ms,
            registry: self.registry,
            inputs: self.inputs,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ZoneFrameRequirements {
    pub(super) requires_cpu_sampling_canvas: bool,
    pub(super) requires_published_surface: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedZoneFrameTarget {
    Scene,
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedZoneResidency {
    Cpu,
    #[cfg_attr(
        not(any(feature = "wgpu", feature = "servo-gpu-import")),
        allow(dead_code)
    )]
    Gpu,
}

impl RenderedZoneResidency {
    fn from_producer_frame(frame: &ProducerFrame) -> Self {
        match frame {
            ProducerFrame::Canvas(_)
            | ProducerFrame::Surface(_)
            | ProducerFrame::ScreenPublication(_) => Self::Cpu,
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => Self::Gpu,
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(_) => Self::Gpu,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedZoneFreshness {
    Fresh,
    Retained,
}

#[derive(Clone)]
enum RenderedZoneFramePayload {
    Scene(ProducerFrame),
    Display(PendingDisplayZoneFrame),
}

#[derive(Clone)]
pub(super) struct RenderedZoneFrame {
    pub(super) zone_id: ZoneId,
    pub(super) target: RenderedZoneFrameTarget,
    pub(super) residency: RenderedZoneResidency,
    pub(super) freshness: RenderedZoneFreshness,
    payload: RenderedZoneFramePayload,
}

pub(super) struct RenderedZoneParts {
    pub(super) display_zone_frames: Vec<(ZoneId, PendingDisplayZoneFrame)>,
    pub(super) zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub(super) active_display_zone_ids: Vec<ZoneId>,
}

#[derive(Default)]
pub(super) struct RenderedZoneSet {
    scene_frames: Vec<RenderedZoneFrame>,
    display_frames: Vec<RenderedZoneFrame>,
    active_display_zone_ids: Vec<ZoneId>,
}

impl RenderedZoneSet {
    pub(super) fn mark_direct_zone_active(&mut self, zone_id: ZoneId) {
        self.active_display_zone_ids.push(zone_id);
    }

    pub(super) fn push_fresh_direct_zone_frame(
        &mut self,
        zone_id: ZoneId,
        frame: PendingDisplayZoneFrame,
    ) {
        self.push_direct_zone_frame(zone_id, frame, RenderedZoneFreshness::Fresh);
    }

    pub(super) fn push_retained_direct_zone_frame(
        &mut self,
        zone_id: ZoneId,
        frame: PendingDisplayZoneFrame,
    ) {
        self.push_direct_zone_frame(zone_id, frame, RenderedZoneFreshness::Retained);
    }

    pub(super) fn push_fresh_scene_zone_frame(&mut self, zone_id: ZoneId, frame: ProducerFrame) {
        let residency = RenderedZoneResidency::from_producer_frame(&frame);
        self.scene_frames.push(RenderedZoneFrame {
            zone_id,
            target: RenderedZoneFrameTarget::Scene,
            residency,
            freshness: RenderedZoneFreshness::Fresh,
            payload: RenderedZoneFramePayload::Scene(frame),
        });
    }

    pub(super) fn into_parts(self) -> RenderedZoneParts {
        let mut parts = RenderedZoneParts {
            display_zone_frames: Vec::new(),
            zone_canvases: Vec::new(),
            active_display_zone_ids: self.active_display_zone_ids,
        };
        for frame in self.scene_frames {
            push_rendered_zone_frame(&mut parts, frame);
        }
        for frame in self.display_frames {
            push_rendered_zone_frame(&mut parts, frame);
        }
        parts
    }

    pub(super) fn into_parts_for_zone_order(self, zones: &[Zone]) -> RenderedZoneParts {
        let mut parts = RenderedZoneParts {
            display_zone_frames: Vec::new(),
            zone_canvases: Vec::new(),
            active_display_zone_ids: self.active_display_zone_ids,
        };
        let mut frames = self.scene_frames;
        frames.extend(self.display_frames);
        for zone in zones {
            while let Some(position) = frames.iter().position(|frame| frame.zone_id == zone.id) {
                push_rendered_zone_frame(&mut parts, frames.remove(position));
            }
        }
        for frame in frames {
            push_rendered_zone_frame(&mut parts, frame);
        }
        parts
    }

    fn push_direct_zone_frame(
        &mut self,
        zone_id: ZoneId,
        frame: PendingDisplayZoneFrame,
        freshness: RenderedZoneFreshness,
    ) {
        let residency = RenderedZoneResidency::from_producer_frame(&frame.frame);
        self.display_frames.push(RenderedZoneFrame {
            zone_id,
            target: RenderedZoneFrameTarget::Display,
            residency,
            freshness,
            payload: RenderedZoneFramePayload::Display(frame),
        });
    }
}

fn push_rendered_zone_frame(parts: &mut RenderedZoneParts, frame: RenderedZoneFrame) {
    let RenderedZoneFrame {
        zone_id,
        target,
        residency,
        freshness,
        payload,
    } = frame;
    debug_assert!(matches!(
        freshness,
        RenderedZoneFreshness::Fresh | RenderedZoneFreshness::Retained
    ));
    match payload {
        RenderedZoneFramePayload::Scene(scene_frame) => {
            debug_assert_eq!(target, RenderedZoneFrameTarget::Scene);
            debug_assert_eq!(
                residency,
                RenderedZoneResidency::from_producer_frame(&scene_frame)
            );
            parts.zone_canvases.push((zone_id, scene_frame));
        }
        RenderedZoneFramePayload::Display(display_frame) => {
            debug_assert_eq!(target, RenderedZoneFrameTarget::Display);
            debug_assert_eq!(
                residency,
                RenderedZoneResidency::from_producer_frame(&display_frame.frame)
            );
            parts
                .zone_canvases
                .push((zone_id, display_frame.frame.clone()));
            parts.display_zone_frames.push((zone_id, display_frame));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("zone '{zone_name}' effect '{effect_name}' ({effect_id}) failed: {error}")]
pub(crate) struct ZoneEffectError {
    pub(crate) effect_id: String,
    pub(crate) effect_name: String,
    pub(crate) zone_id: ZoneId,
    pub(crate) zone_name: String,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(super) struct RetainedRenderZoneFrame {
    pub(super) dependency_key: SceneDependencyKey,
    pub(super) scene_frame: ProducerFrame,
    pub(super) display_zone_frames: Vec<(ZoneId, PendingDisplayZoneFrame)>,
    pub(super) active_display_zone_ids: Vec<ZoneId>,
    pub(super) zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub(super) led_sampling_strategy: RetainedLedSamplingStrategy,
    pub(super) logical_layer_count: u32,
}

#[derive(Clone)]
pub(super) struct RetainedDirectZoneFrame {
    pub(super) frame: PendingDisplayZoneFrame,
    pub(super) rendered_at_ms: u64,
    pub(super) dependency_key: SceneDependencyKey,
}

#[derive(Clone)]
pub(super) struct RetainedMaterializedZoneFrame {
    pub(super) frame: DisplayZoneCanvasFrame,
    pub(super) rendered_at_ms: u64,
    pub(super) dependency_key: SceneDependencyKey,
    pub(super) display_target: DisplayFaceTarget,
    pub(super) display_route: DisplayZoneOutputRoute,
    pub(super) empty_direct_shell: bool,
}

pub(super) struct CachedMediaProducer {
    pub(super) hash_sha256: String,
    pub(super) producer: MediaProducer,
}

pub(super) enum MediaLayerFrame {
    Ready {
        frame: ProducerFrame,
        health: LayerHealth,
    },
    Loading,
    Missing,
    Failed(String),
}
