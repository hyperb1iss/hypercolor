//! The crossfade each screen runs while a scene transition does.
//!
//! A screen fades the way the composed scene and the LEDs do: from a frozen
//! base to the incoming picture by the transition's eased progress, all on
//! the render thread, so the frame a screen's lane publishes is the finished
//! picture. The base is the backdrop the screen showed when the transition
//! began, before any face: a face is laid over the mix and stays as sharp
//! as it is (Spec 95 §5.10).
//!
//! The base is held as a picture in panel space, not as the outgoing zone's
//! frame and viewport. A transition that interrupts another has to start
//! from the mix that was on the panel, and only a picture can hold a mix.
//! It is taken once per screen when a transition's key first appears, from
//! what the frame before left behind anyway, so a frame outside a
//! transition does nothing here but compare two keys.

use std::collections::HashMap;

use hypercolor_core::bus::{CanvasFrame, DisplayZoneFrame, DisplayZoneViewport};
use hypercolor_types::canvas::Canvas;
use hypercolor_types::device::DeviceId;
#[cfg(feature = "wgpu")]
use hypercolor_types::device::DisplayFrameFormat;
#[cfg(feature = "wgpu")]
use hypercolor_types::layer::BlendMode;
use hypercolor_types::scene::SceneId;
#[cfg(feature = "wgpu")]
use hypercolor_types::scene::ZoneId;
use tracing::debug;

use super::display_lane::PlacedScreen;
use super::pipeline_runtime::ComposeRuntime;
#[cfg(feature = "wgpu")]
use super::producer_queue::ProducerFrame;
use super::scene_snapshot::SceneRuntimeSnapshot;
#[cfg(feature = "wgpu")]
use super::sparkleflinger::{
    DisplayFinalizeCacheKey, DisplayFinalizeDispatch, DisplayFinalizeFade, DisplayFinalizeFadeBase,
    DisplayFinalizeParams,
};
use crate::display_output::PanelCropper;

/// Which scene transition a frame renders in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenTransitionKey {
    epoch: u64,
    from_scene: Option<SceneId>,
    to_scene: Option<SceneId>,
}

/// The scene transition a frame renders in, as far as its screens go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScreenTransition {
    pub(super) key: ScreenTransitionKey,
    /// How far the incoming scene has faded in, eased, from 0 to 1.
    pub(super) weight: f32,
}

impl ScreenTransition {
    #[cfg(test)]
    pub(super) fn for_test(epoch: u64, weight: f32) -> Self {
        Self {
            key: ScreenTransitionKey {
                epoch,
                from_scene: None,
                to_scene: None,
            },
            weight,
        }
    }

    pub(super) fn of(scene_runtime: &SceneRuntimeSnapshot) -> Option<Self> {
        let transition = scene_runtime.active_transition.as_ref()?;
        Some(Self {
            key: ScreenTransitionKey {
                epoch: transition.epoch,
                from_scene: transition.from_scene,
                to_scene: transition.to_scene,
            },
            weight: transition.eased_progress.clamp(0.0, 1.0),
        })
    }
}

/// The backdrop a screen fades from.
struct ScreenFadeBase {
    /// The panel the base was taken for. A screen whose panel has changed
    /// since has no base, and cuts to the incoming picture.
    width: u32,
    height: u32,
    picture: FadePicture,
}

enum FadePicture {
    /// Nothing: the screen's zone rendered nothing. Only a compositor that
    /// finalizes keeps it apart; as pixels, black is a picture like any.
    #[cfg(feature = "wgpu")]
    Black,
    /// On the GPU, as the output of one of the screen's two base routes.
    #[cfg(feature = "wgpu")]
    Route { slot: u8 },
    /// As pixels, where the display workers finish the frames.
    Pixels(CanvasFrame),
}

/// Every screen's fade through the scene transition in progress.
#[derive(Default)]
pub(crate) struct ScreenFades {
    /// The transition the bases were taken for.
    transition: Option<ScreenTransitionKey>,
    /// How far that transition had come when the screens were last mixed:
    /// the weight of the mix a transition that interrupts it starts from.
    weight: f32,
    bases: HashMap<DeviceId, ScreenFadeBase>,
    /// The screens of the last frame, as its scene placed them.
    last_screens: Vec<PlacedScreen>,
    /// The one color every placed screen showed in the last frame, when it
    /// showed no zone: black for a scene that rendered nothing, the off
    /// color while output was off.
    last_flat: Option<[u8; 3]>,
    cropper: Option<PanelCropper>,
}

impl ScreenFades {
    /// Record what a frame's screens were, for the transition that may begin
    /// with the next frame to take its bases from.
    pub(super) fn note_frame(
        &mut self,
        screens: Vec<PlacedScreen>,
        flat: Option<[u8; 3]>,
        weight: Option<f32>,
    ) {
        self.last_screens = screens;
        self.last_flat = flat;
        if let Some(weight) = weight {
            self.weight = weight;
        }
    }

    /// Output went off: every placed screen shows `color` now, whatever it
    /// was fading through. The fades are over, their bases go, and a
    /// transition that begins before output is back starts from `color`.
    pub(super) fn retire_for_off_output(&mut self, screens: Vec<PlacedScreen>, color: [u8; 3]) {
        self.transition = None;
        self.bases.clear();
        self.weight = 0.0;
        self.last_screens = screens;
        self.last_flat = Some(color);
    }

    /// Whether any screen is part way through a fade.
    pub(super) fn in_progress(&self) -> bool {
        self.transition.is_some() && !self.bases.is_empty()
    }

    fn base(&self, device_id: DeviceId, width: u32, height: u32) -> Option<&ScreenFadeBase> {
        self.bases
            .get(&device_id)
            .filter(|base| (base.width, base.height) == (width, height))
    }

    /// The fade a finalize pass for `device_id` runs at `weight`, when the
    /// screen has a base for a panel of this size.
    #[cfg(feature = "wgpu")]
    pub(super) fn finalize_fade(
        &self,
        device_id: DeviceId,
        width: u32,
        height: u32,
        weight: f32,
    ) -> Option<DisplayFinalizeFade> {
        let base = self.base(device_id, width, height)?;
        Some(DisplayFinalizeFade {
            base: match base.picture {
                FadePicture::Black => DisplayFinalizeFadeBase::Black,
                FadePicture::Route { slot } => {
                    DisplayFinalizeFadeBase::Route(base_route_key(device_id, slot, width, height))
                }
                FadePicture::Pixels(_) => return None,
            },
            weight,
        })
    }

    /// The GPU routes the bases in use live on, which have to stay cached
    /// while the fade runs.
    #[cfg(feature = "wgpu")]
    pub(super) fn base_route_ids(&self) -> impl Iterator<Item = ZoneId> + '_ {
        self.bases
            .iter()
            .filter_map(|(device_id, base)| match base.picture {
                FadePicture::Route { slot } => Some(base_route_id(*device_id, slot)),
                FadePicture::Black | FadePicture::Pixels(_) => None,
            })
    }

    /// Whether `device_id` has a base for the display workers' pixels.
    pub(super) fn fades_pixels(&self, device_id: DeviceId, width: u32, height: u32) -> bool {
        self.base(device_id, width, height)
            .is_some_and(|base| matches!(base.picture, FadePicture::Pixels(_)))
    }

    /// The screen's backdrop part way through the fade: `incoming` cropped
    /// to the panel through `viewport` and mixed over the screen's base at
    /// `weight`, before the panel's brightness and mask.
    pub(super) fn mix_pixels(
        &mut self,
        device_id: DeviceId,
        incoming: &CanvasFrame,
        viewport: &DisplayZoneViewport,
        width: u32,
        height: u32,
        weight: f32,
    ) -> Option<CanvasFrame> {
        let mut pixels = self.crop(incoming, viewport, width, height)?;
        let base = self.base(device_id, width, height)?;
        match &base.picture {
            FadePicture::Pixels(base) if base.rgba_bytes().len() == pixels.len() => {
                mix_rgba(base.rgba_bytes(), &mut pixels, weight);
            }
            FadePicture::Pixels(_) => return None,
            #[cfg(feature = "wgpu")]
            FadePicture::Black | FadePicture::Route { .. } => return None,
        }
        Some(CanvasFrame::from_owned_canvas(
            Canvas::from_vec(pixels, width, height),
            0,
            0,
        ))
    }

    /// `source` cropped to a panel the way a display worker crops it.
    fn crop(
        &mut self,
        source: &CanvasFrame,
        viewport: &DisplayZoneViewport,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        if self.cropper.is_none() {
            self.cropper = PanelCropper::new()
                .inspect_err(|error| debug!(%error, "screen fade could not start its cropper"))
                .ok();
        }
        self.cropper
            .as_mut()?
            .crop(source, viewport, width, height)
            .inspect_err(|error| debug!(%error, "screen fade could not crop a backdrop"))
            .ok()
            .map(<[u8]>::to_vec)
    }
}

/// Start, restart or end the screens' fades for the frame about to render.
///
/// Called before anything of the frame renders, while the frame before is
/// still what the screens show: a transition whose key is new takes each
/// screen's base from it. A key that has not changed leaves everything as
/// it is, which is every frame outside a transition and every frame of one
/// but its first.
pub(super) fn begin_screen_fades(
    compose: &mut ComposeRuntime<'_>,
    transition: Option<ScreenTransition>,
) {
    let key = transition.map(|transition| transition.key);
    if compose.render_zone_runtime.screen_fades.transition == key {
        return;
    }
    let mut fades = std::mem::take(&mut compose.render_zone_runtime.screen_fades);
    // Whether a transition was already running, and this one interrupts it.
    let interrupted = std::mem::replace(&mut fades.transition, key).is_some();
    if key.is_none() {
        fades.bases.clear();
        compose.render_zone_runtime.screen_fades = fades;
        return;
    }

    #[cfg(feature = "wgpu")]
    let finalizes = compose.display_sparkleflinger.finalizes_display_frames();
    #[cfg(not(feature = "wgpu"))]
    let finalizes = false;
    let mut bases = HashMap::with_capacity(fades.last_screens.len());
    if finalizes {
        // A transition that interrupts another starts from the mix on the
        // panel: the last incoming backdrop over the old base, at the
        // weight it had.
        #[cfg(feature = "wgpu")]
        let interrupted_weight = interrupted.then_some(fades.weight);
        #[cfg(feature = "wgpu")]
        for screen in &fades.last_screens {
            let (width, height) = (screen.output.width, screen.output.height);
            let previous = interrupted_weight
                .zip(fades.base(screen.device_id, width, height))
                .map(|(weight, base)| (weight, &base.picture));
            if let Some(picture) = base_on_gpu(compose, screen, fades.last_flat, previous) {
                bases.insert(
                    screen.device_id,
                    ScreenFadeBase {
                        width,
                        height,
                        picture,
                    },
                );
            }
        }
    } else {
        // Where the workers finish frames, each screen's last backdrop is
        // retained with the viewport it was published with, and a backdrop
        // published mid-fade is the mix itself: a transition that
        // interrupts another needs nothing more.
        let _ = interrupted;
        let retained = compose
            .render_zone_runtime
            .retained_backdrops()
            .map(|(screen, frame)| (screen.clone(), frame.frame.clone()))
            .collect::<Vec<_>>();
        for (screen, frame) in retained {
            let DisplayZoneFrame::Backdrop(backdrop) = frame else {
                continue;
            };
            let (width, height) = (screen.output.width, screen.output.height);
            let picture = if backdrop.cropped {
                Some(backdrop.frame)
            } else {
                fades
                    .crop(&backdrop.frame, &backdrop.viewport, width, height)
                    .map(|pixels| {
                        CanvasFrame::from_owned_canvas(
                            Canvas::from_vec(pixels, width, height),
                            0,
                            0,
                        )
                    })
            };
            if let Some(picture) = picture {
                bases.insert(
                    screen.device_id,
                    ScreenFadeBase {
                        width,
                        height,
                        picture: FadePicture::Pixels(picture),
                    },
                );
            }
        }
        // A screen with no backdrop retained showed the frame's one color,
        // if the frame had one: the off color of output that was off.
        if let Some([r, g, b]) = fades.last_flat {
            for screen in &fades.last_screens {
                let (width, height) = (screen.output.width, screen.output.height);
                bases.entry(screen.device_id).or_insert_with(|| {
                    let mut flat = Canvas::new(width, height);
                    flat.fill(hypercolor_types::canvas::Rgba::new(r, g, b, 255));
                    ScreenFadeBase {
                        width,
                        height,
                        picture: FadePicture::Pixels(CanvasFrame::from_owned_canvas(flat, 0, 0)),
                    }
                });
            }
        }
    }
    fades.bases = bases;
    fades.weight = 0.0;
    compose.render_zone_runtime.screen_fades = fades;
}

/// Take a screen's base on the GPU: the backdrop it showed in the last
/// frame, drawn once to panel space, over the base of the transition this
/// one interrupts if there was one.
///
/// The picture is the output of a backdrop-only finalize pass on a route of
/// its own, kept on the GPU and never read back. A screen has two such
/// routes, so a base can be drawn from the one before it.
#[cfg(feature = "wgpu")]
fn base_on_gpu(
    compose: &mut ComposeRuntime<'_>,
    screen: &PlacedScreen,
    last_flat: Option<[u8; 3]>,
    previous: Option<(f32, &FadePicture)>,
) -> Option<FadePicture> {
    let (width, height) = (screen.output.width, screen.output.height);
    // What the screen showed in the last frame: the frame's one color when
    // it had one, else its zone's frame, or nothing when the zone rendered
    // nothing. A frame nobody kept is not known to be black, and gives no
    // base.
    let flat;
    let outgoing = match last_flat {
        Some([0, 0, 0]) => None,
        Some(color) => {
            flat = ProducerFrame::Surface(
                compose
                    .output_artifacts
                    .static_surface(width, height, color),
            );
            Some(&flat)
        }
        None => compose
            .render_zone_runtime
            .retained_zone_frames()?
            .iter()
            .find_map(|(zone_id, frame)| (*zone_id == screen.zone_id).then_some(frame)),
    };
    let (slot, fade) = match previous {
        None => (0, None),
        Some((_, FadePicture::Pixels(_))) => return None,
        Some((weight, FadePicture::Black)) => (
            0,
            Some(DisplayFinalizeFade {
                base: DisplayFinalizeFadeBase::Black,
                weight,
            }),
        ),
        Some((weight, FadePicture::Route { slot })) => (
            1 - *slot,
            Some(DisplayFinalizeFade {
                base: DisplayFinalizeFadeBase::Route(base_route_key(
                    screen.device_id,
                    *slot,
                    width,
                    height,
                )),
                weight,
            }),
        ),
    };
    let fades_from_black = fade.is_none_or(|fade| fade.base == DisplayFinalizeFadeBase::Black);
    if outgoing.is_none() && fades_from_black {
        return Some(FadePicture::Black);
    }

    let viewport = screen.output.viewport;
    let params = DisplayFinalizeParams {
        cache_key: base_route_key(screen.device_id, slot, width, height),
        width,
        height,
        // The base is the backdrop as its zone drew it. The panel's
        // brightness and mask belong to the pass that mixes it.
        circular: false,
        brightness: 1.0,
        viewport_position: viewport.position,
        viewport_size: viewport.size,
        viewport_rotation: viewport.rotation,
        viewport_scale: viewport.scale,
        viewport_edge_behavior: viewport.edge_behavior,
        blend_mode: BlendMode::Alpha,
        opacity: 0.0,
        fade,
    };
    let black;
    let source = if let Some(frame) = outgoing {
        frame
    } else {
        black = ProducerFrame::Surface(compose.output_artifacts.static_surface(
            width,
            height,
            [0, 0, 0],
        ));
        &black
    };
    match compose
        .display_sparkleflinger
        .begin_finalize_display_backdrop(source, params)
    {
        Ok(DisplayFinalizeDispatch::Pending(pending)) => {
            // The picture stays on the GPU; nothing waits for its readback.
            compose
                .display_sparkleflinger
                .discard_pending_display_finalization(pending);
            Some(FadePicture::Route { slot })
        }
        Ok(DisplayFinalizeDispatch::Unsupported | DisplayFinalizeDispatch::Saturated) => None,
        Err(error) => {
            debug!(%error, "screen fade could not take a base; the screen will cut");
            None
        }
    }
}

/// The GPU route one of a screen's two bases lives on. The ids are derived
/// from the device and distinct from every scene zone's and every lane's.
#[cfg(feature = "wgpu")]
fn base_route_id(device_id: DeviceId, slot: u8) -> ZoneId {
    const NAMESPACES: [uuid::Uuid; 2] = [
        uuid::Uuid::from_u128(0x7363_7265_656e_2d66_6164_652d_6261_7330),
        uuid::Uuid::from_u128(0x7363_7265_656e_2d66_6164_652d_6261_7331),
    ];
    ZoneId(uuid::Uuid::new_v5(
        &NAMESPACES[usize::from(slot.min(1))],
        device_id.as_uuid().as_bytes(),
    ))
}

#[cfg(feature = "wgpu")]
fn base_route_key(
    device_id: DeviceId,
    slot: u8,
    width: u32,
    height: u32,
) -> DisplayFinalizeCacheKey {
    DisplayFinalizeCacheKey {
        zone_id: base_route_id(device_id, slot),
        device_id,
        width,
        height,
        circular: false,
        frame_format: DisplayFrameFormat::Rgb,
    }
}

/// Mix the `incoming` picture over `base` in place, alpha left opaque: a
/// `weight` of 0 keeps the base and 1 the incoming picture.
#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the weight is clamped to 0..=256 and the mix of two bytes fits a byte"
)]
fn mix_rgba(base: &[u8], incoming: &mut [u8], weight: f32) {
    let weight = if weight.is_finite() {
        weight.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let incoming_weight = (weight * 256.0 + 0.5) as u32;
    let base_weight = 256 - incoming_weight;
    let mix = |base: u8, incoming: u8| {
        ((u32::from(base) * base_weight + u32::from(incoming) * incoming_weight + 128) >> 8) as u8
    };
    for (incoming, base) in incoming
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(base.as_chunks::<4>().0)
    {
        *incoming = [
            mix(base[0], incoming[0]),
            mix(base[1], incoming[1]),
            mix(base[2], incoming[2]),
            u8::MAX,
        ];
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_core::bus::{CanvasFrame, DisplayZoneViewport};
    use hypercolor_types::canvas::{Canvas, Rgba};
    use hypercolor_types::device::DeviceId;
    use hypercolor_types::spatial::{EdgeBehavior, NormalizedPosition};

    use super::{FadePicture, ScreenFadeBase, ScreenFades, ScreenTransition, mix_rgba};

    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> CanvasFrame {
        let mut canvas = Canvas::new(width, height);
        canvas.fill(Rgba::new(rgb[0], rgb[1], rgb[2], 255));
        CanvasFrame::from_owned_canvas(canvas, 0, 0)
    }

    fn whole_canvas() -> DisplayZoneViewport {
        DisplayZoneViewport {
            position: NormalizedPosition::new(0.5, 0.5),
            size: NormalizedPosition::new(1.0, 1.0),
            rotation: 0.0,
            scale: 1.0,
            edge_behavior: EdgeBehavior::Clamp,
        }
    }

    /// A base is a picture for one panel. A screen whose panel has another
    /// shape since, the same number of pixels or not, has no base: its rows
    /// would be read at the wrong width. It cuts to the incoming picture.
    #[test]
    fn base_taken_for_another_panel_is_no_base() {
        let screen = DeviceId::new();
        let mut fades = ScreenFades::default();
        fades.bases.insert(
            screen,
            ScreenFadeBase {
                width: 4,
                height: 2,
                picture: FadePicture::Pixels(solid(4, 2, [255, 0, 0])),
            },
        );
        let incoming = solid(8, 8, [0, 0, 255]);

        assert!(fades.fades_pixels(screen, 4, 2));
        let mixed = fades
            .mix_pixels(screen, &incoming, &whole_canvas(), 4, 2, 0.5)
            .expect("the panel the base was taken for fades");
        assert_eq!(&mixed.rgba_bytes()[..4], [128, 0, 128, 255]);

        // The same pixel count, turned on its side.
        assert!(!fades.fades_pixels(screen, 2, 4));
        assert!(
            fades
                .mix_pixels(screen, &incoming, &whole_canvas(), 2, 4, 0.5)
                .is_none()
        );
        #[cfg(feature = "wgpu")]
        {
            fades.bases.insert(
                screen,
                ScreenFadeBase {
                    width: 4,
                    height: 2,
                    picture: FadePicture::Route { slot: 0 },
                },
            );
            assert!(fades.finalize_fade(screen, 4, 2, 0.5).is_some());
            assert!(fades.finalize_fade(screen, 2, 4, 0.5).is_none());
        }
    }

    /// Output going off ends every fade. No base is left to hold a route on
    /// the GPU or to hand the next transition as its history, and what the
    /// screens show is the color output went off to.
    #[test]
    fn output_going_off_leaves_no_fade_behind() {
        let screen = DeviceId::new();
        let mut fades = ScreenFades {
            transition: Some(ScreenTransition::for_test(1, 0.5).key),
            weight: 0.5,
            ..ScreenFades::default()
        };
        fades.bases.insert(
            screen,
            ScreenFadeBase {
                width: 4,
                height: 2,
                picture: FadePicture::Pixels(solid(4, 2, [255, 0, 0])),
            },
        );
        assert!(fades.in_progress());

        fades.retire_for_off_output(Vec::new(), [0, 0, 0]);

        assert!(!fades.in_progress());
        assert!(!fades.fades_pixels(screen, 4, 2));
        assert_eq!(fades.transition, None);
        assert_eq!(fades.last_flat, Some([0, 0, 0]));
        #[cfg(feature = "wgpu")]
        assert_eq!(fades.base_route_ids().count(), 0);
    }

    #[test]
    fn mix_runs_from_the_base_to_the_incoming_picture() {
        let base = [255, 0, 0, 255, 10, 20, 30, 255];
        let incoming = [0, 0, 255, 255, 10, 20, 30, 255];

        let mut start = incoming;
        mix_rgba(&base, &mut start, 0.0);
        assert_eq!(start, base);

        let mut half = incoming;
        mix_rgba(&base, &mut half, 0.5);
        assert_eq!(half, [128, 0, 128, 255, 10, 20, 30, 255]);

        let mut end = incoming;
        mix_rgba(&base, &mut end, 1.0);
        assert_eq!(end, incoming);
    }
}
