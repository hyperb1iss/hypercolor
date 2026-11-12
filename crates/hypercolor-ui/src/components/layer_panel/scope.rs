//! Leptos-free rules for which of a zone's outputs a layer applies to.
//!
//! A layer applies to the whole zone, or to the outputs it names. This
//! module holds everything about that which needs no DOM: the outputs a
//! target can name, the draft the target editor edits, the words a target
//! goes by, which rows a selection puts in front, and when a layer has no
//! output left in its zone. Tested from `tests/layer_scope_tests.rs`.

use std::collections::{BTreeSet, HashSet};

use hypercolor_types::api::scene::{ZoneMember, ZoneResource};
use hypercolor_types::layer::{LayerScope, SceneLayer};

/// The segment name a screen's output carries in a zone's membership.
const SCREEN_SEGMENT: &str = "Display";

/// The words for a layer that applies to every output of its zone.
pub const WHOLE_ZONE: &str = "Whole zone";

/// The words for a layer whose outputs have all left its zone.
pub const NOT_IN_ZONE: &str = "Not in this zone";

/// One output of a zone, as a target names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOutput {
    /// The membership id a scope holds.
    pub id: String,
    /// What the output is called beside its device: the device's name for
    /// a device with one output, the channel's for one of several.
    pub name: String,
    /// The layout device id the output belongs to.
    pub device_id: String,
    pub device_name: String,
    /// A screen. Screens take faces through their own surface, so a target
    /// cannot pick one, though a stored scope that names one still reads.
    pub screen: bool,
}

/// Whether a zone member is a screen.
#[must_use]
pub fn is_screen_member(member: &ZoneMember) -> bool {
    member.segment.as_deref() == Some(SCREEN_SEGMENT)
}

/// Every output of `zone`, in the zone's order. `names` gives a member's
/// own shown name and its device's.
#[must_use]
pub fn zone_target_outputs(
    zone: &ZoneResource,
    names: impl Fn(&ZoneMember) -> (String, String),
) -> Vec<TargetOutput> {
    zone.members
        .iter()
        .map(|member| {
            let (name, device_name) = names(member);
            TargetOutput {
                id: member.id.to_string(),
                name,
                device_id: member.device_id.clone(),
                device_name,
                screen: is_screen_member(member),
            }
        })
        .collect()
}

/// The outputs of one device, as the target editor lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetGroup {
    pub device_id: String,
    pub device_name: String,
    pub outputs: Vec<TargetOutput>,
}

impl TargetGroup {
    /// The ids of the group's outputs.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.outputs
            .iter()
            .map(|output| output.id.clone())
            .collect()
    }
}

/// The outputs a target can pick, grouped by device in the zone's order.
/// Screens are left out.
#[must_use]
pub fn target_groups(outputs: &[TargetOutput]) -> Vec<TargetGroup> {
    let mut groups: Vec<TargetGroup> = Vec::new();
    for output in outputs.iter().filter(|output| !output.screen) {
        match groups
            .iter_mut()
            .find(|group| group.device_id == output.device_id)
        {
            Some(group) => group.outputs.push(output.clone()),
            None => groups.push(TargetGroup {
                device_id: output.device_id.clone(),
                device_name: output.device_name.clone(),
                outputs: vec![output.clone()],
            }),
        }
    }
    groups
}

/// How many outputs a target can pick from. A zone with fewer than two
/// has nothing to choose between, so it shows no target at all.
#[must_use]
pub fn pickable_count(outputs: &[TargetOutput]) -> usize {
    outputs.iter().filter(|output| !output.screen).count()
}

/// How much of a device a target holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checked {
    None,
    Some,
    All,
}

/// What a layer applies to, while it is being chosen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    WholeZone,
    /// These outputs, by id. Empty while every box is unticked, which no
    /// layer can be saved as.
    Outputs(BTreeSet<String>),
}

impl Target {
    /// The target a stored scope stands for.
    #[must_use]
    pub fn of(scope: Option<&LayerScope>) -> Self {
        scope.map_or(Self::WholeZone, |scope| {
            Self::Outputs(scope.outputs.iter().cloned().collect())
        })
    }

    /// The target a canvas selection stands for: its pickable outputs, or
    /// the whole zone when it holds none.
    #[must_use]
    pub fn from_selection(selection: &HashSet<String>, outputs: &[TargetOutput]) -> Self {
        let picked: BTreeSet<String> = outputs
            .iter()
            .filter(|output| !output.screen && selection.contains(&output.id))
            .map(|output| output.id.clone())
            .collect();
        if picked.is_empty() {
            Self::WholeZone
        } else {
            Self::Outputs(picked)
        }
    }

    /// The scope a layer with this target stores: none for the whole zone.
    /// An empty pick is no scope a layer can store, so check
    /// [`Self::is_complete`] first.
    #[must_use]
    pub fn scope(&self) -> Option<LayerScope> {
        match self {
            Self::WholeZone => None,
            Self::Outputs(ids) => Some(LayerScope {
                outputs: ids.iter().cloned().collect(),
            }),
        }
    }

    /// Whether the target names something: the whole zone, or at least one
    /// output.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !matches!(self, Self::Outputs(ids) if ids.is_empty())
    }

    #[must_use]
    pub fn is_whole_zone(&self) -> bool {
        matches!(self, Self::WholeZone)
    }

    #[must_use]
    pub fn holds(&self, id: &str) -> bool {
        matches!(self, Self::Outputs(ids) if ids.contains(id))
    }

    /// The output ids the target names; none for the whole zone.
    #[must_use]
    pub fn ids(&self) -> HashSet<String> {
        match self {
            Self::WholeZone => HashSet::new(),
            Self::Outputs(ids) => ids.iter().cloned().collect(),
        }
    }

    /// Tick or untick every id of `ids` together: a set the target already
    /// holds in full leaves it, any other joins it. Ticking from the whole
    /// zone starts a pick with just these.
    pub fn toggle(&mut self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let mut picked = match std::mem::take(self) {
            Self::WholeZone => BTreeSet::new(),
            Self::Outputs(picked) => picked,
        };
        if ids.iter().all(|id| picked.contains(id)) {
            for id in ids {
                picked.remove(id);
            }
        } else {
            picked.extend(ids.iter().cloned());
        }
        *self = Self::Outputs(picked);
    }

    /// How much of `group` the target holds.
    #[must_use]
    pub fn checked(&self, group: &TargetGroup) -> Checked {
        let held = group
            .outputs
            .iter()
            .filter(|output| self.holds(&output.id))
            .count();
        if held == 0 {
            Checked::None
        } else if held == group.outputs.len() {
            Checked::All
        } else {
            Checked::Some
        }
    }

    /// The ids the target names that are no output of the zone any more.
    #[must_use]
    pub fn missing(&self, outputs: &[TargetOutput]) -> Vec<String> {
        match self {
            Self::WholeZone => Vec::new(),
            Self::Outputs(ids) => ids
                .iter()
                .filter(|id| !outputs.iter().any(|output| output.id == **id))
                .cloned()
                .collect(),
        }
    }

    /// Drop every id that is no output of the zone any more.
    pub fn drop_missing(&mut self, outputs: &[TargetOutput]) {
        if let Self::Outputs(ids) = self {
            ids.retain(|id| outputs.iter().any(|output| output.id == *id));
        }
    }
}

/// What a layer's target reads as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetWords {
    /// No scope: the layer applies to every output.
    WholeZone,
    /// A scope with outputs in the zone, in words, and how many of its
    /// ids are not in the zone any more.
    Outputs { label: String, missing: usize },
    /// A scope none of whose outputs is in the zone.
    Dormant,
}

impl TargetWords {
    /// The words themselves.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::WholeZone => WHOLE_ZONE,
            Self::Outputs { label, .. } => label,
            Self::Dormant => NOT_IN_ZONE,
        }
    }
}

/// What `scope` reads as in a zone holding `outputs`.
#[must_use]
pub fn target_words(scope: Option<&LayerScope>, outputs: &[TargetOutput]) -> TargetWords {
    let Some(scope) = scope else {
        return TargetWords::WholeZone;
    };
    let ids: HashSet<&str> = scope.outputs.iter().map(String::as_str).collect();
    match outputs_label(&ids, outputs) {
        Some(label) => {
            let live = outputs
                .iter()
                .filter(|output| ids.contains(output.id.as_str()))
                .count();
            TargetWords::Outputs {
                label,
                missing: ids.len().saturating_sub(live),
            }
        }
        None => TargetWords::Dormant,
    }
}

/// What a draft target reads as, for the line under "Add layer" and the
/// banner while picking on the canvas. An empty pick reads as nothing
/// chosen yet.
#[must_use]
pub fn draft_words(target: &Target, outputs: &[TargetOutput]) -> String {
    match target {
        Target::WholeZone => WHOLE_ZONE.to_owned(),
        Target::Outputs(ids) => {
            let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
            outputs_label(&ids, outputs).unwrap_or_else(|| "No outputs yet".to_owned())
        }
    }
}

/// The names behind a set of output ids: one device or channel by its
/// name, two joined by "and", more by their count. A device whose outputs
/// are all among them goes by the device's name. `None` when none of the
/// ids is an output of the zone.
#[must_use]
pub fn outputs_label(ids: &HashSet<&str>, outputs: &[TargetOutput]) -> Option<String> {
    let live: Vec<&TargetOutput> = outputs
        .iter()
        .filter(|output| ids.contains(output.id.as_str()))
        .collect();
    if live.is_empty() {
        return None;
    }
    let mut names: Vec<&str> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for output in &live {
        if seen.contains(&output.device_id.as_str()) {
            continue;
        }
        seen.push(&output.device_id);
        let of_device = |candidate: &&TargetOutput| candidate.device_id == output.device_id;
        let held = live.iter().copied().filter(of_device).count();
        let whole = outputs.iter().filter(of_device).count();
        if held == whole {
            names.push(&output.device_name);
        } else {
            names.extend(
                live.iter()
                    .copied()
                    .filter(of_device)
                    .map(|output| output.name.as_str()),
            );
        }
    }
    Some(match names.as_slice() {
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        _ => format!("{} outputs", live.len()),
    })
}

/// Every output a set of ids names, in full, for a tooltip: "Fan 1 (Uni
/// Fan SL), Corsair K70".
#[must_use]
pub fn outputs_detail(ids: &HashSet<&str>, outputs: &[TargetOutput]) -> String {
    outputs
        .iter()
        .filter(|output| ids.contains(output.id.as_str()))
        .map(|output| {
            if output.name == output.device_name {
                output.name.clone()
            } else {
                format!("{} ({})", output.name, output.device_name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `layer` names outputs and none of them is a member of its
/// zone: it draws nothing until one comes back.
#[must_use]
pub fn is_dormant(layer: &SceneLayer, members: &HashSet<&str>) -> bool {
    layer
        .scope
        .as_ref()
        .is_some_and(|scope| !scope.outputs.iter().any(|id| members.contains(id.as_str())))
}

/// Whether `layer` reaches any of the `selected` outputs. A layer on the
/// whole zone reaches every output.
#[must_use]
pub fn applies_to(layer: &SceneLayer, selected: &HashSet<String>) -> bool {
    layer
        .scope
        .as_ref()
        .is_none_or(|scope| scope.outputs.iter().any(|id| selected.contains(id)))
}

/// Whether a row stands back while `selected` is being shown: there is a
/// selection and the layer does not reach it.
#[must_use]
pub fn stands_back(layer: &SceneLayer, selected: &HashSet<String>) -> bool {
    !selected.is_empty() && !applies_to(layer, selected)
}

/// The boxes a row lights on the canvas while it is hovered: the outputs
/// its layer names that the zone holds. A layer on the whole zone singles
/// none out.
#[must_use]
pub fn hover_outputs(layer: &SceneLayer, members: &HashSet<&str>) -> HashSet<String> {
    layer.scope.as_ref().map_or_else(HashSet::new, |scope| {
        scope
            .outputs
            .iter()
            .filter(|id| members.contains(id.as_str()))
            .cloned()
            .collect()
    })
}

/// Whether the box under the pointer is one `layer` names, so its row
/// answers the hover.
#[must_use]
pub fn names_output(layer: &SceneLayer, output_id: Option<&str>) -> bool {
    match (&layer.scope, output_id) {
        (Some(scope), Some(output_id)) => scope.outputs.iter().any(|id| id == output_id),
        _ => false,
    }
}

/// "1 layer", "3 layers".
#[must_use]
pub fn layer_count_words(count: usize) -> String {
    if count == 1 {
        "1 layer".to_owned()
    } else {
        format!("{count} layers")
    }
}
