//! Layout endpoints — `/api/v1/layouts/*`.
//!
//! Spatial layouts map effect canvas regions to physical LED positions.
//! This module provides CRUD operations against an in-memory store
//! of [`SpatialLayout`] objects.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use hypercolor_types::spatial::{DeviceZone, SpatialLayout, ZoneGroup};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{persist_layouts, persist_runtime_session};

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
    pub group_count: usize,
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
    pub groups: Option<Vec<ZoneGroup>>,
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
            group_count: layout.groups.len(),
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
        spatial.layout().clone()
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
        groups: Vec::new(),
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
        group_count: 0,
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
        mut zones,
        groups,
    } = body;

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

    let groups_replaced = if let Some(groups) = groups {
        existing.groups = groups;
        true
    } else {
        false
    };

    if let Some(zones) = zones.as_mut() {
        sanitize_group_membership(&existing.groups, zones);
    } else if groups_replaced {
        sanitize_group_membership(&existing.groups, &mut existing.zones);
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
        group_count: existing.groups.len(),
        is_active: existing.id == active_layout_id,
    };

    drop(layouts);
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

    {
        let mut spatial = state.spatial_engine.write().await;
        spatial.update_layout(layout.clone());
    }
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
    Json(mut layout): Json<SpatialLayout>,
) -> Response {
    sanitize_group_membership(&layout.groups, &mut layout.zones);

    {
        let mut spatial = state.spatial_engine.write().await;
        spatial.update_layout(layout);
    }

    ApiResponse::ok(serde_json::json!({ "previewing": true }))
}

/// `DELETE /api/v1/layouts/:id` — Delete a layout.
pub async fn delete_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let active_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().clone()
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

    if let Some(layout) = next_active_layout {
        {
            let mut spatial = state.spatial_engine.write().await;
            spatial.update_layout(layout);
        }
        persist_runtime_session(&state).await;
    }

    persist_layouts(&state).await;

    ApiResponse::ok(serde_json::json!({
        "id": key,
        "deleted": true,
    }))
}

fn empty_default_layout(previous: &SpatialLayout) -> SpatialLayout {
    SpatialLayout {
        id: "default".to_owned(),
        name: "Default Layout".to_owned(),
        description: None,
        canvas_width: previous.canvas_width,
        canvas_height: previous.canvas_height,
        zones: Vec::new(),
        groups: Vec::new(),
        default_sampling_mode: previous.default_sampling_mode.clone(),
        default_edge_behavior: previous.default_edge_behavior.clone(),
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

fn sanitize_group_membership(groups: &[ZoneGroup], zones: &mut [DeviceZone]) {
    let valid_group_ids: HashSet<&str> = groups.iter().map(|group| group.id.as_str()).collect();

    for zone in zones.iter_mut() {
        let Some(group_id) = zone.group_id.as_ref() else {
            continue;
        };
        if valid_group_ids.contains(group_id.as_str()) {
            continue;
        }

        warn!(
            zone_id = %zone.id,
            group_id = %group_id,
            "Clearing orphaned layout zone group reference"
        );
        zone.group_id = None;
    }
}
