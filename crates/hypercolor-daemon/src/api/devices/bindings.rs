//! Portable identity binding endpoints.
//!
//! `GET /devices/bindings` surfaces the two halves of a re-bind decision:
//! layout bindings no attached device resolves, and attached devices no
//! layout references. `POST /devices/rebind` executes the decision by
//! re-pinning the chosen device's portable key onto the orphaned
//! binding's recorded identity, which heals the layouts without editing
//! them and holds across restarts through the alias overlay.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::Response;

use hypercolor_core::device::{DeviceLifecycleManager, PortableRebindError};
use hypercolor_types::api::devices::{
    DeviceBindingsResponse, RebindCandidateSummary, RebindDeviceRequest, RebindDeviceResponse,
    UnresolvedBindingSummary,
};
use hypercolor_types::device::{DeviceFingerprint, DeviceId, DeviceState};
use hypercolor_types::event::HypercolorEvent;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::device_aliases;

/// `GET /api/v1/devices/bindings`
pub async fn get_device_bindings(State(state): State<Arc<AppState>>) -> Response {
    let referenced = referenced_bindings(&state).await;
    let attached = attached_devices(&state).await;

    let referenced_ids: BTreeSet<String> = referenced.keys().cloned().collect();

    // A Reconnecting device is hardware that vanished: its binding is an
    // orphan to surface and it is not a re-bind target, but it IS a
    // recorded identity a replacement can inherit.
    let reconnecting = DeviceState::Reconnecting.variant_name().to_lowercase();
    let derived_ids: BTreeSet<&str> = attached
        .iter()
        .filter(|device| device.status != reconnecting)
        .map(|device| device.layout_device_id.as_str())
        .collect();
    let registry_recorded: BTreeSet<&str> = attached
        .iter()
        .filter(|device| device.status == reconnecting)
        .map(|device| device.layout_device_id.as_str())
        .collect();

    let overlay = device_aliases::load(&state.data_dir.join(device_aliases::DEVICE_ALIASES_FILE))
        .unwrap_or_default();
    let recorded_bindings: BTreeSet<&str> = overlay
        .aliases
        .values()
        .filter_map(|record| record.layout_device_id.as_deref())
        .collect();

    let unresolved = referenced
        .into_iter()
        .filter(|(layout_device_id, _)| !derived_ids.contains(layout_device_id.as_str()))
        .map(|(layout_device_id, layout_ids)| UnresolvedBindingSummary {
            rebindable: recorded_bindings.contains(layout_device_id.as_str())
                || registry_recorded.contains(layout_device_id.as_str()),
            layout_device_id,
            layout_ids: layout_ids.into_iter().collect(),
        })
        .collect();

    let candidates = attached
        .into_iter()
        .filter(|device| {
            device.status != reconnecting && !referenced_ids.contains(&device.layout_device_id)
        })
        .collect();

    ApiResponse::ok(DeviceBindingsResponse {
        unresolved,
        candidates,
    })
}

/// `POST /api/v1/devices/rebind`
pub async fn rebind_device(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RebindDeviceRequest>,
) -> Response {
    let layout_device_id = request.layout_device_id.trim();
    if layout_device_id.is_empty() {
        return ApiError::validation("layout_device_id must not be empty");
    }
    let Ok(device_id) = request.device_id.parse::<DeviceId>() else {
        return ApiError::validation("device_id must be a device UUID");
    };

    let Some(fingerprint) = recorded_fingerprint_for(&state, layout_device_id, device_id).await
    else {
        return ApiError::not_found(format!(
            "no recorded identity derives layout binding '{layout_device_id}'"
        ));
    };

    // A cross-driver inheritance would pin the key but derive a
    // different layout id (the owner prefix comes from the device), so
    // the binding would stay orphaned while the response said healed.
    // Refuse before mutating anything.
    if let Some(target) = state.device_registry.get(&device_id).await {
        let would_derive =
            DeviceLifecycleManager::canonical_layout_device_id(&target.info, Some(&fingerprint));
        if would_derive != layout_device_id {
            return ApiError::validation(format!(
                "device would derive layout binding '{would_derive}', not \
                 '{layout_device_id}'; a binding can only be inherited within its driver"
            ));
        }
    }

    let rebound = match state
        .device_registry
        .rebind_portable_identity(&device_id, fingerprint.clone())
        .await
    {
        Ok(rebound) => rebound,
        Err(PortableRebindError::UnknownDevice) => {
            return ApiError::not_found(format!("Device not found: {device_id}"));
        }
        Err(PortableRebindError::Unclaimed) => {
            return ApiError::validation(
                "device carries no portable identity claim; re-bind it by editing the layout",
            );
        }
        Err(PortableRebindError::TargetActive) => {
            return ApiError::conflict(
                "the binding's device is currently active, so it was not replaced",
            );
        }
    };

    let alias_path = state.data_dir.join(device_aliases::DEVICE_ALIASES_FILE);
    if let Err(error) =
        device_aliases::sync_from_registry(&alias_path, &state.device_registry).await
    {
        return ApiError::internal(format!(
            "re-bind applied but the alias overlay failed to persist: {error:#}"
        ));
    }

    let portable_key = state
        .device_registry
        .claim_for_id(&device_id)
        .await
        .map(|claim| claim.key().to_string())
        .unwrap_or_default();
    let resolved_layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(&rebound.info, Some(&fingerprint));

    // A live device keeps rendering through its pre-rebind layout id
    // until routing is re-pointed; the registry alone is not enough.
    let remap = {
        let mut lifecycle = state.lifecycle_manager.lock().await;
        lifecycle.rebind_layout_device_id(device_id, resolved_layout_device_id.clone())
    };
    if let Some((backend_id, previous_layout_id)) = remap
        && previous_layout_id != resolved_layout_device_id
    {
        let mut manager = state.backend_manager.lock().await;
        manager.unmap_device(&previous_layout_id);
        manager.map_device(resolved_layout_device_id.clone(), backend_id, device_id);
    }

    // Inherited settings exist only in the registry until written under
    // the replacement's own canonical key: the predecessor's persisted
    // row lives under its old portable key, which nothing derives
    // anymore, so skipping this write would lose the inherited name and
    // brightness on the next restart.
    if let Err(error) =
        super::persist_device_settings_for(&state, device_id, &rebound.user_settings).await
    {
        tracing::warn!(
            device_id = %device_id,
            error = %error,
            "Re-bind applied but the inherited settings were not persisted"
        );
    }

    state.event_bus.publish(HypercolorEvent::DeviceRebound {
        device_id: device_id.to_string(),
        layout_device_id: resolved_layout_device_id.clone(),
        portable_key: portable_key.clone(),
    });

    ApiResponse::ok(RebindDeviceResponse {
        device_id: device_id.to_string(),
        layout_device_id: resolved_layout_device_id,
        portable_key,
    })
}

/// Every layout binding id referenced by persisted layouts or the active
/// spatial layout, with the layouts that reference it.
async fn referenced_bindings(state: &AppState) -> BTreeMap<String, BTreeSet<String>> {
    let mut referenced: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut collect = |layout_id: &str, zones: &[hypercolor_types::spatial::Output]| {
        for zone in zones {
            if zone.device_id.is_empty() {
                continue;
            }
            referenced
                .entry(zone.device_id.clone())
                .or_default()
                .insert(layout_id.to_owned());
        }
    };

    {
        let layouts = state.layouts.read().await;
        for (layout_id, layout) in layouts.iter() {
            collect(layout_id, &layout.zones);
        }
    }
    {
        let spatial = state.spatial_engine.read().await;
        let layout = spatial.layout();
        collect(&layout.id, &layout.zones);
    }

    referenced
}

/// Every registered device with the layout binding id it derives.
async fn attached_devices(state: &AppState) -> Vec<RebindCandidateSummary> {
    let mut devices = Vec::new();
    for tracked in state.device_registry.list().await {
        let id = tracked.info.id;
        let fingerprint = state.device_registry.fingerprint_for_id(&id).await;
        let layout_device_id = {
            let lifecycle = state.lifecycle_manager.lock().await;
            lifecycle
                .layout_device_id_for(id)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    DeviceLifecycleManager::canonical_layout_device_id(
                        &tracked.info,
                        fingerprint.as_ref(),
                    )
                })
        };
        let portable_key = state
            .device_registry
            .claim_for_id(&id)
            .await
            .map(|claim| claim.key().to_string());

        devices.push(RebindCandidateSummary {
            device_id: id.to_string(),
            name: tracked.info.name.clone(),
            layout_device_id,
            status: tracked.state.variant_name().to_lowercase(),
            portable_key,
        });
    }
    devices
}

/// The fingerprint whose identity the orphaned binding refers to.
///
/// A still-registered predecessor (another device deriving the binding)
/// is preferred as the freshest source; the alias overlay covers the
/// cross-restart case where the predecessor is gone entirely.
async fn recorded_fingerprint_for(
    state: &AppState,
    layout_device_id: &str,
    exclude: DeviceId,
) -> Option<DeviceFingerprint> {
    for tracked in state.device_registry.list().await {
        let id = tracked.info.id;
        if id == exclude {
            continue;
        }
        let Some(fingerprint) = state.device_registry.fingerprint_for_id(&id).await else {
            continue;
        };
        let derived =
            DeviceLifecycleManager::canonical_layout_device_id(&tracked.info, Some(&fingerprint));
        if derived == layout_device_id {
            return Some(fingerprint);
        }
    }

    let overlay =
        device_aliases::load(&state.data_dir.join(device_aliases::DEVICE_ALIASES_FILE)).ok()?;
    overlay
        .aliases
        .values()
        .find(|record| record.layout_device_id.as_deref() == Some(layout_device_id))
        .map(|record| DeviceFingerprint(record.fingerprint.clone()))
}
