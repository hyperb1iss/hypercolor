//! Profile endpoints — `/api/v1/profiles/*`.
//!
//! Profiles are named snapshots of runtime state: active effect, control
//! values, layout, and brightness. They are persisted to `profiles.json`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use hypercolor_core::effect::create_renderer_for_metadata_with_mode;
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{effects, persist_runtime_session};
use crate::discovery;
use crate::profile_store::{Profile, ResolveProfileError};
use crate::session::{current_global_brightness, set_global_brightness};

// ── Request / Response Types ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateProfileRequest {
    pub name: String,
    pub description: Option<String>,
    pub brightness: Option<u8>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProfileRequest {
    pub name: String,
    pub description: Option<String>,
    pub brightness: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct ApplyProfileRequest {
    pub transition_ms: Option<u32>,
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
    let mut items: Vec<Profile> = profiles.values().cloned().collect();
    items.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
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
    let key = match resolve_profile_key(&profiles, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return ApiError::not_found(format!("Profile not found: {id}")),
        Err(ResolveProfileError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Profile name is ambiguous: {name}"));
        }
    };

    let profile = profiles.get(&key).expect("resolved profile key must exist");
    ApiResponse::ok(profile.clone())
}

/// `POST /api/v1/profiles` — Create a new profile from current runtime state.
pub async fn create_profile(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateProfileRequest>,
) -> Response {
    let mut profile = match snapshot_profile(
        &state,
        format!("prof_{}", Uuid::now_v7()),
        body.name,
        body.description,
        body.brightness,
    )
    .await
    {
        Ok(profile) => profile,
        Err(error) => return ApiError::validation(error),
    };

    let mut profiles = state.profiles.write().await;
    match profiles.find_existing_name_key(&profile.name, None) {
        Ok(Some(existing_id)) if body.force => {
            profile.id = existing_id;
        }
        Ok(Some(_)) => {
            return ApiError::conflict(format!("Profile already exists: {}", profile.name));
        }
        Ok(None) => {}
        Err(ResolveProfileError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Profile name is ambiguous: {name}"));
        }
    }
    profiles.insert(profile.clone());
    if let Err(error) = profiles.save() {
        return ApiError::internal(format!("Failed to persist profile store: {error}"));
    }

    if body.force {
        ApiResponse::ok(profile)
    } else {
        ApiResponse::created(profile)
    }
}

/// `PUT /api/v1/profiles/:id` — Update a profile (full replacement).
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateProfileRequest>,
) -> Response {
    let profile_id = {
        let profiles = state.profiles.read().await;
        match resolve_profile_key(&profiles, &id) {
            Ok(Some(key)) => key,
            Ok(None) => return ApiError::not_found(format!("Profile not found: {id}")),
            Err(ResolveProfileError::AmbiguousName(name)) => {
                return ApiError::conflict(format!("Profile name is ambiguous: {name}"));
            }
        }
    };

    let profile = match snapshot_profile(
        &state,
        profile_id.clone(),
        body.name,
        body.description,
        body.brightness,
    )
    .await
    {
        Ok(profile) => profile,
        Err(error) => return ApiError::validation(error),
    };

    let mut profiles = state.profiles.write().await;
    match profiles.find_existing_name_key(&profile.name, Some(&profile_id)) {
        Ok(Some(_)) => {
            return ApiError::conflict(format!("Profile already exists: {}", profile.name));
        }
        Ok(None) => {}
        Err(ResolveProfileError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Profile name is ambiguous: {name}"));
        }
    }
    profiles.insert(profile.clone());
    if let Err(error) = profiles.save() {
        return ApiError::internal(format!("Failed to persist profile store: {error}"));
    }

    ApiResponse::ok(profile)
}

/// `DELETE /api/v1/profiles/:id` — Delete a profile.
pub async fn delete_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let mut profiles = state.profiles.write().await;
    let key = match resolve_profile_key(&profiles, &id) {
        Ok(Some(key)) => key,
        Ok(None) => return ApiError::not_found(format!("Profile not found: {id}")),
        Err(ResolveProfileError::AmbiguousName(name)) => {
            return ApiError::conflict(format!("Profile name is ambiguous: {name}"));
        }
    };

    profiles.remove(&key);
    if let Err(error) = profiles.save() {
        return ApiError::internal(format!("Failed to persist profile store: {error}"));
    }

    ApiResponse::ok(serde_json::json!({
        "id": key,
        "deleted": true,
    }))
}

/// `POST /api/v1/profiles/:id/apply` — Apply a profile.
pub async fn apply_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Option<Json<ApplyProfileRequest>>,
) -> Response {
    if let Err(error) = validate_apply_request(body.as_ref()) {
        return ApiError::bad_request(error);
    }

    let profile = {
        let profiles = state.profiles.read().await;
        let key = match resolve_profile_key(&profiles, &id) {
            Ok(Some(key)) => key,
            Ok(None) => return ApiError::not_found(format!("Profile not found: {id}")),
            Err(ResolveProfileError::AmbiguousName(name)) => {
                return ApiError::conflict(format!("Profile name is ambiguous: {name}"));
            }
        };
        profiles
            .get(&key)
            .expect("resolved profile key must exist")
            .clone()
    };

    if let Err(error) = apply_profile_snapshot(&state, &profile).await {
        return ApiError::internal(error);
    }

    state
        .event_bus
        .publish(hypercolor_types::event::HypercolorEvent::ProfileLoaded {
            profile_id: profile.id.clone(),
            profile_name: profile.name.clone(),
            trigger: hypercolor_types::event::ChangeTrigger::Api,
        });
    persist_runtime_session(&state).await;

    ApiResponse::ok(serde_json::json!({
        "profile": profile,
        "applied": true,
    }))
}

pub(crate) async fn apply_profile_snapshot(
    state: &AppState,
    profile: &Profile,
) -> Result<(), String> {
    let brightness = profile.brightness.map(|value| f32::from(value) / 100.0);
    let layout = if let Some(layout_id) = profile.layout_id.as_deref() {
        let layouts = state.layouts.read().await;
        Some(
            layouts
                .get(layout_id)
                .cloned()
                .ok_or_else(|| format!("profile layout not found: {layout_id}"))?,
        )
    } else {
        None
    };

    let prepared_effect = if let Some(effect_id) = profile.effect_id.as_deref() {
        let metadata = {
            let registry = state.effect_registry.read().await;
            effects::resolve_effect_metadata(&registry, effect_id)
                .ok_or_else(|| format!("profile effect not found: {effect_id}"))?
        };

        let render_acceleration_mode =
            crate::api::configured_render_acceleration_mode(state.config_manager.as_ref());
        let renderer = create_renderer_for_metadata_with_mode(&metadata, render_acceleration_mode)
            .map_err(|error| {
                format!(
                    "failed to prepare renderer for profile effect '{}': {error}",
                    metadata.name
                )
            })?;

        Some((metadata, renderer))
    } else {
        None
    };

    if let Some((metadata, renderer)) = prepared_effect {
        let mut engine = state.effect_engine.lock().await;
        engine
            .activate(renderer, metadata.clone())
            .map_err(|error| {
                format!(
                    "failed to activate profile effect '{}': {error}",
                    metadata.name
                )
            })?;

        let mut rejected_controls = Vec::new();
        for (name, value) in &profile.controls {
            if let Err(error) = engine.set_control_checked(name, value) {
                rejected_controls.push(format!("{name} ({error})"));
            }
        }

        if let Some(active_preset_id) = &profile.active_preset_id {
            engine.set_active_preset_id(active_preset_id.clone());
        }

        if !rejected_controls.is_empty() {
            warn!(
                profile_id = %profile.id,
                rejected_controls = ?rejected_controls,
                "Profile apply skipped one or more invalid control values"
            );
        }
    }

    if let Some(layout) = layout {
        {
            let mut spatial = state.spatial_engine.write().await;
            spatial.update_layout(layout);
        }

        let runtime = super::discovery_runtime(state);
        discovery::sync_active_layout_connectivity(&runtime, None).await;
    }

    if let Some(normalized) = brightness {
        let mut settings = state.device_settings.write().await;
        settings.set_global_brightness(normalized);
        settings
            .save()
            .map_err(|error| format!("failed to persist global brightness: {error}"))?;
        drop(settings);
        set_global_brightness(&state.power_state, normalized);
    }

    Ok(())
}

async fn snapshot_profile(
    state: &Arc<AppState>,
    id: String,
    name: String,
    description: Option<String>,
    brightness_override: Option<u8>,
) -> Result<Profile, String> {
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("name must not be empty".to_owned());
    }

    let brightness = Some(
        brightness_override
            .unwrap_or_else(|| brightness_percent(current_global_brightness(&state.power_state))),
    );
    let layout_id = {
        let spatial = state.spatial_engine.read().await;
        Some(spatial.layout().id.clone())
    };
    let (effect_id, effect_name, active_preset_id, controls) = {
        let engine = state.effect_engine.lock().await;
        (
            engine.active_metadata().map(|meta| meta.id.to_string()),
            engine.active_metadata().map(|meta| meta.name.clone()),
            engine.active_preset_id().map(ToOwned::to_owned),
            engine.active_controls().clone(),
        )
    };

    Ok(Profile {
        id,
        name,
        description,
        brightness,
        effect_id,
        effect_name,
        active_preset_id,
        controls,
        layout_id,
    }
    .normalized())
}

#[allow(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "brightness is clamped to 0-100 percent before narrowing to a byte"
)]
fn brightness_percent(brightness: f32) -> u8 {
    let scaled = (brightness.clamp(0.0, 1.0) * 100.0).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= 100.0 {
        100
    } else {
        scaled as u8
    }
}

fn resolve_profile_key(
    profiles: &crate::profile_store::ProfileStore,
    id_or_name: &str,
) -> Result<Option<String>, ResolveProfileError> {
    profiles.resolve_key(id_or_name)
}

fn validate_apply_request(body: Option<&Json<ApplyProfileRequest>>) -> Result<(), String> {
    let transition_ms = body.and_then(|payload| payload.transition_ms).unwrap_or(0);
    if transition_ms == 0 {
        return Ok(());
    }

    Err(
        "Profile transitions are not implemented yet; only immediate apply is supported today."
            .to_owned(),
    )
}
