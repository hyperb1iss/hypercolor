use std::collections::HashMap;

#[cfg(feature = "wgpu")]
use tracing::debug;

use hypercolor_core::bus::{DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget};
use hypercolor_core::types::canvas::PublishedSurface;
#[cfg(any(feature = "wgpu", test))]
use hypercolor_types::device::DisplayFrameFormat;
use hypercolor_types::scene::{DisplayFaceTarget, ZoneId};

use super::pipeline_runtime::ComposeRuntime;
#[cfg(feature = "wgpu")]
use super::pipeline_runtime::PendingDisplayFinalizeWork;
use super::producer_queue::ProducerFrame;
use super::render_groups::{GroupCanvasFrame, PendingGroupCanvasFrame};
use super::scene_dependency::SceneDependencyKey;
#[cfg(feature = "wgpu")]
use super::sparkleflinger::{
    DisplayFinalizeCacheKey, DisplayFinalizeDispatch, DisplayFinalizeFrame, DisplayFinalizeParams,
};

pub(super) struct DisplayLaneRoutes<'a> {
    pub(super) current: &'a HashMap<ZoneId, DisplayGroupOutputRoute>,
    pub(super) fallback: &'a HashMap<ZoneId, DisplayGroupOutputRoute>,
}

impl DisplayLaneRoutes<'_> {
    fn route_for_group(&self, group_id: &ZoneId) -> Option<&DisplayGroupOutputRoute> {
        self.current
            .get(group_id)
            .or_else(|| self.fallback.get(group_id))
    }
}

pub(super) struct DisplayLaneContext<'a> {
    pub(super) elapsed_ms: u32,
    pub(super) dependency_key: SceneDependencyKey,
    pub(super) target_fps: &'a HashMap<ZoneId, u32>,
    pub(super) routes: DisplayLaneRoutes<'a>,
}

pub(super) struct DisplayLaneMaterializer<'a, 'runtime> {
    compose: &'a mut ComposeRuntime<'runtime>,
    context: DisplayLaneContext<'a>,
}

impl<'a, 'runtime> DisplayLaneMaterializer<'a, 'runtime> {
    pub(super) fn new(
        compose: &'a mut ComposeRuntime<'runtime>,
        context: DisplayLaneContext<'a>,
    ) -> Self {
        Self { compose, context }
    }

    pub(super) fn materialize_group_canvases(
        &mut self,
        active_group_ids: &[ZoneId],
        group_canvases: Vec<(ZoneId, PendingGroupCanvasFrame)>,
        scene_frame: &ProducerFrame,
    ) -> Vec<(ZoneId, GroupCanvasFrame)> {
        #[cfg(feature = "wgpu")]
        self.compose
            .discard_display_finalizations_except(active_group_ids);
        #[cfg(not(feature = "wgpu"))]
        let _ = active_group_ids;

        group_canvases
            .into_iter()
            .filter_map(|(group_id, frame)| {
                let display_route = self.context.routes.route_for_group(&group_id).cloned();
                let display_target = frame.display_target.clone();
                let empty_direct_shell = frame.empty_direct_shell;
                if let Some(route) = display_route.as_ref()
                    && let Some(frame) = self
                        .compose
                        .render_group_runtime
                        .reuse_retained_materialized_group_frame(
                            group_id,
                            self.context.elapsed_ms,
                            self.context.target_fps.get(&group_id).copied(),
                            self.context.dependency_key,
                            &display_target,
                            route,
                            empty_direct_shell,
                        )
                {
                    return Some((group_id, frame));
                }

                let (materialized, fresh_materialization) = if let Some(materialized) = self
                    .materialize_group_canvas(group_id, frame, scene_frame, display_route.as_ref())
                {
                    (materialized, true)
                } else {
                    let retained = display_route.as_ref().and_then(|route| {
                        self.compose
                            .render_group_runtime
                            .reuse_latest_materialized_group_frame(
                                group_id,
                                &display_target,
                                route,
                                empty_direct_shell,
                            )
                    })?;
                    #[cfg(feature = "wgpu")]
                    crate::render_thread::sparkleflinger::gpu::record_gpu_display_finalize_latch();
                    (retained, false)
                };
                if fresh_materialization && let Some(route) = display_route.as_ref() {
                    self.compose
                        .render_group_runtime
                        .retain_materialized_group_frame(
                            group_id,
                            self.context.elapsed_ms,
                            self.context.dependency_key,
                            &display_target,
                            route,
                            empty_direct_shell,
                            &materialized,
                        );
                }

                Some((group_id, materialized))
            })
            .collect()
    }

    fn materialize_group_canvas(
        &mut self,
        group_id: ZoneId,
        group_canvas: PendingGroupCanvasFrame,
        scene_frame: &ProducerFrame,
        display_route: Option<&DisplayGroupOutputRoute>,
    ) -> Option<GroupCanvasFrame> {
        let PendingGroupCanvasFrame {
            frame,
            display_target,
            ..
        } = group_canvas;
        if let Some(frame) = self.finalize_display_group_canvas(
            group_id,
            scene_frame,
            &frame,
            &display_target,
            display_route,
        ) {
            return Some(GroupCanvasFrame {
                frame,
                display_target: DisplayGroupTarget {
                    device_id: display_target.device_id,
                    blend_mode: display_target.blend_mode,
                    opacity: display_target.opacity,
                    finalized: true,
                },
            });
        }
        if display_route_matches_target(display_route, &display_target) {
            return None;
        }

        let surface = match frame {
            ProducerFrame::Canvas(canvas) => PublishedSurface::from_owned_canvas(canvas, 0, 0),
            ProducerFrame::Surface(surface) => surface,
            #[cfg(feature = "servo-gpu-import")]
            ProducerFrame::Gpu(frame) => self
                .compose
                .display_sparkleflinger
                .materialize_output_surface(ProducerFrame::Gpu(frame))?,
            #[cfg(feature = "wgpu")]
            ProducerFrame::GpuTexture(frame) => self
                .compose
                .display_sparkleflinger
                .materialize_output_surface(ProducerFrame::GpuTexture(frame))?,
        };

        Some(GroupCanvasFrame {
            frame: DisplayGroupFrame::from_surface(surface),
            display_target: (&display_target).into(),
        })
    }

    fn finalize_display_group_canvas(
        &mut self,
        group_id: ZoneId,
        scene_frame: &ProducerFrame,
        face_frame: &ProducerFrame,
        display_target: &DisplayFaceTarget,
        display_route: Option<&DisplayGroupOutputRoute>,
    ) -> Option<DisplayGroupFrame> {
        let Some(display_route) =
            display_route.filter(|route| route.device_id == display_target.device_id)
        else {
            return None;
        };

        self.finalize_display_group_canvas_with_route(
            group_id,
            scene_frame,
            face_frame,
            display_target,
            display_route,
        )
    }

    #[cfg(not(feature = "wgpu"))]
    fn finalize_display_group_canvas_with_route(
        &mut self,
        group_id: ZoneId,
        scene_frame: &ProducerFrame,
        face_frame: &ProducerFrame,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
    ) -> Option<DisplayGroupFrame> {
        let _ = group_id;
        let _ = scene_frame;
        let _ = face_frame;
        let _ = display_target;
        let _ = display_route;
        None
    }

    #[cfg(feature = "wgpu")]
    fn finalize_display_group_canvas_with_route(
        &mut self,
        group_id: ZoneId,
        scene_frame: &ProducerFrame,
        face_frame: &ProducerFrame,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
    ) -> Option<DisplayGroupFrame> {
        let params = DisplayFinalizeParams {
            cache_key: DisplayFinalizeCacheKey {
                group_id,
                device_id: display_route.device_id,
                width: display_route.width,
                height: display_route.height,
                circular: display_route.circular,
                frame_format: display_route.frame_format,
            },
            width: display_route.width,
            height: display_route.height,
            circular: display_route.circular,
            brightness: display_route.brightness,
            viewport_position: display_route.viewport.position,
            viewport_size: display_route.viewport.size,
            viewport_rotation: display_route.viewport.rotation,
            viewport_scale: display_route.viewport.scale,
            viewport_edge_behavior: display_route.viewport.edge_behavior,
            blend_mode: display_target.blend_mode,
            opacity: display_target.opacity,
        };

        let completed = match self.finish_pending_display_finalize_work(
            group_id,
            display_target,
            display_route,
        ) {
            DisplayFinalizeProgress::Ready(frame) => Some(frame),
            DisplayFinalizeProgress::Pending => return None,
            DisplayFinalizeProgress::Idle => None,
        };

        let dispatch = if display_route.frame_format == DisplayFrameFormat::Jpeg {
            self.compose
                .display_sparkleflinger
                .begin_finalize_display_face_yuv420(scene_frame, face_frame, params)
        } else {
            self.compose
                .display_sparkleflinger
                .begin_finalize_display_face(scene_frame, face_frame, params)
        };

        match dispatch {
            Ok(DisplayFinalizeDispatch::Pending(pending)) => {
                let mut work = PendingDisplayFinalizeWork {
                    dependency_key: self.context.dependency_key,
                    display_target: display_target.clone(),
                    display_route: display_route.clone(),
                    frame_format: display_route.frame_format,
                    pending,
                };
                if let Some(frame) = self.try_finish_display_finalize_work(&mut work) {
                    return Some(frame);
                }
                self.compose.display_finalize_runtime.insert(group_id, work);
                None
            }
            Ok(
                dispatch @ (DisplayFinalizeDispatch::Unsupported
                | DisplayFinalizeDispatch::Saturated),
            ) => {
                debug_assert!(display_finalize_dispatch_reuses_retained_frame(&dispatch));
                None
            }
            Err(error) => {
                debug!(%error, "GPU display-face finalization deferred to retained frame");
                None
            }
        }
        .or(completed)
    }

    #[cfg(feature = "wgpu")]
    fn finish_pending_display_finalize_work(
        &mut self,
        group_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
    ) -> DisplayFinalizeProgress {
        let Some(mut work) = self.compose.display_finalize_runtime.take(group_id) else {
            return DisplayFinalizeProgress::Idle;
        };
        if !work.matches(
            self.context.dependency_key,
            display_target,
            display_route,
            display_route.frame_format,
        ) {
            self.compose
                .display_sparkleflinger
                .discard_pending_display_finalization(work.pending);
            return DisplayFinalizeProgress::Idle;
        }

        if let Some(frame) = self.try_finish_display_finalize_work(&mut work) {
            DisplayFinalizeProgress::Ready(frame)
        } else {
            self.compose.display_finalize_runtime.insert(group_id, work);
            DisplayFinalizeProgress::Pending
        }
    }

    #[cfg(feature = "wgpu")]
    fn try_finish_display_finalize_work(
        &mut self,
        work: &mut PendingDisplayFinalizeWork,
    ) -> Option<DisplayGroupFrame> {
        match self
            .compose
            .display_sparkleflinger
            .try_finish_pending_display_finalization(&mut work.pending)
        {
            Ok(Some(frame)) => display_finalize_frame_to_group(frame, work.frame_format),
            Ok(None) => None,
            Err(error) => {
                debug!(%error, "GPU display-face finalization deferred to retained frame");
                None
            }
        }
    }
}

#[cfg(feature = "wgpu")]
enum DisplayFinalizeProgress {
    Idle,
    Pending,
    Ready(DisplayGroupFrame),
}

fn display_route_matches_target(
    display_route: Option<&DisplayGroupOutputRoute>,
    display_target: &DisplayFaceTarget,
) -> bool {
    display_route.is_some_and(|route| route.device_id == display_target.device_id)
}

pub(super) fn display_groups_require_composed_scene(
    group_canvases: &[(ZoneId, PendingGroupCanvasFrame)],
) -> bool {
    group_canvases
        .iter()
        .any(|(_, frame)| frame.display_target.blends_with_effect())
}

#[cfg(feature = "wgpu")]
fn display_finalize_dispatch_reuses_retained_frame(dispatch: &DisplayFinalizeDispatch) -> bool {
    match dispatch {
        DisplayFinalizeDispatch::Unsupported | DisplayFinalizeDispatch::Saturated => true,
        DisplayFinalizeDispatch::Pending(_) => false,
    }
}

#[cfg(feature = "wgpu")]
fn display_finalize_frame_to_group(
    frame: DisplayFinalizeFrame,
    frame_format: DisplayFrameFormat,
) -> Option<DisplayGroupFrame> {
    match (frame_format, frame) {
        (DisplayFrameFormat::Jpeg, DisplayFinalizeFrame::Yuv420(frame)) => {
            Some(DisplayGroupFrame::Yuv420(frame))
        }
        (DisplayFrameFormat::Rgb, DisplayFinalizeFrame::Rgba(surface)) => {
            Some(DisplayGroupFrame::from_surface(surface))
        }
        (DisplayFrameFormat::Jpeg, DisplayFinalizeFrame::Rgba(_))
        | (DisplayFrameFormat::Rgb, DisplayFinalizeFrame::Yuv420(_)) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_core::bus::{
        DisplayGroupFrame, DisplayGroupOutputRoute, DisplayGroupTarget, DisplayGroupViewport,
        DisplayYuv420Frame,
    };
    use hypercolor_core::types::canvas::Canvas;
    use hypercolor_types::canvas::Rgba;
    #[cfg(feature = "wgpu")]
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::device::{DeviceId, DisplayFrameFormat};
    use hypercolor_types::scene::{DisplayFaceBlendMode, DisplayFaceTarget, ZoneId};
    use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

    #[cfg(feature = "wgpu")]
    use super::{
        DisplayLaneContext, DisplayLaneMaterializer, DisplayLaneRoutes,
        display_finalize_dispatch_reuses_retained_frame, display_finalize_frame_to_group,
        display_groups_require_composed_scene,
    };
    #[cfg(not(feature = "wgpu"))]
    use super::{DisplayLaneRoutes, display_groups_require_composed_scene};
    #[cfg(feature = "wgpu")]
    use crate::render_thread::composition_planner::CompositionPlanner;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::pipeline_runtime::{
        ComposeRuntime, DisplayFinalizeRuntime, OutputArtifactsState,
    };
    use crate::render_thread::producer_queue::ProducerFrame;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::producer_queue::ProducerQueue;
    use crate::render_thread::render_groups::PendingGroupCanvasFrame;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::render_groups::{GroupCanvasFrame, ZoneRuntime};
    #[cfg(feature = "wgpu")]
    use crate::render_thread::scene_dependency::SceneDependencyKey;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::sparkleflinger::{
        DisplayFinalizeDispatch, DisplayFinalizeFrame, SparkleFlinger,
    };

    #[test]
    fn blended_display_group_forces_composed_scene_for_finalization() {
        let device_id = DeviceId::new();
        let replace = PendingGroupCanvasFrame {
            frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
            display_target: DisplayFaceTarget {
                device_id,
                blend_mode: DisplayFaceBlendMode::Replace,
                opacity: 1.0,
            },
            empty_direct_shell: false,
        };
        let blended = PendingGroupCanvasFrame {
            frame: ProducerFrame::Canvas(Canvas::new(4, 4)),
            display_target: DisplayFaceTarget {
                device_id,
                blend_mode: DisplayFaceBlendMode::Alpha,
                opacity: 0.88,
            },
            empty_direct_shell: false,
        };

        assert!(!display_groups_require_composed_scene(&[(
            ZoneId::new(),
            replace
        )]));
        assert!(display_groups_require_composed_scene(&[(
            ZoneId::new(),
            blended
        )]));
    }

    #[test]
    fn display_route_matching_requires_the_target_device() {
        let device_id = DeviceId::new();
        let target = DisplayFaceTarget {
            device_id,
            blend_mode: DisplayFaceBlendMode::Replace,
            opacity: 1.0,
        };
        let route = display_route(device_id, 1.0);
        let mut other_route = route.clone();
        other_route.device_id = DeviceId::new();

        assert!(super::display_route_matches_target(Some(&route), &target));
        assert!(!super::display_route_matches_target(
            Some(&other_route),
            &target
        ));
        assert!(!super::display_route_matches_target(None, &target));
    }

    #[test]
    fn display_route_for_group_falls_back_to_snapshot_route_when_bus_route_is_absent() {
        let group_id = ZoneId::new();
        let fallback_device = DeviceId::new();
        let bus_device = DeviceId::new();
        let fallback_route = display_route(fallback_device, 0.8);
        let mut bus_route = fallback_route.clone();
        bus_route.device_id = bus_device;

        let fallback_routes = HashMap::from([(group_id, fallback_route.clone())]);
        let empty_bus_routes = HashMap::new();
        let routes = DisplayLaneRoutes {
            current: &empty_bus_routes,
            fallback: &fallback_routes,
        };
        assert_eq!(routes.route_for_group(&group_id), Some(&fallback_route));

        let bus_routes = HashMap::from([(group_id, bus_route.clone())]);
        let routes = DisplayLaneRoutes {
            current: &bus_routes,
            fallback: &fallback_routes,
        };
        assert_eq!(routes.route_for_group(&group_id), Some(&bus_route));
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn display_lane_reuses_retained_materialized_frame_within_route_cadence() {
        let group_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = display_route(device_id, 1.0);
        let retained_frame = group_canvas_frame(&display_target, [255, 0, 0], true);
        let dependency_key = SceneDependencyKey::new(1, 1);
        let mut harness = DisplayLaneHarness::new();
        harness
            .render_group_runtime
            .retain_materialized_group_frame(
                group_id,
                100,
                dependency_key,
                &display_target,
                &display_route,
                false,
                &retained_frame,
            );

        let current_routes = HashMap::from([(group_id, display_route)]);
        let fallback_routes = HashMap::new();
        let target_fps = HashMap::from([(group_id, 30)]);
        let context = DisplayLaneContext {
            elapsed_ms: 120,
            dependency_key,
            target_fps: &target_fps,
            routes: DisplayLaneRoutes {
                current: &current_routes,
                fallback: &fallback_routes,
            },
        };
        let scene_frame = ProducerFrame::Canvas(color_canvas([0, 0, 255]));
        let fresh_frame = PendingGroupCanvasFrame {
            frame: ProducerFrame::Canvas(color_canvas([0, 255, 0])),
            display_target,
            empty_direct_shell: false,
        };

        let mut compose = harness.compose_runtime();
        let materialized = DisplayLaneMaterializer::new(&mut compose, context)
            .materialize_group_canvases(&[group_id], vec![(group_id, fresh_frame)], &scene_frame);

        assert_eq!(materialized.len(), 1);
        assert_eq!(first_pixel(&materialized[0].1.frame), [255, 0, 0, 255]);
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn unsupported_display_finalize_reuses_latest_retained_materialized_frame() {
        let group_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = display_route(device_id, 1.0);
        let retained_frame = group_canvas_frame(&display_target, [255, 0, 0], true);
        let dependency_key = SceneDependencyKey::new(1, 1);
        let mut harness = DisplayLaneHarness::new();
        harness
            .render_group_runtime
            .retain_materialized_group_frame(
                group_id,
                100,
                dependency_key,
                &display_target,
                &display_route,
                false,
                &retained_frame,
            );

        let current_routes = HashMap::from([(group_id, display_route)]);
        let fallback_routes = HashMap::new();
        let target_fps = HashMap::from([(group_id, 30)]);
        let context = DisplayLaneContext {
            elapsed_ms: 140,
            dependency_key,
            target_fps: &target_fps,
            routes: DisplayLaneRoutes {
                current: &current_routes,
                fallback: &fallback_routes,
            },
        };
        let scene_frame = ProducerFrame::Canvas(color_canvas([0, 0, 255]));
        let fresh_frame = PendingGroupCanvasFrame {
            frame: ProducerFrame::Canvas(color_canvas([0, 255, 0])),
            display_target,
            empty_direct_shell: false,
        };

        let mut compose = harness.compose_runtime();
        let materialized = DisplayLaneMaterializer::new(&mut compose, context)
            .materialize_group_canvases(&[group_id], vec![(group_id, fresh_frame)], &scene_frame);

        assert_eq!(materialized.len(), 1);
        assert_eq!(first_pixel(&materialized[0].1.frame), [255, 0, 0, 255]);
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn display_lane_pipelines_next_finalize_when_previous_completes() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            return;
        };
        let group_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = rgb_display_route(device_id);
        let dependency_key = SceneDependencyKey::new(1, 1);
        let mut elapsed_ms = 100;

        let red = wait_for_materialized_color(
            &mut harness,
            group_id,
            &display_target,
            &display_route,
            dependency_key,
            &mut elapsed_ms,
            [255, 0, 0],
            [255, 0, 0, 255],
        );
        assert!(red, "first display finalization should complete");

        let green = wait_for_materialized_color(
            &mut harness,
            group_id,
            &display_target,
            &display_route,
            dependency_key,
            &mut elapsed_ms,
            [0, 255, 0],
            [0, 255, 0, 255],
        );
        assert!(
            green,
            "completing one finalization should submit the next face frame"
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn unsupported_and_saturated_display_finalize_dispatches_retain_previous_frame() {
        assert!(display_finalize_dispatch_reuses_retained_frame(
            &DisplayFinalizeDispatch::Unsupported
        ));
        assert!(display_finalize_dispatch_reuses_retained_frame(
            &DisplayFinalizeDispatch::Saturated
        ));
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn display_finalize_frame_format_dispatch_rejects_mismatched_frames() {
        let surface = hypercolor_core::types::canvas::PublishedSurface::from_owned_canvas(
            color_canvas([255, 0, 0]),
            7,
            11,
        );
        let yuv = DisplayYuv420Frame::from_vec(vec![0; 6], 2, 2, 2, 1, 4, 1, 7, 11);

        let rgb_frame = display_finalize_frame_to_group(
            DisplayFinalizeFrame::Rgba(surface),
            DisplayFrameFormat::Rgb,
        )
        .expect("RGB finalize should accept RGBA display frames");
        assert!(matches!(rgb_frame, DisplayGroupFrame::Canvas(_)));

        let jpeg_frame = display_finalize_frame_to_group(
            DisplayFinalizeFrame::Yuv420(yuv),
            DisplayFrameFormat::Jpeg,
        )
        .expect("JPEG finalize should accept YUV420 display frames");
        assert!(matches!(jpeg_frame, DisplayGroupFrame::Yuv420(_)));

        let mismatched_surface =
            hypercolor_core::types::canvas::PublishedSurface::from_owned_canvas(
                color_canvas([255, 0, 0]),
                7,
                11,
            );
        let mismatched_yuv = DisplayYuv420Frame::from_vec(vec![0; 6], 2, 2, 2, 1, 4, 1, 7, 11);
        assert!(
            display_finalize_frame_to_group(
                DisplayFinalizeFrame::Rgba(mismatched_surface),
                DisplayFrameFormat::Jpeg,
            )
            .is_none()
        );
        assert!(
            display_finalize_frame_to_group(
                DisplayFinalizeFrame::Yuv420(mismatched_yuv),
                DisplayFrameFormat::Rgb,
            )
            .is_none()
        );
    }

    fn display_route(device_id: DeviceId, brightness: f32) -> DisplayGroupOutputRoute {
        DisplayGroupOutputRoute {
            device_id,
            width: 480,
            height: 480,
            circular: true,
            brightness,
            frame_format: DisplayFrameFormat::Jpeg,
            viewport: DisplayGroupViewport {
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                edge_behavior: EdgeBehavior::Clamp,
            },
        }
    }

    #[cfg(feature = "wgpu")]
    fn rgb_display_route(device_id: DeviceId) -> DisplayGroupOutputRoute {
        let mut route = display_route(device_id, 1.0);
        route.width = 2;
        route.height = 2;
        route.circular = false;
        route.frame_format = DisplayFrameFormat::Rgb;
        route
    }

    #[cfg(feature = "wgpu")]
    fn color_canvas(rgb: [u8; 3]) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(Rgba::new(rgb[0], rgb[1], rgb[2], 255));
        canvas
    }

    #[cfg(feature = "wgpu")]
    fn group_canvas_frame(
        display_target: &DisplayFaceTarget,
        rgb: [u8; 3],
        finalized: bool,
    ) -> GroupCanvasFrame {
        GroupCanvasFrame {
            frame: DisplayGroupFrame::from_surface(
                hypercolor_core::types::canvas::PublishedSurface::from_owned_canvas(
                    color_canvas(rgb),
                    0,
                    0,
                ),
            ),
            display_target: DisplayGroupTarget {
                device_id: display_target.device_id,
                blend_mode: display_target.blend_mode,
                opacity: display_target.opacity,
                finalized,
            },
        }
    }

    #[cfg(feature = "wgpu")]
    fn first_pixel(frame: &DisplayGroupFrame) -> [u8; 4] {
        match frame {
            DisplayGroupFrame::Canvas(frame) => frame.rgba_bytes()[0..4]
                .try_into()
                .expect("canvas frame should have at least one pixel"),
            DisplayGroupFrame::Yuv420(_) => panic!("expected RGBA display frame"),
        }
    }

    #[cfg(feature = "wgpu")]
    fn materialize_display_color(
        harness: &mut DisplayLaneHarness,
        group_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        dependency_key: SceneDependencyKey,
        elapsed_ms: u32,
        rgb: [u8; 3],
    ) -> Option<[u8; 4]> {
        let current_routes = HashMap::from([(group_id, display_route.clone())]);
        let fallback_routes = HashMap::new();
        let target_fps = HashMap::from([(group_id, 30)]);
        let context = DisplayLaneContext {
            elapsed_ms,
            dependency_key,
            target_fps: &target_fps,
            routes: DisplayLaneRoutes {
                current: &current_routes,
                fallback: &fallback_routes,
            },
        };
        let scene_frame = ProducerFrame::Canvas(color_canvas([0, 0, 0]));
        let face_frame = PendingGroupCanvasFrame {
            frame: ProducerFrame::Canvas(color_canvas(rgb)),
            display_target: display_target.clone(),
            empty_direct_shell: false,
        };
        let mut compose = harness.compose_runtime();
        let materialized = DisplayLaneMaterializer::new(&mut compose, context)
            .materialize_group_canvases(&[group_id], vec![(group_id, face_frame)], &scene_frame);

        match materialized.as_slice() {
            [] => None,
            [(_, frame)] => Some(first_pixel(&frame.frame)),
            _ => panic!("single display group should produce at most one frame"),
        }
    }

    #[cfg(feature = "wgpu")]
    fn wait_for_materialized_color(
        harness: &mut DisplayLaneHarness,
        group_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayGroupOutputRoute,
        dependency_key: SceneDependencyKey,
        elapsed_ms: &mut u32,
        submitted_rgb: [u8; 3],
        expected_rgba: [u8; 4],
    ) -> bool {
        let start = std::time::Instant::now();
        loop {
            *elapsed_ms = elapsed_ms.saturating_add(33);
            if materialize_display_color(
                harness,
                group_id,
                display_target,
                display_route,
                dependency_key,
                *elapsed_ms,
                submitted_rgb,
            ) == Some(expected_rgba)
            {
                return true;
            }
            if start.elapsed() >= std::time::Duration::from_secs(2) {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[cfg(feature = "wgpu")]
    struct DisplayLaneHarness {
        screen_queue: ProducerQueue,
        composition_planner: CompositionPlanner,
        sparkleflinger: SparkleFlinger,
        display_sparkleflinger: SparkleFlinger,
        display_finalize_runtime: DisplayFinalizeRuntime,
        render_group_runtime: ZoneRuntime,
        output_artifacts: OutputArtifactsState,
    }

    #[cfg(feature = "wgpu")]
    impl DisplayLaneHarness {
        fn new() -> Self {
            Self {
                screen_queue: ProducerQueue::new(),
                composition_planner: CompositionPlanner::new(),
                sparkleflinger: SparkleFlinger::cpu(),
                display_sparkleflinger: SparkleFlinger::cpu(),
                display_finalize_runtime: DisplayFinalizeRuntime::default(),
                render_group_runtime: ZoneRuntime::new(2, 2),
                output_artifacts: OutputArtifactsState::default(),
            }
        }

        fn with_gpu_display() -> Option<Self> {
            let mut harness = Self::new();
            harness.display_sparkleflinger =
                SparkleFlinger::new(RenderAccelerationMode::Gpu).ok()?;
            Some(harness)
        }

        fn compose_runtime(&mut self) -> ComposeRuntime<'_> {
            ComposeRuntime {
                screen_queue: &mut self.screen_queue,
                composition_planner: &mut self.composition_planner,
                sparkleflinger: &mut self.sparkleflinger,
                display_sparkleflinger: &mut self.display_sparkleflinger,
                display_finalize_runtime: &mut self.display_finalize_runtime,
                render_group_runtime: &mut self.render_group_runtime,
                output_artifacts: &mut self.output_artifacts,
            }
        }
    }
}
