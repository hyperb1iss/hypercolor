//! Config endpoints — `/api/v1/config*`.

use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::{info, warn};
use utoipa::ToSchema;

use hypercolor_core::config::canonical_audio_device_id;
use hypercolor_core::engine::FpsTier;
use hypercolor_core::input::{
    AudioReconfigurationConflict, InputSource, ScreenReconfigurationConflict, SourceKind,
    SourceState,
};
use hypercolor_types::audio::{AudioPipelineConfig, AudioSourceType};
use hypercolor_types::config::{CaptureConfig, HypercolorConfig};
use hypercolor_types::config_registry::{
    self, ApplyPolicy, ConfigKeyDescriptor, KeyPattern, LiveSection, Redaction,
};

use axum::Extension;

use crate::api::capture::protected_control_rejection;
use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};

pub use hypercolor_types::api::config::{ConfigApplyQuery, ConfigDocument, ConfigKeyResponse};

/// Render an internal config failure.
///
/// The chain goes to tracing and the wire sees the canonical generic
/// message, so a serialization fault cannot leak a config path or value.
fn internal_config_error(message: impl Into<String>) -> Response {
    DomainError::Internal(anyhow::anyhow!(message.into())).into_response()
}
use crate::scene_transactions::SceneTransaction;

/// The outcome of a config write, reset, or whole-config reset.
#[derive(Debug, Serialize, ToSchema)]
pub struct ConfigMutationResponse {
    /// The mutated key, or null for a whole-config reset.
    pub key: Option<String>,
    /// The effective value after the write, rendered like any read.
    /// Null for a whole-config reset, whose payload spans every key.
    pub value: Option<serde_json::Value>,
    /// Whether the daemon re-applied the change to a running subsystem.
    pub live: bool,
    /// Whether the registry classifies this key as boot-frozen, so the
    /// persisted value only takes effect at the next daemon start.
    pub requires_restart: bool,
    /// Restart-classified roots whose persisted value now differs from
    /// the one the daemon booted with.
    pub pending_restart: Vec<String>,
    /// The config file the write landed in.
    pub path: String,
}

/// `GET /api/v1/config` — the effective config, rendered for reading.
pub async fn show_config(State(state): State<Arc<AppState>>) -> Response {
    let config = config_snapshot(&state);
    let value = match serde_json::to_value(config) {
        Ok(value) => value,
        Err(error) => return internal_config_error(format!("Failed to serialize config: {error}")),
    };

    let serde_json::Value::Object(values) = redact_document(value) else {
        return internal_config_error("Effective config did not serialize as an object");
    };
    envelope::ok(ConfigDocument {
        values: values.into_iter().collect(),
    })
}

/// `GET /api/v1/config/schema` — the key registry clients read to learn
/// how each key applies, renders, and validates.
pub async fn get_config_schema() -> Response {
    envelope::ok(config_registry::schema_entries())
}

/// `GET /api/v1/config/keys/{key}` — read one dotted config key.
pub async fn get_config_key(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Response {
    if !config_registry::is_valid_key(&key) {
        return DomainError::malformed(format!("Malformed config key: {key}")).into_response();
    }

    let config = config_snapshot(&state);
    let value = match serde_json::to_value(config) {
        Ok(value) => value,
        Err(error) => return internal_config_error(format!("Failed to serialize config: {error}")),
    };

    let Some(found) = get_json_path(&value, &key) else {
        return DomainError::not_found(ResourceKind::ConfigKey, key).into_response();
    };

    envelope::ok(ConfigKeyResponse {
        value: redact_key(&key, found.clone()),
        key,
    })
}

/// Privacy-bearing config keys. Mutating them starts, retargets, or
/// enables screen, audio, or host-input capture, so they carry the same
/// control-credential requirement as the dedicated capture endpoints
/// (`PUT /capture/source` guards the identical `capture.source` mutation).
///
/// The whole `capture` domain qualifies (screen content is the most
/// sensitive plane and every leaf feeds the capture reconfiguration
/// transaction). For audio and input, only the leaves that enable capture
/// or retarget a device qualify: DSP tuning (`audio.fft_size`,
/// `audio.smoothing`, ...) and interaction routing policy shape an
/// already-consented stream and stay credential-free so a keyless install
/// keeps its sliders.
fn key_requires_protected_control(key: &str) -> bool {
    if key == "capture"
        || key
            .strip_prefix("capture")
            .is_some_and(|rest| rest.starts_with('.'))
    {
        return true;
    }
    matches!(
        key,
        "audio"
            | "audio.enabled"
            | "audio.device"
            | "input"
            | "input.enabled"
            | "input.keyboard"
            | "input.mouse"
    )
}

/// Pseudo-key resetting the LED calibration cluster in one request. Not a
/// registry key: the four fields are leaves of `capture`, not a subtree,
/// so a section reset cannot address them together.
pub(crate) const CAPTURE_CALIBRATION_RESET_KEY: &str = "capture.calibration";
pub(crate) const CAPTURE_CALIBRATION_FIELDS: [&str; 4] = [
    "capture.target_led_white_x",
    "capture.target_led_white_y",
    "capture.target_led_reference_white_nits",
    "capture.target_led_peak_nits",
];

/// `PUT /api/v1/config/keys/{key}` — write one dotted key and persist.
///
/// The request body is the value itself, so a section writes as a JSON
/// object and a string writes as a JSON string.
pub(crate) async fn put_config_key(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Query(apply): Query<ConfigApplyQuery>,
    Extension(auth_context): Extension<RequestAuthContext>,
    Json(value): Json<serde_json::Value>,
) -> Response {
    if !config_registry::is_valid_key(&key) {
        return DomainError::malformed(format!("Malformed config key: {key}")).into_response();
    }
    if key_requires_protected_control(&key)
        && let Some(rejection) = protected_control_rejection(auth_context)
    {
        return rejection;
    }

    write_config_key(&state, &key, value, apply.live).await
}

/// `DELETE /api/v1/config/keys/{key}` — restore one key to its default.
pub(crate) async fn delete_config_key(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Query(apply): Query<ConfigApplyQuery>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if key != CAPTURE_CALIBRATION_RESET_KEY && !config_registry::is_valid_key(&key) {
        return DomainError::malformed(format!("Malformed config key: {key}")).into_response();
    }
    if key_requires_protected_control(&key)
        && let Some(rejection) = protected_control_rejection(auth_context)
    {
        return rejection;
    }

    reset_config_state(&state, Some(&key), apply.live).await
}

/// `POST /api/v1/config/reset` — restore the whole config to defaults.
///
/// A full reset rewrites the capture domain along with everything else,
/// so it carries the protected-control requirement.
pub(crate) async fn reset_config(
    State(state): State<Arc<AppState>>,
    Query(apply): Query<ConfigApplyQuery>,
    Extension(auth_context): Extension<RequestAuthContext>,
) -> Response {
    if let Some(rejection) = protected_control_rejection(auth_context) {
        return rejection;
    }
    reset_config_state(&state, None, apply.live).await
}

async fn write_config_key(
    state: &Arc<AppState>,
    raw_key: &str,
    value: serde_json::Value,
    live_requested: bool,
) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return internal_config_error("Config manager unavailable in this runtime");
    };

    let current_snapshot = Arc::clone(&manager.get());
    let current = (*current_snapshot).clone();
    let mut root = match serde_json::to_value(&current) {
        Ok(v) => v,
        Err(e) => return internal_config_error(format!("Failed to serialize config: {e}")),
    };

    let key = raw_key.to_owned();
    let parsed_value = canonicalize_config_value(&key, value);
    let sections = live_sections_for(Some(&key));
    let apply_capture = sections.capture && live_requested;

    let value_is_unchanged =
        get_json_path(&root, &key).is_some_and(|current| current == &parsed_value);
    let capture_runtime_matches = if value_is_unchanged && apply_capture {
        capture_runtime_matches(state, &current_snapshot).await
    } else {
        true
    };

    if value_is_unchanged && capture_runtime_matches {
        info!(
            key,
            live_requested, "Skipping config update because value is unchanged"
        );
        return envelope::ok(mutation_result(
            manager,
            Some(key.clone()),
            Some(redact_key(&key, parsed_value)),
            false,
        ));
    }

    if !set_json_path(&mut root, &key, parsed_value.clone()) {
        return DomainError::validation(format!("Invalid config key path: {raw_key}"))
            .into_response();
    }

    let updated: HypercolorConfig = match serde_json::from_value(root) {
        Ok(cfg) => cfg,
        Err(error) => {
            return rejected_value(&key, "type validation", &error.to_string());
        }
    };
    if let Err(rejection) = validate_driver_config_scope(state, Some(&key), &updated) {
        return rejected_value(&rejection.key, "driver validation", &rejection.detail);
    }
    if let Err(error) = updated.capture.validate() {
        return DomainError::validation(error.to_string()).into_response();
    }

    if apply_capture {
        match apply_capture_config_transaction(state, &current_snapshot, updated.capture.clone())
            .await
        {
            Ok(()) => {
                let effective_config = manager.get();
                let effective_root = match serde_json::to_value(&**effective_config) {
                    Ok(value) => value,
                    Err(error) => {
                        return internal_config_error(format!(
                            "Failed to serialize canonicalized config: {error}"
                        ));
                    }
                };
                let Some(effective_value) = get_json_path(&effective_root, &key).cloned() else {
                    return internal_config_error(format!(
                        "Canonicalized config is missing expected key: {key}"
                    ));
                };
                return envelope::ok(mutation_result(
                    manager,
                    Some(key.clone()),
                    Some(redact_key(&key, effective_value)),
                    true,
                ));
            }
            Err(CaptureConfigTransactionError::Conflict) => {
                return DomainError::conflict(
                    "Capture config changed while its live runtime was prepared; retry the update",
                )
                .into_response();
            }
            Err(CaptureConfigTransactionError::Prepare(error)) => {
                return DomainError::validation(format!(
                    "Failed to prepare live screen capture config: {error}"
                ))
                .into_response();
            }
            Err(CaptureConfigTransactionError::Persist(error)) => {
                return internal_config_error(format!("Failed to persist config: {error}"));
            }
            Err(CaptureConfigTransactionError::Commit(error)) => {
                return DomainError::conflict(format!(
                    "Screen capture graph changed during live apply: {error}"
                ))
                .into_response();
            }
        }
    }

    // Re-apply the validated key against the freshest config under the
    // manager's write lock, so a concurrent targeted writer (e.g. the
    // capture restore-token sink) is not clobbered by this handler's
    // earlier snapshot.
    manager.modify(|config| {
        let reapplied = serde_json::to_value(&*config).ok().and_then(|mut root| {
            set_json_path(&mut root, &key, parsed_value.clone())
                .then(|| serde_json::from_value::<HypercolorConfig>(root).ok())
                .flatten()
        });
        *config = reapplied.unwrap_or_else(|| updated.clone());
    });
    if let Err(e) = manager.save() {
        return internal_config_error(format!("Failed to persist config: {e}"));
    }
    let effective_config = manager.get();
    let effective_root = match serde_json::to_value(&**effective_config) {
        Ok(value) => value,
        Err(error) => {
            return internal_config_error(format!(
                "Failed to serialize canonicalized config: {error}"
            ));
        }
    };
    let Some(effective_value) = get_json_path(&effective_root, &key).cloned() else {
        return internal_config_error(format!(
            "Canonicalized config is missing expected key: {}",
            key
        ));
    };

    let live_applied = apply_live_sections(state, sections, Some(&key), live_requested).await;

    // Published only after the save succeeded: consumers (sync intake,
    // UI hints) treat this as "the persisted config changed". The
    // payload renders like any other read surface, so a secret-
    // classified key fans out masked.
    let old_value = serde_json::to_value(&current)
        .ok()
        .and_then(|previous| get_json_path(&previous, &key).cloned())
        .map(|value| redact_key(&key, value));
    state
        .event_bus
        .publish(hypercolor_types::event::HypercolorEvent::ConfigChanged {
            key: key.clone(),
            old_value,
            new_value: redact_key(&key, effective_value.clone()),
        });

    envelope::ok(mutation_result(
        manager,
        Some(key.clone()),
        Some(redact_key(&key, effective_value)),
        live_applied,
    ))
}

/// Build the shared mutation payload.
///
/// `requires_restart` reports the registry's classification of the
/// mutated key; a whole-config reset spans every section, including the
/// boot-frozen ones, so it always reports true.
fn mutation_result(
    manager: &Arc<hypercolor_core::config::ConfigManager>,
    key: Option<String>,
    value: Option<serde_json::Value>,
    live: bool,
) -> ConfigMutationResponse {
    let requires_restart = key.as_deref().is_none_or(config_registry::requires_restart);
    ConfigMutationResponse {
        key,
        value,
        live,
        requires_restart,
        pending_restart: manager.pending_restart(),
        path: manager.path().display().to_string(),
    }
}

fn canonicalize_config_value(key: &str, value: serde_json::Value) -> serde_json::Value {
    if key == "audio.device" {
        value.as_str().map_or(value.clone(), |device| {
            serde_json::Value::String(canonical_audio_device_id(device))
        })
    } else {
        value
    }
}

// ── Registry dispatch ───────────────────────────────────────────────

/// The live subsystems one mutation has to re-apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LiveSections {
    audio: bool,
    capture: bool,
    input: bool,
    render: bool,
}

impl LiveSections {
    const fn is_empty(self) -> bool {
        !(self.audio || self.capture || self.input || self.render)
    }

    const fn add(&mut self, apply: ApplyPolicy) {
        match apply {
            ApplyPolicy::Live(LiveSection::Audio) => self.audio = true,
            ApplyPolicy::Live(LiveSection::Capture) => self.capture = true,
            ApplyPolicy::Live(LiveSection::Input) => self.input = true,
            ApplyPolicy::Live(LiveSection::Render) => self.render = true,
            ApplyPolicy::LiveOnRead
            | ApplyPolicy::NextScan
            | ApplyPolicy::Restart
            | ApplyPolicy::Inert => {}
        }
    }
}

/// The live subsystems a write at `key` touches, straight from the
/// registry. `None` is a whole-config write, which touches every one.
///
/// The key's own policy comes from its most specific descriptor. Every
/// descriptor nested *below* the key contributes too, because writing a
/// section overwrites the keys inside it: a write to `daemon` carries
/// the render knobs `daemon.target_fps` and friends with it.
fn live_sections_for(key: Option<&str>) -> LiveSections {
    let mut sections = LiveSections::default();
    let Some(key) = key else {
        for descriptor in config_registry::registry() {
            sections.add(descriptor.apply);
        }
        return sections;
    };

    sections.add(config_registry::descriptor_for(key).apply);
    let prefix = format!("{key}.");
    for descriptor in config_registry::registry()
        .iter()
        .filter(|descriptor| descriptor.pattern.root().starts_with(&prefix))
    {
        sections.add(descriptor.apply);
    }
    sections
}

/// Whether a write at `key` covers `target` — the same containment the
/// section dispatch uses, narrowed to one knob an applier retunes.
fn write_covers(key: Option<&str>, target: &str) -> bool {
    key.is_none_or(|key| key == target || target.starts_with(&format!("{key}.")))
}

/// Re-apply the live sections a write touched.
///
/// Capture is absent by design: its applier is a transaction that
/// persists the config itself, so callers run it before the generic
/// save rather than here.
async fn apply_live_sections(
    state: &Arc<AppState>,
    sections: LiveSections,
    key: Option<&str>,
    live_requested: bool,
) -> bool {
    if !live_requested {
        if !sections.is_empty() {
            info!(
                key = key.unwrap_or("<all>"),
                "Persisted config change without live apply; restart the daemon to activate it"
            );
        }
        return false;
    }

    let mut applied = false;
    if sections.audio {
        applied |= apply_audio_config_change(state, key).await;
    }
    if sections.render {
        applied |= apply_render_config_change(state, key).await;
    }
    if sections.input {
        applied |= apply_input_config_change(state, key).await;
    }
    applied
}

// ── Read-surface redaction ──────────────────────────────────────────

/// What a masked value renders as.
fn redacted_marker() -> serde_json::Value {
    serde_json::json!({ "redacted": true })
}

/// Render a whole config document for a read surface.
///
/// Sections the registry classifies `Secret` are masked. A dynamic
/// namespace masks per entry, so the entry names — already public
/// through their own resource routes — survive while every value inside
/// them is hidden.
fn redact_document(mut document: serde_json::Value) -> serde_json::Value {
    let Some(root) = document.as_object_mut() else {
        return document;
    };

    for (key, value) in root.iter_mut() {
        let descriptor = config_registry::descriptor_for(key);
        if matches!(descriptor.redaction, Redaction::Secret) {
            *value = mask_section(descriptor, std::mem::take(value));
        }
    }
    document
}

/// Render one key's value for a read surface.
fn redact_key(key: &str, value: serde_json::Value) -> serde_json::Value {
    let descriptor = config_registry::descriptor_for(key);
    if !matches!(descriptor.redaction, Redaction::Secret) {
        return value;
    }

    if key == descriptor.pattern.root() {
        mask_section(descriptor, value)
    } else {
        redacted_marker()
    }
}

fn mask_section(descriptor: &ConfigKeyDescriptor, value: serde_json::Value) -> serde_json::Value {
    if let KeyPattern::Namespace(_) = descriptor.pattern
        && let Some(entries) = value.as_object()
    {
        return serde_json::Value::Object(
            entries
                .keys()
                .map(|entry| (entry.clone(), redacted_marker()))
                .collect(),
        );
    }
    redacted_marker()
}

/// Restore one key, or the whole config, to defaults.
async fn reset_config_state(
    state: &Arc<AppState>,
    raw_key: Option<&str>,
    live_requested: bool,
) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return internal_config_error("Config manager unavailable in this runtime");
    };

    let current_snapshot = Arc::clone(&manager.get());
    let requested_key = raw_key.map(ToOwned::to_owned);
    let updated: HypercolorConfig = if let Some(key) = requested_key.as_deref() {
        let mut current = match serde_json::to_value(&*current_snapshot) {
            Ok(v) => v,
            Err(e) => return internal_config_error(format!("Failed to serialize config: {e}")),
        };
        let defaults = match serde_json::to_value(HypercolorConfig::default()) {
            Ok(v) => v,
            Err(e) => {
                return internal_config_error(format!("Failed to serialize default config: {e}"));
            }
        };
        let reset_fields: &[&str] = if key == CAPTURE_CALIBRATION_RESET_KEY {
            &CAPTURE_CALIBRATION_FIELDS
        } else {
            std::slice::from_ref(&key)
        };
        for field in reset_fields {
            let Some(default_value) = get_json_path(&defaults, field) else {
                return DomainError::not_found(ResourceKind::ConfigKey, *field).into_response();
            };

            if !set_json_path(&mut current, field, default_value.clone()) {
                return DomainError::validation(format!("Invalid config key path: {field}"))
                    .into_response();
            }
        }

        match serde_json::from_value(current) {
            Ok(cfg) => cfg,
            // The default this rebuilds around is the daemon's, but the
            // document it lands in still holds the neighbors a secret
            // key keeps company with.
            Err(error) => return rejected_value(key, "type validation", &error.to_string()),
        }
    } else {
        full_reset_config(&current_snapshot)
    };
    // A full reset carries the driver entries through untouched, so gating
    // it on their validity would reject the one request a user makes to
    // recover from a config they can no longer edit by hand.
    if let Some(key) = requested_key.as_deref()
        && let Err(rejection) = validate_driver_config_scope(state, Some(key), &updated)
    {
        return rejected_value(&rejection.key, "driver validation", &rejection.detail);
    }
    if let Err(error) = updated.capture.validate() {
        return DomainError::validation(error.to_string()).into_response();
    }

    // The calibration pseudo-key is absent from the registry; any of its
    // four member fields yields the same capture live-section answer.
    let sections_key = if requested_key.as_deref() == Some(CAPTURE_CALIBRATION_RESET_KEY) {
        Some(CAPTURE_CALIBRATION_FIELDS[0])
    } else {
        requested_key.as_deref()
    };
    let sections = live_sections_for(sections_key);
    let apply_capture = sections.capture && live_requested;
    let capture_live_applied = if apply_capture {
        match apply_capture_config_transaction(state, &current_snapshot, updated.capture.clone())
            .await
        {
            Ok(()) => true,
            Err(CaptureConfigTransactionError::Conflict) => {
                return DomainError::conflict(
                    "Capture config changed while its live runtime was prepared; retry the reset",
                )
                .into_response();
            }
            Err(CaptureConfigTransactionError::Prepare(error)) => {
                return DomainError::validation(format!(
                    "Failed to prepare live screen capture config: {error}"
                ))
                .into_response();
            }
            Err(CaptureConfigTransactionError::Persist(error)) => {
                return internal_config_error(format!("Failed to persist config: {error}"));
            }
            Err(CaptureConfigTransactionError::Commit(error)) => {
                return DomainError::conflict(format!(
                    "Screen capture graph changed during live apply: {error}"
                ))
                .into_response();
            }
        }
    } else {
        false
    };

    // A key-scoped capture reset persisted inside the transaction, so
    // the generic re-derive below would rewrite what it just committed.
    if apply_capture && let Some(key) = requested_key.as_deref() {
        let effective_value = serde_json::to_value(&**manager.get())
            .ok()
            .and_then(|root| get_json_path(&root, key).cloned())
            .map(|value| redact_key(key, value));
        return envelope::ok(mutation_result(
            manager,
            Some(key.to_owned()),
            effective_value,
            true,
        ));
    }

    // Both shapes re-derive against the freshest config under the write
    // lock (same race protection as set), so a driver credential write that
    // landed since the snapshot is preserved rather than clobbered.
    let reset_key = requested_key.clone();
    manager.modify(move |config| {
        let Some(key) = reset_key.as_deref() else {
            *config = full_reset_config(config);
            return;
        };
        let reapplied = serde_json::to_value(HypercolorConfig::default())
            .ok()
            .and_then(|defaults| get_json_path(&defaults, key).cloned())
            .and_then(|default_value| {
                let mut root = serde_json::to_value(&*config).ok()?;
                set_json_path(&mut root, key, default_value)
                    .then(|| serde_json::from_value::<HypercolorConfig>(root).ok())
                    .flatten()
            });
        *config = reapplied.unwrap_or(updated);
    });
    if let Err(e) = manager.save() {
        return internal_config_error(format!("Failed to persist config: {e}"));
    }

    let live_applied =
        apply_live_sections(state, sections, requested_key.as_deref(), live_requested).await
            || capture_live_applied;

    // One event per reset; a whole-config reset publishes the empty key so
    // consumers re-read everything rather than diffing per field. It carries
    // no payload because the preserved driver and extension sections hold
    // credentials, and this event fans out to every `events` subscriber.
    let reset_event_key = requested_key.clone().unwrap_or_default();
    let new_value = if reset_event_key.is_empty() {
        None
    } else {
        serde_json::to_value(&**manager.get())
            .ok()
            .and_then(|root| get_json_path(&root, &reset_event_key).cloned())
            .map(|value| redact_key(&reset_event_key, value))
    };
    state
        .event_bus
        .publish(hypercolor_types::event::HypercolorEvent::ConfigChanged {
            key: reset_event_key,
            old_value: None,
            new_value: new_value.clone().unwrap_or(serde_json::Value::Null),
        });

    envelope::ok(mutation_result(
        manager,
        requested_key,
        new_value,
        live_applied,
    ))
}

/// Build the whole-config reset result: defaults, plus the sections the
/// daemon does not author.
///
/// `drivers` entries carry credentials written by driver pairing flows, the
/// flattened `extensions` sections belong to out-of-tree crates that share
/// this file, and `include` names files only the user knows about; none of
/// it is recoverable once a save drops it. The copy has to be explicit
/// because normalization only inserts missing defaults, so it can seed a
/// driver entry but never reconstruct its settings.
fn full_reset_config(current: &HypercolorConfig) -> HypercolorConfig {
    let mut reset = HypercolorConfig {
        include: current.include.clone(),
        drivers: current.drivers.clone(),
        extensions: current.extensions.clone(),
        ..HypercolorConfig::default()
    };
    crate::startup::normalize_daemon_driver_configs(&mut reset);
    reset
}

fn config_snapshot(state: &AppState) -> HypercolorConfig {
    if let Some(manager) = state.config_manager.as_ref() {
        let current = manager.get();
        (**current).clone()
    } else {
        HypercolorConfig::default()
    }
}

/// A driver's own validator rejecting an entry, with the key it names
/// kept apart from the detail so the caller can decide what to render.
struct DriverConfigRejection {
    key: String,
    detail: String,
}

fn validate_driver_config_scope(
    state: &AppState,
    key: Option<&str>,
    config: &HypercolorConfig,
) -> Result<(), DriverConfigRejection> {
    let driver_ids = match key {
        None | Some("drivers") => state.driver_registry.ids(),
        Some(value) => value
            .strip_prefix("drivers.")
            .and_then(|rest| rest.split('.').next())
            .filter(|driver_id| !driver_id.is_empty())
            .map_or_else(Vec::new, |driver_id| vec![driver_id.to_owned()]),
    };

    for driver_id in driver_ids {
        let Some(driver) = state.driver_registry.get(&driver_id) else {
            continue;
        };
        let Some(provider) = driver.config() else {
            continue;
        };
        let entry = config.drivers.get(&driver_id).cloned().unwrap_or_default();
        provider
            .validate_config(&entry)
            .map_err(|error| DriverConfigRejection {
                key: format!("drivers.{driver_id}"),
                detail: error.to_string(),
            })?;
    }

    Ok(())
}

/// Render a rejected value without echoing it back on a secret key.
///
/// Serde and driver validators quote the value they refused, so a
/// secret-classified key would put the submitted credential in the
/// response body and in whatever logs that body. Those keys report the
/// key and the class of failure; plain keys keep the detail that makes
/// the error actionable.
fn rejected_value(key: &str, class: &str, detail: &str) -> Response {
    if config_registry::is_redacted(key) {
        DomainError::validation(format!("Value for '{key}' failed {class}")).into_response()
    } else {
        DomainError::validation(format!("Value for '{key}' failed {class}: {detail}"))
            .into_response()
    }
}

fn get_json_path<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    let mut cursor = value;
    for part in key.split('.') {
        cursor = cursor.get(part)?;
    }
    Some(cursor)
}

fn set_json_path(root: &mut serde_json::Value, key: &str, value: serde_json::Value) -> bool {
    let mut cursor = root;
    let mut parts = key.split('.').peekable();

    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            let Some(obj) = cursor.as_object_mut() else {
                return false;
            };
            obj.insert(part.to_owned(), value);
            return true;
        }

        let Some(obj) = cursor.as_object_mut() else {
            return false;
        };
        cursor = obj
            .entry(part.to_owned())
            .or_insert_with(|| serde_json::json!({}));
    }

    false
}

async fn apply_audio_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    info!(
        key = key.unwrap_or("<all>"),
        "Applying live audio config change"
    );

    match reconfigure_input_manager(state).await {
        Ok(()) => true,
        Err(error) => {
            warn!(
                key = key.unwrap_or("<all>"),
                %error,
                "Failed to apply live audio config; change will take effect after daemon restart"
            );
            false
        }
    }
}

async fn reconfigure_input_manager(state: &Arc<AppState>) -> anyhow::Result<()> {
    let Some(manager) = state.config_manager.as_ref() else {
        return Ok(());
    };

    let mut conflict_count = 0_u64;
    loop {
        let latest_config = Arc::clone(&manager.get());
        let capture_active = current_live_audio_capture_demand(state).await;
        let audio_device = latest_config.audio.device.clone();
        let audio_name = format!("AudioInput({audio_device})");
        let effective_config = audio_pipeline_config(latest_config.as_ref());
        let (plan, previous_sources) = {
            let input_manager = state.input_manager.lock().await;
            (
                input_manager.plan_audio_runtime_config(
                    latest_config.audio.enabled,
                    &effective_config,
                    &audio_name,
                    capture_active,
                )?,
                input_manager.source_names(),
            )
        };
        let replacement_sources = if latest_config.audio.enabled {
            vec![audio_name]
        } else {
            Vec::new()
        };

        info!(
            audio_enabled = latest_config.audio.enabled,
            audio_device = %audio_device,
            capture_active,
            conflict_count,
            previous_sources = ?previous_sources,
            replacement_sources = ?replacement_sources,
            "Applying targeted live audio config change"
        );

        let mut prepared = tokio::task::spawn_blocking(move || plan.prepare())
            .await
            .context("audio reconfiguration preparation task failed")??;
        if !manager.is_current(&latest_config) {
            anyhow::bail!("audio config changed while live reconfiguration was prepared");
        }
        let mut input_manager = state.input_manager.lock().await;
        match input_manager.commit_audio_runtime_config(&mut prepared) {
            Ok(retirement) => {
                let sources = input_manager.source_names();
                drop(input_manager);
                drop(prepared);
                retirement.retire();
                info!(
                    audio_device = %audio_device,
                    conflict_count,
                    sources = ?sources,
                    "Live audio config change applied"
                );
                return Ok(());
            }
            Err(error)
                if error
                    .downcast_ref::<AudioReconfigurationConflict>()
                    .is_some()
                    && manager.is_current(&latest_config) =>
            {
                conflict_count = conflict_count.saturating_add(1);
                drop(input_manager);
                drop(prepared);
            }
            Err(error) => {
                drop(input_manager);
                drop(prepared);
                return Err(error);
            }
        }
    }
}

async fn current_live_audio_capture_demand(state: &Arc<AppState>) -> bool {
    let power_state = *state.power_state.borrow();
    if power_state.sleeping() {
        return false;
    }

    let active_effect_ids = {
        let scene_manager = state.scene_manager.snapshot().await;
        scene_manager
            .active_render_groups()
            .iter()
            .filter(|group| group.enabled)
            .flat_map(hypercolor_types::scene::Zone::effect_ids)
            .collect::<Vec<_>>()
    };
    if active_effect_ids.is_empty() {
        return false;
    }

    state
        .domains
        .effects
        .any_audio_reactive(active_effect_ids)
        .await
}

fn audio_pipeline_config(config: &HypercolorConfig) -> AudioPipelineConfig {
    AudioPipelineConfig {
        source: audio_source_from_device(&config.audio.device, config.audio.enabled),
        fft_size: usize::try_from(config.audio.fft_size).unwrap_or(1024),
        smoothing: config.audio.smoothing.clamp(0.0, 1.0),
        gain: 1.0,
        noise_floor: noise_gate_to_db(config.audio.noise_gate),
        beat_sensitivity: config.audio.beat_sensitivity.max(0.01),
    }
}

fn audio_source_from_device(device: &str, enabled: bool) -> AudioSourceType {
    if !enabled {
        return AudioSourceType::None;
    }

    let normalized = device.trim();
    if normalized.eq_ignore_ascii_case("none") {
        AudioSourceType::None
    } else if normalized.eq_ignore_ascii_case("default") {
        AudioSourceType::SystemMonitor
    } else if normalized.eq_ignore_ascii_case("microphone") {
        AudioSourceType::Microphone
    } else {
        AudioSourceType::Named(normalized.to_owned())
    }
}

fn noise_gate_to_db(noise_gate: f32) -> f32 {
    let linear = noise_gate.clamp(0.000_001, 1.0);
    20.0 * linear.log10()
}

#[derive(Debug, thiserror::Error)]
enum CaptureConfigTransactionError {
    #[error("capture config identity changed during preparation")]
    Conflict,
    #[error(transparent)]
    Prepare(anyhow::Error),
    #[error(transparent)]
    Persist(anyhow::Error),
    #[error(transparent)]
    Commit(ScreenReconfigurationConflict),
}

async fn apply_capture_config_transaction(
    state: &Arc<AppState>,
    expected_config: &Arc<HypercolorConfig>,
    capture: CaptureConfig,
) -> Result<(), CaptureConfigTransactionError> {
    let Some(manager) = state.config_manager.as_ref() else {
        return Err(CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
            "config manager unavailable"
        )));
    };
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let (plan, capacity_plan, capacity_preparation, admission_coordinator) = {
        let input_manager = state.input_manager.lock().await;
        let plan = input_manager.plan_screen_runtime_config(capture.enabled);
        let installed_capacity = input_manager.screen_resource_capacity();
        let capacity_plan = crate::startup::services::screen_capacity_plan_for_backend(
            &capture,
            installed_capacity.backend_capacity(),
        )
        .map_err(CaptureConfigTransactionError::Prepare)?;
        let analysis = crate::startup::services::screen_analysis_plan_for_demand(
            &capture,
            plan.capture_demand(),
            capacity_plan.total_capacity(),
        )
        .map_err(CaptureConfigTransactionError::Prepare)?;
        let capacity_preparation = input_manager
            .prepare_screen_capacity_plan(
                capacity_plan.total_capacity(),
                analysis.map_or(
                    0,
                    hypercolor_core::input::screen::ScreenAnalysisResourcePlan::peak_bytes,
                ),
            )
            .map_err(|error| CaptureConfigTransactionError::Prepare(anyhow::anyhow!(error)))?;
        if plan.enabled() && capacity_preparation.is_none() {
            return Err(CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
                "screen capacity admission is not installed"
            )));
        }
        (
            plan,
            capacity_plan,
            capacity_preparation,
            input_manager.screen_admission_coordinator(),
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let plan = {
        let input_manager = state.input_manager.lock().await;
        input_manager.plan_screen_runtime_config(capture.enabled)
    };
    let (mut replacement, persistence) = if plan.enabled() {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let (mut source, persistence) =
            crate::startup::services::prepare_platform_screen_capture_source(
                &capture,
                Arc::clone(manager),
                expected_config,
                admission_coordinator,
                capacity_plan.total_capacity(),
            )
            .map_err(CaptureConfigTransactionError::Prepare)?;
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        let (mut source, persistence) =
            crate::startup::services::prepare_platform_screen_capture_source(
                &capture,
                Arc::clone(manager),
                expected_config,
            )
            .map_err(CaptureConfigTransactionError::Prepare)?;
        source.set_source_graph_generation(plan.replacement_source_graph_generation());
        source
            .set_screen_capture_demand(plan.capture_demand())
            .map_err(CaptureConfigTransactionError::Prepare)?;
        let source = Some(
            tokio::task::spawn_blocking(move || {
                source.start()?;
                Ok::<_, anyhow::Error>(source)
            })
            .await
            .map_err(|error| {
                CaptureConfigTransactionError::Prepare(anyhow::anyhow!(
                    "capture preparation task failed: {error}"
                ))
            })?
            .map_err(CaptureConfigTransactionError::Prepare)?,
        );
        (source, Some(persistence))
    } else {
        (None, None)
    };
    if plan.capture_demand().is_active()
        && let Some(status) = replacement
            .as_ref()
            .and_then(|source| source.source_status_handle())
        && let Err(error) = validate_prepared_capture_status(status).await
    {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Prepare(error));
    }

    let mut input_manager = state.input_manager.lock().await;
    if let Err(error) = input_manager.validate_screen_runtime_config(&plan, &replacement) {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        drop(input_manager);
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Commit(error));
    }
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    if let Some(capacity_preparation) = &capacity_preparation
        && let Err(error) = input_manager.validate_screen_capacity(capacity_preparation)
    {
        if let Some(persistence) = &persistence {
            persistence.revoke();
        }
        drop(input_manager);
        stop_prepared_capture_source(replacement).await;
        return Err(CaptureConfigTransactionError::Commit(error));
    }
    let persistence_result = if let Some(persistence) = &persistence {
        manager.save_capture_and_activate_if_current(
            expected_config,
            persistence.epoch(),
            persistence.source_identity(),
            capture.clone(),
        )
    } else {
        manager
            .modify_and_save_if_current(expected_config, |config| {
                config.capture.clone_from(&capture);
            })
            .map(|saved| saved.then(|| Arc::clone(&manager.get())))
    };
    match persistence_result {
        Ok(Some(_)) => {}
        Ok(None) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            drop(input_manager);
            stop_prepared_capture_source(replacement).await;
            return Err(CaptureConfigTransactionError::Conflict);
        }
        Err(error) => {
            if let Some(persistence) = &persistence {
                persistence.revoke();
            }
            drop(input_manager);
            stop_prepared_capture_source(replacement).await;
            return Err(CaptureConfigTransactionError::Persist(error));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let retirement = if let Some(capacity_preparation) = capacity_preparation {
        input_manager.commit_screen_capacity_and_runtime_config(
            capacity_preparation,
            &plan,
            &mut replacement,
        )
    } else {
        input_manager.commit_screen_runtime_config(&plan, &mut replacement)
    }
    .expect("screen capacity and runtime were validated under the same input-manager lock");
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let retirement = input_manager
        .commit_screen_runtime_config(&plan, &mut replacement)
        .expect("screen runtime plan was validated under the same input-manager lock");
    if !capture.enabled {
        input_manager.remove_screen_sources();
    }
    manager.mark_capture_runtime_applied(&capture);
    drop(input_manager);

    if let Err(error) = tokio::task::spawn_blocking(move || retirement.retire()).await {
        warn!(%error, "Detached capture source retirement task failed");
    }
    if let Some(persistence) = persistence
        && let Err(error) = tokio::task::spawn_blocking(move || persistence.commit()).await
    {
        warn!(%error, "Capture identity persistence task failed");
    }
    if let Err(error) = state
        .scene_transactions
        .push(SceneTransaction::SetScreenCaptureConfigured(
            capture.enabled,
        ))
    {
        warn!(%error, "Render pipeline stopped before capture state publication");
    }
    info!(
        enabled = capture.enabled,
        "Applied live screen capture config"
    );
    Ok(())
}

/// How long a prepared replacement source may take to become usable.
///
/// Windows rebuilds in-process and settles in tens of milliseconds. A
/// Wayland rebuild is a D-Bus portal round trip plus PipeWire negotiation:
/// a restore-token reconnect settles in one to three seconds, so a 500ms
/// gate rejected every live capture reconfiguration on Linux while the
/// last-good publication stayed correctly retained. The gate still exists
/// and still fails typed when consent is required but never granted.
#[cfg(target_os = "linux")]
const PREPARED_CAPTURE_USABILITY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(6);
#[cfg(not(target_os = "linux"))]
const PREPARED_CAPTURE_USABILITY_DEADLINE: std::time::Duration =
    std::time::Duration::from_millis(500);

async fn validate_prepared_capture_status(
    status: hypercolor_core::input::SourceStatusHandle,
) -> anyhow::Result<()> {
    let mut subscription = status.subscribe();
    let deadline = tokio::time::Instant::now() + PREPARED_CAPTURE_USABILITY_DEADLINE;
    loop {
        let snapshot = subscription.snapshot();
        match snapshot.state {
            SourceState::Live => return Ok(()),
            SourceState::Degraded if snapshot.resource_count > 0 => return Ok(()),
            SourceState::Starting => {}
            _ => {
                anyhow::bail!(
                    "{}",
                    snapshot.issue.as_ref().map_or_else(
                        || format!("capture source is not usable ({:?})", snapshot.state),
                        |issue| issue.message.to_string()
                    )
                );
            }
        }
        match tokio::time::timeout_at(deadline, subscription.changed()).await {
            Ok(Some(_)) => {}
            Ok(None) => anyhow::bail!("capture source status closed before becoming usable"),
            Err(_) => anyhow::bail!(
                "capture source did not become usable within {:?}",
                PREPARED_CAPTURE_USABILITY_DEADLINE
            ),
        }
    }
}

async fn capture_runtime_matches(
    state: &Arc<AppState>,
    expected_config: &Arc<HypercolorConfig>,
) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };
    let input_manager = state.input_manager.lock().await;
    if !manager.is_current(expected_config)
        || !manager.capture_runtime_matches(&expected_config.capture)
    {
        return false;
    }
    let registry = input_manager.source_status_registry();
    let statuses = registry.snapshot().statuses();
    capture_statuses_match(&expected_config.capture, &statuses)
}

fn capture_statuses_match(
    capture: &CaptureConfig,
    statuses: &[Arc<hypercolor_core::input::SourceStatus>],
) -> bool {
    let mut screen = statuses
        .iter()
        .filter(|status| status.kind == SourceKind::Screen && !status.retired);
    let first = screen.next();
    if !capture.enabled {
        return first.is_none();
    }
    let Some(status) = first else {
        return false;
    };
    if screen.next().is_some() || !status.configured || !status.consented {
        return false;
    }
    matches!(status.state, SourceState::Live)
        || matches!(status.state, SourceState::Degraded) && status.resource_count > 0
        || matches!(status.state, SourceState::Stopped) && !status.demanded
}

async fn stop_prepared_capture_source(source: Option<Box<dyn InputSource>>) {
    let Some(mut source) = source else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || source.stop()).await;
}

/// Apply host-input config changes live.
///
/// Enable/disable adds or removes the interaction source on the running
/// input manager. Activation converges on the next frame through the
/// uncached interaction demand reconcile, so a source added while an
/// interactive effect is already running starts capturing immediately.
async fn apply_input_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };

    let input = manager.get().input.clone();
    let route_snapshot = state.interaction_routing.snapshot();
    let route_changed = route_snapshot.daemon_policy != input.daemon_route
        || route_snapshot.preview_policy != input.preview_route;
    if route_changed {
        state.interaction_routing.publish_policies(
            route_snapshot
                .config_generation
                .checked_add(1)
                .expect("interaction route config generation exhausted"),
            input.daemon_route,
            input.preview_route,
        );
    }
    if matches!(key, Some("input.daemon_route" | "input.preview_route")) {
        return route_changed;
    }

    let mut input_manager = state.input_manager.lock().await;
    // Only the host hardware source is consent-gated; the browser injection
    // source is always registered and must survive enable/disable toggles.
    let had_source = input_manager.has_host_capture_source();
    let replacement = crate::startup::services::build_interaction_source(&input);

    // Rebuild on any change so keyboard/mouse toggles apply, not just enable
    // and disable.
    input_manager.remove_host_capture_sources();
    let Some(mut source) = replacement else {
        if had_source {
            info!("Disabled host input capture live");
        }
        return had_source || route_changed;
    };

    if let Err(error) = source.start() {
        warn!(%error, "Failed to start live host input source");
        return had_source || route_changed;
    }
    input_manager.add_source(source);
    info!("Applied live host input capture config");
    true
}

/// Apply render config changes live: FPS retune and canvas resize.
///
/// FPS changes go directly to the `RenderLoop`. Canvas dimension changes
/// are queued as an acknowledged layout transaction and take effect at the
/// next frame boundary without blocking the pipeline.
async fn apply_render_config_change(state: &Arc<AppState>, key: Option<&str>) -> bool {
    let Some(manager) = state.config_manager.as_ref() else {
        return false;
    };

    let config = manager.get();
    let mut applied = false;

    if write_covers(key, "daemon.target_fps") {
        let tier = FpsTier::from_fps(config.daemon.target_fps);
        state.configured_max_fps_tier.set(tier);
        let mut loop_guard = state.render_loop.write().await;
        loop_guard.fps_controller_mut().set_max_tier(tier);
        loop_guard.set_tier(tier);
        info!(
            target_fps = config.daemon.target_fps,
            resolved_tier = %tier,
            "Applied live render FPS change"
        );
        applied = true;
    }

    if write_covers(key, "daemon.canvas_width") || write_covers(key, "daemon.canvas_height") {
        let resize_queued = sync_active_layout_canvas_size(
            state,
            config.daemon.canvas_width,
            config.daemon.canvas_height,
        )
        .await;
        info!(
            canvas_width = config.daemon.canvas_width,
            canvas_height = config.daemon.canvas_height,
            resize_queued,
            "Applied live canvas dimension config"
        );
        applied = true;
    }

    applied
}

async fn sync_active_layout_canvas_size(state: &Arc<AppState>, width: u32, height: u32) -> bool {
    let state = Arc::clone(state);
    match tokio::spawn(sync_active_layout_canvas_size_workflow(
        state, width, height,
    ))
    .await
    {
        Ok(applied) => applied,
        Err(error) => {
            warn!(%error, width, height, "Live canvas dimension workflow failed");
            false
        }
    }
}

async fn sync_active_layout_canvas_size_workflow(
    state: Arc<AppState>,
    width: u32,
    height: u32,
) -> bool {
    match state
        .domains
        .layout
        .resize_active_canvas(width, height)
        .await
    {
        Ok(applied) => applied,
        Err(error) => {
            warn!(%error, width, height, "Rejected live canvas dimension config");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use hypercolor_core::config::ConfigManager;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    use hypercolor_core::input::screen::ScreenAdmissionCapacity;
    use hypercolor_core::input::screen::{PixelExtent, ScreenCaptureDemand};
    use hypercolor_core::input::{
        InputData, InputManager, InputSource, ScreenReconfigurationConflict, SourceIssue,
        SourceKind, SourceState, SourceStatus, SourceStatusHandle, SourceStatusReporter,
    };
    use hypercolor_types::config::InteractionRoutePolicy;

    use super::{
        CaptureConfigTransactionError, ConfigApplyQuery, LiveSections,
        apply_capture_config_transaction, apply_input_config_change, capture_statuses_match,
        live_sections_for, put_config_key, validate_prepared_capture_status, write_covers,
    };
    use crate::app_state::AppState;

    struct TestScreenSource {
        running: bool,
        demand: ScreenCaptureDemand,
        stopped: Arc<AtomicBool>,
    }

    impl TestScreenSource {
        fn new(stopped: Arc<AtomicBool>) -> Self {
            Self {
                running: false,
                demand: ScreenCaptureDemand::Inactive,
                stopped,
            }
        }
    }

    impl InputSource for TestScreenSource {
        fn name(&self) -> &'static str {
            "test_screen"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            self.running = true;
            Ok(())
        }

        fn stop(&mut self) {
            self.running = false;
            self.stopped.store(true, Ordering::Release);
        }

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            self.running
        }

        fn is_screen_source(&self) -> bool {
            true
        }

        fn screen_capture_demand(&self) -> ScreenCaptureDemand {
            self.demand
        }

        fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
            self.demand = demand;
            Ok(())
        }
    }

    fn test_screen_demand() -> ScreenCaptureDemand {
        ScreenCaptureDemand::active(
            PixelExtent::new(640, 480).expect("test screen extent should be non-empty"),
        )
    }

    fn screen_status(state: SourceState, resource_count: usize) -> Arc<SourceStatus> {
        screen_status_handle(state, resource_count).snapshot()
    }

    fn screen_status_handle(state: SourceState, resource_count: usize) -> SourceStatusHandle {
        let mut reporter =
            SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
        reporter.set_source_graph_generation(1);
        if state == SourceState::Stopped {
            return reporter.handle();
        }
        let session = reporter
            .begin_session()
            .expect("test source session begins")
            .expect("manager-bound source creates a session");
        match state {
            SourceState::Starting => {}
            SourceState::Live => {
                session.mark_event_driven_live_without_deadline(resource_count);
            }
            SourceState::Degraded => {
                session.degraded_with_resources(
                    SourceIssue::new("test_degraded", "reduced capture", true),
                    resource_count,
                );
            }
            SourceState::Unavailable => {
                session.unavailable(SourceIssue::new(
                    "test_unavailable",
                    "capture unavailable",
                    true,
                ));
            }
            SourceState::Failed => {
                session.failed(SourceIssue::new("test_failed", "capture failed", false));
            }
            SourceState::Stopped => unreachable!("stopped status returned before session start"),
        }
        reporter.handle()
    }

    fn starting_screen_status() -> SourceStatusHandle {
        let mut reporter =
            SourceStatusReporter::new("test-screen", SourceKind::Screen, "test", true, true, true);
        reporter.set_source_graph_generation(1);
        reporter
            .begin_session()
            .expect("test source session begins")
            .expect("manager-bound source creates a session");
        reporter.handle()
    }

    #[test]
    fn registry_dispatch_routes_one_section_per_live_key() {
        assert_eq!(
            live_sections_for(Some("audio.device")),
            LiveSections {
                audio: true,
                ..LiveSections::default()
            }
        );
        assert_eq!(
            live_sections_for(Some("capture.enabled")),
            LiveSections {
                capture: true,
                ..LiveSections::default()
            }
        );
        assert_eq!(
            live_sections_for(Some("input.enabled")),
            LiveSections {
                input: true,
                ..LiveSections::default()
            }
        );
        assert_eq!(
            live_sections_for(Some("daemon.target_fps")),
            LiveSections {
                render: true,
                ..LiveSections::default()
            }
        );
    }

    #[test]
    fn registry_dispatch_applies_nothing_for_non_live_policies() {
        // Restart, NextScan, LiveOnRead, and Inert keys all persist
        // without a live subsystem to re-apply.
        for key in [
            "daemon.port",
            "discovery.scan_interval_secs",
            "session.sleep_behavior",
            "tui.theme",
            "drivers.wled.known_ips",
        ] {
            assert!(
                live_sections_for(Some(key)).is_empty(),
                "{key} should not dispatch a live section"
            );
        }
    }

    #[test]
    fn writing_a_section_carries_the_live_keys_nested_under_it() {
        // The exact render overrides live under a Restart-classified
        // section, so a whole-section write still retunes the loop.
        let daemon = live_sections_for(Some("daemon"));
        assert!(daemon.render);
        assert!(!daemon.audio);
        assert!(write_covers(Some("daemon"), "daemon.target_fps"));
        assert!(!write_covers(Some("daemon.port"), "daemon.target_fps"));
        assert!(write_covers(None, "daemon.canvas_width"));
    }

    #[test]
    fn a_whole_config_write_touches_every_live_section() {
        let sections = live_sections_for(None);
        assert!(sections.audio);
        assert!(sections.capture);
        assert!(sections.input);
        // The regression this fixes: the old hand predicate matched
        // three exact keys and ignored the whole-config case, so a full
        // reset persisted a new target FPS without ever retuning.
        assert!(sections.render);
    }

    #[test]
    fn read_surfaces_mask_secret_namespaces_and_leave_plain_keys_alone() {
        let document = serde_json::json!({
            "audio": { "device": "default" },
            "drivers": {
                "wled": { "enabled": true, "known_ips": ["192.168.1.50"] },
            },
            "cloud": { "api_key": "secret" },
        });

        let redacted = super::redact_document(document);

        assert_eq!(redacted["audio"]["device"], serde_json::json!("default"));
        assert_eq!(
            redacted["drivers"]["wled"],
            serde_json::json!({ "redacted": true })
        );
        assert_eq!(redacted["cloud"], serde_json::json!({ "redacted": true }));
    }

    #[test]
    fn a_masked_document_still_parses_as_a_config() {
        // Clients type this response as the config struct, so the mask
        // has to keep the document readable rather than break the read
        // surface it protects.
        let mut config = hypercolor_types::config::HypercolorConfig::default();
        config.drivers.insert(
            "wled".to_owned(),
            hypercolor_types::config::DriverConfigEntry::enabled(
                [("known_ips".to_owned(), serde_json::json!(["192.168.1.50"]))]
                    .into_iter()
                    .collect(),
            ),
        );
        config
            .extensions
            .insert("cloud".to_owned(), serde_json::json!({ "token": "secret" }));

        let document =
            super::redact_document(serde_json::to_value(&config).expect("config projects to JSON"));
        let parsed: hypercolor_types::config::HypercolorConfig =
            serde_json::from_value(document).expect("a masked config still deserializes");

        assert!(parsed.drivers.contains_key("wled"));
        assert_eq!(
            parsed.drivers["wled"].settings.get("known_ips"),
            None,
            "the masked entry keeps its name and drops its settings"
        );
        assert_eq!(parsed.daemon.port, config.daemon.port);
    }

    #[test]
    fn key_reads_mask_at_every_depth_of_a_secret_namespace() {
        assert_eq!(
            super::redact_key("drivers.wled.known_ips", serde_json::json!(["10.0.0.1"])),
            serde_json::json!({ "redacted": true })
        );
        assert_eq!(
            super::redact_key("drivers.wled", serde_json::json!({ "enabled": true })),
            serde_json::json!({ "redacted": true })
        );
        assert_eq!(
            super::redact_key(
                "drivers",
                serde_json::json!({ "wled": { "enabled": true }, "hue": {} })
            ),
            serde_json::json!({ "wled": { "redacted": true }, "hue": { "redacted": true } })
        );
        assert_eq!(
            super::redact_key("daemon.port", serde_json::json!(9420)),
            serde_json::json!(9420)
        );
    }

    #[tokio::test]
    async fn route_only_input_config_changes_publish_without_rebuilding_sources() {
        let mut state = AppState::new();
        let config_path = std::env::temp_dir().join(format!(
            "hypercolor-route-config-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos()
        ));
        let manager = Arc::new(
            ConfigManager::new(config_path).expect("test config manager should initialize"),
        );
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        let graph_generation = state.input_manager.lock().await.source_graph_generation();

        manager.modify(|config| config.input.daemon_route = InteractionRoutePolicy::Merge);
        assert!(apply_input_config_change(&state, Some("input.daemon_route")).await);
        let first = state.interaction_routing.snapshot();
        assert_eq!(first.daemon_policy, InteractionRoutePolicy::Merge);
        assert_eq!(first.config_generation, 2);

        manager.modify(|config| config.input.preview_route = InteractionRoutePolicy::Host);
        assert!(apply_input_config_change(&state, Some("input.preview_route")).await);
        let second = state.interaction_routing.snapshot();
        assert_eq!(second.preview_policy, InteractionRoutePolicy::Host);
        assert_eq!(second.config_generation, 3);
        assert_eq!(
            state.input_manager.lock().await.source_graph_generation(),
            graph_generation
        );
    }

    #[tokio::test]
    async fn demanded_starting_capture_times_out_instead_of_committing() {
        let error = validate_prepared_capture_status(starting_screen_status())
            .await
            .expect_err("starting capture must become usable before commit");

        assert!(error.to_string().contains("did not become usable within"));
    }

    #[tokio::test]
    async fn demanded_degraded_capture_commits_only_with_usable_resources() {
        validate_prepared_capture_status(screen_status_handle(SourceState::Degraded, 1))
            .await
            .expect("degraded capture with resources is usable");

        let error =
            validate_prepared_capture_status(screen_status_handle(SourceState::Degraded, 0))
                .await
                .expect_err("degraded capture without resources is unusable");
        assert!(error.to_string().contains("reduced capture"));
    }

    #[test]
    fn capture_runtime_health_rejects_missing_stopped_failed_and_extra_sources() {
        let mut capture = hypercolor_types::config::CaptureConfig {
            enabled: true,
            ..hypercolor_types::config::CaptureConfig::default()
        };
        assert!(!capture_statuses_match(&capture, &[]));
        assert!(!capture_statuses_match(
            &capture,
            &[screen_status(SourceState::Stopped, 0)]
        ));
        assert!(!capture_statuses_match(
            &capture,
            &[screen_status(SourceState::Failed, 0)]
        ));
        assert!(capture_statuses_match(
            &capture,
            &[screen_status(SourceState::Degraded, 1)]
        ));

        capture.enabled = false;
        assert!(!capture_statuses_match(
            &capture,
            &[
                screen_status(SourceState::Stopped, 0),
                screen_status(SourceState::Stopped, 0),
            ]
        ));
    }

    #[test]
    fn capture_runtime_fingerprint_rejects_divergent_config() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = ConfigManager::new(tempdir.path().join("hypercolor.toml"))
            .expect("test config manager should initialize");
        let applied = manager.get().capture.clone();
        manager.mark_capture_runtime_applied(&applied);
        let mut divergent = applied.clone();
        divergent.capture_fps += 1;

        assert!(manager.capture_runtime_matches(&applied));
        assert!(!manager.capture_runtime_matches(&divergent));
    }

    #[test]
    fn screen_runtime_commit_preserves_demand_and_retires_after_swap() {
        let mut manager = InputManager::new();
        manager
            .set_screen_capture_demand(test_screen_demand())
            .expect("screen demand should cache before a source exists");
        let first_plan = manager.plan_screen_runtime_config(true);
        assert_eq!(first_plan.capture_demand(), test_screen_demand());

        let first_stopped = Arc::new(AtomicBool::new(false));
        let mut first = Box::new(TestScreenSource::new(Arc::clone(&first_stopped)));
        first
            .set_screen_capture_demand(first_plan.capture_demand())
            .expect("prepared source should accept demand");
        first.start().expect("prepared source should start");
        let mut first = Some(first as Box<dyn InputSource>);
        manager
            .commit_screen_runtime_config(&first_plan, &mut first)
            .expect("initial prepared source should commit")
            .retire();
        assert!(first.is_none());

        let replacement_plan = manager.plan_screen_runtime_config(true);
        assert_eq!(replacement_plan.capture_demand(), test_screen_demand());
        let replacement_stopped = Arc::new(AtomicBool::new(false));
        let mut replacement = Box::new(TestScreenSource::new(replacement_stopped));
        replacement
            .set_screen_capture_demand(replacement_plan.capture_demand())
            .expect("replacement should accept demand");
        replacement.start().expect("replacement should start");
        let mut replacement = Some(replacement as Box<dyn InputSource>);
        let retirement = manager
            .commit_screen_runtime_config(&replacement_plan, &mut replacement)
            .expect("replacement should commit");

        assert!(!first_stopped.load(Ordering::Acquire));
        retirement.retire();
        assert!(first_stopped.load(Ordering::Acquire));
    }

    #[test]
    fn screen_runtime_commit_rejects_stale_graph_without_consuming_replacement() {
        let mut manager = InputManager::new();
        let plan = manager.plan_screen_runtime_config(true);
        let stopped = Arc::new(AtomicBool::new(false));
        let mut source = Box::new(TestScreenSource::new(Arc::clone(&stopped)));
        source.start().expect("prepared source should start");
        let mut replacement = Some(source as Box<dyn InputSource>);
        manager.add_source(Box::new(hypercolor_core::input::MediaSource::new()));

        assert!(matches!(
            manager.commit_screen_runtime_config(&plan, &mut replacement),
            Err(ScreenReconfigurationConflict::GraphChanged)
        ));
        assert!(replacement.is_some());
        assert!(!stopped.load(Ordering::Acquire));
        replacement
            .as_mut()
            .expect("failed commit preserves replacement ownership")
            .stop();
        assert!(stopped.load(Ordering::Acquire));
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[tokio::test]
    async fn capture_transaction_applies_publication_capacity_with_config() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.enabled = false;
        capture.publication_memory_bytes = Some(30_000);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        state
            .input_manager
            .lock()
            .await
            .set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(40_000, 40_000),
                ScreenAdmissionCapacity::new(30_000, 40_000),
                ScreenAdmissionCapacity::new(20_000, 40_000),
            )
            .expect("empty manager should accept test capacity");

        apply_capture_config_transaction(&state, &expected, capture.clone())
            .await
            .expect("valid publication capacity should apply");

        assert_eq!(manager.get().capture, capture);
        assert!(manager.capture_runtime_matches(&capture));
        let capacity = state
            .input_manager
            .lock()
            .await
            .screen_publication_capacity();
        assert_eq!(capacity.byte_budget(), 30_000);
        assert_eq!(capacity.backend_capacity(), 40_000);
        assert_eq!(
            state.input_manager.lock().await.screen_resource_capacity(),
            ScreenAdmissionCapacity::new(40_000, 40_000)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[tokio::test]
    async fn capture_transaction_conflict_preserves_publication_capacity() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.enabled = false;
        capture.publication_memory_bytes = Some(30_000);
        manager.modify(|config| config.capture.capture_fps += 1);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        state
            .input_manager
            .lock()
            .await
            .set_screen_capacity_plan(
                ScreenAdmissionCapacity::new(40_000, 40_000),
                ScreenAdmissionCapacity::new(20_000, 40_000),
                ScreenAdmissionCapacity::new(20_000, 40_000),
            )
            .expect("empty manager should accept test capacity");

        let result = apply_capture_config_transaction(&state, &expected, capture).await;

        assert!(matches!(
            result,
            Err(CaptureConfigTransactionError::Conflict)
        ));
        let capacity = state
            .input_manager
            .lock()
            .await
            .screen_publication_capacity();
        assert_eq!(capacity, ScreenAdmissionCapacity::new(20_000, 40_000));
        assert_eq!(
            manager.get().capture.capture_fps,
            expected.capture.capture_fps + 1
        );
        assert_eq!(manager.get().capture.publication_memory_bytes, None);
    }

    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn failed_windows_capture_preparation_preserves_old_graph_and_config() {
        let config_path = std::env::temp_dir().join(format!(
            "hypercolor-capture-config-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos()
        ));
        let manager = Arc::new(
            ConfigManager::new(config_path.clone()).expect("test config manager should initialize"),
        );
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        {
            let mut input_manager = state.input_manager.lock().await;
            let mut old = Box::new(TestScreenSource::new(Arc::new(AtomicBool::new(false))));
            old.start().expect("old test source should start");
            input_manager.add_source(old);
            input_manager
                .set_screen_capture_demand(test_screen_demand())
                .expect("old source should accept active demand");
        }
        let graph_generation = state.input_manager.lock().await.source_graph_generation();
        let admission_coordinator = state
            .input_manager
            .lock()
            .await
            .screen_admission_coordinator();
        let reserved_before = admission_coordinator.snapshot().reserved_bytes();
        let expected = Arc::clone(&manager.get());
        let mut capture = expected.capture.clone();
        capture.source = "monitor:hypercolor-test-source-that-does-not-exist".to_owned();

        let result = apply_capture_config_transaction(&state, &expected, capture).await;

        assert!(matches!(
            result,
            Err(CaptureConfigTransactionError::Prepare(_))
        ));
        assert_eq!(manager.get().capture.source, "auto");
        let input_manager = state.input_manager.lock().await;
        assert_eq!(input_manager.source_graph_generation(), graph_generation);
        assert!(input_manager.has_screen_source());
        assert!(
            input_manager
                .source_names()
                .iter()
                .any(|name| name == "test_screen")
        );
        assert_eq!(
            admission_coordinator.snapshot().reserved_bytes(),
            reserved_before
        );
        assert!(!config_path.exists());
    }

    #[tokio::test]
    async fn unchanged_disabled_capture_repairs_stale_runtime_source() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        manager.modify(|config| config.capture.enabled = false);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        let stopped = Arc::new(AtomicBool::new(false));
        {
            let mut input_manager = state.input_manager.lock().await;
            let mut source = Box::new(TestScreenSource::new(Arc::clone(&stopped)));
            source.start().expect("stale source should start");
            input_manager.add_source(source);
            let mut extra = Box::new(TestScreenSource::new(Arc::new(AtomicBool::new(false))));
            extra.start().expect("extra stale source should start");
            input_manager.add_source(extra);
        }

        let response = put_config_key(
            axum::extract::State(Arc::clone(&state)),
            axum::extract::Path("capture.enabled".to_owned()),
            axum::extract::Query(ConfigApplyQuery { live: true }),
            axum::Extension(crate::api::security::RequestAuthContext::control()),
            axum::Json(serde_json::json!(false)),
        )
        .await;

        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("config response body should be readable");
        assert_eq!(
            status,
            axum::http::StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&body)
        );
        assert!(!state.input_manager.lock().await.has_screen_source());
        assert!(stopped.load(Ordering::Acquire));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unchanged_capture_rejects_a_concurrent_config_generation() {
        let tempdir = tempfile::tempdir().expect("temporary config directory should build");
        let manager = Arc::new(
            ConfigManager::new(tempdir.path().join("hypercolor.toml"))
                .expect("test config manager should initialize"),
        );
        manager.modify(|config| config.capture.enabled = false);
        let initial = Arc::clone(&manager.get());
        manager.mark_capture_runtime_applied(&initial.capture);
        let mut state = AppState::new();
        state.config_manager = Some(Arc::clone(&manager));
        let state = Arc::new(state);
        let input_manager = state.input_manager.lock().await;
        let request_state = Arc::clone(&state);
        let unchanged_fps = initial.capture.capture_fps;
        let request = tokio::spawn(async move {
            put_config_key(
                axum::extract::State(request_state),
                axum::extract::Path("capture.capture_fps".to_owned()),
                axum::extract::Query(ConfigApplyQuery { live: true }),
                axum::Extension(crate::api::security::RequestAuthContext::control()),
                axum::Json(serde_json::json!(unchanged_fps)),
            )
            .await
        });

        tokio::task::yield_now().await;
        let mut competing = (*initial).clone();
        competing.capture.capture_fps += 1;
        let competing_capture = competing.capture.clone();
        manager.update(competing);
        manager.mark_capture_runtime_applied(&competing_capture);
        drop(input_manager);

        let response = request.await.expect("unchanged request should complete");
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("config response body should be readable");
        assert_eq!(
            status,
            axum::http::StatusCode::CONFLICT,
            "{}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(manager.get().capture, competing_capture);
        assert!(manager.capture_runtime_matches(&competing_capture));
    }
}
