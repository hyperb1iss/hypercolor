//! Orders primary-effect snapshots and retains only their definition fields.

use crate::api::{PrimaryEffectMetadata, PrimaryEffectView};

/// One owner for full refreshes and control-only reconciliation.
#[derive(Debug, Default)]
pub struct PrimaryEffectRefresh {
    generation: u64,
    metadata: Option<PrimaryEffectMetadata>,
}

impl PrimaryEffectRefresh {
    /// Retire older requests, optionally invalidating the effect definition.
    pub fn begin(&mut self, invalidate_metadata: bool) -> (u64, Option<PrimaryEffectMetadata>) {
        self.generation = self.generation.wrapping_add(1);
        if invalidate_metadata {
            self.metadata = None;
        }
        (self.generation, self.metadata.clone())
    }

    /// Accept only the newest snapshot, including its metadata cache update.
    pub fn finish(&mut self, generation: u64, view: Option<&PrimaryEffectView>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.metadata = view.map(PrimaryEffectMetadata::from);
        true
    }
}
