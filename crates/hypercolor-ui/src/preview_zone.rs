//! Which zone the main preview follows.
//!
//! A multi-zone scene's scene canvas projects every zone into its
//! devices' layout boxes (Spec 86 §5.1). That is the right picture for
//! hardware and an unreadable one for a person, so previews show one zone
//! at a time. A page with a zone picker beside its preview shows the zone
//! it has focused; every other page shows the Default zone. A single-zone
//! scene's canvas already is its one zone, so it keeps the scene canvas.

use leptos::prelude::*;

use crate::route_ui::mounted_page_has_own_preview;
use crate::zones::ZonesContext;

/// The main preview's zone target, provided at the app root.
#[derive(Clone, Copy)]
pub struct PreviewZoneContext {
    /// The zone `WsContext::canvas_frame` carries, or `None` while it
    /// carries the scene canvas.
    pub zone: ReadSignal<Option<String>>,
    pub(crate) set_zone: WriteSignal<Option<String>>,
}

impl PreviewZoneContext {
    #[must_use]
    pub const fn new(
        zone: ReadSignal<Option<String>>,
        set_zone: WriteSignal<Option<String>>,
    ) -> Self {
        Self { zone, set_zone }
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
        let zone = target.get();
        if preview.zone.get_untracked() != zone {
            preview.set_zone.set(zone);
        }
    });
}
