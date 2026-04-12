//! Overlay catalog endpoints — `/api/v1/overlays/*`.
//!
//! The catalog publishes every renderer the daemon knows about along with its
//! availability (some renderers are gated behind runtime preconditions like
//! Servo multi-session support), JSON Schema for the source config, and a
//! default payload that UIs can seed forms from.

use serde::Deserialize;
use serde_json::Value;

use super::client;

/// Catalog entry for a single overlay renderer type.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OverlayCatalogEntry {
    /// Stable identifier used in `OverlaySource::type` discriminants.
    pub id: String,
    /// Human-readable label for the catalog modal.
    pub label: String,
    /// Short explanatory copy shown next to the label.
    pub description: String,
    /// Availability: may be unconditional or gated behind daemon state.
    pub availability: OverlayAvailability,
    /// JSON Schema describing the renderer's source configuration.
    pub config_schema: Value,
    /// Default config payload that the UI can use to seed new slots.
    pub default_config: Value,
}

/// Whether an overlay type is usable right now, and if not, why.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OverlayAvailability {
    /// Renderer is ready to accept new slots.
    Available,
    /// Renderer is present but gated off until `reason` resolves.
    Gated { reason: String },
}

impl OverlayAvailability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// `GET /api/v1/overlays/catalog` — list available overlay renderer types.
pub async fn fetch_overlay_catalog() -> Result<Vec<OverlayCatalogEntry>, String> {
    client::fetch_json::<Vec<OverlayCatalogEntry>>("/api/v1/overlays/catalog")
        .await
        .map_err(Into::into)
}
