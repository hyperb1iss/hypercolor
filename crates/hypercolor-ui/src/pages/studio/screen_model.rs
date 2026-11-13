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
use hypercolor_types::layer::{LayerScope, SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneMutationMode, ZoneRole};

use crate::components::layer_panel::scope::is_screen_member;
pub use crate::components::layer_panel::scope::{is_screen_layer, reorder_within};
use crate::zones::surface::zone_display_name;

use super::tab_model::{ScreenName, screen_labels};

/// The chip a face saved with its screen, outside any scene, wears.
pub const KEPT_CHIP: &str = "Saved with this screen";

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

/// A screen some zone places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedScreen {
    /// The zone that places it.
    pub zone_id: String,
    /// That zone's shown name.
    pub zone_name: String,
    /// The screen's output in that zone.
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
    /// "Behind it, from Main".
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
                            (Some(device_id), true) => Composition::Display(device_id.clone()),
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
    /// The face's name; `None` when the screens show no face, and the
    /// chip offers one instead.
    pub face: Option<String>,
}

impl ScreenChip {
    /// The words beside the name: what a click opens.
    #[must_use]
    pub fn action(&self) -> &'static str {
        if self.face.is_some() {
            "Face settings"
        } else {
            "Face"
        }
    }
}

/// The chip for screens whose top layer is called `top`, or whose saved
/// face is `kept`.
#[must_use]
pub fn screen_chip(top: Option<&str>, kept: Option<&str>) -> ScreenChip {
    let named = |name: &&str| !name.trim().is_empty();
    ScreenChip {
        face: top
            .filter(named)
            .or_else(|| kept.filter(named))
            .map(|name| name.trim().to_owned()),
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
