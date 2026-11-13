//! The rules for a screen in Studio: where its layers live, what the
//! composition panel lists for it, what its chip says, where a new face
//! goes, and how a face is copied onto the same screen of another scene.

use std::collections::{HashMap, HashSet};

use hypercolor_types::api::displays::{DisplayFaceRendering, DisplayFaceScope};
use hypercolor_types::api::scene::{ZoneMember, ZoneMemberId, ZoneResource, ZoneViewOf};
use hypercolor_types::device::DeviceId;
use hypercolor_types::layer::{
    BlendMode, LayerAdjust, LayerScope, LayerSource, LayerTransform, SceneLayer, SceneLayerId,
};
use hypercolor_types::scene::{DisplayFaceTarget, SceneMutationMode, ZoneId, ZoneRole};
use hypercolor_ui::pages::studio::screen_model::{
    Composition, FaceReport, FollowedLayoutStep, KEPT_CHIP, KEPT_NOTICE, PlacedScreen, ScreenMove,
    ScreenRef, StackHome, add_outcome_words, card_face, copy_layer_to_scene, copy_prompt,
    faceless_displays, followed_layout_step, is_screen_layer, kept_face_row, placed_screens,
    plan_add_to_all_screens, plan_screen_adds, reorder_within, screen_chip, screen_panel,
    screens_label, selected_screens, takes_copies, unplaced_face, unplaced_screens,
    zone_screen_lines,
};

fn member(id: &str, device: &str, segment: Option<&str>, name: &str) -> ZoneMember {
    ZoneMember {
        id: ZoneMemberId(id.to_owned()),
        device_id: device.to_owned(),
        segment: segment.map(str::to_owned),
        name: name.to_owned(),
    }
}

fn zone(name: &str, role: ZoneRole, members: Vec<ZoneMember>) -> ZoneResource {
    ZoneResource {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: None,
        role,
        enabled: true,
        brightness: 1.0,
        color: None,
        display_target: None,
        members,
        layout: None,
        layers: Vec::new(),
        view_of: None,
    }
}

/// A display zone for `device`: a view of `owner` when one is given, a
/// stored zone of the screen's own otherwise.
fn display_zone(device: DeviceId, owner: Option<(&ZoneResource, &str)>) -> ZoneResource {
    let mut zone = zone("LCD", ZoneRole::Display, Vec::new());
    zone.display_target = Some(DisplayFaceTarget::new(device));
    zone.view_of = owner.map(|(owner, output)| ZoneViewOf {
        zone: owner.id,
        member: ZoneMemberId(output.to_owned()),
    });
    zone
}

fn layer(name: &str, scope: Option<&[&str]>) -> SceneLayer {
    SceneLayer {
        id: SceneLayerId::new(),
        name: Some(name.to_owned()),
        source: LayerSource::ColorFill {
            rgba: [0.2, 0.4, 0.6, 1.0],
        },
        blend: BlendMode::Alpha,
        opacity: 1.0,
        transform: LayerTransform::default(),
        adjust: LayerAdjust::default(),
        bindings: Vec::new(),
        enabled: true,
        scope: scope.map(|ids| LayerScope {
            outputs: ids.iter().map(|id| (*id).to_owned()).collect(),
        }),
    }
}

fn ids(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// A rig of three panels and a keyboard: Main holds the keyboard and two
/// panels, Desk holds the third.
struct Rig {
    zones: Vec<ZoneResource>,
    devices: [DeviceId; 3],
    physical: HashMap<String, String>,
}

impl Rig {
    fn new() -> Self {
        let devices = [DeviceId::new(), DeviceId::new(), DeviceId::new()];
        let main = zone(
            "Primary",
            ZoneRole::Primary,
            vec![
                member("k70", "usb:k70", None, "K70"),
                member("lcd1", "sim:lcd1", Some("Display"), "TL LCD 1"),
                member("lcd2", "sim:lcd2", Some("Display"), "TL LCD 2"),
            ],
        );
        let desk = zone(
            "Desk",
            ZoneRole::Custom,
            vec![member("lcd3", "sim:lcd3", Some("Display"), "Pump")],
        );
        let views = vec![
            display_zone(devices[0], Some((&main, "lcd1"))),
            display_zone(devices[1], Some((&main, "lcd2"))),
            display_zone(devices[2], Some((&desk, "lcd3"))),
        ];
        let physical = HashMap::from([
            ("sim:lcd1".to_owned(), devices[0].to_string()),
            ("sim:lcd2".to_owned(), devices[1].to_string()),
            ("sim:lcd3".to_owned(), devices[2].to_string()),
        ]);
        let mut zones = vec![main, desk];
        zones.extend(views);
        Self {
            zones,
            devices,
            physical,
        }
    }

    fn screens(&self) -> Vec<PlacedScreen> {
        placed_screens(&self.zones, &self.physical, true)
    }

    fn main(&mut self) -> &mut ZoneResource {
        &mut self.zones[0]
    }
}

// ── Where a screen's layers live ─────────────────────────────────────────

#[test]
fn a_placed_screen_keeps_its_layers_in_the_zone_that_places_it() {
    let rig = Rig::new();
    let screens = rig.screens();

    let names: Vec<&str> = screens.iter().map(|screen| screen.name.as_str()).collect();
    assert_eq!(names, ["TL LCD 1", "TL LCD 2", "Pump"]);
    assert_eq!(screens[0].zone_name, "Main");
    assert_eq!(screens[2].zone_name, "Desk");
    assert!(
        screens
            .iter()
            .all(|screen| screen.home == Some(StackHome::Owner))
    );
    assert_eq!(
        screens[0].home_zone_id(),
        Some(rig.zones[0].id.to_string().as_str())
    );
    assert_eq!(
        screens[0].device_id.as_deref(),
        Some(rig.devices[0].to_string().as_str())
    );
}

#[test]
fn a_screen_with_a_stored_zone_is_addressed_through_that_zone() {
    // A stack the daemon could not convert exactly stays in a zone of the
    // screen's own, with no view_of, and is written through that zone.
    let mut rig = Rig::new();
    rig.zones[2].view_of = None;
    let stored = rig.zones[2].id.to_string();

    let screens = rig.screens();
    assert_eq!(screens[0].home, Some(StackHome::Stored(stored.clone())));
    assert_eq!(screens[0].home_zone_id(), Some(stored.as_str()));
    assert_eq!(screens[1].home, Some(StackHome::Owner));
}

#[test]
fn a_daemon_from_before_screen_layers_stores_every_screen_in_its_own_zone() {
    let mut rig = Rig::new();
    for zone in &mut rig.zones[2..] {
        zone.view_of = None;
    }
    // One screen has no zone stored for it at all: nothing can hold a layer.
    rig.zones.pop();

    let screens = placed_screens(&rig.zones, &rig.physical, false);
    assert!(matches!(screens[0].home, Some(StackHome::Stored(_))));
    assert!(matches!(screens[1].home, Some(StackHome::Stored(_))));
    assert_eq!(screens[2].home, None);
    assert_eq!(screens[2].home_zone_id(), None);
}

#[test]
fn identical_panel_names_are_told_apart_by_count() {
    let mut rig = Rig::new();
    for member in &mut rig.main().members {
        if member.segment.is_some() {
            member.name = "Uni Fan TL LCD".to_owned();
        }
    }
    let names: Vec<String> = rig
        .screens()
        .into_iter()
        .map(|screen| screen.name)
        .collect();
    assert_eq!(names, ["Uni Fan TL LCD 1", "Uni Fan TL LCD 2", "Pump"]);
}

#[test]
fn a_display_no_zone_places_is_unplaced() {
    let rig = Rig::new();
    let stray = DeviceId::new();
    let displays = vec![
        (rig.devices[0].to_string(), "TL LCD 1".to_owned()),
        (stray.to_string(), "Spare panel".to_owned()),
    ];
    assert_eq!(unplaced_screens(&displays, &rig.screens()), ["Spare panel"]);
}

// ── The selection that turns the panel to screens ────────────────────────

#[test]
fn only_a_selection_of_screens_alone_turns_the_panel_to_them() {
    let rig = Rig::new();
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();

    let one = selected_screens(&screens, &main, &ids(&["lcd1"]));
    assert_eq!(screens_label(&one), "TL LCD 1");
    let two = selected_screens(&screens, &main, &ids(&["lcd1", "lcd2"]));
    assert_eq!(screens_label(&two), "TL LCD 1 and TL LCD 2");

    // A keyboard in the selection makes it a selection of the zone's
    // outputs, which the zone's own layer list answers.
    assert!(selected_screens(&screens, &main, &ids(&["lcd1", "k70"])).is_empty());
    assert!(selected_screens(&screens, &main, &ids(&[])).is_empty());
    // Another zone's screen is not this zone's selection.
    assert!(selected_screens(&screens, &main, &ids(&["lcd3"])).is_empty());
}

#[test]
fn three_screens_go_by_their_count() {
    let mut rig = Rig::new();
    rig.main()
        .members
        .push(member("lcd4", "sim:lcd4", Some("Display"), "TL LCD 3"));
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();
    let three = selected_screens(&screens, &main, &ids(&["lcd1", "lcd2", "lcd4"]));
    assert_eq!(screens_label(&three), "3 screens");
}

// ── The two groups of the panel ──────────────────────────────────────────

#[test]
fn a_layer_is_a_screens_own_only_when_screens_are_all_it_reaches() {
    let rig = Rig::new();
    let main = &rig.zones[0];
    assert!(is_screen_layer(main, &layer("Clock", Some(&["lcd1"]))));
    assert!(is_screen_layer(
        main,
        &layer("Clock", Some(&["lcd1", "lcd2"]))
    ));
    // A departed output does not count; the screens that remain do.
    assert!(is_screen_layer(
        main,
        &layer("Clock", Some(&["gone", "lcd1"]))
    ));
    assert!(!is_screen_layer(
        main,
        &layer("Glow", Some(&["k70", "lcd1"]))
    ));
    assert!(!is_screen_layer(main, &layer("Bubbles", None)));
    assert!(!is_screen_layer(main, &layer("Lost", Some(&["gone"]))));
}

#[test]
fn the_panel_lists_a_screens_layers_and_what_is_behind_it() {
    let mut rig = Rig::new();
    let bubbles = layer("Bubbles", None);
    let ripples = layer("Ripples", Some(&["k70"]));
    let glow = layer("Glow", Some(&["k70", "lcd1"]));
    let clock = layer("Neon Clock", Some(&["lcd1"]));
    let badge = layer("Badge", Some(&["lcd1", "lcd2"]));
    let other = layer("Pulse Temp", Some(&["lcd2"]));
    rig.main().layers = vec![
        bubbles.clone(),
        ripples,
        clock.clone(),
        glow.clone(),
        badge.clone(),
        other,
    ];
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();

    let panel = screen_panel(
        &rig.zones,
        &selected_screens(&screens, &main, &ids(&["lcd1"])),
    )
    .expect("the zone holds the screen");

    assert_eq!(panel.title, "TL LCD 1");
    assert_eq!(panel.heading, "On this screen");
    assert_eq!(panel.behind_heading, "Behind it, from Main");
    // Its own layers, top first: the shared badge over the clock. The
    // other panel's face is not among them.
    let rows: Vec<SceneLayerId> = panel.rows.iter().map(|row| row.layer_id).collect();
    assert_eq!(rows, [badge.id, clock.id]);
    assert!(panel.rows.iter().all(|row| row.zone_id == main));
    assert_eq!(panel.rows[1].stack_index, 2);
    // The bottom one meets what is behind the screen; the one above
    // blends within the stack.
    assert!(panel.rows[1].meets_backdrop);
    assert!(!panel.rows[0].meets_backdrop);
    assert!(
        panel
            .rows
            .iter()
            .all(|row| row.composition == Composition::Layer)
    );
    // Behind it: the canvas layers that reach it, top first. The layer on
    // the keyboard alone does not.
    let behind: Vec<(SceneLayerId, bool)> = panel
        .behind
        .iter()
        .map(|row| (row.layer_id, row.whole_zone))
        .collect();
    assert_eq!(behind, [(glow.id, false), (bubbles.id, true)]);
}

#[test]
fn a_disabled_bottom_layer_hands_the_backdrop_to_the_one_above() {
    let mut rig = Rig::new();
    let mut clock = layer("Neon Clock", Some(&["lcd1"]));
    clock.enabled = false;
    let badge = layer("Badge", Some(&["lcd1"]));
    rig.main().layers = vec![clock.clone(), badge.clone()];
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();

    let panel = screen_panel(
        &rig.zones,
        &selected_screens(&screens, &main, &ids(&["lcd1"])),
    )
    .expect("the zone holds the screen");
    let meets: Vec<(SceneLayerId, bool)> = panel
        .rows
        .iter()
        .map(|row| (row.layer_id, row.meets_backdrop))
        .collect();
    assert_eq!(meets, [(badge.id, true), (clock.id, false)]);
}

#[test]
fn several_screens_share_one_row_for_a_layer_on_all_of_them() {
    let mut rig = Rig::new();
    let clock = layer("Neon Clock", Some(&["lcd1", "lcd2"]));
    let badge = layer("Badge", Some(&["lcd2"]));
    rig.main().layers = vec![layer("Bubbles", None), clock.clone(), badge.clone()];
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();

    let panel = screen_panel(
        &rig.zones,
        &selected_screens(&screens, &main, &ids(&["lcd1", "lcd2"])),
    )
    .expect("the zone holds the screens");

    assert_eq!(panel.heading, "On these screens");
    assert_eq!(panel.behind_heading, "Behind them, from Main");
    let rows: Vec<SceneLayerId> = panel.rows.iter().map(|row| row.layer_id).collect();
    assert_eq!(rows, [badge.id, clock.id]);
    // The clock is the bottom of both stacks.
    assert!(panel.rows[1].meets_backdrop);
    assert!(!panel.rows[0].meets_backdrop);
    assert_eq!(panel.behind.len(), 1);
}

#[test]
fn a_stored_zone_lists_its_whole_stack_and_holds_the_composition_itself() {
    let mut rig = Rig::new();
    rig.zones[2].view_of = None;
    let face = layer("Neon Clock", None);
    let overlay = layer("Badge", None);
    rig.zones[2].layers = vec![face.clone(), overlay.clone()];
    rig.main().layers = vec![layer("Bubbles", None)];
    let stored = rig.zones[2].id.to_string();
    let device = rig.devices[0].to_string();
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();

    let panel = screen_panel(
        &rig.zones,
        &selected_screens(&screens, &main, &ids(&["lcd1"])),
    )
    .expect("the zone holds the screen");

    // It reads the same as any screen: its layers, then what is behind.
    assert_eq!(panel.heading, "On this screen");
    let rows: Vec<SceneLayerId> = panel.rows.iter().map(|row| row.layer_id).collect();
    assert_eq!(rows, [overlay.id, face.id]);
    assert!(panel.rows.iter().all(|row| row.zone_id == stored));
    assert_eq!(panel.rows[1].composition, Composition::Display(device));
    assert_eq!(panel.rows[0].composition, Composition::Layer);
    assert_eq!(panel.behind.len(), 1);
}

#[test]
fn layers_reorder_within_a_screens_stack_without_moving_the_rest() {
    let [base, clock, ripples, badge] = [
        SceneLayerId::new(),
        SceneLayerId::new(),
        SceneLayerId::new(),
        SceneLayerId::new(),
    ];
    let stack = [base, clock, ripples, badge];
    let group = [clock, badge];

    assert_eq!(
        reorder_within(&stack, &group, clock, 1),
        Some(vec![base, badge, ripples, clock])
    );
    assert_eq!(
        reorder_within(&stack, &group, badge, -1),
        Some(vec![base, badge, ripples, clock])
    );
    // The edges of the group are the edges of the move.
    assert_eq!(reorder_within(&stack, &group, badge, 1), None);
    assert_eq!(reorder_within(&stack, &group, clock, -1), None);
    assert_eq!(reorder_within(&stack, &group, ripples, 1), None);
}

// ── The chip, the card, and the saved face ───────────────────────────────

#[test]
fn the_chip_names_the_face_or_offers_one() {
    let named = screen_chip(Some("Neon Clock"), None);
    assert_eq!(named.face.as_deref(), Some("Neon Clock"));
    assert_eq!(named.action(), "Face settings");

    // A saved face that is showing is the face to name.
    let kept = screen_chip(None, Some("Pulse Temp"));
    assert_eq!(kept.face.as_deref(), Some("Pulse Temp"));
    assert_eq!(kept.action(), "Face settings");

    let empty = screen_chip(None, None);
    assert_eq!(empty.face, None);
    assert_eq!(empty.action(), "Face");
    assert_eq!(screen_chip(Some("  "), None).face, None);
}

#[test]
fn a_card_names_the_top_layer_of_its_own_screen() {
    let mut rig = Rig::new();
    let clock = layer("Neon Clock", Some(&["lcd1", "lcd2"]));
    let badge = layer("Badge", Some(&["lcd2"]));
    rig.main().layers = vec![layer("Bubbles", None), clock.clone(), badge.clone()];
    let screens = rig.screens();

    assert_eq!(
        card_face(&rig.zones, &screens[0]).map(|layer| layer.id),
        Some(clock.id)
    );
    assert_eq!(
        card_face(&rig.zones, &screens[1]).map(|layer| layer.id),
        Some(badge.id)
    );
    // A screen with nothing of its own names no face, whatever is behind.
    assert!(card_face(&rig.zones, &screens[2]).is_none());
}

#[test]
fn a_zones_card_lists_its_screens_with_the_face_each_shows() {
    let mut rig = Rig::new();
    rig.main().layers = vec![layer("Bubbles", None), layer("Neon Clock", Some(&["lcd1"]))];
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();
    // The second panel shows a face saved with it.
    let kept = HashMap::from([(rig.devices[1].to_string(), "Sensor Grid".to_owned())]);
    let name = |layer: &SceneLayer| layer.name.clone().unwrap_or_default();

    let lines = zone_screen_lines(&rig.zones, &screens, &main, name, &kept);
    let shown: Vec<(&str, Option<&str>)> = lines
        .iter()
        .map(|line| (line.screen.name.as_str(), line.face.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            ("TL LCD 1", Some("Neon Clock")),
            ("TL LCD 2", Some("Sensor Grid")),
        ]
    );

    // Another zone's screen is that zone's to list, and shows nothing.
    let desk = rig.zones[1].id.to_string();
    let lines = zone_screen_lines(&rig.zones, &screens, &desk, name, &kept);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].screen.name, "Pump");
    assert_eq!(lines[0].face, None);
}

#[test]
fn a_screen_no_zone_places_still_names_the_face_it_shows() {
    // Its box is in no zone, and its face goes on showing from a zone of
    // the screen's own. Its card names that face like any screen's.
    let mut rig = Rig::new();
    let spare = DeviceId::new();
    let mut stored = display_zone(spare, None);
    stored.layers = vec![layer("Under", None), layer("System Pulse", None)];
    rig.zones.push(stored);

    let face = unplaced_face(&rig.zones, &spare.to_string()).expect("the stored stack has a face");
    assert_eq!(face.name.as_deref(), Some("System Pulse"));
    // A placed screen's view is no stored stack, and a display the scene
    // lists nothing for has none.
    assert!(unplaced_face(&rig.zones, &rig.devices[0].to_string()).is_none());
    assert!(unplaced_face(&rig.zones, &DeviceId::new().to_string()).is_none());
}

#[test]
fn the_saved_face_is_asked_about_only_where_the_scene_shows_nothing() {
    // A display zone lists a screen's layers, placed or not. The views
    // of a real scene carry them; here the first panel's does.
    let mut rig = Rig::new();
    rig.zones[2].layers = vec![layer("Neon Clock", None)];
    let spare = DeviceId::new();
    rig.zones.push(display_zone(spare, None));

    let asked = faceless_displays(&rig.zones);
    assert_eq!(
        asked,
        [
            rig.devices[1].to_string(),
            rig.devices[2].to_string(),
            spare.to_string(),
        ]
    );
}

#[test]
fn a_screen_placed_where_the_rig_layout_decides_is_carried_into_that_layout() {
    let primary = zone("Main", ZoneRole::Primary, Vec::new());
    let display = display_zone(DeviceId::new(), Some((&primary, "lcd")));
    let zones = vec![primary.clone(), display];
    let arrived = ScreenMove {
        output_id: "lcd".to_owned(),
        from: None,
        to: Some(primary.id),
    };
    assert_eq!(
        followed_layout_step(&zones, std::slice::from_ref(&arrived)),
        FollowedLayoutStep {
            save: Some(primary.id),
            forget: Vec::new(),
        }
    );

    // Taking it out again is forgotten there, or the layout hands it back.
    let left = ScreenMove {
        output_id: "lcd".to_owned(),
        from: Some(primary.id),
        to: None,
    };
    assert_eq!(
        followed_layout_step(&zones, &[left]),
        FollowedLayoutStep {
            save: None,
            forget: vec!["lcd".to_owned()],
        }
    );

    // A box that only moved within the zone asks nothing.
    let stayed = ScreenMove {
        output_id: "lcd".to_owned(),
        from: Some(primary.id),
        to: Some(primary.id),
    };
    assert!(followed_layout_step(&zones, &[stayed]).is_empty());
}

#[test]
fn a_scene_with_zones_of_its_own_keeps_its_screens_itself() {
    let primary = zone("Main", ZoneRole::Primary, Vec::new());
    let desk = zone("Desk", ZoneRole::Custom, Vec::new());
    let zones = vec![primary.clone(), desk.clone()];
    let moves = [
        ScreenMove {
            output_id: "lcd".to_owned(),
            from: None,
            to: Some(primary.id),
        },
        ScreenMove {
            output_id: "pump".to_owned(),
            from: Some(desk.id),
            to: None,
        },
    ];
    assert!(followed_layout_step(&zones, &moves).is_empty());
}

fn report(scope: DisplayFaceScope, rendering: Option<DisplayFaceRendering>) -> FaceReport {
    FaceReport {
        live_scope: scope,
        rendering,
        name: "Neon Clock".to_owned(),
        effect_id: "effect-1".to_owned(),
    }
}

#[test]
fn a_saved_face_gets_a_row_only_while_it_is_what_the_screen_shows() {
    let showing = report(
        DisplayFaceScope::Default,
        Some(DisplayFaceRendering::Default),
    );
    let row = kept_face_row(Some(&showing)).expect("the saved face is showing");
    assert_eq!(row.name, "Neon Clock");
    assert_eq!(row.effect_id, "effect-1");

    // The scene's own media or color covers it: no row.
    let covered = report(DisplayFaceScope::Default, Some(DisplayFaceRendering::Scene));
    assert_eq!(kept_face_row(Some(&covered)), None);
    // The scene has a face of its own: the report is not the saved one.
    let scene = report(DisplayFaceScope::Scene, Some(DisplayFaceRendering::Scene));
    assert_eq!(kept_face_row(Some(&scene)), None);
    assert_eq!(kept_face_row(None), None);

    // A daemon that does not say what is on the panel showed a saved face
    // whenever it reported one.
    let older = report(DisplayFaceScope::Default, None);
    assert!(kept_face_row(Some(&older)).is_some());
}

#[test]
fn the_saved_face_says_what_it_is_and_what_editing_it_means() {
    assert_eq!(KEPT_CHIP, "Saved with this screen");
    assert_eq!(
        KEPT_NOTICE,
        "Changes apply in every scene using this saved face."
    );
}

// ── Where a new face goes ────────────────────────────────────────────────

#[test]
fn one_face_on_three_screens_of_a_zone_is_one_layer() {
    let mut rig = Rig::new();
    rig.main()
        .members
        .push(member("lcd0", "sim:lcd0", Some("Display"), "TL LCD 0"));
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();
    let three = selected_screens(&screens, &main, &ids(&["lcd0", "lcd1", "lcd2"]));

    let adds = plan_screen_adds(&rig.zones, &three);

    assert_eq!(adds.len(), 1);
    assert_eq!(adds[0].zone_id, main);
    // Sorted, as a stored scope is.
    assert_eq!(
        adds[0].scope.as_ref().map(|scope| scope.outputs.clone()),
        Some(vec![
            "lcd0".to_owned(),
            "lcd1".to_owned(),
            "lcd2".to_owned()
        ])
    );
    assert_eq!(adds[0].screens.len(), 3);
}

#[test]
fn add_to_all_screens_makes_one_layer_per_zone_that_owns_screens() {
    let mut rig = Rig::new();
    rig.main().layers = vec![layer("Bubbles", None), layer("Clock", Some(&["lcd1"]))];
    let screens = rig.screens();
    let main = rig.zones[0].id.to_string();
    let desk = rig.zones[1].id.to_string();

    let plan = plan_add_to_all_screens(&rig.zones, &screens, &["Spare panel".to_owned()]);

    let targets: Vec<(&str, Option<Vec<String>>)> = plan
        .adds
        .iter()
        .map(|add| {
            (
                add.zone_id.as_str(),
                add.scope.as_ref().map(|scope| scope.outputs.clone()),
            )
        })
        .collect();
    assert_eq!(
        targets,
        [
            (
                main.as_str(),
                Some(vec!["lcd1".to_owned(), "lcd2".to_owned()])
            ),
            (desk.as_str(), Some(vec!["lcd3".to_owned()])),
        ]
    );
    // One screen in Main already has a face and one has none, so the new
    // layer is the bottom of a stack somewhere and meets the backdrop.
    assert_eq!(plan.adds[0].beneath, 0);
    assert_eq!(plan.screen_count(), 3);
    assert_eq!(plan.zone_names(), ["Main", "Desk"]);
    assert_eq!(plan.skipped, ["Spare panel"]);
    assert_eq!(
        plan.summary(),
        "3 screens in Main and Desk. Spare panel is in no zone and is skipped."
    );
}

#[test]
fn add_to_all_screens_writes_a_stored_zone_through_itself() {
    let mut rig = Rig::new();
    rig.zones[2].view_of = None;
    rig.zones[2].layers = vec![layer("Old face", None)];
    let stored = rig.zones[2].id.to_string();
    let screens = rig.screens();

    let plan = plan_add_to_all_screens(&rig.zones, &screens, &[]);

    assert_eq!(plan.adds.len(), 3);
    let own = plan
        .adds
        .iter()
        .find(|add| add.zone_id == stored)
        .expect("the stored zone takes its own layer");
    assert_eq!(own.scope, None);
    assert_eq!(own.screens, ["TL LCD 1"]);
    assert_eq!(own.beneath, 1);
    assert_eq!(plan.summary(), "3 screens in Main and Desk.");
}

#[test]
fn an_add_reports_what_landed_and_what_did_not() {
    let names = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        add_outcome_words("Neon Clock", &names(&["TL LCD 1"]), &[]),
        "Neon Clock added to TL LCD 1"
    );
    assert_eq!(
        add_outcome_words("Neon Clock", &names(&["A", "B", "C"]), &[]),
        "Neon Clock added to 3 screens"
    );
    assert_eq!(
        add_outcome_words("Neon Clock", &names(&["A", "B"]), &names(&["Pump"])),
        "Neon Clock added to A and B. It could not be added to Pump."
    );
    assert_eq!(
        add_outcome_words("Neon Clock", &[], &names(&["Pump"])),
        "Neon Clock could not be added"
    );
}

// ── Copy to every scene ──────────────────────────────────────────────────

#[test]
fn copies_go_to_every_other_scene_that_is_not_locked() {
    assert!(takes_copies("b", SceneMutationMode::Live, "a"));
    assert!(!takes_copies("a", SceneMutationMode::Live, "a"));
    assert!(!takes_copies("b", SceneMutationMode::Snapshot, "a"));
    assert_eq!(
        copy_prompt("Neon Clock", "TL LCD 1", 6),
        "Use Neon Clock on TL LCD 1 in 6 scenes?"
    );
    assert_eq!(
        copy_prompt("Neon Clock", "3 screens", 1),
        "Use Neon Clock on 3 screens in 1 scene?"
    );
}

#[test]
fn a_copy_replaces_the_screens_layers_in_the_other_scene() {
    // The other scene places both panels in one zone under its own
    // output ids, with a face one panel has alone and one both share.
    let rig = Rig::new();
    let mut other = zone(
        "Primary",
        ZoneRole::Primary,
        vec![
            member("a", "usb:k70", None, "K70"),
            member("b", "sim:lcd1", Some("Display"), "TL LCD 1"),
            member("c", "sim:lcd2", Some("Display"), "TL LCD 2"),
        ],
    );
    let base = layer("Aurora", None);
    let own = layer("Old clock", Some(&["b"]));
    let shared = layer("Badge", Some(&["b", "c"]));
    let keys = layer("Ripples", Some(&["a"]));
    other.layers = vec![base.clone(), own, shared.clone(), keys.clone()];
    let mut zones = vec![other];

    let mut face = layer("Neon Clock", Some(&["lcd1"]));
    face.blend = BlendMode::Screen;
    face.opacity = 0.8;
    let screen = ScreenRef {
        device_id: rig.devices[0].to_string(),
        layout_id: "sim:lcd1".to_owned(),
    };

    assert_eq!(copy_layer_to_scene(&mut zones, &face, &[screen]), 1);

    let layers = &zones[0].layers;
    let names: Vec<&str> = layers
        .iter()
        .map(|layer| layer.name.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(names, ["Aurora", "Badge", "Ripples", "Neon Clock"]);
    // The shared layer stops naming this screen and keeps the other.
    assert_eq!(
        layers[1].scope.as_ref().map(|scope| scope.outputs.clone()),
        Some(vec!["c".to_owned()])
    );
    // The copy is limited to the screen by that scene's output id, is a
    // layer of its own, and keeps how it blends.
    let copy = &layers[3];
    assert_eq!(
        copy.scope.as_ref().map(|scope| scope.outputs.clone()),
        Some(vec!["b".to_owned()])
    );
    assert_ne!(copy.id, face.id);
    assert_eq!(copy.blend, BlendMode::Screen);
    assert!((copy.opacity - 0.8).abs() < f32::EPSILON);
    // Lighting is untouched.
    assert_eq!(layers[0], base);
    assert_eq!(layers[2], keys);
}

#[test]
fn a_copy_on_two_screens_of_one_zone_is_one_layer_there() {
    let rig = Rig::new();
    let other = zone(
        "Primary",
        ZoneRole::Primary,
        vec![
            member("b", "sim:lcd1", Some("Display"), "TL LCD 1"),
            member("c", "sim:lcd2", Some("Display"), "TL LCD 2"),
        ],
    );
    let mut zones = vec![other];
    let face = layer("Neon Clock", Some(&["lcd1", "lcd2"]));
    let screens = [
        ScreenRef {
            device_id: rig.devices[0].to_string(),
            layout_id: "sim:lcd1".to_owned(),
        },
        ScreenRef {
            device_id: rig.devices[1].to_string(),
            layout_id: "sim:lcd2".to_owned(),
        },
    ];

    assert_eq!(copy_layer_to_scene(&mut zones, &face, &screens), 2);
    assert_eq!(zones[0].layers.len(), 1);
    assert_eq!(
        zones[0].layers[0]
            .scope
            .as_ref()
            .map(|scope| scope.outputs.clone()),
        Some(vec!["b".to_owned(), "c".to_owned()])
    );
}

#[test]
fn a_copy_into_a_stored_zone_becomes_that_zones_one_layer() {
    // A scene not opened since faces became layers still stores a zone
    // for the screen. The copy becomes its one layer, and its blend and
    // amount become how that zone meets what is behind it.
    let rig = Rig::new();
    let mut stored = display_zone(rig.devices[0], None);
    stored.layers = vec![layer("Old face", None), layer("Old overlay", None)];
    let mut zones = vec![zone("Primary", ZoneRole::Primary, Vec::new()), stored];

    let mut face = layer("Neon Clock", Some(&["lcd1"]));
    face.blend = BlendMode::Screen;
    face.opacity = 0.6;
    let screen = ScreenRef {
        device_id: rig.devices[0].to_string(),
        layout_id: "sim:lcd1".to_owned(),
    };

    assert_eq!(copy_layer_to_scene(&mut zones, &face, &[screen]), 1);

    let stored = &zones[1];
    assert_eq!(stored.layers.len(), 1);
    assert_eq!(stored.layers[0].name.as_deref(), Some("Neon Clock"));
    assert_eq!(stored.layers[0].scope, None);
    assert!((stored.layers[0].opacity - 1.0).abs() < f32::EPSILON);
    let target = stored.display_target.as_ref().expect("a display zone");
    assert_eq!(target.blend_mode, BlendMode::Screen);
    assert!((target.opacity - 0.6).abs() < f32::EPSILON);
}

#[test]
fn a_scene_that_places_the_screen_nowhere_takes_no_copy() {
    let rig = Rig::new();
    let mut zones = vec![zone(
        "Primary",
        ZoneRole::Primary,
        vec![member("a", "usb:k70", None, "K70")],
    )];
    let before = zones.clone();
    let face = layer("Neon Clock", Some(&["lcd1"]));
    let screen = ScreenRef {
        device_id: rig.devices[0].to_string(),
        layout_id: "sim:lcd1".to_owned(),
    };

    assert_eq!(copy_layer_to_scene(&mut zones, &face, &[screen]), 0);
    assert_eq!(zones, before);
}
