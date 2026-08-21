//! Layout endpoints — `/api/v1/layouts/*`.
//!
//! Spatial layouts map effect canvas regions to physical LED positions.
//! This module provides CRUD operations against an in-memory store
//! of [`SpatialLayout`] objects.

#[cfg(feature = "persistence-test-hooks")]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(feature = "persistence-test-hooks")]
use std::sync::Mutex as StdMutex;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::canvas::SurfaceDescriptor;
use hypercolor_types::spatial::{Output, SamplingMode, SpatialLayout};
#[cfg(feature = "persistence-test-hooks")]
use tokio::sync::{Notify, Semaphore};

use crate::api::envelope;
use crate::api::{persist_layout_auto_exclusions, persist_layouts};
use crate::app_state::AppState;
use crate::discovery;
use crate::domain::layout::LayoutPersistenceStatus;
use crate::domain::{DomainError, ResourceKind};
use crate::layout_auto_exclusions;
use crate::scene_transactions::{
    LayoutTransactionRejection, LayoutUpdateError, PreparedLayoutUpdate,
};

pub use hypercolor_types::api::layouts::{
    ApplyLayoutResponse, CreateLayoutRequest, DeleteLayoutResponse, LayoutListQuery,
    LayoutListResponse, LayoutSummary, PreviewLayoutResponse, UpdateLayoutRequest,
};

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

    pub(crate) async fn wait(
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

#[derive(Debug)]
enum ResolveLayoutError {
    AmbiguousName(String),
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/layouts` — List all spatial layouts.
pub async fn list_layouts(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LayoutListQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return DomainError::validation("limit must be between 1 and 200").into_response();
    }
    let offset = query.offset.unwrap_or(0);

    let active_layout_id = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().id.clone()
    };
    let layouts = state.layouts.read().await;
    let mut items: Vec<LayoutSummary> = layouts
        .values()
        .map(|layout| LayoutSummary {
            id: layout.id.clone(),
            name: layout.name.clone(),
            canvas_width: layout.canvas_width,
            canvas_height: layout.canvas_height,
            zone_count: layout.zones.len(),
            is_active: layout.id == active_layout_id,
        })
        .collect();
    items.sort_by_cached_key(|item| item.name.to_lowercase());

    if query.active.unwrap_or(false) {
        items.retain(|layout| layout.is_active);
    }

    let total = items.len();
    let paged_items: Vec<LayoutSummary> = items.into_iter().skip(offset).take(limit).collect();
    let has_more = offset.saturating_add(limit) < total;
    envelope::ok(LayoutListResponse {
        items: paged_items,
        total: u64::try_from(total).expect("layout count fits in u64"),
        page: Some(hypercolor_types::api::PageInfo {
            offset: u64::try_from(offset).expect("layout offset fits in u64"),
            limit: u64::try_from(limit).expect("layout limit fits in u64"),
            has_more,
        }),
    })
}

/// `GET /api/v1/layouts/{id}` — Get a single layout with full zone data.
pub async fn get_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let layouts = state.layouts.read().await;
    let key = match resolve_layout_key(&layouts, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return DomainError::not_found(ResourceKind::Layout, &id).into_response(),
        Err(ResolveLayoutError::AmbiguousName(name)) => {
            return DomainError::conflict(format!("Layout name is ambiguous: {name}"))
                .into_response();
        }
    };

    let layout = layouts.get(&key).expect("resolved layout key must exist");
    envelope::ok(layout)
}

/// `GET /api/v1/layouts/active` — Get currently active layout.
pub async fn get_active_layout(State(state): State<Arc<AppState>>) -> Response {
    let active = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };
    envelope::ok(active)
}

/// `POST /api/v1/layouts` — Create a new layout.
pub async fn create_layout(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateLayoutRequest>,
) -> Response {
    await_layout_mutation(tokio::spawn(create_layout_workflow(state, body))).await
}

async fn create_layout_workflow(state: Arc<AppState>, body: CreateLayoutRequest) -> Response {
    let normalized_name = match normalize_layout_name(&body.name) {
        Ok(name) => name,
        Err(error) => return DomainError::validation(error).into_response(),
    };
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Create,
            &normalized_name,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let (default_canvas_width, default_canvas_height) = {
        let spatial = state.spatial_engine.snapshot();
        let layout = spatial.layout();
        (layout.canvas_width, layout.canvas_height)
    };
    let canvas_width = body.canvas_width.unwrap_or(default_canvas_width);
    let canvas_height = body.canvas_height.unwrap_or(default_canvas_height);
    if let Err(error) = validate_canvas_dimensions(canvas_width, canvas_height) {
        return DomainError::validation(error).into_response();
    }

    let mut layouts = state.layouts.write().await;
    if layouts
        .values()
        .any(|layout| layout.name.eq_ignore_ascii_case(&normalized_name))
    {
        return DomainError::conflict(format!("Layout already exists: {normalized_name}"))
            .into_response();
    }

    #[cfg(feature = "persistence-test-hooks")]
    let mutation_reference = normalized_name.clone();
    let id = format!("layout_{}", uuid::Uuid::now_v7());
    let layout = SpatialLayout {
        id: id.clone(),
        name: normalized_name,
        description: body.description,
        canvas_width,
        canvas_height,
        zones: Vec::new(),
        default_sampling_mode: hypercolor_types::spatial::SamplingMode::Bilinear,
        default_edge_behavior: hypercolor_types::spatial::EdgeBehavior::Clamp,
        spaces: None,
        version: 1,
    };

    let summary = LayoutSummary {
        id: layout.id.clone(),
        name: layout.name.clone(),
        canvas_width: layout.canvas_width,
        canvas_height: layout.canvas_height,
        zone_count: 0,
        is_active: false,
    };

    layouts.insert(id.clone(), layout);
    drop(layouts);
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::Create,
            &mutation_reference,
        )
        .await;
    if let Err(error) = persist_layouts(&state).await {
        state.layouts.write().await.remove(&id);
        let rollback_errors = persist_layouts(&state)
            .await
            .err()
            .map(|error| format!("layout store rollback failed: {error}"));
        drop(guard);
        return layout_store_persistence_error_response("create", error, rollback_errors);
    }
    drop(guard);
    envelope::created(summary)
}

/// `PUT /api/v1/layouts/{id}` — Update an existing layout.
pub async fn update_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLayoutRequest>,
) -> Response {
    await_layout_mutation(tokio::spawn(update_layout_workflow(state, id, body))).await
}

async fn update_layout_workflow(
    state: Arc<AppState>,
    id: String,
    body: UpdateLayoutRequest,
) -> Response {
    if let Some(zones) = &body.zones {
        for output in zones {
            if let Err(error) = validate_output_sampling_radii(output) {
                return DomainError::validation(error).into_response();
            }
        }
    }

    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Update,
            &id,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let active_layout_id = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().id.clone()
    };
    let mut layouts = state.layouts.write().await;
    let key = match resolve_layout_key(&layouts, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return DomainError::not_found(ResourceKind::Layout, &id).into_response(),
        Err(ResolveLayoutError::AmbiguousName(name)) => {
            return DomainError::conflict(format!("Layout name is ambiguous: {name}"))
                .into_response();
        }
    };

    let existing = layouts
        .get(&key)
        .expect("resolved layout key must exist")
        .clone();

    let UpdateLayoutRequest {
        name,
        description,
        canvas_width,
        canvas_height,
        zones,
    } = body;
    let previous_zones = zones.as_ref().map(|_| existing.zones.clone());
    let updated_zones_for_exclusions = zones.clone();
    let layout_id = existing.id.clone();
    let previous_layout = existing.clone();
    let mut updated = existing;

    if let Some(name) = name {
        let normalized_name = match normalize_layout_name(&name) {
            Ok(name) => name,
            Err(error) => return DomainError::validation(error).into_response(),
        };
        updated.name = normalized_name;
    }
    updated.description = description;

    if let Some(w) = canvas_width {
        updated.canvas_width = w;
    }
    if let Some(h) = canvas_height {
        updated.canvas_height = h;
    }
    if let Err(error) = validate_canvas_dimensions(updated.canvas_width, updated.canvas_height) {
        return DomainError::validation(error).into_response();
    }

    if let Some(zones) = zones {
        updated.zones = zones;
    }
    if let Err(error) = SpatialEngine::try_new(updated.clone()) {
        return DomainError::validation(error.to_string()).into_response();
    }

    let summary = LayoutSummary {
        id: updated.id.clone(),
        name: updated.name.clone(),
        canvas_width: updated.canvas_width,
        canvas_height: updated.canvas_height,
        zone_count: updated.zones.len(),
        is_active: updated.id == active_layout_id,
    };
    layouts.insert(key, updated);

    drop(layouts);
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::Update,
            &layout_id,
        )
        .await;
    if let Err(error) = persist_layouts(&state).await {
        state
            .layouts
            .write()
            .await
            .insert(layout_id.clone(), previous_layout);
        let rollback_errors = persist_layouts(&state)
            .await
            .err()
            .map(|error| format!("layout store rollback failed: {error}"));
        drop(guard);
        return layout_store_persistence_error_response("update", error, rollback_errors);
    }
    if let (Some(previous_zones), Some(updated_zones)) =
        (previous_zones, updated_zones_for_exclusions)
    {
        update_layout_auto_exclusions(&state, &layout_id, &previous_zones, &updated_zones).await;
    }
    drop(guard);
    envelope::ok(summary)
}

/// `POST /api/v1/layouts/{id}/apply` — Apply a saved layout to the spatial engine.
pub async fn apply_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    await_layout_mutation(tokio::spawn(apply_layout_workflow(state, id))).await
}

async fn apply_layout_workflow(state: Arc<AppState>, id: String) -> Response {
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Apply,
            &id,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let layout = {
        let layouts = state.layouts.read().await;
        let key = match resolve_layout_key(&layouts, &id) {
            Ok(Some(key)) => key,
            Ok(None) => return DomainError::not_found(ResourceKind::Layout, &id).into_response(),
            Err(ResolveLayoutError::AmbiguousName(name)) => {
                return DomainError::conflict(format!("Layout name is ambiguous: {name}"))
                    .into_response();
            }
        };
        layouts
            .get(&key)
            .expect("resolved layout key must exist")
            .clone()
    };

    let admission = state
        .layout
        .admit_persisted_update_under_guard(&guard, layout.clone())
        .await;
    drop(guard);
    if let Err(error) = admission {
        return layout_update_error_response(error);
    }
    let persistence = state.layout.converge_persisted_update().await;
    layout_persistence_response(
        ApplyLayoutResponse {
            layout,
            applied: true,
            persistence_pending: persistence == LayoutPersistenceStatus::Pending,
        },
        persistence,
    )
}

/// `PUT /api/v1/layouts/active/preview` — Push a layout to the spatial engine without persisting.
///
/// Used by the layout editor for live preview while dragging zones.
pub async fn preview_layout(
    State(state): State<Arc<AppState>>,
    Json(layout): Json<SpatialLayout>,
) -> Response {
    await_layout_mutation(tokio::spawn(preview_layout_workflow(state, layout))).await
}

async fn preview_layout_workflow(state: Arc<AppState>, layout: SpatialLayout) -> Response {
    if let Err(error) = validate_canvas_dimensions(layout.canvas_width, layout.canvas_height) {
        return DomainError::validation(error).into_response();
    }
    if let Err(error) = validate_layout_sampling_radii(&layout) {
        return DomainError::validation(error).into_response();
    }

    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Preview,
            &layout.id,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    #[cfg(feature = "persistence-test-hooks")]
    let reference = layout.id.clone();
    let prepared = match PreparedLayoutUpdate::try_new(layout) {
        Ok(prepared) => prepared,
        Err(error) => return layout_update_error_response(error.into()),
    };
    if let Err(error) = crate::scene_transactions::apply_prepared_layout_update_under_guard(
        state.spatial_engine.clone(),
        state.scene_manager.clone(),
        state.scene_transactions.clone(),
        &guard,
        prepared,
    )
    .await
    {
        return layout_update_error_response(error);
    }
    drop(guard);
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::AfterRendererMutation,
            LayoutMutationTestOperation::Preview,
            &reference,
        )
        .await;
    let runtime = super::discovery_runtime(&state);
    discovery::sync_active_layout_connectivity(&runtime, None).await;
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::AfterWorkflow,
            LayoutMutationTestOperation::Preview,
            &reference,
        )
        .await;

    envelope::ok(PreviewLayoutResponse { previewing: true })
}

/// `DELETE /api/v1/layouts/{id}` — Delete a layout.
pub async fn delete_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    await_layout_mutation(tokio::spawn(delete_layout_workflow(state, id))).await
}

async fn delete_layout_workflow(state: Arc<AppState>, id: String) -> Response {
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::BeforeGuard,
            LayoutMutationTestOperation::Delete,
            &id,
        )
        .await;
    let guard = state.scene_transactions.acquire_layout_update_guard().await;
    let active_layout = {
        let spatial = state.spatial_engine.snapshot();
        spatial.layout().as_ref().clone()
    };
    let mut layouts = state.layouts.write().await;
    let key = match resolve_layout_key(&layouts, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return DomainError::not_found(ResourceKind::Layout, &id).into_response(),
        Err(ResolveLayoutError::AmbiguousName(name)) => {
            return DomainError::conflict(format!("Layout name is ambiguous: {name}"))
                .into_response();
        }
    };

    let removed_layout = layouts
        .remove(&key)
        .expect("resolved layout key must exist");
    let next_active_layout = if key == active_layout.id {
        let mut candidates: Vec<SpatialLayout> = layouts.values().cloned().collect();
        candidates.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Some(
            candidates
                .into_iter()
                .next()
                .unwrap_or_else(|| empty_default_layout(&active_layout)),
        )
    } else {
        None
    };
    drop(layouts);
    #[cfg(feature = "persistence-test-hooks")]
    state
        .layout_mutation_test_hooks
        .wait(
            LayoutMutationTestPoint::AfterMemoryMutation,
            LayoutMutationTestOperation::Delete,
            &key,
        )
        .await;

    let active_layout_changed = next_active_layout.is_some();
    if let Some(layout) = next_active_layout
        && let Err(error) = state
            .layout
            .admit_persisted_update_under_guard(&guard, layout)
            .await
    {
        state
            .layouts
            .write()
            .await
            .insert(key.clone(), removed_layout);
        drop(guard);
        return layout_update_error_response(error);
    }
    if let Err(error) = persist_layouts(&state).await {
        state
            .layouts
            .write()
            .await
            .insert(key.clone(), removed_layout);
        let mut rollback_errors = Vec::new();
        if let Err(rollback_error) = persist_layouts(&state).await {
            rollback_errors.push(format!("layout store rollback failed: {rollback_error}"));
        }
        if active_layout_changed
            && let Err(rollback_error) = state
                .layout
                .admit_persisted_update_under_guard(&guard, active_layout)
                .await
        {
            rollback_errors.push(format!("active layout rollback failed: {rollback_error}"));
        }
        drop(guard);
        if active_layout_changed
            && state.layout.converge_persisted_update().await == LayoutPersistenceStatus::Pending
        {
            rollback_errors.push("active layout rollback persistence remains pending".to_owned());
        }
        return layout_store_persistence_error_response("delete", error, rollback_errors);
    }
    let exclusions_changed = {
        let mut exclusions = state.layout_auto_exclusions.write().await;
        exclusions
            .remove(&layout_auto_exclusions::LayoutAutoExclusionKey::layout(
                key.as_str(),
            ))
            .is_some()
    };

    if exclusions_changed {
        persist_layout_auto_exclusions(&state).await;
    }
    drop(guard);

    let persistence = if active_layout_changed {
        state.layout.converge_persisted_update().await
    } else {
        LayoutPersistenceStatus::Synchronized
    };

    layout_persistence_response(
        DeleteLayoutResponse {
            id: key,
            deleted: true,
            persistence_pending: persistence == LayoutPersistenceStatus::Pending,
        },
        persistence,
    )
}

async fn await_layout_mutation(workflow: tokio::task::JoinHandle<Response>) -> Response {
    match workflow.await {
        Ok(response) => response,
        Err(error) => layout_update_error_response(LayoutUpdateError::Coordinator(format!(
            "layout mutation workflow failed: {error}"
        ))),
    }
}

fn layout_store_persistence_error_response(
    action: &str,
    error: anyhow::Error,
    rollback_errors: impl IntoIterator<Item = String>,
) -> Response {
    let mut message = format!("failed to persist layout store during {action}: {error}");
    for rollback_error in rollback_errors {
        message.push_str("; ");
        message.push_str(&rollback_error);
    }
    DomainError::Internal(anyhow::anyhow!(message)).into_response()
}

fn layout_update_error_response(error: LayoutUpdateError) -> Response {
    match &error {
        LayoutUpdateError::SpatialPlan(_)
        | LayoutUpdateError::Transaction(LayoutTransactionRejection::PreparationFailed {
            ..
        }) => DomainError::validation(error.to_string()).into_response(),
        LayoutUpdateError::Transaction(LayoutTransactionRejection::Superseded)
        | LayoutUpdateError::PersistenceSuperseded => {
            DomainError::conflict(error.to_string()).into_response()
        }
        LayoutUpdateError::Transaction(LayoutTransactionRejection::RendererStopped)
        | LayoutUpdateError::Coordinator(_)
        | LayoutUpdateError::Persistence(_)
        | LayoutUpdateError::PersistenceRollback(_) => {
            DomainError::Internal(anyhow::anyhow!(error.to_string())).into_response()
        }
    }
}

fn layout_persistence_response<T: serde::Serialize>(
    data: T,
    persistence: LayoutPersistenceStatus,
) -> Response {
    match persistence {
        LayoutPersistenceStatus::Synchronized => envelope::ok(data),
        LayoutPersistenceStatus::Pending => envelope::accepted(data),
    }
}

async fn update_layout_auto_exclusions(
    state: &Arc<AppState>,
    layout_id: &str,
    previous_zones: &[Output],
    updated_zones: &[Output],
) {
    let changed = {
        let mut exclusions = state.layout_auto_exclusions.write().await;
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
        persist_layout_auto_exclusions(state).await;
    }
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

fn resolve_layout_key(
    layouts: &std::collections::HashMap<String, SpatialLayout>,
    id_or_name: &str,
) -> Result<Option<String>, ResolveLayoutError> {
    if layouts.contains_key(id_or_name) {
        return Ok(Some(id_or_name.to_owned()));
    }

    let matches: Vec<String> = layouts
        .iter()
        .filter(|(_, layout)| layout.name.eq_ignore_ascii_case(id_or_name))
        .map(|(id, _)| id.clone())
        .collect();

    if matches.len() > 1 {
        return Err(ResolveLayoutError::AmbiguousName(id_or_name.to_owned()));
    }

    Ok(matches.first().cloned())
}

fn normalize_layout_name(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Layout name must not be empty".to_owned());
    }
    Ok(trimmed.to_owned())
}

fn validate_canvas_dimensions(width: u32, height: u32) -> Result<(), String> {
    SurfaceDescriptor::rgba8888(width, height)
        .try_non_empty_byte_len()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn validate_layout_sampling_radii(layout: &SpatialLayout) -> Result<(), String> {
    validate_sampling_mode_radii(&layout.default_sampling_mode, "default sampling mode")?;
    for output in &layout.zones {
        validate_output_sampling_radii(output)?;
    }
    Ok(())
}

pub(crate) fn validate_output_sampling_radii(output: &Output) -> Result<(), String> {
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
