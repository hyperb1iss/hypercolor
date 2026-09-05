//! Device endpoints — `/api/v1/devices/*`.
//!
//! Core CRUD, identify flows, and shared helpers live here. Attachment,
//! pairing, and discovery endpoints are split into sibling submodules.

mod attachments;
mod discovery;
mod pairing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use tracing::{debug, warn};

use hypercolor_color::Rgb;
use hypercolor_core::device::{BackendIo, DirectControlGuard};
use hypercolor_driver_api::DriverTrackedDevice;
use hypercolor_types::attachment::{ComponentBinding, ComponentSlot};
use hypercolor_types::device::{
    DeviceId, DeviceInfo, DeviceState, DeviceTopologyHint, DeviceUserSettings, DriverTransportKind,
};
use hypercolor_types::event::HypercolorEvent;

use crate::api::envelope;
use crate::app_state::AppState;
use crate::discovery as core_discovery;
use crate::domain::output::brightness_percent;
use crate::domain::{DomainError, ResourceKind};

pub use hypercolor_types::api::devices::{IdentifyAttachmentRequest, ListDevicesQuery};

pub use attachments::{
    ComponentBindingSummary, DeleteAttachmentsResponse, DeviceComponentsResponse,
    DeviceComponentsUpdateResponse, UpdateAttachmentsRequest, delete_attachments, get_attachments,
    update_attachments,
};
pub use discovery::{DiscoverRequest, discover_devices};
pub use pairing::{DeletePairingResponse, PairDeviceResponse, delete_pairing, pair_device};

// ── Request / Response Types ─────────────────────────────────────────────

// Wire contracts live in hypercolor-types::api::devices — shared with the
// web UI and the TUI so request/response drift is a compile error. Local
pub use hypercolor_types::api::devices::{
    DeleteDeviceResponse, DeviceConnectionSummary, DeviceListResponse, DeviceSummary,
    IdentifyAttachmentResponse, IdentifyDeviceResponse, IdentifyRequest, IdentifySegmentResponse,
    SegmentSummary, SegmentTopologySummary, UpdateDeviceRequest,
};

const IDENTIFY_FLASH_INTERVAL_MS: u64 = 250;
const DEFAULT_IDENTIFY_COLOR_RGB: [u8; 3] = [255, 255, 255];

#[derive(Debug)]
enum ResolveDeviceError {
    AmbiguousName(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DeviceListIncludes {
    attachments: bool,
}

impl DeviceListIncludes {
    fn parse(raw: Option<&str>) -> Result<Self, DomainError> {
        let mut includes = Self::default();
        for token in raw.unwrap_or_default().split(',') {
            match token.trim() {
                "" => {}
                "attachments" => includes.attachments = true,
                other => {
                    return Err(DomainError::validation_field(
                        "include",
                        format!("unknown expansion '{other}'; expected attachments"),
                    ));
                }
            }
        }
        Ok(includes)
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/devices` — List all tracked devices.
pub async fn list_devices(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListDevicesQuery>,
) -> Response {
    let includes = match DeviceListIncludes::parse(query.include.as_deref()) {
        Ok(includes) => includes,
        Err(error) => return error.into_response(),
    };
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return DomainError::validation("limit must be between 1 and 200").into_response();
    }
    let offset = query.offset.unwrap_or(0);

    let devices = state.device_registry.list().await;
    let status_filter = match parse_status_filter(query.status.as_deref()) {
        Ok(filter) => filter,
        Err(error) => return DomainError::validation(error).into_response(),
    };
    let backend_filter = query
        .backend_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let driver_filter = query
        .driver
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let query_filter = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);

    let filtered_devices: Vec<_> = devices
        .iter()
        .filter(|tracked| {
            status_filter
                .as_deref()
                .is_none_or(|expected| tracked.state.variant_name().eq_ignore_ascii_case(expected))
        })
        .filter(|tracked| {
            backend_filter.as_deref().is_none_or(|expected| {
                tracked.info.output_backend_id().to_ascii_lowercase() == *expected
            })
        })
        .filter(|tracked| {
            driver_filter
                .as_deref()
                .is_none_or(|expected| tracked.info.driver_id().to_ascii_lowercase() == *expected)
        })
        .filter(|tracked| {
            query_filter.as_deref().is_none_or(|needle| {
                let name = tracked.info.name.to_ascii_lowercase();
                let vendor = tracked.info.vendor.to_ascii_lowercase();
                name.contains(needle) || vendor.contains(needle)
            })
        })
        .collect();
    let mut items: Vec<(DeviceSummary, DeviceInfo)> = Vec::with_capacity(filtered_devices.len());
    for tracked in filtered_devices {
        let layout_device_id = ensure_default_logical_entry(&state, &tracked.info).await;
        let metadata = state
            .device_registry
            .metadata_for_id(&tracked.info.id)
            .await;
        let summary = match summarize_device_for_response(
            &state,
            &tracked.info,
            &tracked.state,
            tracked.user_settings.brightness,
            layout_device_id,
            metadata.as_ref(),
        )
        .await
        {
            Ok(summary) => summary,
            Err(error) => {
                warn!(
                    error = %error,
                    device_id = %tracked.info.id,
                    driver_id = %tracked.info.driver_id(),
                    "failed to summarize device pairing state"
                );
                return DomainError::Internal(anyhow::Error::new(error)).into_response();
            }
        };
        items.push((summary, tracked.info.clone()));
    }
    items.sort_by_cached_key(|(summary, _)| summary.name.to_lowercase());

    let total = items.len();
    let mut paged_items: Vec<(DeviceSummary, DeviceInfo)> =
        items.into_iter().skip(offset).take(limit).collect();
    if includes.attachments {
        let profiles = state.attachment_profiles.read().await;
        let registry = state.attachment_registry.read().await;
        for (summary, info) in &mut paged_items {
            let profile = profiles.get_or_default(info);
            summary.attachments = Some(attachments::summarize_attachment_profile(
                info, profile, &registry,
            ));
        }
    }
    let has_more = offset.saturating_add(limit) < total;
    envelope::ok(DeviceListResponse {
        items: paged_items
            .into_iter()
            .map(|(summary, _)| summary)
            .collect(),
        total: u64::try_from(total).expect("device count fits in u64"),
        page: Some(hypercolor_types::api::PageInfo {
            offset: u64::try_from(offset).expect("device offset fits in u64"),
            limit: u64::try_from(limit).expect("device limit fits in u64"),
            has_more,
        }),
    })
}

/// `GET /api/v1/devices/{id}` — Get a single device.
pub async fn get_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    let layout_device_id = ensure_default_logical_entry(&state, &tracked.info).await;
    let metadata = state
        .device_registry
        .metadata_for_id(&tracked.info.id)
        .await;

    match summarize_device_for_response(
        &state,
        &tracked.info,
        &tracked.state,
        tracked.user_settings.brightness,
        layout_device_id,
        metadata.as_ref(),
    )
    .await
    {
        Ok(summary) => envelope::ok(summary),
        Err(error) => DomainError::Internal(anyhow::Error::new(error)).into_response(),
    }
}

/// `PUT /api/v1/devices/{id}` — Update a device's metadata.
pub async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateDeviceRequest>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    if body.name.is_none() && body.enabled.is_none() && body.brightness.is_none() {
        return DomainError::validation(
            "At least one field must be provided: name, enabled, or brightness",
        )
        .into_response();
    }

    let normalized_name = match body.name {
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                return DomainError::validation("Device name must not be empty").into_response();
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };
    let normalized_brightness = match body.brightness {
        Some(brightness) if brightness <= 100 => Some(percent_to_brightness(brightness)),
        Some(_) => {
            return DomainError::validation("Device brightness must be between 0 and 100")
                .into_response();
        }
        None => None,
    };

    let enabled_handled_by_lifecycle = if let Some(enabled) = body.enabled {
        let runtime = super::discovery_runtime(&state);
        match core_discovery::apply_user_enabled_state(&runtime, device_id, enabled).await {
            Ok(core_discovery::UserEnabledStateResult::Applied) => true,
            Ok(core_discovery::UserEnabledStateResult::MissingLifecycle) => false,
            Err(error) => {
                return DomainError::Internal(anyhow::anyhow!(
                    "Failed to update device state for {id}: {error}"
                ))
                .into_response();
            }
        }
    } else {
        false
    };

    let Some(mut updated) = state
        .device_registry
        .update_user_settings(
            &device_id,
            normalized_name,
            body.enabled,
            normalized_brightness,
        )
        .await
    else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    if !enabled_handled_by_lifecycle && let Some(enabled) = body.enabled {
        let fallback_state = if enabled {
            DeviceState::Known
        } else {
            DeviceState::Disabled
        };
        let _ = state
            .device_registry
            .set_state(&device_id, fallback_state)
            .await;
        if let Some(tracked) = state.device_registry.get(&device_id).await {
            updated = tracked;
        }
    }

    if let Err(error) = persist_device_settings_for(&state, device_id, &updated.user_settings).await
    {
        return DomainError::Internal(anyhow::anyhow!(
            "Failed to persist device settings: {error}"
        ))
        .into_response();
    }
    sync_device_output_brightness(&state, device_id, &updated.user_settings).await;
    publish_device_settings_changed(&state, device_id, &updated.user_settings);
    if body.enabled == Some(true) {
        activate_reenabled_layout_device(&state, device_id, &updated.info).await;
        if let Some(tracked) = state.device_registry.get(&device_id).await {
            updated = tracked;
        }
    }

    let layout_device_id = ensure_default_logical_entry(&state, &updated.info).await;
    let metadata = state
        .device_registry
        .metadata_for_id(&updated.info.id)
        .await;

    match summarize_device_for_response(
        &state,
        &updated.info,
        &updated.state,
        updated.user_settings.brightness,
        layout_device_id,
        metadata.as_ref(),
    )
    .await
    {
        Ok(summary) => envelope::ok(summary),
        Err(error) => DomainError::Internal(anyhow::Error::new(error)).into_response(),
    }
}

/// `DELETE /api/v1/devices/{id}` — Remove a device from tracking.
pub async fn delete_device(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };
    let driver_id = tracked.info.driver_id().to_owned();
    let removed = if let Some(driver) = state.driver_registry().get(&driver_id)
        && let Some(provider) = driver.runtime_cache()
    {
        let inventory = state.driver_host().driver_inventory();
        let guard = inventory.operation_guard().await;
        let device = DriverTrackedDevice {
            info: tracked.info.clone(),
            metadata: state
                .device_registry
                .metadata_for_id(&device_id)
                .await
                .unwrap_or_default(),
            fingerprint: state.device_registry.fingerprint_for_id(&device_id).await,
            current_state: tracked.state,
        };
        if let Err(error) = inventory.update_driver_guarded(&guard, &driver_id, |current| {
            provider.forget_device(current, &device)
        }) {
            return DomainError::Internal(anyhow::anyhow!(
                "Failed to forget {driver_id} discovery inventory: {error}"
            ))
            .into_response();
        }
        let removed = state.device_registry.remove(&device_id).await;
        drop(guard);
        removed
    } else {
        state.device_registry.remove(&device_id).await
    };

    if removed.is_none() {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    }
    crate::api::prune_scene_display_zones_for_device(&state, device_id).await;

    envelope::ok(DeleteDeviceResponse {
        id: device_id.to_string(),
        removed: true,
    })
}

/// `POST /api/v1/devices/{id}/identify` — Flash identification pattern.
pub async fn identify_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<IdentifyRequest>>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    let duration_ms = body.as_ref().and_then(|b| b.duration_ms).unwrap_or(3000);
    if duration_ms == 0 || duration_ms > 120_000 {
        return DomainError::validation("duration_ms must be between 1 and 120000").into_response();
    }
    let requested_color = match body.as_ref().and_then(|b| b.color.as_deref()) {
        Some(color) => match Rgb::from_hex(color.trim()) {
            Ok(color) => Some(color),
            Err(_) => {
                return DomainError::validation("color must be a hex value (RRGGBB or RGB)")
                    .into_response();
            }
        },
        None => None,
    };
    let color = requested_color.map(identify_color_echo);
    let identify_rgb = requested_color.map_or(DEFAULT_IDENTIFY_COLOR_RGB, identify_color_channels);
    let identify_brightness = (state.output_power.snapshot().effective_brightness()
        * tracked.user_settings.brightness)
        .clamp(0.0, 1.0);
    let identify_color = scale_rgb(identify_rgb, identify_brightness);
    let led_count = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if led_count == 0 {
        return DomainError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
        ))
        .into_response();
    }

    let backend_id = resolved_backend_id(&tracked.info);
    sync_identify_usb_protocol_config(state.as_ref(), device_id, &tracked.info).await;
    let device_metadata = state.device_registry.metadata_for_id(&device_id).await;
    let connection = device_connection_summary(&tracked.info, device_metadata.as_ref());
    let on_frame = vec![identify_color; led_count];
    let (direct_backend, disconnect_after_identify, direct_control) =
        match prepare_identify_backend(&state, device_id, &tracked.info, tracked.state, &backend_id)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => return error.into_response(),
        };
    debug!(
        backend_id = %backend_id,
        device_id = %device_id,
        led_count,
        color = ?identify_rgb,
        effective_brightness = identify_brightness,
        connection_transport = %connection.transport,
        connection_endpoint = ?connection.endpoint,
        disconnect_after_identify,
        "identify enabling direct control and issuing initial on-frame"
    );

    if let Err(error) = start_identify_output(
        &state,
        &direct_backend,
        device_id,
        &on_frame,
        &tracked.info.name,
    )
    .await
    {
        drop(direct_control);
        if disconnect_after_identify {
            let _ = direct_backend.disconnect(device_id).await;
        }
        return error.into_response();
    }

    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        backend = %backend_id,
        led_count,
        duration_ms,
        color = ?identify_rgb,
        effective_brightness = identify_brightness,
        connection_transport = %connection.transport,
        connection_endpoint = ?connection.endpoint,
        "Identify flash started"
    );
    tokio::spawn(run_identify_flash(
        Arc::clone(&state),
        direct_backend,
        backend_id,
        device_id,
        on_frame,
        Duration::from_millis(duration_ms),
        disconnect_after_identify,
        direct_control,
    ));

    envelope::ok(IdentifyDeviceResponse {
        device_id: device_id.to_string(),
        identifying: true,
        duration_ms,
        color,
    })
}

/// `POST /api/v1/devices/{id}/segments/{segment}/identify` — Flash one segment.
#[allow(
    clippy::too_many_lines,
    reason = "the handler intentionally keeps validation, direct-control orchestration, and response shaping together"
)]
pub async fn identify_segment(
    State(state): State<Arc<AppState>>,
    Path((id, segment)): Path<(String, String)>,
    body: Option<Json<IdentifyRequest>>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    let segment_index = match resolve_segment_index(&tracked.info, &segment) {
        Ok(index) => index,
        Err(error) => return error.into_response(),
    };

    let total_leds = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if total_leds == 0 {
        return DomainError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
        ))
        .into_response();
    }

    let duration_ms = body.as_ref().and_then(|b| b.duration_ms).unwrap_or(3000);
    if duration_ms == 0 || duration_ms > 120_000 {
        return DomainError::validation("duration_ms must be between 1 and 120000").into_response();
    }
    let requested_color = match body.as_ref().and_then(|b| b.color.as_deref()) {
        Some(color) => match Rgb::from_hex(color.trim()) {
            Ok(color) => Some(color),
            Err(_) => {
                return DomainError::validation("color must be a hex value (RRGGBB or RGB)")
                    .into_response();
            }
        },
        None => None,
    };
    let color = requested_color.map(identify_color_echo);
    let identify_rgb = requested_color.map_or(DEFAULT_IDENTIFY_COLOR_RGB, identify_color_channels);
    let identify_brightness = (state.output_power.snapshot().effective_brightness()
        * tracked.user_settings.brightness)
        .clamp(0.0, 1.0);
    let identify_color = scale_rgb(identify_rgb, identify_brightness);

    let on_frame = build_segment_identify_frame(&tracked.info, segment_index, identify_color);

    let backend_id = resolved_backend_id(&tracked.info);
    sync_identify_usb_protocol_config(state.as_ref(), device_id, &tracked.info).await;
    let (direct_backend, disconnect_after_identify, direct_control) =
        match prepare_identify_backend(&state, device_id, &tracked.info, tracked.state, &backend_id)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => return error.into_response(),
        };

    if let Err(error) = start_identify_output(
        &state,
        &direct_backend,
        device_id,
        &on_frame,
        &tracked.info.name,
    )
    .await
    {
        drop(direct_control);
        if disconnect_after_identify {
            let _ = direct_backend.disconnect(device_id).await;
        }
        return error.into_response();
    }

    let segment_name = tracked.info.segments[segment_index].name.clone();
    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        segment = %segment_name,
        segment_index,
        backend = %backend_id,
        duration_ms,
        color = ?identify_rgb,
        "Segment identify flash started"
    );
    tokio::spawn(run_identify_flash(
        Arc::clone(&state),
        direct_backend,
        backend_id,
        device_id,
        on_frame,
        Duration::from_millis(duration_ms),
        disconnect_after_identify,
        direct_control,
    ));

    envelope::ok(IdentifySegmentResponse {
        device_id: device_id.to_string(),
        segment,
        segment_name,
        identifying: true,
        duration_ms,
        color,
    })
}

/// `POST /api/v1/devices/{id}/attachments/{slot}/identify` — Flash a single
/// attachment component within a slot.
#[allow(
    clippy::too_many_lines,
    reason = "the handler intentionally keeps validation, direct-control orchestration, and response shaping together"
)]
pub async fn identify_attachment(
    State(state): State<Arc<AppState>>,
    Path((id, slot_id)): Path<(String, String)>,
    body: Option<Json<IdentifyAttachmentRequest>>,
) -> Response {
    let device_id = match resolve_device_id_or_error(&state, &id).await {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };

    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return DomainError::not_found(ResourceKind::Device, &id).into_response();
    };

    let total_leds = usize::try_from(tracked.info.total_led_count()).unwrap_or_default();
    if total_leds == 0 {
        return DomainError::conflict(format!(
            "Device has no LEDs to identify: {}",
            tracked.info.name
        ))
        .into_response();
    }

    let duration_ms = body
        .as_ref()
        .and_then(|b| b.base.duration_ms)
        .unwrap_or(3000);
    if duration_ms == 0 || duration_ms > 120_000 {
        return DomainError::validation("duration_ms must be between 1 and 120000").into_response();
    }
    let requested_color = match body.as_ref().and_then(|b| b.base.color.as_deref()) {
        Some(color) => match Rgb::from_hex(color.trim()) {
            Ok(color) => Some(color),
            Err(_) => {
                return DomainError::validation("color must be a hex value (RRGGBB or RGB)")
                    .into_response();
            }
        },
        None => None,
    };
    let color = requested_color.map(identify_color_echo);
    let identify_rgb = requested_color.map_or(DEFAULT_IDENTIFY_COLOR_RGB, identify_color_channels);
    let identify_brightness = (state.output_power.snapshot().effective_brightness()
        * tracked.user_settings.brightness)
        .clamp(0.0, 1.0);
    let identify_color = scale_rgb(identify_rgb, identify_brightness);

    let binding_index = body.as_ref().and_then(|b| b.binding_index).unwrap_or(0);
    let instance = body.as_ref().and_then(|b| b.instance);

    let on_frame = {
        let profiles = state.attachment_profiles.read().await;
        let registry = state.attachment_registry.read().await;
        match build_attachment_identify_frame(
            &profiles,
            &registry,
            ComponentIdentifyTarget {
                binding_index,
                device_id,
                instance,
                slot_id: &slot_id,
            },
            total_leds,
            identify_color,
        ) {
            Ok(frame) => frame,
            Err(error) => return error.into_response(),
        }
    };

    let backend_id = resolved_backend_id(&tracked.info);
    sync_identify_usb_protocol_config(state.as_ref(), device_id, &tracked.info).await;
    let (direct_backend, disconnect_after_identify, direct_control) =
        match prepare_identify_backend(&state, device_id, &tracked.info, tracked.state, &backend_id)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => return error.into_response(),
        };

    if let Err(error) = start_identify_output(
        &state,
        &direct_backend,
        device_id,
        &on_frame,
        &tracked.info.name,
    )
    .await
    {
        drop(direct_control);
        if disconnect_after_identify {
            let _ = direct_backend.disconnect(device_id).await;
        }
        return error.into_response();
    }

    tracing::info!(
        device_id = %device_id,
        device = %tracked.info.name,
        slot_id = %slot_id,
        binding_index,
        instance,
        backend = %backend_id,
        duration_ms,
        color = ?identify_rgb,
        "Attachment identify flash started"
    );
    tokio::spawn(run_identify_flash(
        Arc::clone(&state),
        direct_backend,
        backend_id,
        device_id,
        on_frame,
        Duration::from_millis(duration_ms),
        disconnect_after_identify,
        direct_control,
    ));

    envelope::ok(IdentifyAttachmentResponse {
        device_id: device_id.to_string(),
        slot_id,
        binding_index,
        instance,
        identifying: true,
        duration_ms,
        color,
    })
}

// ── Shared helpers ───────────────────────────────────────────────────────

async fn sync_identify_usb_protocol_config(
    state: &AppState,
    device_id: DeviceId,
    info: &DeviceInfo,
) {
    let profile = {
        let profiles = state.attachment_profiles.read().await;
        profiles.get(&info.id.to_string()).cloned()
    };

    let Some(profile) = profile else {
        state.usb_protocol_configs.remove_device(device_id).await;
        return;
    };

    let registry = state.attachment_registry.read().await;
    let applied = state
        .usb_protocol_configs
        .apply_attachment_profile(device_id, info, &profile, &registry)
        .await;
    if applied {
        debug!(
            device_id = %device_id,
            device = %info.name,
            "refreshed USB protocol attachment config before identify"
        );
    } else {
        state.usb_protocol_configs.remove_device(device_id).await;
    }
}

pub(super) async fn ensure_default_logical_entry(
    state: &AppState,
    device_info: &DeviceInfo,
) -> String {
    let fallback_layout_id = state
        .domains
        .layout
        .resolved_layout_device_id(&state.domains.devices.layout_runtime(), device_info)
        .await;

    let mut store = state.logical_devices.write().await;
    let default = crate::logical_devices::ensure_default_logical_device(
        &mut store,
        device_info.id,
        &fallback_layout_id,
        &device_info.name,
        device_info.total_led_count(),
    );
    default.id
}

pub(super) async fn summarize_device_for_response(
    state: &AppState,
    info: &DeviceInfo,
    device_state: &DeviceState,
    brightness: f32,
    layout_device_id: String,
    metadata: Option<&HashMap<String, String>>,
) -> Result<DeviceSummary, hypercolor_driver_api::DriverError> {
    Ok(DeviceSummary {
        id: info.id.to_string(),
        layout_device_id,
        name: info.name.clone(),
        origin: info.origin.clone(),
        presentation: crate::network::device_presentation(state.driver_registry().as_ref(), info),
        status: device_state.variant_name().to_lowercase(),
        brightness: brightness_percent(brightness),
        firmware_version: info.firmware_version.clone(),
        connection: device_connection_summary(info, metadata),
        total_leds: info.total_led_count(),
        auth: pairing::build_device_auth_summary(state, info, device_state, metadata).await?,
        segments: info
            .segments
            .iter()
            .enumerate()
            .map(|(i, z)| SegmentSummary {
                id: format!("segment_{i}"),
                name: z.name.clone(),
                led_count: z.led_count,
                topology: format!("{:?}", z.topology).to_lowercase(),
                topology_hint: Some(summarize_segment_topology(&z.topology)),
            })
            .collect(),
        attachments: None,
    })
}

pub(super) async fn refreshed_device_summary(
    state: &AppState,
    device_id: DeviceId,
) -> Result<Option<DeviceSummary>, hypercolor_driver_api::DriverError> {
    let Some(tracked) = state.device_registry.get(&device_id).await else {
        return Ok(None);
    };
    let layout_device_id = ensure_default_logical_entry(state, &tracked.info).await;
    let metadata = state.device_registry.metadata_for_id(&device_id).await;

    Ok(Some(
        summarize_device_for_response(
            state,
            &tracked.info,
            &tracked.state,
            tracked.user_settings.brightness,
            layout_device_id,
            metadata.as_ref(),
        )
        .await?,
    ))
}

fn device_connection_summary(
    info: &DeviceInfo,
    metadata: Option<&HashMap<String, String>>,
) -> DeviceConnectionSummary {
    let ip = metadata_value(metadata, "ip").map(str::to_owned);
    let hostname = metadata_value(metadata, "hostname").map(str::to_owned);
    let label = device_connection_label(&info.origin.transport, metadata);
    let endpoint = hostname
        .clone()
        .or_else(|| ip.clone())
        .or_else(|| label.clone());

    DeviceConnectionSummary {
        transport: info.origin.transport.as_id().to_owned(),
        label,
        endpoint,
        ip,
        hostname,
    }
}

fn device_connection_label(
    transport: &DriverTransportKind,
    metadata: Option<&HashMap<String, String>>,
) -> Option<String> {
    match transport {
        DriverTransportKind::Usb => metadata_value(metadata, "serial")
            .map(str::to_owned)
            .or_else(|| metadata_value(metadata, "usb_path").map(|path| format!("USB {path}"))),
        DriverTransportKind::Smbus => match (
            metadata_value(metadata, "bus_path"),
            metadata_value(metadata, "smbus_address"),
        ) {
            (Some(bus_path), Some(address)) => Some(format!("{bus_path} {address}")),
            (None, Some(address)) => Some(format!("SMBus {address}")),
            (Some(bus_path), None) => Some(bus_path.to_owned()),
            (None, None) => metadata_value(metadata, "serial").map(str::to_owned),
        },
        _ => metadata_value(metadata, "serial").map(str::to_owned),
    }
}

fn metadata_value<'a>(metadata: Option<&'a HashMap<String, String>>, key: &str) -> Option<&'a str> {
    metadata
        .and_then(|values| values.get(key))
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn percent_to_brightness(percent: u8) -> f32 {
    (f32::from(percent) / 100.0).clamp(0.0, 1.0)
}

fn scale_rgb(color: [u8; 3], brightness: f32) -> [u8; 3] {
    let scaled = Rgb::new(color[0], color[1], color[2]).scale(brightness);
    [scaled.r, scaled.g, scaled.b]
}

pub(super) async fn device_settings_key(state: &AppState, device_id: DeviceId) -> String {
    crate::device_settings::resolve_device_settings_key(
        &state.device_registry,
        &state.device_settings,
        device_id,
    )
    .await
}

pub(crate) async fn persist_device_settings_for(
    state: &AppState,
    device_id: DeviceId,
    settings: &DeviceUserSettings,
) -> Result<(), String> {
    let key = device_settings_key(state, device_id).await;
    state
        .device_settings
        .persist_device_settings(
            &key,
            crate::device_settings::StoredDeviceSettings {
                name: settings.name.clone(),
                disabled: !settings.enabled,
                brightness: settings.brightness,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    state
        .event_bus
        .publish(HypercolorEvent::DeviceSettingsChanged { key: Some(key) });
    Ok(())
}

pub(crate) async fn sync_device_output_brightness(
    state: &AppState,
    device_id: DeviceId,
    settings: &DeviceUserSettings,
) {
    let mut manager = state.backend_manager.lock().await;
    manager.set_device_output_brightness(device_id, settings.brightness);
}

pub(crate) fn publish_device_settings_changed(
    state: &AppState,
    device_id: DeviceId,
    settings: &DeviceUserSettings,
) {
    let mut changes = HashMap::new();
    changes.insert(
        "name".to_owned(),
        settings
            .name
            .as_ref()
            .map_or(serde_json::Value::Null, |name| {
                serde_json::Value::String(name.clone())
            }),
    );
    changes.insert(
        "enabled".to_owned(),
        serde_json::Value::Bool(settings.enabled),
    );
    changes.insert(
        "brightness".to_owned(),
        serde_json::Value::from(brightness_percent(settings.brightness)),
    );
    state
        .event_bus
        .publish(HypercolorEvent::DeviceStateChanged {
            device_id: device_id.to_string(),
            changes,
        });
}

pub(crate) async fn activate_reenabled_layout_device(
    state: &AppState,
    device_id: DeviceId,
    info: &DeviceInfo,
) {
    let runtime = super::discovery_runtime(state);
    let backend_id = resolved_backend_id(info);
    match core_discovery::activate_pairable_device(&runtime, device_id, &backend_id).await {
        Ok(_) => {}
        Err(error) => {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                device = %info.name,
                error = %error,
                "failed to activate re-enabled device for active layout"
            );
        }
    }
}

fn summarize_segment_topology(topology: &DeviceTopologyHint) -> SegmentTopologySummary {
    match topology {
        DeviceTopologyHint::Strip => SegmentTopologySummary::Strip,
        DeviceTopologyHint::Matrix { rows, cols } => SegmentTopologySummary::Matrix {
            rows: *rows,
            cols: *cols,
        },
        DeviceTopologyHint::Ring { count } => SegmentTopologySummary::Ring { count: *count },
        DeviceTopologyHint::Point => SegmentTopologySummary::Point,
        DeviceTopologyHint::Display {
            width,
            height,
            circular,
            ..
        } => SegmentTopologySummary::Display {
            width: *width,
            height: *height,
            circular: *circular,
        },
        DeviceTopologyHint::Custom => SegmentTopologySummary::Custom,
    }
}

async fn resolve_device_id(
    state: &AppState,
    id_or_name: &str,
) -> Result<Option<DeviceId>, ResolveDeviceError> {
    if let Ok(id) = id_or_name.parse::<DeviceId>() {
        return Ok(Some(id));
    }

    let devices = state.device_registry.list().await;
    let matches: Vec<DeviceId> = devices
        .iter()
        .filter(|d| d.info.name.eq_ignore_ascii_case(id_or_name))
        .map(|d| d.info.id)
        .collect();

    if matches.len() > 1 {
        return Err(ResolveDeviceError::AmbiguousName(id_or_name.to_owned()));
    }
    Ok(matches.first().copied())
}

pub(super) async fn resolve_device_id_or_error(
    state: &AppState,
    id_or_name: &str,
) -> Result<DeviceId, DomainError> {
    match resolve_device_id(state, id_or_name).await {
        Ok(Some(id)) => Ok(id),
        Ok(None) => Err(DomainError::not_found(ResourceKind::Device, id_or_name)),
        Err(ResolveDeviceError::AmbiguousName(name)) => Err(DomainError::conflict(format!(
            "Device name is ambiguous: {name}"
        ))),
    }
}

pub(super) fn resolved_backend_id(info: &DeviceInfo) -> String {
    info.output_backend_id().to_owned()
}

fn parse_status_filter(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }

    match normalized.as_str() {
        "known" | "connected" | "active" | "reconnecting" | "disabled" => Ok(Some(normalized)),
        _ => Err(format!(
            "Invalid status filter '{raw}'. Expected one of: known, connected, active, reconnecting, disabled"
        )),
    }
}

async fn run_identify_flash(
    state: Arc<AppState>,
    direct_backend: BackendIo,
    backend_id: String,
    device_id: DeviceId,
    on_frame: Vec<[u8; 3]>,
    duration: Duration,
    disconnect_after_identify: bool,
    direct_control: DirectControlGuard,
) {
    if on_frame.is_empty() {
        return;
    }

    let off_frame = vec![[0, 0, 0]; on_frame.len()];
    let started_at = Instant::now();
    let mut show_on = false;
    let mut identify_failed = false;
    let mut phase_index = 0_u32;
    let mut power_state = state.output_power.subscribe();

    loop {
        if power_state.borrow().sleeping() {
            break;
        }
        if started_at.elapsed() >= duration {
            break;
        }

        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(IDENTIFY_FLASH_INTERVAL_MS)) => {}
            changed = power_state.changed() => {
                if changed.is_err() || power_state.borrow().sleeping() {
                    break;
                }
                continue;
            }
        }

        let frame = if show_on { &on_frame } else { &off_frame };
        let phase = if show_on { "on" } else { "off" };
        phase_index = phase_index.saturating_add(1);
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            phase_index,
            phase,
            elapsed_ms = started_at.elapsed().as_millis(),
            frame_leds = frame.len(),
            "identify issuing flash phase"
        );
        let result =
            write_identify_output_if_running(&state, &direct_backend, device_id, frame).await;

        match result {
            Ok(true) => {}
            Ok(false) => {
                break;
            }
            Err(error) => {
                warn!(
                    backend_id = %backend_id,
                    device_id = %device_id,
                    error = %error,
                    "identify write failed"
                );
                identify_failed = true;
                break;
            }
        }

        show_on = !show_on;
    }

    let output_power = state.output_power.transition().await;
    if !identify_failed {
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            elapsed_ms = started_at.elapsed().as_millis(),
            "identify issuing final clear frame"
        );
        let clear_result = direct_backend.write_colors(device_id, &off_frame).await;
        if let Err(error) = clear_result {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                error = %error,
                "identify clear write failed"
            );
        }
    }

    drop(direct_control);

    let power = output_power.snapshot();
    if power.sleeping() {
        state
            .domains
            .output
            .publish_static_snapshot(power.effective_off_output_color())
            .await;
    }
    debug!(
        backend_id = %backend_id,
        device_id = %device_id,
        elapsed_ms = started_at.elapsed().as_millis(),
        identify_failed,
        "identify released direct control"
    );

    if disconnect_after_identify {
        if let Err(error) = direct_backend.disconnect(device_id).await {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                error = %error,
                "identify temporary disconnect failed"
            );
        } else {
            debug!(
                backend_id = %backend_id,
                device_id = %device_id,
                "identify released temporary backend connection"
            );
        }
    }

    if identify_failed {
        return;
    }

    tracing::info!(
        device_id = %device_id,
        backend = %backend_id,
        "Identify flash completed"
    );
}

async fn start_identify_output(
    state: &AppState,
    direct_backend: &BackendIo,
    device_id: DeviceId,
    colors: &[[u8; 3]],
    device_name: &str,
) -> Result<(), DomainError> {
    match write_identify_output_if_running(state, direct_backend, device_id, colors).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(DomainError::conflict(format!(
            "Cannot identify {device_name} while global output is paused"
        ))),
        Err(error) => {
            warn!(
                device_id = %device_id,
                error = %error,
                "identify initial write failed"
            );
            Err(DomainError::Internal(anyhow::anyhow!(
                "Failed to start identify flash for {device_name}: {error}"
            )))
        }
    }
}

async fn write_identify_output_if_running(
    state: &AppState,
    direct_backend: &BackendIo,
    device_id: DeviceId,
    colors: &[[u8; 3]],
) -> anyhow::Result<bool> {
    let output_power = state.output_power.transition().await;
    if output_power.snapshot().sleeping() {
        return Ok(false);
    }
    direct_backend.write_colors(device_id, colors).await?;
    Ok(true)
}

async fn prepare_identify_backend(
    state: &Arc<AppState>,
    device_id: DeviceId,
    info: &DeviceInfo,
    device_state: DeviceState,
    backend_id: &str,
) -> Result<(BackendIo, bool, DirectControlGuard), DomainError> {
    let manager = Arc::clone(&state.backend_manager);
    let direct_backend = {
        let manager = manager.lock().await;
        let Some(direct_backend) = manager.backend_io(backend_id) else {
            if !device_state.is_renderable() {
                return Err(DomainError::conflict(format!(
                    "Device is not connected: {} (state={device_state})",
                    info.name
                )));
            }
            return Err(DomainError::Internal(anyhow::anyhow!(
                "Failed to start identify flash for {}: backend '{backend_id}' is not registered",
                info.name
            )));
        };
        direct_backend
    };
    let supports_temporary_identify = device_state != DeviceState::Disabled
        && direct_backend.supports_temporary_direct_control(info).await;
    if !device_state.is_renderable() && !supports_temporary_identify {
        warn!(
            backend_id = %backend_id,
            device_id = %device_id,
            device = %info.name,
            device_state = %device_state,
            supports_direct = info.capabilities.supports_direct,
            led_count = info.total_led_count(),
            "identify requested for non-renderable device but backend cannot temporarily connect it"
        );
        return Err(DomainError::conflict(format!(
            "Device is not connected: {} (state={device_state})",
            info.name
        )));
    }

    let disconnect_after_identify = if device_state.is_renderable() {
        false
    } else if supports_temporary_identify {
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            device = %info.name,
            device_state = %device_state,
            "temporarily connecting device for identify"
        );
        if let Err(error) = direct_backend.connect(device_id).await {
            warn!(
                backend_id = %backend_id,
                device_id = %device_id,
                device = %info.name,
                error = %error,
                "temporary identify connect failed"
            );
            return Err(DomainError::conflict(format!(
                "Device is not connected and temporary identify failed for {}: {error}",
                info.name
            )));
        }
        if let Ok(Some(refreshed_info)) = direct_backend.connected_device_info(device_id).await {
            let _ = state
                .device_registry
                .update_info(&device_id, refreshed_info)
                .await;
        }
        debug!(
            backend_id = %backend_id,
            device_id = %device_id,
            device = %info.name,
            "temporary identify connect succeeded"
        );
        true
    } else {
        return Err(DomainError::conflict(format!(
            "Device is not connected: {} (state={device_state})",
            info.name
        )));
    };

    let direct_control = manager
        .lock()
        .await
        .begin_direct_control(backend_id, device_id);

    Ok((direct_backend, disconnect_after_identify, direct_control))
}

/// The identify responses echo the requested color back as uppercase
/// `#RRGGBB`, whatever casing or shorthand the request used.
fn identify_color_echo(color: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

fn identify_color_channels(color: Rgb) -> [u8; 3] {
    [color.r, color.g, color.b]
}

// ── Identify helpers ─────────────────────────────────────────────────────

/// Resolve a segment specifier (`"segment_0"`, `"0"`, or name) to an index.
fn resolve_segment_index(info: &DeviceInfo, segment_id: &str) -> Result<usize, DomainError> {
    if let Some(stripped) = segment_id.strip_prefix("segment_")
        && let Ok(index) = stripped.parse::<usize>()
        && index < info.segments.len()
    {
        return Ok(index);
    }

    if let Ok(index) = segment_id.parse::<usize>()
        && index < info.segments.len()
    {
        return Ok(index);
    }

    let needle = segment_id.to_ascii_lowercase();
    for (index, segment) in info.segments.iter().enumerate() {
        if segment.name.to_ascii_lowercase() == needle {
            return Ok(index);
        }
    }

    Err(DomainError::not_found(ResourceKind::Zone, segment_id))
}

/// Build a full-device LED frame with only one segment lit.
fn build_segment_identify_frame(
    info: &DeviceInfo,
    segment_index: usize,
    color: [u8; 3],
) -> Vec<[u8; 3]> {
    let total_leds = usize::try_from(info.total_led_count()).unwrap_or_default();
    let mut frame = vec![[0_u8; 3]; total_leds];

    let mut offset = 0_usize;
    for (index, segment) in info.segments.iter().enumerate() {
        let count = usize::try_from(segment.led_count).unwrap_or_default();
        if index == segment_index {
            for led in &mut frame[offset..offset + count] {
                *led = color;
            }
        }
        offset += count;
    }

    frame
}

/// Build a full-device LED frame with only a single attachment component lit.
#[derive(Clone, Copy)]
struct ComponentIdentifyTarget<'a> {
    device_id: DeviceId,
    slot_id: &'a str,
    binding_index: usize,
    instance: Option<u32>,
}

fn build_attachment_identify_frame(
    profiles: &crate::attachment_profiles::ComponentProfileStore,
    registry: &hypercolor_core::attachment::ComponentRegistry,
    target: ComponentIdentifyTarget<'_>,
    total_leds: usize,
    color: [u8; 3],
) -> Result<Vec<[u8; 3]>, DomainError> {
    let ComponentIdentifyTarget {
        device_id,
        slot_id,
        binding_index,
        instance,
    } = target;
    let device_key = device_id.to_string();
    let profile = profiles
        .get(&device_key)
        .ok_or_else(|| DomainError::not_found(ResourceKind::AttachmentProfile, device_id))?;

    let slot = profile
        .slots
        .iter()
        .find(|s| s.id == slot_id)
        .ok_or_else(|| DomainError::not_found(ResourceKind::AttachmentSlot, slot_id))?;

    let slot_bindings: Vec<(usize, &ComponentBinding)> = profile
        .bindings
        .iter()
        .enumerate()
        .filter(|(_, binding)| binding.slot_id == slot_id && binding.enabled)
        .collect();

    if slot_bindings.is_empty() {
        return Err(DomainError::validation_field(
            "slot_id",
            format!("No enabled bindings in slot '{slot_id}'"),
        ));
    }
    let (start, led_count) = if let Some(instance_index) = instance {
        resolve_attachment_instance_range(
            registry,
            slot_bindings.as_slice(),
            slot,
            binding_index,
            instance_index,
        )?
    } else {
        resolve_attachment_component_range(registry, slot_bindings.as_slice(), slot, binding_index)?
    };
    let end = (start + led_count).min(total_leds);

    let mut frame = vec![[0_u8; 3]; total_leds];
    for led in &mut frame[start..end] {
        *led = color;
    }

    Ok(frame)
}

fn resolve_attachment_instance_range(
    registry: &hypercolor_core::attachment::ComponentRegistry,
    slot_bindings: &[(usize, &ComponentBinding)],
    slot: &ComponentSlot,
    binding_index: usize,
    instance_index: u32,
) -> Result<(usize, usize), DomainError> {
    let available = slot_bindings
        .iter()
        .map(|(index, _)| index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let (_, binding) = slot_bindings
        .iter()
        .find(|(index, _)| *index == binding_index)
        .ok_or_else(|| {
            DomainError::validation_field(
                "binding_index",
                format!(
                    "Binding index {binding_index} not found in slot '{slot_id}' (available: {available})",
                    slot_id = slot.id
                ),
            )
        })?;

    let template = registry.get(&binding.template_id).ok_or_else(|| {
        DomainError::not_found(ResourceKind::AttachmentTemplate, &binding.template_id)
    })?;
    let total_instances = binding.instances.max(1);
    if instance_index >= total_instances {
        return Err(DomainError::validation_field(
            "instance",
            format!(
                "Instance {instance_index} out of range for binding {binding_index} in slot '{slot_id}' (instances: {total_instances})",
                slot_id = slot.id
            ),
        ));
    }

    let slot_start = usize::try_from(slot.led_start).unwrap_or_default();
    let binding_offset = usize::try_from(binding.led_offset).unwrap_or_default();
    let instance_stride = usize::try_from(template.led_count()).unwrap_or_default();
    let instance_offset = usize::try_from(instance_index).unwrap_or_default();

    Ok((
        slot_start + binding_offset + instance_offset.saturating_mul(instance_stride),
        instance_stride,
    ))
}

fn resolve_attachment_component_range(
    registry: &hypercolor_core::attachment::ComponentRegistry,
    slot_bindings: &[(usize, &ComponentBinding)],
    slot: &ComponentSlot,
    component_index: usize,
) -> Result<(usize, usize), DomainError> {
    let mut sorted = slot_bindings
        .iter()
        .map(|(binding_index, binding)| {
            let template = registry.get(&binding.template_id).ok_or_else(|| {
                DomainError::not_found(ResourceKind::AttachmentTemplate, &binding.template_id)
            })?;
            Ok((*binding_index, *binding, template))
        })
        .collect::<Result<Vec<_>, DomainError>>()?;
    sorted.sort_by(|left, right| {
        left.1
            .led_offset
            .cmp(&right.1.led_offset)
            .then_with(|| left.2.name.cmp(&right.2.name))
            .then_with(|| left.2.id.cmp(&right.2.id))
            .then_with(|| left.0.cmp(&right.0))
    });

    let mut remaining = component_index;
    for (_, binding, template) in sorted {
        let instances = usize::try_from(binding.instances.max(1)).unwrap_or(usize::MAX);
        let instance_stride = usize::try_from(template.led_count()).unwrap_or_default();
        if remaining < instances {
            let slot_start = usize::try_from(slot.led_start).unwrap_or_default();
            let binding_offset = usize::try_from(binding.led_offset).unwrap_or_default();
            return Ok((
                slot_start + binding_offset + remaining.saturating_mul(instance_stride),
                instance_stride,
            ));
        }
        remaining = remaining.saturating_sub(instances);
    }

    let available = slot_bindings
        .iter()
        .map(|(_, binding)| usize::try_from(binding.instances.max(1)).unwrap_or(usize::MAX))
        .fold(0_usize, usize::saturating_add);
    Err(DomainError::validation_field(
        "binding_index",
        format!(
            "Component index {component_index} out of range for slot '{slot_id}' (available components: {available})",
            slot_id = slot.id
        ),
    ))
}
