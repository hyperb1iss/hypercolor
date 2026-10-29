use std::collections::HashMap;

use hypercolor_types::portable_setup::PortableDeviceOutput;

use super::{DeviceSettingsAccess, DeviceSettingsStore, serialize_settings_snapshot};
use crate::domain::DomainError;
use crate::domain::portable_setup::prepared::PreparedStoreReplacement;
use crate::persistence::AtomicFileWriter;

impl DeviceSettingsAccess {
    pub(crate) async fn prepare_portable_output(
        &self,
        expected: &HashMap<String, PortableDeviceOutput>,
        desired: &HashMap<String, PortableDeviceOutput>,
        brightness: Option<(f32, f32)>,
    ) -> Result<PreparedStoreReplacement<DeviceSettingsStore>, DomainError> {
        let guard = self.store.clone().write_owned().await;
        if let Some(reason) = &guard.refuse_writes {
            return Err(DomainError::conflict(reason.clone()));
        }
        let mut candidate = guard.clone();
        for (key, expected) in expected {
            let current = guard.device_settings_for_key(key).unwrap_or_default();
            if current.disabled != expected.disabled
                || current.brightness != expected.brightness
                || current.rotation != expected.rotation
            {
                return Err(DomainError::conflict(
                    "persisted device output changed before restoration",
                ));
            }
            let value = desired.get(key).cloned().unwrap_or(PortableDeviceOutput {
                disabled: false,
                brightness: 1.0,
                rotation: hypercolor_types::scene::DisplayRotation::default(),
            });
            let mut settings = current;
            settings.disabled = value.disabled;
            settings.brightness = value.brightness;
            settings.rotation = value.rotation;
            candidate.set_device_settings(key, settings);
        }
        if let Some((expected, desired)) = brightness {
            if guard.snapshot.global_brightness != expected {
                return Err(DomainError::conflict(
                    "brightness changed before restoration",
                ));
            }
            candidate.snapshot.global_brightness = desired;
        }
        let payload =
            serialize_settings_snapshot(&candidate.snapshot).map_err(DomainError::Internal)?;
        let writer = AtomicFileWriter::new(&candidate.path)
            .map_err(|error| DomainError::Internal(error.into()))?;
        Ok(PreparedStoreReplacement::new(
            guard, candidate, &writer, payload,
        ))
    }
}
