//! Portable catalog preparation retains the normal layout update authority.

use std::collections::{BTreeMap, HashMap};

use hypercolor_types::portable_setup::{PortableLayout, SetupBindings};
use hypercolor_types::spatial::SpatialLayout;
use tokio::sync::OwnedRwLockWriteGuard;

use super::LayoutContext;
use crate::domain::DomainError;
use crate::persistence::{AtomicFileWriter, AtomicWriteReservation};
use crate::persistence::{AtomicWriteWaitOutcome, PersistenceShutdown};
use crate::scene_transactions::LayoutUpdateGuard;

pub(crate) struct PreparedPortableLayouts {
    entries: OwnedRwLockWriteGuard<HashMap<String, SpatialLayout>>,
    candidate: Option<HashMap<String, SpatialLayout>>,
    write: Option<AtomicWriteReservation>,
    payload: Vec<u8>,
    durable: bool,
    _guard: LayoutUpdateGuard,
}

impl LayoutContext {
    pub(crate) async fn prepare_portable_replace(
        &self,
        guard: LayoutUpdateGuard,
        expected: &BTreeMap<String, PortableLayout>,
        desired: &BTreeMap<String, PortableLayout>,
        bindings: &SetupBindings,
        projection_bindings: &SetupBindings,
        canvas: (u32, u32),
    ) -> Result<PreparedPortableLayouts, DomainError> {
        let entries = self.catalog.owned_entries().write_owned().await;
        let mut source = entries.clone();
        let current = self.current();
        source.entry(current.id.clone()).or_insert(current);
        let projected = source
            .iter()
            .map(|(id, layout)| {
                PortableLayout::project(layout, projection_bindings)
                    .map(|value| (id.clone(), value))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(|error| DomainError::validation(error.to_string()))?;
        if &projected != expected {
            return Err(DomainError::conflict(
                "layouts changed before setup restoration",
            ));
        }
        let candidate = desired
            .iter()
            .map(|(id, layout)| (id.clone(), layout.materialize(bindings, canvas)))
            .collect::<HashMap<_, _>>();
        let payload = crate::layout_store::serialize(&candidate).map_err(DomainError::Internal)?;
        let writer = AtomicFileWriter::new(self.catalog.path())
            .map_err(|error| DomainError::Internal(error.into()))?;
        Ok(PreparedPortableLayouts {
            entries,
            candidate: Some(candidate),
            write: Some(writer.reserve()),
            payload,
            durable: false,
            _guard: guard,
        })
    }
}

impl PreparedPortableLayouts {
    pub(crate) fn persist(&mut self, shutdown: &PersistenceShutdown) -> AtomicWriteWaitOutcome {
        if shutdown.is_shutdown() {
            return AtomicWriteWaitOutcome::ShutdownPending;
        }
        let result = self
            .write
            .take()
            .expect("layout candidate persisted once")
            .admit(std::mem::take(&mut self.payload))
            .commit_and_wait_durable_or_shutdown(shutdown);
        self.durable = matches!(result, AtomicWriteWaitOutcome::Written);
        result
    }

    pub(crate) fn publish(&mut self) {
        assert!(self.durable, "layout candidate durable before publication");
        *self.entries = self
            .candidate
            .take()
            .expect("layout candidate published once");
    }
}
