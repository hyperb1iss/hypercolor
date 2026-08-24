use std::sync::Arc;

use super::{
    InputPublicationDemandRevision, RegisteredScreenBranchDemand, ScreenInputGraphGeneration,
};

/// Immutable exact screen-publication demand delivered to capture workers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenPublicationDemandSnapshot {
    revision: InputPublicationDemandRevision,
    graph_generation: ScreenInputGraphGeneration,
    branches: Arc<[RegisteredScreenBranchDemand]>,
}

impl ScreenPublicationDemandSnapshot {
    /// Build a demand snapshot without merging independent branches.
    #[must_use]
    pub const fn new(
        revision: InputPublicationDemandRevision,
        graph_generation: ScreenInputGraphGeneration,
        branches: Arc<[RegisteredScreenBranchDemand]>,
    ) -> Self {
        Self {
            revision,
            graph_generation,
            branches,
        }
    }

    /// Authoritative demand revision carried across worker handoff.
    #[must_use]
    pub const fn revision(&self) -> InputPublicationDemandRevision {
        self.revision
    }

    /// Input-graph generation fencing source replacement.
    #[must_use]
    pub const fn graph_generation(&self) -> ScreenInputGraphGeneration {
        self.graph_generation
    }

    /// Every independently registered unresolved branch.
    #[must_use]
    pub const fn branches(&self) -> &Arc<[RegisteredScreenBranchDemand]> {
        &self.branches
    }

    /// Whether any exact branch remains registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.branches.is_empty()
    }
}
