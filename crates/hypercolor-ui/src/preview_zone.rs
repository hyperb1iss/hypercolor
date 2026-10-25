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

use crate::route_ui::mounted_page_has_own_preview;
use crate::zones::{ZoneEffectState, ZonesContext};

/// The main preview's zone target, provided at the app root.
#[derive(Clone, Copy)]
pub struct PreviewZoneContext {
    /// The zone `WsContext::canvas_frame` carries, or `None` while it
    /// carries the scene canvas.
    pub zone: Memo<Option<String>>,
    pub(crate) set_zone: WriteSignal<Option<String>>,
    /// Lifts the main preview's hold on its first subscription once the
    /// zone it should follow is known.
    pub(crate) set_resolved: WriteSignal<bool>,
}

impl PreviewZoneContext {
    #[must_use]
    pub const fn new(
        zone: Memo<Option<String>>,
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

/// The effect the main preview's frames belong to, which may file them
/// as its card art.
///
/// The scene canvas belongs to the primary effect only while the loaded
/// scene has one zone. A multi-zone scene canvas, which a daemon that
/// cannot select zones streams, is a patchwork no one effect owns, and
/// the canvas of a scene that has not loaded may be one. A shown zone's
/// frames belong to its effect when that effect is the zone's only layer.
#[must_use]
pub fn preview_frame_owner(
    shown_zone: Option<&str>,
    single_zone_scene: bool,
    primary_effect: Option<String>,
    zone_effects: &[ZoneEffectState],
) -> Option<String> {
    match shown_zone {
        None => primary_effect.filter(|_| single_zone_scene),
        Some(zone_id) => zone_effects
            .iter()
            .find(|state| state.zone.id == zone_id)
            .and_then(|state| state.sole_effect_id().map(ToOwned::to_owned)),
    }
}

/// What a player beside a preview acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerTarget {
    /// The effect previous, next, and shuffle step away from.
    pub current_effect: Option<String>,
    /// The zone the stepped-to effect applies to; `None` is the primary
    /// zone.
    pub apply_zone: Option<String>,
}

/// The state of the zone a preview shows, for the controls beside it.
///
/// A failed scene refetch empties the zone list while the preview keeps
/// its zone (see [`settled_preview_zone`]), so the state last seen for
/// that zone stands in until the next document lands. A zone missing from
/// a loaded scene has no state.
#[must_use]
pub fn shown_zone_state(
    shown_zone: Option<&str>,
    zone_effects: &[ZoneEffectState],
    last_seen: Option<&ZoneEffectState>,
) -> Option<ZoneEffectState> {
    let zone_id = shown_zone?;
    zone_effects
        .iter()
        .find(|state| state.zone.id == zone_id)
        .or_else(|| last_seen.filter(|state| zone_effects.is_empty() && state.zone.id == zone_id))
        .cloned()
}

/// The target of a player that sits beside a preview: the zone that
/// preview shows.
///
/// `shown` is the state of [`PreviewZoneContext::zone`]'s zone. While a
/// zone is shown, the player applies to it, so a thumbnail of the Default
/// zone never steers a zone focused on another page. The Default zone
/// steps from the primary effect, which an apply updates at once and which
/// is what a Default zone apply replaces; another zone steps from its top
/// effect. On the scene canvas the player steps from the primary effect
/// and applies to `target_zone`, the apply zone of the app's target zone.
#[must_use]
pub fn player_target(
    shown: Option<&ZoneEffectState>,
    primary_effect: Option<&str>,
    target_zone: Option<&str>,
) -> PlayerTarget {
    match shown {
        Some(state) if state.zone.role != ZoneRole::Primary => PlayerTarget {
            current_effect: state.effect_id.clone(),
            apply_zone: Some(state.zone.id.clone()),
        },
        Some(_) => PlayerTarget {
            current_effect: primary_effect.map(ToOwned::to_owned),
            apply_zone: None,
        },
        None => PlayerTarget {
            current_effect: primary_effect.map(ToOwned::to_owned),
            apply_zone: target_zone.map(ToOwned::to_owned),
        },
    }
}

/// Where the main preview points once a scene read settles.
///
/// A read that returned the scene document moves it to `policy`, the
/// [`preview_zone_for`] answer. A failed read describes no zones, so it
/// keeps `current`: the scene canvas before any document has loaded, and
/// the zone already on screen through a failed refetch or a reconnect,
/// rather than dropping to the scene canvas's patchwork.
#[must_use]
pub fn settled_preview_zone(
    document_loaded: bool,
    policy: Option<String>,
    current: Option<&str>,
) -> Option<String> {
    if document_loaded {
        policy
    } else {
        current.map(ToOwned::to_owned)
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
        let policy = target.get();
        let document_loaded = zones.active_scene.with(Option::is_some);
        // Compared with what was asked for rather than with the shown
        // zone: a daemon that cannot select zones shows the scene canvas
        // whatever is asked for.
        preview.set_zone.maybe_update(|wanted| {
            let next = settled_preview_zone(document_loaded, policy, wanted.as_deref());
            let changed = *wanted != next;
            if changed {
                *wanted = next;
            }
            changed
        });
        preview
            .set_resolved
            .maybe_update(|resolved| !std::mem::replace(resolved, true));
    });
}
