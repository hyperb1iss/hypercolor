use std::collections::{HashMap, HashSet};
use std::path::Path;

use hypercolor_core::device::UsbProtocolConfigStore;
use hypercolor_types::device::DeviceId;
use tokio::sync::RwLock;

use crate::attachment_profiles::ComponentProfileStore;
use crate::domain::DomainError;
use crate::domain::context::SceneContext;
use crate::logical_devices::LogicalDevice;

pub(crate) struct DeviceRemovalContext<'a> {
    pub scene: &'a SceneContext,
    pub logical_devices: &'a RwLock<HashMap<String, LogicalDevice>>,
    pub logical_devices_path: &'a Path,
    pub attachment_profiles: &'a RwLock<ComponentProfileStore>,
    pub usb_protocol_configs: &'a UsbProtocolConfigStore,
}

pub(crate) async fn forget_saved_content(
    state: &DeviceRemovalContext<'_>,
    target: &str,
    physical_id: Option<DeviceId>,
) -> Result<(), DomainError> {
    let mut targets = HashSet::from([target.to_owned()]);
    if let Some(physical_id) = physical_id {
        targets.insert(physical_id.to_string());
        targets.extend(
            state
                .logical_devices
                .read()
                .await
                .values()
                .filter(|entry| entry.physical_device_id == physical_id)
                .map(|entry| entry.id.clone()),
        );
    }
    state
        .scene
        .forget_layout_targets(targets.clone(), target)
        .await?;
    if let Some(physical_id) = physical_id {
        let mut profiles = state.attachment_profiles.write().await;
        let mut candidate = profiles.clone();
        if candidate.remove(&physical_id.to_string()).is_some() {
            candidate.save().map_err(DomainError::Internal)?;
            *profiles = candidate;
        }
        drop(profiles);
        state.usb_protocol_configs.remove_device(physical_id).await;
    }
    let mut logical = state.logical_devices.write().await;
    let mut candidate = logical.clone();
    candidate.retain(|id, _| !targets.contains(id));
    if candidate != *logical {
        let pending =
            crate::logical_devices::reserve_save_segments(state.logical_devices_path, &candidate)
                .map_err(DomainError::Internal)?;
        crate::logical_devices::save_reserved_segments(pending).map_err(DomainError::Internal)?;
        *logical = candidate;
    }
    Ok(())
}
