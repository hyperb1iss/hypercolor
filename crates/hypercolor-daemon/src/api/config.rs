//! Config endpoints — `/api/v1/config*`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use tracing::info;

use hypercolor_core::config::canonical_audio_device_id;
use hypercolor_types::config::HypercolorConfig;
use hypercolor_types::config_registry;

use axum::Extension;

use crate::api::capture::protected_control_rejection;
use crate::api::envelope;
use crate::api::security::RequestAuthContext;
use crate::app_state::AppState;
use crate::domain::{DomainError, ResourceKind};

mod live;
mod redaction;

use live::{
    CaptureConfigTransactionError, apply_capture_config_transaction, apply_live_sections,
    capture_runtime_matches, live_sections_for,
};
use redaction::{redact_document, redact_key};

pub use hypercolor_types::api::config::{
    ConfigApplyQuery, ConfigDocument, ConfigKeyResponse, ConfigMutationResponse,
};

/// Build an internal config failure.
///
/// The chain goes to tracing and the wire sees the canonical generic
/// message, so a serialization fault cannot leak a config path or value.
fn internal_config_error(message: impl Into<String>) -> DomainError {
    DomainError::Internal(anyhow::anyhow!(message.into()))
}

/// `GET /api/v1/config` — the effective config, rendered for reading.
pub async fn show_config(State(state): State<Arc<AppState>>) -> Response {
    let config = config_snapshot(&state);
    let value = match serde_json::to_value(config) {
        Ok(value) => value,
        Err(error) => {
            return internal_config_error(format!("Failed to serialize config: {error}"))
                .into_response();
        }
    };

    let serde_json::Value::Object(values) = redact_document(value) else {
        return internal_config_error("Effective config did not serialize as an object")
            .into_response();
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
        Err(error) => {
            return internal_config_error(format!("Failed to serialize config: {error}"))
                .into_response();
        }
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
        return internal_config_error("Config manager unavailable in this runtime").into_response();
    };

    let current_snapshot = Arc::clone(&manager.get());
    let current = (*current_snapshot).clone();
    let mut root = match serde_json::to_value(&current) {
        Ok(v) => v,
        Err(e) => {
            return internal_config_error(format!("Failed to serialize config: {e}"))
                .into_response();
        }
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
            return rejected_value(&key, "type validation", &error.to_string()).into_response();
        }
    };
    if let Err(rejection) = validate_driver_config_scope(state, Some(&key), &updated) {
        return rejected_value(&rejection.key, "driver validation", &rejection.detail)
            .into_response();
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
                        ))
                        .into_response();
                    }
                };
                let Some(effective_value) = get_json_path(&effective_root, &key).cloned() else {
                    return internal_config_error(format!(
                        "Canonicalized config is missing expected key: {key}"
                    ))
                    .into_response();
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
                return internal_config_error(format!("Failed to persist config: {error}"))
                    .into_response();
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
        return internal_config_error(format!("Failed to persist config: {e}")).into_response();
    }
    let effective_config = manager.get();
    let effective_root = match serde_json::to_value(&**effective_config) {
        Ok(value) => value,
        Err(error) => {
            return internal_config_error(format!(
                "Failed to serialize canonicalized config: {error}"
            ))
            .into_response();
        }
    };
    let Some(effective_value) = get_json_path(&effective_root, &key).cloned() else {
        return internal_config_error(format!(
            "Canonicalized config is missing expected key: {}",
            key
        ))
        .into_response();
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

/// Restore one key, or the whole config, to defaults.
async fn reset_config_state(
    state: &Arc<AppState>,
    raw_key: Option<&str>,
    live_requested: bool,
) -> Response {
    let Some(manager) = state.config_manager.as_ref() else {
        return internal_config_error("Config manager unavailable in this runtime").into_response();
    };

    let current_snapshot = Arc::clone(&manager.get());
    let requested_key = raw_key.map(ToOwned::to_owned);
    let updated: HypercolorConfig = if let Some(key) = requested_key.as_deref() {
        let mut current = match serde_json::to_value(&*current_snapshot) {
            Ok(v) => v,
            Err(e) => {
                return internal_config_error(format!("Failed to serialize config: {e}"))
                    .into_response();
            }
        };
        let defaults = match serde_json::to_value(HypercolorConfig::default()) {
            Ok(v) => v,
            Err(e) => {
                return internal_config_error(format!("Failed to serialize default config: {e}"))
                    .into_response();
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
            Err(error) => {
                return rejected_value(key, "type validation", &error.to_string()).into_response();
            }
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
        return rejected_value(&rejection.key, "driver validation", &rejection.detail)
            .into_response();
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
                return internal_config_error(format!("Failed to persist config: {error}"))
                    .into_response();
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
        return internal_config_error(format!("Failed to persist config: {e}")).into_response();
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

/// Build a rejected-value error without echoing it back on a secret key.
///
/// Serde and driver validators quote the value they refused, so a
/// secret-classified key would put the submitted credential in the
/// response body and in whatever logs that body. Those keys report the
/// key and the class of failure; plain keys keep the detail that makes
/// the error actionable.
fn rejected_value(key: &str, class: &str, detail: &str) -> DomainError {
    if config_registry::is_redacted(key) {
        DomainError::validation(format!("Value for '{key}' failed {class}"))
    } else {
        DomainError::validation(format!("Value for '{key}' failed {class}: {detail}"))
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

#[cfg(test)]
mod tests;
