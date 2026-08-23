//! Contract tests for the Studio device-assignment logic.

use hypercolor_types::api::scene::{MemberPlacement, ZoneLayoutResource, ZoneMember, ZoneMemberId};
use hypercolor_types::scene::{ZoneId, ZoneRole};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};

use hypercolor_ui::api::ZoneResource;
use hypercolor_ui::pages::studio::device_assignment::{
    DeviceMeta, device_rows_for_zone, sort_device_rows, unassigned_device_rows,
};

/// One `Output` output: a device id, an optional channel, an LED count.
fn output(device_id: &str, zone_name: Option<&str>, leds: u32) -> Output {
    Output {
        id: format!("{device_id}:{}", zone_name.unwrap_or("0")),
        name: device_id.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: zone_name.map(str::to_owned),
        position: NormalizedPosition { x: 0.5, y: 0.5 },
        size: NormalizedPosition { x: 0.2, y: 0.1 },
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: leds,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

fn meta(layout_device_id: &str, name: &str, total_leds: u32) -> DeviceMeta {
    DeviceMeta {
        layout_device_id: layout_device_id.to_owned(),
        name: name.to_owned(),
        total_leds,
    }
}

fn zone_resource_with_outputs(outputs: Vec<Output>) -> ZoneResource {
    let (members, placements) = outputs
        .into_iter()
        .map(|output| {
            let member_id = ZoneMemberId(output.id);
            let member = ZoneMember {
                id: member_id.clone(),
                device_id: output.device_id,
                segment: output.zone_name,
                name: output.name,
            };
            let placement = MemberPlacement {
                member: member_id,
                position: output.position,
                size: output.size,
                rotation: output.rotation,
                scale: output.scale,
                orientation: output.orientation,
                topology: output.topology,
            };
            (member, placement)
        })
        .unzip();
    ZoneResource {
        id: ZoneId::new(),
        name: "Zone".to_owned(),
        description: None,
        brightness: 1.0,
        enabled: true,
        color: None,
        display_target: None,
        role: ZoneRole::Primary,
        members,
        layout: Some(ZoneLayoutResource { placements }),
        layers: Vec::new(),
    }
}

#[test]
fn device_rows_collapse_outputs_of_one_device() {
    let outputs = vec![
        output("usb:keeb", Some("ch1"), 50),
        output("usb:keeb", Some("ch2"), 18),
        output("smbus:dram", None, 8),
    ];
    let devices = vec![
        meta("usb:keeb", "Corsair K70", 68),
        meta("smbus:dram", "ASUS Aura DRAM", 8),
    ];
    let zone = zone_resource_with_outputs(outputs);
    let rows = device_rows_for_zone(&zone, &devices);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].device_id, "usb:keeb");
    assert_eq!(rows[0].name, "Corsair K70");
    assert_eq!(rows[0].led_count, 68);
    assert_eq!(rows[0].member_count, 2);
    assert!(rows[0].resolved);
    assert_eq!(rows[1].device_id, "smbus:dram");
    assert_eq!(rows[1].member_count, 1);
}

#[test]
fn device_rows_keep_first_seen_order() {
    let outputs = vec![
        output("c", None, 1),
        output("a", None, 1),
        output("c", None, 1),
        output("b", None, 1),
    ];
    let zone = zone_resource_with_outputs(outputs);
    let rows = device_rows_for_zone(&zone, &[]);
    let ids: Vec<&str> = rows.iter().map(|row| row.device_id.as_str()).collect();
    assert_eq!(ids, ["c", "a", "b"]);
}

#[test]
fn sort_device_rows_orders_connected_then_name_offline_last() {
    // First-seen order is scrambled and mixes a resolved + unresolved set.
    let outputs = vec![
        output("usb:zed", None, 5),
        output("usb:ghost", None, 3),
        output("usb:apex", None, 7),
    ];
    let devices = vec![meta("usb:zed", "Zebra", 5), meta("usb:apex", "Apex", 7)];
    let zone = zone_resource_with_outputs(outputs);
    let mut rows = device_rows_for_zone(&zone, &devices);
    sort_device_rows(&mut rows);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    // Connected devices first, alphabetical; the offline row sinks last.
    assert_eq!(names, ["Apex", "Zebra", "usb:ghost"]);
    assert!(rows[0].resolved && rows[1].resolved);
    assert!(!rows[2].resolved);
}

#[test]
fn device_rows_fall_back_to_id_when_unresolved() {
    let zone = zone_resource_with_outputs(vec![output("usb:ghost", None, 12)]);
    let rows = device_rows_for_zone(&zone, &[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "usb:ghost");
    assert!(!rows[0].resolved);
}

#[test]
fn unassigned_rows_exclude_placed_and_ledless_devices() {
    let zones = vec![zone_resource_with_outputs(vec![output(
        "usb:keeb", None, 50,
    )])];
    let devices = vec![
        meta("usb:keeb", "Corsair K70", 50),
        meta("smbus:dram", "ASUS Aura DRAM", 8),
        meta("usb:lcd", "Corsair LCD", 0),
    ];
    let rows = unassigned_device_rows(&zones, &devices);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].device_id, "smbus:dram");
    assert_eq!(rows[0].led_count, 8);
}

#[test]
fn unassigned_rows_empty_when_every_device_is_placed() {
    let zones = vec![zone_resource_with_outputs(vec![
        output("a", None, 10),
        output("b", None, 10),
    ])];
    let devices = vec![meta("a", "A", 10), meta("b", "B", 10)];
    assert!(unassigned_device_rows(&zones, &devices).is_empty());
}
