//! Newly discovered targets receive saved definitions before they can be exported.

use hypercolor_types::portable_setup::PortableDeviceOutput;
use std::sync::Arc;
use uuid::Uuid;

use super::export::ProjectionIdentity;
use super::restore::ActiveRestore;
use super::{
    PortableSetupService, RestoreMode, RestoreOptions, RestoreReport, SetupIdentity, fingerprint,
    internal,
};
use crate::domain::DomainError;

impl PortableSetupService {
    pub async fn reconcile_bindings(
        &self,
        identity: SetupIdentity,
    ) -> Result<Option<RestoreReport>, DomainError> {
        self.reconcile_projection(ProjectionIdentity::Explicit(identity))
            .await
    }

    pub(super) async fn reconcile_projection(
        &self,
        identity: ProjectionIdentity,
    ) -> Result<Option<RestoreReport>, DomainError> {
        let service = self.clone();
        let setup_id = identity.setup_id();
        tokio::spawn(async move {
            let _gate = service.restore_runtime.gate.lock().await;
            service.ensure_restore_open()?;
            service.recover_pending_locked().await?;
            service.recover_locked(setup_id).await?;
            let Some(receipt) = service.active_projection_receipt(
                setup_id,
                &hypercolor_types::portable_setup::SetupBindings::default(),
            )?
            else {
                return Ok(None);
            };
            let identity = identity.resolve(Some(&receipt));
            let bindings = service
                .assign_bindings(identity.setup_id, Some(&receipt.applied.manifest))
                .await?;
            let newly_bound = receipt
                .applied
                .manifest
                .devices
                .keys()
                .copied()
                .filter(|id| {
                    bindings.local_device(*id).is_some()
                        && (receipt.unresolved_bindings.contains(id)
                            || bindings.local_device(*id)
                                != receipt.materialized_bindings.local_device(*id))
                })
                .collect::<std::collections::HashSet<_>>();
            if newly_bound.is_empty() {
                return Ok(None);
            }
            let current = service.export_raw(identity, &bindings).await?;
            let mut candidate = current.manifest.clone();
            let saved = &receipt.applied.manifest;
            for id in &newly_bound {
                let default = PortableDeviceOutput {
                    disabled: false,
                    brightness: 1.0,
                    rotation: hypercolor_types::scene::DisplayRotation::default(),
                };
                let old = current.manifest.device_output.get(id).unwrap_or(&default);
                let target = saved.device_output.get(id).unwrap_or(&default);
                if old != &default && old != target {
                    return Err(DomainError::conflict(
                        "newly connected device was edited before saved settings could be restored",
                    ));
                }
                candidate.device_output.insert(*id, target.clone());
                reconcile_optional(&mut candidate.display_faces, &saved.display_faces, *id)?;
                reconcile_optional(&mut candidate.attachments, &saved.attachments, *id)?;
            }
            for (id, logical) in &saved.logical_devices {
                if newly_bound.contains(&logical.device) {
                    if candidate
                        .logical_devices
                        .get(id)
                        .is_some_and(|value| value != logical)
                    {
                        return Err(DomainError::conflict(
                            "newly connected logical device was edited before restoration",
                        ));
                    }
                    candidate.logical_devices.insert(*id, logical.clone());
                }
            }
            for (id, scene) in &mut candidate.scenes {
                let Some(prior) = saved.scenes.get(id) else {
                    continue;
                };
                for (index, old) in prior.zones.iter().enumerate() {
                    if old
                        .display_target
                        .as_ref()
                        .is_some_and(|target| newly_bound.contains(&target.device))
                        && !scene.zones.iter().any(|zone| zone.id == old.id)
                    {
                        scene
                            .zones
                            .insert(index.min(scene.zones.len()), old.clone());
                    }
                }
                for zone in &mut scene.zones {
                    let Some(old) = prior.zones.iter().find(|old| old.id == zone.id) else {
                        continue;
                    };
                    if old
                        .display_target
                        .as_ref()
                        .is_some_and(|target| newly_bound.contains(&target.device))
                    {
                        let mut expected = old.clone();
                        expected.display_target.clone_from(&zone.display_target);
                        if *zone != expected && zone.display_target != old.display_target {
                            return Err(DomainError::conflict(
                                "display scene changed before its saved target was connected",
                            ));
                        }
                        zone.display_target.clone_from(&old.display_target);
                    }
                }
            }
            let options = RestoreOptions {
                operation_id: Uuid::now_v7(),
                expected_revision: current.revision,
                mode: RestoreMode::Merge,
                apply_selection: false,
            };
            let active = ActiveRestore::begin(Arc::clone(&service.restore_runtime));
            let report = Box::pin(service.apply_candidate(
                current.manifest,
                candidate.clone(),
                fingerprint(&candidate)?,
                bindings,
                options,
                &active.shutdown,
                None,
                None,
            ))
            .await?;
            Ok(Some(report))
        })
        .await
        .map_err(internal)?
    }
}

fn reconcile_optional<T: Clone + PartialEq>(
    current: &mut std::collections::BTreeMap<Uuid, T>,
    saved: &std::collections::BTreeMap<Uuid, T>,
    id: Uuid,
) -> Result<(), DomainError> {
    if let Some(target) = saved.get(&id) {
        if current.get(&id).is_some_and(|value| value != target) {
            return Err(DomainError::conflict(
                "newly connected device has newer authored settings",
            ));
        }
        current.insert(id, target.clone());
    }
    Ok(())
}
