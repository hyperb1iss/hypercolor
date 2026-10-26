//! Contract tests for the Studio zone tab strip rules.

use std::collections::HashMap;

use hypercolor_types::api::scene::{ZoneMember, ZoneMemberId};
use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::{DisplayFaceTarget, ZoneId, ZoneRole};

use hypercolor_ui::api::{ApiError, ZoneResource};
use hypercolor_ui::pages::studio::tab_model::{
    DeleteBlock, ReorderTarget, ScreenName, deletable, delete_prompt, describe_zone_failure,
    drag_target, drop_slot, fit_tab_count, full_zone_index, next_zone_name, nudge_target,
    screen_label_map, screen_labels, split_overflow, zone_device_count, zone_move_index,
};

fn zone(name: &str, role: ZoneRole, devices: &[&str]) -> ZoneResource {
    ZoneResource {
        id: ZoneId::new(),
        name: name.to_owned(),
        description: None,
        role,
        enabled: true,
        brightness: 1.0,
        color: None,
        display_target: None,
        members: devices
            .iter()
            .enumerate()
            .map(|(index, device)| ZoneMember {
                id: ZoneMemberId(format!("{name}-{index}")),
                device_id: (*device).to_owned(),
                segment: None,
                name: format!("{device} output"),
            })
            .collect(),
        layout: None,
        layers: Vec::new(),
    }
}

fn screen(name: &str, device: DeviceId) -> ZoneResource {
    let mut zone = zone(name, ZoneRole::Display, &[]);
    zone.display_target = Some(DisplayFaceTarget::new(device));
    zone
}

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn the_primary_and_the_last_zone_cannot_be_deleted() {
    let main = zone("Main", ZoneRole::Primary, &["a"]);
    let keyboard = zone("Keyboard", ZoneRole::Custom, &["k"]);
    let lcd = screen("LCD", DeviceId::new());
    let zones = vec![main.clone(), lcd.clone(), keyboard.clone()];
    assert_eq!(
        deletable(&zones, &main.id.to_string()),
        Err(DeleteBlock::Primary)
    );
    assert_eq!(deletable(&zones, &keyboard.id.to_string()), Ok(()));
    // A Screen is not a zone tab, so it has nothing to delete here.
    assert_eq!(
        deletable(&zones, &lcd.id.to_string()),
        Err(DeleteBlock::Missing)
    );

    let only = zone("Solo", ZoneRole::Custom, &[]);
    let lonely = vec![only.clone(), lcd];
    assert_eq!(
        deletable(&lonely, &only.id.to_string()),
        Err(DeleteBlock::LastZone)
    );
    assert!(!DeleteBlock::Primary.reason().is_empty());
}

#[test]
fn deleting_asks_only_when_devices_would_become_unassigned() {
    let keyboard = zone("Keyboard", ZoneRole::Custom, &["k", "k", "m"]);
    assert_eq!(zone_device_count(&keyboard), 2);
    assert_eq!(delete_prompt("Keyboard", 0), None);
    let one = delete_prompt("Keyboard", 1).expect("a zone with devices asks first");
    assert_eq!(one.title, "Delete \u{2018}Keyboard\u{2019}?");
    assert_eq!(one.detail, "1 device becomes unassigned.");
    let two = delete_prompt("Keyboard", 2).expect("a zone with devices asks first");
    assert_eq!(two.detail, "2 devices become unassigned.");
}

#[test]
fn nudging_moves_one_tab_and_stops_at_the_edges() {
    let led = ids(&["main", "roli", "push"]);
    assert_eq!(nudge_target(&led, "main", -1), None);
    assert_eq!(nudge_target(&led, "push", 1), None);
    assert_eq!(
        nudge_target(&led, "push", -1),
        Some(ReorderTarget::Before("roli".to_owned()))
    );
    assert_eq!(
        nudge_target(&led, "main", 1),
        Some(ReorderTarget::Before("push".to_owned()))
    );
    assert_eq!(
        nudge_target(&led, "roli", 1),
        Some(ReorderTarget::AfterLast)
    );
    assert_eq!(nudge_target(&led, "missing", 1), None);
}

#[test]
fn reorder_indices_address_the_full_zone_list_with_screens_between_tabs() {
    // The live rig's shape: the primary zone, five Screens, then two
    // custom zones.
    let order = [
        ("main", true),
        ("lcd1", false),
        ("lcd2", false),
        ("lcd3", false),
        ("lcd4", false),
        ("lcd5", false),
        ("roli", true),
        ("push", true),
    ];
    // Push 2 one tab left lands before ROLI, past every Screen.
    assert_eq!(
        full_zone_index(&order, "push", &ReorderTarget::Before("roli".into())),
        Some(6)
    );
    // ROLI to the front lands at index 0.
    assert_eq!(
        full_zone_index(&order, "roli", &ReorderTarget::Before("main".into())),
        Some(0)
    );
    // Main to the end lands after the last LED zone in the list without it.
    assert_eq!(
        full_zone_index(&order, "main", &ReorderTarget::AfterLast),
        Some(7)
    );
    // Moves that change nothing send nothing.
    assert_eq!(
        full_zone_index(&order, "push", &ReorderTarget::AfterLast),
        None
    );
    assert_eq!(
        full_zone_index(&order, "roli", &ReorderTarget::Before("push".into())),
        None
    );
    assert_eq!(
        full_zone_index(&order, "ghost", &ReorderTarget::AfterLast),
        None
    );
}

#[test]
fn trailing_screens_stay_behind_a_zone_moved_to_the_end() {
    let order = [("main", true), ("keys", true), ("lcd", false)];
    assert_eq!(
        full_zone_index(&order, "main", &ReorderTarget::AfterLast),
        Some(1)
    );
}

#[test]
fn zone_move_index_reads_roles_from_zone_resources() {
    let main = zone("Main", ZoneRole::Primary, &[]);
    let lcd = screen("LCD", DeviceId::new());
    let roli = zone("ROLI", ZoneRole::Custom, &[]);
    let zones = vec![main.clone(), lcd, roli.clone()];
    assert_eq!(
        zone_move_index(
            &zones,
            &roli.id.to_string(),
            &ReorderTarget::Before(main.id.to_string())
        ),
        Some(0)
    );
}

#[test]
fn a_drag_drops_before_the_tab_it_lands_ahead_of() {
    let led = ids(&["a", "b", "c", "d", "e"]);
    let visible = ids(&["a", "b", "c"]);
    assert_eq!(drop_slot(5.0, &[10.0, 30.0]), 0);
    assert_eq!(drop_slot(20.0, &[10.0, 30.0]), 1);
    assert_eq!(drop_slot(40.0, &[10.0, 30.0]), 2);
    assert_eq!(
        drag_target(&visible, "c", 0, &led),
        Some(ReorderTarget::Before("a".to_owned()))
    );
    // Past the last visible tab, ahead of the tabs folded into overflow.
    assert_eq!(
        drag_target(&visible, "a", 2, &led),
        Some(ReorderTarget::Before("d".to_owned()))
    );
    let everything = ids(&["a", "b", "c"]);
    assert_eq!(
        drag_target(&everything, "a", 2, &everything),
        Some(ReorderTarget::AfterLast)
    );
    assert_eq!(drag_target(&ids(&["a"]), "a", 0, &ids(&["a"])), None);
}

#[test]
fn tabs_shrink_before_any_overflow() {
    // Five 88px tabs with 4px gaps need 456px.
    assert_eq!(fit_tab_count(456.0, 5, 88.0, 4.0, 84.0), 5);
    assert_eq!(fit_tab_count(10_000.0, 5, 88.0, 4.0, 84.0), 5);
    // One pixel short: the tail folds behind an 84px "more" button.
    assert_eq!(fit_tab_count(455.0, 5, 88.0, 4.0, 84.0), 4);
    assert_eq!(fit_tab_count(300.0, 5, 88.0, 4.0, 84.0), 2);
    // Never fewer than one tab, so the active tab always shows.
    assert_eq!(fit_tab_count(40.0, 5, 88.0, 4.0, 84.0), 1);
    assert_eq!(fit_tab_count(40.0, 0, 88.0, 4.0, 84.0), 0);
    assert_eq!(fit_tab_count(40.0, 1, 88.0, 4.0, 84.0), 1);
}

#[test]
fn the_active_tab_always_renders_when_the_tail_overflows() {
    assert_eq!(split_overflow(3, Some(2), 5), (vec![0, 1, 2], vec![]));
    assert_eq!(split_overflow(5, Some(1), 3), (vec![0, 1, 2], vec![3, 4]));
    assert_eq!(split_overflow(5, Some(4), 3), (vec![0, 1, 4], vec![2, 3]));
    assert_eq!(split_overflow(5, None, 2), (vec![0, 1], vec![2, 3, 4]));
    assert_eq!(split_overflow(5, Some(3), 1), (vec![3], vec![0, 1, 2, 4]));
}

#[test]
fn new_zones_take_the_first_free_default_name() {
    assert_eq!(next_zone_name(&["Main"]), "New zone");
    assert_eq!(next_zone_name(&["Main", "new zone"]), "New zone 2");
    assert_eq!(
        next_zone_name(&["New zone", "New zone 2", "New zone 4"]),
        "New zone 3"
    );
}

#[test]
fn repeated_screen_names_take_device_labels_or_ordinals() {
    let named = |name: &str, label: Option<&str>| ScreenName {
        name: name.to_owned(),
        device_label: label.map(str::to_owned),
    };
    let labels = screen_labels(&[
        named("Ableton Push 2", Some("Ableton Push 2 \u{b7} Display")),
        named("TL LCD", Some("TL LCD 1")),
        named("TL LCD", Some("TL LCD 2")),
        named("TL LCD", Some("TL LCD 3")),
    ]);
    assert_eq!(
        labels,
        vec!["Ableton Push 2", "TL LCD 1", "TL LCD 2", "TL LCD 3"]
    );

    // A missing or duplicated label falls back to ordinals for the group.
    let labels = screen_labels(&[
        named("Pump LCD", Some("Pump")),
        named("Pump LCD", None),
        named("Ring", Some("Same")),
        named("Ring", Some("Same")),
    ]);
    assert_eq!(labels, vec!["Pump LCD 1", "Pump LCD 2", "Ring 1", "Ring 2"]);
}

#[test]
fn screen_label_map_reads_display_output_names_from_led_zones() {
    let first = DeviceId::new();
    let second = DeviceId::new();
    let mut main = zone("Main", ZoneRole::Primary, &["lianli:lcd-a", "lianli:lcd-b"]);
    main.members[0].name = "TL LCD 1".to_owned();
    main.members[1].name = "TL LCD 2".to_owned();
    let a = screen("Lian Li LCD", first);
    let b = screen("Lian Li LCD", second);
    let zones = vec![main, a.clone(), b.clone()];
    let layout_ids = HashMap::from([
        (first.to_string(), "lianli:lcd-a".to_owned()),
        (second.to_string(), "lianli:lcd-b".to_owned()),
    ]);
    let labels = screen_label_map(&zones, &layout_ids);
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[&a.id.to_string()], "TL LCD 1");
    assert_eq!(labels[&b.id.to_string()], "TL LCD 2");
}

#[test]
fn zone_write_failures_read_as_words() {
    let http = |status: u16, message: &str| ApiError::Http {
        status,
        code: None,
        message: Some(message.to_owned()),
        details: None,
    };
    let old_daemon = http(
        422,
        "Failed to deserialize the JSON body into the target type: index: unknown field \
         `index`, expected one of `name`, `enabled`, `brightness`, `color`",
    );
    assert_eq!(
        describe_zone_failure("Couldn't reorder zones", &old_daemon),
        "This Hypercolor daemon can't reorder zones yet. Update Hypercolor to reorder."
    );
    assert_eq!(
        describe_zone_failure(
            "Zone update failed",
            &http(
                422,
                "Failed to deserialize the JSON body into the target type: name: bad"
            )
        ),
        "Zone update failed: the daemon didn't accept the request"
    );
    assert_eq!(
        describe_zone_failure("Couldn't reorder zones", &http(404, "Zone not found.")),
        "Couldn't reorder zones: Zone not found"
    );
    assert_eq!(
        describe_zone_failure("Zone update failed", &ApiError::http(503, None)),
        "Zone update failed (HTTP 503)"
    );
    assert_eq!(
        describe_zone_failure("Zone update failed", &ApiError::Network("reset".into())),
        "Zone update failed: the daemon could not be reached"
    );
}
