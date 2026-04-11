use std::collections::HashMap;

use anyhow::Context;
use hypercolor_core::device::{BackendIo, BackendManager, DeviceLifecycleManager, SegmentRange};
use hypercolor_types::device::{
    DeviceFamily, DeviceFingerprint, DeviceId, DeviceInfo, DeviceTopologyHint, DeviceUserSettings,
};
use hypercolor_types::event::{DeviceRef, HypercolorEvent, ZoneRef};
use tracing::info;

use super::DiscoveryRuntime;
use crate::device_settings::StoredDeviceSettings;
use crate::logical_devices;

pub(crate) async fn apply_persisted_device_settings(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
) -> DeviceUserSettings {
    let fallback_settings = runtime
        .device_registry
        .get(&device_id)
        .await
        .map_or_else(DeviceUserSettings::default, |tracked| tracked.user_settings);
    let key = runtime
        .device_registry
        .fingerprint_for_id(&device_id)
        .await
        .map_or_else(
            || device_id.to_string(),
            |fingerprint| fingerprint.to_string(),
        );
    let persisted_settings = {
        let store = runtime.device_settings.read().await;
        store
            .device_settings_for_key(&key)
            .map_or(fallback_settings, stored_device_settings_to_user_settings)
    };

    let _ = runtime
        .device_registry
        .replace_user_settings(&device_id, persisted_settings.clone())
        .await;

    let mut manager = runtime.backend_manager.lock().await;
    manager.set_device_output_brightness(device_id, persisted_settings.brightness);
    persisted_settings
}

fn stored_device_settings_to_user_settings(settings: StoredDeviceSettings) -> DeviceUserSettings {
    DeviceUserSettings {
        name: settings.name,
        enabled: !settings.disabled,
        brightness: settings.brightness.clamp(0.0, 1.0),
    }
}

pub(super) async fn refresh_connected_device_info(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    let maybe_info = backend_io(runtime, backend_id)
        .await?
        .connected_device_info(device_id)
        .await?;

    if let Some(info) = maybe_info {
        let _ = runtime.device_registry.update_info(&device_id, info).await;
    }

    Ok(())
}

pub(super) async fn backend_io(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
) -> anyhow::Result<BackendIo> {
    let manager = runtime.backend_manager.lock().await;
    manager
        .backend_io(backend_id)
        .with_context(|| format!("backend '{backend_id}' is not registered"))
}

pub(super) async fn apply_dynamic_usb_protocol_config(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) {
    if backend_id != "usb" {
        return;
    }

    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    };

    if tracked.info.family != DeviceFamily::PrismRgb
        || tracked.info.model.as_deref() != Some("prism_s")
    {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    }

    let prism_s_config = {
        let registry = runtime.attachment_registry.read().await;
        let profiles = runtime.attachment_profiles.read().await;
        profiles.prism_s_config_for_device(&tracked.info, &registry)
    };

    if let Some(config) = prism_s_config {
        runtime
            .usb_protocol_configs
            .set_prism_s_config(device_id, config)
            .await;
    } else {
        runtime.usb_protocol_configs.remove_device(device_id).await;
    }
}

pub(super) async fn connect_backend_device(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
) -> anyhow::Result<()> {
    apply_dynamic_usb_protocol_config(runtime, backend_id, device_id).await;
    let io = backend_io(runtime, backend_id).await?;
    let target_fps = io.connect_with_refresh(device_id).await?;

    let mut manager = runtime.backend_manager.lock().await;
    manager.set_cached_target_fps(backend_id, device_id, target_fps);
    manager.map_device(
        layout_device_id.to_owned(),
        backend_id.to_owned(),
        device_id,
    );
    Ok(())
}

pub(super) async fn disconnect_backend_device(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    backend_io(runtime, backend_id)
        .await?
        .disconnect(device_id)
        .await?;

    {
        let mut manager = runtime.backend_manager.lock().await;
        let _ = manager.remove_device_mappings_for_physical(backend_id, device_id);
    }
    runtime.usb_protocol_configs.remove_device(device_id).await;
    Ok(())
}

pub(super) async fn ensure_default_logical_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    physical_layout_id: &str,
    device_name: &str,
    led_count: u32,
) {
    let mut logical_store = runtime.logical_devices.write().await;
    logical_devices::ensure_default_logical_device(
        &mut logical_store,
        device_id,
        physical_layout_id,
        device_name,
        led_count,
    );
}

pub(super) async fn sync_logical_mappings_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    backend_id: &str,
    fallback_layout_id: &str,
) {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return;
    };

    let total_leds = tracked.info.total_led_count();
    ensure_default_logical_for_device(
        runtime,
        device_id,
        fallback_layout_id,
        &tracked.info.name,
        total_leds,
    )
    .await;

    let (logical_entries, legacy_default_ids) = {
        let logical_store = runtime.logical_devices.read().await;
        let legacy_ids = logical_devices::legacy_default_ids_for_physical(
            &logical_store,
            device_id,
            fallback_layout_id,
        );
        let entries = logical_devices::list_for_physical(&logical_store, device_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .collect::<Vec<_>>();
        (entries, legacy_ids)
    };

    let mut manager = runtime.backend_manager.lock().await;
    let _ = manager.remove_device_mappings_for_physical(backend_id, device_id);

    let fallback = SegmentRange::new(0, usize::try_from(total_leds).unwrap_or_default());

    if logical_entries.is_empty() {
        map_device_with_zone_segments(
            &mut manager,
            fallback_layout_id.to_owned(),
            backend_id.to_owned(),
            device_id,
            Some(fallback),
            &tracked.info,
        );
        map_physical_device_alias(
            &mut manager,
            backend_id,
            device_id,
            fallback_layout_id,
            fallback,
            &tracked.info,
        );
        return;
    }

    let mut default_enabled = false;
    for logical in logical_entries {
        let start = usize::try_from(logical.led_start).unwrap_or_default();
        let length = usize::try_from(logical.led_count).unwrap_or_default();
        if logical.id == fallback_layout_id {
            default_enabled = true;
        }
        map_device_with_zone_segments(
            &mut manager,
            logical.id,
            backend_id.to_owned(),
            device_id,
            Some(SegmentRange::new(start, length)),
            &tracked.info,
        );
    }

    if default_enabled {
        map_physical_device_alias(
            &mut manager,
            backend_id,
            device_id,
            fallback_layout_id,
            fallback,
            &tracked.info,
        );
    }

    for legacy_id in legacy_default_ids {
        map_device_with_zone_segments(
            &mut manager,
            legacy_id,
            backend_id.to_owned(),
            device_id,
            Some(fallback),
            &tracked.info,
        );
    }
}

pub(super) async fn desired_connect_behavior(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    device_info: &DeviceInfo,
    backend_id: &str,
    fingerprint: Option<&DeviceFingerprint>,
    discovered_behavior: hypercolor_core::device::DiscoveryConnectBehavior,
    user_enabled: bool,
) -> hypercolor_core::device::DiscoveryConnectBehavior {
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(backend_id, device_info, fingerprint);
    ensure_default_logical_for_device(
        runtime,
        device_id,
        &layout_device_id,
        &device_info.name,
        device_info.total_led_count(),
    )
    .await;

    if !user_enabled || !discovered_behavior.should_auto_connect() {
        return hypercolor_core::device::DiscoveryConnectBehavior::Deferred;
    }

    if active_layout_targets_enabled_device(runtime, device_id, &layout_device_id).await {
        hypercolor_core::device::DiscoveryConnectBehavior::AutoConnect
    } else {
        hypercolor_core::device::DiscoveryConnectBehavior::Deferred
    }
}

pub(super) async fn active_layout_targets_enabled_device(
    runtime: &DiscoveryRuntime,
    physical_id: DeviceId,
    layout_device_id: &str,
) -> bool {
    let candidate_ids = {
        let logical_store = runtime.logical_devices.read().await;
        let mut candidates = logical_devices::list_for_physical(&logical_store, physical_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.id)
            .collect::<std::collections::HashSet<_>>();

        let default_enabled = logical_store
            .get(layout_device_id)
            .is_none_or(|entry| entry.enabled);
        if default_enabled {
            candidates.insert(layout_device_id.to_owned());
            candidates.extend(logical_devices::legacy_default_ids_for_physical(
                &logical_store,
                physical_id,
                layout_device_id,
            ));
            candidates.insert(physical_id.to_string());
            candidates.insert(format!("device:{physical_id}"));
        }

        candidates
    };

    let spatial = runtime.spatial_engine.read().await;
    spatial
        .layout()
        .zones
        .iter()
        .any(|zone| candidate_ids.contains(&zone.device_id))
}

fn map_device_with_zone_segments(
    manager: &mut BackendManager,
    layout_device_id: impl Into<String>,
    backend_id: impl Into<String>,
    device_id: DeviceId,
    segment: Option<SegmentRange>,
    device_info: &DeviceInfo,
) {
    let layout_device_id = layout_device_id.into();
    manager.map_device_with_segment(layout_device_id.clone(), backend_id, device_id, segment);
    let _ = manager.set_device_zone_segments(&layout_device_id, device_info);
}

fn map_physical_device_alias(
    manager: &mut BackendManager,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
    segment: SegmentRange,
    device_info: &DeviceInfo,
) {
    let physical_alias = device_id.to_string();
    if physical_alias != layout_device_id {
        map_device_with_zone_segments(
            manager,
            physical_alias,
            backend_id.to_owned(),
            device_id,
            Some(segment),
            device_info,
        );
    }

    let legacy_alias = format!("device:{device_id}");
    if legacy_alias != layout_device_id {
        map_device_with_zone_segments(
            manager,
            legacy_alias,
            backend_id.to_owned(),
            device_id,
            Some(segment),
            device_info,
        );
    }
}

pub(super) async fn publish_device_connected(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return;
    };

    let zones = build_zone_refs(&tracked.info);
    info!(
        device = %tracked.info.name,
        device_id = %tracked.info.id,
        backend = %backend_id,
        led_count = tracked.info.total_led_count(),
        zones = zones.len(),
        "device connected"
    );
    runtime.event_bus.publish(HypercolorEvent::DeviceConnected {
        device_id: tracked.info.id.to_string(),
        name: tracked.info.name.clone(),
        backend: backend_id.to_owned(),
        led_count: tracked.info.total_led_count(),
        zones,
    });
}

fn build_zone_refs(info: &DeviceInfo) -> Vec<ZoneRef> {
    info.zones
        .iter()
        .map(|zone| ZoneRef {
            zone_id: format!("{}:{}", info.id, zone.name),
            device_id: info.id.to_string(),
            topology: topology_hint_name(&zone.topology).to_owned(),
            led_count: zone.led_count,
        })
        .collect()
}

const fn topology_hint_name(topology: &DeviceTopologyHint) -> &'static str {
    match topology {
        DeviceTopologyHint::Strip => "strip",
        DeviceTopologyHint::Matrix { .. } => "matrix",
        DeviceTopologyHint::Ring { .. } => "ring",
        DeviceTopologyHint::Point => "point",
        DeviceTopologyHint::Display { .. } => "display",
        DeviceTopologyHint::Custom => "custom",
    }
}

pub(crate) async fn sync_registry_state(runtime: &DiscoveryRuntime, device_id: DeviceId) {
    let state = {
        let lifecycle = runtime.lifecycle_manager.lock().await;
        lifecycle.state(device_id)
    };
    if let Some(state) = state {
        let _ = runtime.device_registry.set_state(&device_id, state).await;
    }
}

pub(super) async fn device_log_label(runtime: &DiscoveryRuntime, device_id: DeviceId) -> String {
    runtime.device_registry.get(&device_id).await.map_or_else(
        || device_id.to_string(),
        |tracked| format!("{} ({device_id})", tracked.info.name),
    )
}

pub(super) fn format_error_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" | caused_by: ")
}

pub(crate) fn backend_id_for_device(
    family: &DeviceFamily,
    metadata: Option<&HashMap<String, String>>,
) -> String {
    if let Some(metadata) = metadata {
        if let Some(backend_id) = metadata.get("backend_id")
            && !backend_id.trim().is_empty()
        {
            return backend_id.clone();
        }

        let has_usb_identity = metadata.contains_key("usb_path")
            || (metadata.contains_key("vendor_id") && metadata.contains_key("product_id"));
        if has_usb_identity {
            return "usb".to_owned();
        }
    }

    match family {
        DeviceFamily::Custom(_) => family.id().into_owned(),
        _ => family.backend_id().to_owned(),
    }
}

pub(super) fn device_ref_for_tracked(
    family: &DeviceFamily,
    info: &DeviceInfo,
    metadata: Option<&HashMap<String, String>>,
) -> DeviceRef {
    DeviceRef {
        id: info.id.to_string(),
        name: info.name.clone(),
        backend: backend_id_for_device(family, metadata),
        led_count: info.total_led_count(),
    }
}
