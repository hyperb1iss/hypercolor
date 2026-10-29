//! Native preparation translates portable identities without changing host settings.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use hypercolor_types::device::{DeviceId, DeviceInfo};
use hypercolor_types::library::{PlaylistId, PresetId};
use hypercolor_types::portable_setup::{PortableDeviceOutput, PortableSetupV1, SetupBindings};

use super::{PortableSetupService, internal};
use crate::attachment_profiles::{ComponentProfileStore, PreparedPortableAttachments};
use crate::device_settings::{DeviceSettingsKeys, device_settings_keys};
use crate::domain::DomainError;
use crate::library::LibraryPortableSnapshot;

pub(super) struct DevicePreparation {
    pub infos: HashMap<DeviceId, DeviceInfo>,
    pub live_before: HashMap<DeviceId, PortableDeviceOutput>,
    pub live_after: HashMap<DeviceId, PortableDeviceOutput>,
    pub stored_before: HashMap<String, PortableDeviceOutput>,
    pub stored_after: HashMap<String, PortableDeviceOutput>,
}

impl PortableSetupService {
    pub(super) async fn prepare_device_sources(
        &self,
        before: &PortableSetupV1,
        desired: &PortableSetupV1,
        bindings: &SetupBindings,
        recovering: bool,
    ) -> Result<DevicePreparation, DomainError> {
        let mut result = DevicePreparation {
            infos: HashMap::new(),
            live_before: HashMap::new(),
            live_after: HashMap::new(),
            stored_before: HashMap::new(),
            stored_after: HashMap::new(),
        };
        for device in self.state.device_registry.list().await {
            let id = device.info.id;
            let Some(portable) = bindings.devices.get(&id) else {
                continue;
            };
            let actual = PortableDeviceOutput {
                disabled: !device.user_settings.enabled,
                brightness: device.user_settings.brightness,
                rotation: device.user_settings.display_rotation,
            };
            let default = PortableDeviceOutput {
                disabled: false,
                brightness: 1.0,
                rotation: hypercolor_types::scene::DisplayRotation::default(),
            };
            let expected = before.device_output.get(portable).unwrap_or(&default);
            let target = desired.device_output.get(portable).unwrap_or(&default);
            if &actual != expected && !(recovering && &actual == target) {
                return Err(DomainError::conflict(
                    "device output changed before setup restoration",
                ));
            }
            let DeviceSettingsKeys { canonical, .. } =
                device_settings_keys(&self.state.device_registry, id).await;
            let stored = self
                .state
                .device_settings
                .device_settings_for_key(&canonical)
                .await
                .unwrap_or_default();
            result.stored_before.insert(
                canonical.clone(),
                PortableDeviceOutput {
                    disabled: stored.disabled,
                    brightness: stored.brightness,
                    rotation: stored.rotation,
                },
            );
            result.stored_after.insert(canonical, target.clone());
            result.live_before.insert(id, actual);
            result.live_after.insert(id, target.clone());
            result.infos.insert(id, device.info);
        }
        Ok(result)
    }

    pub(super) async fn prepare_attachments(
        &self,
        before: &PortableSetupV1,
        desired: &PortableSetupV1,
        bindings: &SetupBindings,
        devices: &HashMap<DeviceId, DeviceInfo>,
        catalog: &hypercolor_core::attachment::ComponentRegistry,
        recovering: bool,
    ) -> Result<PreparedPortableAttachments, DomainError> {
        let source = self
            .state
            .attachment_profiles
            .read()
            .await
            .portable_snapshot();
        let mut candidate = source.clone();
        for (local, portable) in &bindings.devices {
            let key = local.to_string();
            let actual = source.get(&key).map(Into::into);
            let expected = before.attachments.get(portable);
            let target = desired.attachments.get(portable);
            if actual.as_ref() != expected && !(recovering && actual.as_ref() == target) {
                return Err(DomainError::conflict(
                    "attachments changed before setup restoration",
                ));
            }
            match (target, devices.get(local)) {
                (Some(profile), Some(device)) => {
                    let value =
                        crate::attachment_profiles::resolution::regenerate_attachment_profile(
                            device,
                            profile.materialize(),
                            catalog,
                        )?;
                    candidate.insert(key, value);
                }
                (None, _) => {
                    candidate.remove(&key);
                }
                (Some(_), None) => {}
            }
        }
        ComponentProfileStore::prepare_portable_replace(
            Arc::clone(&self.state.attachment_profiles),
            &source,
            candidate,
        )
        .await
        .map_err(internal)
    }
}

pub(super) fn library_snapshot(manifest: &PortableSetupV1) -> LibraryPortableSnapshot {
    LibraryPortableSnapshot {
        presets: manifest
            .presets
            .iter()
            .map(|(id, value)| (PresetId(*id), value.clone()))
            .collect(),
        playlists: manifest
            .playlists
            .iter()
            .map(|(id, value)| (PlaylistId(*id), value.clone()))
            .collect(),
    }
}

/// Atomic store files may independently contain their before or after value
/// after a crash. Any third value is a local edit and is never overwritten.
pub(super) fn recovery_map<K: Ord + std::fmt::Display, V: PartialEq>(
    domain: &str,
    current: &BTreeMap<K, V>,
    before: &BTreeMap<K, V>,
    desired: &BTreeMap<K, V>,
) -> Result<(), DomainError> {
    for key in current.keys().chain(before.keys()).chain(desired.keys()) {
        let actual = current.get(key);
        if actual != before.get(key) && actual != desired.get(key) {
            return Err(DomainError::conflict(format!(
                "local data at {domain}.{key} diverged from an interrupted setup restoration"
            )));
        }
    }
    Ok(())
}

impl PortableSetupService {
    pub(super) async fn reconcile_runtime_outputs(
        &self,
        changed_enabled: &[DeviceId],
        reconnect_network_outputs: bool,
    ) -> Result<(), DomainError> {
        let runtime = self.state.domains.devices.discovery_runtime();
        for id in changed_enabled {
            loop {
                let Some(current) = self.state.device_registry.get(id).await else {
                    break;
                };
                let enabled = current.user_settings.enabled;
                match crate::discovery::apply_user_enabled_state(&runtime, *id, enabled)
                    .await
                    .map_err(internal)?
                {
                    crate::discovery::UserEnabledStateResult::Applied => {}
                    crate::discovery::UserEnabledStateResult::MissingLifecycle => {
                        let state = if enabled {
                            hypercolor_types::device::DeviceState::Known
                        } else {
                            hypercolor_types::device::DeviceState::Disabled
                        };
                        self.state.device_registry.set_state(id, state).await;
                    }
                }
                if self
                    .state
                    .device_registry
                    .get(id)
                    .await
                    .is_none_or(|device| device.user_settings.enabled == enabled)
                {
                    break;
                }
            }
        }
        self.state
            .event_bus
            .publish(hypercolor_types::event::HypercolorEvent::DeviceSettingsChanged { key: None });
        if self.state.output_power.snapshot().output_override
            == crate::output_power::OutputOverride::Stopped
        {
            self.state
                .domains
                .devices
                .release_renderable_network_devices()
                .await;
        } else if reconnect_network_outputs
            && !self.state.output_power.snapshot().session_release_active()
        {
            self.state.domains.devices.schedule_output_reconnect(true);
        }
        self.state.domains.output.reconcile_static_hold().await;
        Ok(())
    }
}
