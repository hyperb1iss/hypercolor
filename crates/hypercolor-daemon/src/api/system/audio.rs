use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::response::Response;
use cpal::traits::{DeviceTrait, HostTrait};
use hypercolor_core::config::canonical_audio_device_id;
#[cfg(target_os = "linux")]
use hypercolor_core::input::audio::linux;
use hypercolor_types::api::system::{AudioDeviceInfo, AudioDevicesResponse};
use tracing::{debug, warn};

use crate::api::envelope;
use crate::app_state::AppState;

/// `GET /api/v1/system/audio-devices` — Enumerate audio input devices.
pub async fn list_audio_devices(State(state): State<Arc<AppState>>) -> Response {
    let current = current_audio_device_id(&state);
    let devices = audio_device_options(&current);

    envelope::ok(AudioDevicesResponse { devices, current })
}

pub(crate) fn capture_input_available() -> bool {
    if cfg!(any(target_os = "windows", target_os = "macos")) {
        return true;
    }
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn current_audio_device_id(state: &AppState) -> String {
    state.config_manager.as_ref().map_or_else(
        || "default".to_owned(),
        |manager| canonical_audio_device_id(&manager.get().audio.device),
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
    #[cfg(target_os = "linux")]
    if let Ok(devices) = enumerate_linux_audio_input_devices()
        && !devices.is_empty()
    {
        return Ok(devices);
    }

    enumerate_cpal_audio_input_devices()
}

#[cfg(target_os = "linux")]
fn enumerate_linux_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    Ok(linux::enumerate_named_audio_sources()?
        .into_iter()
        .map(|source| AudioDeviceInfo {
            id: source.id,
            name: source.name,
            description: source.description,
        })
        .collect())
}

fn enumerate_cpal_audio_input_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
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
            "Filtered unsupported or synthetic audio devices from the input list"
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
        name: "System Monitor".to_owned(),
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

fn is_serverish_device_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    [
        "sound server",
        "pipewire",
        "pulseaudio",
        "default alsa output",
        "default output",
        "discard all samples",
        "rate converter plugin",
        "plugin for channel",
        "plugin using speex",
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
