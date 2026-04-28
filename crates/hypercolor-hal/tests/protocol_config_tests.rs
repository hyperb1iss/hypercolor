use hypercolor_hal::drivers::nollie::GpuCableType;
use hypercolor_hal::drivers::prismrgb::PrismSGpuCable;
use hypercolor_hal::{ProtocolRuntimeConfig, runtime_config_for_attachment_profile};
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
        family: DeviceFamily::PrismRgb,
        model: Some("prism_s".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("prismrgb", "usb", ConnectionType::Usb)
            .with_protocol_id("prismrgb/prism-s"),
        zones: Vec::new(),
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    }
}

fn nollie32_info() -> DeviceInfo {
    DeviceInfo {
        id: DeviceId::new(),
        name: "Nollie 32".to_owned(),
        vendor: "Nollie".to_owned(),
        family: DeviceFamily::Nollie,
        model: Some("nollie_32".to_owned()),
        connection_type: ConnectionType::Usb,
        origin: DeviceOrigin::native("nollie", "usb", ConnectionType::Usb)
            .with_protocol_id("nollie/nollie-32"),
        zones: vec![ZoneInfo {
            name: "Channel 1".to_owned(),
            led_count: 256,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Grb,
        }],
        firmware_version: None,
        capabilities: DeviceCapabilities::default(),
    }
}

fn profile(bindings: Vec<AttachmentBinding>) -> DeviceAttachmentProfile {
    DeviceAttachmentProfile {
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
        bindings,
        suggested_zones: vec![],
    }
}

fn binding(slot_id: &str, template_id: &str) -> AttachmentBinding {
    AttachmentBinding {
        slot_id: slot_id.to_owned(),
        template_id: template_id.to_owned(),
        name: None,
        enabled: true,
        instances: 1,
        led_offset: 0,
    }
}

fn template_leds(binding: &AttachmentBinding) -> Option<u32> {
    match binding.template_id.as_str() {
        "gpu-dual" => Some(108),
        "gpu-triple" => Some(162),
        "atx" => Some(120),
        _ => None,
    }
}

#[test]
fn prism_s_runtime_config_defaults_without_bindings() {
    let config =
        runtime_config_for_attachment_profile(&prism_s_info(), &profile(Vec::new()), template_leds)
            .expect("Prism S runtime config");

    let ProtocolRuntimeConfig::PrismS(config) = config else {
        panic!("expected Prism S config");
    };

    assert!(config.atx_present);
    assert_eq!(config.gpu_cable, Some(PrismSGpuCable::Triple8Pin));

    let config = ProtocolRuntimeConfig::PrismS(config);
    assert_eq!(config.atx_attachment_leds(), 120);
    assert_eq!(config.gpu_attachment_leds(), 162);
}

#[test]
fn prism_s_runtime_config_derives_gpu_cable_from_binding_led_count() {
    let config = runtime_config_for_attachment_profile(
        &prism_s_info(),
        &profile(vec![binding("gpu-strimer", "gpu-dual")]),
        template_leds,
    )
    .expect("Prism S runtime config");

    let ProtocolRuntimeConfig::PrismS(config) = config else {
        panic!("expected Prism S config");
    };

    assert!(!config.atx_present);
    assert_eq!(config.gpu_cable, Some(PrismSGpuCable::Dual8Pin));

    let config = ProtocolRuntimeConfig::PrismS(config);
    assert_eq!(config.atx_attachment_leds(), 0);
    assert_eq!(config.gpu_attachment_leds(), 108);
}

#[test]
fn nollie32_runtime_config_derives_cable_flags() {
    let config = runtime_config_for_attachment_profile(
        &nollie32_info(),
        &profile(vec![
            binding("atx-strimer", "atx"),
            binding("gpu-strimer", "gpu-triple"),
        ]),
        template_leds,
    )
    .expect("Nollie32 runtime config");

    let ProtocolRuntimeConfig::Nollie32(config) = config else {
        panic!("expected Nollie32 config");
    };

    assert!(config.atx_cable_present);
    assert_eq!(config.gpu_cable_type, GpuCableType::Triple8Pin);

    let config = ProtocolRuntimeConfig::Nollie32(config);
    assert_eq!(config.atx_attachment_leds(), 120);
    assert_eq!(config.gpu_attachment_leds(), 162);
}

#[test]
fn runtime_config_ignores_unmanaged_protocols() {
    let mut info = prism_s_info();
    info.origin = DeviceOrigin::native("generic", "usb", ConnectionType::Usb);

    assert!(
        runtime_config_for_attachment_profile(&info, &profile(Vec::new()), template_leds).is_none()
    );
}
