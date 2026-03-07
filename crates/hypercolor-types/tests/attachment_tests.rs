use hypercolor_types::attachment::{
    AttachmentBinding, AttachmentCanvasSize, AttachmentCategory, AttachmentCompatibility,
    AttachmentOrigin, AttachmentSlot, AttachmentTemplate, AttachmentTemplateManifest,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceTopologyHint, ZoneInfo,
};
use hypercolor_types::spatial::{Corner, LedTopology, NormalizedPosition};

fn sample_template() -> AttachmentTemplateManifest {
    AttachmentTemplateManifest {
        schema_version: 1,
        template: AttachmentTemplate {
            id: "strimer-gpu-triple-8".into(),
            name: "Triple 8-pin GPU Strimer".into(),
            category: AttachmentCategory::Strimer,
            origin: AttachmentOrigin::BuiltIn,
            description: "Prism S GPU cable template".into(),
            default_size: AttachmentCanvasSize::default(),
            topology: LedTopology::Matrix {
                width: 27,
                height: 6,
                serpentine: false,
                start_corner: Corner::TopLeft,
            },
            compatible_slots: vec![AttachmentCompatibility {
                families: vec!["prismrgb".into()],
                models: vec!["prism_s".into()],
                slots: vec!["gpu-strimer".into()],
            }],
            tags: vec!["gpu".into(), "cable".into()],
        },
    }
}

fn sample_device() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Prism S".into(),
        vendor: "PrismRGB".into(),
        family: DeviceFamily::PrismRgb,
        connection_type: ConnectionType::Usb,
        zones: vec![
            ZoneInfo {
                name: "ATX Strimer".into(),
                led_count: 120,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
                color_format: DeviceColorFormat::Rgb,
            },
            ZoneInfo {
                name: "GPU Strimer".into(),
                led_count: 162,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 27 },
                color_format: DeviceColorFormat::Rgb,
            },
        ],
        firmware_version: Some("1.0.0".into()),
        capabilities: DeviceCapabilities {
            led_count: 282,
            supports_direct: true,
            supports_brightness: false,
            max_fps: 60,
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
        },
    };

    let toml_str = toml::to_string_pretty(&manifest).expect("toml serialize");
    let back: AttachmentTemplateManifest = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back, manifest);
}

#[test]
fn attachment_slot_supports_built_in_and_custom_templates() {
    let slot = AttachmentSlot {
        id: "gpu-strimer".into(),
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
fn attachment_compatibility_matches_family_model_and_slot() {
    let compatibility = AttachmentCompatibility {
        families: vec!["prismrgb".into()],
        models: vec!["prism_s".into()],
        slots: vec!["gpu-strimer".into()],
    };

    assert!(compatibility.matches("prismrgb", Some("prism_s"), "gpu-strimer"));
    assert!(!compatibility.matches("prismrgb", Some("prism_8"), "gpu-strimer"));
    assert!(!compatibility.matches("nollie", Some("prism_s"), "gpu-strimer"));
    assert!(!compatibility.matches("prismrgb", None, "gpu-strimer"));
}

#[test]
fn attachment_binding_defaults_enabled() {
    let binding = AttachmentBinding {
        slot_id: "gpu-strimer".into(),
        template_id: "strimer-gpu-triple-8".into(),
        name: Some("GPU Cable".into()),
        enabled: true,
    };

    let toml_str = toml::to_string_pretty(&binding).expect("toml serialize");
    let back: AttachmentBinding = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back, binding);
}
