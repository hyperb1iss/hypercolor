use std::num::NonZeroU32;
use std::sync::Arc;

use hypercolor_core::input::screen::{
    CaptureColorimetry, CaptureEpoch, CaptureGeometry, CapturePixelFormat, CaptureRotation,
    CaptureSourceId, InputPublicationDemandRevision, PhysicalOrigin, PixelExtent,
    RegisteredScreenBranchDemand, ResolvedScreenSource, ResolvedScreenSourceConfig,
    ScreenAdmissionCapacity, ScreenAspectPolicy, ScreenBackendResourceIdentity,
    ScreenCaptureBackend, ScreenExtentRequest, ScreenInputGraphGeneration, ScreenPlanBuilder,
    ScreenProcessingProfile, ScreenPublicationKind, ScreenPublicationRequest, ScreenResourceApi,
    ScreenSourceReflection, ScreenSourceSelector, ScreenUpscalePolicy,
    ScreenWorkerExactLedgerBuilder, ScreenWorkerLedgerBuildError, SourceScale,
};

fn non_zero(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn extent(width: u32, height: u32) -> PixelExtent {
    PixelExtent::new(width, height).expect("test extent is non-empty")
}

fn source() -> ResolvedScreenSource {
    let extent = extent(17, 11);
    let geometry = CaptureGeometry::new(
        PhysicalOrigin::default(),
        extent,
        extent,
        CaptureRotation::Identity,
        None,
        SourceScale::ONE,
    )
    .expect("test geometry is valid");
    ResolvedScreenSource::new(
        ScreenSourceSelector::Configured,
        CaptureEpoch {
            source_id: CaptureSourceId::new("synthetic:ledger")
                .expect("test source id is non-empty"),
            topology_generation: 3,
            session_generation: 5,
        },
        ResolvedScreenSourceConfig::new(
            geometry,
            extent,
            ScreenSourceReflection::None,
            CapturePixelFormat::Rgba8,
            CaptureColorimetry::SRGB,
            ScreenBackendResourceIdentity::new(
                ScreenCaptureBackend::Synthetic,
                ScreenResourceApi::Cpu,
                7,
                11,
            ),
        ),
    )
}

fn prepared() -> (
    ScreenPlanBuilder,
    hypercolor_core::input::screen::PreparingScreenPlan,
    hypercolor_core::input::screen::ScreenWorkerPreparationTicket,
) {
    let source = source();
    let demand = RegisteredScreenBranchDemand::new(
        ScreenPublicationRequest::new(
            ScreenSourceSelector::Configured,
            ScreenPublicationKind::Zones {
                columns: non_zero(13),
                rows: non_zero(7),
            },
            ScreenExtentRequest::bounded(
                Some(non_zero(17)),
                Some(non_zero(11)),
                ScreenUpscalePolicy::Never,
            ),
            ScreenAspectPolicy::Contain,
            Arc::new(ScreenProcessingProfile::default()),
        ),
        non_zero(60),
    )
    .resolve_with_color_capabilities(
        &source,
        hypercolor_core::input::screen::CpuReductionExecutor::new(
            std::num::NonZeroUsize::MIN,
            non_zero(3),
        )
        .expect("test CPU executor builds")
        .capabilities(),
    )
    .expect("test branch resolves");
    let mut builder = ScreenPlanBuilder::new();
    let mut preparing = builder
        .prepare(
            [demand],
            None,
            InputPublicationDemandRevision::new(1),
            ScreenInputGraphGeneration::new(1),
            ScreenAdmissionCapacity::new(u64::MAX, u64::MAX),
        )
        .expect("test plan prepares");
    let ticket = preparing
        .worker_ticket(&source.epoch().source_id)
        .expect("test source owns one worker ticket");
    (builder, preparing, ticket)
}

#[test]
fn ticket_scoped_builder_rejects_unknown_duplicate_missing_and_understated_reports() {
    let (_, _, ticket) = prepared();
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("ledger report metadata prepares");
    assert!(matches!(
        ledger.report("not-required", 0),
        Err(ScreenWorkerLedgerBuildError::UnknownResource { .. })
    ));
    let first = ledger.ticket().required_minimums()[0].clone();
    ledger
        .report(first.name(), first.minimum_bytes())
        .expect("first required scope reports");
    assert!(matches!(
        ledger.report(first.name(), 0),
        Err(ScreenWorkerLedgerBuildError::DuplicateResource { .. })
    ));
    assert!(matches!(
        ledger.finish(),
        Err(ScreenWorkerLedgerBuildError::MissingResource { .. })
    ));

    let (_, _, ticket) = prepared();
    let positive_minimum = ticket
        .required_minimums()
        .iter()
        .find(|minimum| minimum.minimum_bytes() > 0)
        .expect("materialized Zones require retained physical planes")
        .clone();
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("ledger report metadata prepares");
    assert!(matches!(
        ledger.report(
            positive_minimum.name(),
            positive_minimum.minimum_bytes() - 1
        ),
        Err(ScreenWorkerLedgerBuildError::UnderstatedResource { .. })
    ));

    let (_, _, ticket) = prepared();
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("ledger report metadata prepares");
    assert!(matches!(
        ledger.report_scoped("decoder-scratch", "not-required", 64),
        Err(ScreenWorkerLedgerBuildError::UnknownResourceScope { .. })
    ));
    let first = ledger.ticket().required_minimums()[0].clone();
    assert!(matches!(
        ledger.report_scoped(first.name(), first.name(), 64),
        Err(ScreenWorkerLedgerBuildError::DuplicateResource { .. })
    ));
}

#[test]
fn ticket_scoped_builder_binds_every_actual_resource_and_commits() {
    let (mut builder, mut preparing, ticket) = prepared();
    let required_count = ticket.required_minimums().len();
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("ledger report metadata prepares");
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, actual_bytes) in reports {
        ledger
            .report(&name, actual_bytes)
            .expect("required scope reports exact retained bytes");
    }
    let acknowledged = ledger.finish().expect("exact ledger acknowledges");
    assert_eq!(acknowledged.lifetimes().len(), required_count);
    assert_eq!(
        acknowledged.token().exact_ledger().resources().len(),
        required_count
    );
    let (token, lifetimes) = acknowledged.into_parts();
    preparing
        .acknowledge(token)
        .expect("prepared token belongs to the candidate");
    let revision = InputPublicationDemandRevision::new(1);
    let graph = ScreenInputGraphGeneration::new(1);
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .unwrap_or_else(|failure| panic!("test plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, revision, graph)
        .unwrap_or_else(|failure| panic!("test plan commits: {}", failure.error()));
    assert_eq!(lifetimes.len(), required_count);
    assert_eq!(committed.plan().branches().len(), 1);
}

#[test]
fn ticket_scoped_builder_infers_additional_resource_kind_from_required_scope() {
    let (mut builder, mut preparing, ticket) = prepared();
    let runtime_scope = ticket
        .required_minimums()
        .iter()
        .find(|minimum| minimum.name().as_ref() == "worker-runtime-total")
        .expect("active source requires one runtime accounting scope")
        .clone();
    let required_count = ticket.required_minimums().len();
    let expected_kind = runtime_scope.resource();
    let mut ledger =
        ScreenWorkerExactLedgerBuilder::new(ticket).expect("ledger report metadata prepares");
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
        .collect::<Vec<_>>();
    for (name, actual_bytes) in reports {
        ledger
            .report(&name, actual_bytes)
            .expect("required scope reports exact retained bytes");
    }
    ledger
        .report_scoped("decoder-scratch", runtime_scope.name(), 2_048)
        .expect("additional allocation inherits its admitted scope");
    assert!(matches!(
        ledger.report_scoped("decoder-scratch", runtime_scope.name(), 4_096),
        Err(ScreenWorkerLedgerBuildError::DuplicateResource { .. })
    ));
    let acknowledged = ledger.finish().expect("scoped allocation acknowledges");
    let scratch = acknowledged
        .token()
        .exact_ledger()
        .resources()
        .iter()
        .find(|resource| resource.name().as_ref() == "decoder-scratch")
        .expect("scoped allocation is retained in the exact ledger");
    assert_eq!(scratch.accounting_scope(), runtime_scope.name());
    assert_eq!(scratch.resource(), expected_kind);
    assert_eq!(scratch.bytes(), 2_048);
    let (token, lifetimes) = acknowledged.into_parts();
    preparing
        .acknowledge(token)
        .expect("scoped allocation token belongs to the candidate");
    let revision = InputPublicationDemandRevision::new(1);
    let graph = ScreenInputGraphGeneration::new(1);
    let armed = preparing
        .arm(builder.current().generation(), revision, graph)
        .unwrap_or_else(|failure| panic!("test plan arms: {}", failure.error()));
    let committed = builder
        .commit(armed, revision, graph)
        .unwrap_or_else(|failure| panic!("test plan commits: {}", failure.error()));
    assert_eq!(lifetimes.len(), required_count + 1);
    assert_eq!(committed.plan().branches().len(), 1);
}
