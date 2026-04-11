//! Overlay catalog endpoints — `/api/v1/overlays/*`.

use axum::response::Response;
use serde::Serialize;
use serde_json::{Value, json};

use crate::api::envelope::ApiResponse;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayCatalogAvailability {
    Available,
    Gated,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayCatalogEntry {
    #[serde(rename = "type")]
    pub overlay_type: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub availability: OverlayCatalogAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gating_reason: Option<&'static str>,
    pub config_schema: Value,
    pub default_config: Value,
}

/// `GET /api/v1/overlays/catalog` — Available overlay types and source schemas.
pub async fn get_overlay_catalog() -> Response {
    ApiResponse::ok(build_overlay_catalog())
}

#[must_use]
pub fn build_overlay_catalog() -> Vec<OverlayCatalogEntry> {
    vec![
        OverlayCatalogEntry {
            overlay_type: "clock",
            title: "Clock",
            description: "Digital or analog clock face with optional date and SVG template support.",
            availability: OverlayCatalogAvailability::Available,
            gating_reason: None,
            config_schema: json!({
                "type": "object",
                "required": ["style", "hour_format", "color"],
                "additionalProperties": false,
                "properties": {
                    "style": {
                        "type": "string",
                        "enum": ["digital", "analog"],
                    },
                    "hour_format": {
                        "type": "string",
                        "enum": ["twelve", "twenty_four"],
                    },
                    "show_seconds": {
                        "type": "boolean",
                        "default": false,
                    },
                    "show_date": {
                        "type": "boolean",
                        "default": false,
                    },
                    "date_format": {
                        "type": ["string", "null"],
                    },
                    "font_family": {
                        "type": ["string", "null"],
                    },
                    "color": {
                        "type": "string",
                    },
                    "secondary_color": {
                        "type": ["string", "null"],
                    },
                    "template": {
                        "type": ["string", "null"],
                    },
                },
            }),
            default_config: json!({
                "style": "digital",
                "hour_format": "twenty_four",
                "show_seconds": false,
                "show_date": false,
                "date_format": null,
                "font_family": null,
                "color": "#80ffea",
                "secondary_color": null,
                "template": null,
            }),
        },
        OverlayCatalogEntry {
            overlay_type: "sensor",
            title: "Sensor",
            description: "Numeric, gauge, bar, or minimal telemetry readout backed by the shared system snapshot.",
            availability: OverlayCatalogAvailability::Available,
            gating_reason: None,
            config_schema: json!({
                "type": "object",
                "required": [
                    "sensor",
                    "style",
                    "range_min",
                    "range_max",
                    "color_min",
                    "color_max"
                ],
                "additionalProperties": false,
                "properties": {
                    "sensor": {
                        "type": "string",
                    },
                    "style": {
                        "type": "string",
                        "enum": ["numeric", "gauge", "bar", "minimal"],
                    },
                    "unit_label": {
                        "type": ["string", "null"],
                    },
                    "range_min": {
                        "type": "number",
                    },
                    "range_max": {
                        "type": "number",
                    },
                    "color_min": {
                        "type": "string",
                    },
                    "color_max": {
                        "type": "string",
                    },
                    "font_family": {
                        "type": ["string", "null"],
                    },
                    "template": {
                        "type": ["string", "null"],
                    },
                },
            }),
            default_config: json!({
                "sensor": "cpu_temp",
                "style": "numeric",
                "unit_label": "\u{00b0}C",
                "range_min": 0.0,
                "range_max": 100.0,
                "color_min": "#80ffea",
                "color_max": "#ff6363",
                "font_family": null,
                "template": null,
            }),
        },
        OverlayCatalogEntry {
            overlay_type: "image",
            title: "Image",
            description: "Static image or animated GIF overlay with fit-mode control and alpha support.",
            availability: OverlayCatalogAvailability::Available,
            gating_reason: None,
            config_schema: json!({
                "type": "object",
                "required": ["path", "fit"],
                "additionalProperties": false,
                "properties": {
                    "path": {
                        "type": "string",
                    },
                    "speed": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "default": 1.0,
                    },
                    "fit": {
                        "type": "string",
                        "enum": ["cover", "contain", "stretch", "original"],
                    },
                },
            }),
            default_config: json!({
                "path": "",
                "speed": 1.0,
                "fit": "contain",
            }),
        },
        OverlayCatalogEntry {
            overlay_type: "text",
            title: "Text",
            description: "Styled text overlay with optional sensor interpolation and marquee scrolling.",
            availability: OverlayCatalogAvailability::Available,
            gating_reason: None,
            config_schema: json!({
                "type": "object",
                "required": ["text", "font_size", "color", "align"],
                "additionalProperties": false,
                "properties": {
                    "text": {
                        "type": "string",
                    },
                    "font_family": {
                        "type": ["string", "null"],
                    },
                    "font_size": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                    },
                    "color": {
                        "type": "string",
                    },
                    "align": {
                        "type": "string",
                        "enum": ["left", "center", "right"],
                    },
                    "scroll": {
                        "type": "boolean",
                        "default": false,
                    },
                    "scroll_speed": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "default": 30.0,
                    },
                },
            }),
            default_config: json!({
                "text": "CPU {sensor:cpu_temp}\u{00b0}C",
                "font_family": null,
                "font_size": 28.0,
                "color": "#ffffff",
                "align": "center",
                "scroll": false,
                "scroll_speed": 30.0,
            }),
        },
        OverlayCatalogEntry {
            overlay_type: "html",
            title: "HTML",
            description: "LightScript-compatible HTML overlay for custom canvas widgets and dashboards.",
            availability: OverlayCatalogAvailability::Gated,
            gating_reason: Some(
                "HTML overlays are gated until Servo supports multi-session rendering alongside HTML effects.",
            ),
            config_schema: json!({
                "type": "object",
                "required": ["path"],
                "additionalProperties": false,
                "properties": {
                    "path": {
                        "type": "string",
                    },
                    "properties": {
                        "type": "object",
                        "default": {},
                    },
                    "render_interval_ms": {
                        "type": "integer",
                        "minimum": 16,
                        "default": 1000,
                    },
                },
            }),
            default_config: json!({
                "path": "",
                "properties": {},
                "render_interval_ms": 1000,
            }),
        },
    ]
}
