//! Effect authoring tools: `install_effect` and `get_effect_console`.
//!
//! With `capture_canvas`, these close the livecoding loop over MCP alone:
//! install a page, read what it printed, look at what it drew, repeat.

use serde_json::{Value, json};
use strum::VariantNames;

use super::{ToolDefinition, ToolError, output_schema, serialize_result};
use crate::api::effects::{EffectInstallError, MAX_EFFECT_UPLOAD_BYTES, install_html_effect};
use crate::app_state::AppState;
use crate::domain::effect::console::{
    ConsoleRead, DEFAULT_CONSOLE_LIMIT, MAX_CONSOLE_LIMIT, console_cursor, read_console,
};
use crate::mcp::results::InstalledEffectResult;
use hypercolor_core::effect::console::EffectConsoleJournal;
use hypercolor_types::api::effects::{EffectConsoleLevel, EffectConsoleResponse};

pub(super) fn build_install_effect() -> ToolDefinition {
    ToolDefinition {
        name: "install_effect".into(),
        title: "Install HTML Effect".into(),
        description: "Install or update an HTML effect from its complete page source; the fastest way to livecode an effect. The page needs a <title>, a <canvas>, and a <script>, and declares controls with <meta property=...> tags. Validation problems come back as an invalid-parameter error that lists each one. Reusing a file_name replaces that effect in place and keeps its id, so a layer already running it reloads with the new code on its own; apply a new effect once with set_effect. Then read get_effect_console with after=console_after for errors the new page threw, and capture_canvas to see what it draws.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "html": {
                    "type": "string",
                    "description": "Complete HTML document for the effect",
                    "minLength": 1,
                    "maxLength": MAX_EFFECT_UPLOAD_BYTES
                },
                "file_name": {
                    "type": "string",
                    "description": "Installed file name such as ember.html. Defaults to a slug of the page title. Reuse it to update the same effect in place.",
                    "minLength": 1,
                    "maxLength": 128
                }
            },
            "required": ["html"],
            "additionalProperties": false
        }),
        output_schema: output_schema::<InstalledEffectResult>(),
        read_only: false,
        destructive: true,
        idempotent: true,
    }
}

pub(super) fn build_get_effect_console() -> ToolDefinition {
    ToolDefinition {
        name: "get_effect_console".into(),
        title: "Read Effect Console".into(),
        description: "Read what HTML effect pages printed: console.log output, warnings, and uncaught script errors, oldest first. Locations point at the effect's own source file and line. Pass a previous next_seq (or install_effect's console_after) as after to read only newer output, and level=warn to skip routine logging.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "after": {
                    "type": "integer",
                    "description": "Return only messages recorded after this cursor; 0 reads everything retained",
                    "default": 0,
                    "minimum": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Most messages to return",
                    "default": DEFAULT_CONSOLE_LIMIT,
                    "minimum": 1,
                    "maximum": MAX_CONSOLE_LIMIT
                },
                "effect": {
                    "type": "string",
                    "description": "Effect id or name to narrow to"
                },
                "level": {
                    "type": "string",
                    "enum": EffectConsoleLevel::VARIANTS,
                    "description": "Lowest severity to include",
                    "default": "debug"
                }
            },
            "additionalProperties": false
        }),
        output_schema: output_schema::<EffectConsoleResponse>(),
        read_only: true,
        destructive: false,
        idempotent: true,
    }
}

pub(super) async fn handle_install_effect_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let html = params
        .get("html")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::MissingParam("html".into()))?;
    let file_name = params.get("file_name").and_then(Value::as_str);

    // Taken before the page loads, so the cursor covers everything the new
    // page prints from its first line on.
    let console_after = console_cursor(EffectConsoleJournal::global());
    let installed =
        install_html_effect(state, file_name, html)
            .await
            .map_err(|error| match error {
                EffectInstallError::Invalid(problems) => ToolError::InvalidParam {
                    param: "html".into(),
                    reason: format!("effect failed validation: {}", problems.join("; ")),
                },
                EffectInstallError::TooLarge => ToolError::InvalidParam {
                    param: "html".into(),
                    reason: format!("exceeds the {MAX_EFFECT_UPLOAD_BYTES} byte limit"),
                },
                EffectInstallError::Domain(error) => error.into(),
            })?;

    serialize_result(InstalledEffectResult {
        id: installed.metadata.id.to_string(),
        name: installed.metadata.name,
        path: installed.source_path.display().to_string(),
        controls: installed.metadata.controls.len(),
        presets: installed.metadata.presets.len(),
        replaced_existing: installed.replaced_existing,
        console_after,
    })
}

pub(super) async fn handle_get_effect_console_with_state(
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let level = match params.get("level").and_then(Value::as_str) {
        None => EffectConsoleLevel::default(),
        Some(raw) => raw.parse().map_err(|_| ToolError::InvalidParam {
            param: "level".into(),
            reason: format!("unknown level '{raw}'"),
        })?,
    };
    let read = ConsoleRead {
        after: params.get("after").and_then(Value::as_u64).unwrap_or(0),
        limit: params
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_CONSOLE_LIMIT, |limit| {
                usize::try_from(limit).unwrap_or(usize::MAX)
            }),
        effect: params
            .get("effect")
            .and_then(Value::as_str)
            .map(str::to_owned),
        level,
    };
    serialize_result(
        read_console(&state.domains.effects, EffectConsoleJournal::global(), read).await?,
    )
}
