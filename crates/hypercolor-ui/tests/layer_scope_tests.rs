//! The rules behind a layer's target: which outputs it can name, what it
//! reads as, which rows a selection puts in front, and when a layer has no
//! output left.

use std::collections::{BTreeSet, HashSet};

use hypercolor_types::api::scene::{ZoneMember, ZoneMemberId, ZoneResource};
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerScope, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_ui::components::layer_panel::scope::{
    Checked, NOT_IN_ZONE, Target, TargetOutput, TargetWords, WHOLE_ZONE, applies_to, draft_words,
    hover_outputs, is_dormant, layer_count_words, names_output, outputs_detail, outputs_label,
    pickable_count, stands_back, target_groups, target_words, zone_target_outputs,
};

fn member(id: &str, device: &str, segment: Option<&str>, name: &str) -> ZoneMember {
    ZoneMember {
        id: ZoneMemberId(id.to_owned()),
        device_id: device.to_owned(),
        segment: segment.map(str::to_owned),
        name: name.to_owned(),
    }
}

/// A zone with a keyboard, a mouse, a two-channel strimer, and a screen.
fn zone() -> ZoneResource {
    ZoneResource {
        id: ZoneId::new(),
        name: "Main".to_owned(),
        description: None,
        role: ZoneRole::Primary,
        enabled: true,
        brightness: 1.0,
        color: None,
        display_target: None,
        members: vec![
            member("k70", "usb:k70", None, "K70"),
            member("basilisk", "usb:basilisk", None, "Basilisk"),
            member("atx", "usb:strimer", Some("ATX"), "Strimer: ATX"),
            member("gpu", "usb:strimer", Some("GPU"), "Strimer: GPU"),
            member("lcd", "usb:lcd", Some("Display"), "TL LCD 1"),
        ],
        layout: None,
        layers: Vec::new(),
    }
}

fn outputs() -> Vec<TargetOutput> {
    zone_target_outputs(&zone(), |member| {
        let device = match member.device_id.as_str() {
            "usb:k70" => "K70",
            "usb:basilisk" => "Basilisk",
            "usb:strimer" => "Strimer",
            _ => "TL LCD 1",
        };
        let name = member
            .segment
            .clone()
            .filter(|segment| segment != "Display")
            .unwrap_or_else(|| device.to_owned());
        (name, device.to_owned())
    })
}

fn layer(scope: Option<&[&str]>) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: Some("Ripples".to_owned()),
        source: LayerSource::ColorFill {
            rgba: [0.2, 0.4, 0.6, 1.0],
        },
        blend: BlendMode::Screen,
        opacity: 0.75,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
        scope: scope.map(scope_of),
    }
}

fn scope_of(ids: &[&str]) -> LayerScope {
    LayerScope {
        outputs: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

fn ids(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn refs<'a>(ids: &[&'a str]) -> HashSet<&'a str> {
    ids.iter().copied().collect()
}

#[test]
fn the_target_editor_lists_outputs_by_device_and_leaves_screens_out() {
    let outputs = outputs();
    let groups = target_groups(&outputs);

    let devices: Vec<&str> = groups
        .iter()
        .map(|group| group.device_name.as_str())
        .collect();
    assert_eq!(devices, ["K70", "Basilisk", "Strimer"]);
    assert_eq!(groups[2].ids(), ["atx", "gpu"]);
    assert!(
        groups
            .iter()
            .flat_map(|group| &group.outputs)
            .all(|output| output.id != "lcd"),
        "a screen is not a target in this wave"
    );
    assert_eq!(pickable_count(&outputs), 4);
}

#[test]
fn a_zone_with_one_led_output_has_nothing_to_choose_between() {
    let mut zone = zone();
    zone.members
        .retain(|member| member.id.0 == "k70" || member.segment.as_deref() == Some("Display"));
    let outputs = zone_target_outputs(&zone, |member| (member.name.clone(), member.name.clone()));
    assert_eq!(pickable_count(&outputs), 1);
}

#[test]
fn a_selection_becomes_the_target_and_a_screen_alone_means_the_whole_zone() {
    let outputs = outputs();

    assert_eq!(
        Target::from_selection(&ids(&[]), &outputs),
        Target::WholeZone
    );
    assert_eq!(
        Target::from_selection(&ids(&["k70", "lcd"]), &outputs),
        Target::Outputs(BTreeSet::from(["k70".to_owned()]))
    );
    assert_eq!(
        Target::from_selection(&ids(&["lcd"]), &outputs),
        Target::WholeZone
    );
    assert_eq!(
        Target::from_selection(&ids(&["gone"]), &outputs),
        Target::WholeZone
    );
}

#[test]
fn ticking_starts_a_pick_and_a_device_toggles_as_one() {
    let outputs = outputs();
    let groups = target_groups(&outputs);
    let strimer = &groups[2];
    let mut target = Target::WholeZone;

    target.toggle(&["k70".to_owned()]);
    assert_eq!(target.scope(), Some(scope_of(&["k70"])));
    assert!(!target.is_whole_zone());

    target.toggle(&["atx".to_owned()]);
    assert_eq!(target.checked(strimer), Checked::Some);
    // A device partly held takes the rest; held in full, it leaves.
    target.toggle(&strimer.ids());
    assert_eq!(target.checked(strimer), Checked::All);
    target.toggle(&strimer.ids());
    assert_eq!(target.checked(strimer), Checked::None);
    assert_eq!(target.scope(), Some(scope_of(&["k70"])));
}

#[test]
fn an_empty_pick_is_not_a_target_and_never_reads_as_the_whole_zone() {
    let outputs = outputs();
    let mut target = Target::of(Some(&scope_of(&["k70"])));

    target.toggle(&["k70".to_owned()]);

    assert!(!target.is_complete());
    assert!(!target.is_whole_zone());
    assert_eq!(draft_words(&target, &outputs), "No outputs yet");
    assert!(Target::WholeZone.is_complete());
    assert_eq!(Target::WholeZone.scope(), None);
}

#[test]
fn a_stored_scope_is_sorted_and_unique_however_it_was_picked() {
    let mut target = Target::WholeZone;
    target.toggle(&["k70".to_owned(), "atx".to_owned()]);
    target.toggle(&["basilisk".to_owned()]);
    assert_eq!(target.scope(), Some(scope_of(&["atx", "basilisk", "k70"])));
}

#[test]
fn a_target_remembers_outputs_that_left_until_told_to_drop_them() {
    let outputs = outputs();
    let mut target = Target::of(Some(&scope_of(&["gone", "k70"])));

    assert_eq!(target.missing(&outputs), ["gone"]);
    target.drop_missing(&outputs);
    assert!(target.missing(&outputs).is_empty());
    assert_eq!(target.scope(), Some(scope_of(&["k70"])));
}

#[test]
fn chips_use_the_words_of_the_spec() {
    let outputs = outputs();
    let words = |ids: &[&str]| target_words(Some(&scope_of(ids)), &outputs);

    assert_eq!(target_words(None, &outputs), TargetWords::WholeZone);
    assert_eq!(target_words(None, &outputs).label(), WHOLE_ZONE);
    assert_eq!(words(&["k70"]).label(), "K70");
    assert_eq!(words(&["k70", "basilisk"]).label(), "K70 and Basilisk");
    assert_eq!(words(&["k70", "basilisk", "atx"]).label(), "3 outputs");
    assert_eq!(words(&["gone"]), TargetWords::Dormant);
    assert_eq!(words(&["gone"]).label(), NOT_IN_ZONE);
}

#[test]
fn a_device_held_in_full_goes_by_its_name_and_a_channel_by_its_own() {
    let outputs = outputs();
    let label = |ids: &[&str]| outputs_label(&refs(ids), &outputs);

    assert_eq!(label(&["atx", "gpu"]).as_deref(), Some("Strimer"));
    assert_eq!(label(&["atx"]).as_deref(), Some("ATX"));
    assert_eq!(
        label(&["k70", "atx", "gpu"]).as_deref(),
        Some("K70 and Strimer")
    );
    assert_eq!(
        label(&["atx", "gpu", "k70", "basilisk"]).as_deref(),
        Some("4 outputs")
    );
    assert_eq!(label(&[]), None);
    assert_eq!(
        outputs_detail(&refs(&["k70", "atx"]), &outputs),
        "K70, ATX (Strimer)"
    );
}

#[test]
fn a_scope_with_some_outputs_gone_reads_by_the_ones_still_here() {
    let outputs = outputs();
    assert_eq!(
        target_words(Some(&scope_of(&["gone", "k70", "left"])), &outputs),
        TargetWords::Outputs {
            label: "K70".to_owned(),
            missing: 2,
        }
    );
}

#[test]
fn a_layer_is_dormant_only_when_none_of_its_outputs_is_a_member() {
    let members = refs(&["k70", "basilisk", "lcd"]);

    assert!(!is_dormant(&layer(None), &members));
    assert!(!is_dormant(&layer(Some(&["k70"])), &members));
    assert!(!is_dormant(&layer(Some(&["gone", "k70"])), &members));
    assert!(is_dormant(&layer(Some(&["gone"])), &members));
    // A screen is a member like any other: a layer on it is not dormant.
    assert!(!is_dormant(&layer(Some(&["lcd"])), &members));
}

#[test]
fn a_selection_puts_the_rows_that_reach_it_in_front() {
    let whole = layer(None);
    let keyboard = layer(Some(&["k70"]));
    let strimer = layer(Some(&["atx", "gpu"]));
    let selected = ids(&["k70"]);

    assert!(applies_to(&whole, &selected));
    assert!(applies_to(&keyboard, &selected));
    assert!(!applies_to(&strimer, &selected));

    assert!(!stands_back(&whole, &selected));
    assert!(!stands_back(&keyboard, &selected));
    assert!(stands_back(&strimer, &selected));
    // Nothing selected, nothing stands back.
    assert!(!stands_back(&strimer, &ids(&[])));
    // A selection across devices keeps every row that reaches part of it.
    assert!(!stands_back(&strimer, &ids(&["k70", "gpu"])));
}

#[test]
fn hovering_a_row_lights_its_live_outputs_and_a_box_answers_its_rows() {
    let members = refs(&["k70", "atx", "gpu"]);

    assert!(hover_outputs(&layer(None), &members).is_empty());
    assert_eq!(
        hover_outputs(&layer(Some(&["gone", "k70"])), &members),
        ids(&["k70"])
    );

    assert!(names_output(&layer(Some(&["k70"])), Some("k70")));
    assert!(!names_output(&layer(Some(&["k70"])), Some("atx")));
    assert!(!names_output(&layer(Some(&["k70"])), None));
    // Every box is under a whole-zone layer, so none singles it out.
    assert!(!names_output(&layer(None), Some("k70")));
}

#[test]
fn layers_are_counted_in_words() {
    assert_eq!(layer_count_words(1), "1 layer");
    assert_eq!(layer_count_words(3), "3 layers");
}
