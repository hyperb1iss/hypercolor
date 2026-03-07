//! Settings-focused API helpers and endpoints.
//!
//! Currently exposes backend support needed by the Settings UI spec,
//! starting with audio input device enumeration.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::response::Response;
use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;
use tracing::warn;

use crate::api::AppState;
use crate::api::envelope::ApiResponse;

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

/// `GET /api/v1/audio/devices` — Enumerate audio input devices for the Settings UI.
pub async fn list_audio_devices(State(state): State<Arc<AppState>>) -> Response {
    let current = current_audio_device_id(&state);
    let devices = audio_device_options(&current);

    ApiResponse::ok(AudioDevicesResponse { devices, current })
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
        |manager| manager.get().audio.device.clone(),
    )
}

fn audio_device_options(current: &str) -> Vec<AudioDeviceInfo> {
    let mut devices = vec![default_audio_device()];

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
        let default_rank = if device.id == "default" { 0 } else { 1 };
        (default_rank, device.name.to_ascii_lowercase())
    });
    devices
}

fn enumerate_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

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

        devices.push(AudioDeviceInfo {
            id: name.clone(),
            name: name.clone(),
            description: name,
        });
    }

    Ok(devices)
}

fn default_audio_device() -> AudioDeviceInfo {
    AudioDeviceInfo {
        id: "default".to_owned(),
        name: "Default".to_owned(),
        description: "System default input or monitor".to_owned(),
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
