//! Which zone the main preview follows.
//!
//! A multi-zone scene's scene canvas projects every zone into its
//! devices' layout boxes (Spec 86 §5.1). That is the right picture for
//! hardware and an unreadable one for a person, so previews show one zone
//! at a time. A page with a zone picker beside its preview shows the zone
//! it has focused; every other page shows the Default zone. A single-zone
//! scene's canvas already is its one zone, so it keeps the scene canvas.

use hypercolor_types::scene::ZoneRole;
use leptos::prelude::*;

use crate::apply_target::ApplyTarget;
use crate::route_ui::mounted_page_has_own_preview;
use crate::zones::{ZoneEffectState, ZonesContext};

/// The main preview's zone target, provided at the app root.
#[derive(Clone, Copy)]
pub struct PreviewZoneContext {
    /// The zone `WsContext::canvas_frame` carries, or `None` while it
    /// carries the scene canvas.
    pub zone: ReadSignal<Option<String>>,
    pub(crate) set_zone: WriteSignal<Option<String>>,
    /// Lifts the main preview's hold on its first subscription once the
    /// zone it should follow is known.
    pub(crate) set_resolved: WriteSignal<bool>,
}

impl PreviewZoneContext {
    #[must_use]
    pub const fn new(
        zone: ReadSignal<Option<String>>,
        set_zone: WriteSignal<Option<String>>,
        set_resolved: WriteSignal<bool>,
    ) -> Self {
        Self {
            zone,
            set_zone,
            set_resolved,
        }
    }
}

/// The zone a page's preview shows, or `None` for the scene canvas.
///
/// `focused_zone` is the picker's choice, already checked to be an LED
/// zone of the active scene; `primary_zone` is the scene's Default zone.
#[must_use]
pub fn preview_zone_for(
    multi_zone: bool,
    page_has_zone_picker: bool,
    focused_zone: Option<&str>,
    primary_zone: Option<&str>,
) -> Option<String> {
    if !multi_zone {
        return None;
    }
    let zone = if page_has_zone_picker {
        focused_zone.or(primary_zone)
    } else {
        primary_zone
    };
    zone.map(ToOwned::to_owned)
}

/// What a player beside a preview acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerTarget {
    /// The effect previous, next, and shuffle step away from.
    pub current_effect: Option<String>,
    /// Where the stepped-to effect applies.
    pub apply_target: ApplyTarget,
}

/// The target of a player that sits beside a preview: the zone that
/// preview shows.
///
/// `shown` is the state of [`PreviewZoneContext::zone`]'s zone. While a
/// zone is shown, the player applies to it, so a thumbnail of the Default
/// zone never steers a zone focused on another page. The Default zone
/// steps from the primary effect, which an apply updates at once and which
/// is what a Default zone apply replaces; another zone steps from its top
/// effect. On the scene canvas the player keeps the scene-wide behavior:
/// step from the primary effect and apply wherever quick-applies go.
#[must_use]
pub fn player_target(
    shown: Option<&ZoneEffectState>,
    primary_effect: Option<&str>,
    quick_apply_target: &ApplyTarget,
) -> PlayerTarget {
    match shown {
        Some(state) if state.zone.role != ZoneRole::Primary => PlayerTarget {
            current_effect: state.effect_id.clone(),
            apply_target: ApplyTarget::Zone(state.zone.id.clone()),
        },
        Some(_) => PlayerTarget {
            current_effect: primary_effect.map(ToOwned::to_owned),
            apply_target: ApplyTarget::Primary,
        },
        None => PlayerTarget {
            current_effect: primary_effect.map(ToOwned::to_owned),
            apply_target: quick_apply_target.clone(),
        },
    }
}

/// Keep the main preview on the zone the current page should show. Call
/// once from inside the router, where the route is known.
pub fn follow_preview_zone(pathname: Memo<String>) {
    let (Some(preview), Some(zones)) = (
        use_context::<PreviewZoneContext>(),
        use_context::<ZonesContext>(),
    ) else {
        return;
    };
    let target = Memo::new(move |_| {
        let has_picker = mounted_page_has_own_preview(&pathname.get());
        let focused = zones.target_zone().map(|zone| zone.id);
        let primary = zones.primary_zone().map(|zone| zone.id);
        preview_zone_for(
            zones.multi_zone.get(),
            has_picker,
            focused.as_deref(),
            primary.as_deref(),
        )
    });
    Effect::new(move |_| {
        // Until the scene is read, a multi-zone scene looks like a
        // single-zone one, and following that guess would open the preview
        // on its patchwork scene canvas. The main preview holds its first
        // subscription until this lifts it.
        if !zones.scene_resolved.get() {
            return;
        }
        let zone = target.get();
        if preview.zone.get_untracked() != zone {
            preview.set_zone.set(zone);
        }
        preview
            .set_resolved
            .maybe_update(|resolved| !std::mem::replace(resolved, true));
    });
}
