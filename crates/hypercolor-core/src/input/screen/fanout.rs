//! Immutable CPU publication routing prepared from one committed authority.

use std::mem::size_of;
use std::sync::Arc;

use thiserror::Error;

use super::reducer::branch_requires_materialization;
use super::{
    CpuZoneMaterializationError, PreparedCpuMaterializationWorkspace, PreparedCpuReductionBatch,
    PreparedCpuZoneMaterializer, ResolvedScreenPublicationDescriptor, ScreenBranchPublisher,
    ScreenCommittedState, ScreenPhysicalReductionDescriptor, ScreenPlanGeneration,
    ScreenPublicationHubError, ScreenPublicationKind, ScreenWorkerBinding,
};

/// Plan-time routing class for one exact logical branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedCpuLogicalFanoutKind {
    /// Identity Surface bytes can be reduced directly into its writable slot.
    DirectSurface,
    /// Surface policy requires a branch-local processor over physical bytes.
    MaterializedSurface,
    /// Zones use a prepared exact sampling kernel over physical bytes.
    Zones,
}

/// One cached logical publisher and its optional CPU zone materializer.
#[derive(Debug)]
pub struct PreparedCpuLogicalFanout {
    kind: PreparedCpuLogicalFanoutKind,
    publisher: ScreenBranchPublisher,
    zone_materializer: Option<PreparedCpuZoneMaterializer>,
}

impl PreparedCpuLogicalFanout {
    /// Exact branch behavior selected at preparation.
    #[must_use]
    pub const fn kind(&self) -> PreparedCpuLogicalFanoutKind {
        self.kind
    }

    /// Descriptor-bound publisher cached from the committed snapshot.
    #[must_use]
    pub const fn publisher(&self) -> &ScreenBranchPublisher {
        &self.publisher
    }

    /// Exact logical descriptor owned by this route.
    #[must_use]
    pub fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        self.publisher.descriptor()
    }

    /// Prepared zone kernel when this route publishes Zones.
    #[must_use]
    pub const fn zone_materializer(&self) -> Option<&PreparedCpuZoneMaterializer> {
        self.zone_materializer.as_ref()
    }
}

/// All logical branches consuming one canonical physical reduction.
#[derive(Debug)]
pub struct PreparedCpuPhysicalFanout {
    batch_index: usize,
    workspace_index: Option<usize>,
    branches: Vec<PreparedCpuLogicalFanout>,
}

impl PreparedCpuPhysicalFanout {
    /// Canonical prepared CPU batch index for this physical key.
    #[must_use]
    pub const fn batch_index(&self) -> usize {
        self.batch_index
    }

    /// Retained plane index when any logical branch needs materialization.
    #[must_use]
    pub const fn workspace_index(&self) -> Option<usize> {
        self.workspace_index
    }

    /// Canonically ordered logical branches consuming this physical key.
    #[must_use]
    pub fn branches(&self) -> &[PreparedCpuLogicalFanout] {
        &self.branches
    }
}

/// Allocation-complete CPU routing snapshot for one source and plan generation.
#[derive(Debug)]
pub struct PreparedCpuPublicationFanout {
    authority: Arc<ScreenCommittedState>,
    batch: PreparedCpuReductionBatch,
    physical: Vec<PreparedCpuPhysicalFanout>,
    allocation_byte_len: u64,
}

impl PreparedCpuPublicationFanout {
    /// Cache exact physical-to-logical routes and publisher capabilities once.
    ///
    /// # Errors
    ///
    /// Rejects substituted plan, batch, workspace, source, or worker authority;
    /// malformed physical grouping; unsupported zone policy; and fallible
    /// plan-lifetime metadata allocation.
    pub fn prepare(
        batch: &PreparedCpuReductionBatch,
        workspace: &PreparedCpuMaterializationWorkspace,
        authority: Arc<ScreenCommittedState>,
        binding: &ScreenWorkerBinding,
    ) -> Result<Self, CpuPublicationFanoutError> {
        if authority.plan().generation() != batch.plan_generation() {
            return Err(CpuPublicationFanoutError::PlanGenerationMismatch {
                batch: batch.plan_generation(),
                authority: authority.plan().generation(),
            });
        }
        if workspace.plan_generation() != batch.plan_generation() || !workspace.belongs_to(batch) {
            return Err(CpuPublicationFanoutError::WorkspaceBatchMismatch);
        }
        if binding.source_id() != &batch.source().epoch().source_id {
            return Err(CpuPublicationFanoutError::WorkerSourceMismatch);
        }
        let plan = authority.plan();
        let mut physical = Vec::new();
        physical
            .try_reserve_exact(batch.len())
            .map_err(|_| CpuPublicationFanoutError::AllocationFailed)?;
        let mut workspace_cursor = 0;
        for batch_index in 0..batch.len() {
            let descriptor = batch
                .descriptor(batch_index)
                .expect("prepared batch index is bounded by its length");
            let reduction_index = plan
                .physical_reductions()
                .binary_search_by(|demand| demand.descriptor().cmp(descriptor))
                .map_err(|_| CpuPublicationFanoutError::PhysicalPlanMismatch { batch_index })?;
            let demand = &plan.physical_reductions()[reduction_index];
            let workspace_index = match workspace.batch_index(workspace_cursor) {
                Some(observed) if observed < batch_index => {
                    return Err(CpuPublicationFanoutError::WorkspaceOrderMismatch);
                }
                Some(observed) if observed == batch_index => {
                    if workspace.physical_descriptor(workspace_cursor) != Some(descriptor) {
                        return Err(CpuPublicationFanoutError::WorkspacePhysicalMismatch {
                            workspace_index: workspace_cursor,
                        });
                    }
                    let selected = Some(workspace_cursor);
                    workspace_cursor += 1;
                    selected
                }
                _ => None,
            };
            let mut branches = Vec::new();
            branches
                .try_reserve_exact(demand.branch_indices().len())
                .map_err(|_| CpuPublicationFanoutError::AllocationFailed)?;
            let mut requires_workspace = false;
            for &branch_index in demand.branch_indices() {
                let branch = plan.branches().get(branch_index).ok_or(
                    CpuPublicationFanoutError::BranchIndexOutOfBounds {
                        branch_index,
                        branch_count: plan.branches().len(),
                    },
                )?;
                let branch_descriptor = branch.descriptor();
                if branch_descriptor.physical() != descriptor {
                    return Err(CpuPublicationFanoutError::BranchPhysicalMismatch { branch_index });
                }
                let publisher = authority.publisher(branch_descriptor, binding)?;
                let (kind, zone_materializer) = match branch_descriptor.kind() {
                    ScreenPublicationKind::Surface
                        if branch_requires_materialization(branch_descriptor) =>
                    {
                        requires_workspace = true;
                        (PreparedCpuLogicalFanoutKind::MaterializedSurface, None)
                    }
                    ScreenPublicationKind::Surface => {
                        (PreparedCpuLogicalFanoutKind::DirectSurface, None)
                    }
                    ScreenPublicationKind::Zones { .. } => {
                        requires_workspace = true;
                        (
                            PreparedCpuLogicalFanoutKind::Zones,
                            Some(PreparedCpuZoneMaterializer::prepare(branch_descriptor)?),
                        )
                    }
                };
                branches.push(PreparedCpuLogicalFanout {
                    kind,
                    publisher,
                    zone_materializer,
                });
            }
            if requires_workspace != workspace_index.is_some() {
                return Err(CpuPublicationFanoutError::WorkspaceSelectionMismatch {
                    batch_index,
                    required: requires_workspace,
                });
            }
            physical.push(PreparedCpuPhysicalFanout {
                batch_index,
                workspace_index,
                branches,
            });
        }
        if workspace_cursor != workspace.len() {
            return Err(CpuPublicationFanoutError::WorkspaceOrderMismatch);
        }
        let allocation_byte_len = allocation_byte_len(&physical, physical.capacity())?;
        Ok(Self {
            authority,
            batch: batch.clone(),
            physical,
            allocation_byte_len,
        })
    }

    /// Exact committed plan generation retained by this routing snapshot.
    #[must_use]
    pub fn plan_generation(&self) -> ScreenPlanGeneration {
        self.authority.plan().generation()
    }

    /// Prepared physical batch retained by this routing snapshot.
    #[must_use]
    pub const fn batch(&self) -> &PreparedCpuReductionBatch {
        &self.batch
    }

    /// Number of canonical physical routes for this source.
    #[must_use]
    pub fn len(&self) -> usize {
        self.physical.len()
    }

    /// Whether this source has no committed physical work.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.physical.is_empty()
    }

    /// Canonically ordered physical routes.
    #[must_use]
    pub fn physical(&self) -> &[PreparedCpuPhysicalFanout] {
        &self.physical
    }

    /// Exact physical descriptor for one cached route.
    #[must_use]
    pub fn physical_descriptor(
        &self,
        physical_index: usize,
    ) -> Option<&ScreenPhysicalReductionDescriptor> {
        self.physical
            .get(physical_index)
            .and_then(|route| self.batch.descriptor(route.batch_index))
    }

    /// Number of exact logical branches cached across all physical routes.
    #[must_use]
    pub fn branch_count(&self) -> usize {
        self.physical
            .iter()
            .map(|physical| physical.branches.len())
            .sum()
    }

    /// Heap bytes retained exclusively by fanout metadata and sampling kernels.
    #[must_use]
    pub const fn allocation_byte_len(&self) -> u64 {
        self.allocation_byte_len
    }
}

fn allocation_byte_len(
    physical: &[PreparedCpuPhysicalFanout],
    physical_capacity: usize,
) -> Result<u64, CpuPublicationFanoutError> {
    let mut total = checked_bytes::<PreparedCpuPhysicalFanout>(physical_capacity)?;
    for route in physical {
        total = total
            .checked_add(checked_bytes::<PreparedCpuLogicalFanout>(
                route.branches.capacity(),
            )?)
            .ok_or(CpuPublicationFanoutError::AllocationAccountingOverflow)?;
        for branch in &route.branches {
            if let Some(materializer) = &branch.zone_materializer {
                total = total
                    .checked_add(materializer.precomputed_byte_len())
                    .ok_or(CpuPublicationFanoutError::AllocationAccountingOverflow)?;
            }
        }
    }
    Ok(total)
}

fn checked_bytes<T>(count: usize) -> Result<u64, CpuPublicationFanoutError> {
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(size_of::<T>())
                .ok()
                .and_then(|item_size| count.checked_mul(item_size))
        })
        .ok_or(CpuPublicationFanoutError::AllocationAccountingOverflow)
}

/// Preparation failure for immutable CPU publication fanout.
#[derive(Debug, Error)]
pub enum CpuPublicationFanoutError {
    /// Prepared physical work and committed authority name different generations.
    #[error("CPU fanout batch generation {batch:?} does not match authority {authority:?}")]
    PlanGenerationMismatch {
        batch: ScreenPlanGeneration,
        authority: ScreenPlanGeneration,
    },
    /// Retained physical storage was prepared from another batch identity.
    #[error("CPU fanout workspace belongs to another prepared batch")]
    WorkspaceBatchMismatch,
    /// Worker authority names another capture source.
    #[error("CPU fanout worker binding names another source")]
    WorkerSourceMismatch,
    /// A prepared physical key is absent from the committed plan.
    #[error("CPU fanout physical key {batch_index} is absent from committed authority")]
    PhysicalPlanMismatch { batch_index: usize },
    /// Workspace indices are not a canonical subsequence of physical batch indices.
    #[error("CPU fanout workspace order does not match the prepared physical batch")]
    WorkspaceOrderMismatch,
    /// One retained plane names another physical descriptor.
    #[error("CPU fanout workspace plane {workspace_index} names another physical key")]
    WorkspacePhysicalMismatch { workspace_index: usize },
    /// A committed physical group names an invalid branch index.
    #[error("CPU fanout branch index {branch_index} escapes {branch_count} committed branches")]
    BranchIndexOutOfBounds {
        branch_index: usize,
        branch_count: usize,
    },
    /// A grouped branch does not consume its enclosing physical key.
    #[error("CPU fanout branch {branch_index} names another physical key")]
    BranchPhysicalMismatch { branch_index: usize },
    /// Workspace selection disagrees with branch-local processing requirements.
    #[error("CPU fanout workspace selection for key {batch_index} must be {required}")]
    WorkspaceSelectionMismatch { batch_index: usize, required: bool },
    /// Publisher authority could not bind one committed branch.
    #[error(transparent)]
    Publisher(#[from] ScreenPublicationHubError),
    /// A Zones branch could not prepare its exact sampling kernel.
    #[error(transparent)]
    ZoneMaterializer(#[from] CpuZoneMaterializationError),
    /// Plan-lifetime fanout metadata could not be reserved.
    #[error("failed to allocate CPU publication fanout metadata")]
    AllocationFailed,
    /// Fanout-owned heap byte accounting exceeded the u64 ledger.
    #[error("CPU publication fanout allocation accounting overflowed")]
    AllocationAccountingOverflow,
}
