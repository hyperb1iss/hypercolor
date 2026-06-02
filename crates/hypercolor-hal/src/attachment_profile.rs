//! HAL-owned attachment profile rules for protocol-specific slot topology.

use hypercolor_types::attachment::{
    ComponentBinding, ComponentCategory, ComponentSlot, DeviceComponentProfile,
};
use hypercolor_types::device::{DeviceInfo, DeviceTopologyHint};

const PRISM_S_PROTOCOL_ID: &str = "prismrgb/prism-s";
const NOLLIE32_PROTOCOL_ID: &str = "nollie/nollie-32";
const NOLLIE32_NOS2_PROTOCOL_IDS: &[&str] = &["nollie/nollie-32-nos2", "nollie/nollie-32-nos2-alt"];
const GENERIC_CHANNEL_PROTOCOL_IDS: &[&str] = &[
    "nollie/prism-8",
    "nollie/nollie-8-v2",
    "nollie/nollie-32",
    "nollie/nollie-32-nos2",
    "nollie/nollie-32-nos2-alt",
    "prismrgb/prism-mini",
];

#[must_use]
pub fn effective_attachment_slots(
    device: &DeviceInfo,
    bindings: &[ComponentBinding],
) -> Vec<ComponentSlot> {
    let mut slots = device.default_attachment_profile().slots;
    augment_generic_channel_categories(device, &mut slots);
    append_nollie32_cable_slots(device, &mut slots);
    normalize_prism_s_slot_offsets(device, bindings, &mut slots);
    normalize_nollie32_slot_offsets(device, bindings, &mut slots);
    slots
}

pub fn normalize_attachment_profile_slots(
    device: &DeviceInfo,
    profile: &mut DeviceComponentProfile,
) {
    augment_generic_channel_categories(device, &mut profile.slots);
    append_nollie32_cable_slots(device, &mut profile.slots);
    normalize_prism_s_slot_offsets(device, &profile.bindings, &mut profile.slots);
    normalize_nollie32_slot_offsets(device, &profile.bindings, &mut profile.slots);
}

fn augment_generic_channel_categories(device: &DeviceInfo, slots: &mut [ComponentSlot]) {
    if !GENERIC_CHANNEL_PROTOCOL_IDS
        .iter()
        .any(|protocol_id| has_protocol(device, protocol_id))
    {
        return;
    }

    for slot in slots.iter_mut().filter(|slot| {
        slot.name.starts_with("Channel ")
            && slot
                .suggested_categories
                .contains(&ComponentCategory::Strip)
    }) {
        for category in [
            ComponentCategory::Fan,
            ComponentCategory::Aio,
            ComponentCategory::Heatsink,
            ComponentCategory::Ring,
        ] {
            if !slot.suggested_categories.contains(&category) {
                slot.suggested_categories.push(category);
            }
        }
    }
}

fn normalize_prism_s_slot_offsets(
    device: &DeviceInfo,
    bindings: &[ComponentBinding],
    slots: &mut [ComponentSlot],
) {
    if !has_protocol(device, PRISM_S_PROTOCOL_ID) {
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

fn append_nollie32_cable_slots(device: &DeviceInfo, slots: &mut Vec<ComponentSlot>) {
    if !is_nollie32_protocol(device) {
        return;
    }

    let main_leds = device
        .zones
        .iter()
        .filter(|zone| matches!(zone.topology, DeviceTopologyHint::Strip))
        .map(|zone| zone.led_count)
        .sum::<u32>();

    if !slots.iter().any(|slot| slot.id == "atx-strimer") {
        slots.push(ComponentSlot {
            id: "atx-strimer".to_owned(),
            name: "ATX Strimer".to_owned(),
            led_start: main_leds,
            led_count: 120,
            suggested_categories: vec![ComponentCategory::Strimer, ComponentCategory::Matrix],
            allowed_templates: vec!["lian-li-atx-strimer".to_owned()],
            allow_custom: true,
        });
    }

    if !slots.iter().any(|slot| slot.id == "gpu-strimer") {
        slots.push(ComponentSlot {
            id: "gpu-strimer".to_owned(),
            name: "GPU Strimer".to_owned(),
            led_start: main_leds.saturating_add(120),
            led_count: 162,
            suggested_categories: vec![ComponentCategory::Strimer, ComponentCategory::Matrix],
            allowed_templates: vec![
                "lian-li-gpu-strimer-4x27".to_owned(),
                "lian-li-gpu-strimer-6x27".to_owned(),
            ],
            allow_custom: true,
        });
    }
}

fn normalize_nollie32_slot_offsets(
    device: &DeviceInfo,
    bindings: &[ComponentBinding],
    slots: &mut [ComponentSlot],
) {
    if !is_nollie32_protocol(device) {
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

fn has_protocol(device: &DeviceInfo, protocol_id: &str) -> bool {
    device.origin.protocol_id.as_deref() == Some(protocol_id)
}

fn is_nollie32_protocol(device: &DeviceInfo) -> bool {
    has_protocol(device, NOLLIE32_PROTOCOL_ID)
        || device
            .origin
            .protocol_id
            .as_deref()
            .is_some_and(|value| NOLLIE32_NOS2_PROTOCOL_IDS.contains(&value))
}
