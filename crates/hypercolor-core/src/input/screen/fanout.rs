//! Immutable CPU publication routing prepared from one committed authority.

use std::mem::size_of;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;

use super::reducer::branch_requires_materialization;
use super::{
    CaptureCadence, CaptureCadenceError, CaptureFrame, CapturePacer, CpuReductionError,
    CpuReductionExecutor, CpuSurfaceMaterializationError, CpuZoneMaterializationError,
    PreparedCpuMaterializationWorkspace, PreparedCpuReductionBatch, PreparedCpuSurfaceMaterializer,
    PreparedCpuZoneMaterializer, PreparedScreenPublication, RawCaptureSurface,
    ResolvedScreenPublicationDescriptor, ScreenBranchPublisher, ScreenCapturePlan,
    ScreenCommittedState, ScreenPayloadKind, ScreenPhysicalReductionDescriptor,
    ScreenPlanGeneration, ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError,
    ScreenPublicationKind, ScreenPublicationMetadata, ScreenWorkerBinding,
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
    descriptor: ResolvedScreenPublicationDescriptor,
    publisher: Option<ScreenBranchPublisher>,
    surface_materializer: Option<PreparedCpuSurfaceMaterializer>,
    zone_materializer: Option<PreparedCpuZoneMaterializer>,
    cadence: CaptureCadence,
    pacer: CapturePacer,
    next_due_at: Option<Instant>,
    pending_due: bool,
    last_accepted_sequence: Option<u64>,
}

impl PreparedCpuLogicalFanout {
    /// Exact branch behavior selected at preparation.
    #[must_use]
    pub const fn kind(&self) -> PreparedCpuLogicalFanoutKind {
        self.kind
    }

    /// Descriptor-bound publisher cached from the committed snapshot.
    #[must_use]
    pub fn publisher(&self) -> &ScreenBranchPublisher {
        self.publisher
            .as_ref()
            .expect("bound CPU fanout routes retain publisher authority")
    }

    /// Exact logical descriptor owned by this route.
    #[must_use]
    pub const fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.descriptor
    }

    /// Prepared zone kernel when this route publishes Zones.
    #[must_use]
    pub const fn zone_materializer(&self) -> Option<&PreparedCpuZoneMaterializer> {
        self.zone_materializer.as_ref()
    }

    /// Prepared Surface processor when this route changes physical bytes.
    #[must_use]
    pub const fn surface_materializer(&self) -> Option<&PreparedCpuSurfaceMaterializer> {
        self.surface_materializer.as_ref()
    }

    /// Mutable plan-lifetime state for staging one Surface publication.
    #[must_use]
    pub fn surface_materializer_mut(&mut self) -> Option<&mut PreparedCpuSurfaceMaterializer> {
        self.surface_materializer.as_mut()
    }

    /// Mutable plan-lifetime state for staging one Zones publication.
    #[must_use]
    pub fn zone_materializer_mut(&mut self) -> Option<&mut PreparedCpuZoneMaterializer> {
        self.zone_materializer.as_mut()
    }

    fn observe_deadline(&mut self, now: Instant) -> Result<(), CaptureCadenceError> {
        let Some(deadline) = self.next_due_at else {
            self.next_due_at = Some(now);
            return Ok(());
        };
        if now >= deadline {
            self.pending_due = true;
            self.next_due_at = Some(self.pacer.advance_deadline(deadline, now)?);
        }
        Ok(())
    }

    fn accepts_sequence(&self, sequence: u64) -> bool {
        self.pending_due
            && self
                .last_accepted_sequence
                .is_none_or(|previous| sequence > previous)
    }
}

/// Outcome of one allocation-free CPU fanout scheduling pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuPublicationFanoutReport {
    published: usize,
    pressured: usize,
    needs_source: bool,
}

impl CpuPublicationFanoutReport {
    /// Logical branches accepted by the hub.
    #[must_use]
    pub const fn published(self) -> usize {
        self.published
    }

    /// Branch attempts skipped because every admitted slot was retained.
    #[must_use]
    pub const fn pressured(self) -> usize {
        self.pressured
    }

    /// Whether at least one due branch still needs a newer native frame.
    #[must_use]
    pub const fn needs_source(self) -> bool {
        self.needs_source
    }
}

/// Allocation-complete CPU fanout awaiting immutable runtime authority.
#[derive(Debug)]
pub struct PreparedCpuPublicationFanoutCandidate {
    batch: PreparedCpuReductionBatch,
    physical: Vec<PreparedCpuPhysicalFanout>,
    executor: Option<CpuReductionExecutor>,
    workspace: Option<PreparedCpuMaterializationWorkspace>,
    workspace_schedule: Vec<usize>,
    reservations: Vec<CpuPendingPublication>,
    publications: Vec<PreparedScreenPublication>,
    direct_batch_indices: Vec<Option<usize>>,
    allocation_byte_len: u64,
}

#[derive(Debug)]
struct CpuPendingPublication {
    physical_index: usize,
    branch_index: usize,
}

impl PreparedCpuPublicationFanoutCandidate {
    /// Prepare exact routing metadata and branch-local kernels before commit.
    ///
    /// # Errors
    ///
    /// Rejects substituted plan, batch, or workspace identities; malformed
    /// physical grouping; unsupported zone policy; and fallible plan-lifetime
    /// metadata allocation.
    pub fn prepare(
        batch: &PreparedCpuReductionBatch,
        workspace: &PreparedCpuMaterializationWorkspace,
        plan: &ScreenCapturePlan,
    ) -> Result<Self, CpuPublicationFanoutError> {
        if plan.generation() != batch.plan_generation() {
            return Err(CpuPublicationFanoutError::CandidatePlanGenerationMismatch {
                batch: batch.plan_generation(),
                candidate: plan.generation(),
            });
        }
        if workspace.plan_generation() != batch.plan_generation() || !workspace.belongs_to(batch) {
            return Err(CpuPublicationFanoutError::WorkspaceBatchMismatch);
        }
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
            let mut requires_workspace = demand.branch_indices().len() > 1;
            for &branch_index in demand.branch_indices() {
                let branch = plan.branches().get(branch_index).ok_or(
                    CpuPublicationFanoutError::BranchIndexOutOfBounds {
                        branch_index,
                        branch_count: plan.branches().len(),
                    },
                )?;
                let branch_descriptor = branch.descriptor();
                let cadence = CaptureCadence::new(branch.requested_hz().get())?;
                if branch_descriptor.physical() != descriptor {
                    return Err(CpuPublicationFanoutError::BranchPhysicalMismatch { branch_index });
                }
                let (kind, surface_materializer, zone_materializer) = match branch_descriptor.kind()
                {
                    ScreenPublicationKind::Surface
                        if branch_requires_materialization(branch_descriptor) =>
                    {
                        requires_workspace = true;
                        (
                            PreparedCpuLogicalFanoutKind::MaterializedSurface,
                            Some(PreparedCpuSurfaceMaterializer::prepare_stateful(
                                branch_descriptor,
                                batch.plan_generation(),
                            )?),
                            None,
                        )
                    }
                    ScreenPublicationKind::Surface => {
                        (PreparedCpuLogicalFanoutKind::DirectSurface, None, None)
                    }
                    ScreenPublicationKind::Zones { .. } => {
                        requires_workspace = true;
                        (
                            PreparedCpuLogicalFanoutKind::Zones,
                            None,
                            Some(PreparedCpuZoneMaterializer::prepare_stateful(
                                branch_descriptor,
                                batch.plan_generation(),
                            )?),
                        )
                    }
                };
                branches.push(PreparedCpuLogicalFanout {
                    kind,
                    descriptor: branch_descriptor.clone(),
                    publisher: None,
                    surface_materializer,
                    zone_materializer,
                    cadence,
                    pacer: cadence.pacer(),
                    next_due_at: None,
                    pending_due: false,
                    last_accepted_sequence: None,
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
        let mut workspace_schedule = Vec::new();
        workspace_schedule
            .try_reserve_exact(physical.len())
            .map_err(|_| CpuPublicationFanoutError::AllocationFailed)?;
        let branch_count = physical.iter().map(|route| route.branches.len()).sum();
        let mut reservations = Vec::new();
        reservations
            .try_reserve_exact(branch_count)
            .map_err(|_| CpuPublicationFanoutError::AllocationFailed)?;
        let mut publications = Vec::new();
        publications
            .try_reserve_exact(branch_count)
            .map_err(|_| CpuPublicationFanoutError::AllocationFailed)?;
        let mut direct_batch_indices = Vec::new();
        direct_batch_indices
            .try_reserve_exact(branch_count)
            .map_err(|_| CpuPublicationFanoutError::AllocationFailed)?;
        let allocation_byte_len = allocation_byte_len(&physical, physical.capacity())?
            .checked_add(checked_bytes::<usize>(workspace_schedule.capacity())?)
            .and_then(|bytes| {
                checked_bytes::<CpuPendingPublication>(reservations.capacity())
                    .ok()
                    .and_then(|reservation_bytes| bytes.checked_add(reservation_bytes))
            })
            .and_then(|bytes| {
                checked_bytes::<PreparedScreenPublication>(publications.capacity())
                    .ok()
                    .and_then(|publication_bytes| bytes.checked_add(publication_bytes))
            })
            .and_then(|bytes| {
                checked_bytes::<Option<usize>>(direct_batch_indices.capacity())
                    .ok()
                    .and_then(|schedule_bytes| bytes.checked_add(schedule_bytes))
            })
            .ok_or(CpuPublicationFanoutError::AllocationAccountingOverflow)?;
        Ok(Self {
            batch: batch.clone(),
            physical,
            executor: None,
            workspace: None,
            workspace_schedule,
            reservations,
            publications,
            direct_batch_indices,
            allocation_byte_len,
        })
    }

    fn attach_execution(
        mut self,
        executor: CpuReductionExecutor,
        workspace: PreparedCpuMaterializationWorkspace,
    ) -> Self {
        self.executor = Some(executor);
        self.workspace = Some(workspace);
        self
    }

    /// Heap bytes retained by unpublished routing metadata and kernels.
    #[must_use]
    pub const fn allocation_byte_len(&self) -> u64 {
        self.allocation_byte_len
    }

    /// Bind one unpublished candidate to an exact immutable authority snapshot.
    ///
    /// Successful binding only moves owned storage and clones retained `Arc`
    /// capabilities. It performs no heap allocation.
    ///
    /// # Errors
    ///
    /// Rejects stale authority, another source worker, or a substituted worker
    /// generation without exposing a partially bound fanout.
    pub fn bind(
        mut self,
        authority: Arc<ScreenCommittedState>,
        binding: &ScreenWorkerBinding,
    ) -> Result<PreparedCpuPublicationFanout, CpuPublicationFanoutError> {
        if authority.plan().generation() != self.batch.plan_generation() {
            return Err(CpuPublicationFanoutError::PlanGenerationMismatch {
                batch: self.batch.plan_generation(),
                authority: authority.plan().generation(),
            });
        }
        if binding.source_id() != &self.batch.source().epoch().source_id {
            return Err(CpuPublicationFanoutError::WorkerSourceMismatch);
        }
        if binding.plan_generation() != self.batch.plan_generation() {
            return Err(CpuPublicationFanoutError::WorkerPlanGenerationMismatch {
                candidate: self.batch.plan_generation(),
                binding: binding.plan_generation(),
            });
        }
        for route in &mut self.physical {
            for branch in &mut route.branches {
                branch.publisher = Some(authority.publisher(&branch.descriptor, binding)?);
                branch.next_due_at = Some(Instant::now());
            }
        }
        Ok(PreparedCpuPublicationFanout {
            authority,
            batch: self.batch,
            physical: self.physical,
            executor: self.executor,
            workspace: self.workspace,
            workspace_schedule: self.workspace_schedule,
            reservations: self.reservations,
            publications: self.publications,
            direct_batch_indices: self.direct_batch_indices,
            allocation_byte_len: self.allocation_byte_len,
        })
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

    /// Mutable logical routes for plan-lifetime branch processing state.
    #[must_use]
    pub fn branches_mut(&mut self) -> &mut [PreparedCpuLogicalFanout] {
        &mut self.branches
    }
}

/// Allocation-complete CPU routing snapshot for one source and plan generation.
#[derive(Debug)]
pub struct PreparedCpuPublicationFanout {
    authority: Arc<ScreenCommittedState>,
    batch: PreparedCpuReductionBatch,
    physical: Vec<PreparedCpuPhysicalFanout>,
    executor: Option<CpuReductionExecutor>,
    workspace: Option<PreparedCpuMaterializationWorkspace>,
    workspace_schedule: Vec<usize>,
    reservations: Vec<CpuPendingPublication>,
    publications: Vec<PreparedScreenPublication>,
    direct_batch_indices: Vec<Option<usize>>,
    allocation_byte_len: u64,
}

impl PreparedCpuPublicationFanout {
    /// Allocate an unpublished fanout candidate from one exact candidate plan.
    ///
    /// # Errors
    ///
    /// Returns the same validation and allocation failures as
    /// [`PreparedCpuPublicationFanoutCandidate::prepare`].
    pub fn prepare_candidate(
        batch: &PreparedCpuReductionBatch,
        workspace: &PreparedCpuMaterializationWorkspace,
        plan: &ScreenCapturePlan,
    ) -> Result<PreparedCpuPublicationFanoutCandidate, CpuPublicationFanoutError> {
        PreparedCpuPublicationFanoutCandidate::prepare(batch, workspace, plan)
    }

    /// Prepare a self-contained execution candidate with owned physical planes.
    ///
    /// # Errors
    ///
    /// Returns the same validation failures as [`Self::prepare_candidate`].
    pub fn prepare_executable_candidate(
        executor: &CpuReductionExecutor,
        batch: &PreparedCpuReductionBatch,
        workspace: PreparedCpuMaterializationWorkspace,
        plan: &ScreenCapturePlan,
    ) -> Result<PreparedCpuPublicationFanoutCandidate, CpuPublicationFanoutError> {
        let candidate = PreparedCpuPublicationFanoutCandidate::prepare(batch, &workspace, plan)?;
        Ok(candidate.attach_execution(executor.clone(), workspace))
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

    /// Mutable physical routes for plan-lifetime branch processing state.
    #[must_use]
    pub fn physical_mut(&mut self) -> &mut [PreparedCpuPhysicalFanout] {
        &mut self.physical
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

    /// Earliest branch deadline after the most recent scheduling pass.
    #[must_use]
    pub fn next_due_at(&self) -> Option<Instant> {
        self.physical
            .iter()
            .flat_map(|physical| physical.branches.iter())
            .filter_map(|branch| branch.next_due_at)
            .min()
    }

    /// Publish every independently due logical branch from one optional frame.
    ///
    /// Deadlines advance on attempts rather than successful delivery. Missing
    /// or stale native frames leave demand pending, while branch-local pressure
    /// consumes that cadence tick and preserves the previous publication.
    ///
    /// # Errors
    ///
    /// Rejects metadata-only fanouts, stale frames, reducer failures,
    /// substituted hub authority, and branch-processing failures.
    pub fn publish_due(
        &mut self,
        hub: &ScreenPublicationHub,
        frame: Option<&CaptureFrame<RawCaptureSurface>>,
        now: Instant,
        health: ScreenPublicationHealth,
    ) -> Result<CpuPublicationFanoutReport, CpuPublicationFanoutError> {
        self.observe_deadlines(now)?;
        self.publish_due_inner(hub, frame, now, health, None)
    }

    /// Publish only physical routes selected by an immutable preparation mask.
    ///
    /// Deadlines still advance for every logical branch so GPU-reduced routes
    /// and native fallback routes share one coherent scheduler. The returned
    /// source demand reflects only selected routes.
    ///
    /// # Errors
    ///
    /// Rejects a mask with another plan shape and preserves every error from
    /// [`Self::publish_due`].
    pub fn publish_due_masked(
        &mut self,
        hub: &ScreenPublicationHub,
        frame: Option<&CaptureFrame<RawCaptureSurface>>,
        now: Instant,
        health: ScreenPublicationHealth,
        physical_mask: &[bool],
    ) -> Result<CpuPublicationFanoutReport, CpuPublicationFanoutError> {
        if physical_mask.len() != self.physical.len() {
            return Err(CpuPublicationFanoutError::PhysicalMaskLengthMismatch {
                expected: self.physical.len(),
                actual: physical_mask.len(),
            });
        }
        self.observe_deadlines(now)?;
        self.publish_due_inner(hub, frame, now, health, Some(physical_mask))
    }

    /// Whether one physical route has a due logical consumer awaiting bytes.
    #[must_use]
    pub fn physical_pending(&self, physical_index: usize) -> bool {
        self.physical
            .get(physical_index)
            .is_some_and(|physical| physical.branches.iter().any(|branch| branch.pending_due))
    }

    fn observe_deadlines(&mut self, now: Instant) -> Result<(), CpuPublicationFanoutError> {
        for physical in &mut self.physical {
            for branch in &mut physical.branches {
                branch.observe_deadline(now)?;
            }
        }
        Ok(())
    }

    fn publish_due_inner(
        &mut self,
        hub: &ScreenPublicationHub,
        frame: Option<&CaptureFrame<RawCaptureSurface>>,
        now: Instant,
        health: ScreenPublicationHealth,
        physical_mask: Option<&[bool]>,
    ) -> Result<CpuPublicationFanoutReport, CpuPublicationFanoutError> {
        let Some(frame) = frame else {
            return Ok(CpuPublicationFanoutReport {
                needs_source: self.any_pending(physical_mask),
                ..CpuPublicationFanoutReport::default()
            });
        };
        let sequence = frame.metadata().sequence;
        let native_sequence =
            NonZeroU64::new(sequence).ok_or(CpuPublicationFanoutError::NativeSequenceZero)?;
        let current_authority = hub.committed_state();
        if !Arc::ptr_eq(&current_authority, &self.authority) {
            return Err(ScreenPublicationHubError::PublisherStale {
                expected: current_authority.plan().generation(),
                observed: self.authority.plan().generation(),
            }
            .into());
        }
        let executor = self
            .executor
            .as_ref()
            .ok_or(CpuPublicationFanoutError::ExecutionNotAttached)?;
        let workspace = self
            .workspace
            .as_mut()
            .ok_or(CpuPublicationFanoutError::ExecutionNotAttached)?;
        let mut report = CpuPublicationFanoutReport::default();
        let plan_generation = self.authority.plan().generation();
        self.reservations.clear();
        self.publications.clear();
        self.direct_batch_indices.clear();
        for (physical_index, physical) in self.physical.iter_mut().enumerate() {
            if physical_mask.is_some_and(|mask| !mask[physical_index]) {
                continue;
            }
            for (branch_index, branch) in physical.branches.iter_mut().enumerate() {
                if !branch.accepts_sequence(sequence) {
                    if branch.pending_due {
                        report.needs_source = true;
                    }
                    continue;
                }
                if now
                    > branch
                        .cadence
                        .freshness_deadline(frame.metadata().captured_at)?
                {
                    report.needs_source = true;
                    continue;
                }
                let metadata = publication_intent(branch, frame, native_sequence, plan_generation)?;
                let payload_kind = match branch.kind {
                    PreparedCpuLogicalFanoutKind::DirectSurface
                    | PreparedCpuLogicalFanoutKind::MaterializedSurface => {
                        ScreenPayloadKind::Surface
                    }
                    PreparedCpuLogicalFanoutKind::Zones => ScreenPayloadKind::Zones,
                };
                match hub.prepare_writable_publication(branch.publisher(), payload_kind, &metadata)
                {
                    Ok(publication) => {
                        self.reservations.push(CpuPendingPublication {
                            physical_index,
                            branch_index,
                        });
                        self.publications.push(publication);
                        self.direct_batch_indices.push(
                            physical
                                .workspace_index
                                .is_none()
                                .then_some(physical.batch_index),
                        );
                    }
                    Err(ScreenPublicationHubError::PublicationPressure { .. }) => {
                        branch.pending_due = false;
                        report.pressured += 1;
                    }
                    Err(error) => {
                        clear_pending_publications(
                            &mut self.reservations,
                            &mut self.publications,
                            &mut self.direct_batch_indices,
                        );
                        return Err(error.into());
                    }
                }
            }
        }

        self.workspace_schedule.clear();
        for pending in &self.reservations {
            let physical = &self.physical[pending.physical_index];
            let Some(workspace_index) = physical.workspace_index else {
                continue;
            };
            if self.workspace_schedule.last() != Some(&workspace_index)
                && workspace.completed_source_sequence(workspace_index) != Some(sequence)
            {
                self.workspace_schedule.push(workspace_index);
            }
        }
        if let Err(error) = executor.execute_aligned_publications(
            &self.batch,
            frame,
            workspace,
            &self.workspace_schedule,
            &self.direct_batch_indices,
            &mut self.publications,
        ) {
            clear_pending_publications(
                &mut self.reservations,
                &mut self.publications,
                &mut self.direct_batch_indices,
            );
            return Err(error.into());
        }

        let (physical_routes, reservations, publications) = (
            &mut self.physical,
            &self.reservations,
            &mut self.publications,
        );
        for reservation_index in 0..reservations.len() {
            let physical_index = reservations[reservation_index].physical_index;
            let branch_index = reservations[reservation_index].branch_index;
            let physical = &mut physical_routes[physical_index];
            let Some(workspace_index) = physical.workspace_index else {
                continue;
            };
            let Some(pixels) = workspace.pixels(workspace_index) else {
                discard_all_stages(physical_routes, reservations, plan_generation);
                clear_pending_publications(
                    &mut self.reservations,
                    &mut self.publications,
                    &mut self.direct_batch_indices,
                );
                return Err(CpuPublicationFanoutError::WorkspacePublicationUnavailable {
                    workspace_index,
                });
            };
            let descriptor = self
                .batch
                .descriptor(physical.batch_index)
                .expect("prepared fanout batch index is valid");
            let branch = &mut physical.branches[branch_index];
            let result = stage_workspace_publication(
                branch,
                descriptor,
                pixels,
                frame,
                plan_generation,
                &mut publications[reservation_index],
            );
            if let Err(error) = result {
                discard_all_stages(physical_routes, reservations, plan_generation);
                clear_pending_publications(
                    &mut self.reservations,
                    &mut self.publications,
                    &mut self.direct_batch_indices,
                );
                return Err(error);
            }
        }

        for pending in &self.reservations {
            let branch = &self.physical[pending.physical_index].branches[pending.branch_index];
            if let Err(error) = validate_branch_stage(branch, plan_generation) {
                discard_all_stages(&mut self.physical, &self.reservations, plan_generation);
                clear_pending_publications(
                    &mut self.reservations,
                    &mut self.publications,
                    &mut self.direct_batch_indices,
                );
                return Err(error);
            }
        }
        if let Err(error) = hub.finalize_writable_publications(&mut self.publications, now, health)
        {
            discard_all_stages(&mut self.physical, &self.reservations, plan_generation);
            clear_pending_publications(
                &mut self.reservations,
                &mut self.publications,
                &mut self.direct_batch_indices,
            );
            return Err(error.into());
        }
        for pending in &self.reservations {
            let branch = &mut self.physical[pending.physical_index].branches[pending.branch_index];
            commit_branch_stage(branch, plan_generation)
                .expect("validated branch stage commits after atomic hub publication");
            branch.pending_due = false;
            branch.last_accepted_sequence = Some(sequence);
            report.published += 1;
        }
        clear_pending_publications(
            &mut self.reservations,
            &mut self.publications,
            &mut self.direct_batch_indices,
        );
        report.needs_source |= self.any_pending(physical_mask);
        Ok(report)
    }

    /// Publish one already-reduced physical RGBA plane to its due logical
    /// consumers without re-running the native CPU reducer.
    ///
    /// The physical index belongs to this immutable fanout generation. Hub
    /// reservations, branch-local staging, and finalization remain atomic.
    ///
    /// # Errors
    ///
    /// Rejects stale authority, invalid route identity, malformed bytes,
    /// zero or stale sequences, publication pressure failures, and branch
    /// materialization failures.
    pub fn publish_prereduced_physical(
        &mut self,
        hub: &ScreenPublicationHub,
        physical_index: usize,
        pixels: &[u8],
        source_sequence: u64,
        captured_at: Instant,
        now: Instant,
        health: ScreenPublicationHealth,
    ) -> Result<CpuPublicationFanoutReport, CpuPublicationFanoutError> {
        let descriptor = self.physical_descriptor(physical_index).cloned().ok_or(
            CpuPublicationFanoutError::PhysicalIndexOutOfRange {
                index: physical_index,
                len: self.physical.len(),
            },
        )?;
        let expected = u64::from(descriptor.reduction_extent().width())
            .checked_mul(u64::from(descriptor.reduction_extent().height()))
            .and_then(|pixels| pixels.checked_mul(4))
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or(CpuPublicationFanoutError::PhysicalByteLengthOverflow)?;
        if pixels.len() != expected {
            return Err(CpuPublicationFanoutError::PhysicalByteLengthMismatch {
                expected,
                actual: pixels.len(),
            });
        }
        let native_sequence = NonZeroU64::new(source_sequence)
            .ok_or(CpuPublicationFanoutError::NativeSequenceZero)?;
        let current_authority = hub.committed_state();
        if !Arc::ptr_eq(&current_authority, &self.authority) {
            return Err(ScreenPublicationHubError::PublisherStale {
                expected: current_authority.plan().generation(),
                observed: self.authority.plan().generation(),
            }
            .into());
        }

        let plan_generation = self.authority.plan().generation();
        let mut report = CpuPublicationFanoutReport::default();
        self.reservations.clear();
        self.publications.clear();
        self.direct_batch_indices.clear();
        let physical = &mut self.physical[physical_index];
        for (branch_index, branch) in physical.branches.iter_mut().enumerate() {
            if !branch.accepts_sequence(source_sequence) {
                if branch.pending_due {
                    report.needs_source = true;
                }
                continue;
            }
            if now > branch.cadence.freshness_deadline(captured_at)? {
                report.needs_source = true;
                continue;
            }
            let metadata = ScreenPublicationMetadata::try_intent(
                branch.descriptor.source_epoch().clone(),
                plan_generation,
                native_sequence,
                captured_at,
                branch.cadence.freshness_deadline(captured_at)?,
            )?;
            let payload_kind = match branch.kind {
                PreparedCpuLogicalFanoutKind::DirectSurface
                | PreparedCpuLogicalFanoutKind::MaterializedSurface => ScreenPayloadKind::Surface,
                PreparedCpuLogicalFanoutKind::Zones => ScreenPayloadKind::Zones,
            };
            match hub.prepare_writable_publication(branch.publisher(), payload_kind, &metadata) {
                Ok(publication) => {
                    self.reservations.push(CpuPendingPublication {
                        physical_index,
                        branch_index,
                    });
                    self.publications.push(publication);
                }
                Err(ScreenPublicationHubError::PublicationPressure { .. }) => {
                    branch.pending_due = false;
                    report.pressured += 1;
                }
                Err(error) => {
                    clear_pending_publications(
                        &mut self.reservations,
                        &mut self.publications,
                        &mut self.direct_batch_indices,
                    );
                    return Err(error.into());
                }
            }
        }

        for reservation_index in 0..self.reservations.len() {
            let branch_index = self.reservations[reservation_index].branch_index;
            let branch = &mut self.physical[physical_index].branches[branch_index];
            if let Err(error) = stage_prereduced_publication(
                branch,
                &descriptor,
                pixels,
                captured_at,
                plan_generation,
                &mut self.publications[reservation_index],
            ) {
                discard_all_stages(&mut self.physical, &self.reservations, plan_generation);
                clear_pending_publications(
                    &mut self.reservations,
                    &mut self.publications,
                    &mut self.direct_batch_indices,
                );
                return Err(error);
            }
        }
        for pending in &self.reservations {
            let branch = &self.physical[pending.physical_index].branches[pending.branch_index];
            if let Err(error) = validate_branch_stage(branch, plan_generation) {
                discard_all_stages(&mut self.physical, &self.reservations, plan_generation);
                clear_pending_publications(
                    &mut self.reservations,
                    &mut self.publications,
                    &mut self.direct_batch_indices,
                );
                return Err(error);
            }
        }
        if let Err(error) = hub.finalize_writable_publications(&mut self.publications, now, health)
        {
            discard_all_stages(&mut self.physical, &self.reservations, plan_generation);
            clear_pending_publications(
                &mut self.reservations,
                &mut self.publications,
                &mut self.direct_batch_indices,
            );
            return Err(error.into());
        }
        for pending in &self.reservations {
            let branch = &mut self.physical[pending.physical_index].branches[pending.branch_index];
            commit_branch_stage(branch, plan_generation)
                .expect("validated pre-reduced branch stage commits after atomic publication");
            branch.pending_due = false;
            branch.last_accepted_sequence = Some(source_sequence);
            report.published += 1;
        }
        clear_pending_publications(
            &mut self.reservations,
            &mut self.publications,
            &mut self.direct_batch_indices,
        );
        report.needs_source |= self.physical_pending(physical_index);
        Ok(report)
    }

    fn any_pending(&self, physical_mask: Option<&[bool]>) -> bool {
        self.physical
            .iter()
            .enumerate()
            .filter(|(index, _)| physical_mask.is_none_or(|mask| mask[*index]))
            .flat_map(|(_, physical)| physical.branches.iter())
            .any(|branch| branch.pending_due)
    }
}

fn discard_all_stages(
    physical: &mut [PreparedCpuPhysicalFanout],
    reservations: &[CpuPendingPublication],
    plan_generation: ScreenPlanGeneration,
) {
    for pending in reservations {
        discard_branch_stage(
            &mut physical[pending.physical_index].branches[pending.branch_index],
            plan_generation,
        )
        .expect("prepared branch stage belongs to the active plan generation");
    }
}

fn clear_pending_publications(
    reservations: &mut Vec<CpuPendingPublication>,
    publications: &mut Vec<PreparedScreenPublication>,
    direct_batch_indices: &mut Vec<Option<usize>>,
) {
    publications.clear();
    reservations.clear();
    direct_batch_indices.clear();
}

fn stage_workspace_publication(
    branch: &mut PreparedCpuLogicalFanout,
    physical_descriptor: &ScreenPhysicalReductionDescriptor,
    physical_pixels: &[u8],
    frame: &CaptureFrame<RawCaptureSurface>,
    plan_generation: ScreenPlanGeneration,
    publication: &mut PreparedScreenPublication,
) -> Result<(), CpuPublicationFanoutError> {
    match branch.kind {
        PreparedCpuLogicalFanoutKind::DirectSurface => {
            let output = publication
                .surface_pixels_mut()
                .map_err(CpuPublicationFanoutError::Publisher)?;
            if output.len() != physical_pixels.len() {
                return Err(CpuPublicationFanoutError::DirectSurfaceLengthMismatch {
                    expected: physical_pixels.len(),
                    actual: output.len(),
                });
            }
            output.copy_from_slice(physical_pixels);
        }
        PreparedCpuLogicalFanoutKind::MaterializedSurface => {
            branch
                .surface_materializer
                .as_mut()
                .expect("materialized Surface routes own a processor")
                .stage(
                    plan_generation,
                    physical_descriptor,
                    physical_pixels,
                    frame.metadata().captured_at,
                    publication,
                )?;
        }
        PreparedCpuLogicalFanoutKind::Zones => {
            let staged = branch
                .zone_materializer
                .as_mut()
                .expect("Zones routes own a processor")
                .stage(
                    plan_generation,
                    physical_descriptor,
                    physical_pixels,
                    frame.metadata().captured_at,
                    publication,
                )?;
            let columns = std::num::NonZeroU32::new(staged.columns())
                .ok_or(CpuPublicationFanoutError::InvalidEffectiveZoneShape)?;
            let rows = std::num::NonZeroU32::new(staged.rows())
                .ok_or(CpuPublicationFanoutError::InvalidEffectiveZoneShape)?;
            publication.set_effective_zone_shape(columns, rows)?;
        }
    }
    Ok(())
}

fn stage_prereduced_publication(
    branch: &mut PreparedCpuLogicalFanout,
    physical_descriptor: &ScreenPhysicalReductionDescriptor,
    physical_pixels: &[u8],
    captured_at: Instant,
    plan_generation: ScreenPlanGeneration,
    publication: &mut PreparedScreenPublication,
) -> Result<(), CpuPublicationFanoutError> {
    match branch.kind {
        PreparedCpuLogicalFanoutKind::DirectSurface => {
            let output = publication
                .surface_pixels_mut()
                .map_err(CpuPublicationFanoutError::Publisher)?;
            if output.len() != physical_pixels.len() {
                return Err(CpuPublicationFanoutError::DirectSurfaceLengthMismatch {
                    expected: physical_pixels.len(),
                    actual: output.len(),
                });
            }
            output.copy_from_slice(physical_pixels);
        }
        PreparedCpuLogicalFanoutKind::MaterializedSurface => {
            branch
                .surface_materializer
                .as_mut()
                .expect("materialized Surface routes own a processor")
                .stage(
                    plan_generation,
                    physical_descriptor,
                    physical_pixels,
                    captured_at,
                    publication,
                )?;
        }
        PreparedCpuLogicalFanoutKind::Zones => {
            let staged = branch
                .zone_materializer
                .as_mut()
                .expect("Zones routes own a processor")
                .stage(
                    plan_generation,
                    physical_descriptor,
                    physical_pixels,
                    captured_at,
                    publication,
                )?;
            let columns = std::num::NonZeroU32::new(staged.columns())
                .ok_or(CpuPublicationFanoutError::InvalidEffectiveZoneShape)?;
            let rows = std::num::NonZeroU32::new(staged.rows())
                .ok_or(CpuPublicationFanoutError::InvalidEffectiveZoneShape)?;
            publication.set_effective_zone_shape(columns, rows)?;
        }
    }
    Ok(())
}

fn commit_branch_stage(
    branch: &mut PreparedCpuLogicalFanout,
    plan_generation: ScreenPlanGeneration,
) -> Result<(), CpuPublicationFanoutError> {
    match branch.kind {
        PreparedCpuLogicalFanoutKind::DirectSurface => Ok(()),
        PreparedCpuLogicalFanoutKind::MaterializedSurface => branch
            .surface_materializer
            .as_mut()
            .expect("materialized Surface routes own a processor")
            .commit_staged(plan_generation)
            .map_err(Into::into),
        PreparedCpuLogicalFanoutKind::Zones => branch
            .zone_materializer
            .as_mut()
            .expect("Zones routes own a processor")
            .commit_staged(plan_generation)
            .map_err(Into::into),
    }
}

fn validate_branch_stage(
    branch: &PreparedCpuLogicalFanout,
    plan_generation: ScreenPlanGeneration,
) -> Result<(), CpuPublicationFanoutError> {
    match branch.kind {
        PreparedCpuLogicalFanoutKind::DirectSurface => Ok(()),
        PreparedCpuLogicalFanoutKind::MaterializedSurface => branch
            .surface_materializer
            .as_ref()
            .expect("materialized Surface routes own a processor")
            .validate_staged(plan_generation)
            .map_err(Into::into),
        PreparedCpuLogicalFanoutKind::Zones => branch
            .zone_materializer
            .as_ref()
            .expect("Zones routes own a processor")
            .validate_staged(plan_generation)
            .map_err(Into::into),
    }
}

fn discard_branch_stage(
    branch: &mut PreparedCpuLogicalFanout,
    plan_generation: ScreenPlanGeneration,
) -> Result<(), CpuPublicationFanoutError> {
    match branch.kind {
        PreparedCpuLogicalFanoutKind::DirectSurface => Ok(()),
        PreparedCpuLogicalFanoutKind::MaterializedSurface => branch
            .surface_materializer
            .as_mut()
            .expect("materialized Surface routes own a processor")
            .discard_staged(plan_generation)
            .map_err(Into::into),
        PreparedCpuLogicalFanoutKind::Zones => branch
            .zone_materializer
            .as_mut()
            .expect("Zones routes own a processor")
            .discard_staged(plan_generation)
            .map_err(Into::into),
    }
}

fn publication_intent(
    branch: &PreparedCpuLogicalFanout,
    frame: &CaptureFrame<RawCaptureSurface>,
    native_sequence: NonZeroU64,
    plan_generation: ScreenPlanGeneration,
) -> Result<ScreenPublicationMetadata, CpuPublicationFanoutError> {
    Ok(ScreenPublicationMetadata::try_intent(
        branch.descriptor.source_epoch().clone(),
        plan_generation,
        native_sequence,
        frame.metadata().captured_at,
        branch
            .cadence
            .freshness_deadline(frame.metadata().captured_at)?,
    )?)
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
            if let Some(materializer) = &branch.surface_materializer {
                total = total
                    .checked_add(materializer.precomputed_byte_len())
                    .ok_or(CpuPublicationFanoutError::AllocationAccountingOverflow)?;
            }
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
    /// Metadata-only fanout preparation omitted owned execution resources.
    #[error("CPU publication fanout has no attached executor or workspace")]
    ExecutionNotAttached,
    /// Hub metadata requires positive native sequence identity.
    #[error("CPU publication fanout received native sequence zero")]
    NativeSequenceZero,
    /// A runtime physical selection mask belongs to another prepared shape.
    #[error("CPU fanout physical mask has {actual} entries; expected {expected}")]
    PhysicalMaskLengthMismatch { expected: usize, actual: usize },
    /// A pre-reduced result names no route in this immutable generation.
    #[error("CPU fanout physical index {index} is outside {len} prepared routes")]
    PhysicalIndexOutOfRange { index: usize, len: usize },
    /// A physical output plane cannot be addressed by this process.
    #[error("CPU fanout physical output byte length exceeds usize")]
    PhysicalByteLengthOverflow,
    /// A pre-reduced plane does not match its exact physical descriptor.
    #[error("pre-reduced physical plane has {actual} bytes; expected {expected}")]
    PhysicalByteLengthMismatch { expected: usize, actual: usize },
    /// A scheduled retained plane did not contain completed physical bytes.
    #[error("CPU publication workspace plane {workspace_index} is unavailable")]
    WorkspacePublicationUnavailable { workspace_index: usize },
    /// A direct logical Surface slot differs from its shared physical plane.
    #[error("direct CPU Surface has {actual} bytes; expected {expected}")]
    DirectSurfaceLengthMismatch { expected: usize, actual: usize },
    /// Stateful zone processing produced a zero effective dimension.
    #[error("CPU publication fanout produced an invalid effective zone shape")]
    InvalidEffectiveZoneShape,
    /// Prepared physical work and the unpublished candidate name different generations.
    #[error("CPU fanout batch generation {batch:?} does not match candidate {candidate:?}")]
    CandidatePlanGenerationMismatch {
        batch: ScreenPlanGeneration,
        candidate: ScreenPlanGeneration,
    },
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
    /// Worker authority belongs to another committed plan generation.
    #[error("CPU fanout candidate generation {candidate:?} does not match worker {binding:?}")]
    WorkerPlanGenerationMismatch {
        candidate: ScreenPlanGeneration,
        binding: ScreenPlanGeneration,
    },
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
    /// CPU physical reduction failed.
    #[error(transparent)]
    Reduction(#[from] CpuReductionError),
    /// Branch cadence could not advance or represent freshness.
    #[error(transparent)]
    Cadence(#[from] CaptureCadenceError),
    /// A Zones branch could not prepare its exact sampling kernel.
    #[error(transparent)]
    ZoneMaterializer(#[from] CpuZoneMaterializationError),
    /// A Surface branch could not prepare its exact processor.
    #[error(transparent)]
    SurfaceMaterializer(#[from] CpuSurfaceMaterializationError),
    /// Plan-lifetime fanout metadata could not be reserved.
    #[error("failed to allocate CPU publication fanout metadata")]
    AllocationFailed,
    /// Fanout-owned heap byte accounting exceeded the u64 ledger.
    #[error("CPU publication fanout allocation accounting overflowed")]
    AllocationAccountingOverflow,
}
