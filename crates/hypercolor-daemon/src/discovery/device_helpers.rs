use anyhow::Context;
use hypercolor_core::device::{BackendIo, BackendManager, DeviceLifecycleManager, SegmentRange};
use hypercolor_driver_api::{DeviceLifecyclePolicy, DiscoveredDevice};
use hypercolor_types::device::{
    DeviceError, DeviceFingerprint, DeviceId, DeviceInfo, DeviceTopologyHint, DeviceUserSettings,
};
use hypercolor_types::event::{DeviceRef, HypercolorEvent, ZoneRef};
use tracing::info;

use std::time::Duration;

use super::DiscoveryRuntime;
use crate::device_settings::StoredDeviceSettings;
use crate::logical_devices;

const DEVICE_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) async fn apply_persisted_device_settings(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
) -> DeviceUserSettings {
    let fallback_settings = runtime
        .device_registry
        .get(&device_id)
        .await
        .map_or_else(DeviceUserSettings::default, |tracked| tracked.user_settings);
    let key = crate::device_settings::resolve_device_settings_key(
        &runtime.device_registry,
        &runtime.device_settings,
        device_id,
    )
    .await;
    let persisted_settings = runtime
        .device_settings
        .device_settings_for_key(&key)
        .await
        .map_or(fallback_settings, stored_device_settings_to_user_settings);

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
        display_rotation: settings.rotation,
    }
}

pub(super) async fn refresh_connected_device_info(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    let io = backend_io(runtime, backend_id).await?;
    let maybe_info = io.connected_device_info(device_id).await?;
    let metadata = io.connected_device_metadata(device_id).await?;

    if let Some(metadata) = metadata {
        let tracked = runtime
            .device_registry
            .get(&device_id)
            .await
            .context("connected device is no longer tracked")?;
        let fingerprint = runtime
            .device_registry
            .fingerprint_for_id(&device_id)
            .await
            .context("connected device has no registered fingerprint")?;
        let mut info = maybe_info.unwrap_or(tracked.info);
        info.id = device_id;
        runtime
            .device_registry
            .refresh_discovered(
                &device_id,
                DiscoveredDevice {
                    info,
                    fingerprint,
                    metadata,
                    connect_behavior: tracked.connect_behavior,
                    claim: None,
                },
            )
            .await
            .context("connected device identity changed during metadata refresh")?;
    } else if let Some(info) = maybe_info {
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

pub(super) async fn lifecycle_policy_for_device_info(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    info: &DeviceInfo,
) -> DeviceLifecyclePolicy {
    let Ok(io) = backend_io(runtime, backend_id).await else {
        return DeviceLifecyclePolicy::default();
    };

    io.lifecycle_policy(info)
}

pub(super) async fn lifecycle_policy_for_device(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> DeviceLifecyclePolicy {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return DeviceLifecyclePolicy::default();
    };

    lifecycle_policy_for_device_info(runtime, backend_id, &tracked.info).await
}

pub(super) async fn sync_host_attachment_profile_config(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    backend: &BackendIo,
) {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    };

    if !backend.supports_host_attachment_profiles(&tracked.info) {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    }

    let profile = {
        let profiles = runtime.attachment_profiles.read().await;
        profiles.get(&tracked.info.id.to_string()).cloned()
    };
    let Some(profile) = profile else {
        runtime.usb_protocol_configs.remove_device(device_id).await;
        return;
    };

    let registry = {
        let registry = runtime.attachment_registry.read().await;
        registry.clone()
    };
    let applied = runtime
        .usb_protocol_configs
        .apply_attachment_profile(device_id, &tracked.info, &profile, &registry)
        .await;

    if !applied {
        runtime.usb_protocol_configs.remove_device(device_id).await;
    }
}

pub(super) async fn connect_backend_device_with_timeout(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
    timeout: Duration,
) -> Result<(), DeviceError> {
    connect_backend_device_inner(
        runtime,
        backend_id,
        device_id,
        layout_device_id,
        Some(timeout),
    )
    .await
}

async fn connect_backend_device_inner(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
    layout_device_id: &str,
    timeout: Option<Duration>,
) -> Result<(), DeviceError> {
    let io = {
        let manager = runtime.backend_manager.lock().await;
        manager
            .backend_io(backend_id)
            .ok_or_else(|| DeviceError::NotFound {
                device: format!("backend {backend_id}"),
            })?
    };
    adopt_discovered_device(runtime, device_id, &io).await?;
    sync_host_attachment_profile_config(runtime, device_id, &io).await;
    let output_cadence = match timeout {
        Some(timeout) => io.connect_with_timeout(device_id, timeout).await?,
        None => io.connect(device_id).await?,
    };
    let frame_sink = io.frame_sink(device_id);

    let mut manager = runtime.backend_manager.lock().await;
    manager.set_cached_output_cadence(backend_id, device_id, output_cadence);
    manager.set_device_frame_sink(backend_id, device_id, frame_sink);
    manager.map_device(
        layout_device_id.to_owned(),
        backend_id.to_owned(),
        device_id,
    );
    Ok(())
}

/// Hand the backend the descriptor it needs before a connect. The scan
/// adopts a device under the id it discovered it with, which for a
/// device without a serial is fresh every scan, so any connect that
/// bypasses the lifecycle path has to adopt under the registry's id first.
pub(crate) async fn adopt_discovered_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    backend: &BackendIo,
) -> Result<(), DeviceError> {
    let Some(tracked) = runtime.device_registry.get(&device_id).await else {
        return Ok(());
    };
    let Some(fingerprint) = runtime.device_registry.fingerprint_for_id(&device_id).await else {
        return Ok(());
    };
    let metadata = runtime
        .device_registry
        .metadata_for_id(&device_id)
        .await
        .unwrap_or_default();
    let claim = runtime.device_registry.claim_for_id(&device_id).await;

    backend.adopt_device(&DiscoveredDevice {
        fingerprint,
        connect_behavior: tracked.connect_behavior,
        info: tracked.info,
        metadata,
        claim,
    })
}

pub(super) async fn disconnect_backend_device(
    runtime: &DiscoveryRuntime,
    backend_id: &str,
    device_id: DeviceId,
) -> anyhow::Result<()> {
    {
        let mut manager = runtime.backend_manager.lock().await;
        let _ = manager.remove_device_mappings_for_physical(backend_id, device_id);
    }
    runtime.usb_protocol_configs.remove_device(device_id).await;

    let io = backend_io(runtime, backend_id).await?;
    Ok(
        tokio::time::timeout(DEVICE_DISCONNECT_TIMEOUT, io.disconnect(device_id))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "timed out disconnecting device {device_id} using backend '{backend_id}'"
                )
            })??,
    )
}

pub(super) async fn ensure_default_logical_for_device(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    physical_layout_id: &str,
    device_name: &str,
    led_count: u32,
) {
    let mut logical_store = runtime.logical_devices.write().await;
    if let Err(error) = logical_devices::ensure_persisted_default(
        &runtime.logical_devices_path,
        &mut logical_store,
        device_id,
        physical_layout_id,
        device_name,
        led_count,
    ) {
        tracing::warn!(%error, %device_id, "Failed to persist controller identity");
    }
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

    let logical_entries = {
        let logical_store = runtime.logical_devices.read().await;
        logical_devices::list_for_physical(&logical_store, device_id)
            .into_iter()
            .filter(|entry| entry.enabled)
            .collect::<Vec<_>>()
    };

    let mut manager = runtime.backend_manager.lock().await;
    let _ = manager.clear_device_mappings_for_physical(backend_id, device_id);

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
        return;
    }

    for logical in logical_entries {
        let start = usize::try_from(logical.led_start).unwrap_or_default();
        let length = usize::try_from(logical.led_count).unwrap_or_default();
        map_device_with_zone_segments(
            &mut manager,
            logical.id,
            backend_id.to_owned(),
            device_id,
            Some(SegmentRange::new(start, length)),
            &tracked.info,
        );
    }
}

pub(crate) async fn desired_connect_behavior(
    runtime: &DiscoveryRuntime,
    device_id: DeviceId,
    device_info: &DeviceInfo,
    fingerprint: Option<&DeviceFingerprint>,
    discovered_behavior: hypercolor_driver_api::DiscoveryConnectBehavior,
    user_enabled: bool,
) -> hypercolor_driver_api::DiscoveryConnectBehavior {
    let layout_device_id =
        DeviceLifecycleManager::canonical_layout_device_id(device_info, fingerprint);
    ensure_default_logical_for_device(
        runtime,
        device_id,
        &layout_device_id,
        &device_info.name,
        device_info.total_led_count(),
    )
    .await;

    if !user_enabled || !discovered_behavior.should_auto_connect() {
        return hypercolor_driver_api::DiscoveryConnectBehavior::Deferred;
    }

    if topology_unknown_until_connect(device_info) {
        return hypercolor_driver_api::DiscoveryConnectBehavior::AutoConnect;
    }

    if runtime
        .layout
        .active_layout_targets_enabled_device(runtime, device_id, &layout_device_id)
        .await
    {
        hypercolor_driver_api::DiscoveryConnectBehavior::AutoConnect
    } else {
        hypercolor_driver_api::DiscoveryConnectBehavior::Deferred
    }
}

/// Whether a device has nothing a layout could place yet: no light segment
/// with LEDs. Hubs and radios learn their topology from the hardware at
/// connect, and a screen gets its surface zone seeded once it is up, so
/// waiting for the layout to target such a device would wait forever.
pub(crate) fn topology_unknown_until_connect(info: &DeviceInfo) -> bool {
    !info.segments.iter().any(|segment| {
        segment.led_count > 0 && !matches!(segment.topology, DeviceTopologyHint::Display { .. })
    })
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
        origin: tracked.info.origin.clone(),
        led_count: tracked.info.total_led_count(),
        zones,
    });
}

fn build_zone_refs(info: &DeviceInfo) -> Vec<ZoneRef> {
    info.segments
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

pub(super) fn device_ref_for_tracked(info: &DeviceInfo) -> DeviceRef {
    DeviceRef {
        id: info.id.to_string(),
        name: info.name.clone(),
        origin: info.origin.clone(),
        led_count: info.total_led_count(),
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_types::device::{
        ConnectionType, DeviceCapabilities, DeviceColorFormat, DeviceFamily, DeviceOrigin,
        SegmentInfo,
    };

    use super::*;

    fn device(segments: Vec<SegmentInfo>) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(),
            name: "Probe".to_owned(),
            vendor: "Test".to_owned(),
            family: DeviceFamily::new_static("probe", "Probe"),
            model: None,
            connection_type: ConnectionType::Usb,
            origin: DeviceOrigin::native("probe", "usb", ConnectionType::Usb),
            segments,
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        }
    }

    fn segment(led_count: u32, topology: DeviceTopologyHint) -> SegmentInfo {
        SegmentInfo {
            name: "Segment".to_owned(),
            led_count,
            topology,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }
    }

    #[test]
    fn a_hub_with_no_segments_yet_connects_without_a_layout() {
        assert!(topology_unknown_until_connect(&device(Vec::new())));
    }

    #[test]
    fn a_screen_only_device_connects_without_a_layout() {
        let screen = segment(
            0,
            DeviceTopologyHint::Display {
                width: 480,
                height: 480,
                circular: true,
                format: hypercolor_types::device::DisplayFrameFormat::Jpeg,
            },
        );
        assert!(topology_unknown_until_connect(&device(vec![screen])));
    }

    #[test]
    fn a_device_with_placeable_leds_waits_for_the_layout() {
        let ring = segment(20, DeviceTopologyHint::Ring { count: 20 });
        assert!(!topology_unknown_until_connect(&device(vec![ring])));
        let screen = segment(
            0,
            DeviceTopologyHint::Display {
                width: 480,
                height: 480,
                circular: true,
                format: hypercolor_types::device::DisplayFrameFormat::Jpeg,
            },
        );
        let ring = segment(20, DeviceTopologyHint::Ring { count: 20 });
        assert!(!topology_unknown_until_connect(&device(vec![screen, ring])));
    }
}
