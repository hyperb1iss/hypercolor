//! Spatial layout catalog, mutation, activation, and durability authority.

mod workflows;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "persistence-test-hooks")]
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use hypercolor_types::api::layouts::LayoutSummary;
use hypercolor_types::canvas::SurfaceDescriptor;
use hypercolor_types::scene::SceneId;
use hypercolor_types::spatial::{Output, SamplingMode, SpatialLayout};
use tokio::sync::RwLock;
#[cfg(feature = "persistence-test-hooks")]
use tokio::sync::{Notify, Semaphore};

use crate::domain::context::{DeviceContext, RuntimeSessionService};
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::domain::{DomainError, ResourceKind};
use crate::layout_auto_exclusions;
use crate::persistence::{AtomicFileWriter, AtomicWriteOutcome};
use crate::runtime_state::RuntimeSessionError;
use crate::scene_transactions::{
    LayoutPersistenceOutcome, LayoutPersistencePhase, LayoutTransactionRejection,
    LayoutUpdateError, LayoutUpdateGuard, PreparedLayoutUpdate, SceneActivationGuard,
    SceneTransactionQueue, apply_prepared_layout_update_under_guard_with_persistence,
};

const LAYOUT_DURABILITY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutPersistenceStatus {
    Synchronized,
    Pending,
}

pub(crate) struct LayoutMutationResult<T> {
    pub data: T,
    pub persistence: LayoutPersistenceStatus,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutMutationTestPoint {
    BeforeGuard,
    AfterMemoryMutation,
    AfterRendererMutation,
    AfterWorkflow,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutMutationTestOperation {
    Create,
    Update,
    Apply,
    Preview,
    Delete,
    ConfigResize,
    SimulatorPrune,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug)]
pub struct LayoutMutationTestBarrier {
    entered: Notify,
    release: Semaphore,
}

#[cfg(feature = "persistence-test-hooks")]
impl LayoutMutationTestBarrier {
    pub async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    pub fn release(&self) {
        self.release.add_permits(1);
    }
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, Default)]
pub struct LayoutMutationTestHooks {
    barriers: Arc<StdMutex<HashMap<LayoutMutationTestKey, Arc<LayoutMutationTestBarrier>>>>,
}

#[cfg(feature = "persistence-test-hooks")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LayoutMutationTestKey {
    point: LayoutMutationTestPoint,
    operation: LayoutMutationTestOperation,
    reference: String,
}

#[cfg(feature = "persistence-test-hooks")]
impl LayoutMutationTestHooks {
    pub fn install(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: impl Into<String>,
    ) -> Arc<LayoutMutationTestBarrier> {
        let barrier = Arc::new(LayoutMutationTestBarrier {
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        self.barriers
            .lock()
            .expect("layout mutation test hooks should lock")
            .insert(
                LayoutMutationTestKey {
                    point,
                    operation,
                    reference: reference.into(),
                },
                Arc::clone(&barrier),
            );
        barrier
    }

    async fn wait(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: &str,
    ) {
        let barrier = self
            .barriers
            .lock()
            .expect("layout mutation test hooks should lock")
            .remove(&LayoutMutationTestKey {
                point,
                operation,
                reference: reference.to_owned(),
            });
        if let Some(barrier) = barrier {
            barrier.entered.notify_one();
            let _permit = barrier
                .release
                .acquire()
                .await
                .expect("layout mutation test barrier should remain open");
        }
    }
}

pub(crate) struct LayoutContextResources {
    pub layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    pub layouts_path: PathBuf,
    pub layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    pub layout_auto_exclusions_path: PathBuf,
}

#[derive(Clone)]
pub struct LayoutContext {
    layouts: Arc<RwLock<HashMap<String, SpatialLayout>>>,
    layouts_path: PathBuf,
    layout_auto_exclusions: Arc<RwLock<layout_auto_exclusions::LayoutAutoExclusionStore>>,
    layout_auto_exclusions_path: PathBuf,
    spatial: SpatialService,
    scenes: SceneService,
    transactions: SceneTransactionQueue,
    runtime_state_path: PathBuf,
    runtime_session: RuntimeSessionService,
    devices: DeviceContext,
    #[cfg(feature = "persistence-test-hooks")]
    test_hooks: LayoutMutationTestHooks,
}

impl LayoutContext {
    pub(crate) fn new(
        resources: LayoutContextResources,
        spatial: SpatialService,
        scenes: SceneService,
        transactions: SceneTransactionQueue,
        runtime_state_path: PathBuf,
        runtime_session: RuntimeSessionService,
        devices: DeviceContext,
    ) -> Self {
        Self {
            layouts: resources.layouts,
            layouts_path: resources.layouts_path,
            layout_auto_exclusions: resources.layout_auto_exclusions,
            layout_auto_exclusions_path: resources.layout_auto_exclusions_path,
            spatial,
            scenes,
            transactions,
            runtime_state_path,
            runtime_session,
            devices,
            #[cfg(feature = "persistence-test-hooks")]
            test_hooks: LayoutMutationTestHooks::default(),
        }
    }

    pub(crate) async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        self.transactions.acquire_scene_activation_guard().await
    }

    pub(crate) async fn acquire_update_guard(&self) -> LayoutUpdateGuard {
        self.transactions.acquire_layout_update_guard().await
    }

    pub(crate) async fn admit_persisted_update_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        let prepared = PreparedLayoutUpdate::try_new(layout)?;
        let persistence = LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_session: self.runtime_session.clone(),
        };
        apply_prepared_layout_update_under_guard_with_persistence(
            self.spatial.clone(),
            self.scenes.clone(),
            self.transactions.clone(),
            guard,
            prepared,
            move |phase| {
                let persistence = persistence.clone();
                async move { persist_layout_runtime_phase(&persistence, phase).await }
            },
        )
        .await
    }

    pub(crate) async fn apply_persisted_update(
        &self,
        guard: LayoutUpdateGuard,
        layout: SpatialLayout,
    ) -> Result<(), LayoutUpdateError> {
        self.admit_persisted_update_under_guard(&guard, layout)
            .await?;
        drop(guard);
        let _ = self.converge_persisted_update().await;
        Ok(())
    }

    pub(crate) async fn converge_persisted_update(&self) -> LayoutPersistenceStatus {
        self.devices.sync_connectivity().await;
        let persistence = LayoutPersistenceContext {
            runtime_state_path: self.runtime_state_path.clone(),
            runtime_session: self.runtime_session.clone(),
        };
        match persist_layout_runtime_phase(&persistence, LayoutPersistencePhase::Converge).await {
            LayoutPersistenceOutcome::Written => LayoutPersistenceStatus::Synchronized,
            LayoutPersistenceOutcome::Superseded => {
                tracing::warn!("layout committed but convergence persistence was superseded");
                LayoutPersistenceStatus::Pending
            }
            LayoutPersistenceOutcome::BeforeAdmission(error) => {
                tracing::warn!(%error, "layout committed before convergence persistence was admitted");
                LayoutPersistenceStatus::Pending
            }
            LayoutPersistenceOutcome::RetryArmed(error) => {
                tracing::warn!(%error, "layout committed with convergence persistence retry armed");
                LayoutPersistenceStatus::Pending
            }
        }
    }

    async fn persist_catalog(&self) -> anyhow::Result<()> {
        let layouts = self.layouts.read().await;
        crate::layout_store::save(&self.layouts_path, &layouts)
    }

    pub(crate) async fn persist_catalog_best_effort(&self) {
        if let Err(error) = self.persist_catalog().await {
            tracing::warn!(
                path = %self.layouts_path.display(),
                %error,
                "Failed to persist layout store"
            );
        }
    }

    async fn reconcile_layout_auto_exclusions(
        &self,
        layout_id: &str,
        previous_zones: &[Output],
        updated_zones: &[Output],
    ) {
        let changed = {
            let mut exclusions = self.layout_auto_exclusions.write().await;
            let key = layout_auto_exclusions::LayoutAutoExclusionKey::layout(layout_id);
            let current = exclusions.get(&key).cloned().unwrap_or_default();
            let next = layout_auto_exclusions::reconcile_layout_device_exclusions(
                previous_zones,
                updated_zones,
                &current,
            );
            if next == current {
                false
            } else {
                if next.is_empty() {
                    exclusions.remove(&key);
                } else {
                    exclusions.insert(key, next);
                }
                true
            }
        };
        if changed {
            self.persist_layout_auto_exclusions().await;
        }
    }

    async fn remove_layout_auto_exclusions(&self, layout_id: &str) {
        let removed = {
            let mut exclusions = self.layout_auto_exclusions.write().await;
            exclusions
                .remove(&layout_auto_exclusions::LayoutAutoExclusionKey::layout(
                    layout_id,
                ))
                .is_some()
        };
        if removed {
            self.persist_layout_auto_exclusions().await;
        }
    }

    async fn persist_layout_auto_exclusions(&self) {
        let exclusions = self.layout_auto_exclusions.read().await;
        if let Err(error) =
            layout_auto_exclusions::save(&self.layout_auto_exclusions_path, &exclusions)
        {
            tracing::warn!(
                path = %self.layout_auto_exclusions_path.display(),
                %error,
                "Failed to persist layout auto-exclusion store"
            );
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    /// Expose cancellation barriers to persistence integration tests.
    pub fn test_hooks(&self) -> &LayoutMutationTestHooks {
        &self.test_hooks
    }

    #[cfg(feature = "persistence-test-hooks")]
    /// Wait at one installed persistence integration barrier.
    pub async fn wait_test_hook(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: &str,
    ) {
        self.test_hooks.wait(point, operation, reference).await;
    }

    #[cfg(not(feature = "persistence-test-hooks"))]
    async fn wait_test_hook(
        &self,
        _point: LayoutMutationTestPoint,
        _operation: LayoutMutationTestOperation,
        _reference: &str,
    ) {
    }

    #[doc(hidden)]
    /// Expose the catalog only to integration fixtures that seed invalid state.
    pub fn catalog_for_test(&self) -> &RwLock<HashMap<String, SpatialLayout>> {
        &self.layouts
    }

    #[doc(hidden)]
    /// Expose the catalog path to durability failure-injection fixtures.
    #[must_use]
    pub fn catalog_path_for_test(&self) -> &Path {
        &self.layouts_path
    }
}

#[cfg(not(feature = "persistence-test-hooks"))]
#[derive(Clone, Copy)]
enum LayoutMutationTestPoint {
    BeforeGuard,
    AfterMemoryMutation,
    AfterRendererMutation,
    AfterWorkflow,
}

#[cfg(not(feature = "persistence-test-hooks"))]
#[derive(Clone, Copy)]
enum LayoutMutationTestOperation {
    Create,
    Update,
    Apply,
    Preview,
    Delete,
    ConfigResize,
    SimulatorPrune,
}

async fn await_layout_workflow<T>(
    workflow: tokio::task::JoinHandle<Result<T, DomainError>>,
) -> Result<T, DomainError> {
    workflow.await.map_err(|error| {
        DomainError::Internal(anyhow::anyhow!("layout mutation workflow failed: {error}"))
    })?
}

fn layout_summary(layout: &SpatialLayout, is_active: bool) -> LayoutSummary {
    LayoutSummary {
        id: layout.id.clone(),
        name: layout.name.clone(),
        canvas_width: layout.canvas_width,
        canvas_height: layout.canvas_height,
        zone_count: layout.zones.len(),
        is_active,
    }
}

fn resolve_layout_key(
    layouts: &HashMap<String, SpatialLayout>,
    id_or_name: &str,
) -> Result<String, DomainError> {
    if layouts.contains_key(id_or_name) {
        return Ok(id_or_name.to_owned());
    }
    let matches: Vec<String> = layouts
        .iter()
        .filter(|(_, layout)| layout.name.eq_ignore_ascii_case(id_or_name))
        .map(|(id, _)| id.clone())
        .collect();
    match matches.as_slice() {
        [] => Err(DomainError::not_found(ResourceKind::Layout, id_or_name)),
        [key] => Ok(key.clone()),
        _ => Err(DomainError::conflict(format!(
            "Layout name is ambiguous: {id_or_name}"
        ))),
    }
}

fn normalize_layout_name(raw: &str) -> Result<String, DomainError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DomainError::validation("Layout name must not be empty"));
    }
    Ok(trimmed.to_owned())
}

fn validate_canvas_dimensions(width: u32, height: u32) -> Result<(), DomainError> {
    SurfaceDescriptor::rgba8888(width, height)
        .try_non_empty_byte_len()
        .map(|_| ())
        .map_err(|error| DomainError::validation(error.to_string()))
}

pub(crate) fn validate_layout_sampling_radii(layout: &SpatialLayout) -> Result<(), String> {
    validate_sampling_mode_radii(&layout.default_sampling_mode, "default sampling mode")?;
    for output in &layout.zones {
        validate_output_sampling_radii_text(output)?;
    }
    Ok(())
}

fn validate_output_sampling_radii(output: &Output) -> Result<(), DomainError> {
    validate_output_sampling_radii_text(output).map_err(DomainError::validation)
}

fn validate_output_sampling_radii_text(output: &Output) -> Result<(), String> {
    let Some(mode) = &output.sampling_mode else {
        return Ok(());
    };
    validate_sampling_mode_radii(mode, &format!("sampling mode for output '{}'", output.id))
}

fn validate_sampling_mode_radii(mode: &SamplingMode, field: &str) -> Result<(), String> {
    match mode {
        SamplingMode::AreaAverage { radius_x, radius_y } => {
            for (axis, radius) in [("radius_x", radius_x), ("radius_y", radius_y)] {
                if !radius.is_finite() || *radius < 0.0 {
                    return Err(format!(
                        "{field} {axis} must be finite and greater than or equal to 0"
                    ));
                }
            }
        }
        SamplingMode::GaussianArea { sigma, .. } => {
            if !sigma.is_finite() || *sigma < 0.0 {
                return Err(format!(
                    "{field} sigma must be finite and greater than or equal to 0"
                ));
            }
        }
        SamplingMode::Nearest | SamplingMode::Bilinear => {}
    }
    Ok(())
}

fn empty_default_layout(previous: &SpatialLayout) -> SpatialLayout {
    SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: previous.canvas_width,
        canvas_height: previous.canvas_height,
        zones: Vec::new(),
        default_sampling_mode: previous.default_sampling_mode.clone(),
        default_edge_behavior: previous.default_edge_behavior,
        spaces: previous.spaces.clone(),
        version: previous.version,
    }
}

fn layout_store_persistence_error(
    action: &str,
    error: anyhow::Error,
    rollback_errors: impl IntoIterator<Item = String>,
) -> DomainError {
    let mut message = format!("failed to persist layout store during {action}: {error}");
    for rollback_error in rollback_errors {
        message.push_str("; ");
        message.push_str(&rollback_error);
    }
    DomainError::Internal(anyhow::anyhow!(message))
}

fn layout_update_domain_error(error: LayoutUpdateError) -> DomainError {
    match &error {
        LayoutUpdateError::SpatialPlan(_)
        | LayoutUpdateError::Transaction(LayoutTransactionRejection::PreparationFailed {
            ..
        }) => DomainError::validation(error.to_string()),
        LayoutUpdateError::Transaction(LayoutTransactionRejection::Superseded)
        | LayoutUpdateError::PersistenceSuperseded => DomainError::conflict(error.to_string()),
        LayoutUpdateError::Transaction(LayoutTransactionRejection::RendererStopped)
        | LayoutUpdateError::Coordinator(_)
        | LayoutUpdateError::Persistence(_)
        | LayoutUpdateError::PersistenceRollback(_) => {
            DomainError::Internal(anyhow::anyhow!(error.to_string()))
        }
    }
}

#[derive(Clone)]
struct LayoutPersistenceContext {
    runtime_state_path: PathBuf,
    runtime_session: RuntimeSessionService,
}

async fn persist_layout_runtime_phase(
    context: &LayoutPersistenceContext,
    phase: LayoutPersistencePhase,
) -> LayoutPersistenceOutcome {
    let writer = match AtomicFileWriter::new(&context.runtime_state_path) {
        Ok(writer) => writer,
        Err(error) => return LayoutPersistenceOutcome::BeforeAdmission(error.to_string()),
    };
    let pending = match crate::runtime_state::reserve_save(&context.runtime_state_path) {
        Ok(pending) => pending,
        Err(error) => return LayoutPersistenceOutcome::BeforeAdmission(error.to_string()),
    };
    let requires_durable_completion = matches!(
        &phase,
        LayoutPersistencePhase::Rollback | LayoutPersistencePhase::Converge
    );
    let mut snapshot = context.runtime_session.snapshot().await;
    if let LayoutPersistencePhase::Precommit(candidate) = phase {
        snapshot.active_layout_id = Some(candidate.layout.id);
        if candidate.active_scene_id == Some(SceneId::DEFAULT) {
            snapshot.default_scene_groups = candidate.active_render_groups.to_vec();
        }
    }
    let outcome = match crate::runtime_state::save_reserved(pending, &snapshot) {
        Ok(AtomicWriteOutcome::Written) => LayoutPersistenceOutcome::Written,
        Ok(AtomicWriteOutcome::Superseded) => LayoutPersistenceOutcome::Superseded,
        Err(error @ RuntimeSessionError::Persist { .. }) => {
            LayoutPersistenceOutcome::RetryArmed(error.to_string())
        }
        Err(error) => LayoutPersistenceOutcome::BeforeAdmission(error.to_string()),
    };
    if requires_durable_completion
        && matches!(
            &outcome,
            LayoutPersistenceOutcome::Superseded | LayoutPersistenceOutcome::RetryArmed(_)
        )
    {
        return flush_layout_runtime_persistence(writer).await;
    }
    outcome
}

async fn flush_layout_runtime_persistence(writer: AtomicFileWriter) -> LayoutPersistenceOutcome {
    match tokio::task::spawn_blocking(move || writer.flush(LAYOUT_DURABILITY_TIMEOUT)).await {
        Ok(Ok(_)) => LayoutPersistenceOutcome::Written,
        Ok(Err(error)) => LayoutPersistenceOutcome::RetryArmed(error.to_string()),
        Err(error) => LayoutPersistenceOutcome::RetryArmed(format!(
            "layout persistence flush task failed: {error}"
        )),
    }
}
