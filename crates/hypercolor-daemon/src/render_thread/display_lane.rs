use std::collections::HashMap;

#[cfg(feature = "wgpu")]
use tracing::debug;

use hypercolor_core::bus::{
    DisplayBackdropFrame, DisplayZoneFrame, DisplayZoneOutputRoute, DisplayZoneTarget,
    ScreenBackdropRoute, screen_backdrop_lane_id,
};
use hypercolor_types::canvas::PublishedSurface;
use hypercolor_types::device::DeviceId;
#[cfg(feature = "wgpu")]
use hypercolor_types::device::DisplayFrameFormat;
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::{DisplayFaceTarget, Zone, ZoneId};

use super::pipeline_runtime::ComposeRuntime;
#[cfg(feature = "wgpu")]
use super::pipeline_runtime::PendingDisplayFinalizeWork;
use super::producer_queue::ProducerFrame;
use super::render_zones::{
    DisplayZoneCanvasFrame, PendingDisplayZoneFrame, RetainedBackdrop, ZoneContentRevision,
};
use super::scene_dependency::SceneDependencyKey;
#[cfg(feature = "wgpu")]
use super::screen_fade::ScreenFadeStep;
use super::sparkleflinger::DisplayFinalizeFade;
#[cfg(feature = "wgpu")]
use super::sparkleflinger::{
    DisplayFinalizeCacheKey, DisplayFinalizeDispatch, DisplayFinalizeFrame, DisplayFinalizeParams,
};
use crate::display_output::screen_placement;

pub(super) struct DisplayLaneRoutes<'a> {
    pub(super) current: &'a HashMap<ZoneId, DisplayZoneOutputRoute>,
    pub(super) fallback: &'a HashMap<ZoneId, DisplayZoneOutputRoute>,
}

impl DisplayLaneRoutes<'_> {
    fn route_for_zone(&self, zone_id: &ZoneId) -> Option<&DisplayZoneOutputRoute> {
        self.current
            .get(zone_id)
            .or_else(|| self.fallback.get(zone_id))
    }
}

/// A screen the display output thread drives, placed in the scene this
/// frame renders.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlacedScreen {
    pub(crate) device_id: DeviceId,
    /// The zone whose layout places the screen.
    pub(crate) zone_id: ZoneId,
    /// The screen's surface, with the placing zone's viewport for it,
    /// turned the way the panel is mounted.
    pub(crate) output: DisplayZoneOutputRoute,
    /// Frame rate cap for the screen while it shows its backdrop alone.
    pub(crate) target_fps: u32,
}

/// Place every screen the display output thread drives in `zones`, the
/// zones of the frame being rendered.
///
/// A route only names a screen's device; the zone that places it and that
/// zone's viewport are read here, from the scene whose frames the screen is
/// about to show. A placement resolved anywhere else could describe another
/// scene by the time its frame arrived. A screen no zone places is left out.
pub(super) fn place_screens(
    routes: &HashMap<DeviceId, ScreenBackdropRoute>,
    zones: &[Zone],
) -> Vec<PlacedScreen> {
    let mut screens = routes
        .iter()
        .filter_map(|(device_id, route)| {
            let placement =
                screen_placement(zones, &route.layout_device_ids, route.has_led_segments)?;
            let mut output = route.output.clone();
            let mount_rotation = output.viewport.rotation;
            output.viewport = placement.viewport;
            output.viewport.rotation += mount_rotation;
            Some(PlacedScreen {
                device_id: *device_id,
                zone_id: placement.zone_id,
                output,
                target_fps: route.target_fps,
            })
        })
        .collect::<Vec<_>>();
    screens.sort_by_key(|screen| screen.device_id.as_uuid());
    screens
}

/// The display lanes of a frame that composes no scene: every screen `zones`
/// place shows `picture`, one flat color, and no face is live.
pub(super) fn static_backdrop_lanes(
    routes: &HashMap<DeviceId, ScreenBackdropRoute>,
    zones: &[Zone],
    picture: &PublishedSurface,
) -> MaterializedDisplayLanes {
    let screens = place_screens(routes, zones);
    MaterializedDisplayLanes {
        active_lane_ids: screens
            .iter()
            .map(|screen| screen_backdrop_lane_id(screen.device_id))
            .collect(),
        // A flat color is the same picture at any size, so it goes out as
        // a finished frame for each worker to fit to its panel.
        frames: screens
            .iter()
            .map(|screen| {
                (
                    screen_backdrop_lane_id(screen.device_id),
                    DisplayZoneCanvasFrame {
                        frame: DisplayZoneFrame::from_surface(picture.clone()),
                        display_target: backdrop_lane_target(screen.device_id, true),
                    },
                )
            })
            .collect(),
    }
}

/// Where each placed screen's backdrop comes from this frame.
///
/// A screen's backdrop is the frame of the zone that places it, seen through
/// that zone's viewport for the screen (Spec 95 §5.10). Each screen resolves
/// its own, so screens whose boxes overlap in different zones never show
/// each other's zone.
#[derive(Clone, Copy)]
pub(super) struct ScreenBackdrops<'a> {
    /// The screens the frame's scene places, by device.
    pub(super) screens: &'a [PlacedScreen],
    /// Each scene zone's frame this pass.
    pub(super) zone_frames: &'a [(ZoneId, ProducerFrame)],
    /// The content revision of each zone whose frame says what it was made
    /// from. A screen holds its picture while its zone's revision stands.
    pub(super) zone_revisions: &'a [(ZoneId, ZoneContentRevision)],
    /// What a screen shows when its zone renders nothing.
    pub(super) black: &'a ProducerFrame,
}

impl<'a> ScreenBackdrops<'a> {
    fn screen(&self, device_id: DeviceId) -> Option<&'a PlacedScreen> {
        self.screens
            .iter()
            .find(|screen| screen.device_id == device_id)
    }

    /// The frame a zone gives its screens, or `None` for a zone that
    /// renders nothing this pass.
    ///
    /// It is always the zone's own frame, a scene transition included: the
    /// composed scene crossfade is one flattened picture, and a screen that
    /// read it would show whichever zone was painted last over its box.
    /// Each screen fades its own backdrop instead (Spec 95 §5.10).
    fn zone_frame(&self, zone_id: ZoneId) -> Option<&'a ProducerFrame> {
        self.zone_frames
            .iter()
            .find_map(|(id, frame)| (*id == zone_id).then_some(frame))
    }

    fn zone_revision(&self, zone_id: ZoneId) -> Option<ZoneContentRevision> {
        self.zone_revisions
            .iter()
            .find_map(|(id, revision)| (*id == zone_id).then_some(*revision))
    }

    /// The backdrop of the screen on `device_id`. A screen no zone places,
    /// or one whose zone renders nothing, shows black.
    fn frame_for_screen(&self, device_id: DeviceId) -> &'a ProducerFrame {
        self.screen(device_id)
            .and_then(|screen| self.zone_frame(screen.zone_id))
            .unwrap_or(self.black)
    }
}

/// The screens the active display zones put a face on.
pub(super) fn faced_screens(zones: &[Zone], active_zone_ids: &[ZoneId]) -> Vec<DeviceId> {
    zones
        .iter()
        .filter(|zone| active_zone_ids.contains(&zone.id))
        .filter_map(|zone| Some(zone.display_target.as_ref()?.device_id))
        .collect()
}

pub(super) struct DisplayLaneContext<'a> {
    pub(super) elapsed_ms: u64,
    pub(super) dependency_key: SceneDependencyKey,
    pub(super) target_fps: &'a HashMap<ZoneId, u32>,
    pub(super) routes: DisplayLaneRoutes<'a>,
    pub(super) backdrops: ScreenBackdrops<'a>,
    /// How far the scene transition in progress has faded the incoming
    /// scene in, or `None` when no transition runs. A screen with a base
    /// shows its backdrop mixed over it at this weight.
    pub(super) fade: Option<f32>,
}

/// Every display lane's frame this pass, with the lanes that stay live.
pub(super) struct MaterializedDisplayLanes {
    pub(super) frames: Vec<(ZoneId, DisplayZoneCanvasFrame)>,
    /// Face zones and backdrop lanes, including ones with no fresh frame.
    pub(super) active_lane_ids: Vec<ZoneId>,
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

    /// Materialize the frame's display lanes: each face over its screen's
    /// backdrop, and a backdrop alone for each placed screen with no face.
    ///
    /// `faced` are the screens the active display zones put a face on, with
    /// or without a frame this pass.
    pub(super) fn materialize(
        &mut self,
        active_zone_ids: &[ZoneId],
        faced: &[DeviceId],
        zone_canvases: Vec<(ZoneId, PendingDisplayZoneFrame)>,
    ) -> MaterializedDisplayLanes {
        #[cfg(feature = "wgpu")]
        let finalizes = self
            .compose
            .display_sparkleflinger
            .finalizes_display_frames();
        #[cfg(not(feature = "wgpu"))]
        let finalizes = false;
        let backdrops = self.context.backdrops;
        // A compositor that finalizes shows a face over its backdrop in one
        // pass. One that does not hands the worker the backdrop of every
        // placed screen: to crop alone, or for a face to blend over and
        // turn with.
        let backdrop_screens = backdrops
            .screens
            .iter()
            .filter(|screen| !finalizes || !faced.contains(&screen.device_id))
            .collect::<Vec<_>>();
        let backdrop_devices = backdrop_screens
            .iter()
            .map(|screen| screen.device_id)
            .collect::<Vec<_>>();
        let mut active_lane_ids = active_zone_ids.to_vec();
        active_lane_ids.extend(
            backdrop_devices
                .iter()
                .copied()
                .map(screen_backdrop_lane_id),
        );
        #[cfg(feature = "wgpu")]
        if self.context.fade.is_some() {
            // The bases a fade mixes over live on routes of their own.
            let mut cached = active_lane_ids.clone();
            cached.extend(
                self.compose
                    .render_zone_runtime
                    .screen_fades
                    .base_route_ids(),
            );
            self.compose.discard_display_finalizations_except(&cached);
        } else {
            self.compose
                .discard_display_finalizations_except(&active_lane_ids);
        }
        self.compose
            .render_zone_runtime
            .retain_backdrop_frames_for(&backdrop_devices);

        let mut frames = self.materialize_zone_canvases(zone_canvases, finalizes);
        for screen in backdrop_screens {
            if let Some(frame) = self.materialize_backdrop_lane(screen, finalizes) {
                frames.push((screen_backdrop_lane_id(screen.device_id), frame));
            }
        }
        MaterializedDisplayLanes {
            frames,
            active_lane_ids,
        }
    }

    fn materialize_zone_canvases(
        &mut self,
        zone_canvases: Vec<(ZoneId, PendingDisplayZoneFrame)>,
        finalizes: bool,
    ) -> Vec<(ZoneId, DisplayZoneCanvasFrame)> {
        zone_canvases
            .into_iter()
            .filter_map(|(zone_id, frame)| {
                let display_target = frame.display_target.clone();
                // The face's route says what its panel is. Where the scene
                // places the screen is this frame's to say.
                let placed = self.context.backdrops.screen(display_target.device_id);
                let display_route =
                    self.context
                        .routes
                        .route_for_zone(&zone_id)
                        .cloned()
                        .map(|mut route| {
                            if let Some(screen) =
                                placed.filter(|screen| screen.device_id == route.device_id)
                            {
                                route.viewport = screen.output.viewport;
                            }
                            route
                        });
                let empty_direct_shell = frame.empty_direct_shell;
                if let Some(route) = display_route.as_ref()
                    && let Some(frame) = self
                        .compose
                        .render_zone_runtime
                        .reuse_retained_materialized_zone_frame(
                            zone_id,
                            self.context.elapsed_ms,
                            self.context.target_fps.get(&zone_id).copied(),
                            self.context.dependency_key,
                            &display_target,
                            route,
                            empty_direct_shell,
                        )
                {
                    return Some((zone_id, frame));
                }

                let (materialized, fresh_materialization) = if let Some(materialized) =
                    self.materialize_zone_canvas(zone_id, frame, display_route.as_ref(), finalizes)
                {
                    (materialized, true)
                } else {
                    let retained = display_route.as_ref().and_then(|route| {
                        self.compose
                            .render_zone_runtime
                            .reuse_latest_materialized_zone_frame(
                                zone_id,
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
                        .render_zone_runtime
                        .retain_materialized_zone_frame(
                            zone_id,
                            self.context.elapsed_ms,
                            self.context.target_fps.get(&zone_id).copied(),
                            self.context.dependency_key,
                            &display_target,
                            route,
                            empty_direct_shell,
                            &materialized,
                        );
                }

                Some((zone_id, materialized))
            })
            .collect()
    }

    fn materialize_zone_canvas(
        &mut self,
        zone_id: ZoneId,
        zone_canvas: PendingDisplayZoneFrame,
        display_route: Option<&DisplayZoneOutputRoute>,
        finalizes: bool,
    ) -> Option<DisplayZoneCanvasFrame> {
        let PendingDisplayZoneFrame {
            frame,
            display_target,
            ..
        } = zone_canvas;
        // A face that replaces its backdrop never shows it, so it is
        // finalized over black instead of reading the zone's frame.
        let backdrops = self.context.backdrops;
        let (backdrop, fade) = if display_target.blends_with_effect() {
            (
                backdrops.frame_for_screen(display_target.device_id),
                display_route.and_then(|route| self.finalize_fade(route)),
            )
        } else {
            (backdrops.black, None)
        };
        if let Some(finalized) = self.finalize_display_zone_canvas(
            zone_id,
            backdrop,
            Some(&frame),
            &display_target,
            display_route,
            None,
            fade,
        ) {
            self.note_fade_shown(display_target.device_id, &finalized);
            return Some(DisplayZoneCanvasFrame {
                frame: finalized.frame,
                display_target: DisplayZoneTarget {
                    device_id: display_target.device_id,
                    blend_mode: display_target.blend_mode,
                    opacity: display_target.opacity,
                    finalized: true,
                },
            });
        }
        // A routed face on a compositor that finalizes is published
        // finalized or not at all: a missed frame holds the last finalized
        // one rather than handing the workers a raw face to blend.
        if finalizes && display_route_matches_target(display_route, &display_target) {
            return None;
        }

        Some(DisplayZoneCanvasFrame {
            frame: DisplayZoneFrame::from_surface(self.cpu_surface(frame)?),
            display_target: (&display_target).into(),
        })
    }

    /// The fade a finalize pass for `route`'s screen runs this frame: the
    /// transition's weight over the screen's base, when it has one for the
    /// panel the route draws.
    fn finalize_fade(&self, route: &DisplayZoneOutputRoute) -> Option<DisplayFinalizeFade> {
        #[cfg(feature = "wgpu")]
        {
            let weight = self.context.fade?;
            self.compose.render_zone_runtime.screen_fades.finalize_fade(
                route.device_id,
                route.width,
                route.height,
                weight,
            )
        }
        #[cfg(not(feature = "wgpu"))]
        {
            let _ = route;
            None
        }
    }

    /// Record how far into the fade the picture a screen is about to
    /// publish is, for a transition that interrupts this one to start the
    /// screen from.
    fn note_fade_shown(&mut self, device_id: DeviceId, finalized: &FinalizedDisplayFrame) {
        #[cfg(feature = "wgpu")]
        if let Some(step) = finalized.fade {
            self.compose
                .render_zone_runtime
                .screen_fades
                .note_shown(device_id, step);
        }
        #[cfg(not(feature = "wgpu"))]
        let _ = (device_id, finalized);
    }

    /// Materialize the backdrop lane of a placed screen: its backdrop
    /// finalized for the panel where the compositor finalizes, and otherwise
    /// the owning zone's frame for the screen's worker to crop. While a
    /// scene transition runs, a screen with a base shows its backdrop part
    /// way in over the one it showed before.
    fn materialize_backdrop_lane(
        &mut self,
        screen: &PlacedScreen,
        finalizes: bool,
    ) -> Option<DisplayZoneCanvasFrame> {
        let device_id = screen.device_id;
        let lane_id = screen_backdrop_lane_id(device_id);
        let backdrops = self.context.backdrops;
        let backdrop = backdrops.zone_frame(screen.zone_id);
        let (width, height) = (screen.output.width, screen.output.height);
        let finalize_fade = if finalizes {
            self.finalize_fade(&screen.output)
        } else {
            None
        };
        let pixel_fade = self.context.fade.filter(|_| {
            !finalizes
                && self
                    .compose
                    .render_zone_runtime
                    .screen_fades
                    .fades_pixels(device_id, width, height)
        });
        // A zone that renders nothing has no content to hold a picture by,
        // and a fading picture is a new one at every step.
        let content = backdrop
            .filter(|_| finalize_fade.is_none() && pixel_fade.is_none())
            .and_then(|_| backdrops.zone_revision(screen.zone_id));
        let retained = self
            .compose
            .render_zone_runtime
            .reuse_retained_backdrop_frame(self.context.elapsed_ms, screen, content)
            .filter(|retained| {
                let (RetainedBackdrop::Unchanged(frame) | RetainedBackdrop::Held(frame)) = retained;
                frame.display_target.finalized == finalizes
            });
        if !finalizes {
            let frame = match (pixel_fade, retained) {
                // The worker paces its own encodes, so outside a fade only
                // a picture that has not changed is held here.
                (None, Some(RetainedBackdrop::Unchanged(frame)))
                // A fade is mixed here, at the screen's own cadence.
                | (
                    Some(_),
                    Some(RetainedBackdrop::Unchanged(frame) | RetainedBackdrop::Held(frame)),
                ) => return Some(frame),
                (fade, _) => {
                    let surface = self.cpu_surface(backdrop.unwrap_or(backdrops.black).clone())?;
                    let incoming = hypercolor_core::bus::CanvasFrame::from_surface(surface);
                    let mixed = fade.and_then(|weight| {
                        self.compose.render_zone_runtime.screen_fades.mix_pixels(
                            device_id,
                            &incoming,
                            &screen.output.viewport,
                            width,
                            height,
                            weight,
                        )
                    });
                    // The viewport travels with the canvas it applies to,
                    // so the worker never crops one scene's canvas through
                    // another's box. A mixed backdrop is cropped already.
                    let cropped = mixed.is_some();
                    DisplayZoneCanvasFrame {
                        frame: DisplayZoneFrame::Backdrop(DisplayBackdropFrame {
                            frame: mixed.unwrap_or(incoming),
                            viewport: screen.output.viewport,
                            cropped,
                        }),
                        display_target: backdrop_lane_target(device_id, false),
                    }
                }
            };
            self.compose.render_zone_runtime.retain_backdrop_frame(
                self.context.elapsed_ms,
                screen,
                content,
                &frame,
            );
            return Some(frame);
        }
        if backdrop.is_none() && finalize_fade.is_none() {
            // Black needs no finalize pass, and a render loop that is about
            // to idle would not come back to collect one. It is the same
            // picture at any size, so the worker fits it to the panel. A
            // screen fading to black gets here once its fade is over.
            self.discard_pending_display_finalize_work(lane_id);
            return Some(DisplayZoneCanvasFrame {
                frame: DisplayZoneFrame::from_surface(self.cpu_surface(backdrops.black.clone())?),
                display_target: backdrop_lane_target(device_id, true),
            });
        }

        match retained {
            Some(RetainedBackdrop::Unchanged(frame)) => {
                // Work still in flight would only finalize this picture
                // again, or an older one.
                self.discard_pending_display_finalize_work(lane_id);
                return Some(frame);
            }
            Some(RetainedBackdrop::Held(frame)) => return Some(frame),
            None => {}
        }

        let finalized = self.finalize_display_zone_canvas(
            lane_id,
            backdrop.unwrap_or(backdrops.black),
            None,
            &backdrop_finalize_target(device_id),
            Some(&screen.output),
            content,
            finalize_fade,
        );
        let Some(finalized) = finalized else {
            let retained = self
                .compose
                .render_zone_runtime
                .reuse_latest_backdrop_frame(screen)
                .filter(|frame| frame.display_target.finalized)?;
            #[cfg(feature = "wgpu")]
            crate::render_thread::sparkleflinger::gpu::record_gpu_display_finalize_latch();
            return Some(retained);
        };
        self.note_fade_shown(device_id, &finalized);
        let frame = DisplayZoneCanvasFrame {
            frame: finalized.frame,
            display_target: backdrop_lane_target(device_id, true),
        };
        self.compose.render_zone_runtime.retain_backdrop_frame(
            self.context.elapsed_ms,
            screen,
            finalized.content,
            &frame,
        );
        Some(frame)
    }

    /// The CPU pixels of a frame the display workers finish themselves.
    fn cpu_surface(&mut self, frame: ProducerFrame) -> Option<PublishedSurface> {
        Some(match frame {
            ProducerFrame::Canvas(canvas) => PublishedSurface::from_owned_canvas(canvas, 0, 0),
            ProducerFrame::Surface(surface) => surface,
            ProducerFrame::ScreenPublication(publication) => {
                let (canvas, _) =
                    ProducerFrame::ScreenPublication(publication).into_cpu_render_frame()?;
                PublishedSurface::from_owned_canvas(canvas, 0, 0)
            }
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
        })
    }

    fn finalize_display_zone_canvas(
        &mut self,
        zone_id: ZoneId,
        backdrop_frame: &ProducerFrame,
        face_frame: Option<&ProducerFrame>,
        display_target: &DisplayFaceTarget,
        display_route: Option<&DisplayZoneOutputRoute>,
        content: Option<ZoneContentRevision>,
        fade: Option<DisplayFinalizeFade>,
    ) -> Option<FinalizedDisplayFrame> {
        let Some(display_route) =
            display_route.filter(|route| route.device_id == display_target.device_id)
        else {
            return None;
        };

        self.finalize_display_zone_canvas_with_route(
            zone_id,
            backdrop_frame,
            face_frame,
            display_target,
            display_route,
            content,
            fade,
        )
    }

    #[cfg(not(feature = "wgpu"))]
    fn finalize_display_zone_canvas_with_route(
        &mut self,
        zone_id: ZoneId,
        backdrop_frame: &ProducerFrame,
        face_frame: Option<&ProducerFrame>,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        content: Option<ZoneContentRevision>,
        fade: Option<DisplayFinalizeFade>,
    ) -> Option<FinalizedDisplayFrame> {
        let _ = fade;
        let _ = content;
        let _ = zone_id;
        let _ = backdrop_frame;
        let _ = face_frame;
        let _ = display_target;
        let _ = display_route;
        None
    }

    #[cfg(not(feature = "wgpu"))]
    fn discard_pending_display_finalize_work(&mut self, zone_id: ZoneId) {
        let _ = zone_id;
    }

    /// Finalize a display lane's frame: `face_frame` over `backdrop_frame`
    /// seen through the route's viewport, or the backdrop alone when the
    /// lane has no face. `content` is what a lane with no face shows, and
    /// `fade` mixes the backdrop over the screen's base under the face.
    #[cfg(feature = "wgpu")]
    fn finalize_display_zone_canvas_with_route(
        &mut self,
        zone_id: ZoneId,
        backdrop_frame: &ProducerFrame,
        face_frame: Option<&ProducerFrame>,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        content: Option<ZoneContentRevision>,
        fade: Option<DisplayFinalizeFade>,
    ) -> Option<FinalizedDisplayFrame> {
        let params = DisplayFinalizeParams {
            cache_key: DisplayFinalizeCacheKey {
                zone_id,
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
            fade,
        };

        let completed =
            match self.finish_pending_display_finalize_work(zone_id, display_target, display_route)
            {
                DisplayFinalizeProgress::Ready(frame) => Some(frame),
                DisplayFinalizeProgress::Pending => return None,
                DisplayFinalizeProgress::Idle => None,
            };
        // A finalization that just landed for the content the zone still
        // shows is this lane's frame. Asking again would submit a pass and
        // a readback for the same picture.
        if content.is_some()
            && completed
                .as_ref()
                .is_some_and(|completed| completed.content == content)
        {
            return completed;
        }

        let dispatch = match face_frame {
            Some(face_frame) if display_route.frame_format == DisplayFrameFormat::Jpeg => self
                .compose
                .display_sparkleflinger
                .begin_finalize_display_face_yuv420(backdrop_frame, face_frame, params),
            Some(face_frame) => self
                .compose
                .display_sparkleflinger
                .begin_finalize_display_face(backdrop_frame, face_frame, params),
            None => self
                .compose
                .display_sparkleflinger
                .begin_finalize_display_backdrop(backdrop_frame, params),
        };

        match dispatch {
            Ok(DisplayFinalizeDispatch::Pending(pending)) => {
                #[cfg(test)]
                self.compose.display_finalize_runtime.note_dispatch();
                let mut work = PendingDisplayFinalizeWork {
                    dependency_key: self.context.dependency_key,
                    display_target: display_target.clone(),
                    display_route: display_route.clone(),
                    frame_format: display_route.frame_format,
                    content,
                    fade: fade.and_then(|fade| {
                        self.compose
                            .render_zone_runtime
                            .screen_fades
                            .step(fade.weight)
                    }),
                    pending,
                };
                match self.try_finish_display_finalize_work(&mut work) {
                    DisplayFinalizeAttempt::Finished(Some(frame)) => return Some(frame),
                    DisplayFinalizeAttempt::Finished(None) | DisplayFinalizeAttempt::Failed => {}
                    DisplayFinalizeAttempt::InFlight => {
                        self.compose.display_finalize_runtime.insert(zone_id, work);
                    }
                }
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
    fn discard_pending_display_finalize_work(&mut self, zone_id: ZoneId) {
        if let Some(work) = self.compose.display_finalize_runtime.take(zone_id) {
            self.compose
                .display_sparkleflinger
                .discard_pending_display_finalization(work.pending);
        }
    }

    #[cfg(feature = "wgpu")]
    fn finish_pending_display_finalize_work(
        &mut self,
        zone_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
    ) -> DisplayFinalizeProgress {
        let Some(mut work) = self.compose.display_finalize_runtime.take(zone_id) else {
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

        match self.try_finish_display_finalize_work(&mut work) {
            DisplayFinalizeAttempt::Finished(Some(frame)) => DisplayFinalizeProgress::Ready(frame),
            DisplayFinalizeAttempt::Finished(None) | DisplayFinalizeAttempt::Failed => {
                DisplayFinalizeProgress::Idle
            }
            DisplayFinalizeAttempt::InFlight => {
                self.compose.display_finalize_runtime.insert(zone_id, work);
                DisplayFinalizeProgress::Pending
            }
        }
    }

    #[cfg(feature = "wgpu")]
    fn try_finish_display_finalize_work(
        &mut self,
        work: &mut PendingDisplayFinalizeWork,
    ) -> DisplayFinalizeAttempt {
        match self
            .compose
            .display_sparkleflinger
            .try_finish_pending_display_finalization(&mut work.pending)
        {
            Ok(Some(frame)) => DisplayFinalizeAttempt::Finished(
                display_finalize_frame_to_zone(frame, work.frame_format).map(|frame| {
                    FinalizedDisplayFrame {
                        frame,
                        content: work.content,
                        fade: work.fade,
                    }
                }),
            ),
            Ok(None) => DisplayFinalizeAttempt::InFlight,
            Err(error) => {
                // The failed finalization already released its readback slot,
                // so the work is dropped and the next frame dispatches anew.
                debug!(%error, "GPU display-face finalization deferred to retained frame");
                DisplayFinalizeAttempt::Failed
            }
        }
    }
}

/// A frame finalized for a screen, with what its zone showed when the
/// finalization was dispatched. A finalization can land a frame later than
/// it was asked for, so this is not always what the zone shows now.
struct FinalizedDisplayFrame {
    frame: DisplayZoneFrame,
    #[cfg_attr(not(feature = "wgpu"), allow(dead_code))]
    content: Option<ZoneContentRevision>,
    /// The step of the screen's fade the frame shows, if it shows one.
    #[cfg(feature = "wgpu")]
    fade: Option<ScreenFadeStep>,
}

#[cfg(feature = "wgpu")]
enum DisplayFinalizeProgress {
    Idle,
    Pending,
    Ready(FinalizedDisplayFrame),
}

/// One poll of a display finalization still in flight.
#[cfg(feature = "wgpu")]
enum DisplayFinalizeAttempt {
    /// The readback finished; `None` when its format did not match the route.
    Finished(Option<FinalizedDisplayFrame>),
    /// The readback is still in flight and should be polled next frame.
    InFlight,
    /// The finalization failed and released its resources.
    Failed,
}

fn display_route_matches_target(
    display_route: Option<&DisplayZoneOutputRoute>,
    display_target: &DisplayFaceTarget,
) -> bool {
    display_route.is_some_and(|route| route.device_id == display_target.device_id)
}

/// The lane target a screen's backdrop is published under. The frame is the
/// screen's whole picture, so it replaces whatever the panel showed.
fn backdrop_lane_target(device_id: DeviceId, finalized: bool) -> DisplayZoneTarget {
    DisplayZoneTarget {
        device_id,
        blend_mode: BlendMode::Replace,
        opacity: 1.0,
        finalized,
    }
}

/// The finalize parameters of a lane with no face: nothing is laid over the
/// backdrop.
fn backdrop_finalize_target(device_id: DeviceId) -> DisplayFaceTarget {
    DisplayFaceTarget {
        device_id,
        blend_mode: BlendMode::Alpha,
        opacity: 0.0,
    }
}

#[cfg(feature = "wgpu")]
fn display_finalize_dispatch_reuses_retained_frame(dispatch: &DisplayFinalizeDispatch) -> bool {
    match dispatch {
        DisplayFinalizeDispatch::Unsupported | DisplayFinalizeDispatch::Saturated => true,
        DisplayFinalizeDispatch::Pending(_) => false,
    }
}

#[cfg(feature = "wgpu")]
fn display_finalize_frame_to_zone(
    frame: DisplayFinalizeFrame,
    frame_format: DisplayFrameFormat,
) -> Option<DisplayZoneFrame> {
    match (frame_format, frame) {
        (DisplayFrameFormat::Jpeg, DisplayFinalizeFrame::Yuv420(frame)) => {
            Some(DisplayZoneFrame::Yuv420(frame))
        }
        (DisplayFrameFormat::Rgb, DisplayFinalizeFrame::Rgba(surface)) => {
            Some(DisplayZoneFrame::from_surface(surface))
        }
        (DisplayFrameFormat::Jpeg, DisplayFinalizeFrame::Rgba(_))
        | (DisplayFrameFormat::Rgb, DisplayFinalizeFrame::Yuv420(_)) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[cfg(feature = "wgpu")]
    use hypercolor_core::bus::screen_backdrop_lane_id;
    #[cfg(feature = "wgpu")]
    use hypercolor_core::bus::{DisplayYuv420Frame, DisplayZoneFrame, DisplayZoneTarget};
    use hypercolor_core::bus::{DisplayZoneOutputRoute, DisplayZoneViewport};
    use hypercolor_types::canvas::{Canvas, Rgba};
    #[cfg(feature = "wgpu")]
    use hypercolor_types::config::RenderAccelerationMode;
    use hypercolor_types::device::{DeviceId, DisplayFrameFormat};
    use hypercolor_types::layer::BlendMode;
    use hypercolor_types::scene::{DisplayFaceTarget, ZoneId};
    use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

    #[cfg(feature = "wgpu")]
    use super::{
        DisplayLaneContext, DisplayLaneMaterializer, MaterializedDisplayLanes,
        display_finalize_dispatch_reuses_retained_frame, display_finalize_frame_to_zone,
    };
    use super::{DisplayLaneRoutes, PlacedScreen, ScreenBackdrops};
    #[cfg(feature = "wgpu")]
    use crate::render_thread::composition_planner::CompositionPlanner;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::pipeline_runtime::{
        ComposeRuntime, DisplayFinalizeRuntime, EffectDeltaClock, OutputArtifactsState,
    };
    use crate::render_thread::producer_queue::ProducerFrame;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::producer_queue::ProducerFrameIdentity;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::producer_queue::ProducerQueue;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::render_zones::{
        DisplayZoneCanvasFrame, PendingDisplayZoneFrame, ZoneContentRevision, ZoneRuntime,
    };
    #[cfg(feature = "wgpu")]
    use crate::render_thread::scene_dependency::SceneDependencyKey;
    #[cfg(feature = "wgpu")]
    use crate::render_thread::screen_fade::{ScreenTransition, begin_screen_fades};
    #[cfg(feature = "wgpu")]
    use crate::render_thread::sparkleflinger::{
        DisplayFinalizeDispatch, DisplayFinalizeFrame, SparkleFlinger,
    };

    #[test]
    fn display_route_matching_requires_the_target_device() {
        let device_id = DeviceId::new();
        let target = DisplayFaceTarget {
            device_id,
            blend_mode: BlendMode::Replace,
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
    fn display_route_for_zone_falls_back_to_snapshot_route_when_bus_route_is_absent() {
        let zone_id = ZoneId::new();
        let fallback_device = DeviceId::new();
        let bus_device = DeviceId::new();
        let fallback_route = display_route(fallback_device, 0.8);
        let mut bus_route = fallback_route.clone();
        bus_route.device_id = bus_device;

        let fallback_routes = HashMap::from([(zone_id, fallback_route.clone())]);
        let empty_bus_routes = HashMap::new();
        let routes = DisplayLaneRoutes {
            current: &empty_bus_routes,
            fallback: &fallback_routes,
        };
        assert_eq!(routes.route_for_zone(&zone_id), Some(&fallback_route));

        let bus_routes = HashMap::from([(zone_id, bus_route.clone())]);
        let routes = DisplayLaneRoutes {
            current: &bus_routes,
            fallback: &fallback_routes,
        };
        assert_eq!(routes.route_for_zone(&zone_id), Some(&bus_route));
    }

    /// Each screen reads the frame of the zone that places it, and nothing
    /// else: no other zone's frame and no flattened picture of the scene.
    #[test]
    fn screen_backdrops_resolve_each_screen_from_the_zone_that_places_it() {
        fn same(left: &ProducerFrame, right: &ProducerFrame) -> bool {
            std::ptr::eq(left, right)
        }
        let left_zone = ZoneId::new();
        let right_zone = ZoneId::new();
        let idle_zone = ZoneId::new();
        let (left_screen, right_screen, idle_screen, unplaced_screen) = (
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
        );
        let screens = [
            placed_screen(left_screen, left_zone, 15),
            placed_screen(right_screen, right_zone, 15),
            placed_screen(idle_screen, idle_zone, 15),
        ];
        let zone_frames = [
            (left_zone, solid_frame([255, 0, 0])),
            (right_zone, solid_frame([0, 0, 255])),
        ];
        let (left_frame, right_frame) = (&zone_frames[0].1, &zone_frames[1].1);
        let black = black_frame();
        let own_frames = ScreenBackdrops {
            screens: &screens,
            zone_frames: &zone_frames,
            zone_revisions: &[],
            black: &black,
        };

        assert!(same(own_frames.frame_for_screen(left_screen), left_frame));
        assert!(same(own_frames.frame_for_screen(right_screen), right_frame));
        assert!(
            same(own_frames.frame_for_screen(idle_screen), &black),
            "a zone that renders nothing gives its screens black"
        );
        assert!(
            same(own_frames.frame_for_screen(unplaced_screen), &black),
            "a screen no zone places has no backdrop"
        );
    }

    /// A compositor that does not finalize hands each screen's worker the
    /// owning zone's canvas to crop, and a blending face's worker the same
    /// canvas to blend over.
    #[cfg(feature = "wgpu")]
    #[test]
    fn cpu_backdrop_lane_carries_the_owning_zones_canvas_for_the_worker() {
        let mut harness = DisplayLaneHarness::new();
        let (left_zone, right_zone) = (ZoneId::new(), ZoneId::new());
        let (left_screen, right_screen) = (DeviceId::new(), DeviceId::new());
        let screens = HashMap::from([
            (left_screen, placed_screen(left_screen, left_zone, 15)),
            (right_screen, placed_screen(right_screen, right_zone, 15)),
        ]);
        let zone_frames = [
            (left_zone, solid_frame([255, 0, 0])),
            (right_zone, solid_frame([0, 0, 255])),
        ];

        let lanes = harness.materialize(100, &screens, &zone_frames, Vec::new());

        for (screen, expected) in [
            (left_screen, [255, 0, 0, 255]),
            (right_screen, [0, 0, 255, 255]),
        ] {
            let lane = screen_backdrop_lane_id(screen);
            assert!(lanes.active_lane_ids.contains(&lane));
            let frame = lane_frame(&lanes, lane).expect("each placed screen gets a backdrop");
            assert_eq!(first_pixel(&frame.frame), expected);
            assert_eq!(frame.display_target.device_id, screen);
            assert!(
                !frame.display_target.finalized,
                "the worker crops a backdrop this compositor did not finalize"
            );
        }

        // Both faces' workers take the screen's backdrop lane: it carries
        // the viewport a face turns with, and what a blending face shows
        // through.
        let replacing = display_face(left_screen, BlendMode::Replace, [0, 255, 0]);
        let blending = display_face(right_screen, BlendMode::Alpha, [0, 255, 0]);
        let (replacing_zone, blending_zone) = (ZoneId::new(), ZoneId::new());
        let lanes = harness.materialize(
            133,
            &screens,
            &zone_frames,
            vec![(replacing_zone, replacing), (blending_zone, blending)],
        );

        for (screen, expected) in [
            (left_screen, [255, 0, 0, 255]),
            (right_screen, [0, 0, 255, 255]),
        ] {
            let backdrop = lane_frame(&lanes, screen_backdrop_lane_id(screen))
                .expect("a face's worker needs its screen's backdrop lane");
            assert_eq!(first_pixel(&backdrop.frame), expected);
        }
        for face_zone in [replacing_zone, blending_zone] {
            let face = lane_frame(&lanes, face_zone).expect("each face is handed to its worker");
            assert_eq!(first_pixel(&face.frame), [0, 255, 0, 255]);
            assert!(!face.display_target.finalized);
        }
    }

    /// Two screens whose boxes cover the same region of two zones each show
    /// their own zone, with a face and without one.
    #[cfg(feature = "wgpu")]
    #[test]
    fn gpu_lanes_finalize_overlapping_screens_over_their_own_zones() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let (left_zone, right_zone) = (ZoneId::new(), ZoneId::new());
        let (left_screen, right_screen) = (DeviceId::new(), DeviceId::new());
        // Both routes carry the same viewport: the boxes overlap entirely.
        let screens = HashMap::from([
            (left_screen, placed_screen(left_screen, left_zone, 15)),
            (right_screen, placed_screen(right_screen, right_zone, 15)),
        ]);
        let zone_frames = [
            (left_zone, solid_frame([255, 0, 0])),
            (right_zone, solid_frame([0, 0, 255])),
        ];

        let lanes = harness.materialize_until(&screens, &zone_frames, Vec::new, |lanes| {
            lanes.frames.len() == 2
        });
        for (screen, expected) in [
            (left_screen, [255, 0, 0, 255]),
            (right_screen, [0, 0, 255, 255]),
        ] {
            let frame = lane_frame(&lanes, screen_backdrop_lane_id(screen))
                .expect("each screen should finalize its backdrop");
            assert_eq!(first_pixel(&frame.frame), expected);
            assert!(frame.display_target.finalized);
        }

        let (left_face, right_face) = (ZoneId::new(), ZoneId::new());
        let faces = || {
            vec![
                (
                    left_face,
                    display_face(left_screen, BlendMode::Alpha, [0, 255, 0]),
                ),
                (
                    right_face,
                    display_face(right_screen, BlendMode::Alpha, [0, 255, 0]),
                ),
            ]
        };
        let lanes = harness.materialize_until(&screens, &zone_frames, faces, |lanes| {
            lane_frame(lanes, left_face).is_some() && lane_frame(lanes, right_face).is_some()
        });
        assert!(
            lanes
                .active_lane_ids
                .iter()
                .all(|lane| [left_face, right_face].contains(lane)),
            "a finalized face leaves its screen no backdrop lane"
        );
        let left = first_pixel(&lane_frame(&lanes, left_face).expect("left face").frame);
        let right = first_pixel(&lane_frame(&lanes, right_face).expect("right face").frame);
        assert!(
            left[0] > 24 && left[1] > 24 && left[2] == 0,
            "the left face should sit over its red zone: {left:?}"
        );
        assert!(
            right[2] > 24 && right[1] > 24 && right[0] == 0,
            "the right face should sit over its blue zone: {right:?}"
        );
    }

    /// A screen fades its backdrop in over the one it showed when the
    /// transition began, in the finalize pass itself: the lane's frame is
    /// the finished mix. A screen whose panel changes shape part way has no
    /// base for the new panel and cuts to the incoming backdrop.
    #[cfg(feature = "wgpu")]
    #[test]
    fn gpu_backdrop_fades_in_over_its_base_and_cuts_when_its_panel_changes() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone = ZoneId::new();
        let screen = DeviceId::new();
        let lane = screen_backdrop_lane_id(screen);
        let screens = HashMap::from([(screen, placed_screen(screen, zone, 15))]);
        let red = [(zone, surface_frame([255, 0, 0]))];
        let shows = |lanes: &MaterializedDisplayLanes| {
            lane_frame(lanes, lane).map(|frame| first_pixel(&frame.frame))
        };

        // The frame before the transition rendered no zone: black.
        harness.materialize(100, &screens, &[], Vec::new());
        harness.transition(Some((1, 0.5)));
        let half = harness.materialize_until(&screens, &red, Vec::new, |lanes| {
            shows(lanes).is_some_and(|pixel| pixel[0] >= 100)
        });
        let pixel = shows(&half).expect("the fading backdrop");
        assert!(
            (120..=136).contains(&pixel[0]) && pixel[1] == 0 && pixel[2] == 0,
            "half way in from black, red should be at half: {pixel:?}"
        );

        // Same transition, further along.
        harness.transition(Some((1, 1.0)));
        let full = harness.materialize_until(&screens, &red, Vec::new, |lanes| {
            shows(lanes).is_some_and(|pixel| pixel[0] >= 250)
        });
        assert_eq!(shows(&full), Some([255, 0, 0, 255]));

        // The panel turns on its side while the fade is still at half.
        harness.transition(Some((1, 0.5)));
        let mut turned = placed_screen(screen, zone, 15);
        (turned.output.width, turned.output.height) = (4, 1);
        let screens = HashMap::from([(screen, turned)]);
        let cut = harness.materialize_until(&screens, &red, Vec::new, |lanes| {
            lane_frame(lanes, lane).is_some_and(|frame| frame.frame.width() == 4)
        });
        assert_eq!(
            shows(&cut),
            Some([255, 0, 0, 255]),
            "a panel with no base for its new shape should cut to the incoming backdrop"
        );
    }

    /// A screen under its cadence cap is finalized less often than the
    /// transition moves. Interrupted between two of its frames, it starts
    /// the new fade from the picture it last published, mixed at the weight
    /// that picture had, and not at the weight the transition had reached
    /// behind it.
    #[cfg(feature = "wgpu")]
    #[test]
    fn gpu_interrupted_fade_starts_from_the_weight_its_screen_last_published() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone = ZoneId::new();
        let screen = DeviceId::new();
        let lane = screen_backdrop_lane_id(screen);
        let screens = HashMap::from([(screen, placed_screen(screen, zone, 4))]);
        let red = [(zone, surface_frame([255, 0, 0]))];
        let blue = [(zone, surface_frame([0, 0, 255]))];
        let green = [(zone, surface_frame([0, 255, 0]))];
        let shows = |lanes: &MaterializedDisplayLanes| {
            lane_frame(lanes, lane).map(|frame| first_pixel(&frame.frame))
        };

        harness.retain_zone_frames(&red);
        harness.materialize_until(&screens, &red, Vec::new, |lanes| {
            shows(lanes) == Some([255, 0, 0, 255])
        });

        // A quarter of the way to blue, and the screen publishes it.
        harness.transition(Some((1, 0.25)));
        harness.retain_zone_frames(&blue);
        let quarter = harness.materialize_until(&screens, &blue, Vec::new, |lanes| {
            shows(lanes).is_some_and(|pixel| pixel[2] >= 40)
        });
        let published = shows(&quarter).expect("the fading backdrop");
        assert!(
            (185..=197).contains(&published[0]) && (58..=70).contains(&published[2]),
            "a quarter of the way from red to blue: {published:?}"
        );

        // The transition is three quarters through by the next tick, and
        // the screen, under its cap, still shows the quarter.
        harness.transition(Some((1, 0.75)));
        let held = harness.materialize(harness.elapsed_ms + 10, &screens, &blue, Vec::new());
        assert_eq!(shows(&held), Some(published));

        // Green interrupts, half way in at once: half green over the
        // quarter mix the screen was showing.
        harness.transition(Some((2, 0.5)));
        harness.retain_zone_frames(&green);
        let onward = harness.materialize_until(&screens, &green, Vec::new, |lanes| {
            shows(lanes).is_some_and(|pixel| pixel[1] >= 100)
        });
        let pixel = shows(&onward).expect("the interrupting fade");
        assert!(
            (90..=102).contains(&pixel[0])
                && (122..=134).contains(&pixel[1])
                && (26..=38).contains(&pixel[2]),
            "half green over a quarter of the way from red to blue: {pixel:?}"
        );
    }

    /// Where the display workers finish frames, the render thread mixes
    /// the fade itself and hands the worker the finished backdrop, cropped.
    /// A transition that interrupts another starts from that mix, and a
    /// screen whose panel changes shape cuts.
    #[cfg(feature = "wgpu")]
    #[test]
    fn cpu_backdrop_fades_from_its_last_picture_and_cuts_when_its_panel_changes() {
        let mut harness = DisplayLaneHarness::new();
        let zone = ZoneId::new();
        let screen = DeviceId::new();
        let lane = screen_backdrop_lane_id(screen);
        let screens = HashMap::from([(screen, placed_screen(screen, zone, 15))]);
        let backdrop = |lanes: &MaterializedDisplayLanes| {
            let frame = lane_frame(lanes, lane).expect("the screen's backdrop lane");
            let DisplayZoneFrame::Backdrop(backdrop) = &frame.frame else {
                panic!("a compositor that does not finalize publishes a backdrop to crop");
            };
            (
                backdrop.cropped,
                first_pixel(&frame.frame),
                backdrop.frame.width,
            )
        };

        let red = [(zone, solid_frame([255, 0, 0]))];
        let settled = harness.materialize(100, &screens, &red, Vec::new());
        assert_eq!(backdrop(&settled), (false, [255, 0, 0, 255], 2));

        harness.transition(Some((1, 0.5)));
        let blue = [(zone, solid_frame([0, 0, 255]))];
        let half = harness.materialize(10_000, &screens, &blue, Vec::new());
        assert_eq!(
            backdrop(&half),
            (true, [128, 0, 128, 255], 2),
            "half way from red to blue, mixed and cropped for the panel"
        );

        // A second transition begins on the mix the panel was showing.
        harness.transition(Some((2, 0.0)));
        let green = [(zone, solid_frame([0, 255, 0]))];
        let interrupted = harness.materialize(20_000, &screens, &green, Vec::new());
        assert_eq!(backdrop(&interrupted), (true, [128, 0, 128, 255], 2));
        harness.transition(Some((2, 0.5)));
        let onward = harness.materialize(30_000, &screens, &green, Vec::new());
        assert_eq!(backdrop(&onward), (true, [64, 128, 64, 255], 2));

        // The panel turns on its side: no base for the new shape.
        let mut turned = placed_screen(screen, zone, 15);
        (turned.output.width, turned.output.height) = (4, 1);
        let screens = HashMap::from([(screen, turned)]);
        let cut = harness.materialize(40_000, &screens, &green, Vec::new());
        assert_eq!(
            backdrop(&cut),
            (false, [0, 255, 0, 255], 2),
            "a panel with no base for its new shape gets the incoming canvas to crop"
        );

        // The transition ends: the worker crops again.
        harness.transition(None);
        let ended = harness.materialize(50_000, &screens, &green, Vec::new());
        assert_eq!(backdrop(&ended), (false, [0, 255, 0, 255], 2));
    }

    /// A zone whose content stands is finalized for its screen once, however
    /// many ticks pass and however often its frame changes identity on the
    /// way out: one pass and one readback, none while it is collected and
    /// none after. New content costs exactly one more.
    #[cfg(feature = "wgpu")]
    #[test]
    fn gpu_backdrop_of_a_still_zone_is_finalized_exactly_once() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone = ZoneId::new();
        let screen = DeviceId::new();
        let lane = screen_backdrop_lane_id(screen);
        let screens = HashMap::from([(screen, placed_screen(screen, zone, 15))]);
        harness
            .pinned_revisions
            .insert(zone, ZoneContentRevision(1));

        let mut shown = None;
        for tick in 0..60 {
            harness.display_sparkleflinger.wait_for_gpu_idle_for_test();
            harness.elapsed_ms += 100;
            // The same picture in a new frame each tick, the way a still
            // zone is snapshotted and composed again every frame.
            let red = [(zone, surface_frame([255, 0, 0]))];
            let lanes = harness.materialize(harness.elapsed_ms, &screens, &red, Vec::new());
            let Some(frame) = lane_frame(&lanes, lane) else {
                assert!(shown.is_none(), "tick {tick}: the screen lost its backdrop");
                continue;
            };
            assert_eq!(first_pixel(&frame.frame), [255, 0, 0, 255]);
            let identity = surface_identity(&frame.frame);
            assert_eq!(
                *shown.get_or_insert(identity),
                identity,
                "tick {tick}: a still zone's backdrop was finalized again"
            );
        }
        assert!(shown.is_some(), "the backdrop never arrived");
        assert_eq!(
            harness.display_finalize_runtime.dispatch_count(),
            1,
            "a still zone should cost its screen one finalize pass"
        );
        assert_eq!(harness.display_finalize_runtime.pending_zone_count(), 0);

        harness
            .pinned_revisions
            .insert(zone, ZoneContentRevision(2));
        let blue = [(zone, surface_frame([0, 0, 255]))];
        let start = std::time::Instant::now();
        loop {
            harness.display_sparkleflinger.wait_for_gpu_idle_for_test();
            harness.elapsed_ms += 100;
            let lanes = harness.materialize(harness.elapsed_ms, &screens, &blue, Vec::new());
            if lane_frame(&lanes, lane)
                .is_some_and(|frame| first_pixel(&frame.frame) == [0, 0, 255, 255])
            {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "the new content never reached the screen"
            );
        }
        for _ in 0..10 {
            harness.display_sparkleflinger.wait_for_gpu_idle_for_test();
            harness.elapsed_ms += 100;
            harness.materialize(harness.elapsed_ms, &screens, &blue, Vec::new());
        }
        assert_eq!(
            harness.display_finalize_runtime.dispatch_count(),
            2,
            "new content should cost exactly one more finalize pass"
        );
    }

    /// A backdrop that shows what it showed is not finalized again, so a
    /// still scene sends its screens one frame, not one per cadence tick.
    #[cfg(feature = "wgpu")]
    #[test]
    fn gpu_backdrop_lane_keeps_its_frame_while_the_backdrop_is_unchanged() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone = ZoneId::new();
        let screen = DeviceId::new();
        let lane = screen_backdrop_lane_id(screen);
        let screens = HashMap::from([(screen, placed_screen(screen, zone, 15))]);
        let red = [(zone, surface_frame([255, 0, 0]))];

        let first = harness.materialize_until(&screens, &red, Vec::new, |lanes| {
            lane_frame(lanes, lane).is_some()
        });
        let first = lane_frame(&first, lane)
            .expect("backdrop frame")
            .frame
            .clone();
        // Long past the cadence deadline, the same backdrop keeps its frame.
        harness.elapsed_ms += 10_000;
        for _ in 0..4 {
            harness.display_sparkleflinger.wait_for_gpu_idle_for_test();
            harness.elapsed_ms += 100;
            let lanes = harness.materialize(harness.elapsed_ms, &screens, &red, Vec::new());
            let held = lane_frame(&lanes, lane).expect("the backdrop frame should be held");
            assert_eq!(surface_identity(&held.frame), surface_identity(&first));
        }
        assert_eq!(
            harness.display_finalize_runtime.pending_zone_count(),
            0,
            "an unchanged backdrop should leave no finalization in flight"
        );

        let blue = [(zone, surface_frame([0, 0, 255]))];
        let changed = harness.materialize_until(&screens, &blue, Vec::new, |lanes| {
            lane_frame(lanes, lane)
                .is_some_and(|frame| first_pixel(&frame.frame) == [0, 0, 255, 255])
        });
        assert_ne!(
            surface_identity(&lane_frame(&changed, lane).expect("backdrop frame").frame),
            surface_identity(&first)
        );
    }

    /// A screen showing its backdrop alone is finalized at its own cadence
    /// cap, and a face at the face cadence, whatever the render loop runs at.
    #[cfg(feature = "wgpu")]
    #[test]
    fn gpu_backdrop_lane_holds_its_cadence_cap_and_a_face_holds_the_face_cadence() {
        const BACKDROP_FPS: u32 = 15;
        const FACE_FPS: u32 = 30;
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone = ZoneId::new();
        let (bare_screen, face_screen) = (DeviceId::new(), DeviceId::new());
        let face_zone = ZoneId::new();
        let backdrop_lane = screen_backdrop_lane_id(bare_screen);
        let screens = HashMap::from([
            (bare_screen, placed_screen(bare_screen, zone, BACKDROP_FPS)),
            (face_screen, placed_screen(face_screen, zone, BACKDROP_FPS)),
        ]);

        // One second of a 60 fps render loop over a backdrop and a face that
        // both change every frame. The GPU drains between ticks, so every
        // finalization the cadence allows lands.
        let mut backdrop_frames = std::collections::HashSet::new();
        let mut face_frames = std::collections::HashSet::new();
        for tick in 0..60_u32 {
            harness.display_sparkleflinger.wait_for_gpu_idle_for_test();
            let elapsed_ms = 1_000 + u64::from(tick) * 1_000 / 60;
            let shade = u8::try_from(tick + 1).expect("tick fits a color channel");
            let zone_frames = [(zone, surface_frame([shade, 0, 0]))];
            let lanes = harness.materialize(
                elapsed_ms,
                &screens,
                &zone_frames,
                vec![(
                    face_zone,
                    display_face(face_screen, BlendMode::Replace, [0, shade, 0]),
                )],
            );
            if let Some(frame) = lane_frame(&lanes, backdrop_lane) {
                backdrop_frames.insert(surface_identity(&frame.frame));
            }
            if let Some(frame) = lane_frame(&lanes, face_zone) {
                face_frames.insert(surface_identity(&frame.frame));
            }
        }

        assert!(
            (13..=16).contains(&backdrop_frames.len()),
            "a backdrop capped at {BACKDROP_FPS} fps finalized {} frames in a second",
            backdrop_frames.len()
        );
        assert!(
            (27..=31).contains(&face_frames.len()),
            "a face at {FACE_FPS} fps finalized {} frames in a second",
            face_frames.len()
        );
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn display_lane_reuses_retained_materialized_frame_within_route_cadence() {
        let zone_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = display_route(device_id, 1.0);
        let retained_frame = zone_canvas_frame(&display_target, [255, 0, 0], true);
        let dependency_key = SceneDependencyKey::new(1, 1);
        let mut harness = DisplayLaneHarness::new();
        harness.render_zone_runtime.retain_materialized_zone_frame(
            zone_id,
            100,
            Some(30),
            dependency_key,
            &display_target,
            &display_route,
            false,
            &retained_frame,
        );

        let current_routes = HashMap::from([(zone_id, display_route)]);
        let fallback_routes = HashMap::new();
        let target_fps = HashMap::from([(zone_id, 30)]);
        let black = black_frame();
        let context = DisplayLaneContext {
            elapsed_ms: 120,
            dependency_key,
            target_fps: &target_fps,
            routes: DisplayLaneRoutes {
                current: &current_routes,
                fallback: &fallback_routes,
            },
            backdrops: ScreenBackdrops {
                screens: &[],
                zone_frames: &[],
                zone_revisions: &[],
                black: &black,
            },
            fade: None,
        };
        let fresh_frame = PendingDisplayZoneFrame {
            frame: ProducerFrame::Canvas(color_canvas([0, 255, 0])),
            display_target,
            empty_direct_shell: false,
        };

        let mut compose = harness.compose_runtime();
        let materialized = DisplayLaneMaterializer::new(&mut compose, context)
            .materialize(&[zone_id], &[], vec![(zone_id, fresh_frame)])
            .frames;

        assert_eq!(materialized.len(), 1);
        assert_eq!(first_pixel(&materialized[0].1.frame), [255, 0, 0, 255]);
    }

    /// A compositor that finalizes publishes a routed face finalized or not
    /// at all: while a finalization is in flight the last finalized frame
    /// holds, and the workers are never handed the raw face to blend.
    #[cfg(feature = "wgpu")]
    #[test]
    fn display_finalize_in_flight_reuses_latest_retained_materialized_frame() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = rgb_display_route(device_id);
        let retained_frame = zone_canvas_frame(&display_target, [255, 0, 0], true);
        let dependency_key = SceneDependencyKey::new(1, 1);
        harness.render_zone_runtime.retain_materialized_zone_frame(
            zone_id,
            100,
            Some(30),
            dependency_key,
            &display_target,
            &display_route,
            false,
            &retained_frame,
        );
        harness
            .display_sparkleflinger
            .defer_next_display_finalize_resolve_for_test();

        let materialized = materialize_display_color(
            &mut harness,
            zone_id,
            &display_target,
            &display_route,
            dependency_key,
            140,
            [0, 255, 0],
        );

        assert_eq!(materialized, Some([255, 0, 0, 255]));
        assert_eq!(harness.display_finalize_runtime.pending_zone_count(), 1);
    }

    /// A compositor that does not finalize has no finalized frame to hold,
    /// so a routed face goes to its worker as it is.
    #[cfg(feature = "wgpu")]
    #[test]
    fn compositor_that_does_not_finalize_hands_a_routed_face_to_its_worker() {
        let zone_id = ZoneId::new();
        let device_id = DeviceId::new();
        let mut harness = DisplayLaneHarness::new();

        let lanes = harness.materialize(
            140,
            &HashMap::new(),
            &[],
            vec![(
                zone_id,
                display_face(device_id, BlendMode::Replace, [0, 255, 0]),
            )],
        );

        let face = lane_frame(&lanes, zone_id).expect("the face should reach its worker");
        assert_eq!(first_pixel(&face.frame), [0, 255, 0, 255]);
        assert!(!face.display_target.finalized);
    }

    #[cfg(feature = "wgpu")]
    #[test]
    fn display_lane_pipelines_next_finalize_when_previous_completes() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            return;
        };
        let zone_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = rgb_display_route(device_id);
        let dependency_key = SceneDependencyKey::new(1, 1);
        let mut elapsed_ms = 100;

        let red = wait_for_materialized_color(
            &mut harness,
            zone_id,
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
            zone_id,
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

    /// A frame that fails without a retained frame publishes no display
    /// frame, so it discards the display finalizations in flight and frees
    /// their readback slots rather than holding them for a frame that never
    /// claims them.
    #[cfg(feature = "wgpu")]
    #[test]
    fn failed_frame_discards_display_finalizations_in_flight() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = rgb_display_route(device_id);
        let dependency_key = SceneDependencyKey::new(1, 1);
        // Hold the finalization in flight the way a busy GPU does; a software
        // adapter can otherwise finish the readback inside the dispatch call.
        harness
            .display_sparkleflinger
            .defer_next_display_finalize_resolve_for_test();
        let materialized = materialize_display_color(
            &mut harness,
            zone_id,
            &display_target,
            &display_route,
            dependency_key,
            100,
            [255, 0, 0],
        );
        assert_eq!(materialized, None, "the deferred finalization lands later");
        assert_eq!(
            harness.display_finalize_runtime.pending_zone_count(),
            1,
            "the deferred finalization should stay in flight"
        );
        let slots_in_use = |harness: &DisplayLaneHarness| {
            harness
                .display_sparkleflinger
                .display_finalize_readback_slots_in_use_for_test()
                .expect("the GPU display compositor counts its readback slots")
        };
        assert!(
            slots_in_use(&harness) > 0,
            "the finalization in flight holds a slot"
        );

        harness
            .compose_runtime()
            .release_failed_frame_outputs(dependency_key);

        assert_eq!(harness.display_finalize_runtime.pending_zone_count(), 0);
        assert_eq!(
            slots_in_use(&harness),
            0,
            "a failed frame should free every display finalize readback slot"
        );
    }

    /// Device loss destroys a finalization's readback buffer even after its
    /// map completed. The lane reports that finalization as failed and drops
    /// it, freeing its slot, rather than panicking or re-polling dead work.
    #[cfg(feature = "wgpu")]
    #[test]
    fn display_finalization_lost_after_its_map_completed_is_dropped() {
        let Some(mut harness) = DisplayLaneHarness::with_gpu_display() else {
            assert!(
                std::env::var_os("HYPERCOLOR_REQUIRE_GPU_TESTS").is_none(),
                "GPU display finalization test was required but no GPU compositor started"
            );
            return;
        };
        let zone_id = ZoneId::new();
        let device_id = DeviceId::new();
        let display_target = DisplayFaceTarget::new(device_id);
        let display_route = rgb_display_route(device_id);
        let dependency_key = SceneDependencyKey::new(1, 1);
        let materialize = |harness: &mut DisplayLaneHarness, elapsed_ms| {
            materialize_display_color(
                harness,
                zone_id,
                &display_target,
                &display_route,
                dependency_key,
                elapsed_ms,
                [255, 0, 0],
            )
        };
        harness
            .display_sparkleflinger
            .defer_next_display_finalize_resolve_for_test();
        assert_eq!(materialize(&mut harness, 100), None);
        // Land the map, then hold the completed finalization one more frame.
        harness.display_sparkleflinger.wait_for_gpu_idle_for_test();
        harness
            .display_sparkleflinger
            .defer_next_display_finalize_resolve_for_test();
        assert_eq!(materialize(&mut harness, 133), None);
        assert_eq!(harness.display_finalize_runtime.pending_zone_count(), 1);

        harness.display_sparkleflinger.lose_gpu_device_for_test();

        assert_eq!(materialize(&mut harness, 166), None);
        assert_eq!(
            harness.display_finalize_runtime.pending_zone_count(),
            0,
            "a failed finalization must not stay queued"
        );
        assert_eq!(
            harness
                .display_sparkleflinger
                .display_finalize_readback_slots_in_use_for_test(),
            Some(0)
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
        let surface = hypercolor_types::canvas::PublishedSurface::from_owned_canvas(
            color_canvas([255, 0, 0]),
            7,
            11,
        );
        let yuv = DisplayYuv420Frame::from_vec(vec![0; 6], 2, 2, 2, 1, 4, 1, 7, 11);

        let rgb_frame = display_finalize_frame_to_zone(
            DisplayFinalizeFrame::Rgba(surface),
            DisplayFrameFormat::Rgb,
        )
        .expect("RGB finalize should accept RGBA display frames");
        assert!(matches!(rgb_frame, DisplayZoneFrame::Canvas(_)));

        let jpeg_frame = display_finalize_frame_to_zone(
            DisplayFinalizeFrame::Yuv420(yuv),
            DisplayFrameFormat::Jpeg,
        )
        .expect("JPEG finalize should accept YUV420 display frames");
        assert!(matches!(jpeg_frame, DisplayZoneFrame::Yuv420(_)));

        let mismatched_surface = hypercolor_types::canvas::PublishedSurface::from_owned_canvas(
            color_canvas([255, 0, 0]),
            7,
            11,
        );
        let mismatched_yuv = DisplayYuv420Frame::from_vec(vec![0; 6], 2, 2, 2, 1, 4, 1, 7, 11);
        assert!(
            display_finalize_frame_to_zone(
                DisplayFinalizeFrame::Rgba(mismatched_surface),
                DisplayFrameFormat::Jpeg,
            )
            .is_none()
        );
        assert!(
            display_finalize_frame_to_zone(
                DisplayFinalizeFrame::Yuv420(mismatched_yuv),
                DisplayFrameFormat::Rgb,
            )
            .is_none()
        );
    }

    fn black_frame() -> ProducerFrame {
        ProducerFrame::Canvas(Canvas::new(2, 2))
    }

    fn solid_frame(rgb: [u8; 3]) -> ProducerFrame {
        ProducerFrame::Canvas(color_canvas(rgb))
    }

    /// A frame with a stable identity, the way a zone's published surface
    /// has one.
    #[cfg(feature = "wgpu")]
    fn surface_frame(rgb: [u8; 3]) -> ProducerFrame {
        let mut pool = hypercolor_types::canvas::RenderSurfacePool::try_with_slot_count(
            hypercolor_types::canvas::SurfaceDescriptor::rgba8888(2, 2),
            1,
        )
        .expect("test surface pool should allocate");
        let mut lease = pool
            .try_dequeue()
            .expect("test surface pool should dequeue")
            .expect("a new pool has a free slot");
        lease
            .canvas_mut()
            .fill(Rgba::new(rgb[0], rgb[1], rgb[2], 255));
        ProducerFrame::Surface(lease.submit(0, 0))
    }

    /// A raw RGB screen of two by two pixels, placed in `zone_id` and showing
    /// the zone's whole canvas.
    fn placed_screen(device_id: DeviceId, zone_id: ZoneId, target_fps: u32) -> PlacedScreen {
        let mut output = display_route(device_id, 1.0);
        output.width = 2;
        output.height = 2;
        output.circular = false;
        output.frame_format = DisplayFrameFormat::Rgb;
        PlacedScreen {
            device_id,
            zone_id,
            output,
            target_fps,
        }
    }

    #[cfg(feature = "wgpu")]
    fn display_face(
        device_id: DeviceId,
        blend_mode: BlendMode,
        rgb: [u8; 3],
    ) -> PendingDisplayZoneFrame {
        PendingDisplayZoneFrame {
            frame: solid_frame(rgb),
            display_target: DisplayFaceTarget {
                device_id,
                blend_mode,
                opacity: 0.5,
            },
            empty_direct_shell: false,
        }
    }

    #[cfg(feature = "wgpu")]
    fn lane_frame(
        lanes: &MaterializedDisplayLanes,
        lane_id: ZoneId,
    ) -> Option<&DisplayZoneCanvasFrame> {
        lanes
            .frames
            .iter()
            .find_map(|(id, frame)| (*id == lane_id).then_some(frame))
    }

    #[cfg(feature = "wgpu")]
    fn surface_identity(
        frame: &DisplayZoneFrame,
    ) -> hypercolor_types::canvas::PublishedSurfaceStorageIdentity {
        match frame {
            DisplayZoneFrame::Canvas(frame) => frame.surface().storage_identity(),
            DisplayZoneFrame::Backdrop(backdrop) => backdrop.frame.surface().storage_identity(),
            DisplayZoneFrame::Yuv420(_) => panic!("expected RGBA display frame"),
        }
    }

    fn display_route(device_id: DeviceId, brightness: f32) -> DisplayZoneOutputRoute {
        DisplayZoneOutputRoute {
            device_id,
            width: 480,
            height: 480,
            circular: true,
            brightness,
            frame_format: DisplayFrameFormat::Jpeg,
            viewport: DisplayZoneViewport {
                position: NormalizedPosition::new(0.5, 0.5),
                size: NormalizedPosition::new(1.0, 1.0),
                rotation: 0.0,
                scale: 1.0,
                edge_behavior: EdgeBehavior::Clamp,
            },
        }
    }

    #[cfg(feature = "wgpu")]
    fn rgb_display_route(device_id: DeviceId) -> DisplayZoneOutputRoute {
        let mut route = display_route(device_id, 1.0);
        route.width = 2;
        route.height = 2;
        route.circular = false;
        route.frame_format = DisplayFrameFormat::Rgb;
        route
    }

    fn color_canvas(rgb: [u8; 3]) -> Canvas {
        let mut canvas = Canvas::new(2, 2);
        canvas.fill(Rgba::new(rgb[0], rgb[1], rgb[2], 255));
        canvas
    }

    #[cfg(feature = "wgpu")]
    fn zone_canvas_frame(
        display_target: &DisplayFaceTarget,
        rgb: [u8; 3],
        finalized: bool,
    ) -> DisplayZoneCanvasFrame {
        DisplayZoneCanvasFrame {
            frame: DisplayZoneFrame::from_surface(
                hypercolor_types::canvas::PublishedSurface::from_owned_canvas(
                    color_canvas(rgb),
                    0,
                    0,
                ),
            ),
            display_target: DisplayZoneTarget {
                device_id: display_target.device_id,
                blend_mode: display_target.blend_mode,
                opacity: display_target.opacity,
                finalized,
            },
        }
    }

    #[cfg(feature = "wgpu")]
    fn first_pixel(frame: &DisplayZoneFrame) -> [u8; 4] {
        match frame {
            DisplayZoneFrame::Canvas(frame) => frame.rgba_bytes()[0..4]
                .try_into()
                .expect("canvas frame should have at least one pixel"),
            DisplayZoneFrame::Backdrop(backdrop) => backdrop.frame.rgba_bytes()[0..4]
                .try_into()
                .expect("backdrop frame should have at least one pixel"),
            DisplayZoneFrame::Yuv420(_) => panic!("expected RGBA display frame"),
        }
    }

    #[cfg(feature = "wgpu")]
    fn materialize_display_color(
        harness: &mut DisplayLaneHarness,
        zone_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        dependency_key: SceneDependencyKey,
        elapsed_ms: u64,
        rgb: [u8; 3],
    ) -> Option<[u8; 4]> {
        let current_routes = HashMap::from([(zone_id, display_route.clone())]);
        let fallback_routes = HashMap::new();
        let target_fps = HashMap::from([(zone_id, 30)]);
        let black = black_frame();
        let context = DisplayLaneContext {
            elapsed_ms,
            dependency_key,
            target_fps: &target_fps,
            routes: DisplayLaneRoutes {
                current: &current_routes,
                fallback: &fallback_routes,
            },
            backdrops: ScreenBackdrops {
                screens: &[],
                zone_frames: &[],
                zone_revisions: &[],
                black: &black,
            },
            fade: None,
        };
        let face_frame = PendingDisplayZoneFrame {
            frame: ProducerFrame::Canvas(color_canvas(rgb)),
            display_target: display_target.clone(),
            empty_direct_shell: false,
        };
        let mut compose = harness.compose_runtime();
        let materialized = DisplayLaneMaterializer::new(&mut compose, context)
            .materialize(&[zone_id], &[], vec![(zone_id, face_frame)])
            .frames;

        match materialized.as_slice() {
            [] => None,
            [(_, frame)] => Some(first_pixel(&frame.frame)),
            _ => panic!("single display zone should produce at most one frame"),
        }
    }

    #[cfg(feature = "wgpu")]
    fn wait_for_materialized_color(
        harness: &mut DisplayLaneHarness,
        zone_id: ZoneId,
        display_target: &DisplayFaceTarget,
        display_route: &DisplayZoneOutputRoute,
        dependency_key: SceneDependencyKey,
        elapsed_ms: &mut u64,
        submitted_rgb: [u8; 3],
        expected_rgba: [u8; 4],
    ) -> bool {
        let start = std::time::Instant::now();
        loop {
            *elapsed_ms = elapsed_ms.saturating_add(33);
            if materialize_display_color(
                harness,
                zone_id,
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
        effect_delta_clock: EffectDeltaClock,
        render_zone_runtime: ZoneRuntime,
        output_artifacts: OutputArtifactsState,
        elapsed_ms: u64,
        /// The content revision each zone's frame was last given, the way
        /// the zone runtime gives them: a frame that can be told to be the
        /// last one keeps its revision, and any other frame gets a new one.
        zone_revisions: HashMap<ZoneId, (Option<ProducerFrameIdentity>, ZoneContentRevision)>,
        /// Revisions a test sets itself, whatever frame the zone hands over.
        pinned_revisions: HashMap<ZoneId, ZoneContentRevision>,
        next_revision: u64,
        /// The weight of the scene transition the next frames render in.
        fade: Option<f32>,
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
                effect_delta_clock: EffectDeltaClock::default(),
                render_zone_runtime: ZoneRuntime::new(2, 2),
                output_artifacts: OutputArtifactsState::default(),
                elapsed_ms: 100,
                zone_revisions: HashMap::new(),
                pinned_revisions: HashMap::new(),
                next_revision: 0,
                fade: None,
            }
        }

        /// The revision of each zone frame this pass.
        fn revise(
            &mut self,
            zone_frames: &[(ZoneId, ProducerFrame)],
        ) -> Vec<(ZoneId, ZoneContentRevision)> {
            zone_frames
                .iter()
                .map(|(zone_id, frame)| {
                    if let Some(pinned) = self.pinned_revisions.get(zone_id) {
                        return (*zone_id, *pinned);
                    }
                    let identity = frame.stable_identity();
                    let revision = match self.zone_revisions.get(zone_id) {
                        Some((known, revision)) if identity.is_some() && *known == identity => {
                            *revision
                        }
                        _ => {
                            self.next_revision += 1;
                            ZoneContentRevision(self.next_revision)
                        }
                    };
                    self.zone_revisions.insert(*zone_id, (identity, revision));
                    (*zone_id, revision)
                })
                .collect()
        }

        /// Materialize one frame's lanes. Every face is routed to a raw RGB
        /// screen of two by two pixels at 30 fps.
        fn materialize(
            &mut self,
            elapsed_ms: u64,
            screens: &HashMap<DeviceId, PlacedScreen>,
            zone_frames: &[(ZoneId, ProducerFrame)],
            faces: Vec<(ZoneId, PendingDisplayZoneFrame)>,
        ) -> MaterializedDisplayLanes {
            let face_routes = faces
                .iter()
                .map(|(zone_id, face)| (*zone_id, rgb_display_route(face.display_target.device_id)))
                .collect::<HashMap<_, _>>();
            let target_fps = faces
                .iter()
                .map(|(zone_id, _)| (*zone_id, 30))
                .collect::<HashMap<_, _>>();
            let faced = faces
                .iter()
                .map(|(_, face)| face.display_target.device_id)
                .collect::<Vec<_>>();
            let mut placed = screens.values().cloned().collect::<Vec<_>>();
            placed.sort_by_key(|screen| screen.device_id.as_uuid());
            let active_zone_ids = faces
                .iter()
                .map(|(zone_id, _)| *zone_id)
                .collect::<Vec<_>>();
            let fallback_routes = HashMap::new();
            let black = black_frame();
            let zone_revisions = self.revise(zone_frames);
            let context = DisplayLaneContext {
                elapsed_ms,
                dependency_key: SceneDependencyKey::new(1, 1),
                target_fps: &target_fps,
                routes: DisplayLaneRoutes {
                    current: &face_routes,
                    fallback: &fallback_routes,
                },
                backdrops: ScreenBackdrops {
                    screens: &placed,
                    zone_frames,
                    zone_revisions: &zone_revisions,
                    black: &black,
                },
                fade: self.fade,
            };
            let lanes = {
                let mut compose = self.compose_runtime();
                DisplayLaneMaterializer::new(&mut compose, context).materialize(
                    &active_zone_ids,
                    &faced,
                    faces,
                )
            };
            self.render_zone_runtime
                .screen_fades
                .note_frame(placed, zone_frames.is_empty().then_some([0, 0, 0]));
            lanes
        }

        /// Begin, step or end the scene transition the next frames render
        /// in: its epoch and how far it has come.
        /// Stand in for the scene pass that rendered `zone_frames`: the
        /// frames a transition beginning next takes its bases from.
        fn retain_zone_frames(&mut self, zone_frames: &[(ZoneId, ProducerFrame)]) {
            self.render_zone_runtime
                .retain_zone_frames_for_test(zone_frames);
        }

        fn transition(&mut self, transition: Option<(u64, f32)>) {
            self.fade = transition.map(|(_, weight)| weight);
            let transition =
                transition.map(|(epoch, weight)| ScreenTransition::for_test(epoch, weight));
            let mut compose = self.compose_runtime();
            begin_screen_fades(&mut compose, transition);
        }

        /// Materialize frames a cadence interval apart until `done`, the way
        /// frames land a tick or two after a real GPU is asked for them.
        fn materialize_until(
            &mut self,
            screens: &HashMap<DeviceId, PlacedScreen>,
            zone_frames: &[(ZoneId, ProducerFrame)],
            faces: impl Fn() -> Vec<(ZoneId, PendingDisplayZoneFrame)>,
            done: impl Fn(&MaterializedDisplayLanes) -> bool,
        ) -> MaterializedDisplayLanes {
            let start = std::time::Instant::now();
            loop {
                self.elapsed_ms += 100;
                let lanes = self.materialize(self.elapsed_ms, screens, zone_frames, faces());
                if done(&lanes) {
                    return lanes;
                }
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(5),
                    "display lanes did not settle"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
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
                effect_delta_clock: &mut self.effect_delta_clock,
                render_zone_runtime: &mut self.render_zone_runtime,
                output_artifacts: &mut self.output_artifacts,
                last_published_per_zone: false,
            }
        }
    }
}
