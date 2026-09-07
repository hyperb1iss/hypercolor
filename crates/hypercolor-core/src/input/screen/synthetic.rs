//! Synthetic exact screen publications for consumer fixtures.
//!
//! Renderer, preview, and zones consumers read `Arc<ScreenBranchPublication>`
//! snapshots leased from the publication hub. Tests that exercise those
//! consumers need real publications without a capture backend, so this
//! module drives the production plan builder and hub with a CPU synthetic
//! source and hands back the committed publications it accepts.

use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::frame::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, PhysicalOrigin, PixelExtent, SourceScale,
};
use super::hub::{
    ScreenBranchPayload, ScreenBranchPublication, ScreenPublicationColorimetry,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationMetadata, ScreenSurfacePayload,
    ScreenZonesPayload,
};
use super::plan::{
    CommittedScreenPlan, ScreenAdmissionCapacity, ScreenExactResource, ScreenExactResourceLedger,
    ScreenInputGraphGeneration, ScreenPlanBuilder, ScreenResourceLifetime, ScreenWorkerBinding,
    ScreenWorkerPreparationTicket,
};
use super::publication::{
    RegisteredScreenBranchDemand, ResolvedScreenBranchDemand, ResolvedScreenPublicationDescriptor,
    ResolvedScreenSource, ResolvedScreenSourceConfig, ScreenAspectPolicy,
    ScreenBackendResourceIdentity, ScreenCaptureBackend, ScreenColorTransformCapabilities,
    ScreenCursorCapabilities, ScreenExecutorColorCapabilities, ScreenExtentRequest,
    ScreenProcessingProfile, ScreenPublicationExecutorRequest, ScreenPublicationKind,
    ScreenPublicationRequest, ScreenResourceApi, ScreenSourceReflection, ScreenSourceSelector,
};

/// Fixture-only exact publisher backed by the production plan builder.
///
/// One publisher owns one committed CPU branch. Every publish call produces
/// a fresh immutable `Arc<ScreenBranchPublication>` whose payload matches
/// the branch descriptor exactly, so consumer tests read the same snapshot
/// shape the daemon leases from a live hub.
#[doc(hidden)]
pub struct SyntheticScreenPublisher {
    _builder: ScreenPlanBuilder,
    hub: Arc<ScreenPublicationHub>,
    descriptor: ResolvedScreenPublicationDescriptor,
    binding: ScreenWorkerBinding,
    _worker_lifetimes: Vec<ScreenResourceLifetime>,
    native_sequence: u64,
}

impl SyntheticScreenPublisher {
    /// Commit one RGBA8 surface branch at exactly `extent`.
    ///
    /// # Panics
    ///
    /// Panics when the synthetic plan cannot be prepared or committed; the
    /// CPU synthetic source is always resolvable, so a failure here is a
    /// fixture defect.
    #[must_use]
    pub fn surface(extent: PixelExtent) -> Self {
        Self::commit(extent, ScreenPublicationKind::Surface)
    }

    /// Commit one RGB zones branch projected from a source of `extent`.
    ///
    /// # Panics
    ///
    /// Panics when the synthetic plan cannot be prepared or committed.
    #[must_use]
    pub fn zones(extent: PixelExtent, columns: NonZeroU32, rows: NonZeroU32) -> Self {
        Self::commit(extent, ScreenPublicationKind::Zones { columns, rows })
    }

    /// Hub that owns the committed branch, for lease-based consumers.
    #[must_use]
    pub fn hub(&self) -> Arc<ScreenPublicationHub> {
        Arc::clone(&self.hub)
    }

    /// Resolved descriptor of the committed branch.
    #[must_use]
    pub const fn descriptor(&self) -> &ResolvedScreenPublicationDescriptor {
        &self.descriptor
    }

    /// Publish tightly packed RGBA8 pixels covering the branch extent.
    ///
    /// # Panics
    ///
    /// Panics when `pixels` does not match the branch extent.
    pub fn publish_rgba(&mut self, pixels: &[u8]) -> Arc<ScreenBranchPublication> {
        let extent = self.descriptor.geometry().output_extent();
        let payload = ScreenSurfacePayload::try_new(
            extent,
            CapturePixelFormat::Rgba8,
            ScreenPublicationColorimetry::new(self.descriptor.physical().color_pipeline().output()),
            pixels,
        )
        .expect("synthetic surface payload matches its branch extent");
        self.publish(ScreenBranchPayload::Surface(payload))
    }

    /// Publish one RGB color per zone cell in row-major order.
    ///
    /// # Panics
    ///
    /// Panics when `colors` does not match the branch grid.
    pub fn publish_zones(&mut self, colors: &[[u8; 3]]) -> Arc<ScreenBranchPublication> {
        let ScreenPublicationKind::Zones { columns, rows } = self.descriptor.kind() else {
            panic!("synthetic zones publisher owns a zones branch");
        };
        let payload = ScreenZonesPayload::try_new(
            columns,
            rows,
            ScreenPublicationColorimetry::new(self.descriptor.physical().color_pipeline().output()),
            colors,
        )
        .expect("synthetic zones payload matches its branch grid");
        self.publish(ScreenBranchPayload::Zones(payload))
    }

    fn publish(&mut self, payload: ScreenBranchPayload<'_>) -> Arc<ScreenBranchPublication> {
        self.native_sequence += 1;
        let now = Instant::now();
        let metadata = ScreenPublicationMetadata::try_new(
            self.descriptor.source_epoch().clone(),
            self.binding.plan_generation(),
            NonZeroU64::new(self.native_sequence).expect("synthetic sequence starts at one"),
            now,
            now,
            now + Duration::from_secs(1),
            ScreenPublicationHealth::Healthy,
        )
        .expect("synthetic publication timeline is valid");
        let publisher = self
            .hub
            .publisher(&self.descriptor, &self.binding)
            .expect("synthetic branch stays committed");
        let receipt = self
            .hub
            .publish(&publisher, payload, &metadata)
            .expect("synthetic payload matches its descriptor");
        Arc::clone(receipt.publication())
    }

    fn commit(extent: PixelExtent, kind: ScreenPublicationKind) -> Self {
        let source = synthetic_source(extent);
        let demand = resolve_demand(&source, kind);
        let descriptor = demand.descriptor().clone();
        let mut builder = ScreenPlanBuilder::new();
        let hub = builder.publication_hub();
        let graph_generation = ScreenInputGraphGeneration::new(1);
        let demand_revision = builder
            .current()
            .demand_revision()
            .next()
            .expect("synthetic demand revision is representable");
        let mut preparing = builder
            .prepare(
                [demand],
                demand_revision,
                graph_generation,
                ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
            )
            .expect("synthetic plan prepares");
        let mut worker_lifetimes = Vec::new();
        for required_source in preparing.required_sources().to_vec() {
            let ticket = preparing
                .worker_ticket(&required_source)
                .expect("synthetic source owns a worker ticket");
            let (ledger, lifetimes) = exact_ledger(&ticket);
            let token = ticket
                .acknowledge(ledger, &lifetimes)
                .expect("synthetic worker resources satisfy the ticket");
            preparing
                .acknowledge(token)
                .expect("synthetic worker token belongs to the candidate");
            worker_lifetimes.extend(lifetimes);
        }
        let armed = preparing
            .arm(
                builder.current().generation(),
                demand_revision,
                graph_generation,
            )
            .unwrap_or_else(|failure| panic!("synthetic plan arms: {}", failure.error()));
        let committed = builder
            .commit(armed, demand_revision, graph_generation)
            .unwrap_or_else(|failure| panic!("synthetic plan commits: {}", failure.error()));
        reclaim(committed);
        let binding = builder
            .committed_state()
            .worker_bindings()
            .iter()
            .find(|binding| binding.source_id() == &source_id())
            .cloned()
            .expect("synthetic source has a worker binding");
        Self {
            _builder: builder,
            hub,
            descriptor,
            binding,
            _worker_lifetimes: worker_lifetimes,
            native_sequence: 0,
        }
    }
}

fn source_id() -> CaptureSourceId {
    CaptureSourceId::new("synthetic:consumer-fixture").expect("synthetic source id is non-empty")
}

fn synthetic_source(extent: PixelExtent) -> ResolvedScreenSource {
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("synthetic geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: source_id(),
            topology_generation: 1,
            session_generation: 1,
        },
        ResolvedScreenSourceConfig::new_with_cursor_capabilities(
            geometry,
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenCursorCapabilities::clean_with_separate_cursor(),
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                1,
                1,
            ),
        ),
    )
}

fn resolve_demand(
    source: &ResolvedScreenSource,
    kind: ScreenPublicationKind,
) -> ResolvedScreenBranchDemand {
    let registered = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            kind,
            ScreenPublicationExecutorRequest::Cpu,
            ScreenExtentRequest::Native,
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        NonZeroU32::new(60).expect("synthetic cadence is non-zero"),
    );
    let capabilities = ScreenColorTransformCapabilities::new(
        true,
        true,
        true,
        registered
            .request()
            .processing_profile()
            .algorithm_revision(),
    );
    registered
        .resolve_with_executor_capabilities(
            source,
            ScreenExecutorColorCapabilities::new(capabilities, capabilities),
        )
        .expect("synthetic CPU demand resolves")
}

fn exact_ledger(
    ticket: &ScreenWorkerPreparationTicket,
) -> (ScreenExactResourceLedger, Vec<ScreenResourceLifetime>) {
    let resources = ticket
        .required_minimums()
        .iter()
        .map(|minimum| {
            ScreenExactResource::try_new(
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .expect("synthetic exact resources are representable");
    let lifetimes = resources
        .iter()
        .map(|resource| ticket.bind_resource_lifetime(resource))
        .collect::<Result<Vec<_>, _>>()
        .expect("synthetic resource lifetimes bind");
    let ledger =
        ScreenExactResourceLedger::try_new(resources).expect("synthetic exact ledger is valid");
    (ledger, lifetimes)
}

fn reclaim(committed: CommittedScreenPlan) {
    let (_, retirement) = committed.into_parts();
    retirement
        .try_reclaim()
        .expect("unobserved retired pools reclaim immediately");
}
