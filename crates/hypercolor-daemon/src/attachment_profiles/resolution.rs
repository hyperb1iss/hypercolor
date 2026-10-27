//! Attachment validation and derived zones shared by edits and restores.

use crate::domain::DomainError;
use hypercolor_core::attachment::{ComponentRegistry, normalize_attachment_profile_slots};
use hypercolor_types::attachment::{
    ComponentBinding, ComponentSlot, ComponentSuggestedZone, ComponentTemplate,
    DeviceComponentProfile,
};
use hypercolor_types::device::DeviceInfo;
use std::collections::HashMap;

/// Validate authored bindings against the target device and candidate templates.
/// Cached zone suggestions are replaced only after the complete profile validates.
pub(crate) fn regenerate_attachment_profile(
    device: &DeviceInfo,
    mut profile: DeviceComponentProfile,
    registry: &ComponentRegistry,
) -> Result<DeviceComponentProfile, DomainError> {
    normalize_attachment_profile_slots(device, &mut profile);
    let resolved =
        validate_attachment_bindings(device, &profile.slots, &profile.bindings, registry)?;
    profile.bindings = resolved.iter().map(|item| item.binding.clone()).collect();
    profile.suggested_zones = suggested_attachment_zones(&resolved);
    Ok(profile)
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedComponentBinding {
    pub(crate) index: usize,
    pub(crate) binding: ComponentBinding,
    pub(crate) slot: ComponentSlot,
    pub(crate) template: ComponentTemplate,
    pub(crate) effective_led_count: u32,
}

pub(crate) fn suggested_attachment_zones(
    bindings: &[ResolvedComponentBinding],
) -> Vec<ComponentSuggestedZone> {
    let mut zones = Vec::new();

    for binding in bindings {
        let template_led_count = binding.template.led_count();
        for instance in 0..binding.binding.instances {
            let led_start = binding
                .slot
                .led_start
                .saturating_add(binding.binding.led_offset)
                .saturating_add(instance.saturating_mul(template_led_count));
            zones.push(ComponentSuggestedZone {
                slot_id: binding.binding.slot_id.clone(),
                template_id: binding.binding.template_id.clone(),
                template_name: binding.template.name.clone(),
                name: attachment_zone_name(binding, instance),
                instance,
                led_start,
                led_count: template_led_count,
                category: binding.template.category.clone(),
                default_size: binding.template.default_size,
                topology: binding.template.topology.clone(),
                led_mapping: binding.template.led_mapping.clone(),
            });
        }
    }

    disambiguate_attachment_zone_names(&mut zones);
    zones
}

fn attachment_zone_name(binding: &ResolvedComponentBinding, instance: u32) -> String {
    match binding.binding.name.as_deref() {
        Some(name) if binding.binding.instances > 1 => {
            format!("{name} - {} {}", binding.template.name, instance + 1)
        }
        Some(name) => name.to_owned(),
        None if binding.binding.instances > 1 => {
            format!("{} {}", binding.template.name, instance + 1)
        }
        None => binding.template.name.clone(),
    }
}

trait NamedComponentZone {
    fn slot_id(&self) -> &str;
    fn name(&self) -> &str;
    fn name_mut(&mut self) -> &mut String;
}

impl NamedComponentZone for ComponentSuggestedZone {
    fn slot_id(&self) -> &str {
        &self.slot_id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn name_mut(&mut self) -> &mut String {
        &mut self.name
    }
}

fn disambiguate_attachment_zone_names<T: NamedComponentZone>(zones: &mut [T]) {
    let mut totals = HashMap::<(String, String), usize>::new();
    for zone in &*zones {
        *totals
            .entry((zone.slot_id().to_owned(), zone.name().to_owned()))
            .or_insert(0) += 1;
    }

    let mut seen = HashMap::<(String, String), usize>::new();
    for zone in zones {
        let base_name = zone.name().to_owned();
        let key = (zone.slot_id().to_owned(), base_name.clone());
        if totals.get(&key).copied().unwrap_or(0) <= 1 {
            continue;
        }

        let index = seen.entry(key).or_insert(0);
        *index += 1;
        *zone.name_mut() = format!("{base_name} {index}");
    }
}

pub(crate) fn resolve_profile_bindings(
    device: &DeviceInfo,
    profile: &DeviceComponentProfile,
    registry: &hypercolor_core::attachment::ComponentRegistry,
) -> Option<Vec<ResolvedComponentBinding>> {
    validate_attachment_bindings(device, &profile.slots, &profile.bindings, registry).ok()
}

pub(crate) fn validate_attachment_bindings(
    device: &DeviceInfo,
    slots: &[ComponentSlot],
    bindings: &[ComponentBinding],
    registry: &hypercolor_core::attachment::ComponentRegistry,
) -> Result<Vec<ResolvedComponentBinding>, DomainError> {
    let slot_index = slots
        .iter()
        .map(|slot| (slot.id.as_str(), slot))
        .collect::<HashMap<_, _>>();
    let mut resolved = Vec::with_capacity(bindings.len());

    for (index, binding) in bindings.iter().enumerate() {
        let slot_id = binding.slot_id.trim();
        if slot_id.is_empty() {
            return Err(DomainError::validation(format!(
                "binding {index} has an empty slot_id"
            )));
        }

        let template_id = binding.template_id.trim();
        if template_id.is_empty() {
            return Err(DomainError::validation(format!(
                "binding {index} has an empty template_id"
            )));
        }

        if binding.instances == 0 {
            return Err(DomainError::validation(format!(
                "binding {index} must set instances to at least 1"
            )));
        }

        let Some(slot) = slot_index.get(slot_id).copied() else {
            return Err(DomainError::validation(format!(
                "binding {index} targets unknown slot '{slot_id}'"
            )));
        };
        let Some(template) = registry.get(template_id) else {
            return Err(DomainError::validation(format!(
                "binding {index} references unknown template '{template_id}'"
            )));
        };

        if !slot.supports_template(template) {
            return Err(DomainError::validation(format!(
                "template '{template_id}' is not allowed for slot '{slot_id}'"
            )));
        }
        if !template_supports_device_slot(template, device, slot_id) {
            return Err(DomainError::validation(format!(
                "template '{template_id}' is not compatible with {} slot '{slot_id}'",
                device.name
            )));
        }

        let effective_led_count = binding.effective_led_count(template);
        let Some(binding_end) = binding.led_offset.checked_add(effective_led_count) else {
            return Err(DomainError::validation(format!(
                "binding {index} exceeds slot '{slot_id}' LED range"
            )));
        };
        if binding_end > slot.led_count {
            return Err(DomainError::validation(format!(
                "binding {index} exceeds slot '{slot_id}' capacity: {binding_end} > {}",
                slot.led_count
            )));
        }

        resolved.push(ResolvedComponentBinding {
            index,
            binding: ComponentBinding {
                slot_id: slot_id.to_owned(),
                template_id: template_id.to_owned(),
                name: normalize_attachment_binding_name(binding.name.as_deref()),
                enabled: binding.enabled,
                instances: binding.instances,
                led_offset: binding.led_offset,
            },
            slot: slot.clone(),
            template: template.clone(),
            effective_led_count,
        });
    }

    validate_attachment_overlaps(&resolved)?;
    Ok(resolved)
}

fn template_supports_device_slot(
    template: &ComponentTemplate,
    device: &DeviceInfo,
    slot_id: &str,
) -> bool {
    device_attachment_compatibility_ids(device)
        .iter()
        .any(|controller_id| {
            template.supports_slot(controller_id, device.model.as_deref(), slot_id)
        })
}

fn device_attachment_compatibility_ids(device: &DeviceInfo) -> Vec<String> {
    let mut ids = Vec::with_capacity(2);
    push_unique_id(&mut ids, device.driver_id().to_owned());
    if let Some(protocol_id) = device.origin.protocol_id.as_deref() {
        push_unique_id(&mut ids, protocol_id.to_owned());
    }
    ids
}

fn push_unique_id(ids: &mut Vec<String>, id: String) {
    if !ids.iter().any(|existing| existing == &id) {
        ids.push(id);
    }
}

fn validate_attachment_overlaps(bindings: &[ResolvedComponentBinding]) -> Result<(), DomainError> {
    let mut enabled = bindings
        .iter()
        .filter(|binding| binding.binding.enabled)
        .collect::<Vec<_>>();
    enabled.sort_by(|left, right| {
        left.binding
            .slot_id
            .cmp(&right.binding.slot_id)
            .then_with(|| left.binding.led_offset.cmp(&right.binding.led_offset))
            .then_with(|| left.index.cmp(&right.index))
    });

    for pair in enabled.windows(2) {
        let [current, next] = pair else {
            continue;
        };
        if current.binding.slot_id != next.binding.slot_id {
            continue;
        }

        let current_end = current
            .binding
            .led_offset
            .saturating_add(current.effective_led_count);
        if next.binding.led_offset < current_end {
            return Err(DomainError::validation(format!(
                "bindings {} and {} overlap within slot '{}'",
                current.index, next.index, current.binding.slot_id
            )));
        }
    }

    Ok(())
}

fn normalize_attachment_binding_name(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceColorSpace, DeviceFamily,
        DeviceFeatures, DeviceId, DeviceOrigin, DeviceTopologyHint, SegmentInfo,
    };

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "Strip".into(),
            vendor: "Test".into(),
            family: DeviceFamily::new_static("wled", "WLED"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("wled", "wled", ConnectionType::Network),
            segments: vec![SegmentInfo {
                name: "Main".into(),
                led_count: 60,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
                layout_hint: None,
            }],
            firmware_version: None,
            capabilities: DeviceCapabilities {
                led_count: 60,
                supports_direct: true,
                supports_brightness: true,
                has_display: false,
                display_resolution: None,
                max_fps: 60,
                color_space: DeviceColorSpace::default(),
                features: DeviceFeatures::default(),
            },
        }
    }

    #[test]
    fn portable_attachment_regeneration_uses_candidate_catalog_and_validates_bindings() {
        let device = device();
        let mut registry = ComponentRegistry::new();
        let template: ComponentTemplate = serde_json::from_value(serde_json::json!({
            "id":"candidate-strip", "name":"Candidate strip", "category":"strip", "vendor":"Test",
            "topology":{"type":"strip", "count":12, "direction":"left_to_right"}
        }))
        .expect("template");
        registry.register(template).expect("candidate template");
        let mut profile = device.default_attachment_profile();
        profile.bindings.push(ComponentBinding {
            slot_id: "main".into(),
            template_id: "candidate-strip".into(),
            name: Some(" Desk ".into()),
            enabled: true,
            instances: 2,
            led_offset: 0,
        });
        let mut restored = regenerate_attachment_profile(&device, profile.clone(), &registry)
            .expect("valid restore");
        assert_eq!(restored.bindings[0].name.as_deref(), Some("Desk"));
        assert_eq!(restored.suggested_zones.len(), 2);
        assert_eq!(restored.suggested_zones[1].led_start, 12);
        restored.suggested_zones[0].name = "Stale cached hint".into();
        assert_ne!(
            regenerate_attachment_profile(&device, restored, &registry)
                .expect("regenerate")
                .suggested_zones[0]
                .name,
            "Stale cached hint"
        );
        assert!(
            regenerate_attachment_profile(&device, profile.clone(), &ComponentRegistry::new())
                .is_err()
        );
        profile.bindings[0].instances = 6;
        assert!(regenerate_attachment_profile(&device, profile, &registry).is_err());
    }
}
