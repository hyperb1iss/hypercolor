//! Layout endpoints — `/api/v1/layouts/*`.
//!
//! Spatial layouts map effect canvas regions to physical LED positions.
//! This module provides CRUD operations against an in-memory store
//! of [`SpatialLayout`] objects.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use hypercolor_types::spatial::{DeviceZone, SpatialLayout};
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{persist_layout_auto_exclusions, persist_layouts, persist_runtime_session};
use crate::discovery;
use crate::layout_auto_exclusions;
use crate::scene_transactions::apply_layout_update;

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct LayoutListResponse {
    pub items: Vec<LayoutSummary>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Serialize)]
pub struct LayoutSummary {
    pub id: String,
    pub name: String,
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub zone_count: usize,
    pub is_active: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateLayoutRequest {
    pub name: String,
    pub description: Option<String>,
    pub canvas_width: Option<u32>,
    pub canvas_height: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLayoutRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub canvas_width: Option<u32>,
    pub canvas_height: Option<u32>,
    pub zones: Option<Vec<DeviceZone>>,
}

#[derive(Debug, Deserialize, Default)]
pub struct LayoutListQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub active: Option<bool>,
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
        return ApiError::validation("limit must be between 1 and 200");
    }
    let offset = query.offset.unwrap_or(0);

    let active_layout_id = {
        let spatial = state.spatial_engine.read().await;
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
    items.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));

    if query.active.unwrap_or(false) {
        items.retain(|layout| layout.is_active);
    }

    let total = items.len();
    let paged_items: Vec<LayoutSummary> = items.into_iter().skip(offset).take(limit).collect();
    let has_more = offset.saturating_add(limit) < total;
    ApiResponse::ok(LayoutListResponse {
        items: paged_items,
        pagination: super::devices::Pagination {
            offset,
            limit,
            total,
            has_more,
        },
    })
}

/// `GET /api/v1/layouts/:id` — Get a single layout with full zone data.
pub async fn get_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let layouts = state.layouts.read().await;
    let key = match resolve_layout_key(&layouts, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return ApiError::not_found(format!("Layout not found: {id}")),
        Err(ResolveLayoutError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Layout name is ambiguous: {name}"));
        }
    };

    let layout = layouts.get(&key).expect("resolved layout key must exist");
    ApiResponse::ok(layout)
}

/// `GET /api/v1/layouts/active` — Get currently active layout.
pub async fn get_active_layout(State(state): State<Arc<AppState>>) -> Response {
    let active = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    ApiResponse::ok(active)
}

/// `POST /api/v1/layouts` — Create a new layout.
pub async fn create_layout(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateLayoutRequest>,
) -> Response {
    let normalized_name = match normalize_layout_name(&body.name) {
        Ok(name) => name,
        Err(error) => return ApiError::validation(error),
    };
    let canvas_width = body.canvas_width.unwrap_or(320);
    let canvas_height = body.canvas_height.unwrap_or(200);
    if let Err(error) = validate_canvas_dimensions(canvas_width, canvas_height) {
        return ApiError::validation(error);
    }

    let mut layouts = state.layouts.write().await;
    if layouts
        .values()
        .any(|layout| layout.name.eq_ignore_ascii_case(&normalized_name))
    {
        return ApiError::conflict(format!("Layout already exists: {normalized_name}"));
    }

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

    layouts.insert(id, layout);
    drop(layouts);
    persist_layouts(&state).await;
    ApiResponse::created(summary)
}

/// `PUT /api/v1/layouts/:id` — Update an existing layout.
pub async fn update_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLayoutRequest>,
) -> Response {
    let mut layouts = state.layouts.write().await;
    let key = match resolve_layout_key(&layouts, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return ApiError::not_found(format!("Layout not found: {id}")),
        Err(ResolveLayoutError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Layout name is ambiguous: {name}"));
        }
    };

    let existing = layouts
        .get_mut(&key)
        .expect("resolved layout key must exist");

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

    if let Some(name) = name {
        let normalized_name = match normalize_layout_name(&name) {
            Ok(name) => name,
            Err(error) => return ApiError::validation(error),
        };
        existing.name = normalized_name;
    }
    existing.description = description;

    if let Some(w) = canvas_width {
        existing.canvas_width = w;
    }
    if let Some(h) = canvas_height {
        existing.canvas_height = h;
    }
    if let Err(error) = validate_canvas_dimensions(existing.canvas_width, existing.canvas_height) {
        return ApiError::validation(error);
    }

    if let Some(zones) = zones {
        existing.zones = zones;
    }

    let active_layout_id = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().id.clone()
    };
    let summary = LayoutSummary {
        id: existing.id.clone(),
        name: existing.name.clone(),
        canvas_width: existing.canvas_width,
        canvas_height: existing.canvas_height,
        zone_count: existing.zones.len(),
        is_active: existing.id == active_layout_id,
    };

    drop(layouts);
    if let (Some(previous_zones), Some(updated_zones)) =
        (previous_zones, updated_zones_for_exclusions)
    {
        update_layout_auto_exclusions(&state, &layout_id, &previous_zones, &updated_zones).await;
    }
    persist_layouts(&state).await;
    ApiResponse::ok(summary)
}

/// `POST /api/v1/layouts/:id/apply` — Apply a saved layout to the spatial engine.
pub async fn apply_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let layout = {
        let layouts = state.layouts.read().await;
        let key = match resolve_layout_key(&layouts, &id) {
            Ok(Some(key)) => key,
            Ok(None) => return ApiError::not_found(format!("Layout not found: {id}")),
            Err(ResolveLayoutError::AmbiguousName(name)) => {
                return ApiError::conflict(format!("Layout name is ambiguous: {name}"));
            }
        };
        layouts
            .get(&key)
            .expect("resolved layout key must exist")
            .clone()
    };

    apply_layout_update(
        &state.spatial_engine,
        &state.scene_transactions,
        layout.clone(),
    )
    .await;
    let runtime = super::discovery_runtime(&state);
    discovery::sync_active_layout_connectivity(&runtime, None).await;
    persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "layout": layout,
        "applied": true,
    }))
}

/// `PUT /api/v1/layouts/active/preview` — Push a layout to the spatial engine without persisting.
///
/// Used by the layout editor for live preview while dragging zones.
pub async fn preview_layout(
    State(state): State<Arc<AppState>>,
    Json(layout): Json<SpatialLayout>,
) -> Response {
    apply_layout_update(&state.spatial_engine, &state.scene_transactions, layout).await;
    let runtime = super::discovery_runtime(&state);
    discovery::sync_active_layout_connectivity(&runtime, None).await;

    ApiResponse::ok(serde_json::json!({ "previewing": true }))
}

/// `DELETE /api/v1/layouts/:id` — Delete a layout.
pub async fn delete_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let active_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };
    let mut layouts = state.layouts.write().await;
    let key = match resolve_layout_key(&layouts, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return ApiError::not_found(format!("Layout not found: {id}")),
        Err(ResolveLayoutError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Layout name is ambiguous: {name}"));
        }
    };

    layouts.remove(&key);
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
    let exclusions_changed = {
        let mut exclusions = state.layout_auto_exclusions.write().await;
        exclusions.remove(&key).is_some()
    };

    if let Some(layout) = next_active_layout {
        apply_layout_update(&state.spatial_engine, &state.scene_transactions, layout).await;
        let runtime = super::discovery_runtime(&state);
        discovery::sync_active_layout_connectivity(&runtime, None).await;
        persist_runtime_session(&state).await;
    }

    persist_layouts(&state).await;
    if exclusions_changed {
        persist_layout_auto_exclusions(&state).await;
    }

    ApiResponse::ok(serde_json::json!({
        "id": key,
        "deleted": true,
    }))
}

async fn update_layout_auto_exclusions(
    state: &Arc<AppState>,
    layout_id: &str,
    previous_zones: &[DeviceZone],
    updated_zones: &[DeviceZone],
) {
    let changed = {
        let mut exclusions = state.layout_auto_exclusions.write().await;
        let current = exclusions.get(layout_id).cloned().unwrap_or_default();
        let next = layout_auto_exclusions::reconcile_layout_device_exclusions(
            previous_zones,
            updated_zones,
            &current,
        );
        if next == current {
            false
        } else {
            if next.is_empty() {
                exclusions.remove(layout_id);
            } else {
                exclusions.insert(layout_id.to_owned(), next);
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
    if width == 0 || height == 0 {
        return Err("canvas_width and canvas_height must be greater than 0".to_owned());
    }
    Ok(())
}
