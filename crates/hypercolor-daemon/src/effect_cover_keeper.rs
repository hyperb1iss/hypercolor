//! Captures a cover for effects that ship none.
//!
//! Built-in native effects, shaders and user HTML without `<meta cover>` have
//! no art of their own. When one of them has been the only effect on the
//! canvas for [`SETTLE`], the keeper captures one frame and keeps it as that
//! effect's cover, so the app, Remote and every cache in between show the
//! same picture of the effect instead of a blank card. Effects with curated
//! or inline art are never captured, and a stored capture is taken again
//! only after the effect's source changes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::time::{Instant, sleep_until};
use tracing::{debug, info, warn};

use hypercolor_core::bus::TimestampedEvent;
use hypercolor_types::event::HypercolorEvent;
use hypercolor_types::scene::ZoneId;

use crate::canvas_capture::{CanvasCaptureRequest, CanvasImageFormat, capture_canvas};
use crate::domain::effect::EffectContext;
use crate::effect_covers;
use crate::preview_runtime::PreviewRuntime;

/// How long an effect must own the canvas before its frame becomes a cover.
pub(crate) const SETTLE: Duration = Duration::from_secs(5);

/// Which effect runs in which zone, as the bus last reported it.
#[derive(Debug, Default)]
pub(crate) struct Stage {
    zones: HashMap<Option<ZoneId>, String>,
}

impl Stage {
    /// Fold one bus event in. Returns whether what runs where changed.
    pub(crate) fn apply(&mut self, event: &HypercolorEvent) -> bool {
        match event {
            HypercolorEvent::EffectStarted {
                effect, zone_id, ..
            } => self.zones.insert(*zone_id, effect.id.clone()).as_ref() != Some(&effect.id),
            HypercolorEvent::EffectStopped {
                effect, zone_id, ..
            } => {
                if zone_id.is_some() {
                    return self.zones.remove(zone_id).is_some();
                }
                let before = self.zones.len();
                self.zones.retain(|_, running| *running != effect.id);
                self.zones.len() != before
            }
            _ => false,
        }
    }

    /// The one effect whose frames fill the canvas, if exactly one runs.
    pub(crate) fn sole_effect(&self) -> Option<&str> {
        let mut effects = self.zones.values();
        let first = effects.next()?;
        effects
            .all(|other| other == first)
            .then_some(first.as_str())
    }

    fn clear(&mut self) {
        self.zones.clear();
    }
}

/// Follow the bus until it closes, capturing covers as effects settle.
pub(crate) async fn run(
    effects: EffectContext,
    preview: Arc<PreviewRuntime>,
    mut events: Receiver<TimestampedEvent>,
) {
    info!("Effect cover keeper active");
    let mut stage = Stage::default();
    let mut due: Option<Instant> = None;
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
                    if stage.apply(&event.event) {
                        due = stage.sole_effect().map(|_| Instant::now() + SETTLE);
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    // What runs where is unknown until the next start.
                    debug!(skipped, "Effect cover keeper lagged; waiting for the next effect");
                    stage.clear();
                    due = None;
                }
                Err(RecvError::Closed) => break,
            },
            () = settle => {
                due = None;
                if let Some(effect_id) = stage.sole_effect().map(str::to_owned) {
                    capture_if_wanted(&effects, &preview, &effect_id).await;
                }
            }
        }
    }
}

async fn capture_if_wanted(effects: &EffectContext, preview: &PreviewRuntime, effect_id: &str) {
    let Some(metadata) = effects.resolve_metadata(effect_id).await else {
        return;
    };
    let probe = metadata.clone();
    let wanted = tokio::task::spawn_blocking(move || effect_covers::wants_capture(&probe))
        .await
        .unwrap_or(false);
    if !wanted {
        return;
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
            return;
        }
    };
    // A frame from before the render loop woke may show the previous effect.
    let Some(frame) = frame.filter(|frame| frame.fresh) else {
        return;
    };
    let name = metadata.name.clone();
    match tokio::task::spawn_blocking(move || {
        effect_covers::store_captured(&metadata, &frame.bytes)
    })
    .await
    {
        Ok(Ok(())) => info!(effect = %name, "Captured a cover for an effect that ships none"),
        Ok(Err(error)) => warn!(effect = %name, %error, "Failed to keep a captured cover"),
        Err(error) => warn!(effect = %name, %error, "Cover capture task failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::event::{ChangeTrigger, EffectRef, EffectStopReason};

    fn effect(id: &str) -> EffectRef {
        EffectRef {
            id: id.to_owned(),
            name: id.to_owned(),
            engine: "wgpu".to_owned(),
        }
    }

    fn started(id: &str, zone: Option<ZoneId>) -> HypercolorEvent {
        HypercolorEvent::EffectStarted {
            effect: effect(id),
            trigger: ChangeTrigger::Api,
            previous: None,
            transition: None,
            zone_id: zone,
            zone_name: None,
        }
    }

    fn stopped(id: &str, zone: Option<ZoneId>) -> HypercolorEvent {
        HypercolorEvent::EffectStopped {
            effect: effect(id),
            reason: EffectStopReason::Replaced,
            zone_id: zone,
            zone_name: None,
        }
    }

    #[test]
    fn one_effect_alone_owns_the_canvas() {
        let mut stage = Stage::default();
        assert!(stage.apply(&started("aurora", None)));
        assert_eq!(stage.sole_effect(), Some("aurora"));
    }

    #[test]
    fn the_same_effect_in_every_zone_still_owns_the_canvas() {
        let mut stage = Stage::default();
        stage.apply(&started("aurora", Some(ZoneId::new())));
        stage.apply(&started("aurora", Some(ZoneId::new())));
        assert_eq!(stage.sole_effect(), Some("aurora"));
    }

    #[test]
    fn two_effects_share_the_canvas_and_neither_is_captured() {
        let mut stage = Stage::default();
        stage.apply(&started("aurora", Some(ZoneId::new())));
        stage.apply(&started("plasma", Some(ZoneId::new())));
        assert_eq!(stage.sole_effect(), None);
    }

    #[test]
    fn stopping_one_of_two_leaves_the_other_alone() {
        let mut stage = Stage::default();
        let left = ZoneId::new();
        stage.apply(&started("aurora", Some(left)));
        stage.apply(&started("plasma", Some(ZoneId::new())));
        assert!(stage.apply(&stopped("aurora", Some(left))));
        assert_eq!(stage.sole_effect(), Some("plasma"));
    }

    #[test]
    fn a_restart_of_the_same_effect_is_not_a_change() {
        let mut stage = Stage::default();
        let zone = Some(ZoneId::new());
        stage.apply(&started("aurora", zone));
        assert!(!stage.apply(&started("aurora", zone)));
    }

    #[test]
    fn a_zoneless_stop_clears_that_effect_everywhere() {
        let mut stage = Stage::default();
        stage.apply(&started("aurora", Some(ZoneId::new())));
        stage.apply(&started("aurora", Some(ZoneId::new())));
        assert!(stage.apply(&stopped("aurora", None)));
        assert_eq!(stage.sole_effect(), None);
    }
}
