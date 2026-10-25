//! Contract tests for the Studio device pane's sections and search.

use std::collections::HashMap;

use hypercolor_types::api::scene::{MemberPlacement, ZoneLayoutResource, ZoneMember, ZoneMemberId};
use hypercolor_types::scene::{UnassignedBehavior, ZoneId, ZoneRole};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, StripDirection};

use hypercolor_ui::api::ZoneResource;
use hypercolor_ui::pages::studio::device_assignment::{DeviceMeta, ZoneDeviceRow};
use hypercolor_ui::pages::studio::pane_model::{
    ElsewhereHint, PaneInputs, PaneModel, build_pane, elsewhere_label, normalize_search,
    parse_unassigned_policy, row_matches, section_count, shows_unassigned,
    unassigned_policy_options, unassigned_policy_value,
};

/// A zone holding `(device id, segment, LEDs)` members, each placed.
fn zone(name: &str, role: ZoneRole, members: &[(&str, Option<&str>, u32)]) -> ZoneResource {
    let (members, placements) = members
        .iter()
        .enumerate()
        .map(|(index, (device, segment, leds))| {
            let id = ZoneMemberId(format!("{name}-{index}"));
            let member = ZoneMember {
                id: id.clone(),
                device_id: (*device).to_owned(),
                segment: segment.map(str::to_owned),
                name: format!("{device} {}", segment.unwrap_or("")),
            };
            let placement = MemberPlacement {
                member: id,
                position: NormalizedPosition { x: 0.5, y: 0.5 },
                size: NormalizedPosition { x: 0.2, y: 0.1 },
                rotation: 0.0,
                scale: 1.0,
                orientation: None,
                topology: LedTopology::Strip {
                    count: *leds,
                    direction: StripDirection::LeftToRight,
                },
            };
            (member, placement)
        })
        .unzip();
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
        layout: Some(ZoneLayoutResource { placements }),
        layers: Vec::new(),
    }
}

fn meta(id: &str, name: &str, leds: u32) -> DeviceMeta {
    DeviceMeta {
        layout_device_id: id.to_owned(),
        name: name.to_owned(),
        total_leds: leds,
    }
}

struct Rig {
    main: ZoneResource,
    push: ZoneResource,
    roli: ZoneResource,
    lcd: ZoneResource,
    devices: Vec<DeviceMeta>,
}

/// A small copy of the live rig: Main holds the keyboard, the Push 2's
/// white buttons, and a controller missing from the registry; the Push 2
/// zone holds its pads and transport; ROLI holds the Lightpad.
fn rig() -> Rig {
    Rig {
        main: zone(
            "Main",
            ZoneRole::Primary,
            &[
                ("usb:k70", None, 123),
                ("push2:1", Some("White Buttons"), 40),
                ("prismrgb:old", None, 120),
            ],
        ),
        push: zone(
            "Push 2",
            ZoneRole::Custom,
            &[
                ("push2:1", Some("Pads"), 64),
                ("push2:1", Some("Transport"), 12),
            ],
        ),
        roli: zone("ROLI", ZoneRole::Custom, &[("roli:pad", None, 225)]),
        lcd: zone("LCD", ZoneRole::Display, &[]),
        devices: vec![
            meta("usb:k70", "Corsair K70", 123),
            meta("push2:1", "Ableton Push 2", 116),
            meta("roli:pad", "Lightpad Block", 225),
            meta("dygma:defy", "Dygma Defy", 176),
            meta("wled:theater", "WLED Theater", 240),
        ],
    }
}

fn pane(
    rig: &Rig,
    active: &ZoneResource,
    search: &str,
    terms: &HashMap<String, Vec<String>>,
) -> PaneModel {
    let zones = vec![
        rig.main.clone(),
        rig.lcd.clone(),
        rig.push.clone(),
        rig.roli.clone(),
    ];
    let saved_only = vec![ZoneDeviceRow {
        device_id: "nollie:gone".to_owned(),
        name: "nollie:gone".to_owned(),
        led_count: 32,
        member_count: 1,
        resolved: false,
    }];
    build_pane(PaneInputs {
        zones: &zones,
        active_zone_id: &active.id.to_string(),
        devices: &rig.devices,
        saved_only,
        search,
        terms,
    })
}

fn names(rows: impl IntoIterator<Item = String>) -> Vec<String> {
    rows.into_iter().collect()
}

#[test]
fn the_active_zone_comes_first_with_offline_devices_split_out() {
    let rig = rig();
    let model = pane(&rig, &rig.main, "", &HashMap::new());
    let active = model.active.expect("Main is an LED zone");
    assert_eq!(active.zone_id, rig.main.id.to_string());
    assert_eq!(active.total, 3);
    assert_eq!(
        names(active.devices.iter().map(|device| device.row.name.clone())),
        vec!["Ableton Push 2", "Corsair K70"]
    );
    assert_eq!(active.offline.len(), 1);
    assert_eq!(active.offline[0].row.device_id, "prismrgb:old");
    assert!(!active.offline[0].row.resolved);
    assert_eq!(active.hits(), 3);
    assert!(!model.searching);
}

#[test]
fn a_split_device_names_where_its_other_outputs_live() {
    let rig = rig();
    let model = pane(&rig, &rig.main, "", &HashMap::new());
    let active = model.active.expect("Main is an LED zone");
    let push = active
        .devices
        .iter()
        .find(|device| device.row.device_id == "push2:1")
        .expect("the Push 2 has outputs in Main");
    // Main's card counts only Main's share of the device.
    assert_eq!(push.row.member_count, 1);
    assert_eq!(push.row.led_count, 40);
    assert_eq!(
        push.elsewhere,
        vec![ElsewhereHint {
            zone_id: rig.push.id.to_string(),
            outputs: 2,
        }]
    );
    let keyboard = active
        .devices
        .iter()
        .find(|device| device.row.device_id == "usb:k70")
        .expect("the keyboard lives in Main");
    assert!(keyboard.elsewhere.is_empty());

    // From the Push 2 tab, the hint points back at Main.
    let model = pane(&rig, &rig.push, "", &HashMap::new());
    let active = model.active.expect("Push 2 is an LED zone");
    assert_eq!(active.devices.len(), 1);
    assert_eq!(active.devices[0].row.member_count, 2);
    assert_eq!(
        active.devices[0].elsewhere,
        vec![ElsewhereHint {
            zone_id: rig.main.id.to_string(),
            outputs: 1,
        }]
    );
}

#[test]
fn other_zones_list_every_other_led_zone_in_scene_order() {
    let rig = rig();
    let model = pane(&rig, &rig.main, "", &HashMap::new());
    let ids: Vec<String> = model
        .others
        .iter()
        .map(|section| section.zone_id.clone())
        .collect();
    assert_eq!(ids, vec![rig.push.id.to_string(), rig.roli.id.to_string()]);
    assert_eq!(model.others[0].total, 1);
    assert!(model.others.iter().all(|section| {
        section
            .devices
            .iter()
            .all(|device| device.elsewhere.is_empty())
    }));
}

#[test]
fn unassigned_holds_unplaced_registry_devices_and_saved_controllers() {
    let rig = rig();
    let model = pane(&rig, &rig.main, "", &HashMap::new());
    assert_eq!(model.unassigned.total, 3);
    assert_eq!(
        names(model.unassigned.rows.iter().map(|row| row.name.clone())),
        vec!["Dygma Defy", "WLED Theater", "nollie:gone"]
    );
    assert!(!model.unassigned.rows[2].resolved);
}

#[test]
fn a_screen_selection_has_no_active_device_section() {
    let rig = rig();
    let model = pane(&rig, &rig.lcd, "", &HashMap::new());
    assert!(model.active.is_none());
    assert_eq!(model.others.len(), 3);
}

#[test]
fn search_filters_every_section_and_counts_hits() {
    let rig = rig();
    let model = pane(&rig, &rig.main, "  PUSH ", &HashMap::new());
    assert!(model.searching);
    let active = model.active.expect("Main is an LED zone");
    assert_eq!(active.hits(), 1);
    assert_eq!(active.total, 3);
    assert_eq!(section_count(active.hits(), active.total, true), "1 of 3");
    // Only zones with hits stay, so ROLI drops out.
    assert_eq!(model.others.len(), 1);
    assert_eq!(model.others[0].zone_id, rig.push.id.to_string());
    assert!(model.unassigned.rows.is_empty());
    assert_eq!(model.unassigned.total, 3);
}

#[test]
fn search_matches_channel_and_component_names() {
    let rig = rig();
    let terms = HashMap::from([
        ("roli:pad".to_owned(), vec!["Grid".to_owned()]),
        (
            "dygma:defy".to_owned(),
            vec!["Left Underglow".to_owned(), "Desk strip".to_owned()],
        ),
    ]);
    let model = pane(&rig, &rig.main, "underglow", &terms);
    assert_eq!(model.active.as_ref().map(|section| section.hits()), Some(0));
    assert!(model.others.is_empty());
    assert_eq!(
        names(model.unassigned.rows.iter().map(|row| row.name.clone())),
        vec!["Dygma Defy"]
    );

    let model = pane(&rig, &rig.main, "grid", &terms);
    assert_eq!(model.others.len(), 1);
    assert_eq!(model.others[0].zone_id, rig.roli.id.to_string());
}

#[test]
fn row_matching_normalizes_and_treats_empty_as_everything() {
    let row = ZoneDeviceRow {
        device_id: "usb:k70".to_owned(),
        name: "Corsair K70".to_owned(),
        led_count: 123,
        member_count: 1,
        resolved: true,
    };
    let terms = HashMap::from([("usb:k70".to_owned(), vec!["Keyboard".to_owned()])]);
    assert_eq!(normalize_search("  K70  "), "k70");
    assert!(row_matches(&row, "", &terms));
    assert!(row_matches(&row, "k70", &terms));
    assert!(row_matches(&row, "keyb", &terms));
    assert!(!row_matches(&row, "mouse", &terms));
    assert_eq!(section_count(2, 19, false), "19");
}

#[test]
fn unassigned_shows_in_single_zone_scenes_only_when_non_empty() {
    assert!(!shows_unassigned(1, 0));
    assert!(shows_unassigned(1, 2));
    assert!(shows_unassigned(2, 0));
}

#[test]
fn elsewhere_labels_count_outputs() {
    assert_eq!(elsewhere_label(1, "Push 2"), "1 more output in Push 2");
    assert_eq!(elsewhere_label(2, "Push 2"), "2 more outputs in Push 2");
}

#[test]
fn the_unassigned_policy_round_trips_through_the_select() {
    let zone_id = ZoneId::new();
    for behavior in [
        UnassignedBehavior::Off,
        UnassignedBehavior::Hold,
        UnassignedBehavior::Fallback(zone_id),
    ] {
        let value = unassigned_policy_value(&behavior);
        assert_eq!(parse_unassigned_policy(&value), Some(behavior));
    }
    assert_eq!(parse_unassigned_policy("fallback:nope"), None);
    assert_eq!(parse_unassigned_policy("sparkle"), None);

    let options = unassigned_policy_options(&[(zone_id.to_string(), "Main".to_owned())]);
    assert_eq!(
        options,
        vec![
            ("off".to_owned(), "Off".to_owned()),
            ("hold".to_owned(), "Hold last colors".to_owned()),
            (format!("fallback:{zone_id}"), "Follow Main".to_owned()),
        ]
    );
}
