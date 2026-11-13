//! Leptos-free rules for a screen in Studio.
//!
//! A screen is an output its zone places: a box on the zone's canvas and a
//! card in the zone's section of the device pane. What it shows is a stack
//! of layers, and this module answers every question about that stack that
//! needs no DOM: where a screen's layers live and how to address them
//! ([`placed_screens`], [`StackHome`]), what the composition panel lists
//! for a selection of screens ([`screen_panel`]), what its chip and card
//! say ([`screen_chip`], [`card_face`]), where a new layer goes
//! ([`plan_screen_adds`], [`plan_add_to_all_screens`]), and how one layer
//! is copied onto the same screens of another scene
//! ([`copy_layer_to_scene`]). Tested from
//! `tests/studio_screen_model_tests.rs`.

use std::collections::{HashMap, HashSet};

use hypercolor_types::api::displays::{
    DisplayFaceRendering, DisplayFaceResponse, DisplayFaceScope,
};
use hypercolor_types::api::scene::{ZoneMember, ZoneResource};
use hypercolor_types::layer::{LayerScope, LayerSource, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneMutationMode, ZoneId, ZoneRole};

use crate::components::layer_panel::scope::is_screen_member;
pub use crate::components::layer_panel::scope::{is_screen_layer, reorder_within};
use crate::zones::surface::zone_display_name;

use super::tab_model::{ScreenName, screen_labels};

/// The chip a face saved with its screen, outside any scene, wears.
pub const KEPT_CHIP: &str = "Saved with this screen";

/// The same chip on a row that stands for several screens' saved faces.
pub const KEPT_CHIP_SEVERAL: &str = "Saved with these screens";

/// What editing such a face means, said beside its controls.
pub const KEPT_NOTICE: &str = "Changes apply in every scene using this saved face.";

/// Where a screen's layers live, which is how every write addresses them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackHome {
    /// In the zone that places the screen, each limited to screens.
    Owner,
    /// In a zone of the screen's own, by this id: how a scene written
    /// before a face was a layer stores one it could not convert exactly,
    /// and how every screen is stored on a daemon from before that.
    Stored(String),
}

/// A screen as Studio addresses it: one some zone places, or one no zone
/// places that the scene still keeps a stack for ([`unplaced_screen`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedScreen {
    /// Whether a zone places it. A screen no zone places has no box, no
    /// output, and no canvas behind it: it shows its own layers alone.
    pub placed: bool,
    /// The zone that places it. For a screen no zone places, the zone of
    /// its own that holds its stack.
    pub zone_id: String,
    /// The placing zone's shown name; empty for a screen no zone places.
    pub zone_name: String,
    /// The screen's output in that zone; empty for a screen no zone
    /// places.
    pub output_id: String,
    /// The layout device id the output carries.
    pub layout_id: String,
    /// What its box and its card call it.
    pub name: String,
    /// The physical display, once the rig knows the device.
    pub device_id: Option<String>,
    /// Where its layers live. `None` when nothing can hold one yet: the
    /// daemon is from before screen layers and stores no zone for it.
    pub home: Option<StackHome>,
}

impl PlacedScreen {
    /// The zone a write to this screen's stack addresses.
    #[must_use]
    pub fn home_zone_id(&self) -> Option<&str> {
        match self.home.as_ref()? {
            StackHome::Owner => Some(&self.zone_id),
            StackHome::Stored(zone_id) => Some(zone_id),
        }
    }
}

/// The stored zone that holds the stack of the display `device_id`: a
/// display zone that is no view of another zone's layers.
fn stored_zone<'a>(zones: &'a [ZoneResource], device_id: &str) -> Option<&'a ZoneResource> {
    zones.iter().find(|zone| {
        zone.role == ZoneRole::Display
            && zone.view_of.is_none()
            && zone
                .display_target
                .as_ref()
                .is_some_and(|target| target.device_id.to_string() == device_id)
    })
}

/// Every screen the scene's zones place, in zone and member order.
///
/// `physical_ids` maps the layout device id an output carries to the
/// display behind it. `screen_layers` says whether the daemon keeps a
/// screen's layers in the zone that places it.
#[must_use]
pub fn placed_screens(
    zones: &[ZoneResource],
    physical_ids: &HashMap<String, String>,
    screen_layers: bool,
) -> Vec<PlacedScreen> {
    let members: Vec<(&ZoneResource, &ZoneMember)> = zones
        .iter()
        .filter(|zone| zone.role != ZoneRole::Display)
        .flat_map(|zone| {
            zone.members
                .iter()
                .filter(|member| is_screen_member(member))
                .map(move |member| (zone, member))
        })
        .collect();
    let names: Vec<ScreenName> = members
        .iter()
        .map(|(_, member)| ScreenName {
            name: member.name.clone(),
            device_label: None,
        })
        .collect();
    members
        .into_iter()
        .zip(screen_labels(&names))
        .map(|((zone, member), name)| {
            let device_id = physical_ids.get(&member.device_id).cloned();
            let stored = device_id
                .as_deref()
                .and_then(|device_id| stored_zone(zones, device_id));
            let home = match stored {
                Some(stored) => Some(StackHome::Stored(stored.id.to_string())),
                None => screen_layers.then_some(StackHome::Owner),
            };
            PlacedScreen {
                placed: true,
                zone_id: zone.id.to_string(),
                zone_name: zone_display_name(&zone.name, zone.role),
                output_id: member.id.to_string(),
                layout_id: member.device_id.clone(),
                name,
                device_id,
                home,
            }
        })
        .collect()
}

/// A display no zone places, as its card knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnplacedDisplay {
    /// The physical display.
    pub device_id: String,
    /// The layout device id an output of it would carry.
    pub layout_id: String,
    /// What its card calls it.
    pub name: String,
}

/// `display` as a screen no zone places: its layers live in the zone the
/// scene keeps for it, and every write to them goes through that zone.
/// `None` when a zone places the display, or the scene keeps no zone for
/// it.
#[must_use]
pub fn unplaced_screen(
    zones: &[ZoneResource],
    placed: &[PlacedScreen],
    display: &UnplacedDisplay,
) -> Option<PlacedScreen> {
    let UnplacedDisplay {
        device_id,
        layout_id,
        name,
    } = display;
    if placed
        .iter()
        .any(|screen| screen.device_id.as_deref() == Some(device_id.as_str()))
    {
        return None;
    }
    let stored = stored_zone(zones, device_id)?;
    Some(PlacedScreen {
        placed: false,
        zone_id: stored.id.to_string(),
        zone_name: String::new(),
        output_id: String::new(),
        layout_id: layout_id.clone(),
        name: name.clone(),
        device_id: Some(device_id.clone()),
        home: Some(StackHome::Stored(stored.id.to_string())),
    })
}

/// The names of the displays no zone places, in the order given:
/// `displays` as `(physical id, name)` pairs.
#[must_use]
pub fn unplaced_screens(displays: &[(String, String)], placed: &[PlacedScreen]) -> Vec<String> {
    displays
        .iter()
        .filter(|(id, _)| {
            !placed
                .iter()
                .any(|screen| screen.device_id.as_deref() == Some(id.as_str()))
        })
        .map(|(_, name)| name.clone())
        .collect()
}

/// The screens of the zone `zone_id` a selection names, when screens are
/// all it names: the selection that turns the composition panel to those
/// screens. Empty for no selection, or one with any other output in it.
#[must_use]
pub fn selected_screens<'a>(
    screens: &'a [PlacedScreen],
    zone_id: &str,
    selection: &HashSet<String>,
) -> Vec<&'a PlacedScreen> {
    let named: Vec<&PlacedScreen> = screens
        .iter()
        .filter(|screen| screen.zone_id == zone_id && selection.contains(&screen.output_id))
        .collect();
    if named.is_empty() || named.len() != selection.len() {
        return Vec::new();
    }
    named
}

/// What a set of screens goes by: one or two by name, more by count.
#[must_use]
pub fn screens_label(screens: &[&PlacedScreen]) -> String {
    match screens {
        [] => String::new(),
        [one] => one.name.clone(),
        [first, second] => format!("{} and {}", first.name, second.name),
        more => format!("{} screens", more.len()),
    }
}

/// How a row's blend and amount are held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Composition {
    /// They are the layer's own blend and opacity.
    Layer,
    /// They are how this display's stored zone meets what is behind it,
    /// which that zone holds apart from its layers.
    Display(String),
    /// That stored zone holds them, and nothing writes them while its
    /// stack has no face: the display's own route changes the face saved
    /// with the screen instead, or finds nothing. The row offers no blend.
    Fixed,
}

/// One layer of a screen's stack, as the panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRow {
    /// The zone whose stack holds the layer, which every write addresses.
    pub zone_id: String,
    pub layer_id: SceneLayerId,
    /// The layer's place in that zone's stack, bottom first.
    pub stack_index: usize,
    /// Its blend and amount are how the screen's stack meets what is
    /// behind it: it is the bottom layer that draws, on some screen shown.
    pub meets_backdrop: bool,
    pub composition: Composition,
}

/// One layer of the zone's canvas that reaches a screen from behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BehindRow {
    pub layer_id: SceneLayerId,
    /// The layer covers the whole zone; otherwise it is limited to
    /// outputs, the screen among them.
    pub whole_zone: bool,
}

/// What the composition panel shows for a selection of screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenPanel {
    /// The screens, in words.
    pub title: String,
    /// "On this screen", or "On these screens".
    pub heading: &'static str,
    /// The screens' own layers, top first.
    pub rows: Vec<ScreenRow>,
    /// "Behind it, from Main". Empty for a screen no zone places, which
    /// has nothing behind it.
    pub behind_heading: String,
    /// The zone's canvas layers that reach the screens, top first.
    pub behind: Vec<BehindRow>,
}

/// The bottom layer of a screen's stack that draws: the lowest enabled
/// one, or the lowest of all when none is enabled.
fn bottom_of<'a>(layers: impl Iterator<Item = &'a SceneLayer> + Clone) -> Option<SceneLayerId> {
    layers
        .clone()
        .find(|layer| layer.enabled)
        .or_else(|| layers.clone().next())
        .map(|layer| layer.id)
}

/// The two groups the panel shows for `screens`, all of one zone: their
/// own layers, and the canvas layers behind them. `None` when the scene
/// no longer holds the zone, or no screen was given.
#[must_use]
pub fn screen_panel(zones: &[ZoneResource], screens: &[&PlacedScreen]) -> Option<ScreenPanel> {
    let first = screens.first()?;
    if !first.placed {
        // A screen no zone places lists the stack of its own zone, top
        // first. No canvas is behind it, so no row is how it meets one.
        let stored = zones
            .iter()
            .find(|zone| zone.id.to_string() == first.zone_id)?;
        return Some(ScreenPanel {
            title: first.name.clone(),
            heading: "On this screen",
            rows: stored
                .layers
                .iter()
                .enumerate()
                .rev()
                .map(|(stack_index, layer)| ScreenRow {
                    zone_id: first.zone_id.clone(),
                    layer_id: layer.id,
                    stack_index,
                    meets_backdrop: false,
                    composition: Composition::Layer,
                })
                .collect(),
            behind_heading: String::new(),
            behind: Vec::new(),
        });
    }
    let owner = zones
        .iter()
        .find(|zone| zone.id.to_string() == first.zone_id)?;
    fn names<'a>(screen: &&'a PlacedScreen) -> &'a str {
        screen.output_id.as_str()
    }
    let reaches = |layer: &SceneLayer, outputs: &[&str]| {
        layer.scope.as_ref().is_some_and(|scope| {
            scope
                .outputs
                .iter()
                .any(|id| outputs.contains(&id.as_str()))
        })
    };

    // The screens whose layers the owning zone holds share one list: a
    // layer on three of them is one row.
    let owned: Vec<&str> = screens
        .iter()
        .filter(|screen| screen.home == Some(StackHome::Owner))
        .map(names)
        .collect();
    let mut bottoms: HashSet<SceneLayerId> = HashSet::new();
    for output in &owned {
        let stack = owner
            .layers
            .iter()
            .filter(|layer| is_screen_layer(owner, layer) && reaches(layer, &[*output]));
        bottoms.extend(bottom_of(stack));
    }
    let mut rows: Vec<ScreenRow> = owner
        .layers
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, layer)| is_screen_layer(owner, layer) && reaches(layer, &owned))
        .map(|(stack_index, layer)| ScreenRow {
            zone_id: first.zone_id.clone(),
            layer_id: layer.id,
            stack_index,
            meets_backdrop: bottoms.contains(&layer.id),
            composition: Composition::Layer,
        })
        .collect();

    // A screen with a zone of its own lists that zone's whole stack.
    for screen in screens {
        let Some(StackHome::Stored(zone_id)) = &screen.home else {
            continue;
        };
        let Some(stored) = zones.iter().find(|zone| zone.id.to_string() == *zone_id) else {
            continue;
        };
        let bottom = bottom_of(stored.layers.iter());
        let has_face = stored
            .layers
            .iter()
            .any(|layer| matches!(layer.source, LayerSource::Effect { .. }));
        rows.extend(
            stored
                .layers
                .iter()
                .enumerate()
                .rev()
                .map(|(stack_index, layer)| {
                    let meets_backdrop = bottom == Some(layer.id);
                    ScreenRow {
                        zone_id: zone_id.clone(),
                        layer_id: layer.id,
                        stack_index,
                        meets_backdrop,
                        composition: match (&screen.device_id, meets_backdrop) {
                            (Some(device_id), true) if has_face => {
                                Composition::Display(device_id.clone())
                            }
                            (Some(_), true) => Composition::Fixed,
                            _ => Composition::Layer,
                        },
                    }
                }),
        );
    }

    let outputs: Vec<&str> = screens.iter().map(names).collect();
    let behind = owner
        .layers
        .iter()
        .rev()
        .filter(|layer| !is_screen_layer(owner, layer))
        .filter(|layer| layer.scope.is_none() || reaches(layer, &outputs))
        .map(|layer| BehindRow {
            layer_id: layer.id,
            whole_zone: layer.scope.is_none(),
        })
        .collect();

    let several = screens.len() > 1;
    Some(ScreenPanel {
        title: screens_label(screens),
        heading: if several {
            "On these screens"
        } else {
            "On this screen"
        },
        rows,
        behind_heading: format!(
            "Behind {}, from {}",
            if several { "them" } else { "it" },
            first.zone_name,
        ),
        behind,
    })
}

/// What removing a layer from the screens shown comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaceRemoval {
    /// Every screen the layer is on is among them: the layer goes.
    Delete,
    /// Other screens show it too: it stays on those, limited to them.
    Narrow(LayerScope),
}

/// What removing a layer limited to `scope` from the screens `shown`
/// comes to. A layer on three screens, removed with one of them selected,
/// leaves the other two as they were: the row stands for the screens
/// shown, and what is not shown is not touched. A layer with no target of
/// its own is its zone's and goes whole.
#[must_use]
pub fn face_removal(scope: Option<&LayerScope>, shown: &[&str]) -> FaceRemoval {
    let Some(scope) = scope else {
        return FaceRemoval::Delete;
    };
    let kept: Vec<String> = scope
        .outputs
        .iter()
        .filter(|id| !shown.contains(&id.as_str()))
        .cloned()
        .collect();
    if kept.is_empty() || kept.len() == scope.outputs.len() {
        FaceRemoval::Delete
    } else {
        FaceRemoval::Narrow(LayerScope { outputs: kept })
    }
}

/// A face saved with its screen that is what the screen shows now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptFaceRow {
    /// The face's name.
    pub name: String,
    /// The effect behind it, whose controls the row opens.
    pub effect_id: String,
}

/// What a display reports about its face, as far as the saved face's row
/// needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceReport {
    /// Where the reported assignment lives: in the scene, or saved with
    /// the screen.
    pub live_scope: DisplayFaceScope,
    /// What is on the panel, from a daemon that says.
    pub rendering: Option<DisplayFaceRendering>,
    pub name: String,
    pub effect_id: String,
}

impl From<&DisplayFaceResponse> for FaceReport {
    fn from(face: &DisplayFaceResponse) -> Self {
        Self {
            live_scope: face.live_scope,
            rendering: face.rendering,
            name: face.effect.name.clone(),
            effect_id: face.effect.id.to_string(),
        }
    }
}

/// The saved face's row for a screen, when its saved face is what shows:
/// the scene gives the screen nothing of its own that draws.
#[must_use]
pub fn kept_face_row(face: Option<&FaceReport>) -> Option<KeptFaceRow> {
    let face = face.filter(|face| face.live_scope == DisplayFaceScope::Default)?;
    // A daemon from before it said what is on the panel reports only
    // where the assignment lives, and showed a saved face whenever the
    // scene had none.
    let showing = face
        .rendering
        .is_none_or(|rendering| rendering == DisplayFaceRendering::Default);
    showing.then(|| KeptFaceRow {
        name: face.name.clone(),
        effect_id: face.effect_id.clone(),
    })
}

/// What the Stage header's chip says for a selection of screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenChip {
    /// What the chip names: the face the screens show, or "Faces" when
    /// they show faces no one name stands for. `None` when they show
    /// none, and the chip offers one instead.
    pub face: Option<String>,
    /// More than one screen is selected.
    several: bool,
}

impl ScreenChip {
    /// Whether the screens show no face at all, so a click asks for one.
    /// Screens that already show faces, even ones no single row can
    /// stand for, open their layers instead: a picker there would put a
    /// new face over faces nobody had looked at.
    #[must_use]
    pub fn offers_face(&self) -> bool {
        self.face.is_none()
    }

    /// The words beside the name: what a click opens.
    #[must_use]
    pub fn action(&self) -> &'static str {
        if self.offers_face() {
            "Face"
        } else {
            "Face settings"
        }
    }

    /// The chip's tooltip.
    #[must_use]
    pub fn title(&self) -> &'static str {
        match (self.offers_face(), self.several) {
            (true, false) => "Give this screen a face",
            (true, true) => "Give these screens a face",
            (false, false) => "Change this screen's face and its settings",
            (false, true) => "Change these screens' faces and their settings",
        }
    }
}

/// The chip for `count` screens whose top layer is called `top`, and
/// whose saved faces come to `kept`.
#[must_use]
pub fn screen_chip(top: Option<&str>, kept: &KeptSelection, count: usize) -> ScreenChip {
    let named = |name: &&str| !name.trim().is_empty();
    let face = top
        .filter(named)
        .or_else(|| kept.row().map(|row| row.name.as_str()).filter(named))
        .map(|name| name.trim().to_owned())
        .or_else(|| {
            // Saved faces that differ, or that only some of the screens
            // show, are still faces on those screens.
            matches!(kept, KeptSelection::Differ | KeptSelection::Some { .. })
                .then(|| "Faces".to_owned())
        });
    ScreenChip {
        face,
        several: count > 1,
    }
}

/// The top layer of one screen's own stack: the face its card names.
#[must_use]
pub fn card_face<'a>(zones: &'a [ZoneResource], screen: &PlacedScreen) -> Option<&'a SceneLayer> {
    let home = zones
        .iter()
        .find(|zone| Some(zone.id.to_string().as_str()) == screen.home_zone_id())?;
    match screen.home.as_ref()? {
        StackHome::Stored(_) => home.layers.last(),
        StackHome::Owner => home.layers.iter().rev().find(|layer| {
            is_screen_layer(home, layer)
                && layer
                    .scope
                    .as_ref()
                    .is_some_and(|scope| scope.outputs.contains(&screen.output_id))
        }),
    }
}

/// The top layer of the stack a screen no zone places keeps: the face it
/// goes on showing, held in a zone of its own until a zone claims the
/// screen. `None` when the display has no such zone, or an empty one.
#[must_use]
pub fn unplaced_face<'a>(zones: &'a [ZoneResource], device_id: &str) -> Option<&'a SceneLayer> {
    stored_zone(zones, device_id)?.layers.last()
}

/// What the saved faces of a selection of screens come to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KeptSelection {
    /// No selected screen shows a face saved with it.
    #[default]
    None,
    /// Every selected screen shows this saved face: one row edits them
    /// all.
    Shared(KeptFaceRow),
    /// Every selected screen shows a saved face, and they are not the
    /// same face.
    Differ,
    /// This many of the `of` selected screens show a saved face; the rest
    /// show layers of the scene.
    Some { kept: usize, of: usize },
}

impl KeptSelection {
    /// The one row that stands for every selected screen.
    #[must_use]
    pub fn row(&self) -> Option<&KeptFaceRow> {
        match self {
            Self::Shared(row) => Some(row),
            _ => None,
        }
    }

    /// The line said in place of a row that cannot stand for them all.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        match self {
            Self::None | Self::Shared(_) => None,
            Self::Differ => Some(
                "These screens show different saved faces. Select one screen to change its face."
                    .to_owned(),
            ),
            Self::Some { kept: 1, .. } => Some(
                "One of these screens shows a face saved with it. Select that screen alone to change it."
                    .to_owned(),
            ),
            Self::Some { kept, of } => Some(format!(
                "{kept} of these {of} screens show faces saved with them. Select one alone to change its face."
            )),
        }
    }
}

/// What the saved faces of the selected screens come to, from what each
/// screen's display reports, in selection order. A display that reports
/// nothing shows no saved face.
#[must_use]
pub fn kept_selection(reports: &[Option<FaceReport>]) -> KeptSelection {
    let rows = reports
        .iter()
        .map(|report| kept_face_row(report.as_ref()))
        .collect::<Vec<_>>();
    let kept = rows.iter().flatten().count();
    if kept == 0 {
        return KeptSelection::None;
    }
    if kept < rows.len() {
        return KeptSelection::Some {
            kept,
            of: rows.len(),
        };
    }
    let mut shown = rows.into_iter().flatten();
    let Some(first) = shown.next() else {
        return KeptSelection::None;
    };
    if shown.all(|row| row.effect_id == first.effect_id) {
        KeptSelection::Shared(first)
    } else {
        KeptSelection::Differ
    }
}

/// One screen output a membership write touched: the zone that held it
/// before, and the zone that holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenMove {
    pub output_id: String,
    pub from: Option<ZoneId>,
    pub to: Option<ZoneId>,
}

/// What a membership write leaves for the rig's layout to learn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FollowedLayoutStep {
    /// The zone to save, so the layout gains the screens that arrived.
    pub save: Option<ZoneId>,
    /// Screen outputs that left, for the layout to forget.
    pub forget: Vec<String>,
}

impl FollowedLayoutStep {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.save.is_none() && self.forget.is_empty()
    }
}

/// What the rig's layout has to learn after `moves` landed in a scene
/// that now holds `zones`.
///
/// A scene with one zone of lights takes that zone's outputs from the
/// rig's layout every time it is rebuilt: on a scene switch, an effect
/// change, a restart. Lights reach that layout as they are discovered, but
/// a screen's box exists only once someone places it, so a screen placed
/// in such a zone is lost at the next rebuild unless the layout is told,
/// and one taken out comes back unless the layout forgets it. A scene
/// with zones of its own keeps its placements itself and asks nothing.
#[must_use]
pub fn followed_layout_step(zones: &[ZoneResource], moves: &[ScreenMove]) -> FollowedLayoutStep {
    let mut lights = zones.iter().filter(|zone| zone.role != ZoneRole::Display);
    let (Some(only), None) = (lights.next(), lights.next()) else {
        return FollowedLayoutStep::default();
    };
    if only.role != ZoneRole::Primary {
        return FollowedLayoutStep::default();
    }
    let arrived = moves
        .iter()
        .any(|moved| moved.to == Some(only.id) && moved.from != Some(only.id));
    let forget = moves
        .iter()
        .filter(|moved| moved.from == Some(only.id) && moved.to != Some(only.id))
        .map(|moved| moved.output_id.clone())
        .collect();
    FollowedLayoutStep {
        save: arrived.then_some(only.id),
        forget,
    }
}

/// The displays whose stack the scene leaves empty, each once: the ones
/// a face saved with the screen may be showing on. A display zone lists a
/// screen's layers whether it holds them or is a view of the zone that
/// does.
#[must_use]
pub fn faceless_displays(zones: &[ZoneResource]) -> Vec<String> {
    let mut displays: Vec<String> = Vec::new();
    for zone in zones
        .iter()
        .filter(|zone| zone.role == ZoneRole::Display && zone.layers.is_empty())
    {
        if let Some(target) = &zone.display_target {
            let device_id = target.device_id.to_string();
            if !displays.contains(&device_id) {
                displays.push(device_id);
            }
        }
    }
    displays
}

/// A screen as a zone's card lists it on a phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenLine {
    pub screen: PlacedScreen,
    /// The face it shows: the top layer of its own, else the face saved
    /// with it. `None` when it shows neither.
    pub face: Option<String>,
}

/// The screens the zone `zone_id` places, each with the face it shows.
/// `layer_name` words a layer, and `kept` names the saved face showing on
/// a display the scene gives nothing.
#[must_use]
pub fn zone_screen_lines(
    zones: &[ZoneResource],
    screens: &[PlacedScreen],
    zone_id: &str,
    layer_name: impl Fn(&SceneLayer) -> String,
    kept: &HashMap<String, String>,
) -> Vec<ScreenLine> {
    screens
        .iter()
        .filter(|screen| screen.zone_id == zone_id)
        .map(|screen| ScreenLine {
            face: card_face(zones, screen).map(&layer_name).or_else(|| {
                screen
                    .device_id
                    .as_ref()
                    .and_then(|device_id| kept.get(device_id).cloned())
            }),
            screen: screen.clone(),
        })
        .collect()
}

/// The screens no zone places, each with the face it goes on showing:
/// `displays` in the order given, leaving out any a zone places or the
/// scene keeps nothing for.
#[must_use]
pub fn unplaced_screen_lines(
    zones: &[ZoneResource],
    placed: &[PlacedScreen],
    displays: &[UnplacedDisplay],
    layer_name: impl Fn(&SceneLayer) -> String,
    kept: &HashMap<String, String>,
) -> Vec<ScreenLine> {
    displays
        .iter()
        .filter_map(|display| unplaced_screen(zones, placed, display))
        .map(|screen| ScreenLine {
            face: card_face(zones, &screen).map(&layer_name).or_else(|| {
                screen
                    .device_id
                    .as_ref()
                    .and_then(|device_id| kept.get(device_id).cloned())
            }),
            screen,
        })
        .collect()
}

/// One layer to create so a face lands on some screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenAdd {
    /// The zone whose stack takes the layer.
    pub zone_id: String,
    /// The screens it is limited to there; none for a zone that is one
    /// screen's own.
    pub scope: Option<LayerScope>,
    /// The name of the zone that places the screens.
    pub zone_name: String,
    /// The screens it lands on, by name.
    pub screens: Vec<String>,
    /// The fewest layers any of those screens already has: zero means the
    /// new layer is the bottom of a stack, and meets what is behind it.
    pub beneath: usize,
}

/// The layers to create so one face lands on every screen of `screens`:
/// one per zone that holds the layers of any of them, in the order the
/// screens come in, each limited to that zone's screens among them.
/// Screens with nowhere to hold a layer are left out.
#[must_use]
pub fn plan_screen_adds(zones: &[ZoneResource], screens: &[&PlacedScreen]) -> Vec<ScreenAdd> {
    let mut adds: Vec<ScreenAdd> = Vec::new();
    for screen in screens {
        let Some(home) = &screen.home else {
            continue;
        };
        let zone_id = match home {
            StackHome::Owner => &screen.zone_id,
            StackHome::Stored(zone_id) => zone_id,
        };
        let beneath = zones
            .iter()
            .find(|zone| zone.id.to_string() == *zone_id)
            .map_or(0, |zone| match home {
                StackHome::Stored(_) => zone.layers.len(),
                StackHome::Owner => zone
                    .layers
                    .iter()
                    .filter(|layer| {
                        is_screen_layer(zone, layer)
                            && layer
                                .scope
                                .as_ref()
                                .is_some_and(|scope| scope.outputs.contains(&screen.output_id))
                    })
                    .count(),
            });
        match adds.iter_mut().find(|add| add.zone_id == *zone_id) {
            Some(add) => {
                if let Some(scope) = &mut add.scope {
                    scope.outputs.push(screen.output_id.clone());
                }
                add.screens.push(screen.name.clone());
                add.beneath = add.beneath.min(beneath);
            }
            None => adds.push(ScreenAdd {
                zone_id: zone_id.clone(),
                scope: matches!(home, StackHome::Owner).then(|| LayerScope {
                    outputs: vec![screen.output_id.clone()],
                }),
                zone_name: screen.zone_name.clone(),
                screens: vec![screen.name.clone()],
                beneath,
            }),
        }
    }
    for add in &mut adds {
        if let Some(scope) = &mut add.scope {
            *scope = scope.normalized();
        }
    }
    adds
}

/// "Add to all screens", planned: the layers to create, and the screens
/// it has to leave out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllScreensPlan {
    pub adds: Vec<ScreenAdd>,
    /// Screens no zone places, by name. A layer needs a zone to live in.
    pub skipped: Vec<String>,
}

impl AllScreensPlan {
    /// How many screens the plan reaches.
    #[must_use]
    pub fn screen_count(&self) -> usize {
        self.adds.iter().map(|add| add.screens.len()).sum()
    }

    /// The zones the plan reaches, by name, each once.
    #[must_use]
    pub fn zone_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for add in &self.adds {
            if !names.contains(&add.zone_name.as_str()) {
                names.push(&add.zone_name);
            }
        }
        names
    }

    /// What the command will do, said before it is run: "5 screens in
    /// Main and Desk. TL LCD 3 is in no zone and is skipped."
    #[must_use]
    pub fn summary(&self) -> String {
        let mut words = format!(
            "{} in {}.",
            count_words(self.screen_count(), "screen"),
            join_names(&self.zone_names()),
        );
        if !self.skipped.is_empty() {
            let skipped: Vec<&str> = self.skipped.iter().map(String::as_str).collect();
            words.push_str(&format!(
                " {} {} in no zone and {} skipped.",
                join_names(&skipped),
                if skipped.len() == 1 { "is" } else { "are" },
                if skipped.len() == 1 { "is" } else { "are" },
            ));
        }
        words
    }
}

/// Plan "Add to all screens" for a scene that places `screens` and leaves
/// `unplaced` in no zone.
#[must_use]
pub fn plan_add_to_all_screens(
    zones: &[ZoneResource],
    screens: &[PlacedScreen],
    unplaced: &[String],
) -> AllScreensPlan {
    let every: Vec<&PlacedScreen> = screens.iter().collect();
    let mut skipped: Vec<String> = screens
        .iter()
        .filter(|screen| screen.home.is_none())
        .map(|screen| screen.name.clone())
        .collect();
    skipped.extend(unplaced.iter().cloned());
    AllScreensPlan {
        adds: plan_screen_adds(zones, &every),
        skipped,
    }
}

/// What an add across several zones reports once it has run: `landed` and
/// `failed` are the screens it reached and the ones it did not.
#[must_use]
pub fn add_outcome_words(name: &str, landed: &[String], failed: &[String]) -> String {
    let on = |names: &[String]| match names {
        [_, _, _, ..] => count_words(names.len(), "screen"),
        _ => join_names(&names.iter().map(String::as_str).collect::<Vec<_>>()),
    };
    match (landed.is_empty(), failed.is_empty()) {
        (false, true) => format!("{name} added to {}", on(landed)),
        (false, false) => format!(
            "{name} added to {}. It could not be added to {}.",
            on(landed),
            on(failed),
        ),
        (true, _) => format!("{name} could not be added"),
    }
}

/// "1 screen", "3 screens".
fn count_words(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// "Main", "Main and Desk", "Main, Desk and Shelf".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A screen as another scene would know it: by its device, since output
/// ids belong to the scene that minted them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRef {
    /// The physical display.
    pub device_id: String,
    /// The layout device id its output carries in any zone.
    pub layout_id: String,
}

/// Whether "Copy to every scene" writes to a saved scene: every scene but
/// the one being copied from, and never one locked as a snapshot.
#[must_use]
pub fn takes_copies(scene_id: &str, mode: SceneMutationMode, active_scene_id: &str) -> bool {
    scene_id != active_scene_id && mode != SceneMutationMode::Snapshot
}

/// Put a copy of `layer` on `screens` in another scene's `zones`,
/// replacing what each of those screens had there. Returns how many of the
/// screens the scene could take it on; zero leaves `zones` untouched.
///
/// A screen the scene stores a zone for takes the copy as that zone's one
/// layer, with the copy's blend and amount as how the zone meets what is
/// behind it. A screen a zone places takes it as a layer limited to the
/// screen: layers that were that screen's alone go, and layers it shared
/// stop naming it. A screen the scene places nowhere has nothing to hold
/// a layer and is passed over.
pub fn copy_layer_to_scene(
    zones: &mut [ZoneResource],
    layer: &SceneLayer,
    screens: &[ScreenRef],
) -> usize {
    let mut landed = 0;
    // The copy a zone has taken for an earlier screen, to widen.
    let mut copies: HashMap<usize, SceneLayerId> = HashMap::new();
    for screen in screens {
        let stored = zones.iter().position(|zone| {
            zone.role == ZoneRole::Display
                && zone
                    .display_target
                    .as_ref()
                    .is_some_and(|target| target.device_id.to_string() == screen.device_id)
        });
        if let Some(index) = stored {
            let zone = &mut zones[index];
            if let Some(target) = &mut zone.display_target {
                target.blend_mode = layer.blend;
                target.opacity = layer.opacity;
            }
            zone.layers = vec![SceneLayer {
                id: SceneLayerId::new(),
                opacity: 1.0,
                enabled: true,
                scope: None,
                ..layer.clone()
            }];
            landed += 1;
            continue;
        }
        let placed = zones.iter().enumerate().find_map(|(index, zone)| {
            (zone.role != ZoneRole::Display)
                .then(|| {
                    zone.members
                        .iter()
                        .find(|member| {
                            is_screen_member(member) && member.device_id == screen.layout_id
                        })
                        .map(|member| (index, member.id.to_string()))
                })
                .flatten()
        });
        let Some((index, output_id)) = placed else {
            continue;
        };
        let zone = &mut zones[index];
        let copy = copies.get(&index).copied();
        let snapshot = zone.clone();
        zone.layers.retain_mut(|existing| {
            if Some(existing.id) == copy || !is_screen_layer(&snapshot, existing) {
                return true;
            }
            let Some(scope) = &mut existing.scope else {
                return true;
            };
            scope.outputs.retain(|id| *id != output_id);
            !scope.outputs.is_empty()
        });
        match copy.and_then(|id| zone.layers.iter_mut().find(|existing| existing.id == id)) {
            Some(existing) => {
                if let Some(scope) = &mut existing.scope {
                    scope.outputs.push(output_id);
                    *scope = scope.normalized();
                }
            }
            None => {
                let id = SceneLayerId::new();
                copies.insert(index, id);
                zone.layers.push(SceneLayer {
                    id,
                    enabled: true,
                    scope: Some(LayerScope {
                        outputs: vec![output_id],
                    }),
                    ..layer.clone()
                });
            }
        }
        landed += 1;
    }
    landed
}

/// The question "Copy to every scene" asks before it writes: "Use Neon
/// Clock on TL LCD 1 in 6 scenes?"
#[must_use]
pub fn copy_prompt(layer_name: &str, screens: &str, scenes: usize) -> String {
    format!(
        "Use {layer_name} on {screens} in {}?",
        count_words(scenes, "scene"),
    )
}
