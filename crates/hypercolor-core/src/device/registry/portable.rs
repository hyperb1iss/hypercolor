//! Scoped output-settings publication without transferring machine ownership.

use std::collections::HashMap;
use std::sync::Arc;

use hypercolor_types::device::DeviceId;
use hypercolor_types::portable_setup::PortableDeviceOutput;
use tokio::sync::OwnedRwLockWriteGuard;

use super::{DeviceRegistry, RegistryInner, bump_device_revision};

/// A prepared output change keeps discovery and ordinary settings mutations
/// behind the existing registry write authority until the host persists it.
pub struct PreparedDeviceOutput {
    registry: DeviceRegistry,
    guard: OwnedRwLockWriteGuard<RegistryInner>,
    desired: HashMap<DeviceId, PortableDeviceOutput>,
}

impl DeviceRegistry {
    pub async fn prepare_output_settings(
        &self,
        expected: &HashMap<DeviceId, PortableDeviceOutput>,
        desired: HashMap<DeviceId, PortableDeviceOutput>,
    ) -> anyhow::Result<PreparedDeviceOutput> {
        let guard = Arc::clone(&self.inner).write_owned().await;
        for (id, expected) in expected {
            let entry = guard
                .devices
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("device disappeared before output restoration"))?;
            let actual = PortableDeviceOutput {
                disabled: !entry.user_settings.enabled,
                brightness: entry.user_settings.brightness,
                rotation: entry.user_settings.display_rotation,
            };
            anyhow::ensure!(
                &actual == expected,
                "device output changed before setup restoration"
            );
        }
        anyhow::ensure!(
            desired.keys().all(|id| expected.contains_key(id)),
            "unobserved output target"
        );
        Ok(PreparedDeviceOutput {
            registry: self.clone(),
            guard,
            desired,
        })
    }
}

impl PreparedDeviceOutput {
    /// Publish after the host's matching settings snapshot is durable.
    pub fn publish(&mut self) {
        let mut changed = false;
        for (id, desired) in std::mem::take(&mut self.desired) {
            let entry = self
                .guard
                .devices
                .get_mut(&id)
                .expect("prepared device retained under registry guard");
            if entry.user_settings.enabled != desired.disabled
                && entry.user_settings.brightness == desired.brightness
                && entry.user_settings.display_rotation == desired.rotation
            {
                continue;
            }
            entry.user_settings.enabled = !desired.disabled;
            entry.user_settings.brightness = desired.brightness;
            entry.user_settings.display_rotation = desired.rotation;
            bump_device_revision(entry);
            changed = true;
        }
        if changed {
            self.registry.bump_generation();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceRegistry;
    use crate::device::mock::{MockDeviceBackend, MockDeviceConfig};
    use hypercolor_types::portable_setup::PortableDeviceOutput;
    use hypercolor_types::spatial::LedTopology;
    use std::collections::HashMap;

    #[tokio::test]
    async fn guarded_output_publication_preserves_name_and_rejects_stale_source() {
        let backend = MockDeviceBackend::new().with_device(&MockDeviceConfig {
            name: "Controller".into(),
            led_count: 8,
            topology: LedTopology::Strip {
                count: 8,
                direction: hypercolor_types::spatial::StripDirection::LeftToRight,
            },
            id: None,
        });
        let info = backend.device_infos()[0].clone();
        let id = info.id;
        let registry = DeviceRegistry::new();
        registry.add(info).await;
        registry
            .update_user_settings(&id, Some("My lights".into()), None, None, None)
            .await;
        let expected = HashMap::from([(
            id,
            PortableDeviceOutput {
                disabled: false,
                brightness: 1.0,
                rotation: hypercolor_types::scene::DisplayRotation::default(),
            },
        )]);
        let desired = HashMap::from([(
            id,
            PortableDeviceOutput {
                disabled: true,
                brightness: 0.4,
                rotation: hypercolor_types::scene::DisplayRotation::default(),
            },
        )]);
        let mut prepared = registry
            .prepare_output_settings(&expected, desired)
            .await
            .expect("prepare");
        assert!(registry.inner.try_write().is_err());
        prepared.publish();
        drop(prepared);
        let current = registry.get(&id).await.expect("device");
        assert_eq!(current.user_settings.name.as_deref(), Some("My lights"));
        assert!(!current.user_settings.enabled);
        assert_eq!(current.user_settings.brightness, 0.4);
        assert!(
            registry
                .prepare_output_settings(&expected, HashMap::new())
                .await
                .is_err()
        );
    }
}
