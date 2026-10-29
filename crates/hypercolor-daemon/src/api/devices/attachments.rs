//! Device attachment endpoints — `/api/v1/devices/{id}/attachments`.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tracing::debug;

use hypercolor_core::attachment::{effective_attachment_slots, normalize_attachment_profile_slots};
use hypercolor_types::attachment::{ComponentBinding, ComponentTemplate, DeviceComponentProfile};
use hypercolor_types::device::{DeviceId, DeviceInfo};

use crate::api::envelope;
use crate::api::extract::{ApiJson, ApiPath};
use crate::app_state::AppState;
use crate::attachment_profiles::resolution::{
    ResolvedComponentBinding, resolve_profile_bindings, suggested_attachment_zones,
    validate_attachment_bindings,
};
use crate::domain::{DomainError, ResourceKind};
use crate::logical_devices;

use super::{ensure_default_logical_entry, resolve_device_id_or_error};

pub use hypercolor_types::api::devices::{
    ComponentBindingSummary, DeleteAttachmentsResponse, DeviceComponentsResponse,
    DeviceComponentsUpdateResponse, UpdateAttachmentsRequest,
};

/// `GET /api/v1/devices/{id}/attachments` — Get a device attachment profile.
pub async fn get_attachments(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
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

    envelope::ok(summarize_attachment_profile(
        &tracked.info,
        profile,
        &registry,
    ))
}

/// `PUT /api/v1/devices/{id}/attachments` — Save a device attachment profile.
pub async fn update_attachments(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<UpdateAttachmentsRequest>,
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
        state
            .domains
            .layout
            .resolved_layout_device_id(&state.domains.devices.layout_runtime(), &tracked.info)
            .await
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
        let layout_device_id = ensure_default_logical_entry(&state, &tracked.info).await;
        state
            .event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DeviceSettingsChanged { key: None });

        layout_device_id
    };
    let needs_layout_update =
        active_layout_targets_device(&state, tracked.info.id, &layout_device_id).await;

    envelope::ok(DeviceComponentsUpdateResponse {
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
    ApiPath(id): ApiPath<String>,
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
    if deleted {
        state
            .event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DeviceSettingsChanged { key: None });
    }

    envelope::ok(DeleteAttachmentsResponse {
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

    let spatial = state.spatial_engine.snapshot();
    spatial.layout().zones.iter().any(|zone| {
        logical_ids
            .iter()
            .any(|candidate| candidate == &zone.device_id)
    })
}
