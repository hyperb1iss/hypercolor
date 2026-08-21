//! MCP resource definitions — read-only contextual data exposed to AI assistants.
//!
//! Resources use the `hypercolor://` URI scheme. The AI can reference these without
//! making tool calls, giving it ambient context about system state.

use serde_json::{Value, json};

use crate::app_state::AppState;

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
            uri: "hypercolor://scenes".into(),
            name: "Saved Scenes".into(),
            description: "All reusable lighting scenes with their names, descriptions, mutation modes, and activation state.".into(),
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

/// Read a resource by URI using live daemon state.
pub async fn read_resource_with_state(uri: &str, state: &AppState) -> Option<Value> {
    match uri {
        "hypercolor://state" => Some(
            serde_json::to_value(super::payload::build_status_payload(state).await)
                .expect("typed MCP status resources should serialize"),
        ),
        "hypercolor://devices" => Some(
            serde_json::to_value(
                super::payload::build_device_inventory_payload(
                    state,
                    super::payload::DeviceInventoryFilter::default(),
                )
                .await,
            )
            .expect("typed MCP device resources should serialize"),
        ),
        "hypercolor://effects" => Some(read_effects_with_state(state).await),
        "hypercolor://scenes" => Some(read_scenes_with_state(state).await),
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
            | "hypercolor://scenes"
            | "hypercolor://audio"
    )
}

// ── Resource Readers ──────────────────────────────────────────────────────

async fn read_effects_with_state(state: &AppState) -> Value {
    let effects = state
        .domains
        .effects
        .all_metadata()
        .await
        .into_iter()
        .map(|metadata| {
            json!({
                "id": metadata.id.to_string(),
                "name": metadata.name,
                "description": metadata.description,
                "category": format!("{}", metadata.category),
                "tags": metadata.tags
            })
        })
        .collect::<Vec<_>>();

    json!({
        "effects": effects,
        "total": effects.len()
    })
}

async fn read_scenes_with_state(state: &AppState) -> Value {
    let scene_manager = state.scene_manager.snapshot().await;
    let active_scene_id = scene_manager.active_scene_id().copied();
    let payload = scene_manager
        .list()
        .into_iter()
        .filter(|scene| scene.kind != hypercolor_types::scene::SceneKind::Ephemeral)
        .map(|scene| {
            json!({
                "id": scene.id.to_string(),
                "name": scene.name,
                "description": scene.description,
                "enabled": scene.enabled,
                "mutation_mode": scene.mutation_mode,
                "layout_id": scene.layout_id,
                "activation_brightness": scene.activation_brightness,
                "active": Some(scene.id) == active_scene_id
            })
        })
        .collect::<Vec<_>>();

    json!({
        "scenes": payload,
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
