//! Device attachment endpoints — `/api/v1/devices/{id}/attachments`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use hypercolor_core::attachment::{effective_attachment_slots, normalize_attachment_profile_slots};
use hypercolor_core::spatial::generate_positions;
use hypercolor_types::attachment::{
    AttachmentBinding, AttachmentSlot, AttachmentSuggestedZone, AttachmentTemplate,
    DeviceAttachmentProfile,
};
use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::logical_devices;

use super::{ensure_default_logical_entry, resolve_device_id_or_response};

#[derive(Debug, Deserialize, Default)]
pub struct UpdateAttachmentsRequest {
    #[serde(default)]
    pub bindings: Vec<AttachmentBinding>,
}

#[derive(Debug, Serialize)]
pub struct DeviceAttachmentsResponse {
    pub device_id: String,
    pub device_name: String,
    pub slots: Vec<AttachmentSlot>,
    pub bindings: Vec<AttachmentBindingSummary>,
    pub suggested_zones: Vec<AttachmentSuggestedZone>,
}

#[derive(Debug, Serialize)]
pub struct DeviceAttachmentsUpdateResponse {
    pub device_id: String,
    pub device_name: String,
    pub slots: Vec<AttachmentSlot>,
    pub bindings: Vec<AttachmentBindingSummary>,
    pub suggested_zones: Vec<AttachmentSuggestedZone>,
    pub needs_layout_update: bool,
}

#[derive(Debug, Serialize)]
pub struct AttachmentBindingSummary {
    pub slot_id: String,
    pub template_id: String,
    pub template_name: String,
    pub name: Option<String>,
    pub enabled: bool,
    pub instances: u32,
    pub led_offset: u32,
    pub effective_led_count: u32,
}

#[derive(Debug, Serialize)]
pub struct AttachmentPreviewResponse {
    pub device_id: String,
    pub device_name: String,
    pub zones: Vec<AttachmentPreviewZone>,
}

#[derive(Debug, Serialize)]
pub struct AttachmentPreviewZone {
    pub slot_id: String,
    pub binding_index: usize,
    pub instance: u32,
    pub template_id: String,
    pub template_name: String,
    pub name: String,
    pub led_start: u32,
    pub led_count: u32,
    pub topology: LedTopology,
    pub led_positions: Vec<NormalizedPosition>,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedAttachmentBinding {
    pub(super) index: usize,
    pub(super) binding: AttachmentBinding,
    pub(super) slot: AttachmentSlot,
    pub(super) template: AttachmentTemplate,
    pub(super) effective_led_count: u32,
}

/// `GET /api/v1/devices/:id/attachments` — Get a device attachment profile.
pub async fn get_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
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

/// `PUT /api/v1/devices/:id/attachments` — Save a device attachment profile.
pub async fn update_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAttachmentsRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };
    let slots = effective_attachment_slots(&tracked.info, &body.bindings);
    let resolved = {
        let registry = state.attachment_registry.read().await;
        match validate_attachment_bindings(&tracked.info, &slots, &body.bindings, &registry) {
            Ok(bindings) => bindings,
            Err(response) => return response,
        }
    };

    let suggested_zones = suggested_attachment_zones(&resolved);
    let profile = DeviceAttachmentProfile {
        schema_version: 1,
        slots: slots.clone(),
        bindings: resolved.iter().map(|item| item.binding.clone()).collect(),
        suggested_zones: suggested_zones.clone(),
    };
    let device_key = tracked.info.id.to_string();
    {
        let mut profiles = state.attachment_profiles.write().await;
        profiles.update(&device_key, profile);
        if let Err(error) = profiles.save() {
            return ApiError::internal(format!("Failed to persist attachment profile: {error}"));
        }
    }

    let layout_device_id = ensure_default_logical_entry(&state, &tracked.info).await;
    let needs_layout_update =
        active_layout_targets_device(&state, tracked.info.id, &layout_device_id).await;

    ApiResponse::ok(DeviceAttachmentsUpdateResponse {
        device_id: tracked.info.id.to_string(),
        device_name: tracked.info.name.clone(),
        slots,
        bindings: summarize_resolved_bindings(&resolved),
        suggested_zones,
        needs_layout_update,
    })
}

/// `POST /api/v1/devices/:id/attachments/preview` — Preview attachment zones.
pub async fn preview_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAttachmentsRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };
    let slots = effective_attachment_slots(&tracked.info, &body.bindings);
    let resolved = {
        let registry = state.attachment_registry.read().await;
        match validate_attachment_bindings(&tracked.info, &slots, &body.bindings, &registry) {
            Ok(bindings) => bindings,
            Err(response) => return response,
        }
    };

    ApiResponse::ok(AttachmentPreviewResponse {
        device_id: tracked.info.id.to_string(),
        device_name: tracked.info.name.clone(),
        zones: preview_attachment_zones(&resolved),
    })
}

/// `DELETE /api/v1/devices/:id/attachments` — Remove a stored attachment profile.
pub async fn delete_attachments(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let device_id = match resolve_device_id_or_response(&state, &id).await {
        Ok(id) => id,
        Err(response) => return response,
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return ApiError::not_found(format!("Device not found: {id}"));
    };

    let deleted = {
        let mut profiles = state.attachment_profiles.write().await;
        let deleted = profiles.remove(&tracked.info.id.to_string()).is_some();
        if deleted && let Err(error) = profiles.save() {
            return ApiError::internal(format!(
                "Failed to persist attachment profile deletion: {error}"
            ));
        }
        deleted
    };

    ApiResponse::ok(serde_json::json!({
        "device_id": tracked.info.id.to_string(),
        "deleted": deleted,
    }))
}

fn summarize_attachment_profile(
    device: &DeviceInfo,
    mut profile: DeviceAttachmentProfile,
    registry: &hypercolor_core::attachment::AttachmentRegistry,
) -> DeviceAttachmentsResponse {
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

    DeviceAttachmentsResponse {
        device_id: device.id.to_string(),
        device_name: device.name.clone(),
        slots: profile.slots,
        bindings,
        suggested_zones,
    }
}

fn summarize_attachment_binding(
    binding: &AttachmentBinding,
    template: Option<&AttachmentTemplate>,
) -> AttachmentBindingSummary {
    AttachmentBindingSummary {
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
    bindings: &[ResolvedAttachmentBinding],
) -> Vec<AttachmentBindingSummary> {
    bindings
        .iter()
        .map(|binding| AttachmentBindingSummary {
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

fn preview_attachment_zones(bindings: &[ResolvedAttachmentBinding]) -> Vec<AttachmentPreviewZone> {
    let mut zones = Vec::new();

    for binding in bindings {
        let led_positions = generate_positions(&binding.template.topology);
        let template_led_count = binding.template.led_count();
        for instance in 0..binding.binding.instances {
            let led_start = binding
                .slot
                .led_start
                .saturating_add(binding.binding.led_offset)
                .saturating_add(instance.saturating_mul(template_led_count));
            zones.push(AttachmentPreviewZone {
                slot_id: binding.binding.slot_id.clone(),
                binding_index: binding.index,
                instance,
                template_id: binding.binding.template_id.clone(),
                template_name: binding.template.name.clone(),
                name: preview_attachment_zone_name(binding, instance),
                led_start,
                led_count: template_led_count,
                topology: binding.template.topology.clone(),
                led_positions: led_positions.clone(),
            });
        }
    }

    disambiguate_attachment_zone_names(&mut zones);
    zones
}

pub(super) fn suggested_attachment_zones(
    bindings: &[ResolvedAttachmentBinding],
) -> Vec<AttachmentSuggestedZone> {
    let mut zones = Vec::new();

    for binding in bindings {
        let template_led_count = binding.template.led_count();
        for instance in 0..binding.binding.instances {
            let led_start = binding
                .slot
                .led_start
                .saturating_add(binding.binding.led_offset)
                .saturating_add(instance.saturating_mul(template_led_count));
            zones.push(AttachmentSuggestedZone {
                slot_id: binding.binding.slot_id.clone(),
                template_id: binding.binding.template_id.clone(),
                template_name: binding.template.name.clone(),
                name: preview_attachment_zone_name(binding, instance),
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

fn preview_attachment_zone_name(binding: &ResolvedAttachmentBinding, instance: u32) -> String {
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

trait NamedAttachmentZone {
    fn slot_id(&self) -> &str;
    fn name(&self) -> &str;
    fn name_mut(&mut self) -> &mut String;
}

impl NamedAttachmentZone for AttachmentPreviewZone {
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

impl NamedAttachmentZone for AttachmentSuggestedZone {
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

fn disambiguate_attachment_zone_names<T: NamedAttachmentZone>(zones: &mut [T]) {
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
    profile: &DeviceAttachmentProfile,
    registry: &hypercolor_core::attachment::AttachmentRegistry,
) -> Option<Vec<ResolvedAttachmentBinding>> {
    validate_attachment_bindings(device, &profile.slots, &profile.bindings, registry).ok()
}

#[expect(
    clippy::result_large_err,
    reason = "private handler helper returns a concrete HTTP response on validation failure"
)]
fn validate_attachment_bindings(
    device: &DeviceInfo,
    slots: &[AttachmentSlot],
    bindings: &[AttachmentBinding],
    registry: &hypercolor_core::attachment::AttachmentRegistry,
) -> Result<Vec<ResolvedAttachmentBinding>, Response> {
    let slot_index = slots
        .iter()
        .map(|slot| (slot.id.as_str(), slot))
        .collect::<HashMap<_, _>>();
    let mut resolved = Vec::with_capacity(bindings.len());

    for (index, binding) in bindings.iter().enumerate() {
        let slot_id = binding.slot_id.trim();
        if slot_id.is_empty() {
            return Err(ApiError::validation(format!(
                "binding {index} has an empty slot_id"
            )));
        }

        let template_id = binding.template_id.trim();
        if template_id.is_empty() {
            return Err(ApiError::validation(format!(
                "binding {index} has an empty template_id"
            )));
        }

        if binding.instances == 0 {
            return Err(ApiError::validation(format!(
                "binding {index} must set instances to at least 1"
            )));
        }

        let Some(slot) = slot_index.get(slot_id).copied() else {
            return Err(ApiError::validation(format!(
                "binding {index} targets unknown slot '{slot_id}'"
            )));
        };
        let Some(template) = registry.get(template_id) else {
            return Err(ApiError::validation(format!(
                "binding {index} references unknown template '{template_id}'"
            )));
        };

        if !slot.supports_template(template) {
            return Err(ApiError::validation(format!(
                "template '{template_id}' is not allowed for slot '{slot_id}'"
            )));
        }
        if !template_supports_device_slot(template, device, slot_id) {
            return Err(ApiError::validation(format!(
                "template '{template_id}' is not compatible with {} slot '{slot_id}'",
                device.name
            )));
        }

        let effective_led_count = binding.effective_led_count(template);
        let Some(binding_end) = binding.led_offset.checked_add(effective_led_count) else {
            return Err(ApiError::validation(format!(
                "binding {index} exceeds slot '{slot_id}' LED range"
            )));
        };
        if binding_end > slot.led_count {
            return Err(ApiError::validation(format!(
                "binding {index} exceeds slot '{slot_id}' capacity: {binding_end} > {}",
                slot.led_count
            )));
        }

        resolved.push(ResolvedAttachmentBinding {
            index,
            binding: AttachmentBinding {
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
    template: &AttachmentTemplate,
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

#[expect(
    clippy::result_large_err,
    reason = "private handler helper returns a concrete HTTP response on validation failure"
)]
fn validate_attachment_overlaps(bindings: &[ResolvedAttachmentBinding]) -> Result<(), Response> {
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
            return Err(ApiError::validation(format!(
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

#[cfg(test)]
mod tests {
    use hypercolor_core::attachment::effective_attachment_slots;
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
}
