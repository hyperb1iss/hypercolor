use hypercolor_types::attachment::{
    AttachmentBinding, AttachmentCanvasSize, AttachmentCategory, AttachmentCompatibility,
    AttachmentOrigin, AttachmentSlot, AttachmentTemplate, AttachmentTemplateManifest,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceFeatures, DeviceId,
    DeviceInfo, DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::spatial::{Corner, LedTopology, NormalizedPosition};

fn sample_template() -> AttachmentTemplateManifest {
    AttachmentTemplateManifest {
        schema_version: 1,
        template: AttachmentTemplate {
            id: "fixture-gpu-template".into(),
            name: "Fixture GPU Accessory".into(),
            category: AttachmentCategory::Strimer,
            origin: AttachmentOrigin::BuiltIn,
            description: "Fixture Controller GPU cable template".into(),
            vendor: "Fixture Accessory".into(),
            default_size: AttachmentCanvasSize::default(),
            topology: LedTopology::Matrix {
                width: 27,
                height: 6,
                serpentine: false,
                start_corner: Corner::TopLeft,
            },
            compatible_slots: vec![AttachmentCompatibility {
                controller_ids: vec!["fixture-controller".into()],
                models: vec!["fixture_model".into()],
                slots: vec!["gpu-port".into()],
            }],
            tags: vec!["gpu".into(), "cable".into()],
            led_names: Some(vec!["Cable 1".into(), "Cable 2".into()]),
            led_mapping: Some(vec![1, 0]),
            image_url: Some("https://assets.hypercolor.dev/strimer.png".into()),
            physical_size_mm: Some((324.0, 34.0)),
        },
    }
}

fn sample_device() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Fixture Controller".into(),
        vendor: "Fixture Controller".into(),
        family: DeviceFamily::new_static("fixture-controller", "Fixture Controller"),
        model: Some("fixture_model".into()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("fixture-controller", "usb", ConnectionType::Usb),
        zones: vec![
            ZoneInfo {
                name: "ATX Strimer".into(),
                led_count: 120,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "GPU Strimer".into(),
                led_count: 162,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 27 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: Some("1.0.0".into()),
        capabilities: DeviceCapabilities {
            led_count: 282,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: hypercolor_types::device::DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        },
    }
}

#[test]
fn attachment_template_toml_round_trip() {
    let manifest = sample_template();
    let toml_str = toml::to_string_pretty(&manifest).expect("toml serialize");
    let back: AttachmentTemplateManifest = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back, manifest);
}

#[test]
fn custom_attachment_template_preserves_positions() {
    let manifest = AttachmentTemplateManifest {
        schema_version: 1,
        template: AttachmentTemplate {
            id: "my-custom-aio".into(),
            name: "My Custom AIO Halo".into(),
            category: AttachmentCategory::Aio,
            origin: AttachmentOrigin::User,
            description: String::new(),
            vendor: "Custom".into(),
            default_size: AttachmentCanvasSize::default(),
            topology: LedTopology::Custom {
                positions: vec![
                    NormalizedPosition::new(0.1, 0.5),
                    NormalizedPosition::new(0.5, 0.1),
                    NormalizedPosition::new(0.9, 0.5),
                    NormalizedPosition::new(0.5, 0.9),
                ],
            },
            compatible_slots: Vec::new(),
            tags: vec!["aio".into(), "custom".into()],
            led_names: None,
            led_mapping: None,
            image_url: None,
            physical_size_mm: None,
        },
    };

    let toml_str = toml::to_string_pretty(&manifest).expect("toml serialize");
    let back: AttachmentTemplateManifest = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back, manifest);
}

#[test]
fn attachment_slot_supports_built_in_and_custom_templates() {
    let slot = AttachmentSlot {
        id: "gpu-port".into(),
        name: "GPU Port".into(),
        led_start: 120,
        led_count: 162,
        suggested_categories: vec![AttachmentCategory::Strimer],
        allowed_templates: Vec::new(),
        allow_custom: false,
    };

    let built_in = sample_template().template;
    assert!(slot.supports_template(&built_in));

    let custom = AttachmentTemplate {
        origin: AttachmentOrigin::User,
        ..built_in.clone()
    };
    assert!(!slot.supports_template(&custom));

    let override_slot = AttachmentSlot {
        allow_custom: false,
        allowed_templates: vec![custom.id.clone()],
        ..slot
    };
    assert!(override_slot.supports_template(&custom));
}

#[test]
fn attachment_slot_accepts_any_other_category_bucket() {
    let slot = AttachmentSlot {
        id: "ambient".into(),
        name: "Ambient".into(),
        led_start: 0,
        led_count: 32,
        suggested_categories: vec![AttachmentCategory::Other("other".into())],
        allowed_templates: Vec::new(),
        allow_custom: true,
    };

    let template = AttachmentTemplate {
        id: "hex-panel".into(),
        name: "Hex Panel".into(),
        category: AttachmentCategory::Other("panel".into()),
        origin: AttachmentOrigin::BuiltIn,
        description: String::new(),
        vendor: "Misc".into(),
        default_size: AttachmentCanvasSize::default(),
        topology: LedTopology::Point,
        compatible_slots: Vec::new(),
        tags: Vec::new(),
        led_names: None,
        led_mapping: None,
        image_url: None,
        physical_size_mm: None,
    };

    assert!(slot.supports_template(&template));
}

#[test]
fn device_info_derives_default_attachment_profile_from_zones() {
    let device = sample_device();
    let profile = device.default_attachment_profile();

    assert_eq!(profile.schema_version, 1);
    assert_eq!(profile.bindings.len(), 0);
    assert_eq!(profile.slots.len(), 2);

    assert_eq!(profile.slots[0].id, "atx-strimer");
    assert_eq!(profile.slots[0].led_start, 0);
    assert_eq!(profile.slots[0].led_count, 120);
    assert!(
        profile.slots[0]
            .suggested_categories
            .contains(&AttachmentCategory::Strimer)
    );

    assert_eq!(profile.slots[1].id, "gpu-strimer");
    assert_eq!(profile.slots[1].led_start, 120);
    assert_eq!(profile.slots[1].led_count, 162);
}

#[test]
fn device_info_default_attachment_profile_deduplicates_slot_ids() {
    let device = DeviceInfo {
        zones: vec![
            ZoneInfo {
                name: "Channel 1".into(),
                led_count: 16,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "Channel 1".into(),
                led_count: 16,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        ..sample_device()
    };

    let profile = device.default_attachment_profile();
    assert_eq!(profile.slots[0].id, "channel-1");
    assert_eq!(profile.slots[1].id, "channel-1-2");
}

#[test]
fn default_attachment_profile_uses_topology_categories_only() {
    let device = DeviceInfo {
        name: "Fixture Controller".into(),
        model: Some("fixture".into()),
        origin: DeviceOrigin::native("fixture-driver", "usb", ConnectionType::Usb)
            .with_protocol_id("fixture/protocol"),
        zones: vec![ZoneInfo {
            name: "Channel 1".into(),
            led_count: 126,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }],
        ..sample_device()
    };

    let profile = device.default_attachment_profile();
    assert!(
        profile.slots[0]
            .suggested_categories
            .contains(&AttachmentCategory::Strip)
    );
    assert!(
        !profile.slots[0]
            .suggested_categories
            .contains(&AttachmentCategory::Fan)
    );
}

#[test]
fn attachment_compatibility_matches_controller_model_and_slot() {
    let compatibility = AttachmentCompatibility {
        controller_ids: vec!["fixture-controller".into()],
        models: vec!["fixture_model".into()],
        slots: vec!["gpu-port".into()],
    };

    assert!(compatibility.matches("fixture-controller", Some("fixture_model"), "gpu-port"));
    assert!(!compatibility.matches("fixture-controller", Some("other_model"), "gpu-port"));
    assert!(!compatibility.matches("other-controller", Some("fixture_model"), "gpu-port"));
    assert!(!compatibility.matches("fixture-controller", None, "gpu-port"));
}

#[test]
fn attachment_binding_round_trips_defaults() {
    let binding = AttachmentBinding {
        slot_id: "gpu-port".into(),
        template_id: "fixture-gpu-template".into(),
        name: Some("GPU Cable".into()),
        enabled: true,
        instances: 3,
        led_offset: 12,
    };

    let toml_str = toml::to_string_pretty(&binding).expect("toml serialize");
    let back: AttachmentBinding = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back, binding);
}

#[test]
fn attachment_binding_defaults_enabled_instances_and_offset() {
    let back: AttachmentBinding = toml::from_str(
        r#"
slot_id = "gpu-port"
template_id = "fixture-gpu-template"
"#,
    )
    .expect("toml deserialize");

    assert!(back.enabled);
    assert_eq!(back.instances, 1);
    assert_eq!(back.led_offset, 0);
}

#[test]
fn attachment_category_unknown_string_round_trips() {
    let category = AttachmentCategory::Other("desk".into());
    let serialized = serde_json::to_string(&category).expect("serialize category");
    assert_eq!(serialized, "\"desk\"");

    let restored: AttachmentCategory =
        serde_json::from_str(&serialized).expect("deserialize category");
    assert_eq!(restored, category);
}
