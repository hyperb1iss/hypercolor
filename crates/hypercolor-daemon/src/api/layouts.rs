//! Layout transport adapters for `/api/v1/layouts/*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use hypercolor_types::spatial::SpatialLayout;

use crate::api::envelope;
use crate::app_state::AppState;
use crate::domain::DomainError;
use crate::domain::layout::LayoutPersistenceStatus;

pub use hypercolor_types::api::layouts::{
    ApplyLayoutResponse, CreateLayoutRequest, DeleteLayoutResponse, LayoutListQuery,
    LayoutListResponse, LayoutSummary, PreviewLayoutResponse, UpdateLayoutRequest,
};

pub async fn list_layouts(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LayoutListQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(50);
    if limit == 0 || limit > 200 {
        return DomainError::validation("limit must be between 1 and 200").into_response();
    }
    let response = state
        .domains
        .layout
        .list(
            limit,
            query.offset.unwrap_or(0),
            query.active.unwrap_or(false),
        )
        .await;
    envelope::ok(response)
}

pub async fn get_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.domains.layout.resolve(&id).await {
        Ok(layout) => envelope::ok(layout),
        Err(error) => error.into_response(),
    }
}

pub async fn get_active_layout(State(state): State<Arc<AppState>>) -> Response {
    envelope::ok(state.domains.layout.current())
}

pub async fn create_layout(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateLayoutRequest>,
) -> Response {
    match state.domains.layout.create(body).await {
        Ok(summary) => envelope::created(summary),
        Err(error) => error.into_response(),
    }
}

pub async fn update_layout(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateLayoutRequest>,
) -> Response {
    match state.domains.layout.update(id, body).await {
        Ok(summary) => envelope::ok(summary),
        Err(error) => error.into_response(),
    }
}

pub async fn apply_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.domains.layout.apply(id).await {
        Ok(result) => layout_persistence_response(result.data, result.persistence),
        Err(error) => error.into_response(),
    }
}

pub async fn preview_layout(
    State(state): State<Arc<AppState>>,
    Json(layout): Json<SpatialLayout>,
) -> Response {
    match state.domains.layout.preview(layout).await {
        Ok(response) => envelope::ok(response),
        Err(error) => error.into_response(),
    }
}

pub async fn delete_layout(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.domains.layout.delete(id).await {
        Ok(result) => layout_persistence_response(result.data, result.persistence),
        Err(error) => error.into_response(),
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
