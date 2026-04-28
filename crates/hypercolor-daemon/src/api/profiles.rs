//! Profile endpoints — `/api/v1/profiles/*`.
//!
//! Profiles are named snapshots of runtime state: active primary effect,
//! display face assignments, control values, layout, and brightness. They are
//! persisted to `profiles.json`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tracing::warn;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::{effects, persist_runtime_session};
use crate::discovery;
use crate::profile_store::{Profile, ProfileDisplay, ProfilePrimary, ResolveProfileError};
use crate::scene_transactions::apply_layout_update;
use crate::session::{current_global_brightness, set_global_brightness};
use hypercolor_core::effect::EffectRegistry;
use hypercolor_types::device::DeviceId;
use hypercolor_types::effect::{ControlValue, EffectMetadata};
use hypercolor_types::library::PresetId;
use hypercolor_types::scene::{RenderGroup, RenderGroupRole};
use hypercolor_types::spatial::SpatialLayout;

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

#[derive(Debug, Deserialize, ToSchema)]
pub struct ApplyProfileRequest {
    pub transition_ms: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ProfileListResponse {
    pub items: Vec<Profile>,
    pub pagination: super::devices::Pagination,
}

#[derive(Debug, Clone)]
struct PreparedProfilePrimary {
    metadata: EffectMetadata,
    controls: HashMap<String, ControlValue>,
    active_preset_id: Option<PresetId>,
}

#[derive(Debug, Clone)]
struct PreparedProfileDisplay {
    device_id: DeviceId,
    device_name: String,
    metadata: EffectMetadata,
    controls: HashMap<String, ControlValue>,
    layout: SpatialLayout,
}

#[derive(Debug, Clone)]
pub(crate) enum ProfileApplyError {
    Conflict(String),
    Internal(String),
}

impl std::fmt::Display for ProfileApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(error) | Self::Internal(error) => f.write_str(error),
        }
    }
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

    let warnings = match apply_profile_snapshot(&state, &profile).await {
        Ok(warnings) => warnings,
        Err(ProfileApplyError::Conflict(error)) => return ApiError::conflict(error),
        Err(ProfileApplyError::Internal(error)) => return ApiError::internal(error),
    };

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
        "warnings": warnings,
    }))
}

pub(crate) async fn apply_profile_snapshot(
    state: &AppState,
    profile: &Profile,
) -> Result<Vec<String>, ProfileApplyError> {
    {
        let scene_manager = state.scene_manager.read().await;
        crate::api::active_scene_id_for_runtime_mutation(&scene_manager)
            .map_err(|error| ProfileApplyError::Conflict(error.message("applying a profile")))?;
    }
    let brightness = profile.brightness.map(|value| f32::from(value) / 100.0);
    let layout = if let Some(layout_id) = profile.layout_id.as_deref() {
        let layouts = state.layouts.read().await;
        Some(layouts.get(layout_id).cloned().ok_or_else(|| {
            ProfileApplyError::Internal(format!("profile layout not found: {layout_id}"))
        })?)
    } else {
        None
    };
    let prepared_primary = prepare_profile_primary(state, profile.primary.as_ref())
        .await
        .map_err(ProfileApplyError::Internal)?;
    let prepared_displays = prepare_profile_displays(state, &profile.displays)
        .await
        .map_err(ProfileApplyError::Internal)?;
    let current_layout = {
        let spatial = state.spatial_engine.read().await;
        spatial.layout().as_ref().clone()
    };

    if let Some(prepared_primary) = prepared_primary {
        let (controls, rejected_controls) = crate::api::effects::normalize_control_values(
            &prepared_primary.metadata,
            &prepared_primary.controls,
        );
        let active_layout = layout.clone().unwrap_or_else(|| current_layout.clone());
        {
            let mut scene_manager = state.scene_manager.write().await;
            scene_manager
                .upsert_primary_group(
                    &prepared_primary.metadata,
                    controls,
                    prepared_primary.active_preset_id,
                    active_layout,
                )
                .map_err(|error| {
                    ProfileApplyError::Internal(format!(
                        "failed to activate profile effect '{}': {error}",
                        prepared_primary.metadata.name
                    ))
                })?;
        }

        if !rejected_controls.is_empty() {
            warn!(
                profile_id = %profile.id,
                rejected_controls = ?rejected_controls,
                "Profile apply skipped one or more invalid control values"
            );
        }
    }

    if !prepared_displays.is_empty() {
        let mut scene_manager = state.scene_manager.write().await;
        for prepared_display in prepared_displays {
            let (controls, rejected_controls) = crate::api::effects::normalize_control_values(
                &prepared_display.metadata,
                &prepared_display.controls,
            );
            scene_manager
                .upsert_display_group(
                    prepared_display.device_id,
                    prepared_display.device_name.as_str(),
                    &prepared_display.metadata,
                    controls,
                    prepared_display.layout,
                )
                .map_err(|error| {
                    ProfileApplyError::Internal(format!(
                        "failed to assign profile display face '{}' to {}: {error}",
                        prepared_display.metadata.name, prepared_display.device_id
                    ))
                })?;

            if !rejected_controls.is_empty() {
                warn!(
                    profile_id = %profile.id,
                    device_id = %prepared_display.device_id,
                    rejected_controls = ?rejected_controls,
                    "Profile apply skipped one or more invalid display-face control values"
                );
            }
        }
    }

    if let Some(layout) = layout {
        apply_layout_update(
            &state.spatial_engine,
            &state.scene_manager,
            &state.scene_transactions,
            layout,
        )
        .await;

        let runtime = super::discovery_runtime(state);
        discovery::sync_active_layout_connectivity(&runtime, None).await;
    }

    if let Some(normalized) = brightness {
        let mut settings = state.device_settings.write().await;
        settings.set_global_brightness(normalized);
        settings.save().map_err(|error| {
            ProfileApplyError::Internal(format!("failed to persist global brightness: {error}"))
        })?;
        drop(settings);
        set_global_brightness(&state.power_state, normalized);
    }

    Ok(Vec::new())
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
    let (primary_group, display_groups) = {
        let scene_manager = state.scene_manager.read().await;
        scene_manager
            .active_scene()
            .map_or((None, Vec::new()), |scene| {
                (
                    scene.primary_group().cloned(),
                    scene
                        .groups
                        .iter()
                        .filter(|group| group.role == RenderGroupRole::Display)
                        .cloned()
                        .collect(),
                )
            })
    };
    let registry = state.effect_registry.read().await;
    let primary = primary_group
        .as_ref()
        .and_then(|group| snapshot_primary_group(&registry, group));
    let displays = display_groups
        .iter()
        .filter_map(|group| snapshot_display_group(&registry, group))
        .collect();

    Ok(Profile {
        id,
        name,
        description,
        primary,
        displays,
        brightness,
        layout_id,
    }
    .normalized())
}

async fn prepare_profile_primary(
    state: &AppState,
    primary: Option<&ProfilePrimary>,
) -> Result<Option<PreparedProfilePrimary>, String> {
    let Some(primary) = primary else {
        return Ok(None);
    };

    let metadata = {
        let registry = state.effect_registry.read().await;
        registry
            .get(&primary.effect_id)
            .map(|entry| entry.metadata.clone())
            .ok_or_else(|| format!("profile effect not found: {}", primary.effect_id))?
    };

    Ok(Some(PreparedProfilePrimary {
        metadata,
        controls: primary.controls.clone(),
        active_preset_id: primary.active_preset_id,
    }))
}

async fn prepare_profile_displays(
    state: &AppState,
    displays: &[ProfileDisplay],
) -> Result<Vec<PreparedProfileDisplay>, String> {
    let resolved_effects = {
        let registry = state.effect_registry.read().await;
        displays
            .iter()
            .map(|display| {
                registry
                    .get(&display.effect_id)
                    .map(|entry| (display, entry.metadata.clone()))
                    .ok_or_else(|| {
                        format!("profile display effect not found: {}", display.effect_id)
                    })
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut prepared = Vec::with_capacity(resolved_effects.len());
    for (display, metadata) in resolved_effects {
        let Some(tracked) = state.device_registry.get(&display.device_id).await else {
            return Err(format!(
                "profile display device not found: {}",
                display.device_id
            ));
        };
        let Some(surface) = crate::api::displays::display_surface_info(&tracked.info) else {
            return Err(format!(
                "profile display target does not support display faces: {}",
                tracked.info.name
            ));
        };
        prepared.push(PreparedProfileDisplay {
            device_id: display.device_id,
            device_name: tracked.info.name.clone(),
            metadata,
            controls: display.controls.clone(),
            layout: crate::api::displays::display_face_layout(
                display.device_id,
                tracked.info.name.as_str(),
                surface,
            ),
        });
    }

    Ok(prepared)
}

fn snapshot_primary_group(
    registry: &EffectRegistry,
    group: &RenderGroup,
) -> Option<ProfilePrimary> {
    let effect_id = group.effect_id?;
    Some(ProfilePrimary {
        effect_id,
        controls: resolved_profile_controls(registry, effect_id, group),
        active_preset_id: group.preset_id,
    })
}

fn snapshot_display_group(
    registry: &EffectRegistry,
    group: &RenderGroup,
) -> Option<ProfileDisplay> {
    let device_id = group.display_target.as_ref()?.device_id;
    let effect_id = group.effect_id?;
    Some(ProfileDisplay {
        device_id,
        effect_id,
        controls: resolved_profile_controls(registry, effect_id, group),
    })
}

fn resolved_profile_controls(
    registry: &EffectRegistry,
    effect_id: hypercolor_types::effect::EffectId,
    group: &RenderGroup,
) -> HashMap<String, ControlValue> {
    registry.get(&effect_id).map_or_else(
        || group.controls.clone(),
        |entry| effects::resolved_control_values(&entry.metadata, group),
    )
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
