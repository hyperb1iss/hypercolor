//! Spatial layout catalog, mutation, activation, and durability authority.

mod auto_layout;
mod catalog;
mod convergence;
mod exclusions;
mod publication;
mod workflows;

use std::collections::{HashMap, HashSet};
#[cfg(feature = "persistence-test-hooks")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "persistence-test-hooks")]
use std::sync::Mutex as StdMutex;

use hypercolor_types::api::layouts::LayoutSummary;
use hypercolor_types::canvas::SurfaceDescriptor;
use hypercolor_types::device::DeviceId;
use hypercolor_types::scene::{SceneId, Zone, ZoneId};
use hypercolor_types::spatial::{Output, SamplingMode, SpatialLayout};
use tokio::sync::watch;
#[cfg(feature = "persistence-test-hooks")]
use tokio::sync::{Notify, RwLock, Semaphore};

use crate::discovery::DiscoveryRuntime;
use crate::domain::context::RuntimeSessionProjection;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::domain::{DomainError, ResourceKind};
use crate::layout_auto_exclusions;
use crate::network::DaemonDriverHost;
use crate::scene_transactions::{
    LayoutTransactionRejection, LayoutUpdateError, LayoutUpdateGuard, SceneActivationGuard,
    SceneTransactionQueue,
};

use self::catalog::LayoutCatalog;
use self::convergence::LayoutConvergence;
use self::exclusions::LayoutExclusions;
use self::publication::LayoutPublication;

#[derive(Clone)]
pub(crate) struct LayoutRuntime {
    discovery: DiscoveryRuntime,
    driver_host: Arc<DaemonDriverHost>,
}

impl LayoutRuntime {
    pub(crate) fn new(discovery: DiscoveryRuntime, driver_host: Arc<DaemonDriverHost>) -> Self {
        Self {
            discovery,
            driver_host,
        }
    }

    const fn discovery(&self) -> &DiscoveryRuntime {
        &self.discovery
    }

    fn driver_host(&self) -> Arc<DaemonDriverHost> {
        Arc::clone(&self.driver_host)
    }
}

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
pub struct LayoutTestFixture<'a> {
    context: &'a LayoutContext,
}

/// Narrow test composition facade for exercising complete layout workflows.
#[doc(hidden)]
pub struct LayoutTestWorkflows<'a> {
    context: &'a LayoutContext,
}

#[cfg(feature = "persistence-test-hooks")]
impl<'a> LayoutTestFixture<'a> {
    #[must_use]
    pub fn hooks(&self) -> &'a LayoutMutationTestHooks {
        &self.context.test_hooks
    }

    #[must_use]
    pub fn catalog(&self) -> &'a RwLock<HashMap<String, SpatialLayout>> {
        self.context.catalog.entries()
    }

    #[must_use]
    pub fn catalog_path(&self) -> &'a Path {
        self.context.catalog.path()
    }

    #[must_use]
    pub fn auto_exclusions(&self) -> &'a RwLock<layout_auto_exclusions::LayoutAutoExclusionStore> {
        self.context.exclusions.entries()
    }

    pub fn replace_current(&self, layout: SpatialLayout) {
        self.context.publication.replace_current(layout);
    }

    pub async fn active_primary_ids(&self) -> (SceneId, ZoneId) {
        self.context.publication.active_primary_ids().await
    }

    pub fn append_auto_zones(
        &self,
        layout: &mut SpatialLayout,
        layout_device_id: &str,
        device_info: &hypercolor_types::device::DeviceInfo,
    ) -> usize {
        auto_layout::append_auto_layout_zones_for_device(layout, layout_device_id, device_info)
    }

    pub fn reconcile_auto_zones(
        &self,
        layout: &mut SpatialLayout,
        layout_device_id: &str,
        device_info: &hypercolor_types::device::DeviceInfo,
    ) -> usize {
        auto_layout::reconcile_auto_layout_zones_for_device(layout, layout_device_id, device_info)
    }
}

impl LayoutTestWorkflows<'_> {
    pub async fn publish(&self, layout: SpatialLayout) -> Result<(), String> {
        let guard = self.context.acquire_update_guard().await;
        let prepared = crate::scene_transactions::PreparedLayoutUpdate::try_new(layout)
            .map_err(|error| error.to_string())?;
        self.context
            .publication
            .apply_prepared_under_guard(&guard, prepared)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn sync_active_layout_for_renderable_devices(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        self.context
            .sync_active_layout_for_renderable_devices(runtime, limit_to_devices)
            .await;
    }

    pub async fn sync_connectivity(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        self.context
            .sync_connectivity(runtime, limit_to_devices)
            .await;
    }
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
    layouts: HashMap<String, SpatialLayout>,
    layouts_path: PathBuf,
    layout_auto_exclusions: layout_auto_exclusions::LayoutAutoExclusionStore,
    layout_auto_exclusions_path: PathBuf,
}

impl LayoutContextResources {
    pub(crate) fn new(
        layouts: HashMap<String, SpatialLayout>,
        layouts_path: PathBuf,
        layout_auto_exclusions: layout_auto_exclusions::LayoutAutoExclusionStore,
        layout_auto_exclusions_path: PathBuf,
    ) -> Self {
        Self {
            layouts,
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
        }
    }

    pub(crate) fn load(
        layouts_path: PathBuf,
        layout_auto_exclusions_path: PathBuf,
        default_layout: &SpatialLayout,
    ) -> Self {
        let mut layouts = crate::layout_store::load(&layouts_path).unwrap_or_else(|error| {
            tracing::warn!(
                path = %layouts_path.display(),
                %error,
                "Failed to load persisted layouts; starting with empty store"
            );
            HashMap::new()
        });
        if crate::layout_store::ensure_default_layout(&mut layouts, default_layout) {
            if let Err(error) = crate::layout_store::save(&layouts_path, &layouts) {
                tracing::warn!(
                    path = %layouts_path.display(),
                    %error,
                    "Failed to persist inserted default layout"
                );
            } else {
                tracing::info!(
                    path = %layouts_path.display(),
                    "Inserted missing default layout into persisted layout store"
                );
            }
        }
        let layout_auto_exclusions = layout_auto_exclusions::load(&layout_auto_exclusions_path)
            .unwrap_or_else(|error| {
                tracing::warn!(
                    path = %layout_auto_exclusions_path.display(),
                    %error,
                    "Failed to load layout auto-exclusions; starting with empty store"
                );
                HashMap::new()
            });
        tracing::info!(
            path = %layouts_path.display(),
            count = layouts.len(),
            "Layout store ready"
        );
        tracing::info!(
            path = %layout_auto_exclusions_path.display(),
            "Layout auto-exclusion store ready"
        );
        Self::new(
            layouts,
            layouts_path,
            layout_auto_exclusions,
            layout_auto_exclusions_path,
        )
    }

    pub(crate) fn catalog(&self) -> &HashMap<String, SpatialLayout> {
        &self.layouts
    }
}

#[derive(Clone)]
pub struct LayoutContext {
    catalog: LayoutCatalog,
    exclusions: LayoutExclusions,
    publication: LayoutPublication,
    convergence: LayoutConvergence,
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
        runtime_projection: RuntimeSessionProjection,
    ) -> Self {
        let catalog = LayoutCatalog::new(resources.layouts, resources.layouts_path);
        let exclusions = LayoutExclusions::new(
            resources.layout_auto_exclusions,
            resources.layout_auto_exclusions_path,
            scenes.clone(),
        );
        let publication = LayoutPublication::new(
            spatial,
            scenes,
            transactions,
            runtime_state_path,
            runtime_projection,
        );
        let convergence =
            LayoutConvergence::new(catalog.clone(), exclusions.clone(), publication.clone());
        Self {
            catalog,
            exclusions,
            publication,
            convergence,
            #[cfg(feature = "persistence-test-hooks")]
            test_hooks: LayoutMutationTestHooks::default(),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the fixture mirrors the production composition boundary"
    )]
    pub fn new_test_context(
        layouts: HashMap<String, SpatialLayout>,
        layouts_path: PathBuf,
        layout_auto_exclusions: layout_auto_exclusions::LayoutAutoExclusionStore,
        layout_auto_exclusions_path: PathBuf,
        spatial: SpatialService,
        scenes: SceneService,
        transactions: SceneTransactionQueue,
        runtime_state_path: PathBuf,
    ) -> Self {
        let (power, _) = watch::channel(crate::session::OutputPowerState::default());
        let projection = RuntimeSessionProjection::new(scenes.clone(), spatial.clone(), power);
        Self::new(
            LayoutContextResources {
                layouts,
                layouts_path,
                layout_auto_exclusions,
                layout_auto_exclusions_path,
            },
            spatial,
            scenes,
            transactions,
            runtime_state_path,
            projection,
        )
    }

    /// Create a narrow facade that drives the production layout workflows.
    #[doc(hidden)]
    #[must_use]
    pub const fn test_workflows(&self) -> LayoutTestWorkflows<'_> {
        LayoutTestWorkflows { context: self }
    }

    pub(crate) async fn restore_startup_layout(
        &self,
        layout_id: &str,
    ) -> Result<Option<SpatialLayout>, DomainError> {
        let Some(layout) = self.catalog.entries().read().await.get(layout_id).cloned() else {
            return Ok(None);
        };
        self.publication
            .restore_startup_layout(layout.clone())
            .await?;
        Ok(Some(layout))
    }

    pub(crate) async fn acquire_scene_activation_guard(&self) -> SceneActivationGuard {
        self.publication.acquire_scene_activation_guard().await
    }

    pub(crate) async fn acquire_update_guard(&self) -> LayoutUpdateGuard {
        self.publication.acquire_update_guard().await
    }

    pub(crate) async fn admit_persisted_update_under_guard(
        &self,
        guard: &LayoutUpdateGuard,
        layout: SpatialLayout,
        runtime: &LayoutRuntime,
    ) -> Result<(), LayoutUpdateError> {
        self.publication
            .admit_persisted_under_guard(guard, layout, runtime.driver_host())
            .await
    }

    pub(crate) async fn converge_persisted_update(
        &self,
        runtime: &LayoutRuntime,
    ) -> LayoutPersistenceStatus {
        self.sync_connectivity(runtime.discovery().clone(), None)
            .await;
        self.publication
            .persist_convergence(runtime.driver_host())
            .await
    }

    pub(crate) async fn resolved_layout_device_id(
        &self,
        runtime: &LayoutRuntime,
        device_info: &hypercolor_types::device::DeviceInfo,
    ) -> String {
        self.convergence
            .resolved_layout_device_id(runtime.discovery(), device_info)
            .await
    }

    pub(crate) async fn layout_outputs_for(
        &self,
        runtime: &LayoutRuntime,
        requested_layout_id: &str,
    ) -> Vec<Output> {
        self.convergence
            .layout_outputs_for(runtime.discovery(), requested_layout_id)
            .await
    }

    pub(crate) async fn connected_display_surface_layouts(
        &self,
        runtime: &LayoutRuntime,
    ) -> Vec<(DeviceId, String, SpatialLayout)> {
        self.convergence
            .connected_display_surface_layouts(runtime.discovery())
            .await
    }

    pub(crate) async fn active_layout_targets_enabled_device(
        &self,
        runtime: &DiscoveryRuntime,
        physical_id: DeviceId,
        layout_device_id: &str,
    ) -> bool {
        self.convergence
            .active_layout_targets_enabled_device(runtime, physical_id, layout_device_id)
            .await
    }

    pub(crate) async fn sync_connectivity(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        self.convergence
            .sync_connectivity(runtime, limit_to_devices)
            .await;
    }

    pub(crate) async fn sync_runtime_connectivity(&self, runtime: &LayoutRuntime) {
        self.sync_connectivity(runtime.discovery().clone(), None)
            .await;
    }

    pub(crate) async fn sync_active_layout_for_renderable_devices(
        &self,
        runtime: DiscoveryRuntime,
        limit_to_devices: Option<HashSet<DeviceId>>,
    ) {
        self.convergence
            .sync_active_layout(runtime, limit_to_devices)
            .await;
    }

    pub(crate) async fn persist_catalog_best_effort(&self) {
        self.catalog.persist_best_effort().await;
    }

    /// Reconcile discovery exclusions after zone layouts change.
    pub async fn reconcile_zone_auto_exclusions(
        &self,
        scene_id: SceneId,
        previous_zones: &[Zone],
        updated_zones: &[Zone],
    ) {
        let context = self.clone();
        let previous_zones = previous_zones.to_vec();
        let updated_zones = updated_zones.to_vec();
        if let Err(error) = tokio::spawn(async move {
            context
                .exclusions
                .reconcile_zones(scene_id, &previous_zones, &updated_zones)
                .await;
        })
        .await
        {
            tracing::warn!(%error, "zone layout exclusion reconciliation failed");
        }
    }

    /// Drop discovery exclusions owned by a removed zone.
    pub async fn remove_zone_auto_exclusions(&self, scene_id: SceneId, zone_id: ZoneId) {
        let context = self.clone();
        if let Err(error) = tokio::spawn(async move {
            context.exclusions.remove_zone(scene_id, zone_id).await;
        })
        .await
        {
            tracing::warn!(%error, "zone layout exclusion removal failed");
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    /// Create the explicit non-production layout fixture capability.
    pub fn test_fixture(&self) -> LayoutTestFixture<'_> {
        LayoutTestFixture { context: self }
    }

    #[cfg(feature = "persistence-test-hooks")]
    /// Wait at one installed persistence integration barrier.
    pub(crate) async fn wait_test_hook(
        &self,
        point: LayoutMutationTestPoint,
        operation: LayoutMutationTestOperation,
        reference: &str,
    ) {
        self.test_hooks.wait(point, operation, reference).await;
    }

    #[cfg(not(feature = "persistence-test-hooks"))]
    fn wait_test_hook(
        &self,
        _point: LayoutMutationTestPoint,
        _operation: LayoutMutationTestOperation,
        _reference: &str,
    ) -> std::future::Ready<()> {
        std::future::ready(())
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
