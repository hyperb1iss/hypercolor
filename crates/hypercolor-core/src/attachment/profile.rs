use hypercolor_types::attachment::{
    AttachmentBinding, AttachmentCategory, AttachmentSlot, DeviceAttachmentProfile,
};
use hypercolor_types::device::{DeviceFamily, DeviceInfo, DeviceTopologyHint};

#[must_use]
pub fn effective_attachment_slots(
    device: &DeviceInfo,
    bindings: &[AttachmentBinding],
) -> Vec<AttachmentSlot> {
    let mut slots = device.default_attachment_profile().slots;
    append_nollie32_cable_slots(device, &mut slots);
    normalize_prism_s_slot_offsets(device, bindings, &mut slots);
    normalize_nollie32_slot_offsets(device, bindings, &mut slots);
    slots
}

pub fn normalize_attachment_profile_slots(
    device: &DeviceInfo,
    profile: &mut DeviceAttachmentProfile,
) {
    append_nollie32_cable_slots(device, &mut profile.slots);
    normalize_prism_s_slot_offsets(device, &profile.bindings, &mut profile.slots);
    normalize_nollie32_slot_offsets(device, &profile.bindings, &mut profile.slots);
}

fn normalize_prism_s_slot_offsets(
    device: &DeviceInfo,
    bindings: &[AttachmentBinding],
    slots: &mut [AttachmentSlot],
) {
    if !is_protocol_device(
        device,
        "prismrgb/prism-s",
        DeviceFamily::PrismRgb,
        "prism_s",
    ) {
        return;
    }

    let has_enabled_atx = bindings
        .iter()
        .any(|binding| binding.enabled && binding.slot_id == "atx-strimer");
    let has_enabled_gpu = bindings
        .iter()
        .any(|binding| binding.enabled && binding.slot_id == "gpu-strimer");

    if !has_enabled_gpu || has_enabled_atx {
        return;
    }

    if let Some(slot) = slots.iter_mut().find(|slot| slot.id == "gpu-strimer") {
        slot.led_start = 0;
    }
}

fn append_nollie32_cable_slots(device: &DeviceInfo, slots: &mut Vec<AttachmentSlot>) {
    if !is_protocol_device(
        device,
        "nollie/nollie-32",
        DeviceFamily::Nollie,
        "nollie_32",
    ) {
        return;
    }

    let main_leds = device
        .zones
        .iter()
        .filter(|zone| matches!(zone.topology, DeviceTopologyHint::Strip))
        .map(|zone| zone.led_count)
        .sum::<u32>();

    if !slots.iter().any(|slot| slot.id == "atx-strimer") {
        slots.push(AttachmentSlot {
            id: "atx-strimer".to_owned(),
            name: "ATX Strimer".to_owned(),
            led_start: main_leds,
            led_count: 120,
            suggested_categories: vec![AttachmentCategory::Strimer, AttachmentCategory::Matrix],
            allowed_templates: vec![
                "nollie-atx-strimer".to_owned(),
                "lian-li-atx-strimer".to_owned(),
            ],
            allow_custom: true,
        });
    }

    if !slots.iter().any(|slot| slot.id == "gpu-strimer") {
        slots.push(AttachmentSlot {
            id: "gpu-strimer".to_owned(),
            name: "GPU Strimer".to_owned(),
            led_start: main_leds.saturating_add(120),
            led_count: 162,
            suggested_categories: vec![AttachmentCategory::Strimer, AttachmentCategory::Matrix],
            allowed_templates: vec![
                "nollie-gpu-strimer-4x27".to_owned(),
                "nollie-gpu-strimer-6x27".to_owned(),
                "lian-li-gpu-strimer-4x27".to_owned(),
                "lian-li-gpu-strimer-6x27".to_owned(),
            ],
            allow_custom: true,
        });
    }
}

fn normalize_nollie32_slot_offsets(
    device: &DeviceInfo,
    bindings: &[AttachmentBinding],
    slots: &mut [AttachmentSlot],
) {
    if !is_protocol_device(
        device,
        "nollie/nollie-32",
        DeviceFamily::Nollie,
        "nollie_32",
    ) {
        return;
    }

    let main_leds = device
        .zones
        .iter()
        .filter(|zone| matches!(zone.topology, DeviceTopologyHint::Strip))
        .map(|zone| zone.led_count)
        .sum::<u32>();
    let has_enabled_atx = bindings
        .iter()
        .any(|binding| binding.enabled && binding.slot_id == "atx-strimer");

    if let Some(slot) = slots.iter_mut().find(|slot| slot.id == "atx-strimer") {
        slot.led_start = main_leds;
    }
    if let Some(slot) = slots.iter_mut().find(|slot| slot.id == "gpu-strimer") {
        slot.led_start = if has_enabled_atx {
            main_leds.saturating_add(120)
        } else {
            main_leds
        };
    }
}

fn is_protocol_device(
    device: &DeviceInfo,
    protocol_id: &str,
    family: DeviceFamily,
    model: &str,
) -> bool {
    device.origin.protocol_id.as_deref() == Some(protocol_id)
        || (device.family == family && device.model.as_deref() == Some(model))
}
