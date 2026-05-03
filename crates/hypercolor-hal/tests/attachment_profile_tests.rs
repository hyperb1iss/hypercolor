use hypercolor_hal::attachment_profile::effective_attachment_slots;
use hypercolor_types::attachment::AttachmentBinding;
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
        capabilities: DeviceCapabilities::default(),
        firmware_version: None,
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
        capabilities: DeviceCapabilities::default(),
        firmware_version: None,
    }
}

fn nollie32_nos2_info() -> DeviceInfo {
    let mut info = nollie32_info();
    info.origin = DeviceOrigin::native("nollie", "usb", ConnectionType::Usb)
        .with_protocol_id("nollie/nollie-32-nos2");
    info
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

#[test]
fn prism_s_gpu_only_slots_are_rebased_to_zero() {
    let slots = effective_attachment_slots(
        &prism_s_info(),
        &[binding("gpu-strimer", "lian-li-gpu-strimer-4x27")],
    );
    let gpu = slots
        .iter()
        .find(|slot| slot.id == "gpu-strimer")
        .expect("gpu slot should exist");

    assert_eq!(gpu.led_start, 0);
}

#[test]
fn prism_s_dual_slot_profiles_keep_gpu_after_atx() {
    let slots = effective_attachment_slots(
        &prism_s_info(),
        &[
            binding("atx-strimer", "lian-li-atx-strimer"),
            binding("gpu-strimer", "lian-li-gpu-strimer-4x27"),
        ],
    );
    let gpu = slots
        .iter()
        .find(|slot| slot.id == "gpu-strimer")
        .expect("gpu slot should exist");

    assert_eq!(gpu.led_start, 120);
}

#[test]
fn nollie32_attachment_slots_append_strimer_cables() {
    let slots = effective_attachment_slots(
        &nollie32_info(),
        &[binding("atx-strimer", "nollie-atx-strimer")],
    );
    let atx = slots
        .iter()
        .find(|slot| slot.id == "atx-strimer")
        .expect("ATX slot should exist");
    let gpu = slots
        .iter()
        .find(|slot| slot.id == "gpu-strimer")
        .expect("GPU slot should exist");

    assert_eq!(atx.led_start, 5120);
    assert_eq!(gpu.led_start, 5240);
}

#[test]
fn nollie32_gpu_slot_rebases_when_atx_is_not_enabled() {
    let slots = effective_attachment_slots(
        &nollie32_info(),
        &[binding("gpu-strimer", "nollie-gpu-strimer-6x27")],
    );
    let gpu = slots
        .iter()
        .find(|slot| slot.id == "gpu-strimer")
        .expect("GPU slot should exist");

    assert_eq!(gpu.led_start, 5120);
}

#[test]
fn nollie32_nos2_attachment_slots_append_strimer_cables() {
    let slots = effective_attachment_slots(
        &nollie32_nos2_info(),
        &[binding("atx-strimer", "nollie-atx-strimer")],
    );
    let gpu = slots
        .iter()
        .find(|slot| slot.id == "gpu-strimer")
        .expect("GPU slot should exist");

    assert_eq!(gpu.led_start, 5240);
}
