use std::sync::Arc;

use tracing::{info, warn};

use hypercolor_core::input::screen::ScreenAnalysisResourcePlan;
use hypercolor_core::input::{
    ScreenReconfigurationConflict, ScreenSource, ScreenSourceSwapCommitError, SourceKind,
    SourceState,
};
use hypercolor_types::config::{CaptureConfig, HypercolorConfig};

use crate::app_state::AppState;
use crate::scene_transactions::SceneTransaction;

#[derive(Debug, thiserror::Error)]
pub(in crate::api::config) enum CaptureConfigTransactionError {
    #[error("capture config identity changed during preparation")]
    Conflict,
    #[error(transparent)]
    Prepare(anyhow::Error),
    #[error(transparent)]
    Persist(anyhow::Error),
    #[error(transparent)]
    Commit(ScreenReconfigurationConflict),
}

pub(in crate::api::config) async fn apply_capture_config_transaction(
    state: &Arc<AppState>,
    expected_config: &Arc<HypercolorConfig>,
    capture: CaptureConfig,
) -> Result<(), CaptureConfigTransactionError> {
    let Some(manager) = state.config_manager.as_ref() else {
        return Err(CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
            "config manager unavailable"
        )));
    };
    let staged = manager
        .stage_capture_config(expected_config, capture.clone())
        .map_err(CaptureConfigTransactionError::Persist)?
        .ok_or(CaptureConfigTransactionError::Conflict)?;
    let (plan, capacity_plan, admission_coordinator) = {
        let input_manager = state.input_manager();
        let installed_capacity = input_manager.screen_resource_capacity();
        let capacity_plan = crate::startup::services::screen_capacity_plan_for_backend(
            &capture,
            installed_capacity.backend_capacity(),
        )
        .map_err(CaptureConfigTransactionError::Prepare)?;
        let analysis_peak_bytes = if capture.enabled {
            let analysis_capacity = capacity_plan
                .total_capacity()
                .byte_budget()
                .min(capacity_plan.total_capacity().backend_capacity());
            ScreenAnalysisResourcePlan::try_new(
                capture.grid_cols,
                capture.grid_rows,
                capture.capture_fps,
                analysis_capacity,
            )
            .map_err(|error| CaptureConfigTransactionError::Prepare(anyhow::anyhow!(error)))?
            .peak_bytes()
        } else {
            0
        };
        let capacity_preparation = input_manager
            .prepare_screen_capacity_plan(capacity_plan.total_capacity(), analysis_peak_bytes)
            .map_err(|error| CaptureConfigTransactionError::Prepare(anyhow::anyhow!(error)))?;
        if capture.enabled && capacity_preparation.is_none() {
            return Err(CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
                "screen capacity admission is not installed"
            )));
        }
        let plan = input_manager
            .plan_screen_source_swap(capture.enabled, capacity_preparation)
            .map_err(|error| {
                CaptureConfigTransactionError::Commit(ScreenReconfigurationConflict::Source(error))
            })?;
        (
            plan,
            capacity_plan,
            input_manager.screen_admission_coordinator(),
        )
    };
    let (mut replacement, persistence) = if plan.enabled() {
        let (mut source, persistence) =
            crate::startup::services::prepare_platform_screen_capture_source(
                &capture,
                Arc::clone(manager),
                expected_config,
                admission_coordinator,
                capacity_plan.total_capacity(),
            )
            .map_err(CaptureConfigTransactionError::Prepare)?;
        source.set_source_graph_generation(plan.replacement_source_graph_generation());
        source
            .set_screen_capture_demand(plan.capture_demand())
            .map_err(CaptureConfigTransactionError::Prepare)?;
        let source = Some(
            tokio::task::spawn_blocking(move || {
                source.start()?;
                Ok::<_, anyhow::Error>(source)
            })
            .await
            .map_err(|error| {
                CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
                    "capture preparation task failed: {error}"
                ))
            })?
            .map_err(CaptureConfigTransactionError::Prepare)?,
        );
        (source, Some(persistence))
    } else {
        (None, None)
    };
    if plan.capture_demand().is_active()
        && let Some(status) = replacement
            .as_ref()
            .and_then(|source| source.source_status_handle())
        && let Err(error) = validate_prepared_capture_status(status).await
    {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Prepare(error));
    }

    let mut prepared = match plan.prepare(&mut replacement) {
        Ok(prepared) => prepared,
        Err(error) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            stop_prepared_capture_source(replacement).await;
            return Err(CaptureConfigTransactionError::Commit(
                ScreenReconfigurationConflict::Source(error),
            ));
        }
    };
    let persistence_authority = persistence
        .as_ref()
        .map(|gate| (gate.epoch(), gate.source_identity()));
    let input_manager = state.input_manager();
    let retirement = match input_manager.commit_screen_source_swap(&mut prepared, |commit| {
        manager
            .commit_staged_capture_if_current(
                expected_config,
                persistence_authority,
                staged,
                |install_live| commit.commit(install_live),
            )
            .map_err(CaptureConfigTransactionError::Persist)?
            .map(|(_, retirement)| retirement)
            .ok_or(CaptureConfigTransactionError::Conflict)
    }) {
        Ok(retirement) => retirement,
        Err(ScreenSourceSwapCommitError::Conflict(error)) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            prepared.discard();
            return Err(CaptureConfigTransactionError::Commit(error));
        }
        Err(ScreenSourceSwapCommitError::Persistence(error)) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            prepared.discard();
            return Err(error);
        }
    };
    prepared.discard();

    if let Err(error) = tokio::task::spawn_blocking(move || retirement.retire()).await {
        warn!(%error, "Detached capture source retirement task failed");
    }
    if let Some(persistence) = persistence
        && let Err(error) = tokio::task::spawn_blocking(move || persistence.commit()).await
    {
        warn!(%error, "Capture identity persistence task failed");
    }
    if let Err(error) = state
        .scene_transactions
        .push(SceneTransaction::SetScreenCaptureConfigured(
            capture.enabled,
        ))
    {
        warn!(%error, "Render pipeline stopped before capture state publication");
    }
    info!(
        enabled = capture.enabled,
        "Applied live screen capture config"
    );
    Ok(())
}

/// How long a prepared replacement source may take to become usable.
///
/// Windows rebuilds in-process and settles in tens of milliseconds. A
/// Wayland rebuild is a D-Bus portal round trip plus PipeWire negotiation:
/// a restore-token reconnect settles in one to three seconds, so a 500ms
/// gate rejected every live capture reconfiguration on Linux while the
/// last-good publication stayed correctly retained. The gate still exists
/// and still fails typed when consent is required but never granted.
#[cfg(target_os = "linux")]
const PREPARED_CAPTURE_USABILITY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(6);
#[cfg(not(target_os = "linux"))]
const PREPARED_CAPTURE_USABILITY_DEADLINE: std::time::Duration =
    std::time::Duration::from_millis(500);

pub(in crate::api::config) async fn validate_prepared_capture_status(
    status: hypercolor_core::input::SourceStatusHandle,
) -> anyhow::Result<()> {
    let mut subscription = status.subscribe();
    let deadline = tokio::time::Instant::now() + PREPARED_CAPTURE_USABILITY_DEADLINE;
    loop {
        let snapshot = subscription.snapshot();
        match snapshot.state {
            SourceState::Live => return Ok(()),
            SourceState::Degraded if snapshot.resource_count > 0 => return Ok(()),
            SourceState::Starting => {}
            _ => {
                anyhow::bail!(
                    "{}",
                    snapshot.issue.as_ref().map_or_else(
                        || format!("capture source is not usable ({:?})", snapshot.state),
                        |issue| issue.message.to_string()
                    )
                );
            }
        }
        match tokio::time::timeout_at(deadline, subscription.changed()).await {
            Ok(Some(_)) => {}
            Ok(None) => anyhow::bail!("capture source status closed before becoming usable"),
            Err(_) => anyhow::bail!(
                "capture source did not become usable within {:?}",
                PREPARED_CAPTURE_USABILITY_DEADLINE
            ),
        }
    }
}

pub(in crate::api::config) async fn capture_runtime_matches(
    state: &Arc<AppState>,
    expected_config: &Arc<HypercolorConfig>,
) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };
    let input_manager = state.input_manager();
    if !manager.is_current(expected_config)
        || !manager.capture_runtime_matches(&expected_config.capture)
    {
        return false;
    }
    let registry = input_manager.source_status_registry();
    let statuses = registry.snapshot().statuses();
    capture_statuses_match(&expected_config.capture, &statuses)
}

pub(in crate::api::config) fn capture_statuses_match(
    capture: &CaptureConfig,
    statuses: &[Arc<hypercolor_core::input::SourceStatus>],
) -> bool {
    let mut screen = statuses
        .iter()
        .filter(|status| status.kind == SourceKind::Screen && !status.retired);
    let first = screen.next();
    if !capture.enabled {
        return first.is_none();
    }
    let Some(status) = first else {
        return false;
    };
    if screen.next().is_some() || !status.configured || !status.consented {
        return false;
    }
    matches!(status.state, SourceState::Live)
        || matches!(status.state, SourceState::Degraded) && status.resource_count > 0
        || matches!(status.state, SourceState::Stopped) && !status.demanded
}

async fn stop_prepared_capture_source(source: Option<Box<dyn ScreenSource>>) {
    let Some(mut source) = source else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || source.stop()).await;
}
