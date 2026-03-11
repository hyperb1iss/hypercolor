//! Settings-focused API helpers and endpoints.
//!
//! Currently exposes backend support needed by the Settings UI spec,
//! starting with audio input device enumeration.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::api::AppState;
use crate::api::envelope::{ApiError, ApiResponse};
use crate::api::persist_runtime_session;
use crate::session::{current_global_brightness, set_global_brightness};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevicesResponse {
    pub devices: Vec<AudioDeviceInfo>,
    pub current: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrightnessSettingsResponse {
    pub brightness: u8,
}

#[derive(Debug, Deserialize)]
pub struct SetBrightnessRequest {
    pub brightness: u8,
}

/// `GET /api/v1/audio/devices` — Enumerate audio input devices for the Settings UI.
pub async fn list_audio_devices(State(state): State<Arc<AppState>>) -> Response {
    let current = current_audio_device_id(&state);
    let devices = audio_device_options(&current);

    ApiResponse::ok(AudioDevicesResponse { devices, current })
}

/// `GET /api/v1/settings/brightness` — Read the configured global brightness.
pub async fn get_brightness(State(state): State<Arc<AppState>>) -> Response {
    ApiResponse::ok(BrightnessSettingsResponse {
        brightness: brightness_percent(current_global_brightness(&state.power_state)),
    })
}

/// `PUT /api/v1/settings/brightness` — Update and persist global brightness.
pub async fn set_brightness(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetBrightnessRequest>,
) -> Response {
    let normalized = f32::from(body.brightness) / 100.0;
    if !(0.0..=1.0).contains(&normalized) {
        return ApiError::validation("brightness must be between 0 and 100");
    }

    {
        let mut settings = state.device_settings.write().await;
        settings.set_global_brightness(normalized);
        if let Err(error) = settings.save() {
            return ApiError::internal(format!("Failed to persist global brightness: {error}"));
        }
    }

    set_global_brightness(&state.power_state, normalized);
    persist_runtime_session(&state).await;

    ApiResponse::ok(BrightnessSettingsResponse {
        brightness: body.brightness,
    })
}

pub(crate) fn audio_input_available() -> bool {
    enumerate_audio_input_devices().is_ok()
}

pub(crate) const fn capture_input_available() -> bool {
    // Hypercolor exposes the screen-processing pipeline, but the host capture
    // backend is not wired in the daemon yet.
    false
}

fn current_audio_device_id(state: &AppState) -> String {
    state.config_manager.as_ref().map_or_else(
        || "default".to_owned(),
        |manager| normalize_audio_device_id(&manager.get().audio.device),
    )
}

fn audio_device_options(current: &str) -> Vec<AudioDeviceInfo> {
    let mut devices = vec![
        default_audio_device(),
        microphone_audio_device(),
        disabled_audio_device(),
    ];

    match enumerate_audio_input_devices() {
        Ok(mut enumerated) => devices.append(&mut enumerated),
        Err(error) => {
            warn!(
                %error,
                "Failed to enumerate audio input devices; returning fallback settings options"
            );
        }
    }

    if should_include_current_device(current, &devices) {
        devices.push(AudioDeviceInfo {
            id: current.to_owned(),
            name: current.to_owned(),
            description: "Configured device (currently unavailable)".to_owned(),
        });
    }

    dedupe_audio_devices(&mut devices);
    devices.sort_by_cached_key(|device| {
        let rank = match device.id.as_str() {
            "default" => 0,
            "microphone" => 1,
            "none" => 2,
            _ => 3,
        };
        (rank, device.name.to_ascii_lowercase())
    });
    devices
}

fn enumerate_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    let mut filtered = Vec::new();

    for device in host
        .input_devices()
        .context("failed to enumerate input devices")?
    {
        let description = match device.description() {
            Ok(description) => description,
            Err(error) => {
                warn!(%error, "Skipping audio device with unreadable description");
                continue;
            }
        };

        let name = description.name().trim().to_owned();
        if name.is_empty() {
            continue;
        }

        if !should_offer_named_audio_device(&name) {
            filtered.push(name);
            continue;
        }

        devices.push(AudioDeviceInfo {
            id: name.clone(),
            name: name.clone(),
            description: name,
        });
    }

    if !filtered.is_empty() {
        debug!(
            filtered = ?filtered,
            "Filtered unsupported or synthetic audio devices from settings input list"
        );
    }
    debug!(
        count = devices.len(),
        "Enumerated named audio capture devices"
    );

    Ok(devices)
}

fn default_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "default".to_owned(),
        name: "System Monitor (Auto)".to_owned(),
        description: "Prefer the active system output monitor source".to_owned(),
    }
}

fn microphone_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "microphone".to_owned(),
        name: "Default Microphone".to_owned(),
        description: "Capture from the default input device".to_owned(),
    }
}

fn disabled_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "none".to_owned(),
        name: "Disabled".to_owned(),
        description: "Send silence to audio-reactive effects".to_owned(),
    }
}

fn should_include_current_device(current: &str, devices: &[AudioDeviceInfo]) -> bool {
    !current.trim().is_empty()
        && !devices
            .iter()
            .any(|device| device.id.eq_ignore_ascii_case(current))
}

fn dedupe_audio_devices(devices: &mut Vec<AudioDeviceInfo>) {
    let mut seen = HashSet::new();
    devices.retain(|device| seen.insert(device.id.to_ascii_lowercase()));
}

#[doc(hidden)]
pub fn should_offer_named_audio_device(name: &str) -> bool {
    let normalized = name.trim();
    !normalized.is_empty()
        && !is_monitorish_device_name(normalized)
        && !is_serverish_device_name(normalized)
}

fn normalize_audio_device_id(device: &str) -> String {
    let trimmed = device.trim();
    if trimmed.eq_ignore_ascii_case("default") || trimmed.eq_ignore_ascii_case("auto") {
        "default".to_owned()
    } else if trimmed.eq_ignore_ascii_case("mic") || trimmed.eq_ignore_ascii_case("microphone") {
        "microphone".to_owned()
    } else if trimmed.eq_ignore_ascii_case("none") {
        "none".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn is_serverish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "sound server",
        "pipewire",
        "pulseaudio",
        "default alsa output",
        "default output",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn is_monitorish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    ["monitor", "loopback", "what u hear", "stereo mix"]
        .iter()
        .any(|needle| normalized.contains(needle))
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
