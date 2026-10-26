//! Captures a cover for effects that ship none.
//!
//! Built-in native effects, shaders and user HTML without `<meta cover>` have
//! no art of their own. When one of them is all the active scene shows, and
//! the scene has held still for [`SETTLE`], the keeper captures one frame and
//! keeps it as that effect's cover, so the app, Remote and every cache in
//! between show the same picture of the effect instead of a blank card.
//!
//! What runs is read from the scene the render thread draws, never folded
//! from events: events only say when to look again. A frame is kept only if
//! the scene revision and the effect's metadata are the same after the
//! capture as before it, and only if it shows something: an effect at rest
//! (a reactive effect with nothing to react to, a player with no media) is
//! looked at again every [`RETRY`], up to [`RETRIES`] times, while it still
//! has the canvas. Effects with curated or inline art are never captured,
//! and a stored capture is taken again only after the effect's source
//! changes.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::time::{Instant, sleep_until};
use tracing::{debug, info, warn};

use hypercolor_core::bus::TimestampedEvent;
use hypercolor_types::effect::EffectId;
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::layer::LayerSource;
use hypercolor_types::scene::Zone;

use crate::canvas_capture::{CanvasCaptureRequest, CanvasImageFormat, capture_canvas};
use crate::domain::effect::EffectContext;
use crate::domain::scene::SceneService;
use crate::effect_covers;
use crate::preview_runtime::PreviewRuntime;

/// How long the canvas must hold still before its frame becomes a cover.
pub(crate) const SETTLE: Duration = Duration::from_secs(5);
/// How long to wait before looking again at an effect whose frame was blank.
pub(crate) const RETRY: Duration = Duration::from_secs(20);
/// How many blank frames one settled canvas gets before the keeper waits for
/// the scene to change.
pub(crate) const RETRIES: u32 = 6;
/// Luma spread (standard deviation, 0 to 255) under which a frame is one
/// flat color, black included, and says nothing about the effect.
const MIN_LUMA_SPREAD: f64 = 6.0;

/// What one look at the canvas came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    /// A cover was kept, or there was nothing to cover.
    Done,
    /// The effect showed a blank frame; look again later.
    Blank,
}

/// Whether a JPEG frame shows anything beyond one flat color.
pub(crate) fn shows_something(jpeg: &[u8]) -> bool {
    let Ok(image) = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg) else {
        return false;
    };
    let luma = image.to_luma8();
    let count = f64::from(u32::try_from(luma.len()).unwrap_or(u32::MAX).max(1));
    let mean = luma.iter().map(|&value| f64::from(value)).sum::<f64>() / count;
    let variance = luma
        .iter()
        .map(|&value| (f64::from(value) - mean).powi(2))
        .sum::<f64>()
        / count;
    variance.sqrt() >= MIN_LUMA_SPREAD
}

/// The one effect whose frames fill the canvas, or `None` when several
/// effects or any other source share it.
///
/// Every visible layer in every resolved zone must be that effect: a color,
/// media or capture layer on top would put something else in the picture.
pub(crate) fn sole_effect(zones: &[Zone]) -> Option<EffectId> {
    let mut sole = None;
    let visible = zones
        .iter()
        .filter(|zone| zone.enabled)
        .flat_map(|zone| zone.layers.iter())
        .filter(|layer| layer.enabled && layer.opacity > 0.0);
    for layer in visible {
        let LayerSource::Effect { effect_id, .. } = layer.source else {
            return None;
        };
        match sole {
            None => sole = Some(effect_id),
            Some(running) if running == effect_id => {}
            Some(_) => return None,
        }
    }
    sole
}

/// Events after which the canvas may show something else.
pub(crate) const fn reshapes_canvas(event: &HypercolorEvent) -> bool {
    matches!(
        event,
        HypercolorEvent::EffectStarted { .. }
            | HypercolorEvent::EffectStopped { .. }
            | HypercolorEvent::EffectRegistryUpdated { .. }
            | HypercolorEvent::SceneActivated { .. }
            | HypercolorEvent::ActiveSceneChanged { .. }
            | HypercolorEvent::SceneTransitionStarted { .. }
            | HypercolorEvent::SceneTransitionComplete { .. }
            | HypercolorEvent::SceneEnabled { .. }
            | HypercolorEvent::ZoneChanged { .. }
            | HypercolorEvent::LayerStackChanged { .. }
            | HypercolorEvent::SceneSettingsChanged { .. }
            | HypercolorEvent::SceneLibraryChanged { .. }
    )
}

/// Follow the bus until it closes, capturing covers whenever the canvas
/// settles. The first look happens [`SETTLE`] after startup, so a restored
/// scene is covered too.
pub(crate) async fn run(
    effects: EffectContext,
    scenes: SceneService,
    preview: Arc<PreviewRuntime>,
    mut events: Receiver<TimestampedEvent>,
) {
    info!("Effect cover keeper active");
    let mut due = Some(Instant::now() + SETTLE);
    let mut blanks = 0;
    loop {
        let settle = async {
            match due {
                Some(deadline) => sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) => {
                    if reshapes_canvas(&event.event) {
                        due = Some(Instant::now() + SETTLE);
                        blanks = 0;
                    }
                }
                // Missed events may have reshaped the canvas; look again.
                Err(RecvError::Lagged(_)) => {
                    due = Some(Instant::now() + SETTLE);
                    blanks = 0;
                }
                Err(RecvError::Closed) => break,
            },
            () = settle => {
                due = None;
                if capture_if_wanted(&effects, &scenes, &preview).await == Look::Blank {
                    blanks += 1;
                    if blanks < RETRIES {
                        due = Some(Instant::now() + RETRY);
                    }
                }
            }
        }
    }
}

async fn capture_if_wanted(
    effects: &EffectContext,
    scenes: &SceneService,
    preview: &PreviewRuntime,
) -> Look {
    let revision = scenes.revision();
    let Some(effect_id) = scenes
        .read_with(|manager| sole_effect(&manager.resolved_zones()))
        .await
    else {
        return Look::Done;
    };
    let Some(metadata) = effects.resolve_metadata(&effect_id.to_string()).await else {
        return Look::Done;
    };
    let probe = metadata.clone();
    let wanted = tokio::task::spawn_blocking(move || effect_covers::wants_capture(&probe))
        .await
        .unwrap_or(false);
    if !wanted {
        return Look::Done;
    }
    let frame = match capture_canvas(
        preview,
        CanvasCaptureRequest::single(CanvasImageFormat::Jpeg),
    )
    .await
    {
        Ok(mut frames) => frames.pop(),
        Err(error) => {
            debug!(effect = %metadata.name, %error, "No canvas frame to keep as a cover");
            return Look::Done;
        }
    };
    // A frame from before the render loop woke may show the previous effect.
    let Some(frame) = frame.filter(|frame| frame.fresh) else {
        return Look::Blank;
    };
    // Anything that reshaped the scene or reloaded the effect while the
    // frame was taken voids it; the event that did so re-arms the keeper.
    if scenes.revision() != revision {
        return Look::Done;
    }
    if effects
        .resolve_metadata(&effect_id.to_string())
        .await
        .as_ref()
        != Some(&metadata)
    {
        return Look::Done;
    }
    let jpeg = frame.bytes;
    let (shows, jpeg) = tokio::task::spawn_blocking(move || (shows_something(&jpeg), jpeg))
        .await
        .unwrap_or_else(|_| (false, Vec::new()));
    if !shows {
        debug!(effect = %metadata.name, "Effect showed a blank frame; looking again later");
        return Look::Blank;
    }
    let name = metadata.name.clone();
    match tokio::task::spawn_blocking(move || effect_covers::store_captured(&metadata, &jpeg)).await
    {
        Ok(Ok(())) => info!(effect = %name, "Captured a cover for an effect that ships none"),
        Ok(Err(error)) => warn!(effect = %name, %error, "Failed to keep a captured cover"),
        Err(error) => warn!(effect = %name, %error, "Cover capture task failed"),
    }
    Look::Done
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use hypercolor_types::layer::{
        BlendMode, LayerAdjust, LayerTransform, SceneLayer, SceneLayerId,
    };
    use hypercolor_types::scene::{ZoneId, ZoneRole};
    use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

    use super::*;

    fn zone(layers: Vec<SceneLayer>) -> Zone {
        Zone {
            id: ZoneId::new(),
            name: "Cover".to_owned(),
            description: None,
            layers,
            layout: SpatialLayout {
                id: "cover-test".to_owned(),
                name: "Cover Test".to_owned(),
                description: None,
                canvas_width: 8,
                canvas_height: 6,
                zones: Vec::new(),
                default_sampling_mode: SamplingMode::Bilinear,
                default_edge_behavior: EdgeBehavior::Clamp,
                version: 1,
            },
            brightness: 1.0,
            enabled: true,
            color: None,
            display_target: None,
            role: ZoneRole::Primary,
            controls_version: 0,
            layers_version: 0,
        }
    }

    fn id(value: u128) -> EffectId {
        EffectId::new(uuid::Uuid::from_u128(value))
    }

    fn effect(value: u128) -> SceneLayer {
        SceneLayer::from_effect(
            SceneLayerId::new(),
            id(value),
            HashMap::new(),
            HashMap::new(),
            None,
        )
    }

    fn color() -> SceneLayer {
        SceneLayer {
            id: SceneLayerId::new(),
            name: None,
            source: LayerSource::ColorFill {
                rgba: [1.0, 0.0, 1.0, 1.0],
            },
            blend: BlendMode::Replace,
            opacity: 1.0,
            transform: LayerTransform::default(),
            adjust: LayerAdjust::default(),
            bindings: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn one_effect_alone_owns_the_canvas() {
        assert_eq!(sole_effect(&[zone(vec![effect(1)])]), Some(id(1)));
    }

    #[test]
    fn the_same_effect_in_every_zone_still_owns_the_canvas() {
        assert_eq!(
            sole_effect(&[zone(vec![effect(1)]), zone(vec![effect(1)])]),
            Some(id(1))
        );
    }

    #[test]
    fn two_effects_share_the_canvas_and_neither_is_captured() {
        assert_eq!(
            sole_effect(&[zone(vec![effect(1)]), zone(vec![effect(2)])]),
            None
        );
        assert_eq!(sole_effect(&[zone(vec![effect(1), effect(2)])]), None);
    }

    #[test]
    fn another_source_in_the_picture_rules_out_a_capture() {
        assert_eq!(sole_effect(&[zone(vec![effect(1), color()])]), None);
    }

    #[test]
    fn hidden_layers_and_disabled_zones_do_not_count() {
        let mut hidden = effect(2);
        hidden.enabled = false;
        let mut clear = color();
        clear.opacity = 0.0;
        let mut off = zone(vec![effect(3)]);
        off.enabled = false;
        assert_eq!(
            sole_effect(&[zone(vec![effect(1), hidden, clear]), off]),
            Some(id(1))
        );
    }

    #[test]
    fn an_empty_scene_has_no_cover_candidate() {
        assert_eq!(sole_effect(&[zone(Vec::new())]), None);
    }

    fn jpeg(pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_fn(32, 20, |x, y| image::Rgb(pixel(x, y)));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .expect("encode test frame");
        bytes.into_inner()
    }

    #[test]
    fn a_flat_frame_is_not_a_cover() {
        assert!(!shows_something(&jpeg(|_, _| [0, 0, 0])));
        assert!(!shows_something(&jpeg(|_, _| [12, 4, 20])));
        assert!(!shows_something(b"not a jpeg"));
    }

    #[test]
    fn a_frame_with_a_picture_is_a_cover() {
        assert!(shows_something(&jpeg(|x, _| {
            let level = u8::try_from(x * 8).unwrap_or(u8::MAX);
            [level, 40, 255 - level]
        })));
    }

    #[test]
    fn scene_and_effect_changes_rearm_the_keeper() {
        assert!(reshapes_canvas(&HypercolorEvent::EffectRegistryUpdated {
            added: 0,
            removed: 0,
            updated: 1,
        }));
    }
}
