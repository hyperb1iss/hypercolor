//! Profile endpoints — `/api/v1/profiles/*`.
//!
//! Profiles are serializable snapshots of the complete system state:
//! active effect, control values, layout, device states, and brightness.
//! This module provides CRUD and apply operations against an in-memory store.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};

// ── Profile Model ────────────────────────────────────────────────────────

/// In-memory profile representation (not in `hypercolor-types` yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub brightness: Option<u8>,
}

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    pub description: Option<String>,
    pub brightness: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub name: String,
    pub description: Option<String>,
    pub brightness: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct ProfileListResponse {
    pub items: Vec<Profile>,
    pub pagination: super::devices::Pagination,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `GET /api/v1/profiles` — List all profiles.
pub async fn list_profiles(State(state): State<Arc<AppState>>) -> Response {
    let profiles = state.profiles.read().await;
    let items: Vec<Profile> = profiles.values().cloned().collect();
    let total = items.len();

    ApiResponse::ok(ProfileListResponse {
        items,
        pagination: super::devices::Pagination {
            offset: 0,
            limit: 50,
            total,
            has_more: false,
        },
    })
}

/// `GET /api/v1/profiles/:id` — Get a single profile.
pub async fn get_profile(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let profiles = state.profiles.read().await;

    let Some(profile) = profiles.get(&id) else {
        return ApiError::not_found(format!("Profile not found: {id}"));
    };

    ApiResponse::ok(profile.clone())
}

/// `POST /api/v1/profiles` — Create a new profile.
pub async fn create_profile(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProfileRequest>,
) -> Response {
    let mut profiles = state.profiles.write().await;

    let id = format!("prof_{}", Uuid::now_v7());

    let profile = Profile {
        id: id.clone(),
        name: body.name,
        description: body.description,
        brightness: body.brightness,
    };

    profiles.insert(id, profile.clone());
    ApiResponse::created(profile)
}

/// `PUT /api/v1/profiles/:id` — Update a profile (full replacement).
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProfileRequest>,
) -> Response {
    let mut profiles = state.profiles.write().await;

    if !profiles.contains_key(&id) {
        return ApiError::not_found(format!("Profile not found: {id}"));
    }

    let profile = Profile {
        id: id.clone(),
        name: body.name,
        description: body.description,
        brightness: body.brightness,
    };

    profiles.insert(id, profile.clone());
    ApiResponse::ok(profile)
}

/// `DELETE /api/v1/profiles/:id` — Delete a profile.
pub async fn delete_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let mut profiles = state.profiles.write().await;

    if profiles.remove(&id).is_none() {
        return ApiError::not_found(format!("Profile not found: {id}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": id,
        "deleted": true,
    }))
}

/// `POST /api/v1/profiles/:id/apply` — Apply a profile.
pub async fn apply_profile(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let profiles = state.profiles.read().await;

    let Some(profile) = profiles.get(&id) else {
        return ApiError::not_found(format!("Profile not found: {id}"));
    };

    // In a full implementation, this would set the effect, controls,
    // layout, device states, and brightness from the profile.
    // For now, publish an event and return the profile reference.
    state.event_bus.publish(
        hypercolor_core::types::event::HypercolorEvent::ProfileLoaded {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            trigger: hypercolor_core::types::event::ChangeTrigger::Api,
        },
    );

    ApiResponse::ok(serde_json::json!({
        "profile": {
            "id": profile.id,
            "name": profile.name,
        },
        "applied": true,
    }))
}
