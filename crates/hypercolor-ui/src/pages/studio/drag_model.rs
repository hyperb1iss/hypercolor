//! Leptos-free rules behind Studio drag and drop and its "Move to" menus.
//!
//! What a drag carries, what each drop target does with it, when a tab
//! held under a drag springs open, where a group of outputs lands on a
//! drop point, how a move is announced, and what a zone made from a
//! device is called. Tab reordering keeps its own rules in `tab_model`.
//! Kept free of `leptos` so the contract is exercised from
//! `tests/studio_drag_model_tests.rs`.

use hypercolor_types::spatial::NormalizedPosition;

/// Where the moving outputs live before the move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Claimed by this LED zone.
    Zone(String),
    /// Claimed by no zone.
    Unassigned,
}

/// Which surface a drag started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragSource {
    /// A device card, channel row, or compact row in the device pane.
    Pane,
    /// An output box on the active zone's canvas.
    Canvas,
}

/// What a drag, or a "Move to" menu, carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovePayload {
    /// The layout device id the outputs belong to.
    pub device_id: String,
    /// The device's shown name; a zone made from the payload takes it.
    pub device_name: String,
    /// What announcements and the drag ghost call the moving thing.
    pub label: String,
    pub origin: Origin,
    /// Ids of the existing outputs that move. Empty for a device in no
    /// zone, whose outputs the daemon mints on arrival.
    pub outputs: Vec<String>,
    /// For a device in no zone: the segments to claim. Empty claims every
    /// light segment.
    pub segments: Vec<String>,
    /// How many outputs move, for the ghost's count badge.
    pub count: usize,
    pub source: DragSource,
}

impl MovePayload {
    /// A device's outputs as one zone claims them.
    #[must_use]
    pub fn zone_device(
        device_id: &str,
        device_name: &str,
        zone_id: &str,
        outputs: Vec<String>,
    ) -> Self {
        Self {
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            label: device_name.to_owned(),
            origin: Origin::Zone(zone_id.to_owned()),
            count: outputs.len(),
            outputs,
            segments: Vec::new(),
            source: DragSource::Pane,
        }
    }

    /// A whole device no zone claims, with `light_segments` outputs to
    /// mint.
    #[must_use]
    pub fn unassigned_device(device_id: &str, device_name: &str, light_segments: usize) -> Self {
        Self {
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            label: device_name.to_owned(),
            origin: Origin::Unassigned,
            outputs: Vec::new(),
            segments: Vec::new(),
            count: light_segments,
            source: DragSource::Pane,
        }
    }

    /// One channel of a device: its output in `origin`'s zone, or, when no
    /// zone holds it, the segment to claim.
    #[must_use]
    pub fn channel(
        device_id: &str,
        device_name: &str,
        channel_name: &str,
        origin: Origin,
        output: Option<String>,
        segment: Option<String>,
    ) -> Self {
        let unassigned = origin == Origin::Unassigned;
        Self {
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            label: channel_label(channel_name, device_name),
            origin,
            outputs: if unassigned {
                Vec::new()
            } else {
                output.into_iter().collect()
            },
            segments: if unassigned {
                segment.into_iter().collect()
            } else {
                Vec::new()
            },
            count: 1,
            source: DragSource::Pane,
        }
    }

    /// Output boxes dragged off the active zone's canvas.
    #[must_use]
    pub fn canvas(
        device_id: &str,
        device_name: &str,
        label: &str,
        zone_id: &str,
        outputs: Vec<String>,
    ) -> Self {
        Self {
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            label: label.to_owned(),
            origin: Origin::Zone(zone_id.to_owned()),
            count: outputs.len(),
            outputs,
            segments: Vec::new(),
            source: DragSource::Canvas,
        }
    }

    /// The count badge the drag ghost shows: only when several outputs move.
    #[must_use]
    pub const fn badge(&self) -> Option<usize> {
        if self.count > 1 {
            Some(self.count)
        } else {
            None
        }
    }

    /// Whether the payload sits in `zone_id` already.
    #[must_use]
    pub fn is_in(&self, zone_id: &str) -> bool {
        matches!(&self.origin, Origin::Zone(origin) if origin == zone_id)
    }
}

/// How a channel is named on its own: "Channel 4 (Nollie 32)".
#[must_use]
pub fn channel_label(channel_name: &str, device_name: &str) -> String {
    let channel = channel_name.trim();
    let device = device_name.trim();
    if channel.is_empty() {
        device.to_owned()
    } else if device.is_empty() || channel.eq_ignore_ascii_case(device) {
        channel.to_owned()
    } else {
        format!("{channel} ({device})")
    }
}

/// A place a drag can be released.
#[derive(Debug, Clone, PartialEq)]
pub enum DropTarget {
    /// The canvas of the open zone, at a normalized point.
    Canvas {
        zone_id: String,
        point: NormalizedPosition,
    },
    /// An LED zone's tab.
    Tab(String),
    /// The "+" after the tab strip.
    NewZone,
    /// A zone's section in the pane: the open zone's own, or an Other
    /// zones row.
    PaneZone(String),
    /// The pane's Unassigned section.
    Unassigned,
}

/// What releasing a payload on a target does.
#[derive(Debug, Clone, PartialEq)]
pub enum DropAction {
    /// Claim the payload for `zone_id`, centered on `point`. Outputs the
    /// zone already holds move to the point.
    Place {
        zone_id: String,
        point: NormalizedPosition,
    },
    /// Move the payload into the zone, placed by its arrival rule.
    MoveTo(String),
    /// Make a zone named after the device and move the payload into it.
    NewZone,
    /// Release the payload from its zone.
    Unassign,
}

/// The action a release performs, or `None` when the target does not take
/// the payload. Only targets that take a payload highlight under it.
///
/// A canvas box moves within its own canvas by itself, so a canvas drag
/// only drops on the tab of another zone.
#[must_use]
pub fn resolve_drop(payload: &MovePayload, target: &DropTarget) -> Option<DropAction> {
    let from_pane = payload.source == DragSource::Pane;
    match target {
        DropTarget::Canvas { zone_id, point } => from_pane.then(|| DropAction::Place {
            zone_id: zone_id.clone(),
            point: *point,
        }),
        DropTarget::Tab(zone_id) => {
            (!payload.is_in(zone_id)).then(|| DropAction::MoveTo(zone_id.clone()))
        }
        DropTarget::NewZone => from_pane.then_some(DropAction::NewZone),
        DropTarget::PaneZone(zone_id) => {
            (from_pane && !payload.is_in(zone_id)).then(|| DropAction::MoveTo(zone_id.clone()))
        }
        DropTarget::Unassigned => {
            (from_pane && payload.origin != Origin::Unassigned).then_some(DropAction::Unassign)
        }
    }
}

/// Whether holding the payload over a tab opens that tab. A canvas drag
/// never does: its boxes belong to the canvas that would close.
#[must_use]
pub fn springs(payload: &MovePayload) -> bool {
    payload.source == DragSource::Pane
}

/// The short line under the drag ghost naming what a release will do.
#[must_use]
pub fn action_hint(action: &DropAction, zone_name: impl Fn(&str) -> String) -> String {
    match action {
        DropAction::Place { zone_id, .. } => format!("Place on {}", zone_name(zone_id)),
        DropAction::MoveTo(zone_id) => format!("Move to {}", zone_name(zone_id)),
        DropAction::NewZone => "New zone".to_owned(),
        DropAction::Unassign => "Unassign".to_owned(),
    }
}

/// The spring-loaded tab: hold a drag over a tab and it opens.
///
/// Entering a tab starts its clock; leaving it, or moving to another tab,
/// starts over. The open tab never charges, and a tab that has opened
/// does not charge again until the pointer leaves it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpringLoad {
    charging: Option<(String, f64)>,
    fired: Option<String>,
}

/// What a hover changed about the spring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpringPhase {
    /// Nothing is charging.
    Idle,
    /// A hold over this tab just started; it opens after
    /// [`SpringLoad::HOLD_MS`].
    Charging(String),
    /// The same tab is still charging.
    Holding,
}

impl SpringLoad {
    /// How long a drag must rest on a tab before the tab opens.
    pub const HOLD_MS: f64 = 500.0;

    /// The pointer is over `tab` (`None`: over no tab) at `now_ms`, while
    /// `open` is the tab already showing.
    pub fn hover(&mut self, tab: Option<&str>, open: Option<&str>, now_ms: f64) -> SpringPhase {
        let Some(tab) = tab else {
            self.charging = None;
            self.fired = None;
            return SpringPhase::Idle;
        };
        if self.fired.as_deref().is_some_and(|fired| fired != tab) {
            self.fired = None;
        }
        if open == Some(tab) || self.fired.as_deref() == Some(tab) {
            self.charging = None;
            return SpringPhase::Idle;
        }
        if self
            .charging
            .as_ref()
            .is_some_and(|(charging, _)| charging == tab)
        {
            return SpringPhase::Holding;
        }
        self.charging = Some((tab.to_owned(), now_ms));
        SpringPhase::Charging(tab.to_owned())
    }

    /// Open the charging tab once its hold has lasted [`Self::HOLD_MS`].
    pub fn tick(&mut self, now_ms: f64) -> Option<String> {
        let (tab, since) = self.charging.as_ref()?;
        if now_ms - since < Self::HOLD_MS {
            return None;
        }
        let tab = tab.clone();
        self.charging = None;
        self.fired = Some(tab.clone());
        Some(tab)
    }

    /// The tab whose hold is running.
    #[must_use]
    pub fn charging(&self) -> Option<&str> {
        self.charging.as_ref().map(|(tab, _)| tab.as_str())
    }

    /// How far the running hold has come, from 0 to 1.
    #[must_use]
    pub fn progress(&self, now_ms: f64) -> f64 {
        self.charging.as_ref().map_or(0.0, |(_, since)| {
            ((now_ms - since) / Self::HOLD_MS).clamp(0.0, 1.0)
        })
    }
}

/// What a zone made from a device is called: the device's name, numbered
/// ("Razer Huntsman V2 2") when a zone already has it (compared
/// case-insensitively). A device with no name falls back to "New zone".
#[must_use]
pub fn zone_name_for_device(device_name: &str, existing: &[&str]) -> String {
    let base = device_name.trim();
    if base.is_empty() {
        return super::tab_model::next_zone_name(existing);
    }
    let taken = |candidate: &str| {
        existing
            .iter()
            .any(|name| name.trim().eq_ignore_ascii_case(candidate))
    };
    if !taken(base) {
        return base.to_owned();
    }
    (2_usize..)
        .map(|n| format!("{base} {n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_owned())
}

/// The announcement for a move between places: "Moved Razer Huntsman V2
/// to Keyboard from Main".
#[must_use]
pub fn moved_announcement(label: &str, to: &str, from: &str) -> String {
    format!("Moved {label} to {to} from {from}")
}

/// The announcement for a move that makes its own zone: "Moved Razer
/// Huntsman V2 to a new zone from Main". The new zone's name is still the
/// rename draft, so the announcement does not call it by that name.
#[must_use]
pub fn new_zone_announcement(label: &str, from: &str) -> String {
    moved_announcement(label, "a new zone", from)
}

/// The announcement for a drop that only repositions outputs within their
/// zone: "Placed Razer Huntsman V2 on Main".
#[must_use]
pub fn placed_announcement(label: &str, zone: &str) -> String {
    format!("Placed {label} on {zone}")
}

/// The announcement when a held tab opens under a drag.
#[must_use]
pub fn opened_announcement(zone: &str) -> String {
    format!("Opened {zone}")
}

/// The announcement for a drag that ends without a move.
pub const CANCELLED_ANNOUNCEMENT: &str = "Move cancelled";

/// The name an origin goes by in announcements, given the zone names.
#[must_use]
pub fn origin_name(origin: &Origin, zone_name: impl Fn(&str) -> String) -> String {
    match origin {
        Origin::Zone(zone_id) => zone_name(zone_id),
        Origin::Unassigned => "Unassigned".to_owned(),
    }
}
