use super::{
    Arc, CaptureCadence, Instant, MacosExactPublicationShared, MacosExactRuntime, MacosNativeRoute,
    MacosNativeTargetManifest, MacosOwnedSource, MacosPublicationSource, PendingMacosNativeRoute,
    ScreenNativePreparationPayload, ScreenPreparedWorkerToken, ScreenPublicationExecutor,
    ScreenResourceKind, ScreenResourceLifetime, ScreenWorkerExactLedgerBuilder,
    ScreenWorkerPreparationTicket, anyhow,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::{
    CpuExactReductionWorkPlan, PreparedCpuPublicationFanout, ScreenRequiredResourceMinimum,
    ScreenSourceSelector,
};

pub(super) fn checked_macos_metadata_bytes<T>(count: usize, resource: &str) -> anyhow::Result<u64> {
    u64::try_from(count)
        .ok()
        .and_then(|count| {
            u64::try_from(std::mem::size_of::<T>())
                .ok()
                .and_then(|size| count.checked_mul(size))
        })
        .ok_or_else(|| anyhow!("macOS exact {resource} metadata accounting overflow"))
}

pub(super) fn preflight_macos_scope_bytes(
    ledger: &mut ScreenWorkerExactLedgerBuilder,
    minimum_remaining: &mut u64,
    bytes: u64,
) -> anyhow::Result<()> {
    let modeled = bytes.min(*minimum_remaining);
    *minimum_remaining -= modeled;
    let additional = bytes - modeled;
    if additional > 0 {
        ledger.preflight_additional_bytes(additional)?;
    }
    Ok(())
}

#[cfg(feature = "macos-capture-fixtures")]
pub(super) fn prepare_macos_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    source: Option<&MacosPublicationSource>,
    exact: &MacosExactPublicationShared,
) -> anyhow::Result<(
    ScreenPreparedWorkerToken,
    Option<(MacosExactRuntime, MacosOwnedSource)>,
)> {
    let candidate = ticket.candidate_plan().clone();
    let source_branches = candidate
        .branches()
        .iter()
        .filter(|branch| branch.descriptor().source_epoch().source_id == *ticket.source_id())
        .collect::<Vec<_>>();
    if source_branches.is_empty() {
        let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
        let reports = ledger
            .ticket()
            .required_minimums()
            .iter()
            .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
            .collect::<Vec<_>>();
        for (name, bytes) in reports {
            ledger.report(&name, bytes)?;
        }
        let (token, _) = ledger.finish()?.into_parts();
        return Ok((token, None));
    }

    let source = source
        .filter(|source| &source.epoch.source_id == ticket.source_id())
        .ok_or_else(|| anyhow!("macOS exact publication source changed before preparation"))?;
    let executor = exact.cpu_executor()?;
    let compute_plan =
        CpuExactReductionWorkPlan::try_for_source(&candidate, ticket.source_id(), |_| true)?;
    let compute_plan = match exact.compute_capacity_policy.exact(executor.worker_count()) {
        Some(capacity) => compute_plan.admit(capacity)?,
        None => compute_plan,
    };
    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
    let mut processing_minimum_remaining = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::ProcessingProfileState)
        .map_or(0, ScreenRequiredResourceMinimum::minimum_bytes);
    let mut worker_minimum_remaining = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::WorkerAdditional)
        .map_or(0, ScreenRequiredResourceMinimum::minimum_bytes);
    let plane_minimum_bytes = ledger
        .ticket()
        .required_minimums()
        .iter()
        .filter(|minimum| minimum.resource() == ScreenResourceKind::PhysicalPlane)
        .try_fold(0_u64, |total, minimum| {
            total
                .checked_add(minimum.minimum_bytes())
                .ok_or_else(|| anyhow!("macOS exact physical-plane accounting overflow"))
        })?;
    let runtime_metadata_bytes = checked_macos_metadata_bytes::<MacosExactRuntime>(1, "runtime")?
        .checked_add(checked_macos_metadata_bytes::<MacosOwnedSource>(
            1,
            "owned source",
        )?)
        .and_then(|bytes| {
            bytes.checked_add(
                checked_macos_metadata_bytes::<MacosNativeRoute>(
                    source_branches.len(),
                    "native routes",
                )
                .ok()?,
            )
        })
        .ok_or_else(|| anyhow!("macOS exact runtime metadata accounting overflow"))?;
    preflight_macos_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        runtime_metadata_bytes,
    )?;

    let (fanout_candidate, fanout_bytes, workspace_bytes) = if compute_plan.cpu_reduction_count()
        == 0
    {
        (None, 0, 0)
    } else {
        let cpu_source =
            source.cpu_source(ScreenSourceSelector::Exact(source.epoch.source_id.clone()));
        let batch_quote = executor.batch_allocation_quote(&cpu_source, &candidate)?;
        preflight_macos_scope_bytes(&mut ledger, &mut processing_minimum_remaining, batch_quote)?;
        let batch = executor.prepare_batch(&cpu_source, &candidate)?;
        let workspace_quote = batch.materialization_workspace_allocation_quote(&candidate)?;
        let workspace_additional_bytes = workspace_quote
            .checked_sub(plane_minimum_bytes)
            .ok_or_else(|| anyhow!("macOS workspace quote understates physical-plane minima"))?;
        preflight_macos_scope_bytes(
            &mut ledger,
            &mut worker_minimum_remaining,
            workspace_additional_bytes,
        )?;
        let workspace = batch.prepare_materialization_workspace(&candidate)?;
        let workspace_bytes = workspace.allocation_byte_len();
        let fanout_quote = PreparedCpuPublicationFanout::candidate_allocation_quote(
            &batch, &workspace, &candidate,
        )?;
        let fanout_additional_bytes = fanout_quote
            .checked_sub(batch_quote)
            .ok_or_else(|| anyhow!("macOS fanout quote understates retained batch backing"))?;
        preflight_macos_scope_bytes(
            &mut ledger,
            &mut processing_minimum_remaining,
            fanout_additional_bytes,
        )?;
        let candidate = PreparedCpuPublicationFanout::prepare_executable_candidate(
            &executor, &batch, workspace, &candidate,
        )?;
        let bytes = candidate.allocation_byte_len();
        (Some(candidate), bytes, workspace_bytes)
    };

    let mut pending_native = Vec::new();
    pending_native.try_reserve_exact(source_branches.len())?;
    for (index, branch) in source_branches.iter().enumerate() {
        let ScreenPublicationExecutor::SourceNative(target) = branch.descriptor().executor() else {
            continue;
        };
        let manifest = Arc::new(MacosNativeTargetManifest::new(branch.descriptor())?);
        let platform = ScreenNativePreparationPayload::new(
            branch.descriptor(),
            ledger.ticket().plan_generation(),
            manifest,
        );
        let resource_name: Arc<str> = Arc::from(format!("macos-native-target-{index}"));
        let capture_resource_name: Arc<str> = Arc::from(format!("macos-native-capture-{index}"));
        let prepared = ledger.prepare_native_target(
            target,
            branch.descriptor(),
            &platform,
            Arc::clone(&resource_name),
            "worker-runtime-total",
        )?;
        ledger.preflight_additional_bytes(source.allocation_bytes)?;
        ledger.report_scoped(
            &capture_resource_name,
            "worker-runtime-total",
            source.allocation_bytes,
        )?;
        pending_native.push(PendingMacosNativeRoute {
            resource_name,
            capture_resource_name,
            descriptor: branch.descriptor().clone(),
            target: prepared,
            requested_hz: branch.requested_hz(),
        });
    }

    let processing_scope = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::ProcessingProfileState)
        .map(|minimum| Arc::clone(minimum.name()));
    if fanout_bytes > 0 && processing_scope.is_none() {
        ledger.report_scoped("macos-cpu-fanout", "worker-runtime-total", fanout_bytes)?;
    }
    let expected_lifetime_count = ledger.prospective_resource_count()?;
    let lifetime_metadata_bytes = checked_macos_metadata_bytes::<ScreenResourceLifetime>(
        expected_lifetime_count,
        "runtime lifetimes",
    )?;
    preflight_macos_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        lifetime_metadata_bytes,
    )?;
    let worker_metadata_bytes = workspace_bytes
        .saturating_sub(plane_minimum_bytes)
        .checked_add(runtime_metadata_bytes)
        .and_then(|bytes| bytes.checked_add(lifetime_metadata_bytes))
        .ok_or_else(|| anyhow!("macOS exact worker accounting overflow"))?;
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| {
            (
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Vec<_>>();
    for (name, resource, minimum) in &reports {
        let actual = match resource {
            ScreenResourceKind::ProcessingProfileState
                if processing_scope.as_ref() == Some(name) =>
            {
                fanout_bytes.max(*minimum)
            }
            ScreenResourceKind::WorkerAdditional => worker_metadata_bytes.max(*minimum),
            _ => *minimum,
        };
        ledger.report(name, actual)?;
    }
    let exact_ledger = ledger.finish()?;
    if exact_ledger.lifetimes().len() != expected_lifetime_count {
        return Err(anyhow!(
            "macOS exact lifetime metadata changed during preparation"
        ));
    }
    let binding = exact_ledger.token().binding().clone();
    let (token, lifetimes) = exact_ledger.into_parts();
    let mut native_routes = Vec::new();
    native_routes.try_reserve_exact(pending_native.len())?;
    for pending in pending_native {
        let shared_resource_name = pending.target.shared_resource_name().cloned();
        let lifetime = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name() == &pending.resource_name)
            .cloned()
            .ok_or_else(|| anyhow!("macOS native target lifetime is missing"))?;
        let capture_lifetime = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name() == &pending.capture_resource_name)
            .cloned()
            .ok_or_else(|| anyhow!("macOS native capture lifetime is missing"))?;
        let shared_lifetime = shared_resource_name
            .as_ref()
            .map(|resource_name| {
                lifetimes
                    .iter()
                    .find(|lifetime| lifetime.resource().name() == resource_name)
                    .cloned()
                    .ok_or_else(|| anyhow!("macOS native shared target lifetime is missing"))
            })
            .transpose()?;
        native_routes.push(MacosNativeRoute {
            descriptor: pending.descriptor,
            target: pending.target.bind_with_shared(lifetime, shared_lifetime)?,
            capture_lifetime,
            pacer: CaptureCadence::new(pending.requested_hz.get())?.pacer(),
            next_publish_at: Instant::now(),
            last_accepted_sequence: None,
            publisher: None,
        });
    }
    let runtime_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "worker-runtime-total")
        .cloned()
        .ok_or_else(|| anyhow!("macOS worker runtime lifetime is missing"))?;
    Ok((
        token,
        Some((
            MacosExactRuntime {
                source: source.clone(),
                binding: binding.clone(),
                _lifetimes: lifetimes,
                native_routes: native_routes.into_boxed_slice(),
                fanout_candidate,
                fanout: None,
            },
            MacosOwnedSource {
                source_id: source.epoch.source_id.clone(),
                binding,
                _runtime_lifetime: runtime_lifetime,
            },
        )),
    ))
}

#[cfg(not(feature = "macos-capture-fixtures"))]
pub(super) fn prepare_macos_exact_runtime(
    ticket: ScreenWorkerPreparationTicket,
    source: Option<&MacosPublicationSource>,
    _exact: &MacosExactPublicationShared,
) -> anyhow::Result<(
    ScreenPreparedWorkerToken,
    Option<(MacosExactRuntime, MacosOwnedSource)>,
)> {
    let candidate = ticket.candidate_plan().clone();
    let source_branches = candidate
        .branches()
        .iter()
        .filter(|branch| branch.descriptor().source_epoch().source_id == *ticket.source_id())
        .collect::<Vec<_>>();
    if source_branches.is_empty() {
        let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
        let reports = ledger
            .ticket()
            .required_minimums()
            .iter()
            .map(|minimum| (Arc::clone(minimum.name()), minimum.minimum_bytes()))
            .collect::<Vec<_>>();
        for (name, bytes) in reports {
            ledger.report(&name, bytes)?;
        }
        let (token, _) = ledger.finish()?.into_parts();
        return Ok((token, None));
    }

    let source = source
        .filter(|source| &source.epoch.source_id == ticket.source_id())
        .ok_or_else(|| anyhow!("macOS exact publication source changed before preparation"))?;
    if source_branches.iter().any(|branch| {
        !matches!(
            branch.descriptor().executor(),
            ScreenPublicationExecutor::SourceNative(_)
        )
    }) {
        return Err(anyhow!(
            "production macOS exact publication requires native execution"
        ));
    }

    let mut ledger = ScreenWorkerExactLedgerBuilder::new(ticket)?;
    let mut worker_minimum_remaining = ledger
        .ticket()
        .required_minimums()
        .iter()
        .find(|minimum| minimum.resource() == ScreenResourceKind::WorkerAdditional)
        .map_or(0, super::ScreenRequiredResourceMinimum::minimum_bytes);
    let runtime_metadata_bytes = checked_macos_metadata_bytes::<MacosExactRuntime>(1, "runtime")?
        .checked_add(checked_macos_metadata_bytes::<MacosOwnedSource>(
            1,
            "owned source",
        )?)
        .and_then(|bytes| {
            bytes.checked_add(
                checked_macos_metadata_bytes::<MacosNativeRoute>(
                    source_branches.len(),
                    "native routes",
                )
                .ok()?,
            )
        })
        .ok_or_else(|| anyhow!("macOS exact runtime metadata accounting overflow"))?;
    preflight_macos_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        runtime_metadata_bytes,
    )?;

    let mut pending_native = Vec::new();
    pending_native.try_reserve_exact(source_branches.len())?;
    for (index, branch) in source_branches.iter().enumerate() {
        let ScreenPublicationExecutor::SourceNative(target) = branch.descriptor().executor() else {
            unreachable!("production macOS source branches were validated as native")
        };
        let manifest = Arc::new(MacosNativeTargetManifest::new(branch.descriptor())?);
        let platform = ScreenNativePreparationPayload::new(
            branch.descriptor(),
            ledger.ticket().plan_generation(),
            manifest,
        );
        let resource_name: Arc<str> = Arc::from(format!("macos-native-target-{index}"));
        let capture_resource_name: Arc<str> = Arc::from(format!("macos-native-capture-{index}"));
        let prepared = ledger.prepare_native_target(
            target,
            branch.descriptor(),
            &platform,
            Arc::clone(&resource_name),
            "worker-runtime-total",
        )?;
        ledger.preflight_additional_bytes(source.allocation_bytes)?;
        ledger.report_scoped(
            &capture_resource_name,
            "worker-runtime-total",
            source.allocation_bytes,
        )?;
        pending_native.push(PendingMacosNativeRoute {
            resource_name,
            capture_resource_name,
            descriptor: branch.descriptor().clone(),
            target: prepared,
            requested_hz: branch.requested_hz(),
        });
    }

    let expected_lifetime_count = ledger.prospective_resource_count()?;
    let lifetime_metadata_bytes = checked_macos_metadata_bytes::<ScreenResourceLifetime>(
        expected_lifetime_count,
        "runtime lifetimes",
    )?;
    preflight_macos_scope_bytes(
        &mut ledger,
        &mut worker_minimum_remaining,
        lifetime_metadata_bytes,
    )?;
    let worker_metadata_bytes = runtime_metadata_bytes
        .checked_add(lifetime_metadata_bytes)
        .ok_or_else(|| anyhow!("macOS exact worker accounting overflow"))?;
    let reports = ledger
        .ticket()
        .required_minimums()
        .iter()
        .map(|minimum| {
            (
                Arc::clone(minimum.name()),
                minimum.resource(),
                minimum.minimum_bytes(),
            )
        })
        .collect::<Vec<_>>();
    for (name, resource, minimum) in &reports {
        let actual = if *resource == ScreenResourceKind::WorkerAdditional {
            worker_metadata_bytes.max(*minimum)
        } else {
            *minimum
        };
        ledger.report(name, actual)?;
    }
    let exact_ledger = ledger.finish()?;
    if exact_ledger.lifetimes().len() != expected_lifetime_count {
        return Err(anyhow!(
            "macOS exact lifetime metadata changed during preparation"
        ));
    }
    let binding = exact_ledger.token().binding().clone();
    let (token, lifetimes) = exact_ledger.into_parts();
    let mut native_routes = Vec::new();
    native_routes.try_reserve_exact(pending_native.len())?;
    for pending in pending_native {
        let shared_resource_name = pending.target.shared_resource_name().cloned();
        let lifetime = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name() == &pending.resource_name)
            .cloned()
            .ok_or_else(|| anyhow!("macOS native target lifetime is missing"))?;
        let capture_lifetime = lifetimes
            .iter()
            .find(|lifetime| lifetime.resource().name() == &pending.capture_resource_name)
            .cloned()
            .ok_or_else(|| anyhow!("macOS native capture lifetime is missing"))?;
        let shared_lifetime = shared_resource_name
            .as_ref()
            .map(|resource_name| {
                lifetimes
                    .iter()
                    .find(|lifetime| lifetime.resource().name() == resource_name)
                    .cloned()
                    .ok_or_else(|| anyhow!("macOS native shared target lifetime is missing"))
            })
            .transpose()?;
        native_routes.push(MacosNativeRoute {
            descriptor: pending.descriptor,
            target: pending.target.bind_with_shared(lifetime, shared_lifetime)?,
            capture_lifetime,
            pacer: CaptureCadence::new(pending.requested_hz.get())?.pacer(),
            next_publish_at: Instant::now(),
            last_accepted_sequence: None,
            publisher: None,
        });
    }
    let runtime_lifetime = lifetimes
        .iter()
        .find(|lifetime| lifetime.resource().name().as_ref() == "worker-runtime-total")
        .cloned()
        .ok_or_else(|| anyhow!("macOS worker runtime lifetime is missing"))?;
    Ok((
        token,
        Some((
            MacosExactRuntime {
                source: source.clone(),
                binding: binding.clone(),
                _lifetimes: lifetimes,
                native_routes: native_routes.into_boxed_slice(),
            },
            MacosOwnedSource {
                source_id: source.epoch.source_id.clone(),
                binding,
                _runtime_lifetime: runtime_lifetime,
            },
        )),
    ))
}
