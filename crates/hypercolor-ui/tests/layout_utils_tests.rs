#![allow(dead_code)]

#[path = "../src/api/mod.rs"]
mod api;
#[path = "../src/layout_geometry.rs"]
mod layout_geometry;

mod channel_names {
    pub fn load_channel_name(device_id: &str, slot_id: &str) -> Option<String> {
        match (device_id, slot_id) {
            ("physical:prism8", "channel-1") => Some("Radiator".to_owned()),
            _ => None,
        }
    }

    pub fn effective_channel_name(device_id: &str, slot_id: &str, default_name: &str) -> String {
        load_channel_name(device_id, slot_id).unwrap_or_else(|| default_name.to_owned())
    }
}

mod style_utils {
    pub fn uuid_v4_hex() -> String {
        "test-uuid".to_owned()
    }
}

mod toasts {
    pub fn toast_success(_msg: &str) {}
    pub fn toast_error(_msg: &str) {}
    pub fn toast_info(_msg: &str) {}
}

mod components {
    pub mod layout_builder {
        #[derive(Clone, Copy)]
        pub struct LayoutWriteHandle;

        impl LayoutWriteHandle {
            pub fn update(
                self,
                _f: impl FnOnce(&mut Option<hypercolor_types::spatial::SpatialLayout>),
            ) {
            }
        }
    }
}

#[path = "../src/layout_utils.rs"]
mod layout_utils;

use hypercolor_types::spatial::{
    DeviceZone, EdgeBehavior, LedTopology, NormalizedPosition, SamplingMode, SpatialLayout,
    ZoneAttachment, ZoneShape,
};
use std::collections::HashMap;

fn ring_zone(
    id: &str,
    name: &str,
    device_id: &str,
    zone_name: Option<&str>,
    display_order: i32,
    attachment: Option<ZoneAttachment>,
) -> DeviceZone {
    DeviceZone {
        id: id.to_owned(),
        name: name.to_owned(),
        device_id: device_id.to_owned(),
        zone_name: zone_name.map(str::to_owned),
        position: NormalizedPosition::new(0.5, 0.5),
        size: NormalizedPosition::new(0.12, 0.12),
        rotation: 0.0,
        scale: 1.0,
        orientation: None,
        topology: LedTopology::Ring {
            count: 20,
            start_angle: 0.0,
            direction: hypercolor_types::spatial::Winding::Clockwise,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: Some(ZoneShape::Ring),
        shape_preset: None,
        display_order,
        attachment,
        brightness: None,
    }
}

fn sample_zone_summary(id: &str, name: &str, led_count: usize) -> api::ZoneSummary {
    api::ZoneSummary {
        id: id.to_owned(),
        name: name.to_owned(),
        led_count,
        topology: "ring".to_owned(),
        topology_hint: Some(api::ZoneTopologySummary::Ring {
            count: u32::try_from(led_count).unwrap_or(u32::MAX),
        }),
    }
}

fn sample_device_summary(name: &str, zones: Vec<api::ZoneSummary>) -> api::DeviceSummary {
    api::DeviceSummary {
        id: "physical:prism8".to_owned(),
        layout_device_id: "usb:prism8:test".to_owned(),
        name: name.to_owned(),
        backend: "test".to_owned(),
        status: "connected".to_owned(),
        brightness: 100,
        firmware_version: None,
        network_ip: None,
        network_hostname: None,
        connection_label: None,
        total_leds: 20,
        auth: None,
        zones,
    }
}

fn sample_attachment_profile(name: Option<&str>) -> api::DeviceAttachmentsResponse {
    api::DeviceAttachmentsResponse {
        device_id: "physical:prism8".to_owned(),
        device_name: "Prism 8".to_owned(),
        slots: vec![hypercolor_types::attachment::AttachmentSlot {
            id: "channel-1".to_owned(),
            name: "Channel 1".to_owned(),
            led_start: 0,
            led_count: 20,
            suggested_categories: Vec::new(),
            allowed_templates: Vec::new(),
            allow_custom: true,
        }],
        bindings: vec![api::AttachmentBindingSummary {
            slot_id: "channel-1".to_owned(),
            template_id: "fan-template-1".to_owned(),
            template_name: "Halo Ring".to_owned(),
            name: name.map(str::to_owned),
            enabled: true,
            instances: 1,
            led_offset: 0,
            effective_led_count: 20,
        }],
        suggested_zones: Vec::new(),
    }
}

fn prism_attachment_layout() -> SpatialLayout {
    SpatialLayout {
        id: "layout".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            ring_zone(
                "source-channel-1",
                "Prism 8 · Channel 1",
                "usb:prism8:test",
                Some("Channel 1"),
                0,
                None,
            ),
            ring_zone(
                "front-fan-1",
                "Front Fan 1",
                "usb:prism8:test",
                Some("channel-1"),
                1,
                Some(ZoneAttachment {
                    template_id: "fan-template-1".to_owned(),
                    slot_id: "channel-1".to_owned(),
                    instance: 0,
                    led_start: Some(0),
                    led_count: Some(20),
                    led_mapping: None,
                }),
            ),
            ring_zone(
                "front-fan-2",
                "Front Fan 2",
                "usb:prism8:test",
                Some("channel-1"),
                2,
                Some(ZoneAttachment {
                    template_id: "fan-template-2".to_owned(),
                    slot_id: "channel-1".to_owned(),
                    instance: 1,
                    led_start: Some(20),
                    led_count: Some(20),
                    led_mapping: None,
                }),
            ),
        ],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    }
}

fn prism_seeded_attachment_layout() -> layout_geometry::SeededAttachmentLayout {
    layout_geometry::SeededAttachmentLayout {
        zones: vec![
            ring_zone(
                "front-fan-1",
                "Front Fan 1",
                "usb:prism8:test",
                Some("channel-1"),
                1,
                Some(ZoneAttachment {
                    template_id: "fan-template-1".to_owned(),
                    slot_id: "channel-1".to_owned(),
                    instance: 0,
                    led_start: Some(0),
                    led_count: Some(20),
                    led_mapping: None,
                }),
            ),
            ring_zone(
                "front-fan-2",
                "Front Fan 2",
                "usb:prism8:test",
                Some("channel-1"),
                2,
                Some(ZoneAttachment {
                    template_id: "fan-template-2".to_owned(),
                    slot_id: "channel-1".to_owned(),
                    instance: 1,
                    led_start: Some(20),
                    led_count: Some(20),
                    led_mapping: None,
                }),
            ),
        ],
    }
}

#[test]
fn zone_name_slot_alias_matching_is_symmetric() {
    assert!(layout_utils::zone_name_matches_slot_alias(
        Some("channel-1"),
        Some("Channel 1"),
    ));
    assert!(layout_utils::zone_name_matches_slot_alias(
        Some("Channel 1"),
        Some("channel-1"),
    ));
    assert!(!layout_utils::zone_name_matches_slot_alias(
        Some("channel-1"),
        Some("Channel 2"),
    ));
}

#[test]
fn attachment_binding_slot_alias_matches_generated_zone_ids_and_names() {
    assert!(layout_utils::attachment_binding_matches_slot_alias(
        "gpu-strimer",
        Some("zone_1"),
        Some("GPU Strimer"),
        "GPU Strimer",
    ));
    assert!(layout_utils::attachment_binding_matches_slot_alias(
        "channel-1",
        Some("channel-1"),
        Some("Channel 1"),
        "Radiator",
    ));
    assert!(!layout_utils::attachment_binding_matches_slot_alias(
        "atx-strimer",
        Some("zone_1"),
        Some("GPU Strimer"),
        "GPU Strimer",
    ));
}

#[test]
fn representative_zone_for_device_prefers_visible_attachment_over_suppressed_source() {
    let layout = prism_attachment_layout();

    assert_eq!(
        layout_utils::representative_zone_id_for_device(&layout, "usb:prism8:test").as_deref(),
        Some("front-fan-1")
    );
}

#[test]
fn representative_zone_for_device_slot_uses_slot_alias_and_skips_source_slot() {
    let layout = prism_attachment_layout();

    assert_eq!(
        layout_utils::representative_zone_id_for_device_slot(
            &layout,
            "usb:prism8:test",
            Some("Channel 1"),
        )
        .as_deref(),
        Some("front-fan-1")
    );
}

#[test]
fn selected_zone_matches_device_slot_when_attachment_alias_is_selected() {
    let layout = prism_attachment_layout();

    assert!(layout_utils::selected_zone_matches_device_slot(
        &layout,
        "front-fan-2",
        "usb:prism8:test",
        Some("Channel 1"),
    ));
    assert!(!layout_utils::selected_zone_matches_device_slot(
        &layout,
        "front-fan-2",
        "usb:prism8:test",
        Some("Channel 2"),
    ));
}

#[test]
fn apply_slot_display_names_to_seeded_attachment_layout_renames_matching_zones() {
    let mut seeded = prism_seeded_attachment_layout();
    let slot_display_names = HashMap::from([("channel-1".to_owned(), "Radiator".to_owned())]);

    layout_utils::apply_slot_display_names_to_seeded_attachment_layout(
        &mut seeded,
        "Prism 8",
        &slot_display_names,
    );

    assert_eq!(seeded.zones[0].name, "Radiator");
    assert_eq!(seeded.zones[0].zone_name.as_deref(), Some("channel-1"));
}

#[test]
fn sync_channel_display_name_in_layout_updates_slot_zone_name() {
    let mut layout = prism_attachment_layout();

    assert!(layout_utils::sync_channel_display_name_in_layout(
        &mut layout,
        "usb:prism8:test",
        "Prism 8",
        "channel-1",
        "Channel 1",
        "Channel 1",
        "Radiator",
    ));

    let source_zone = layout
        .zones
        .iter()
        .find(|zone| zone.id == "source-channel-1")
        .expect("source slot zone should exist");
    assert_eq!(source_zone.name, "Prism 8 · Radiator");
    assert_eq!(source_zone.zone_name.as_deref(), Some("Channel 1"));
    assert_eq!(layout.zones[1].name, "Front Fan 1");
}

#[test]
fn effective_zone_display_uses_physical_device_channel_override() {
    let zone = ring_zone(
        "source-channel-1",
        "Prism 8 · Channel 1",
        "usb:prism8:test",
        Some("Channel 1"),
        0,
        None,
    );
    let devices = vec![sample_device_summary(
        "Prism 8",
        vec![sample_zone_summary("channel-1", "Channel 1", 20)],
    )];

    let display = layout_utils::effective_zone_display(&zone, &devices, &HashMap::new());

    assert_eq!(display.label, "Prism 8 · Radiator");
    assert_eq!(display.default_label, "Prism 8 · Radiator");
    assert_eq!(
        display.identify_target,
        Some(layout_utils::ZoneIdentifyTarget::Device {
            device_id: "physical:prism8".to_owned(),
            zone_id: "channel-1".to_owned(),
        })
    );
}

#[test]
fn effective_zone_display_uses_attachment_binding_override() {
    let zone = ring_zone(
        "front-fan-1",
        "Halo Ring",
        "usb:prism8:test",
        Some("channel-1"),
        1,
        Some(ZoneAttachment {
            template_id: "fan-template-1".to_owned(),
            slot_id: "channel-1".to_owned(),
            instance: 0,
            led_start: Some(0),
            led_count: Some(20),
            led_mapping: None,
        }),
    );
    let devices = vec![sample_device_summary(
        "Prism 8",
        vec![sample_zone_summary("channel-1", "Channel 1", 20)],
    )];
    let attachment_profiles = HashMap::from([(
        "usb:prism8:test".to_owned(),
        sample_attachment_profile(Some("Front Fan")),
    )]);

    let display = layout_utils::effective_zone_display(&zone, &devices, &attachment_profiles);

    assert_eq!(display.label, "Front Fan");
    assert_eq!(display.default_label, "Front Fan");
    assert_eq!(
        display.identify_target,
        Some(layout_utils::ZoneIdentifyTarget::Attachment {
            device_id: "physical:prism8".to_owned(),
            slot_id: "channel-1".to_owned(),
            binding_index: Some(0),
            instance: Some(0),
        })
    );
}

#[test]
fn sync_device_display_name_in_layout_updates_plain_and_prefixed_defaults() {
    let mut layout = SpatialLayout {
        id: "layout".to_owned(),
        name: "Layout".to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: vec![
            ring_zone("single", "Prism 8", "usb:prism8:test", None, 0, None),
            ring_zone(
                "slot",
                "Prism 8 · Channel 1",
                "usb:prism8:test",
                Some("Channel 1"),
                1,
                None,
            ),
            ring_zone(
                "custom",
                "Top Radiator",
                "usb:prism8:test",
                Some("Channel 2"),
                2,
                None,
            ),
        ],
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    assert!(layout_utils::sync_device_display_name_in_layout(
        &mut layout,
        "usb:prism8:test",
        "Prism 8",
        "Aurora Hub",
    ));

    assert_eq!(layout.zones[0].name, "Aurora Hub");
    assert_eq!(layout.zones[1].name, "Aurora Hub · Channel 1");
    assert_eq!(layout.zones[2].name, "Top Radiator");
}
