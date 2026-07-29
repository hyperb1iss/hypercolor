//! Canonical grouping, admission, and continuity-safe screen-plan transitions.

use std::collections::{HashMap, HashSet};
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use thiserror::Error;

use super::hub::{
    ScreenCommitActivation, ScreenCommittedState, ScreenPublicationHub,
    ScreenPublicationRetirement, ScreenPublicationSlotPolicy, ScreenRetirementCharge,
};
use super::reducer::branch_requires_materialization;
use super::{
    CaptureSourceId, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ScreenPhysicalReductionDescriptor, ScreenPublicationKind, ScreenPublicationResidency,
};

const TARGET_PIXEL_BYTES: u64 = 4;
const ZONE_COLOR_BYTES: u64 = 3;

/// Generation of the independently committed screen-publication plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenPlanGeneration(u64);

impl ScreenPlanGeneration {
    /// Numeric generation value. Zero represents the initial empty plan.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, ScreenPlanError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(ScreenPlanError::GenerationExhausted)
    }
}

/// Structural input-graph generation observed by a plan transition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenInputGraphGeneration(u64);

impl ScreenInputGraphGeneration {
    /// Construct a graph-generation fence.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Numeric graph generation.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Monotonic revision of the authoritative input-publication demand snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputPublicationDemandRevision(u64);

impl InputPublicationDemandRevision {
    /// Construct a revision supplied by the authoritative demand publisher.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Raw monotonic revision value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Advance to the next representable revision.
    ///
    /// # Errors
    ///
    /// Returns an exhaustion error at `u64::MAX`.
    pub fn next(self) -> Result<Self, ScreenPlanError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(ScreenPlanError::DemandRevisionExhausted)
    }
}

/// Opaque identity separating concurrent preparations for the same generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScreenPlanTransactionId(NonZeroU64);

impl ScreenPlanTransactionId {
    /// Opaque numeric value used at the worker protocol boundary.
    #[must_use]
    pub const fn get(self) -> NonZeroU64 {
        self.0
    }
}

/// One canonical byte-equivalent branch and its maximum requested cadence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenBranchDemand {
    descriptor: ResolvedScreenPublicationDescriptor,
    requested_hz: NonZeroU32,
}

impl ScreenBranchDemand {
    /// Complete derived byte-equivalence descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.descriptor
    }

    /// Maximum cadence requested by all equal registrations.
    #[must_use]
    pub const fn requested_hz(&self) -> NonZeroU32 {
        self.requested_hz
    }
}

/// One shared physical reduction and its dependent logical branch indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenPhysicalReductionDemand {
    descriptor: ScreenPhysicalReductionDescriptor,
    branch_indices: Vec<usize>,
    requested_hz: NonZeroU32,
}

impl ScreenPhysicalReductionDemand {
    /// Complete physical byte-equivalence descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> &ScreenPhysicalReductionDescriptor {
        &self.descriptor
    }

    /// Canonical branch indices consuming this physical reduction.
    #[must_use]
    pub fn branch_indices(&self) -> &[usize] {
        &self.branch_indices
    }

    /// Maximum cadence across dependent logical branches.
    #[must_use]
    pub const fn requested_hz(&self) -> NonZeroU32 {
        self.requested_hz
    }
}

/// Ordinary compatibility outputs selected from the canonical branch plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCompatibilitySelection {
    surface: ResolvedScreenPublicationDescriptor,
    zones: Option<ResolvedScreenPublicationDescriptor>,
}

impl ScreenCompatibilitySelection {
    /// Select one ordinary surface and an optional ordinary zones branch.
    ///
    /// # Errors
    ///
    /// Rejects descriptors whose kinds do not match their compatibility role.
    pub fn try_new(
        surface: ResolvedScreenPublicationDescriptor,
        zones: Option<ResolvedScreenPublicationDescriptor>,
    ) -> Result<Self, ScreenPlanError> {
        if !matches!(surface.kind(), ScreenPublicationKind::Surface) {
            return Err(ScreenPlanError::CompatibilitySurfaceKindMismatch);
        }
        if zones.as_ref().is_some_and(|descriptor| {
            !matches!(descriptor.kind(), ScreenPublicationKind::Zones { .. })
        }) {
            return Err(ScreenPlanError::CompatibilityZonesKindMismatch);
        }
        Ok(Self { surface, zones })
    }

    /// Exact ordinary surface descriptor.
    #[must_use]
    pub const fn surface(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.surface
    }

    /// Optional exact ordinary zones descriptor.
    #[must_use]
    pub const fn zones(&self) -> Option<&ResolvedScreenPublicationDescriptor> {
        self.zones.as_ref()
    }
}

/// Immutable, deterministic screen-publication plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCapturePlan {
    generation: ScreenPlanGeneration,
    demand_revision: InputPublicationDemandRevision,
    branches: Arc<Vec<ScreenBranchDemand>>,
    physical_reductions: Arc<Vec<ScreenPhysicalReductionDemand>>,
    compatibility_surface: Option<usize>,
    compatibility_zones: Option<usize>,
}

impl ScreenCapturePlan {
    /// Screen-plan generation, independent from input-graph generation.
    #[must_use]
    pub const fn generation(&self) -> ScreenPlanGeneration {
        self.generation
    }

    /// Authoritative demand revision whose snapshot produced this plan.
    #[must_use]
    pub const fn demand_revision(&self) -> InputPublicationDemandRevision {
        self.demand_revision
    }

    /// Canonically ordered derived publication branches.
    #[must_use]
    pub fn branches(&self) -> &[ScreenBranchDemand] {
        self.branches.as_slice()
    }

    /// Canonically ordered shared physical reductions.
    #[must_use]
    pub fn physical_reductions(&self) -> &[ScreenPhysicalReductionDemand] {
        self.physical_reductions.as_slice()
    }

    /// Index of the ordinary branch selected by the compatibility mirror.
    #[must_use]
    pub const fn compatibility_branch_index(&self) -> Option<usize> {
        self.compatibility_surface
    }

    /// Ordinary branch selected by the compatibility mirror.
    #[must_use]
    pub fn compatibility_branch(&self) -> Option<&ScreenBranchDemand> {
        self.compatibility_surface
            .and_then(|index| self.branches.get(index))
    }

    /// Optional ordinary zones branch paired with the compatibility surface.
    #[must_use]
    pub fn compatibility_zones_branch(&self) -> Option<&ScreenBranchDemand> {
        self.compatibility_zones
            .and_then(|index| self.branches.get(index))
    }

    fn contains_descriptor(&self, descriptor: &ResolvedScreenPublicationDescriptor) -> bool {
        self.branches
            .binary_search_by(|branch| branch.descriptor.cmp(descriptor))
            .is_ok()
    }

    fn contains_physical(&self, descriptor: &ScreenPhysicalReductionDescriptor) -> bool {
        self.physical_reductions
            .binary_search_by(|reduction| reduction.descriptor.cmp(descriptor))
            .is_ok()
    }
}

impl Default for ScreenCapturePlan {
    fn default() -> Self {
        Self {
            generation: ScreenPlanGeneration::default(),
            demand_revision: InputPublicationDemandRevision::default(),
            branches: Arc::new(Vec::new()),
            physical_reductions: Arc::new(Vec::new()),
            compatibility_surface: None,
            compatibility_zones: None,
        }
    }
}

/// Resource category used by checked admission diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScreenResourceKind {
    /// Number of pixels in shared physical reductions.
    PhysicalPixels,
    /// Sum of physical row strides.
    PhysicalRowStride,
    /// Shared physical reduction planes.
    PhysicalPlane,
    /// Per-surface derived analysis/output planes.
    SurfaceAnalysisOutput,
    /// Zone grid color storage.
    ZoneGrid,
    /// Zone publication output storage.
    ZoneOutput,
    /// Exhaustive backend-owned allocation accounting for one physical key.
    BackendAllocation,
    /// Exhaustive capture-API-owned allocation accounting for one physical key.
    ApiAllocation,
    /// Exhaustive analyzer, smoothing, and policy state for one logical branch.
    ProcessingProfileState,
    /// Disjoint worker-owned allocation absent from the planner's minimum model.
    WorkerAdditional,
    /// Bounded last-good publication retention slots.
    PublicationRetention,
    /// Bounded in-flight publication versions available to subscribers.
    PublicationSubscriberSlots,
    /// Retired publication pools awaiting reader-safe reclamation.
    PendingPublicationRetirement,
    /// Old-plus-staged aggregate arithmetic.
    OverlapTotal,
    /// Configured process byte budget.
    ByteBudget,
    /// Backend-reported byte capacity.
    BackendCapacity,
    /// Worker-reported exact resource ledger.
    WorkerExactLedger,
}

impl ScreenResourceKind {
    const fn is_exact_accounting(self) -> bool {
        matches!(
            self,
            Self::PhysicalPlane
                | Self::SurfaceAnalysisOutput
                | Self::ZoneGrid
                | Self::ZoneOutput
                | Self::BackendAllocation
                | Self::ApiAllocation
                | Self::ProcessingProfileState
                | Self::WorkerAdditional
        )
    }
}

/// Checked aggregate byte ledger for one plan or plan delta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenResourceLedger {
    physical_pixels: u64,
    physical_row_stride_bytes: u64,
    physical_plane_bytes: u64,
    worker_additional_bytes: u64,
    publication_retention_bytes: u64,
    publication_subscriber_slot_bytes: u64,
    pending_publication_retirement_bytes: u64,
    total_bytes: u64,
}

impl ScreenResourceLedger {
    /// Sum of pixels across unique physical reductions.
    #[must_use]
    pub const fn physical_pixels(self) -> u64 {
        self.physical_pixels
    }

    /// Sum of unique physical row strides.
    #[must_use]
    pub const fn physical_row_stride_bytes(self) -> u64 {
        self.physical_row_stride_bytes
    }

    /// Bytes retained by unique physical reduction planes.
    #[must_use]
    pub const fn physical_plane_bytes(self) -> u64 {
        self.physical_plane_bytes
    }

    /// Exact retained bytes beyond the planner's modeled minimums.
    #[must_use]
    pub const fn worker_additional_bytes(self) -> u64 {
        self.worker_additional_bytes
    }

    /// Bytes admitted for bounded last-good publication retention.
    #[must_use]
    pub const fn publication_retention_bytes(self) -> u64 {
        self.publication_retention_bytes
    }

    /// Bytes admitted for bounded subscriber-held publication versions.
    #[must_use]
    pub const fn publication_subscriber_slot_bytes(self) -> u64 {
        self.publication_subscriber_slot_bytes
    }

    /// Bytes pinned by retired publication pools awaiting reclamation.
    #[must_use]
    pub const fn pending_publication_retirement_bytes(self) -> u64 {
        self.pending_publication_retirement_bytes
    }

    /// Total allocation bytes represented by this ledger.
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        self.total_bytes
    }
}

/// Independent configured and backend capacity fences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenAdmissionCapacity {
    byte_budget: u64,
    backend_capacity: u64,
}

impl ScreenAdmissionCapacity {
    /// Construct byte-based admission limits without dimensional caps.
    #[must_use]
    pub const fn new(byte_budget: u64, backend_capacity: u64) -> Self {
        Self {
            byte_budget,
            backend_capacity,
        }
    }

    /// Configured process byte budget.
    #[must_use]
    pub const fn byte_budget(self) -> u64 {
        self.byte_budget
    }

    /// Backend-reported byte capacity.
    #[must_use]
    pub const fn backend_capacity(self) -> u64 {
        self.backend_capacity
    }
}

/// Admission receipts for active, candidate, staged, and overlap resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenPlanAdmissionLedger {
    active: ScreenResourceLedger,
    candidate: ScreenResourceLedger,
    staged: ScreenResourceLedger,
    overlap: ScreenResourceLedger,
}

impl ScreenPlanAdmissionLedger {
    /// Resources retained by the active plan during preparation.
    #[must_use]
    pub const fn active(self) -> ScreenResourceLedger {
        self.active
    }

    /// Full resources required by the candidate after retirement.
    #[must_use]
    pub const fn candidate(self) -> ScreenResourceLedger {
        self.candidate
    }

    /// New resources not reusable from the active plan.
    #[must_use]
    pub const fn staged(self) -> ScreenResourceLedger {
        self.staged
    }

    /// Active plus staged resources at the transition high-water mark.
    #[must_use]
    pub const fn overlap(self) -> ScreenResourceLedger {
        self.overlap
    }
}

/// One disjoint exact allocation-accounting entry reported by a source worker.
///
/// Entries may aggregate one documented ownership domain, but every byte must
/// belong to exactly one entry. Ticket acknowledgement attests that the ledger
/// exhaustively covers the staged worker resources without overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenExactResource {
    name: Arc<str>,
    accounting_scope: Arc<str>,
    resource: ScreenResourceKind,
    bytes: u64,
}

impl ScreenExactResource {
    /// Construct a named exact worker allocation.
    ///
    /// # Errors
    ///
    /// Rejects an empty name or a metric/capacity kind that does not represent
    /// exact allocation bytes.
    pub fn try_new(
        name: impl Into<Arc<str>>,
        resource: ScreenResourceKind,
        bytes: u64,
    ) -> Result<Self, ScreenPlanError> {
        let name = name.into();
        Self::try_new_scoped(Arc::clone(&name), name, resource, bytes)
    }

    /// Construct a uniquely named entry retained under one required scope.
    ///
    /// Extra worker entries use this constructor to bind their bytes to the
    /// physical or logical resource whose lifetime owns them.
    ///
    /// # Errors
    ///
    /// Rejects empty names/scopes or non-allocation resource kinds.
    pub fn try_new_scoped(
        name: impl Into<Arc<str>>,
        accounting_scope: impl Into<Arc<str>>,
        resource: ScreenResourceKind,
        bytes: u64,
    ) -> Result<Self, ScreenPlanError> {
        let name = name.into();
        let accounting_scope = accounting_scope.into();
        if name.trim().is_empty() {
            return Err(ScreenPlanError::EmptyResourceName);
        }
        if accounting_scope.trim().is_empty() {
            return Err(ScreenPlanError::EmptyResourceScope);
        }
        if !resource.is_exact_accounting() {
            return Err(ScreenPlanError::InvalidExactResourceKind { resource });
        }
        Ok(Self {
            name,
            accounting_scope,
            resource,
            bytes,
        })
    }

    /// Opaque worker resource name.
    #[must_use]
    pub const fn name(&self) -> &Arc<str> {
        &self.name
    }

    /// Required accounting domain controlling this entry's retained lifetime.
    #[must_use]
    pub const fn accounting_scope(&self) -> &Arc<str> {
        &self.accounting_scope
    }

    /// Exact resource category.
    #[must_use]
    pub const fn resource(&self) -> ScreenResourceKind {
        self.resource
    }

    /// Exact allocated bytes.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[derive(Debug)]
struct ScreenResourceLifetimeInner {
    source_id: CaptureSourceId,
    plan_generation: ScreenPlanGeneration,
    demand_revision: InputPublicationDemandRevision,
    transaction_id: ScreenPlanTransactionId,
    worker_nonce: NonZeroU64,
    allocation_nonce: NonZeroU64,
    resource: ScreenExactResource,
    retirement_charge: Arc<ScreenRetirementCharge>,
}

/// Ticket-bound ownership handle for one exact worker allocation.
///
/// Runtime storage retains a clone for its entire physical lifetime. The
/// committed plan retains another clone while the allocation is authoritative.
/// Once commit retires the allocation, its bytes remain charged until every
/// stale job and state snapshot releases this handle.
#[derive(Clone, Debug)]
pub struct ScreenResourceLifetime {
    inner: Arc<ScreenResourceLifetimeInner>,
}

impl ScreenResourceLifetime {
    /// Exact allocation represented by this lifetime.
    #[must_use]
    pub fn resource(&self) -> &ScreenExactResource {
        &self.inner.resource
    }

    pub(crate) fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) fn is_final_owner(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    pub(crate) fn arm_retirement(&self) {
        self.inner.retirement_charge.arm();
    }
}

/// Opaque exact resource ledger returned after real worker allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenExactResourceLedger {
    resources: Arc<Vec<ScreenExactResource>>,
    total_bytes: u64,
}

impl ScreenExactResourceLedger {
    /// Construct a checked exact worker ledger.
    ///
    /// # Errors
    ///
    /// Returns a typed duplicate-name, allocation, or total-byte overflow error.
    pub fn try_new(
        resources: impl IntoIterator<Item = ScreenExactResource>,
    ) -> Result<Self, ScreenPlanError> {
        let mut retained = Vec::new();
        let mut total_bytes = 0_u64;
        for resource in resources {
            reserve_one(&mut retained)?;
            total_bytes = total_bytes.checked_add(resource.bytes).ok_or_else(|| {
                ScreenPlanError::ExactLedgerOverflow {
                    resource: Arc::clone(&resource.name),
                }
            })?;
            retained.push(resource);
        }
        retained.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        if let Some(duplicate) = retained
            .windows(2)
            .find(|pair| pair[0].name == pair[1].name)
        {
            return Err(ScreenPlanError::DuplicateExactResourceName {
                resource: Arc::clone(&duplicate[1].name),
            });
        }
        Ok(Self {
            resources: Arc::new(retained),
            total_bytes,
        })
    }

    /// Exact named worker allocations.
    #[must_use]
    pub fn resources(&self) -> &[ScreenExactResource] {
        self.resources.as_slice()
    }

    /// Checked exact total bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// One required minimum accounting entry in a source worker's candidate delta.
///
/// Zero-byte backend, API, and processing-state minimums are still mandatory:
/// the worker must attest the exhaustive actual domain total, which may be zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenRequiredResourceMinimum {
    name: Arc<str>,
    resource: ScreenResourceKind,
    descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    minimum_bytes: u64,
    retention: ScreenResourceRetentionKey,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ScreenResourceRetentionKey {
    Physical(ScreenPhysicalReductionDescriptor),
    PhysicalPlane(ScreenPhysicalReductionDescriptor),
    Branch(ResolvedScreenPublicationDescriptor),
}

impl ScreenRequiredResourceMinimum {
    /// Stable candidate-local allocation name.
    #[must_use]
    pub const fn name(&self) -> &Arc<str> {
        &self.name
    }

    /// Resource category whose allocation is required.
    #[must_use]
    pub const fn resource(&self) -> ScreenResourceKind {
        self.resource
    }

    /// Complete descriptor owning the allocation.
    #[must_use]
    pub const fn descriptor(&self) -> &Arc<ResolvedScreenPublicationDescriptor> {
        &self.descriptor
    }

    /// Smallest truthful allocation for this candidate resource.
    #[must_use]
    pub const fn minimum_bytes(&self) -> u64 {
        self.minimum_bytes
    }
}

const WORKER_PREPARED: u8 = 0;
const WORKER_ARMED: u8 = 1;
const WORKER_ACTIVE: u8 = 2;
const WORKER_ABORTED: u8 = 3;

/// Observable readiness and commit-notification lifecycle of a worker binding.
///
/// Committed-state membership, not this diagnostic state, is publication
/// authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenWorkerBindingState {
    /// Resources exist but cannot publish.
    Prepared,
    /// Resources passed all fences but remain inactive before commit.
    Armed,
    /// The worker was notified after its candidate became committed.
    Active,
    /// The candidate was explicitly abandoned.
    Aborted,
    /// No committed branch retains this binding.
    Retired,
}

#[derive(Debug)]
pub(crate) struct ScreenWorkerActivationLatch {
    state: AtomicU8,
}

impl ScreenWorkerActivationLatch {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(WORKER_PREPARED),
        }
    }

    pub(crate) fn arm(&self) {
        self.state.store(WORKER_ARMED, Ordering::Release);
    }

    pub(crate) fn activate(&self) {
        self.state.store(WORKER_ACTIVE, Ordering::Release);
    }

    pub(crate) fn abort(&self) {
        self.state.store(WORKER_ABORTED, Ordering::Release);
    }

    fn state(&self) -> ScreenWorkerBindingState {
        match self.state.load(Ordering::Acquire) {
            WORKER_PREPARED => ScreenWorkerBindingState::Prepared,
            WORKER_ARMED => ScreenWorkerBindingState::Armed,
            WORKER_ACTIVE => ScreenWorkerBindingState::Active,
            _ => ScreenWorkerBindingState::Aborted,
        }
    }
}

#[derive(Debug)]
struct ScreenWorkerBindingInner {
    source_id: CaptureSourceId,
    plan_generation: ScreenPlanGeneration,
    demand_revision: InputPublicationDemandRevision,
    transaction_id: ScreenPlanTransactionId,
    worker_nonce: NonZeroU64,
    activation: Arc<ScreenWorkerActivationLatch>,
    retired: AtomicBool,
}

/// Opaque source-, plan-, transaction-, and nonce-bound worker authority.
#[derive(Clone, Debug)]
pub struct ScreenWorkerBinding {
    inner: Arc<ScreenWorkerBindingInner>,
}

impl ScreenWorkerBinding {
    fn new(
        source_id: CaptureSourceId,
        plan_generation: ScreenPlanGeneration,
        demand_revision: InputPublicationDemandRevision,
        transaction_id: ScreenPlanTransactionId,
        worker_nonce: NonZeroU64,
        activation: Arc<ScreenWorkerActivationLatch>,
    ) -> Self {
        Self {
            inner: Arc::new(ScreenWorkerBindingInner {
                source_id,
                plan_generation,
                demand_revision,
                transaction_id,
                worker_nonce,
                activation,
                retired: AtomicBool::new(false),
            }),
        }
    }

    /// Exact source this binding may publish.
    #[must_use]
    pub fn source_id(&self) -> &CaptureSourceId {
        &self.inner.source_id
    }

    /// Candidate plan generation that issued this binding.
    #[must_use]
    pub fn plan_generation(&self) -> ScreenPlanGeneration {
        self.inner.plan_generation
    }

    /// Demand revision that issued this binding.
    #[must_use]
    pub fn demand_revision(&self) -> InputPublicationDemandRevision {
        self.inner.demand_revision
    }

    /// Candidate transaction identity that issued this binding.
    #[must_use]
    pub fn transaction_id(&self) -> ScreenPlanTransactionId {
        self.inner.transaction_id
    }

    /// Worker-ticket nonce that issued this binding.
    #[must_use]
    pub fn worker_nonce(&self) -> NonZeroU64 {
        self.inner.worker_nonce
    }

    /// Current binding lifecycle.
    #[must_use]
    pub fn state(&self) -> ScreenWorkerBindingState {
        if self.inner.retired.load(Ordering::Acquire) {
            ScreenWorkerBindingState::Retired
        } else {
            self.inner.activation.state()
        }
    }

    pub(crate) fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(crate) fn retire(&self) {
        self.inner.retired.store(true, Ordering::Release);
    }
}

/// Source- and plan-bound preparation ticket sent to a worker.
#[derive(Debug)]
pub struct ScreenWorkerPreparationTicket {
    source_id: CaptureSourceId,
    plan_generation: ScreenPlanGeneration,
    demand_revision: InputPublicationDemandRevision,
    transaction_id: ScreenPlanTransactionId,
    worker_nonce: NonZeroU64,
    activation: Arc<ScreenWorkerActivationLatch>,
    candidate: Arc<ScreenCapturePlan>,
    required_minimums: Arc<Vec<ScreenRequiredResourceMinimum>>,
    pending_retired_bytes: Arc<AtomicU64>,
    next_allocation_nonce: AtomicU64,
}

impl ScreenWorkerPreparationTicket {
    /// Source this worker preparation must own.
    #[must_use]
    pub const fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    /// Candidate plan generation this ticket is bound to.
    #[must_use]
    pub const fn plan_generation(&self) -> ScreenPlanGeneration {
        self.plan_generation
    }

    /// Authoritative demand revision this ticket is bound to.
    #[must_use]
    pub const fn demand_revision(&self) -> InputPublicationDemandRevision {
        self.demand_revision
    }

    /// Transaction identity this ticket is bound to.
    #[must_use]
    pub const fn transaction_id(&self) -> ScreenPlanTransactionId {
        self.transaction_id
    }

    /// Worker-owned opaque nonce.
    #[must_use]
    pub const fn worker_nonce(&self) -> NonZeroU64 {
        self.worker_nonce
    }

    /// Complete deterministic resource contract for this source delta.
    #[must_use]
    pub fn required_minimums(&self) -> &[ScreenRequiredResourceMinimum] {
        self.required_minimums.as_slice()
    }

    /// Bind one exact allocation to this ticket before worker acknowledgement.
    ///
    /// The worker retains the returned handle with its storage and also passes
    /// a borrowed handle to [`Self::acknowledge`].
    ///
    /// # Errors
    ///
    /// Returns a typed failure if this ticket exhausts allocation identities.
    pub fn bind_resource_lifetime(
        &self,
        resource: &ScreenExactResource,
    ) -> Result<ScreenResourceLifetime, ScreenPlanError> {
        let previous_nonce = self
            .next_allocation_nonce
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |nonce| {
                nonce.checked_add(1)
            })
            .map_err(|_| ScreenPlanError::ResourceLifetimeNonceExhausted)?;
        let allocation_nonce = previous_nonce
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or(ScreenPlanError::ResourceLifetimeNonceExhausted)?;
        Ok(ScreenResourceLifetime {
            inner: Arc::new(ScreenResourceLifetimeInner {
                source_id: self.source_id.clone(),
                plan_generation: self.plan_generation,
                demand_revision: self.demand_revision,
                transaction_id: self.transaction_id,
                worker_nonce: self.worker_nonce,
                allocation_nonce,
                resource: resource.clone(),
                retirement_charge: Arc::new(ScreenRetirementCharge::new(
                    Arc::clone(&self.pending_retired_bytes),
                    resource.bytes,
                )),
            }),
        })
    }

    /// Attest an exhaustive, disjoint worker ledger and bind it into a token.
    ///
    /// Required planner minima must be present with their exact categories and
    /// at least their minimum bytes. Additional uniquely named accounting
    /// entries are retained and counted in full.
    ///
    /// # Errors
    ///
    /// Rejects omitted, category-mismatched, or understated required minima.
    pub fn acknowledge(
        &self,
        exact_ledger: ScreenExactResourceLedger,
        lifetimes: &[ScreenResourceLifetime],
    ) -> Result<ScreenPreparedWorkerToken, ScreenPlanError> {
        validate_resource_coverage(self.required_minimums.as_slice(), &exact_ledger)?;
        let lifetimes = validate_resource_lifetimes(self, &exact_ledger, lifetimes)?;
        let retained_resources =
            bind_retained_resources(self.required_minimums.as_slice(), &exact_ledger, &lifetimes)?;
        let binding = ScreenWorkerBinding::new(
            self.source_id.clone(),
            self.plan_generation,
            self.demand_revision,
            self.transaction_id,
            self.worker_nonce,
            Arc::clone(&self.activation),
        );
        Ok(ScreenPreparedWorkerToken {
            source_id: self.source_id.clone(),
            plan_generation: self.plan_generation,
            demand_revision: self.demand_revision,
            transaction_id: self.transaction_id,
            worker_nonce: self.worker_nonce,
            candidate: Arc::clone(&self.candidate),
            exact_ledger,
            retained_resources,
            binding,
        })
    }
}

/// Opaque prepared token accepted only by its source and candidate plan.
#[derive(Debug)]
pub struct ScreenPreparedWorkerToken {
    source_id: CaptureSourceId,
    plan_generation: ScreenPlanGeneration,
    demand_revision: InputPublicationDemandRevision,
    transaction_id: ScreenPlanTransactionId,
    worker_nonce: NonZeroU64,
    candidate: Arc<ScreenCapturePlan>,
    exact_ledger: ScreenExactResourceLedger,
    retained_resources: ScreenRetainedResourceLedger,
    binding: ScreenWorkerBinding,
}

impl ScreenPreparedWorkerToken {
    /// Bound source identity.
    #[must_use]
    pub const fn source_id(&self) -> &CaptureSourceId {
        &self.source_id
    }

    /// Bound candidate plan generation.
    #[must_use]
    pub const fn plan_generation(&self) -> ScreenPlanGeneration {
        self.plan_generation
    }

    /// Bound authoritative demand revision.
    #[must_use]
    pub const fn demand_revision(&self) -> InputPublicationDemandRevision {
        self.demand_revision
    }

    /// Bound transaction identity.
    #[must_use]
    pub const fn transaction_id(&self) -> ScreenPlanTransactionId {
        self.transaction_id
    }

    /// Worker-owned opaque nonce.
    #[must_use]
    pub const fn worker_nonce(&self) -> NonZeroU64 {
        self.worker_nonce
    }

    /// Exact resource ledger produced by the worker.
    #[must_use]
    pub const fn exact_ledger(&self) -> &ScreenExactResourceLedger {
        &self.exact_ledger
    }

    /// Opaque worker runtime authority created by this exact ticket.
    #[must_use]
    pub const fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }
}

/// Candidate plan in its resource-preparation phase.
#[derive(Debug)]
pub struct PreparingScreenPlan {
    base_state: Arc<ScreenCommittedState>,
    base_graph_generation: ScreenInputGraphGeneration,
    candidate: Arc<ScreenCapturePlan>,
    admission: ScreenPlanAdmissionLedger,
    capacity: ScreenAdmissionCapacity,
    transaction_id: ScreenPlanTransactionId,
    required_sources: Arc<Vec<CaptureSourceId>>,
    source_resource_contracts: Arc<Vec<ScreenSourceResourceContract>>,
    issued_tickets: HashMap<CaptureSourceId, NonZeroU64>,
    prepared_sources: HashSet<CaptureSourceId>,
    next_worker_nonce: u64,
    prepared_tokens: Vec<ScreenPreparedWorkerToken>,
    activation: Arc<ScreenWorkerActivationLatch>,
}

#[derive(Debug)]
struct ScreenSourceResourceContract {
    source_id: CaptureSourceId,
    required_minimums: Arc<Vec<ScreenRequiredResourceMinimum>>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ScreenRetainedResourceLedger {
    pub(crate) entries: Arc<Vec<ScreenRetainedResourceEntry>>,
    pub(crate) total_bytes: u64,
}

impl ScreenRetainedResourceLedger {
    pub(crate) fn retired_lifetimes(
        &self,
        candidate: &Self,
    ) -> Result<Vec<ScreenResourceLifetime>, ScreenPlanError> {
        let mut retired = Vec::new();
        retired
            .try_reserve_exact(self.entries.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        let mut active_index = 0;
        let mut candidate_index = 0;
        while let Some(active) = self.entries.get(active_index) {
            let Some(next) = candidate.entries.get(candidate_index) else {
                retired.extend(
                    self.entries[active_index..]
                        .iter()
                        .map(|entry| entry.lifetime.clone()),
                );
                break;
            };
            match compare_retained_resources(active, next) {
                std::cmp::Ordering::Less => {
                    retired.push(active.lifetime.clone());
                    active_index += 1;
                }
                std::cmp::Ordering::Greater => {
                    candidate_index += 1;
                }
                std::cmp::Ordering::Equal => {
                    if !active.lifetime.is_same(&next.lifetime) {
                        retired.push(active.lifetime.clone());
                    }
                    active_index += 1;
                    candidate_index += 1;
                }
            }
        }
        Ok(retired)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScreenRetainedResourceEntry {
    pub(crate) name: Arc<str>,
    pub(crate) retention: ScreenResourceRetentionKey,
    pub(crate) descriptor: Arc<ResolvedScreenPublicationDescriptor>,
    pub(crate) lifetime: ScreenResourceLifetime,
}

impl ScreenRetainedResourceEntry {
    fn bytes(&self) -> u64 {
        self.lifetime.resource().bytes
    }
}

impl PreparingScreenPlan {
    /// Plan remaining active throughout preparation.
    #[must_use]
    pub fn active_plan(&self) -> &ScreenCapturePlan {
        self.base_state.plan()
    }

    /// Immutable candidate plan.
    #[must_use]
    pub fn candidate_plan(&self) -> &ScreenCapturePlan {
        self.candidate.as_ref()
    }

    /// Checked admission receipts.
    #[must_use]
    pub const fn admission(&self) -> ScreenPlanAdmissionLedger {
        self.admission
    }

    /// Capacity used for candidate admission.
    #[must_use]
    pub const fn capacity(&self) -> ScreenAdmissionCapacity {
        self.capacity
    }

    /// Base structural graph generation.
    #[must_use]
    pub const fn base_graph_generation(&self) -> ScreenInputGraphGeneration {
        self.base_graph_generation
    }

    /// Opaque transaction identity.
    #[must_use]
    pub const fn transaction_id(&self) -> ScreenPlanTransactionId {
        self.transaction_id
    }

    /// Sources whose candidate deltas require worker acknowledgement.
    #[must_use]
    pub fn required_sources(&self) -> &[CaptureSourceId] {
        self.required_sources.as_slice()
    }

    /// Issue a source- and plan-bound worker ticket.
    ///
    /// # Errors
    ///
    /// Rejects a source without staged work.
    pub fn worker_ticket(
        &mut self,
        source_id: &CaptureSourceId,
    ) -> Result<ScreenWorkerPreparationTicket, ScreenPlanError> {
        let contract = self
            .source_resource_contracts
            .binary_search_by(|contract| contract.source_id.as_str().cmp(source_id.as_str()))
            .ok()
            .and_then(|index| self.source_resource_contracts.get(index))
            .ok_or_else(|| ScreenPlanError::UnexpectedWorkerSource {
                source_id: source_id.clone(),
            })?;
        let required_minimums = Arc::clone(&contract.required_minimums);
        if self.issued_tickets.contains_key(source_id) {
            return Err(ScreenPlanError::WorkerTicketAlreadyIssued {
                source_id: source_id.clone(),
            });
        }
        self.next_worker_nonce = self
            .next_worker_nonce
            .checked_add(1)
            .ok_or(ScreenPlanError::WorkerNonceExhausted)?;
        let worker_nonce =
            NonZeroU64::new(self.next_worker_nonce).ok_or(ScreenPlanError::WorkerNonceExhausted)?;
        self.issued_tickets.insert(source_id.clone(), worker_nonce);
        Ok(ScreenWorkerPreparationTicket {
            source_id: source_id.clone(),
            plan_generation: self.candidate.generation,
            demand_revision: self.candidate.demand_revision,
            transaction_id: self.transaction_id,
            worker_nonce,
            activation: Arc::clone(&self.activation),
            candidate: Arc::clone(&self.candidate),
            required_minimums,
            pending_retired_bytes: self.base_state.pending_retirement_counter(),
            next_allocation_nonce: AtomicU64::new(0),
        })
    }

    /// Accept one opaque worker token after validating every generation fence.
    ///
    /// # Errors
    ///
    /// Rejects tokens from another source, plan, transaction, or duplicate
    /// acknowledgement.
    pub fn acknowledge(&mut self, token: ScreenPreparedWorkerToken) -> Result<(), ScreenPlanError> {
        if !Arc::ptr_eq(&token.candidate, &self.candidate) {
            return Err(ScreenPlanError::WorkerCandidateMismatch {
                expected: self.candidate.generation,
                observed: token.candidate.generation,
            });
        }
        if token.plan_generation != self.candidate.generation {
            return Err(ScreenPlanError::WorkerPlanGenerationMismatch {
                expected: self.candidate.generation,
                observed: token.plan_generation,
            });
        }
        if token.demand_revision != self.candidate.demand_revision {
            return Err(ScreenPlanError::WorkerDemandRevisionMismatch {
                expected: self.candidate.demand_revision,
                observed: token.demand_revision,
            });
        }
        if token.transaction_id != self.transaction_id {
            return Err(ScreenPlanError::WorkerTransactionMismatch {
                expected: self.transaction_id,
                observed: token.transaction_id,
            });
        }
        if self
            .required_sources
            .binary_search_by(|source| source.as_str().cmp(token.source_id.as_str()))
            .is_err()
        {
            return Err(ScreenPlanError::UnexpectedWorkerSource {
                source_id: token.source_id,
            });
        }
        if self
            .issued_tickets
            .get(&token.source_id)
            .is_none_or(|nonce| *nonce != token.worker_nonce)
        {
            return Err(ScreenPlanError::WorkerNonceMismatch {
                source_id: token.source_id,
                observed: token.worker_nonce,
            });
        }
        if self.prepared_sources.contains(&token.source_id) {
            return Err(ScreenPlanError::DuplicateWorkerAcknowledgement {
                source_id: token.source_id,
            });
        }
        reserve_one(&mut self.prepared_tokens)?;
        self.prepared_sources.insert(token.source_id.clone());
        self.prepared_tokens.push(token);
        Ok(())
    }

    /// Enter optional asynchronous backend negotiation without changing active.
    #[must_use]
    pub fn await_backend(self) -> AwaitingBackendScreenPlan {
        AwaitingBackendScreenPlan { preparing: self }
    }

    /// Arm every prepared resource behind the candidate generation fence.
    ///
    /// This pure boundary does not install platform resources; runtime workers
    /// consume the retained opaque tokens when integrating the transition.
    pub fn arm(
        self,
        observed_plan_generation: ScreenPlanGeneration,
        observed_demand_revision: InputPublicationDemandRevision,
        observed_graph_generation: ScreenInputGraphGeneration,
    ) -> Result<ArmedScreenPlan, ScreenPlanArmFailure> {
        if observed_plan_generation != self.base_state.plan().generation {
            return Err(ScreenPlanArmFailure {
                error: ScreenPlanError::BasePlanGenerationConflict {
                    expected: self.base_state.plan().generation,
                    observed: observed_plan_generation,
                },
                preparing: Box::new(self),
            });
        }
        if observed_demand_revision != self.candidate.demand_revision {
            return Err(ScreenPlanArmFailure {
                error: ScreenPlanError::DemandRevisionConflict {
                    expected: self.candidate.demand_revision,
                    observed: observed_demand_revision,
                },
                preparing: Box::new(self),
            });
        }
        if observed_graph_generation != self.base_graph_generation {
            return Err(ScreenPlanArmFailure {
                error: ScreenPlanError::BaseGraphGenerationConflict {
                    expected: self.base_graph_generation,
                    observed: observed_graph_generation,
                },
                preparing: Box::new(self),
            });
        }
        if let Some(source_id) = self
            .required_sources
            .iter()
            .find(|source_id| !self.prepared_sources.contains(*source_id))
        {
            return Err(ScreenPlanArmFailure {
                error: ScreenPlanError::MissingWorkerAcknowledgement {
                    source_id: source_id.clone(),
                },
                preparing: Box::new(self),
            });
        }
        let exact_worker_bytes = match validate_exact_worker_ledgers(&self) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Err(ScreenPlanArmFailure {
                    error,
                    preparing: Box::new(self),
                });
            }
        };
        let candidate_retained_resources = match candidate_retained_resources(
            self.base_state.retained_resources(),
            &self.candidate,
            &self.prepared_tokens,
        ) {
            Ok(resources) => resources,
            Err(error) => {
                return Err(ScreenPlanArmFailure {
                    error,
                    preparing: Box::new(self),
                });
            }
        };
        let (candidate_state, commit_activation) = match ScreenCommittedState::prepare(
            &self.base_state,
            Arc::clone(&self.candidate),
            candidate_retained_resources,
            self.prepared_tokens
                .iter()
                .map(|token| token.binding.clone()),
            Arc::clone(&self.activation),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(ScreenPlanArmFailure {
                    error,
                    preparing: Box::new(self),
                });
            }
        };
        self.activation.arm();
        Ok(ArmedScreenPlan {
            base_state: self.base_state,
            base_graph_generation: self.base_graph_generation,
            candidate_state,
            admission: self.admission,
            transaction_id: self.transaction_id,
            exact_worker_bytes,
            commit_activation: Box::new(commit_activation),
            prepared_tokens: self.prepared_tokens,
        })
    }

    /// Abort preparation while retaining the byte-identical active plan.
    #[must_use]
    pub fn abort(self) -> ScreenPlanAbort {
        self.activation.abort();
        ScreenPlanAbort {
            active_plan: Arc::clone(self.base_state.plan_arc()),
            candidate_generation: self.candidate.generation,
            prepared_tokens: self.prepared_tokens,
        }
    }
}

/// Candidate waiting for asynchronous platform negotiation.
#[derive(Debug)]
pub struct AwaitingBackendScreenPlan {
    preparing: PreparingScreenPlan,
}

impl AwaitingBackendScreenPlan {
    /// Active plan remains unchanged while the backend waits.
    #[must_use]
    pub fn active_plan(&self) -> &ScreenCapturePlan {
        self.preparing.active_plan()
    }

    /// Resume preparation after the backend acknowledges exact resources.
    #[must_use]
    pub fn backend_ready(self) -> PreparingScreenPlan {
        self.preparing
    }

    /// Abort negotiation while retaining the byte-identical active plan.
    #[must_use]
    pub fn abort(self) -> ScreenPlanAbort {
        self.preparing.abort()
    }
}

/// Candidate whose prepared resources are armed but not yet published.
#[derive(Debug)]
pub struct ArmedScreenPlan {
    base_state: Arc<ScreenCommittedState>,
    base_graph_generation: ScreenInputGraphGeneration,
    candidate_state: Arc<ScreenCommittedState>,
    admission: ScreenPlanAdmissionLedger,
    transaction_id: ScreenPlanTransactionId,
    exact_worker_bytes: u64,
    commit_activation: Box<ScreenCommitActivation>,
    prepared_tokens: Vec<ScreenPreparedWorkerToken>,
}

impl ArmedScreenPlan {
    /// Plan still visible before commit.
    #[must_use]
    pub fn active_plan(&self) -> &ScreenCapturePlan {
        self.base_state.plan()
    }

    /// Candidate ready for one committed-plan swap.
    #[must_use]
    pub fn candidate_plan(&self) -> &ScreenCapturePlan {
        self.candidate_state.plan()
    }

    /// Prebuilt immutable state installed byte-for-byte at commit.
    #[must_use]
    pub const fn candidate_state(&self) -> &Arc<ScreenCommittedState> {
        &self.candidate_state
    }

    /// Checked admission receipts.
    #[must_use]
    pub const fn admission(&self) -> ScreenPlanAdmissionLedger {
        self.admission
    }

    /// Opaque transaction identity.
    #[must_use]
    pub const fn transaction_id(&self) -> ScreenPlanTransactionId {
        self.transaction_id
    }

    /// Exact staged bytes acknowledged by source workers.
    #[must_use]
    pub const fn exact_worker_bytes(&self) -> u64 {
        self.exact_worker_bytes
    }

    /// Abort armed resources before publication without changing active.
    #[must_use]
    pub fn abort(self) -> ScreenPlanAbort {
        (*self.commit_activation).abort();
        ScreenPlanAbort {
            active_plan: Arc::clone(self.base_state.plan_arc()),
            candidate_generation: self.candidate_state.plan().generation,
            prepared_tokens: self.prepared_tokens,
        }
    }
}

/// Failed arm retaining the complete preparation for explicit abort or retry.
#[derive(Debug)]
pub struct ScreenPlanArmFailure {
    error: ScreenPlanError,
    preparing: Box<PreparingScreenPlan>,
}

impl ScreenPlanArmFailure {
    /// Exact arm conflict.
    #[must_use]
    pub const fn error(&self) -> &ScreenPlanError {
        &self.error
    }

    /// Recover the preparation without losing worker tokens.
    #[must_use]
    pub fn into_preparing(self) -> PreparingScreenPlan {
        *self.preparing
    }
}

/// Failed commit retaining the armed candidate for explicit abort.
#[derive(Debug)]
pub struct ScreenPlanCommitFailure {
    error: ScreenPlanError,
    armed: Box<ArmedScreenPlan>,
}

impl ScreenPlanCommitFailure {
    /// Exact commit conflict.
    #[must_use]
    pub const fn error(&self) -> &ScreenPlanError {
        &self.error
    }

    /// Recover the armed candidate without losing worker tokens.
    #[must_use]
    pub fn into_armed(self) -> ArmedScreenPlan {
        *self.armed
    }
}

/// Explicit abort receipt retaining active state and prepared worker tokens.
#[derive(Debug)]
pub struct ScreenPlanAbort {
    active_plan: Arc<ScreenCapturePlan>,
    candidate_generation: ScreenPlanGeneration,
    prepared_tokens: Vec<ScreenPreparedWorkerToken>,
}

/// One atomically committed plan plus publication pools awaiting reclamation.
#[must_use = "committed publication retirement must be handed to its owning reclaimer"]
pub struct CommittedScreenPlan {
    plan: ScreenCapturePlan,
    retirement: ScreenPublicationRetirement,
}

impl CommittedScreenPlan {
    /// Newly committed immutable plan.
    #[must_use]
    pub const fn plan(&self) -> &ScreenCapturePlan {
        &self.plan
    }

    /// Split the plan from its explicit out-of-lock retirement handle.
    #[must_use = "the returned publication retirement must reach its owning reclaimer"]
    pub fn into_parts(self) -> (ScreenCapturePlan, ScreenPublicationRetirement) {
        (self.plan, self.retirement)
    }
}

impl std::fmt::Debug for CommittedScreenPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommittedScreenPlan")
            .field("plan", &self.plan)
            .field("retirement", &self.retirement)
            .finish()
    }
}

impl ScreenPlanAbort {
    /// Byte-identical plan that remained active.
    #[must_use]
    pub fn active_plan(&self) -> &ScreenCapturePlan {
        self.active_plan.as_ref()
    }

    /// Candidate generation whose resources must be retired.
    #[must_use]
    pub const fn candidate_generation(&self) -> ScreenPlanGeneration {
        self.candidate_generation
    }

    /// Opaque worker tokens runtime integration must abort or retire.
    #[must_use]
    pub fn prepared_tokens(&self) -> &[ScreenPreparedWorkerToken] {
        &self.prepared_tokens
    }
}

/// Stateful coordinator for prepared screen-plan transactions.
#[derive(Debug)]
pub struct ScreenPlanBuilder {
    next_transaction_id: u64,
    publication_hub: Arc<ScreenPublicationHub>,
}

impl Default for ScreenPlanBuilder {
    fn default() -> Self {
        Self::with_publication_slots(ScreenPublicationSlotPolicy::default())
    }
}

impl ScreenPlanBuilder {
    /// Construct a builder around the initial empty generation.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct with an explicit admitted publication-version slot policy.
    #[must_use]
    pub fn with_publication_slots(slot_policy: ScreenPublicationSlotPolicy) -> Self {
        Self {
            next_transaction_id: 0,
            publication_hub: Arc::new(ScreenPublicationHub::new(slot_policy)),
        }
    }

    /// Current immutable active plan snapshot.
    #[must_use]
    pub fn current(&self) -> Arc<ScreenCapturePlan> {
        Arc::clone(self.publication_hub.committed_state().plan_arc())
    }

    /// Single immutable authority snapshot currently visible to all readers.
    #[must_use]
    pub fn committed_state(&self) -> Arc<ScreenCommittedState> {
        self.publication_hub.committed_state()
    }

    /// Exact bytes retained by the currently committed plan.
    #[must_use]
    pub fn retained_exact_bytes(&self) -> u64 {
        self.publication_hub
            .committed_state()
            .retained_exact_bytes()
    }

    /// Publication authority committed by this coordinator.
    #[must_use]
    pub fn publication_hub(&self) -> Arc<ScreenPublicationHub> {
        Arc::clone(&self.publication_hub)
    }

    /// Prepare and admit a candidate without mutating the active plan.
    ///
    /// Exact descriptors group at maximum cadence, logical branches group by
    /// their physical reduction key, and old-plus-staged resources are checked
    /// against explicit byte and backend capacity. No dimensional, cadence, or
    /// consumer-count cap participates in admission.
    ///
    /// # Errors
    ///
    /// Returns typed canonicalization, arithmetic, or capacity failures.
    pub fn prepare(
        &mut self,
        demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
        compatibility: Option<&ScreenCompatibilitySelection>,
        demand_revision: InputPublicationDemandRevision,
        graph_generation: ScreenInputGraphGeneration,
        capacity: ScreenAdmissionCapacity,
    ) -> Result<PreparingScreenPlan, ScreenPlanError> {
        let base_state = self.publication_hub.committed_state();
        if demand_revision <= base_state.plan().demand_revision {
            return Err(ScreenPlanError::DemandRevisionNotMonotonic {
                committed: base_state.plan().demand_revision,
                candidate: demand_revision,
            });
        }
        self.next_transaction_id = self
            .next_transaction_id
            .checked_add(1)
            .ok_or(ScreenPlanError::TransactionIdentityExhausted)?;
        let transaction_id = ScreenPlanTransactionId(
            NonZeroU64::new(self.next_transaction_id)
                .ok_or(ScreenPlanError::TransactionIdentityExhausted)?,
        );
        let candidate =
            canonicalize_candidate(base_state.plan(), demands, compatibility, demand_revision)?;
        let admission = admit_candidate(
            base_state.plan(),
            &candidate,
            base_state.retained_exact_bytes(),
            self.publication_hub.pending_retired_bytes(),
            base_state.slot_policy(),
            capacity,
        )?;
        let source_resource_contracts = required_source_minimums(base_state.plan(), &candidate)?;
        let mut required_sources = Vec::new();
        required_sources
            .try_reserve_exact(source_resource_contracts.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        required_sources.extend(
            source_resource_contracts
                .iter()
                .map(|contract| contract.source_id.clone()),
        );
        let mut issued_tickets = HashMap::new();
        issued_tickets
            .try_reserve(required_sources.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        let mut prepared_sources = HashSet::new();
        prepared_sources
            .try_reserve(required_sources.len())
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
        Ok(PreparingScreenPlan {
            base_state,
            base_graph_generation: graph_generation,
            candidate: Arc::new(candidate),
            admission,
            capacity,
            transaction_id,
            required_sources: Arc::new(required_sources),
            source_resource_contracts: Arc::new(source_resource_contracts),
            issued_tickets,
            prepared_sources,
            next_worker_nonce: 0,
            prepared_tokens: Vec::new(),
            activation: Arc::new(ScreenWorkerActivationLatch::new()),
        })
    }

    /// Commit one armed candidate after rechecking both base generations.
    ///
    /// The assignment is the pure model's single committed-plan swap. Runtime
    /// integration publishes the immutable snapshot through its atomic holder.
    pub fn commit(
        &mut self,
        armed: ArmedScreenPlan,
        observed_demand_revision: InputPublicationDemandRevision,
        observed_graph_generation: ScreenInputGraphGeneration,
    ) -> Result<CommittedScreenPlan, ScreenPlanCommitFailure> {
        let current = self.publication_hub.committed_state();
        if !Arc::ptr_eq(&current, &armed.base_state) {
            return Err(ScreenPlanCommitFailure {
                error: ScreenPlanError::BaseCommittedStateConflict {
                    expected_plan: armed.base_state.plan().generation,
                    observed_plan: current.plan().generation,
                    expected_demand: armed.base_state.plan().demand_revision,
                    observed_demand: current.plan().demand_revision,
                },
                armed: Box::new(armed),
            });
        }
        if observed_demand_revision != armed.candidate_state.plan().demand_revision {
            return Err(ScreenPlanCommitFailure {
                error: ScreenPlanError::DemandRevisionConflict {
                    expected: armed.candidate_state.plan().demand_revision,
                    observed: observed_demand_revision,
                },
                armed: Box::new(armed),
            });
        }
        if observed_graph_generation != armed.base_graph_generation {
            return Err(ScreenPlanCommitFailure {
                error: ScreenPlanError::BaseGraphGenerationConflict {
                    expected: armed.base_graph_generation,
                    observed: observed_graph_generation,
                },
                armed: Box::new(armed),
            });
        }
        let candidate_plan = armed.candidate_state.plan().clone();
        let ArmedScreenPlan {
            base_state,
            base_graph_generation,
            candidate_state,
            admission,
            transaction_id,
            exact_worker_bytes,
            commit_activation,
            prepared_tokens,
        } = armed;
        match self
            .publication_hub
            .commit(Arc::clone(&candidate_state), commit_activation)
        {
            Ok(retirement) => Ok(CommittedScreenPlan {
                plan: candidate_plan,
                retirement,
            }),
            Err((error, commit_activation)) => Err(ScreenPlanCommitFailure {
                error,
                armed: Box::new(ArmedScreenPlan {
                    base_state,
                    base_graph_generation,
                    candidate_state,
                    admission,
                    transaction_id,
                    exact_worker_bytes,
                    commit_activation,
                    prepared_tokens,
                }),
            }),
        }
    }
}

/// Failure to canonicalize, admit, or transition a screen plan.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ScreenPlanError {
    /// Memory for a candidate plan structure could not be reserved.
    #[error("failed to allocate the candidate screen publication plan")]
    AllocationFailed,
    /// Publication retention plus subscriber slot count overflowed.
    #[error("screen publication slot count overflowed")]
    PublicationSlotCountOverflow,
    /// At least two slots are required to preserve last-good during publish.
    #[error("screen publication slot policy requires at least two total slots")]
    PublicationSlotCountTooSmall,
    /// Pending retirement byte accounting exceeded `u64`.
    #[error("pending publication retirement byte accounting overflowed")]
    RetirementAccountingOverflow,
    /// Unreclaimed retired pools alone exceed current admission capacity.
    #[error(
        "pending publication retirement exceeds {resource:?}: requested {requested}, available {available}"
    )]
    RetirementPressure {
        /// Limiting configured or backend capacity.
        resource: ScreenResourceKind,
        /// Retired bytes still pinned by readers.
        requested: u64,
        /// Explicit bytes available.
        available: u64,
    },
    /// The independent screen-plan generation counter overflowed.
    #[error("screen publication plan generation exhausted")]
    GenerationExhausted,
    /// The authoritative demand revision counter overflowed.
    #[error("input publication demand revision exhausted")]
    DemandRevisionExhausted,
    /// The coordinator exhausted its non-zero transaction identity space.
    #[error("screen publication transaction identity exhausted")]
    TransactionIdentityExhausted,
    /// A preparation exhausted its non-zero worker ticket nonce space.
    #[error("screen publication worker nonce exhausted")]
    WorkerNonceExhausted,
    /// A worker ticket exhausted its non-zero allocation identity space.
    #[error("screen publication resource lifetime nonce exhausted")]
    ResourceLifetimeNonceExhausted,
    /// The compatibility mirror did not name an ordinary candidate branch.
    #[error("compatibility mirror descriptor is absent from the candidate plan")]
    CompatibilityBranchMissing,
    /// Compatibility surface selection named a non-surface branch.
    #[error("compatibility surface selection must name an ordinary Surface branch")]
    CompatibilitySurfaceKindMismatch,
    /// Compatibility zones selection named a non-zones branch.
    #[error("compatibility zones selection must name an ordinary Zones branch")]
    CompatibilityZonesKindMismatch,
    /// A candidate did not advance beyond the committed demand revision.
    #[error(
        "input publication demand revision must advance: committed {committed:?}, candidate {candidate:?}"
    )]
    DemandRevisionNotMonotonic {
        /// Current committed revision.
        committed: InputPublicationDemandRevision,
        /// Rejected candidate revision.
        candidate: InputPublicationDemandRevision,
    },
    /// Checked descriptor resource arithmetic exceeded `u64`.
    #[error(
        "resource arithmetic overflow for {resource:?}: requested {requested}, available {available}, descriptor {descriptor:?}"
    )]
    ArithmeticOverflow {
        /// Descriptor whose resource arithmetic failed.
        descriptor: Arc<ResolvedScreenPublicationDescriptor>,
        /// Limiting resource category.
        resource: ScreenResourceKind,
        /// Exact mathematical byte or element request.
        requested: u128,
        /// Largest representable ledger value.
        available: u64,
    },
    /// Candidate high-water mark exceeds an explicit byte capacity.
    #[error(
        "resource exhausted for {resource:?}: requested {requested}, available {available}, descriptor {descriptor:?}"
    )]
    ResourceExhausted {
        /// Descriptor whose admission crossed the capacity.
        descriptor: Arc<ResolvedScreenPublicationDescriptor>,
        /// Limiting configured or backend capacity.
        resource: ScreenResourceKind,
        /// Old-plus-staged bytes requested.
        requested: u64,
        /// Explicit bytes available.
        available: u64,
    },
    /// An exact worker resource name was empty.
    #[error("exact worker resource names must be non-empty")]
    EmptyResourceName,
    /// An exact worker resource accounting scope was empty.
    #[error("exact worker resource accounting scopes must be non-empty")]
    EmptyResourceScope,
    /// A ledger entry used a metric or capacity kind rather than byte accounting.
    #[error("{resource:?} is not a valid exact worker allocation-accounting kind")]
    InvalidExactResourceKind {
        /// Rejected non-accounting resource category.
        resource: ScreenResourceKind,
    },
    /// An extra exact entry did not bind to a required retained-lifetime scope.
    #[error("exact worker resource {name} references unknown accounting scope {scope}")]
    UnknownExactResourceScope {
        /// Unique extra resource name.
        name: Arc<str>,
        /// Unknown required accounting scope.
        scope: Arc<str>,
    },
    /// An exact worker ledger named one resource more than once.
    #[error("exact worker resource {resource} appears more than once")]
    DuplicateExactResourceName {
        /// Duplicated stable resource name.
        resource: Arc<str>,
    },
    /// An exact worker ledger total overflowed.
    #[error("exact worker resource ledger overflow at {resource}")]
    ExactLedgerOverflow {
        /// Opaque worker resource whose addition overflowed.
        resource: Arc<str>,
    },
    /// An exact allocation had no ticket-bound worker lifetime.
    #[error("exact worker resource {resource} has no bound allocation lifetime")]
    MissingResourceLifetime {
        /// Resource missing its lifetime proof.
        resource: Arc<str>,
    },
    /// More than one lifetime claimed the same exact allocation name.
    #[error("exact worker resource {resource} has duplicate allocation lifetimes")]
    DuplicateResourceLifetime {
        /// Resource with duplicate lifetime proofs.
        resource: Arc<str>,
    },
    /// A lifetime was supplied for an allocation absent from the exact ledger.
    #[error("allocation lifetime references unexpected exact worker resource {resource}")]
    UnexpectedResourceLifetime {
        /// Resource absent from the ledger.
        resource: Arc<str>,
    },
    /// An allocation lifetime belongs to another worker ticket.
    #[error("allocation lifetime for {resource} belongs to another worker ticket")]
    ResourceLifetimeTicketMismatch {
        /// Foreign resource lifetime.
        resource: Arc<str>,
    },
    /// Lifetime accounting differs from its exact ledger entry.
    #[error("allocation lifetime accounting differs for exact worker resource {resource}")]
    ResourceLifetimeAccountingMismatch {
        /// Resource whose lifetime description differs.
        resource: Arc<str>,
    },
    /// Worker ledger omitted a required candidate resource.
    #[error("exact worker ledger omitted required {resource:?} resource {name}")]
    MissingExactResource {
        /// Stable candidate-local resource name.
        name: Arc<str>,
        /// Required resource category.
        resource: ScreenResourceKind,
    },
    /// Worker ledger used the wrong category for a required accounting name.
    #[error("exact worker ledger reported the wrong {resource:?} category for required {name}")]
    ExactResourceKindMismatch {
        /// Required candidate-local resource name.
        name: Arc<str>,
        /// Mismatched resource category.
        resource: ScreenResourceKind,
    },
    /// Worker ledger understated a required candidate allocation.
    #[error(
        "exact worker resource {name} understated {resource:?}: observed {observed}, minimum {minimum}"
    )]
    UnderstatedExactResource {
        /// Stable candidate-local resource name.
        name: Arc<str>,
        /// Required resource category.
        resource: ScreenResourceKind,
        /// Required minimum allocation bytes.
        minimum: u64,
        /// Worker-reported exact allocation bytes.
        observed: u64,
    },
    /// Internal retained accounting fell below the canonical plan minimum.
    #[error(
        "retained exact accounting underflow: retained {retained} bytes, minimum {minimum} bytes"
    )]
    RetainedAccountingUnderflow {
        /// Retained exact committed bytes.
        retained: u64,
        /// Canonical planner minimum bytes.
        minimum: u64,
    },
    /// Worker source has no staged delta in this transaction.
    #[error("worker source {source_id} has no staged plan delta")]
    UnexpectedWorkerSource {
        /// Unexpected stable source identity.
        source_id: CaptureSourceId,
    },
    /// A source already owns its one issued ticket for this transaction.
    #[error("worker source {source_id} already has an issued preparation ticket")]
    WorkerTicketAlreadyIssued {
        /// Source whose ticket was already issued.
        source_id: CaptureSourceId,
    },
    /// Worker token belongs to another complete candidate capability.
    #[error("worker token belongs to another candidate plan")]
    WorkerCandidateMismatch {
        /// Expected candidate generation.
        expected: ScreenPlanGeneration,
        /// Observed candidate generation.
        observed: ScreenPlanGeneration,
    },
    /// Worker token targets another plan generation.
    #[error("worker token plan mismatch: expected {expected:?}, observed {observed:?}")]
    WorkerPlanGenerationMismatch {
        /// Candidate plan generation.
        expected: ScreenPlanGeneration,
        /// Token plan generation.
        observed: ScreenPlanGeneration,
    },
    /// Worker token belongs to another demand revision.
    #[error("worker token demand mismatch: expected {expected:?}, observed {observed:?}")]
    WorkerDemandRevisionMismatch {
        /// Candidate demand revision.
        expected: InputPublicationDemandRevision,
        /// Token demand revision.
        observed: InputPublicationDemandRevision,
    },
    /// Worker token targets another transaction.
    #[error("worker token transaction mismatch: expected {expected:?}, observed {observed:?}")]
    WorkerTransactionMismatch {
        /// Candidate transaction identity.
        expected: ScreenPlanTransactionId,
        /// Token transaction identity.
        observed: ScreenPlanTransactionId,
    },
    /// Worker token did not carry the nonce issued for its source.
    #[error("worker source {source_id} returned unissued nonce {observed}")]
    WorkerNonceMismatch {
        /// Source whose nonce did not match.
        source_id: CaptureSourceId,
        /// Unrecognized worker nonce.
        observed: NonZeroU64,
    },
    /// A source acknowledged the same candidate more than once.
    #[error("worker source {source_id} acknowledged the candidate twice")]
    DuplicateWorkerAcknowledgement {
        /// Duplicated source identity.
        source_id: CaptureSourceId,
    },
    /// A required source has not acknowledged exact resources.
    #[error("worker source {source_id} has not acknowledged exact resources")]
    MissingWorkerAcknowledgement {
        /// Missing source identity.
        source_id: CaptureSourceId,
    },
    /// Active plan generation changed during preparation.
    #[error("base plan generation conflict: expected {expected:?}, observed {observed:?}")]
    BasePlanGenerationConflict {
        /// Plan generation captured at prepare.
        expected: ScreenPlanGeneration,
        /// Plan generation observed at arm or commit.
        observed: ScreenPlanGeneration,
    },
    /// The entire committed authority changed after preparation.
    #[error(
        "base committed state conflict: plan {expected_plan:?}/{observed_plan:?}, demand {expected_demand:?}/{observed_demand:?}"
    )]
    BaseCommittedStateConflict {
        /// Plan generation captured at prepare.
        expected_plan: ScreenPlanGeneration,
        /// Plan generation currently committed.
        observed_plan: ScreenPlanGeneration,
        /// Demand revision captured at prepare.
        expected_demand: InputPublicationDemandRevision,
        /// Demand revision currently committed.
        observed_demand: InputPublicationDemandRevision,
    },
    /// Observed demand changed during preparation or before commit.
    #[error("demand revision conflict: expected {expected:?}, observed {observed:?}")]
    DemandRevisionConflict {
        /// Candidate demand revision.
        expected: InputPublicationDemandRevision,
        /// Revision observed at the fence.
        observed: InputPublicationDemandRevision,
    },
    /// Structural input-graph generation changed during preparation.
    #[error("base graph generation conflict: expected {expected:?}, observed {observed:?}")]
    BaseGraphGenerationConflict {
        /// Graph generation captured at prepare.
        expected: ScreenInputGraphGeneration,
        /// Graph generation observed at arm or commit.
        observed: ScreenInputGraphGeneration,
    },
}

fn validate_exact_worker_ledgers(preparing: &PreparingScreenPlan) -> Result<u64, ScreenPlanError> {
    if preparing.prepared_tokens.is_empty() {
        return Ok(0);
    }
    let owner = preparing
        .candidate
        .branches()
        .first()
        .or_else(|| preparing.base_state.plan().branches().first())
        .map(ScreenBranchDemand::descriptor)
        .expect("worker tokens require an active or candidate descriptor");
    let mut exact_worker_bytes = 0_u64;
    for token in &preparing.prepared_tokens {
        exact_worker_bytes = checked_sum(
            owner,
            ScreenResourceKind::WorkerExactLedger,
            exact_worker_bytes,
            token.exact_ledger.total_bytes,
        )?;
    }
    let staged_publication_bytes = checked_sum(
        owner,
        ScreenResourceKind::PublicationSubscriberSlots,
        preparing.admission.staged.publication_retention_bytes,
        preparing.admission.staged.publication_subscriber_slot_bytes,
    )?;
    let exact_staged_bytes = checked_sum(
        owner,
        ScreenResourceKind::WorkerExactLedger,
        exact_worker_bytes,
        staged_publication_bytes,
    )?;
    let exact_overlap = checked_sum(
        owner,
        ScreenResourceKind::OverlapTotal,
        preparing.admission.active.total_bytes,
        exact_staged_bytes,
    )?;
    if exact_overlap > preparing.capacity.byte_budget {
        return Err(ScreenPlanError::ResourceExhausted {
            descriptor: Arc::new(owner.clone()),
            resource: ScreenResourceKind::ByteBudget,
            requested: exact_overlap,
            available: preparing.capacity.byte_budget,
        });
    }
    if exact_overlap > preparing.capacity.backend_capacity {
        return Err(ScreenPlanError::ResourceExhausted {
            descriptor: Arc::new(owner.clone()),
            resource: ScreenResourceKind::BackendCapacity,
            requested: exact_overlap,
            available: preparing.capacity.backend_capacity,
        });
    }
    Ok(exact_worker_bytes)
}

fn validate_resource_coverage(
    required: &[ScreenRequiredResourceMinimum],
    exact: &ScreenExactResourceLedger,
) -> Result<(), ScreenPlanError> {
    for resource in exact.resources() {
        if let Ok(index) = required
            .binary_search_by(|requirement| requirement.name.as_ref().cmp(resource.name.as_ref()))
        {
            let requirement = &required[index];
            if resource.accounting_scope != requirement.name {
                return Err(ScreenPlanError::UnknownExactResourceScope {
                    name: Arc::clone(&resource.name),
                    scope: Arc::clone(&resource.accounting_scope),
                });
            }
            if requirement.resource != resource.resource {
                return Err(ScreenPlanError::ExactResourceKindMismatch {
                    name: Arc::clone(&resource.name),
                    resource: resource.resource,
                });
            }
            if resource.bytes < requirement.minimum_bytes {
                return Err(ScreenPlanError::UnderstatedExactResource {
                    name: Arc::clone(&resource.name),
                    resource: resource.resource,
                    minimum: requirement.minimum_bytes,
                    observed: resource.bytes,
                });
            }
        } else if required
            .binary_search_by(|requirement| {
                requirement
                    .name
                    .as_ref()
                    .cmp(resource.accounting_scope.as_ref())
            })
            .is_err()
        {
            return Err(ScreenPlanError::UnknownExactResourceScope {
                name: Arc::clone(&resource.name),
                scope: Arc::clone(&resource.accounting_scope),
            });
        }
    }
    let mut exact_index = 0;
    for requirement in required {
        while exact
            .resources()
            .get(exact_index)
            .is_some_and(|resource| resource.name < requirement.name)
        {
            exact_index += 1;
        }
        if exact.resources().get(exact_index).is_none_or(|resource| {
            resource.name != requirement.name || resource.resource != requirement.resource
        }) {
            return Err(ScreenPlanError::MissingExactResource {
                name: Arc::clone(&requirement.name),
                resource: requirement.resource,
            });
        }
    }
    Ok(())
}

fn validate_resource_lifetimes(
    ticket: &ScreenWorkerPreparationTicket,
    exact: &ScreenExactResourceLedger,
    lifetimes: &[ScreenResourceLifetime],
) -> Result<Vec<ScreenResourceLifetime>, ScreenPlanError> {
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(lifetimes.len())
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    for lifetime in lifetimes {
        let inner = &lifetime.inner;
        if inner.source_id != ticket.source_id
            || inner.plan_generation != ticket.plan_generation
            || inner.demand_revision != ticket.demand_revision
            || inner.transaction_id != ticket.transaction_id
            || inner.worker_nonce != ticket.worker_nonce
        {
            return Err(ScreenPlanError::ResourceLifetimeTicketMismatch {
                resource: Arc::clone(inner.resource.name()),
            });
        }
        ordered.push(lifetime.clone());
    }
    ordered.sort_unstable_by(|left, right| {
        left.resource()
            .name
            .cmp(&right.resource().name)
            .then_with(|| {
                left.inner
                    .allocation_nonce
                    .cmp(&right.inner.allocation_nonce)
            })
    });
    if let Some(duplicate) = ordered
        .windows(2)
        .find(|pair| pair[0].resource().name == pair[1].resource().name)
    {
        return Err(ScreenPlanError::DuplicateResourceLifetime {
            resource: Arc::clone(duplicate[1].resource().name()),
        });
    }

    let mut exact_index = 0;
    let mut lifetime_index = 0;
    while let (Some(resource), Some(lifetime)) = (
        exact.resources().get(exact_index),
        ordered.get(lifetime_index),
    ) {
        match resource.name.cmp(&lifetime.resource().name) {
            std::cmp::Ordering::Less => {
                return Err(ScreenPlanError::MissingResourceLifetime {
                    resource: Arc::clone(&resource.name),
                });
            }
            std::cmp::Ordering::Greater => {
                return Err(ScreenPlanError::UnexpectedResourceLifetime {
                    resource: Arc::clone(lifetime.resource().name()),
                });
            }
            std::cmp::Ordering::Equal => {
                if resource != lifetime.resource() {
                    return Err(ScreenPlanError::ResourceLifetimeAccountingMismatch {
                        resource: Arc::clone(&resource.name),
                    });
                }
                exact_index += 1;
                lifetime_index += 1;
            }
        }
    }
    if let Some(resource) = exact.resources().get(exact_index) {
        return Err(ScreenPlanError::MissingResourceLifetime {
            resource: Arc::clone(&resource.name),
        });
    }
    if let Some(lifetime) = ordered.get(lifetime_index) {
        return Err(ScreenPlanError::UnexpectedResourceLifetime {
            resource: Arc::clone(lifetime.resource().name()),
        });
    }
    Ok(ordered)
}

fn bind_retained_resources(
    required: &[ScreenRequiredResourceMinimum],
    exact: &ScreenExactResourceLedger,
    lifetimes: &[ScreenResourceLifetime],
) -> Result<ScreenRetainedResourceLedger, ScreenPlanError> {
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(exact.resources().len())
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    for (resource, lifetime) in exact.resources().iter().zip(lifetimes) {
        let requirement = required
            .binary_search_by(|requirement| requirement.name.cmp(&resource.name))
            .or_else(|_| {
                required.binary_search_by(|requirement| {
                    requirement.name.cmp(&resource.accounting_scope)
                })
            })
            .ok()
            .and_then(|index| required.get(index))
            .ok_or_else(|| ScreenPlanError::UnknownExactResourceScope {
                name: Arc::clone(&resource.name),
                scope: Arc::clone(&resource.accounting_scope),
            })?;
        entries.push(ScreenRetainedResourceEntry {
            name: Arc::clone(&resource.name),
            retention: requirement.retention.clone(),
            descriptor: Arc::clone(&requirement.descriptor),
            lifetime: lifetime.clone(),
        });
    }
    let entries = normalize_retained_resources(entries)?;
    Ok(ScreenRetainedResourceLedger {
        entries: Arc::new(entries),
        total_bytes: exact.total_bytes,
    })
}

fn candidate_retained_resources(
    base: &ScreenRetainedResourceLedger,
    candidate: &ScreenCapturePlan,
    prepared_tokens: &[ScreenPreparedWorkerToken],
) -> Result<ScreenRetainedResourceLedger, ScreenPlanError> {
    let mut entries = Vec::new();
    let prepared_count = prepared_tokens.iter().try_fold(0_usize, |count, token| {
        count
            .checked_add(token.retained_resources.entries.len())
            .ok_or(ScreenPlanError::AllocationFailed)
    })?;
    let maximum_count = base
        .entries
        .len()
        .checked_add(prepared_count)
        .ok_or(ScreenPlanError::AllocationFailed)?;
    entries
        .try_reserve_exact(maximum_count)
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    for entry in base.entries.iter().filter(|entry| match &entry.retention {
        ScreenResourceRetentionKey::Physical(descriptor) => candidate.contains_physical(descriptor),
        ScreenResourceRetentionKey::PhysicalPlane(descriptor) => {
            plan_retains_physical_plane(candidate, descriptor)
        }
        ScreenResourceRetentionKey::Branch(descriptor) => candidate.contains_descriptor(descriptor),
    }) {
        entries.push(entry.clone());
    }
    for entry in prepared_tokens
        .iter()
        .flat_map(|token| token.retained_resources.entries.iter())
    {
        entries.push(entry.clone());
    }
    let entries = normalize_retained_resources(entries)?;
    let mut total_bytes = 0_u64;
    for entry in &entries {
        total_bytes = checked_sum(
            &entry.descriptor,
            ScreenResourceKind::WorkerExactLedger,
            total_bytes,
            entry.bytes(),
        )?;
    }
    let minimum_bytes = full_plan_ledger(candidate)?.total_bytes;
    if total_bytes < minimum_bytes {
        return Err(ScreenPlanError::RetainedAccountingUnderflow {
            retained: total_bytes,
            minimum: minimum_bytes,
        });
    }
    Ok(ScreenRetainedResourceLedger {
        entries: Arc::new(entries),
        total_bytes,
    })
}

fn normalize_retained_resources(
    mut entries: Vec<ScreenRetainedResourceEntry>,
) -> Result<Vec<ScreenRetainedResourceEntry>, ScreenPlanError> {
    entries.sort_unstable_by(compare_retained_resources);
    if let Some(duplicate) = entries
        .windows(2)
        .find(|pair| pair[0].retention == pair[1].retention && pair[0].name == pair[1].name)
    {
        return Err(ScreenPlanError::DuplicateExactResourceName {
            resource: Arc::clone(&duplicate[1].name),
        });
    }
    Ok(entries)
}

fn compare_retained_resources(
    left: &ScreenRetainedResourceEntry,
    right: &ScreenRetainedResourceEntry,
) -> std::cmp::Ordering {
    left.retention
        .cmp(&right.retention)
        .then_with(|| left.name.cmp(&right.name))
}

fn required_source_minimums(
    active: &ScreenCapturePlan,
    candidate: &ScreenCapturePlan,
) -> Result<Vec<ScreenSourceResourceContract>, ScreenPlanError> {
    let source_ids = required_source_ids(active, candidate)?;
    let mut contracts = Vec::new();
    for source_id in source_ids {
        reserve_one(&mut contracts)?;
        contracts.push(ScreenSourceResourceContract {
            source_id,
            required_minimums: Arc::new(Vec::new()),
        });
    }

    for (index, reduction) in candidate.physical_reductions().iter().enumerate() {
        let owner = candidate.branches()[reduction.branch_indices()[0]].descriptor();
        let active_reduction = physical_reduction(active, reduction.descriptor());
        if reduction_retains_physical_plane(candidate, reduction)
            && active_reduction.is_none_or(|active_reduction| {
                !reduction_retains_physical_plane(active, active_reduction)
            })
        {
            let extent = reduction.descriptor().reduction_extent();
            let stride = checked_product(
                owner,
                ScreenResourceKind::PhysicalRowStride,
                u64::from(extent.width()),
                TARGET_PIXEL_BYTES,
            )?;
            let plane_bytes = checked_product(
                owner,
                ScreenResourceKind::PhysicalPlane,
                stride,
                u64::from(extent.height()),
            )?;
            push_required_minimum(
                &mut contracts,
                &owner.source_epoch().source_id,
                ScreenRequiredResourceMinimum {
                    name: Arc::from(format!("physical-{index}-plane")),
                    resource: ScreenResourceKind::PhysicalPlane,
                    descriptor: Arc::new(owner.clone()),
                    minimum_bytes: plane_bytes,
                    retention: ScreenResourceRetentionKey::PhysicalPlane(
                        reduction.descriptor().clone(),
                    ),
                },
            )?;
        }
        if active_reduction.is_some() {
            continue;
        }
        for (suffix, resource) in [
            (
                "backend-domain-total",
                ScreenResourceKind::BackendAllocation,
            ),
            ("api-domain-total", ScreenResourceKind::ApiAllocation),
        ] {
            push_required_minimum(
                &mut contracts,
                &owner.source_epoch().source_id,
                ScreenRequiredResourceMinimum {
                    name: Arc::from(format!("physical-{index}-{suffix}")),
                    resource,
                    descriptor: Arc::new(owner.clone()),
                    minimum_bytes: 0,
                    retention: ScreenResourceRetentionKey::Physical(reduction.descriptor().clone()),
                },
            )?;
        }
    }

    for (index, branch) in candidate.branches().iter().enumerate() {
        if active.contains_descriptor(branch.descriptor()) {
            continue;
        }
        let descriptor = branch.descriptor();
        push_required_minimum(
            &mut contracts,
            &descriptor.source_epoch().source_id,
            ScreenRequiredResourceMinimum {
                name: Arc::from(format!("branch-{index}-processing-profile-total")),
                resource: ScreenResourceKind::ProcessingProfileState,
                descriptor: Arc::new(descriptor.clone()),
                minimum_bytes: 0,
                retention: ScreenResourceRetentionKey::Branch(descriptor.clone()),
            },
        )?;
    }
    for contract in &mut contracts {
        Arc::get_mut(&mut contract.required_minimums)
            .expect("source resource contract is uniquely owned during preparation")
            .sort_unstable_by(|left, right| left.name.cmp(&right.name));
    }
    Ok(contracts)
}

fn push_required_minimum(
    contracts: &mut [ScreenSourceResourceContract],
    source_id: &CaptureSourceId,
    resource: ScreenRequiredResourceMinimum,
) -> Result<(), ScreenPlanError> {
    let contract = contracts
        .binary_search_by(|contract| contract.source_id.as_str().cmp(source_id.as_str()))
        .ok()
        .and_then(|index| contracts.get_mut(index))
        .ok_or_else(|| ScreenPlanError::UnexpectedWorkerSource {
            source_id: source_id.clone(),
        })?;
    let resources = Arc::get_mut(&mut contract.required_minimums)
        .expect("source resource contract is uniquely owned during preparation");
    reserve_one(resources)?;
    resources.push(resource);
    Ok(())
}

fn canonicalize_candidate(
    current: &ScreenCapturePlan,
    demands: impl IntoIterator<Item = ResolvedScreenBranchDemand>,
    compatibility: Option<&ScreenCompatibilitySelection>,
    demand_revision: InputPublicationDemandRevision,
) -> Result<ScreenCapturePlan, ScreenPlanError> {
    let mut resolved = Vec::new();
    for demand in demands {
        reserve_one(&mut resolved)?;
        resolved.push(demand);
    }
    resolved.sort_unstable_by(|left, right| left.descriptor().cmp(right.descriptor()));

    let mut branches: Vec<ScreenBranchDemand> = Vec::new();
    for demand in resolved {
        let (descriptor, requested_hz) = demand.into_parts();
        if let Some(previous) = branches.last_mut()
            && previous.descriptor == descriptor
        {
            previous.requested_hz = previous.requested_hz.max(requested_hz);
            continue;
        }
        reserve_one(&mut branches)?;
        branches.push(ScreenBranchDemand {
            descriptor,
            requested_hz,
        });
    }

    let compatibility_surface = compatibility
        .map(|selection| {
            branches
                .binary_search_by(|branch| branch.descriptor.cmp(selection.surface()))
                .map_err(|_| ScreenPlanError::CompatibilityBranchMissing)
        })
        .transpose()?;
    let compatibility_zones = compatibility
        .and_then(ScreenCompatibilitySelection::zones)
        .map(|descriptor| {
            branches
                .binary_search_by(|branch| branch.descriptor.cmp(descriptor))
                .map_err(|_| ScreenPlanError::CompatibilityBranchMissing)
        })
        .transpose()?;
    let physical_reductions = group_physical_reductions(&branches)?;

    let changed = current.branches.as_slice() != branches.as_slice()
        || current.compatibility_surface != compatibility_surface
        || current.compatibility_zones != compatibility_zones;
    let generation = if changed {
        current.generation.next()?
    } else {
        current.generation
    };
    Ok(ScreenCapturePlan {
        generation,
        demand_revision,
        branches: Arc::new(branches),
        physical_reductions: Arc::new(physical_reductions),
        compatibility_surface,
        compatibility_zones,
    })
}

fn group_physical_reductions(
    branches: &[ScreenBranchDemand],
) -> Result<Vec<ScreenPhysicalReductionDemand>, ScreenPlanError> {
    let mut reductions: Vec<ScreenPhysicalReductionDemand> = Vec::new();
    for (index, branch) in branches.iter().enumerate() {
        if let Some(previous) = reductions.last_mut()
            && previous.descriptor == *branch.descriptor.physical()
        {
            reserve_one(&mut previous.branch_indices)?;
            previous.branch_indices.push(index);
            previous.requested_hz = previous.requested_hz.max(branch.requested_hz);
            continue;
        }
        let mut branch_indices = Vec::new();
        reserve_one(&mut branch_indices)?;
        branch_indices.push(index);
        reserve_one(&mut reductions)?;
        reductions.push(ScreenPhysicalReductionDemand {
            descriptor: branch.descriptor.physical().clone(),
            branch_indices,
            requested_hz: branch.requested_hz,
        });
    }
    Ok(reductions)
}

fn admit_candidate(
    active: &ScreenCapturePlan,
    candidate: &ScreenCapturePlan,
    active_exact_bytes: u64,
    pending_retirement_bytes: u64,
    slot_policy: ScreenPublicationSlotPolicy,
    capacity: ScreenAdmissionCapacity,
) -> Result<ScreenPlanAdmissionLedger, ScreenPlanError> {
    for (available, resource) in [
        (capacity.byte_budget, ScreenResourceKind::ByteBudget),
        (
            capacity.backend_capacity,
            ScreenResourceKind::BackendCapacity,
        ),
    ] {
        if pending_retirement_bytes > available {
            return Err(ScreenPlanError::RetirementPressure {
                resource,
                requested: pending_retirement_bytes,
                available,
            });
        }
    }
    let mut active_ledger = full_plan_ledger(active)?;
    if active_exact_bytes < active_ledger.total_bytes {
        return Err(ScreenPlanError::RetainedAccountingUnderflow {
            retained: active_exact_bytes,
            minimum: active_ledger.total_bytes,
        });
    }
    active_ledger.worker_additional_bytes = active_exact_bytes - active_ledger.total_bytes;
    active_ledger.total_bytes = active_exact_bytes;
    add_publication_slot_usage(&mut active_ledger, active.branches(), slot_policy)?;
    active_ledger.pending_publication_retirement_bytes = pending_retirement_bytes;
    active_ledger.total_bytes = checked_optional_sum(
        admission_owner(active, candidate),
        ScreenResourceKind::PendingPublicationRetirement,
        active_ledger.total_bytes,
        pending_retirement_bytes,
    )?;
    let mut candidate_ledger = full_plan_ledger(candidate)?;
    add_publication_slot_usage(&mut candidate_ledger, candidate.branches(), slot_policy)?;
    let mut staged_ledger = staged_plan_ledger(active, candidate)?;
    let staged_branches = candidate
        .branches()
        .iter()
        .filter(|branch| !active.contains_descriptor(branch.descriptor()));
    add_publication_slot_usage(&mut staged_ledger, staged_branches, slot_policy)?;
    let owner = admission_owner(active, candidate);
    let overlap = merge_ledgers(active_ledger, staged_ledger, owner)?;

    if overlap.total_bytes > capacity.byte_budget {
        let owner = owner.expect("non-zero admission bytes require a descriptor");
        return Err(ScreenPlanError::ResourceExhausted {
            descriptor: Arc::new(owner.clone()),
            resource: ScreenResourceKind::ByteBudget,
            requested: overlap.total_bytes,
            available: capacity.byte_budget,
        });
    }
    if overlap.total_bytes > capacity.backend_capacity {
        let owner = owner.expect("non-zero admission bytes require a descriptor");
        return Err(ScreenPlanError::ResourceExhausted {
            descriptor: Arc::new(owner.clone()),
            resource: ScreenResourceKind::BackendCapacity,
            requested: overlap.total_bytes,
            available: capacity.backend_capacity,
        });
    }
    Ok(ScreenPlanAdmissionLedger {
        active: active_ledger,
        candidate: candidate_ledger,
        staged: staged_ledger,
        overlap,
    })
}

fn full_plan_ledger(plan: &ScreenCapturePlan) -> Result<ScreenResourceLedger, ScreenPlanError> {
    let mut ledger = ScreenResourceLedger::default();
    for reduction in plan.physical_reductions() {
        let owner = &plan.branches()[reduction.branch_indices()[0]].descriptor;
        add_physical_usage(
            &mut ledger,
            reduction.descriptor(),
            owner,
            reduction_retains_physical_plane(plan, reduction),
        )?;
    }
    Ok(ledger)
}

fn staged_plan_ledger(
    active: &ScreenCapturePlan,
    candidate: &ScreenCapturePlan,
) -> Result<ScreenResourceLedger, ScreenPlanError> {
    let mut ledger = ScreenResourceLedger::default();
    for reduction in candidate.physical_reductions() {
        let owner = &candidate.branches()[reduction.branch_indices()[0]].descriptor;
        if let Some(active_reduction) = physical_reduction(active, reduction.descriptor()) {
            if reduction_retains_physical_plane(candidate, reduction)
                && !reduction_retains_physical_plane(active, active_reduction)
            {
                add_physical_plane_usage(&mut ledger, reduction.descriptor(), owner)?;
            }
        } else {
            add_physical_usage(
                &mut ledger,
                reduction.descriptor(),
                owner,
                reduction_retains_physical_plane(candidate, reduction),
            )?;
        }
    }
    Ok(ledger)
}

fn add_publication_slot_usage<'a>(
    ledger: &mut ScreenResourceLedger,
    branches: impl IntoIterator<Item = &'a ScreenBranchDemand>,
    slot_policy: ScreenPublicationSlotPolicy,
) -> Result<(), ScreenPlanError> {
    for branch in branches {
        let descriptor = branch.descriptor();
        let payload_bytes = match descriptor.kind() {
            ScreenPublicationKind::Surface
                if matches!(
                    descriptor.required_residency(),
                    ScreenPublicationResidency::Cpu
                ) =>
            {
                let extent = descriptor.geometry().output_extent();
                let pixels = checked_product(
                    descriptor,
                    ScreenResourceKind::PublicationRetention,
                    u64::from(extent.width()),
                    u64::from(extent.height()),
                )?;
                checked_product(
                    descriptor,
                    ScreenResourceKind::PublicationRetention,
                    pixels,
                    TARGET_PIXEL_BYTES,
                )?
            }
            ScreenPublicationKind::Surface => 0,
            ScreenPublicationKind::Zones { columns, rows } => {
                let cells = checked_product(
                    descriptor,
                    ScreenResourceKind::PublicationRetention,
                    u64::from(columns.get()),
                    u64::from(rows.get()),
                )?;
                checked_product(
                    descriptor,
                    ScreenResourceKind::PublicationRetention,
                    cells,
                    ZONE_COLOR_BYTES,
                )?
            }
        };
        let retention_bytes = checked_product(
            descriptor,
            ScreenResourceKind::PublicationRetention,
            payload_bytes,
            u64::from(slot_policy.retention_slots().get()),
        )?;
        let subscriber_bytes = checked_product(
            descriptor,
            ScreenResourceKind::PublicationSubscriberSlots,
            payload_bytes,
            u64::from(slot_policy.subscriber_slots()),
        )?;
        checked_add_assign(
            &mut ledger.publication_retention_bytes,
            retention_bytes,
            descriptor,
            ScreenResourceKind::PublicationRetention,
        )?;
        checked_add_assign(
            &mut ledger.publication_subscriber_slot_bytes,
            subscriber_bytes,
            descriptor,
            ScreenResourceKind::PublicationSubscriberSlots,
        )?;
        checked_add_assign(
            &mut ledger.total_bytes,
            retention_bytes,
            descriptor,
            ScreenResourceKind::PublicationRetention,
        )?;
        checked_add_assign(
            &mut ledger.total_bytes,
            subscriber_bytes,
            descriptor,
            ScreenResourceKind::PublicationSubscriberSlots,
        )?;
    }
    Ok(())
}

fn add_physical_usage(
    ledger: &mut ScreenResourceLedger,
    descriptor: &ScreenPhysicalReductionDescriptor,
    owner: &ResolvedScreenPublicationDescriptor,
    retains_plane: bool,
) -> Result<(), ScreenPlanError> {
    let extent = descriptor.reduction_extent();
    let pixels = checked_product(
        owner,
        ScreenResourceKind::PhysicalPixels,
        u64::from(extent.width()),
        u64::from(extent.height()),
    )?;
    let stride = checked_product(
        owner,
        ScreenResourceKind::PhysicalRowStride,
        u64::from(extent.width()),
        TARGET_PIXEL_BYTES,
    )?;
    checked_add_assign(
        &mut ledger.physical_pixels,
        pixels,
        owner,
        ScreenResourceKind::PhysicalPixels,
    )?;
    checked_add_assign(
        &mut ledger.physical_row_stride_bytes,
        stride,
        owner,
        ScreenResourceKind::PhysicalRowStride,
    )?;
    if retains_plane {
        add_physical_plane_usage(ledger, descriptor, owner)?;
    }
    Ok(())
}

fn add_physical_plane_usage(
    ledger: &mut ScreenResourceLedger,
    descriptor: &ScreenPhysicalReductionDescriptor,
    owner: &ResolvedScreenPublicationDescriptor,
) -> Result<(), ScreenPlanError> {
    let extent = descriptor.reduction_extent();
    let stride = checked_product(
        owner,
        ScreenResourceKind::PhysicalRowStride,
        u64::from(extent.width()),
        TARGET_PIXEL_BYTES,
    )?;
    let plane_bytes = checked_product(
        owner,
        ScreenResourceKind::PhysicalPlane,
        stride,
        u64::from(extent.height()),
    )?;
    checked_add_assign(
        &mut ledger.physical_plane_bytes,
        plane_bytes,
        owner,
        ScreenResourceKind::PhysicalPlane,
    )?;
    checked_add_assign(
        &mut ledger.total_bytes,
        plane_bytes,
        owner,
        ScreenResourceKind::PhysicalPlane,
    )
}

fn reduction_retains_physical_plane(
    plan: &ScreenCapturePlan,
    reduction: &ScreenPhysicalReductionDemand,
) -> bool {
    reduction.branch_indices().iter().any(|branch_index| {
        plan.branches()
            .get(*branch_index)
            .is_some_and(|branch| branch_requires_materialization(branch.descriptor()))
    })
}

fn physical_reduction<'a>(
    plan: &'a ScreenCapturePlan,
    descriptor: &ScreenPhysicalReductionDescriptor,
) -> Option<&'a ScreenPhysicalReductionDemand> {
    plan.physical_reductions()
        .binary_search_by(|reduction| reduction.descriptor().cmp(descriptor))
        .ok()
        .and_then(|index| plan.physical_reductions().get(index))
}

fn plan_retains_physical_plane(
    plan: &ScreenCapturePlan,
    descriptor: &ScreenPhysicalReductionDescriptor,
) -> bool {
    physical_reduction(plan, descriptor)
        .is_some_and(|reduction| reduction_retains_physical_plane(plan, reduction))
}

fn merge_ledgers(
    left: ScreenResourceLedger,
    right: ScreenResourceLedger,
    owner: Option<&ResolvedScreenPublicationDescriptor>,
) -> Result<ScreenResourceLedger, ScreenPlanError> {
    Ok(ScreenResourceLedger {
        physical_pixels: checked_optional_sum(
            owner,
            ScreenResourceKind::PhysicalPixels,
            left.physical_pixels,
            right.physical_pixels,
        )?,
        physical_row_stride_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::PhysicalRowStride,
            left.physical_row_stride_bytes,
            right.physical_row_stride_bytes,
        )?,
        physical_plane_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::PhysicalPlane,
            left.physical_plane_bytes,
            right.physical_plane_bytes,
        )?,
        worker_additional_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::WorkerAdditional,
            left.worker_additional_bytes,
            right.worker_additional_bytes,
        )?,
        publication_retention_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::PublicationRetention,
            left.publication_retention_bytes,
            right.publication_retention_bytes,
        )?,
        publication_subscriber_slot_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::PublicationSubscriberSlots,
            left.publication_subscriber_slot_bytes,
            right.publication_subscriber_slot_bytes,
        )?,
        pending_publication_retirement_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::PendingPublicationRetirement,
            left.pending_publication_retirement_bytes,
            right.pending_publication_retirement_bytes,
        )?,
        total_bytes: checked_optional_sum(
            owner,
            ScreenResourceKind::OverlapTotal,
            left.total_bytes,
            right.total_bytes,
        )?,
    })
}

fn checked_product(
    descriptor: &ResolvedScreenPublicationDescriptor,
    resource: ScreenResourceKind,
    left: u64,
    right: u64,
) -> Result<u64, ScreenPlanError> {
    left.checked_mul(right)
        .ok_or_else(|| ScreenPlanError::ArithmeticOverflow {
            descriptor: Arc::new(descriptor.clone()),
            resource,
            requested: u128::from(left) * u128::from(right),
            available: u64::MAX,
        })
}

fn checked_sum(
    descriptor: &ResolvedScreenPublicationDescriptor,
    resource: ScreenResourceKind,
    left: u64,
    right: u64,
) -> Result<u64, ScreenPlanError> {
    left.checked_add(right)
        .ok_or_else(|| ScreenPlanError::ArithmeticOverflow {
            descriptor: Arc::new(descriptor.clone()),
            resource,
            requested: u128::from(left) + u128::from(right),
            available: u64::MAX,
        })
}

fn checked_optional_sum(
    descriptor: Option<&ResolvedScreenPublicationDescriptor>,
    resource: ScreenResourceKind,
    left: u64,
    right: u64,
) -> Result<u64, ScreenPlanError> {
    if let Some(value) = left.checked_add(right) {
        Ok(value)
    } else {
        let descriptor = descriptor.expect("non-zero ledger overflow requires a descriptor");
        Err(ScreenPlanError::ArithmeticOverflow {
            descriptor: Arc::new(descriptor.clone()),
            resource,
            requested: u128::from(left) + u128::from(right),
            available: u64::MAX,
        })
    }
}

fn checked_add_assign(
    target: &mut u64,
    value: u64,
    descriptor: &ResolvedScreenPublicationDescriptor,
    resource: ScreenResourceKind,
) -> Result<(), ScreenPlanError> {
    *target = checked_sum(descriptor, resource, *target, value)?;
    Ok(())
}

fn admission_owner<'a>(
    active: &'a ScreenCapturePlan,
    candidate: &'a ScreenCapturePlan,
) -> Option<&'a ResolvedScreenPublicationDescriptor> {
    candidate
        .branches()
        .first()
        .or_else(|| active.branches().first())
        .map(ScreenBranchDemand::descriptor)
}

fn required_source_ids(
    active: &ScreenCapturePlan,
    candidate: &ScreenCapturePlan,
) -> Result<Vec<CaptureSourceId>, ScreenPlanError> {
    let active = branches_by_source(active)?;
    let candidate = branches_by_source(candidate)?;
    let mut sources = Vec::new();
    let maximum_sources = active
        .len()
        .checked_add(candidate.len())
        .ok_or(ScreenPlanError::AllocationFailed)?;
    sources
        .try_reserve_exact(maximum_sources)
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    let mut active_index = 0;
    let mut candidate_index = 0;
    while active_index < active.len() || candidate_index < candidate.len() {
        let active_source = active
            .get(active_index)
            .map(|branch| &branch.descriptor().source_epoch().source_id);
        let candidate_source = candidate
            .get(candidate_index)
            .map(|branch| &branch.descriptor().source_epoch().source_id);
        match (active_source, candidate_source) {
            (Some(active_source), Some(candidate_source)) => {
                match active_source.as_str().cmp(candidate_source.as_str()) {
                    std::cmp::Ordering::Less => {
                        sources.push(active_source.clone());
                        active_index = source_group_end(&active, active_index);
                    }
                    std::cmp::Ordering::Greater => {
                        sources.push(candidate_source.clone());
                        candidate_index = source_group_end(&candidate, candidate_index);
                    }
                    std::cmp::Ordering::Equal => {
                        let active_end = source_group_end(&active, active_index);
                        let candidate_end = source_group_end(&candidate, candidate_index);
                        if active[active_index..active_end]
                            != candidate[candidate_index..candidate_end]
                        {
                            sources.push(active_source.clone());
                        }
                        active_index = active_end;
                        candidate_index = candidate_end;
                    }
                }
            }
            (Some(active_source), None) => {
                sources.push(active_source.clone());
                active_index = source_group_end(&active, active_index);
            }
            (None, Some(candidate_source)) => {
                sources.push(candidate_source.clone());
                candidate_index = source_group_end(&candidate, candidate_index);
            }
            (None, None) => break,
        }
    }
    Ok(sources)
}

fn branches_by_source(
    plan: &ScreenCapturePlan,
) -> Result<Vec<&ScreenBranchDemand>, ScreenPlanError> {
    let mut branches = Vec::new();
    branches
        .try_reserve_exact(plan.branches().len())
        .map_err(|_| ScreenPlanError::AllocationFailed)?;
    branches.extend(plan.branches());
    branches.sort_unstable_by(|left, right| {
        left.descriptor()
            .source_epoch()
            .source_id
            .as_str()
            .cmp(right.descriptor().source_epoch().source_id.as_str())
            .then_with(|| left.descriptor().cmp(right.descriptor()))
    });
    Ok(branches)
}

fn source_group_end(branches: &[&ScreenBranchDemand], start: usize) -> usize {
    let source = &branches[start].descriptor().source_epoch().source_id;
    branches[start..]
        .iter()
        .position(|branch| branch.descriptor().source_epoch().source_id != *source)
        .map_or(branches.len(), |offset| start + offset)
}

fn reserve_one<T>(values: &mut Vec<T>) -> Result<(), ScreenPlanError> {
    if values.len() == values.capacity() {
        values
            .try_reserve(1)
            .map_err(|_| ScreenPlanError::AllocationFailed)?;
    }
    Ok(())
}
