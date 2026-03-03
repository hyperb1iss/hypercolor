//! Layout endpoints — `/api/v1/layouts/*`.
//!
//! Spatial layouts map effect canvas regions to physical LED positions.
//! This module provides CRUD operations against an in-memory store
//! of [`SpatialLayout`] objects.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

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
}

#[derive(Debug, Deserialize)]
pub struct CreateLayoutRequest {
    pub name: String,
    pub canvas_width: Option<u32>,
    pub canvas_height: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLayoutRequest {
    pub name: String,
    pub canvas_width: Option<u32>,
    pub canvas_height: Option<u32>,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/layouts` — List all spatial layouts.
pub async fn list_layouts(State(state): State<Arc<AppState>>) -> Response {
    let layouts = state.layouts.read().await;

    let items: Vec<LayoutSummary> = layouts
        .values()
        .map(|layout| LayoutSummary {
            id: layout.id.clone(),
            name: layout.name.clone(),
            canvas_width: layout.canvas_width,
            canvas_height: layout.canvas_height,
            zone_count: layout.zones.len(),
        })
        .collect();

    let total = items.len();
    ApiResponse::ok(LayoutListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/layouts/:id` — Get a single layout with full zone data.
pub async fn get_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let layouts = state.layouts.read().await;
    let Some(key) = resolve_layout_key(&layouts, &id) else {
        return ApiError::not_found(format!("Layout not found: {id}"));
    };

    let layout = layouts.get(&key).expect("resolved layout key must exist");

    ApiResponse::ok(LayoutSummary {
        id: layout.id.clone(),
        name: layout.name.clone(),
        canvas_width: layout.canvas_width,
        canvas_height: layout.canvas_height,
        zone_count: layout.zones.len(),
    })
}

/// `POST /api/v1/layouts` — Create a new layout.
pub async fn create_layout(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateLayoutRequest>,
) -> Response {
    let mut layouts = state.layouts.write().await;

    let id = format!("layout_{}", uuid::Uuid::now_v7());
    let layout = hypercolor_types::spatial::SpatialLayout {
        id: id.clone(),
        name: body.name.clone(),
        description: None,
        canvas_width: body.canvas_width.unwrap_or(320),
        canvas_height: body.canvas_height.unwrap_or(200),
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
    };

    layouts.insert(id, layout);
    ApiResponse::created(summary)
}

/// `PUT /api/v1/layouts/:id` — Update an existing layout.
pub async fn update_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLayoutRequest>,
) -> Response {
    let mut layouts = state.layouts.write().await;
    let Some(key) = resolve_layout_key(&layouts, &id) else {
        return ApiError::not_found(format!("Layout not found: {id}"));
    };

    let existing = layouts
        .get_mut(&key)
        .expect("resolved layout key must exist");

    existing.name.clone_from(&body.name);
    if let Some(w) = body.canvas_width {
        existing.canvas_width = w;
    }
    if let Some(h) = body.canvas_height {
        existing.canvas_height = h;
    }

    let summary = LayoutSummary {
        id: existing.id.clone(),
        name: existing.name.clone(),
        canvas_width: existing.canvas_width,
        canvas_height: existing.canvas_height,
        zone_count: existing.zones.len(),
    };

    ApiResponse::ok(summary)
}

/// `DELETE /api/v1/layouts/:id` — Delete a layout.
pub async fn delete_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let mut layouts = state.layouts.write().await;
    let Some(key) = resolve_layout_key(&layouts, &id) else {
        return ApiError::not_found(format!("Layout not found: {id}"));
    };

    layouts.remove(&key);

    ApiResponse::ok(serde_json::json!({
        "id": key,
        "deleted": true,
    }))
}

fn resolve_layout_key(
    layouts: &std::collections::HashMap<String, hypercolor_types::spatial::SpatialLayout>,
    id_or_name: &str,
) -> Option<String> {
    if layouts.contains_key(id_or_name) {
        return Some(id_or_name.to_owned());
    }

    layouts
        .iter()
        .find(|(_, layout)| layout.name.eq_ignore_ascii_case(id_or_name))
        .map(|(id, _)| id.clone())
}
