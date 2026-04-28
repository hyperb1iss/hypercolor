//! MCP resource definitions — read-only contextual data exposed to AI assistants.
//!
//! Resources use the `hypercolor://` URI scheme. The AI can reference these without
//! making tool calls, giving it ambient context about system state.

use serde_json::{Value, json};

use crate::api::AppState;
use crate::api::effects::active_effect_metadata;
use crate::session::current_global_brightness;

/// Definition of a single MCP resource.
#[derive(Debug, Clone)]
pub struct ResourceDefinition {
    /// URI for this resource (e.g., `hypercolor://state`).
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Detailed description.
    pub description: String,
    /// MIME type of the resource content.
    pub mime_type: String,
    /// Priority hint for the AI (0.0–1.0, higher = more important).
    pub priority: f32,
}

/// Build all 5 MCP resource definitions.
pub fn build_resource_definitions() -> Vec<ResourceDefinition> {
    vec![
        ResourceDefinition {
            uri: "hypercolor://state".into(),
            name: "System State".into(),
            description: "Current daemon state including active effect, brightness, connected devices, FPS, and input status. Updates on every state change.".into(),
            mime_type: "application/json".into(),
            priority: 0.9,
        },
        ResourceDefinition {
            uri: "hypercolor://devices".into(),
            name: "Device Inventory".into(),
            description: "Full inventory of all known RGB devices with connection status, driver origin, output backend, LED count, zone configuration, and connection details. Updates when devices connect/disconnect.".into(),
            mime_type: "application/json".into(),
            priority: 0.7,
        },
        ResourceDefinition {
            uri: "hypercolor://effects".into(),
            name: "Effect Catalog".into(),
            description: "Complete catalog of all available lighting effects with names, descriptions, categories, tags, and available control parameters. Updates when plugins add/remove effects.".into(),
            mime_type: "application/json".into(),
            priority: 0.8,
        },
        ResourceDefinition {
            uri: "hypercolor://profiles".into(),
            name: "Saved Profiles".into(),
            description: "All saved lighting profiles with their names, descriptions, associated effects, brightness settings, and device targets.".into(),
            mime_type: "application/json".into(),
            priority: 0.6,
        },
        ResourceDefinition {
            uri: "hypercolor://audio".into(),
            name: "Audio Analysis".into(),
            description: "Real-time audio analysis data: overall level, bass/mid/treble energy, beat detection status, beat confidence, and a compact spectrum summary. Updates at ~10Hz when audio is active.".into(),
            mime_type: "application/json".into(),
            priority: 0.4,
        },
    ]
}

/// Read a resource by URI, returning its JSON content.
///
/// Returns `None` if the URI is not recognized.
pub fn read_resource(uri: &str) -> Option<Value> {
    match uri {
        "hypercolor://state" => Some(read_state()),
        "hypercolor://devices" => Some(read_devices()),
        "hypercolor://effects" => Some(read_effects()),
        "hypercolor://profiles" => Some(read_profiles()),
        "hypercolor://audio" => Some(read_audio()),
        _ => None,
    }
}

/// Read a resource by URI using live daemon state.
pub async fn read_resource_with_state(uri: &str, state: &AppState) -> Option<Value> {
    match uri {
        "hypercolor://state" => Some(read_state_with_state(state).await),
        "hypercolor://devices" => Some(read_devices_with_state(state).await),
        "hypercolor://effects" => Some(read_effects_with_state(state).await),
        "hypercolor://profiles" => Some(read_profiles_with_state(state).await),
        "hypercolor://audio" => Some(read_audio_with_state(state)),
        _ => None,
    }
}

/// Check whether a URI matches a known resource.
pub fn is_valid_resource_uri(uri: &str) -> bool {
    matches!(
        uri,
        "hypercolor://state"
            | "hypercolor://devices"
            | "hypercolor://effects"
            | "hypercolor://profiles"
            | "hypercolor://audio"
    )
}

// ── Resource Readers ──────────────────────────────────────────────────────

fn read_state() -> Value {
    // Would read from DaemonState in a real implementation
    json!({
        "running": true,
        "paused": false,
        "brightness": 100,
        "fps": {
            "target": 60,
            "actual": 0.0
        },
        "effect": null,
        "profile": null,
        "devices": {
            "connected": 0,
            "total": 0,
            "total_leds": 0
        },
        "inputs": {
            "audio": "disabled",
            "screen": "disabled"
        },
        "uptime_seconds": 0,
        "version": env!("CARGO_PKG_VERSION")
    })
}

fn read_devices() -> Value {
    // Would enumerate from device manager
    json!({
        "devices": [],
        "summary": {
            "total": 0,
            "connected": 0,
            "total_leds": 0
        }
    })
}

fn read_effects() -> Value {
    // Would enumerate from effect registry
    json!({
        "effects": [],
        "total": 0
    })
}

fn read_profiles() -> Value {
    // Would enumerate from profile manager
    json!({
        "profiles": [],
        "total": 0
    })
}

fn read_audio() -> Value {
    // Would read from spectrum watch channel
    json!({
        "enabled": false,
        "source": null,
        "sample_rate": null,
        "levels": {
            "overall": 0.0,
            "bass": 0.0,
            "mid": 0.0,
            "treble": 0.0
        },
        "beat": {
            "detected": false,
            "confidence": 0.0,
            "bpm_estimate": null
        },
        "spectrum_summary": null
    })
}

async fn read_state_with_state(state: &AppState) -> Value {
    let render_stats = {
        let render_loop = state.render_loop.read().await;
        render_loop.stats()
    };
    let target_fps = render_stats.tier.fps();
    let actual_fps = super::tools::capped_fps(&render_stats);
    let brightness =
        super::tools::brightness_percent(current_global_brightness(&state.power_state));

    let active_effect = active_effect_metadata(state).await.map(|metadata| {
        json!({
            "id": metadata.id.to_string(),
            "name": metadata.name
        })
    });
    let devices = state.device_registry.list().await;
    let connected = devices
        .iter()
        .filter(|device| device.state.is_renderable())
        .count();
    let total_leds: u64 = devices
        .iter()
        .map(|device| u64::from(device.info.total_led_count()))
        .sum();

    let (audio_status, screen_status) = if let Some(config_manager) = state.config_manager.as_ref()
    {
        let config = config_manager.get();
        (
            if config.audio.enabled {
                "enabled"
            } else {
                "disabled"
            },
            if config.capture.enabled {
                "enabled"
            } else {
                "disabled"
            },
        )
    } else {
        ("unknown", "unknown")
    };

    json!({
        "running": matches!(render_stats.state, hypercolor_core::engine::RenderLoopState::Running),
        "paused": matches!(render_stats.state, hypercolor_core::engine::RenderLoopState::Paused),
        "brightness": brightness,
        "fps": {
            "target": target_fps,
            "actual": actual_fps
        },
        "effect": active_effect,
        "devices": {
            "connected": connected,
            "total": devices.len(),
            "total_leds": total_leds
        },
        "inputs": {
            "audio": audio_status,
            "screen": screen_status
        },
        "uptime_seconds": state.start_time.elapsed().as_secs(),
        "version": env!("CARGO_PKG_VERSION")
    })
}

async fn read_devices_with_state(state: &AppState) -> Value {
    let devices = state.device_registry.list().await;
    let connected = devices
        .iter()
        .filter(|device| device.state.is_renderable())
        .count();
    let total_leds: u64 = devices
        .iter()
        .map(|device| u64::from(device.info.total_led_count()))
        .sum();

    let payload = devices
        .iter()
        .map(|device| {
            super::device_payload::inventory_device_payload(state, &device.info, &device.state)
        })
        .collect::<Vec<_>>();

    json!({
        "devices": payload,
        "summary": {
            "total": payload.len(),
            "connected": connected,
            "total_leds": total_leds
        }
    })
}

async fn read_effects_with_state(state: &AppState) -> Value {
    let effects = {
        let registry = state.effect_registry.read().await;
        registry
            .iter()
            .map(|(_, entry)| {
                json!({
                    "id": entry.metadata.id.to_string(),
                    "name": entry.metadata.name,
                    "description": entry.metadata.description,
                    "category": format!("{}", entry.metadata.category),
                    "tags": entry.metadata.tags
                })
            })
            .collect::<Vec<_>>()
    };

    json!({
        "effects": effects,
        "total": effects.len()
    })
}

async fn read_profiles_with_state(state: &AppState) -> Value {
    let profiles = state.profiles.read().await;
    let payload = profiles
        .values()
        .map(|profile| {
            json!({
                "id": profile.id,
                "name": profile.name,
                "description": profile.description,
                "brightness": profile.brightness,
                "primary": profile.primary,
                "displays": profile.displays,
                "layout_id": profile.layout_id
            })
        })
        .collect::<Vec<_>>();

    json!({
        "profiles": payload,
        "total": payload.len()
    })
}

fn read_audio_with_state(state: &AppState) -> Value {
    let spectrum = state.event_bus.spectrum_receiver().borrow().clone();
    let (enabled, device, sample_rate) = if let Some(config_manager) = state.config_manager.as_ref()
    {
        let config = config_manager.get();
        (
            config.audio.enabled,
            Some(config.audio.device.clone()),
            Some(config.audio.fft_size),
        )
    } else {
        (false, None, None)
    };

    json!({
        "enabled": enabled,
        "source": device,
        "sample_rate": sample_rate,
        "levels": {
            "overall": spectrum.level,
            "bass": spectrum.bass,
            "mid": spectrum.mid,
            "treble": spectrum.treble
        },
        "beat": {
            "detected": spectrum.beat,
            "confidence": spectrum.beat_confidence,
            "bpm_estimate": spectrum.bpm
        },
        "spectrum_summary": {
            "bins": spectrum.bins.len()
        }
    })
}
