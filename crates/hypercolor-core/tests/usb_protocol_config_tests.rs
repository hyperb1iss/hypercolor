use hypercolor_core::attachment::AttachmentRegistry;
use hypercolor_core::device::UsbProtocolConfigStore;
use hypercolor_hal::protocol_config::ProtocolRuntimeConfig;
use hypercolor_types::attachment::{AttachmentBinding, AttachmentSlot, DeviceAttachmentProfile};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceId, DeviceInfo,
    DeviceOrigin, DeviceTopologyHint, ZoneInfo,
};

fn prism_s_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "PrismRGB Prism S".to_owned(),
        vendor: "PrismRGB".to_owned(),
        family: DeviceFamily::new_static("prismrgb", "PrismRGB"),
        model: Some("prism_s".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("prismrgb", "usb", ConnectionType::Usb)
            .with_protocol_id("prismrgb/prism-s"),
        zones: vec![
            ZoneInfo {
                name: "ATX Strimer".to_owned(),
                led_count: 120,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 20 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
            ZoneInfo {
                name: "GPU Strimer".to_owned(),
                led_count: 162,
                topology: DeviceTopologyHint::Matrix { rows: 6, cols: 27 },
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            },
        ],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    }
}

fn nollie32_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Nollie 32".to_owned(),
        vendor: "Nollie".to_owned(),
        family: DeviceFamily::new_static("nollie", "Nollie"),
        model: Some("nollie_32".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("nollie", "usb", ConnectionType::Usb)
            .with_protocol_id("nollie/nollie-32"),
        zones: (1..=20)
            .map(|index| ZoneInfo {
                name: format!("Channel {index}"),
                led_count: 256,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Grb,
                layout_hint: None,
            })
            .collect(),
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    }
}

fn attachment_registry() -> AttachmentRegistry {
    let mut registry = AttachmentRegistry::new();
    registry
        .load_builtins()
        .expect("built-in attachments should load");
    registry
}

async fn stored_config(
    configs: &UsbProtocolConfigStore,
    device_id: DeviceId,
) -> ProtocolRuntimeConfig {
    configs
        .config(device_id)
        .await
        .expect("protocol runtime config should be stored")
}

#[tokio::test]
async fn prism_s_config_defaults_to_legacy_full_topology_without_bindings() {
    let info = prism_s_info();
    let registry = attachment_registry();
    let profile = info.default_attachment_profile();
    let configs = UsbProtocolConfigStore::new();

    assert!(
        configs
            .apply_attachment_profile(info.id, &info, &profile, &registry)
            .await
    );

    let config = stored_config(&configs, info.id).await;
    assert_eq!(config.protocol_id(), "prismrgb/prism-s");
    assert_eq!(config.atx_attachment_leds(), 120);
    assert_eq!(config.gpu_attachment_leds(), 162);
}

#[tokio::test]
async fn prism_s_config_derives_dual_gpu_from_attachment_binding() {
    let info = prism_s_info();
    let registry = attachment_registry();
    let mut profile = info.default_attachment_profile();
    profile.bindings = vec![
        AttachmentBinding {
            slot_id: "atx-strimer".to_owned(),
            template_id: "nollie-atx-strimer".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        },
        AttachmentBinding {
            slot_id: "gpu-strimer".to_owned(),
            template_id: "lian-li-gpu-strimer-4x27".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        },
    ];
    let configs = UsbProtocolConfigStore::new();

    assert!(
        configs
            .apply_attachment_profile(info.id, &info, &profile, &registry)
            .await
    );

    let config = stored_config(&configs, info.id).await;
    assert_eq!(config.protocol_id(), "prismrgb/prism-s");
    assert_eq!(config.atx_attachment_leds(), 120);
    assert_eq!(config.gpu_attachment_leds(), 108);
}

#[tokio::test]
async fn prism_s_config_supports_gpu_only_profiles() {
    let info = prism_s_info();
    let registry = attachment_registry();
    let profile = DeviceAttachmentProfile {
        schema_version: 1,
        slots: vec![AttachmentSlot {
            id: "gpu-strimer".to_owned(),
            name: "GPU Strimer".to_owned(),
            led_start: 0,
            led_count: 162,
            suggested_categories: vec![],
            allowed_templates: vec![],
            allow_custom: true,
        }],
        bindings: vec![AttachmentBinding {
            slot_id: "gpu-strimer".to_owned(),
            template_id: "lian-li-gpu-strimer-4x27".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        }],
        suggested_zones: vec![],
    };
    let configs = UsbProtocolConfigStore::new();

    assert!(
        configs
            .apply_attachment_profile(info.id, &info, &profile, &registry)
            .await
    );

    let config = stored_config(&configs, info.id).await;
    assert_eq!(config.protocol_id(), "prismrgb/prism-s");
    assert_eq!(config.atx_attachment_leds(), 0);
    assert_eq!(config.gpu_attachment_leds(), 108);
}

#[tokio::test]
async fn nollie32_config_defaults_to_bare_hub_without_bindings() {
    let info = nollie32_info();
    let registry = attachment_registry();
    let profile = info.default_attachment_profile();
    let configs = UsbProtocolConfigStore::new();

    assert!(
        configs
            .apply_attachment_profile(info.id, &info, &profile, &registry)
            .await
    );

    let config = stored_config(&configs, info.id).await;
    assert_eq!(config.protocol_id(), "nollie/nollie-32");
    assert_eq!(config.atx_attachment_leds(), 0);
    assert_eq!(config.gpu_attachment_leds(), 0);
}

#[tokio::test]
async fn nollie32_config_derives_cables_from_attachment_bindings() {
    let info = nollie32_info();
    let registry = attachment_registry();
    let mut profile = info.default_attachment_profile();
    profile.bindings = vec![
        AttachmentBinding {
            slot_id: "atx-strimer".to_owned(),
            template_id: "lian-li-atx-strimer".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        },
        AttachmentBinding {
            slot_id: "gpu-strimer".to_owned(),
            template_id: "nollie-gpu-strimer-6x27".to_owned(),
            name: None,
            enabled: true,
            instances: 1,
            led_offset: 0,
        },
    ];
    let configs = UsbProtocolConfigStore::new();

    assert!(
        configs
            .apply_attachment_profile(info.id, &info, &profile, &registry)
            .await
    );

    let config = stored_config(&configs, info.id).await;
    assert_eq!(config.protocol_id(), "nollie/nollie-32");
    assert_eq!(config.atx_attachment_leds(), 120);
    assert_eq!(config.gpu_attachment_leds(), 162);
}

#[tokio::test]
async fn attachment_profile_config_ignores_non_usb_protocol_devices() {
    let mut info = prism_s_info();
    info.id = DeviceId::new();
    info.family = DeviceFamily::new_static("hue", "Philips Hue");
    info.model = Some("bridge".to_owned());
    info.origin = DeviceOrigin::native("hue", "hue", ConnectionType::Network);
    let registry = attachment_registry();
    let profile = info.default_attachment_profile();
    let configs = UsbProtocolConfigStore::new();

    assert!(
        !configs
            .apply_attachment_profile(info.id, &info, &profile, &registry)
            .await
    );
    assert!(configs.config(info.id).await.is_none());
}
