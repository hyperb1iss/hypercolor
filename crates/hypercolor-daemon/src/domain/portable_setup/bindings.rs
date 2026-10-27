use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;

use hypercolor_core::device::DeviceLifecycleManager;
use hypercolor_types::portable_setup::{PortableSetupV1, SetupBindings};
use uuid::Uuid;

use super::{PortableSetupService, internal};
use crate::domain::DomainError;
use crate::domain::device_binding::CurrentBinding;
use crate::logical_devices::LogicalDeviceKind;

impl PortableSetupService {
    /// Assign installation-local associations once, then reuse them across boots.
    /// Cross-OS matching uses the existing conservative device class/shape
    /// evidence and requires a unique match in both directions after discovery.
    pub async fn assign_bindings(
        &self,
        setup_id: Uuid,
        incoming: Option<&PortableSetupV1>,
    ) -> Result<SetupBindings, DomainError> {
        let _gate = self.association_gate.lock().await;
        if self.state.discovery_in_progress.load(Ordering::Acquire) {
            return Err(DomainError::conflict(
                "setup associations require a completed discovery sweep",
            ));
        }
        if incoming.is_some_and(|manifest| manifest.setup_id != setup_id) {
            return Err(DomainError::validation(
                "incoming setup identity does not match the association",
            ));
        }
        let device_generation = self.state.device_registry.generation();
        let path = self.root.join(format!("{setup_id}.bindings.json"));
        let mut durable: SetupBindings = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(internal)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => SetupBindings::default(),
            Err(error) => return Err(internal(error)),
        };
        let prior = self.prior_projection(setup_id)?;
        let manifest = incoming.or(prior.as_ref());
        let devices = self
            .state
            .device_registry
            .list()
            .await
            .into_iter()
            .filter(|device| {
                !matches!(
                    device.state,
                    hypercolor_types::device::DeviceState::Reconnecting
                )
            })
            .collect::<Vec<_>>();
        let claims = self.state.device_registry.claims_snapshot().await;
        let quarantined = self.state.device_registry.quarantined_portable_keys().await;
        let mut current = Vec::new();
        for device in &devices {
            let fingerprint = self
                .state
                .device_registry
                .fingerprint_for_id(&device.info.id)
                .await;
            let layout_id = DeviceLifecycleManager::canonical_layout_device_id(
                &device.info,
                fingerprint.as_ref(),
            );
            let hint = fingerprint.as_ref().and_then(|fingerprint| {
                CurrentBinding::from_discovery(
                    &device.info,
                    fingerprint,
                    layout_id.clone(),
                    claims.contains_key(&device.info.id),
                )
            });
            current.push((device.info.id, layout_id, hint));
        }
        let mut assigned = HashMap::new();
        for (device_id, _, _) in &current {
            if let Some(portable) = durable.devices.get(device_id) {
                // An explicit association is valid only within its original setup.
                if manifest.is_none_or(|manifest| manifest.devices.contains_key(portable)) {
                    assigned.insert(*device_id, *portable);
                }
            }
        }
        if !self.state.discovery_in_progress.load(Ordering::Acquire)
            && let Some(manifest) = manifest
        {
            let already: HashSet<Uuid> = assigned.values().copied().collect();
            let mut candidates = HashMap::<Uuid, Vec<_>>::new();
            for (portable_id, definition) in &manifest.devices {
                if already.contains(portable_id) {
                    continue;
                }
                let matches = current
                    .iter()
                    .filter(|(device_id, _, hint)| {
                        if assigned.contains_key(device_id) {
                            return false;
                        }
                        if let Some(key) = &definition.portable_key {
                            !quarantined.contains(key)
                                && claims
                                    .get(device_id)
                                    .is_some_and(|claim| claim.key() == key)
                        } else {
                            definition.binding_hint.as_ref().is_some_and(|source| {
                                hint.as_ref()
                                    .is_some_and(|target| target.matches_portable_hint(source))
                            })
                        }
                    })
                    .map(|(id, _, _)| *id)
                    .collect::<Vec<_>>();
                candidates.insert(*portable_id, matches);
            }
            let mut claimants = HashMap::<_, usize>::new();
            for matches in candidates.values() {
                for id in matches {
                    *claimants.entry(*id).or_default() += 1;
                }
            }
            for (portable, matches) in candidates {
                if let [id] = matches.as_slice()
                    && claimants.get(id) == Some(&1)
                {
                    assigned.insert(*id, portable);
                }
            }
        }
        let mut result = SetupBindings::default();
        for (device_id, layout_id, _) in current {
            let portable = *assigned.entry(device_id).or_insert_with(Uuid::now_v7);
            durable.devices.insert(device_id, portable);
            durable.outputs.insert(layout_id.clone(), portable);
            result.devices.insert(device_id, portable);
            result.outputs.insert(layout_id, portable);
        }
        for logical in self.state.logical_devices.read().await.values() {
            if logical.kind == LogicalDeviceKind::Default {
                continue;
            }
            let portable = *durable
                .outputs
                .entry(logical.id.clone())
                .or_insert_with(Uuid::now_v7);
            result.outputs.insert(logical.id.clone(), portable);
        }
        let bytes = crate::persistence::serialize_json_pretty(&durable).map_err(internal)?;
        if self.state.discovery_in_progress.load(Ordering::Acquire)
            || self.state.device_registry.generation() != device_generation
        {
            return Err(DomainError::conflict(
                "discovery changed while setup associations were prepared",
            ));
        }
        crate::persistence::write_atomic(&path, &bytes).map_err(internal)?;
        Ok(result)
    }
}
