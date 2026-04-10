//! Library MCP tools: profiles (and future favorites, presets, playlists).

use serde_json::{Value, json};

use super::{ToolDefinition, ToolError, default_output_schema};
use crate::api::AppState;
use hypercolor_types::event::{ChangeTrigger, HypercolorEvent};

// ── Tool Definitions ──────────────────────────────────────────────────────

pub(super) fn build_set_profile() -> ToolDefinition {
    ToolDefinition {
        name: "set_profile".into(),
        title: "Apply Lighting Profile".into(),
        description: "Activate a saved lighting profile by name or fuzzy query. Profiles capture the complete lighting state: effect, control parameters, device selection, and brightness.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Profile name or description to search for"
                },
                "transition_ms": {
                    "type": "integer",
                    "description": "Crossfade transition duration in milliseconds",
                    "default": 1000,
                    "minimum": 0,
                    "maximum": 10000
                }
            },
            "required": ["query"]
        }),
        output_schema: default_output_schema(),
        read_only: false,
        idempotent: true,
    }
}

// ── Stateless Handlers ────────────────────────────────────────────────────

pub(super) fn handle_set_profile(params: &Value) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

    let _transition_ms = params
        .get("transition_ms")
        .and_then(Value::as_u64)
        .unwrap_or(1000);

    // Would query profile manager with fuzzy matching
    Ok(json!({
        "profile": null,
        "applied": false,
        "message": format!("No profile matching '{query}' found.")
    }))
}

// ── Stateful Handlers ─────────────────────────────────────────────────────

pub(super) async fn handle_set_profile_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("query".into()))?;

    let profiles = state.profiles.read().await;
    let matched = profiles
        .values()
        .find(|profile| {
            profile.name.eq_ignore_ascii_case(query)
                || profile.name.to_lowercase().contains(&query.to_lowercase())
        })
        .cloned();

    let Some(profile) = matched else {
        return Ok(json!({
            "profile": null,
            "applied": false,
            "message": format!("No profile matching '{query}' found.")
        }));
    };

    crate::api::profiles::apply_profile_snapshot(state, &profile)
        .await
        .map_err(ToolError::Internal)?;
    state.event_bus.publish(HypercolorEvent::ProfileLoaded {
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        trigger: ChangeTrigger::Api,
    });

    Ok(json!({
        "profile": {
            "id": profile.id,
            "name": profile.name,
            "description": profile.description,
            "effect_id": profile.effect_id,
            "layout_id": profile.layout_id
        },
        "applied": true
    }))
}
