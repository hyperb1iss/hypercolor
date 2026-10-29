//! The latest authored native projection belongs to the installation, not a transport.

use std::collections::BTreeMap;
use std::sync::Arc;

use hypercolor_types::portable_setup::{PortableSetupV1, SetupBindings};
use uuid::Uuid;

use super::{PortableSetupService, RestoreReceipt, fingerprint, internal};
use crate::domain::DomainError;
use crate::persistence::{
    AtomicFileWriter, AtomicWriteWaitOutcome, PersistenceShutdown, serialize_json_pretty,
};
use crate::portable_setup_state::ActiveProjection;

impl PortableSetupService {
    pub(super) fn read_active_projection(&self) -> Result<Option<ActiveProjection>, DomainError> {
        let bytes = match std::fs::read(self.root.join("active-projection.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(internal(error)),
        };
        let mut active: ActiveProjection = serde_json::from_slice(&bytes).map_err(internal)?;
        if !matches!(active.schema_version, 1 | 2)
            || active.receipt.setup_id != active.receipt.applied.manifest.setup_id
        {
            return Err(DomainError::validation("invalid active setup projection"));
        }
        active
            .receipt
            .applied
            .manifest
            .validate()
            .map_err(internal)?;
        let revision = fingerprint(&active.receipt.applied.manifest)?;
        if active.schema_version == 1 {
            // Version 1 hashed randomized HashMap iteration, then decoded the
            // committed journal before copying its receipt here. That decode
            // discarded the original ordering; the old digest is unverifiable.
            // Recover only from an identical, independently durable commit.
            let Some(super::journal::RestoreJournal::Committed { receipt, .. }) =
                self.read_journal(active.receipt.setup_id)?
            else {
                return Err(DomainError::validation(
                    "legacy active setup projection has no committed receipt",
                ));
            };
            if serde_json::to_value(&active.receipt).map_err(internal)?
                != serde_json::to_value(&receipt).map_err(internal)?
            {
                return Err(DomainError::validation(
                    "legacy active setup projection differs from its committed receipt",
                ));
            }
            active.receipt.applied.revision = revision;
        } else if revision != active.receipt.applied.revision {
            return Err(DomainError::validation("invalid active setup projection"));
        }
        Ok(Some(active))
    }

    /// The caller holds restore admission, so no successor can replace the
    /// journal that proves a legacy projection before its replacement is durable.
    pub(super) async fn migrate_active_projection(&self) -> Result<(), DomainError> {
        let Some(mut active) = self.read_active_projection()? else {
            return Ok(());
        };
        if active.schema_version == 2 {
            return Ok(());
        }
        active.schema_version = 2;
        let writer =
            AtomicFileWriter::new(&self.root.join("active-projection.json")).map_err(internal)?;
        let payload = serialize_json_pretty(&active).map_err(internal)?;
        let restore = super::restore::ActiveRestore::begin(Arc::clone(&self.restore_runtime));
        tokio::task::spawn_blocking(move || {
            let outcome = writer
                .reserve()
                .admit(payload)
                .commit_and_wait_durable_or_shutdown(&restore.shutdown);
            super::transaction::written(outcome)
        })
        .await
        .map_err(internal)?
    }

    /// Publish a complete record before finalizing the restore journal. A pointer
    /// into that journal would be invalidated by the next pending transaction.
    pub(super) fn write_active_projection(
        &self,
        receipt: &RestoreReceipt,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        let mut receipt = receipt.clone();
        receipt.applied.revision = fingerprint(&receipt.applied.manifest)?;
        let active = ActiveProjection {
            schema_version: 2,
            aliases: self.projection_bindings(receipt.setup_id, &receipt.materialized_bindings)?,
            receipt,
        };
        let writer =
            AtomicFileWriter::new(&self.root.join("active-projection.json")).map_err(internal)?;
        Ok(writer
            .reserve()
            .admit(serialize_json_pretty(&active).map_err(internal)?)
            .commit_and_wait_durable_or_shutdown(shutdown))
    }

    /// Translate only identity domains that belong to devices and outputs. The
    /// old per-setup journals remain receipts, never additional authored state.
    pub(super) fn active_projection_for(
        &self,
        setup_id: Uuid,
        live: &SetupBindings,
    ) -> Result<Option<ActiveProjection>, DomainError> {
        let Some(mut active) = self.read_active_projection()? else {
            return Ok(None);
        };
        let mut target = self.saved_bindings(setup_id)?;
        target.devices.extend(live.devices.clone());
        target.outputs.extend(live.outputs.clone());
        let remap = alias_remap(&active.aliases, &target)?;
        if active.receipt.setup_id != setup_id
            || remap.iter().any(|(source, target)| source != target)
        {
            reconcile_unused_historical_aliases(&mut active, &target, live, &remap);
            active.receipt.applied.manifest = active
                .receipt
                .applied
                .manifest
                .remap_devices_and_outputs(&remap)
                .map_err(internal)?;
            active.receipt.applied.manifest.setup_id = setup_id;
            active.receipt.setup_id = setup_id;
            active.receipt.applied.revision = fingerprint(&active.receipt.applied.manifest)?;
            remap_bindings(&mut active.aliases, &remap);
            remap_bindings(&mut active.receipt.materialized_bindings, &remap);
            for id in &mut active.receipt.unresolved_bindings {
                *id = remap.get(id).copied().unwrap_or(*id);
            }
            active.receipt.unresolved_bindings.sort_unstable();
            active.receipt.unresolved_bindings.dedup();
        }
        Ok(Some(active))
    }

    pub(super) fn active_projection_receipt(
        &self,
        setup_id: Uuid,
        live: &SetupBindings,
    ) -> Result<Option<RestoreReceipt>, DomainError> {
        if let Some(active) = self.active_projection_for(setup_id, live)? {
            return Ok(Some(active.receipt));
        }
        match self.read_journal(setup_id)? {
            Some(super::journal::RestoreJournal::Committed { receipt, .. }) => Ok(Some(*receipt)),
            _ => Ok(None),
        }
    }

    pub(super) fn prior_projection(
        &self,
        setup_id: Uuid,
    ) -> Result<Option<PortableSetupV1>, DomainError> {
        if let Some(receipt) =
            self.active_projection_receipt(setup_id, &SetupBindings::default())?
        {
            return Ok(Some(receipt.applied.manifest));
        }
        match self.read_journal(setup_id)? {
            Some(super::journal::RestoreJournal::Pending { before, .. }) => Ok(Some(*before)),
            _ => Ok(None),
        }
    }
}

/// An old projection can retain an unused identity after its native address was
/// associated with an already-authored offline identity. Only that fully proved
/// historical alias is discarded from this clone; ordinary remapping still
/// rejects collisions between distinct or configured devices.
fn reconcile_unused_historical_aliases(
    active: &mut ActiveProjection,
    target: &SetupBindings,
    live: &SetupBindings,
    remap: &BTreeMap<Uuid, Uuid>,
) {
    let manifest = &active.receipt.applied.manifest;
    let removable: Vec<_> = remap
        .iter()
        .filter_map(|(&source, &destination)| {
            let definition = manifest.devices.get(&source)?;
            if source == destination
                || manifest.devices.get(&destination) != Some(definition)
                || remap.get(&destination).is_some_and(|id| *id != destination)
                || remap.iter().any(|(&other, &id)| {
                    other != source && other != destination && (id == destination || id == source)
                })
                || [source, destination].iter().any(|id| {
                    !active.receipt.unresolved_bindings.contains(id)
                        || binding_contains(live, *id)
                        || binding_contains(&active.receipt.materialized_bindings, *id)
                })
                || device_has_authored_state(manifest, source)
                || active.aliases.devices.values().any(|id| *id == destination)
                || active.aliases.outputs.iter().any(|(local, id)| {
                    *id == destination && *local != format!("portable:{destination}")
                })
            {
                return None;
            }
            let mut proved = false;
            for (local, id) in &active.aliases.devices {
                if *id == source {
                    if target.devices.get(local) != Some(&destination) {
                        return None;
                    }
                    proved = true;
                }
            }
            for (local, id) in &active.aliases.outputs {
                if *id == source && *local != format!("portable:{source}") {
                    if target.outputs.get(local) != Some(&destination) {
                        return None;
                    }
                    proved |= !local.starts_with("portable:");
                }
            }
            proved.then_some(source)
        })
        .collect();
    for id in removable {
        active.receipt.applied.manifest.devices.remove(&id);
    }
}

fn binding_contains(bindings: &SetupBindings, id: Uuid) -> bool {
    bindings
        .devices
        .values()
        .chain(bindings.outputs.values())
        .any(|value| *value == id)
}

fn device_has_authored_state(manifest: &PortableSetupV1, id: Uuid) -> bool {
    manifest.attachments.contains_key(&id)
        || manifest.display_faces.contains_key(&id)
        || manifest.device_output.contains_key(&id)
        || manifest.logical_devices.contains_key(&id)
        || manifest
            .logical_devices
            .values()
            .any(|logical| logical.device == id)
        || manifest
            .layouts
            .values()
            .any(|layout| layout.zones.iter().any(|output| output.device == id))
        || manifest.scenes.values().any(|scene| {
            scene.zones.iter().any(|zone| {
                zone.layout.zones.iter().any(|output| output.device == id)
                    || zone
                        .display_target
                        .as_ref()
                        .is_some_and(|target| target.device == id)
            })
        })
}

fn alias_remap(
    source: &SetupBindings,
    target: &SetupBindings,
) -> Result<BTreeMap<Uuid, Uuid>, DomainError> {
    let mut remap = BTreeMap::new();
    let aliases = source
        .devices
        .iter()
        .filter_map(|(local, id)| target.devices.get(local).map(|target| (*id, *target)))
        .chain(
            source
                .outputs
                .iter()
                .filter_map(|(local, id)| target.outputs.get(local).map(|target| (*id, *target))),
        );
    for (source, target) in aliases {
        if remap
            .insert(source, target)
            .is_some_and(|previous| previous != target)
        {
            return Err(DomainError::conflict(
                "setup aliases disagree about a device identity",
            ));
        }
    }
    Ok(remap)
}

fn remap_bindings(bindings: &mut SetupBindings, remap: &BTreeMap<Uuid, Uuid>) {
    for id in bindings
        .devices
        .values_mut()
        .chain(bindings.outputs.values_mut())
    {
        *id = remap.get(id).copied().unwrap_or(*id);
    }
}
