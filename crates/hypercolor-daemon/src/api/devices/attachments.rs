//! Device attachment endpoints — `/api/v1/devices/{id}/attachments`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use tracing::debug;

use hypercolor_core::attachment::{effective_attachment_slots, normalize_attachment_profile_slots};
use hypercolor_types::attachment::{
    ComponentBinding, ComponentSlot, ComponentSuggestedZone, ComponentTemplate,
    DeviceComponentProfile,
};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use crate::api::AppState;
use crate::api::envelope::ApiResponse;
use crate::domain::{DomainError, ResourceKind};
use crate::logical_devices;

use super::{ensure_default_logical_entry, resolve_device_id_or_error};

pub use hypercolor_types::api::devices::{
    ComponentBindingSummary, DeleteAttachmentsResponse, DeviceComponentsResponse,
    DeviceComponentsUpdateResponse, UpdateAttachmentsRequest,
};

#[derive(Debug, Clone)]
pub(super) struct ResolvedComponentBinding {
    pub(super) index: usize,
    pub(super) binding: ComponentBinding,
    pub(super) slot: ComponentSlot,
    pub(super) template: ComponentTemplate,
    pub(super) effective_led_count: u32,
}

/// `GET /api/v1/devices/{id}/attachments` — Get a device attachment profile.
pub async fn get_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    let mut profile = {
        let profiles = state.attachment_profiles.read().await;
        profiles.get_or_default(&tracked.info)
    };
    normalize_attachment_profile_slots(&tracked.info, &mut profile);
    let registry = state.attachment_registry.read().await;

    ApiResponse::ok(summarize_attachment_profile(
        &tracked.info,
        profile,
        &registry,
    ))
}

/// `PUT /api/v1/devices/{id}/attachments` — Save a device attachment profile.
pub async fn update_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAttachmentsRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };
    let slots = effective_attachment_slots(&tracked.info, &body.bindings);
    let resolved = {
        let registry = state.attachment_registry.read().await;
        match validate_attachment_bindings(&tracked.info, &slots, &body.bindings, &registry) {
            Ok(bindings) => bindings,
            Err(error) => return error.into_response(),
        }
    };

    let suggested_zones = suggested_attachment_zones(&resolved);
    let profile = DeviceComponentProfile {
        schema_version: 1,
        slots: slots.clone(),
        bindings: resolved.iter().map(|item| item.binding.clone()).collect(),
        suggested_zones: suggested_zones.clone(),
    };
    let layout_device_id = if body.validate_only {
        super::resolved_layout_device_id(state.as_ref(), &tracked.info).await
    } else {
        let device_key = tracked.info.id.to_string();
        {
            let mut profiles = state.attachment_profiles.write().await;
            profiles.update(&device_key, profile.clone());
            if let Err(error) = profiles.save() {
                return DomainError::Internal(anyhow::anyhow!(
                    "Failed to persist attachment profile: {error}"
                ))
                .into_response();
            }
        }
        sync_usb_protocol_config(state.as_ref(), device_id, &tracked.info, &profile).await;

        ensure_default_logical_entry(&state, &tracked.info).await
    };
    let needs_layout_update =
        active_layout_targets_device(&state, tracked.info.id, &layout_device_id).await;

    ApiResponse::ok(DeviceComponentsUpdateResponse {
        device_id: tracked.info.id.to_string(),
        device_name: tracked.info.name.clone(),
        slots,
        bindings: summarize_resolved_bindings(&resolved),
        suggested_zones,
        needs_layout_update,
    })
}

/// `DELETE /api/v1/devices/{id}/attachments` — Remove a stored attachment profile.
pub async fn delete_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    let deleted = {
        let mut profiles = state.attachment_profiles.write().await;
        let deleted = profiles.remove(&tracked.info.id.to_string()).is_some();
        if deleted && let Err(error) = profiles.save() {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to persist attachment profile deletion: {error}"
            ))
            .into_response();
        }
        deleted
    };
    state.usb_protocol_configs.remove_device(device_id).await;

    ApiResponse::ok(DeleteAttachmentsResponse {
        device_id: tracked.info.id.to_string(),
        deleted,
    })
}

async fn sync_usb_protocol_config(
    state: &AppState,
    device_id: DeviceId,
    device: &DeviceInfo,
    profile: &DeviceComponentProfile,
) {
    let registry = state.attachment_registry.read().await;
    let applied = state
        .usb_protocol_configs
        .apply_attachment_profile(device_id, device, profile, &registry)
        .await;
    if applied {
        debug!(
            device_id = %device_id,
            device = %device.name,
            "updated USB protocol attachment config"
        );
    } else {
        state.usb_protocol_configs.remove_device(device_id).await;
    }
}

pub(super) fn summarize_attachment_profile(
    device: &DeviceInfo,
    mut profile: DeviceComponentProfile,
    registry: &hypercolor_core::attachment::ComponentRegistry,
) -> DeviceComponentsResponse {
    normalize_attachment_profile_slots(device, &mut profile);
    let suggested_zones = resolve_profile_bindings(device, &profile, registry).map_or_else(
        || profile.suggested_zones.clone(),
        |resolved| suggested_attachment_zones(&resolved),
    );
    let bindings = profile
        .bindings
        .iter()
        .map(|binding| summarize_attachment_binding(binding, registry.get(&binding.template_id)))
        .collect();

    DeviceComponentsResponse {
        device_id: device.id.to_string(),
        device_name: device.name.clone(),
        slots: profile.slots,
        bindings,
        suggested_zones,
    }
}

fn summarize_attachment_binding(
    binding: &ComponentBinding,
    template: Option<&ComponentTemplate>,
) -> ComponentBindingSummary {
    ComponentBindingSummary {
        slot_id: binding.slot_id.clone(),
        template_id: binding.template_id.clone(),
        template_name: template.map_or_else(
            || binding.template_id.clone(),
            |template| template.name.clone(),
        ),
        name: binding.name.clone(),
        enabled: binding.enabled,
        instances: binding.instances,
        led_offset: binding.led_offset,
        effective_led_count: template.map_or(0, |template| binding.effective_led_count(template)),
    }
}

fn summarize_resolved_bindings(
    bindings: &[ResolvedComponentBinding],
) -> Vec<ComponentBindingSummary> {
    bindings
        .iter()
        .map(|binding| ComponentBindingSummary {
            slot_id: binding.binding.slot_id.clone(),
            template_id: binding.binding.template_id.clone(),
            template_name: binding.template.name.clone(),
            name: binding.binding.name.clone(),
            enabled: binding.binding.enabled,
            instances: binding.binding.instances,
            led_offset: binding.binding.led_offset,
            effective_led_count: binding.effective_led_count,
        })
        .collect()
}

pub(super) fn suggested_attachment_zones(
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

fn resolve_profile_bindings(
    device: &DeviceInfo,
    profile: &DeviceComponentProfile,
    registry: &hypercolor_core::attachment::ComponentRegistry,
) -> Option<Vec<ResolvedComponentBinding>> {
    validate_attachment_bindings(device, &profile.slots, &profile.bindings, registry).ok()
}

fn validate_attachment_bindings(
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

async fn active_layout_targets_device(
    state: &AppState,
    physical_id: DeviceId,
    default_layout_id: &str,
) -> bool {
    let mut logical_ids = {
        let store = state.logical_devices.read().await;
        logical_devices::list_for_physical(&store, physical_id)
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>()
    };
    if !logical_ids.iter().any(|id| id == default_layout_id) {
        logical_ids.push(default_layout_id.to_owned());
    }
    let physical_layout_id = physical_id.to_string();
    if !logical_ids.iter().any(|id| id == &physical_layout_id) {
        logical_ids.push(physical_layout_id);
    }

    let spatial = state.spatial_engine.read().await;
    spatial.layout().zones.iter().any(|zone| {
        logical_ids
            .iter()
            .any(|candidate| candidate == &zone.device_id)
    })
}
