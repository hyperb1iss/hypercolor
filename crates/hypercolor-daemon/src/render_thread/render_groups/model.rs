use std::collections::HashMap;

use hypercolor_core::bus::{DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget};
use hypercolor_core::effect::media::MediaProducer;
use hypercolor_core::effect::{EffectRegistry, FrameDataSources};
use hypercolor_core::input::{InteractionData, ScreenData};
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
pub(crate) struct PendingGroupCanvasFrame {
    pub frame: ProducerFrame,
    pub display_target: DisplayFaceTarget,
    pub(crate) empty_direct_shell: bool,
}

#[cfg(test)]
impl PendingGroupCanvasFrame {
    pub(super) fn surface_for_test(&self) -> &PublishedSurface {
        match &self.frame {
            ProducerFrame::Surface(surface) => surface,
            ProducerFrame::Canvas(_) => panic!("direct group test expected a published surface"),
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => panic!("direct group test expected a CPU surface"),
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(_) => panic!("direct group test expected a CPU surface"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct GroupCanvasFrame {
    pub frame: DisplayGroupFrame,
    pub display_target: DisplayGroupTarget,
}

pub(crate) struct ZoneResult {
    pub scene_frame: ProducerFrame,
    pub group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
    pub zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub active_group_canvas_ids: Vec<ZoneId>,
    pub led_sampling_strategy: LedSamplingStrategy,
    pub producer_full_frame_copy: FullFrameCopyMetrics,
    pub render_us: u32,
    pub sample_us: u32,
    pub scene_compose_us: u32,
    pub logical_layer_count: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct ZoneFrameInputs<'a> {
    pub(crate) delta_secs: f32,
    pub(crate) audio: &'a AudioData,
    pub(crate) interaction: &'a InteractionData,
    pub(crate) screen: Option<&'a ScreenData>,
    pub(crate) sensors: &'a SystemSnapshot,
    pub(crate) media: Option<&'a MediaState>,
    pub(crate) net: Option<&'a NetStats>,
    pub(crate) lighting: Option<&'a LightingState>,
}

impl<'a> ZoneFrameInputs<'a> {
    pub(crate) fn sources(&self) -> FrameDataSources<'a> {
        FrameDataSources {
            media: self.media,
            net: self.net,
            lighting: self.lighting,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RenderSceneContext<'a> {
    pub(crate) groups: &'a [Zone],
    pub(crate) active_scene_id: Option<SceneId>,
    pub(crate) dependency_key: SceneDependencyKey,
    pub(crate) elapsed_ms: u32,
    pub(crate) display_group_target_fps: &'a HashMap<ZoneId, u32>,
    pub(crate) display_group_descriptors: &'a HashMap<ZoneId, DisplayDescriptor>,
    pub(crate) registry: &'a EffectRegistry,
    pub(crate) inputs: ZoneFrameInputs<'a>,
}

#[derive(Clone, Copy)]
pub(super) struct GroupFrameContext<'a> {
    pub(super) active_scene_id: Option<SceneId>,
    pub(super) elapsed_ms: u32,
    pub(super) registry: &'a EffectRegistry,
    pub(super) inputs: ZoneFrameInputs<'a>,
}

impl<'a> RenderSceneContext<'a> {
    pub(super) fn group_context(&self) -> GroupFrameContext<'a> {
        GroupFrameContext {
            active_scene_id: self.active_scene_id,
            elapsed_ms: self.elapsed_ms,
            registry: self.registry,
            inputs: self.inputs,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct GroupFrameRequirements {
    pub(super) requires_cpu_sampling_canvas: bool,
    pub(super) requires_published_surface: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedGroupFrameTarget {
    Scene,
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedGroupResidency {
    Cpu,
    #[cfg_attr(
        not(any(feature = "wgpu", feature = "servo-gpu-import")),
        allow(dead_code)
    )]
    Gpu,
}

impl RenderedGroupResidency {
    fn from_producer_frame(frame: &ProducerFrame) -> Self {
        match frame {
            ProducerFrame::Canvas(_) | ProducerFrame::Surface(_) => Self::Cpu,
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(_) => Self::Gpu,
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(_) => Self::Gpu,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedGroupFreshness {
    Fresh,
    Retained,
}

#[derive(Clone)]
enum RenderedGroupFramePayload {
    Scene(ProducerFrame),
    Display(PendingGroupCanvasFrame),
}

#[derive(Clone)]
pub(super) struct RenderedGroupFrame {
    pub(super) group_id: ZoneId,
    pub(super) target: RenderedGroupFrameTarget,
    pub(super) residency: RenderedGroupResidency,
    pub(super) freshness: RenderedGroupFreshness,
    payload: RenderedGroupFramePayload,
}

pub(super) struct RenderedGroupParts {
    pub(super) group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
    pub(super) zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub(super) active_group_canvas_ids: Vec<ZoneId>,
}

#[derive(Default)]
pub(super) struct RenderedGroupSet {
    scene_frames: Vec<RenderedGroupFrame>,
    display_frames: Vec<RenderedGroupFrame>,
    active_group_canvas_ids: Vec<ZoneId>,
}

impl RenderedGroupSet {
    pub(super) fn mark_direct_group_active(&mut self, group_id: ZoneId) {
        self.active_group_canvas_ids.push(group_id);
    }

    pub(super) fn push_fresh_direct_group_frame(
        &mut self,
        group_id: ZoneId,
        frame: PendingGroupCanvasFrame,
    ) {
        self.push_direct_group_frame(group_id, frame, RenderedGroupFreshness::Fresh);
    }

    pub(super) fn push_retained_direct_group_frame(
        &mut self,
        group_id: ZoneId,
        frame: PendingGroupCanvasFrame,
    ) {
        self.push_direct_group_frame(group_id, frame, RenderedGroupFreshness::Retained);
    }

    pub(super) fn push_fresh_scene_group_frame(&mut self, group_id: ZoneId, frame: ProducerFrame) {
        let residency = RenderedGroupResidency::from_producer_frame(&frame);
        self.scene_frames.push(RenderedGroupFrame {
            group_id,
            target: RenderedGroupFrameTarget::Scene,
            residency,
            freshness: RenderedGroupFreshness::Fresh,
            payload: RenderedGroupFramePayload::Scene(frame),
        });
    }

    pub(super) fn into_parts(self) -> RenderedGroupParts {
        let mut parts = RenderedGroupParts {
            group_canvases: Vec::new(),
            zone_canvases: Vec::new(),
            active_group_canvas_ids: self.active_group_canvas_ids,
        };
        for frame in self.scene_frames {
            push_rendered_group_frame(&mut parts, frame);
        }
        for frame in self.display_frames {
            push_rendered_group_frame(&mut parts, frame);
        }
        parts
    }

    pub(super) fn into_parts_for_group_order(self, groups: &[Zone]) -> RenderedGroupParts {
        let mut parts = RenderedGroupParts {
            group_canvases: Vec::new(),
            zone_canvases: Vec::new(),
            active_group_canvas_ids: self.active_group_canvas_ids,
        };
        let mut frames = self.scene_frames;
        frames.extend(self.display_frames);
        for group in groups {
            while let Some(position) = frames.iter().position(|frame| frame.group_id == group.id) {
                push_rendered_group_frame(&mut parts, frames.remove(position));
            }
        }
        for frame in frames {
            push_rendered_group_frame(&mut parts, frame);
        }
        parts
    }

    fn push_direct_group_frame(
        &mut self,
        group_id: ZoneId,
        frame: PendingGroupCanvasFrame,
        freshness: RenderedGroupFreshness,
    ) {
        let residency = RenderedGroupResidency::from_producer_frame(&frame.frame);
        self.display_frames.push(RenderedGroupFrame {
            group_id,
            target: RenderedGroupFrameTarget::Display,
            residency,
            freshness,
            payload: RenderedGroupFramePayload::Display(frame),
        });
    }
}

fn push_rendered_group_frame(parts: &mut RenderedGroupParts, frame: RenderedGroupFrame) {
    let RenderedGroupFrame {
        group_id,
        target,
        residency,
        freshness,
        payload,
    } = frame;
    debug_assert!(matches!(
        freshness,
        RenderedGroupFreshness::Fresh | RenderedGroupFreshness::Retained
    ));
    match payload {
        RenderedGroupFramePayload::Scene(scene_frame) => {
            debug_assert_eq!(target, RenderedGroupFrameTarget::Scene);
            debug_assert_eq!(
                residency,
                RenderedGroupResidency::from_producer_frame(&scene_frame)
            );
            parts.zone_canvases.push((group_id, scene_frame));
        }
        RenderedGroupFramePayload::Display(display_frame) => {
            debug_assert_eq!(target, RenderedGroupFrameTarget::Display);
            debug_assert_eq!(
                residency,
                RenderedGroupResidency::from_producer_frame(&display_frame.frame)
            );
            parts
                .zone_canvases
                .push((group_id, display_frame.frame.clone()));
            parts.group_canvases.push((group_id, display_frame));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("zone '{group_name}' effect '{effect_name}' ({effect_id}) failed: {error}")]
pub(crate) struct ZoneEffectError {
    pub(crate) effect_id: String,
    pub(crate) effect_name: String,
    pub(crate) group_id: ZoneId,
    pub(crate) group_name: String,
    pub(crate) error: String,
}

#[derive(Clone)]
pub(super) struct RetainedRenderGroupFrame {
    pub(super) dependency_key: SceneDependencyKey,
    pub(super) scene_frame: ProducerFrame,
    pub(super) group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
    pub(super) active_group_canvas_ids: Vec<ZoneId>,
    pub(super) zone_canvases: Vec<(ZoneId, ProducerFrame)>,
    pub(super) led_sampling_strategy: RetainedLedSamplingStrategy,
    pub(super) logical_layer_count: u32,
}

#[derive(Clone)]
pub(super) struct RetainedDirectGroupFrame {
    pub(super) frame: PendingGroupCanvasFrame,
    pub(super) rendered_at_ms: u32,
    pub(super) dependency_key: SceneDependencyKey,
}

#[derive(Clone)]
pub(super) struct RetainedMaterializedGroupFrame {
    pub(super) frame: GroupCanvasFrame,
    pub(super) rendered_at_ms: u32,
    pub(super) dependency_key: SceneDependencyKey,
    pub(super) display_target: DisplayFaceTarget,
    pub(super) display_route: DisplayGroupOutputRoute,
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
