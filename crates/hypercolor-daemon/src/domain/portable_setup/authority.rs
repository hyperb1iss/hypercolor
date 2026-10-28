//! The latest authored native projection belongs to the installation, not a transport.

use std::collections::BTreeMap;

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
        let active: ActiveProjection = serde_json::from_slice(&bytes).map_err(internal)?;
        if active.schema_version != 1
            || active.receipt.setup_id != active.receipt.applied.manifest.setup_id
            || fingerprint(&active.receipt.applied.manifest)? != active.receipt.applied.revision
        {
            return Err(DomainError::validation("invalid active setup projection"));
        }
        active
            .receipt
            .applied
            .manifest
            .validate()
            .map_err(internal)?;
        Ok(Some(active))
    }

    /// Publish a complete record before finalizing the restore journal. A pointer
    /// into that journal would be invalidated by the next pending transaction.
    pub(super) fn write_active_projection(
        &self,
        receipt: &RestoreReceipt,
        shutdown: &PersistenceShutdown,
    ) -> Result<AtomicWriteWaitOutcome, DomainError> {
        let active = ActiveProjection {
            schema_version: 1,
            receipt: receipt.clone(),
            aliases: self.projection_bindings(receipt.setup_id, &receipt.materialized_bindings)?,
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
