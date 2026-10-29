//! Orders primary-effect snapshots and retains only their definition fields.

use crate::api::{PrimaryEffectMetadata, PrimaryEffectView};

/// One owner for full refreshes and control-only reconciliation.
#[derive(Debug, Default)]
pub struct PrimaryEffectRefresh {
    generation: u64,
    registry_generation: u64,
    metadata: Option<PrimaryEffectMetadata>,
}

impl PrimaryEffectRefresh {
    /// Retire older requests, optionally invalidating the effect definition.
    pub fn begin(
        &mut self,
        invalidate_metadata: bool,
        registry_generation: u64,
    ) -> (u64, Option<PrimaryEffectMetadata>) {
        self.generation = self.generation.wrapping_add(1);
        if invalidate_metadata || self.registry_generation != registry_generation {
            self.metadata = None;
        }
        self.registry_generation = registry_generation;
        (self.generation, self.metadata.clone())
    }

    /// Accept only the newest snapshot, including its metadata cache update.
    pub fn finish(
        &mut self,
        generation: u64,
        registry_generation: u64,
        view: Option<&PrimaryEffectView>,
    ) -> bool {
        if generation != self.generation || registry_generation != self.registry_generation {
            return false;
        }
        self.metadata = view.map(PrimaryEffectMetadata::from);
        true
    }
}
