//! MCP resource definitions — read-only contextual data exposed to AI assistants.
//!
//! Resources use the `hypercolor://` URI scheme. The AI can reference these without
//! making tool calls, giving it ambient context about system state.

use serde_json::{Value, json};

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
            description: "Full inventory of all known RGB devices with connection status, backend type, LED count, zone configuration, and connection details. Updates when devices connect/disconnect.".into(),
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
