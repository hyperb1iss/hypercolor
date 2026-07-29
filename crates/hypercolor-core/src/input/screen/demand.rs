use std::sync::Arc;

use thiserror::Error;

use super::{
    InputPublicationDemandRevision, RegisteredScreenBranchDemand, ScreenInputGraphGeneration,
    ScreenPublicationKind,
};

/// Immutable exact screen-publication demand delivered to capture workers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenPublicationDemandSnapshot {
    revision: InputPublicationDemandRevision,
    graph_generation: ScreenInputGraphGeneration,
    branches: Arc<[RegisteredScreenBranchDemand]>,
    compatibility_surface: Option<RegisteredScreenBranchDemand>,
    compatibility_zones: Option<RegisteredScreenBranchDemand>,
}

impl ScreenPublicationDemandSnapshot {
    /// Build a validated demand snapshot without merging independent branches.
    ///
    /// # Errors
    ///
    /// Rejects compatibility selections with the wrong payload kind or a
    /// selection that is absent from `branches`.
    pub fn try_new(
        revision: InputPublicationDemandRevision,
        graph_generation: ScreenInputGraphGeneration,
        branches: Arc<[RegisteredScreenBranchDemand]>,
        compatibility_surface: Option<RegisteredScreenBranchDemand>,
        compatibility_zones: Option<RegisteredScreenBranchDemand>,
    ) -> Result<Self, ScreenPublicationDemandError> {
        validate_compatibility(
            &branches,
            compatibility_surface.as_ref(),
            ScreenPublicationKindClass::Surface,
        )?;
        validate_compatibility(
            &branches,
            compatibility_zones.as_ref(),
            ScreenPublicationKindClass::Zones,
        )?;
        Ok(Self {
            revision,
            graph_generation,
            branches,
            compatibility_surface,
            compatibility_zones,
        })
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

    /// Ordinary Surface branch mirrored by compatibility consumers.
    #[must_use]
    pub const fn compatibility_surface(&self) -> Option<&RegisteredScreenBranchDemand> {
        self.compatibility_surface.as_ref()
    }

    /// Ordinary Zones branch mirrored by compatibility consumers.
    #[must_use]
    pub const fn compatibility_zones(&self) -> Option<&RegisteredScreenBranchDemand> {
        self.compatibility_zones.as_ref()
    }

    /// Whether any exact branch remains registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.branches.is_empty()
    }
}

#[derive(Clone, Copy)]
enum ScreenPublicationKindClass {
    Surface,
    Zones,
}

fn validate_compatibility(
    branches: &[RegisteredScreenBranchDemand],
    selection: Option<&RegisteredScreenBranchDemand>,
    expected: ScreenPublicationKindClass,
) -> Result<(), ScreenPublicationDemandError> {
    let Some(selection) = selection else {
        return Ok(());
    };
    let matches_kind = matches!(
        (expected, selection.request().kind()),
        (
            ScreenPublicationKindClass::Surface,
            ScreenPublicationKind::Surface
        ) | (
            ScreenPublicationKindClass::Zones,
            ScreenPublicationKind::Zones { .. }
        )
    );
    if !matches_kind {
        return Err(ScreenPublicationDemandError::CompatibilityKindMismatch);
    }
    if !branches.contains(selection) {
        return Err(ScreenPublicationDemandError::CompatibilityBranchMissing);
    }
    Ok(())
}

/// Structural failure while constructing an exact demand snapshot.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ScreenPublicationDemandError {
    /// Compatibility selection does not match its declared payload lane.
    #[error("screen compatibility selection has the wrong publication kind")]
    CompatibilityKindMismatch,
    /// Compatibility selection is not independently registered.
    #[error("screen compatibility selection is absent from exact branch demand")]
    CompatibilityBranchMissing,
}
