//! MCP tool definitions — the daemon tools exposed to AI assistants.
//!
//! Each tool is a `ToolDefinition` with a JSON Schema input spec. Tool execution
//! is handled by `execute_tool_with_state`, which dispatches to the appropriate
//! handler in a per-cluster submodule.

use std::collections::HashMap;
use std::sync::LazyLock;

use jsonschema::error::ValidationErrorKind;
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::app_state::AppState;
use crate::mcp::selector::SelectorError;

mod devices;
mod displays;
mod effects;
mod scenes;
mod system;

/// Definition of a single MCP tool.
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    /// Tool name in `snake_case` per MCP convention.
    pub name: String,
    /// Human-readable title for display in tool lists.
    pub title: String,
    /// Detailed description of what the tool does and how to use it.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: Value,
    /// JSON Schema for the tool's structured result payload.
    pub output_schema: Value,
    /// Whether this tool only reads state (never modifies).
    pub read_only: bool,
    /// Whether this tool may overwrite state a caller cannot recover.
    ///
    /// A tool is destructive when running it discards something the
    /// client did not supply and cannot get back: the running effect's
    /// live control values, a scene's whole layer tree, a display's
    /// assigned face. A reversible value write (brightness, output
    /// power) or a pure creation (a new scene) is additive, not
    /// destructive. Meaningful only when [`Self::read_only`] is false;
    /// read-only tools declare `false`.
    pub destructive: bool,
    /// Whether repeated calls with the same input produce the same result.
    pub idempotent: bool,
}

/// Build all MCP tool definitions.
pub fn build_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        effects::build_set_effect(),
        effects::build_list_effects(),
        system::build_set_output_power(),
        scenes::build_clear_zone(),
        scenes::build_adjust_controls(),
        effects::build_set_color(),
        devices::build_get_devices(),
        devices::build_set_brightness(),
        system::build_get_status(),
        scenes::build_activate_scene(),
        scenes::build_list_scenes(),
        scenes::build_create_scene(),
        system::build_get_audio_state(),
        system::build_get_sensor_data(),
        displays::build_set_display_face(),
        system::build_get_layout(),
        system::build_diagnose(),
    ]
}

struct InputContract {
    schema: Value,
    validator: jsonschema::Validator,
}

static INPUT_CONTRACTS: LazyLock<Result<HashMap<String, InputContract>, String>> =
    LazyLock::new(|| {
        build_tool_definitions()
            .into_iter()
            .map(|tool| {
                jsonschema::validator_for(&tool.input_schema)
                    .map(|validator| {
                        (
                            tool.name.clone(),
                            InputContract {
                                schema: tool.input_schema,
                                validator,
                            },
                        )
                    })
                    .map_err(|error| format!("invalid input schema for {}: {error}", tool.name))
            })
            .collect()
    });

static OUTPUT_CONTRACTS: LazyLock<Result<HashMap<String, jsonschema::Validator>, String>> =
    LazyLock::new(|| {
        build_tool_definitions()
            .into_iter()
            .map(|tool| {
                jsonschema::validator_for(&tool.output_schema)
                    .map(|validator| (tool.name.clone(), validator))
                    .map_err(|error| format!("invalid output schema for {}: {error}", tool.name))
            })
            .collect()
    });

fn validate_params(name: &str, params: &Value) -> Result<Value, ToolError> {
    let contracts = INPUT_CONTRACTS
        .as_ref()
        .map_err(|error| ToolError::Internal(error.clone()))?;
    let Some(contract) = contracts.get(name) else {
        return Ok(params.clone());
    };
    if let Some(param) = undeclared_parameter(&contract.schema, params, "") {
        return Err(ToolError::InvalidParam {
            reason: format!("{name} does not accept a '{param}' argument"),
            param,
        });
    }
    let errors = contract.validator.iter_errors(params).collect::<Vec<_>>();
    if let Some(error) = errors
        .iter()
        .find(|error| {
            matches!(
                error.kind(),
                ValidationErrorKind::AdditionalProperties { .. }
            )
        })
        .or_else(|| errors.first())
    {
        return Err(tool_validation_error(name, error));
    }

    let mut normalized = params.clone();
    normalize_integer_values(&contract.schema, &mut normalized, "")?;
    Ok(normalized)
}

fn tool_validation_error(name: &str, error: &jsonschema::ValidationError<'_>) -> ToolError {
    match error.kind() {
        ValidationErrorKind::Required { property } => {
            let pointer = error.instance_path().to_string();
            let parent = parameter_path(&pointer);
            let property = property.as_str().unwrap_or("arguments");
            let param = parent.map_or_else(
                || property.to_owned(),
                |parent| format!("{parent}.{property}"),
            );
            ToolError::MissingParam(param)
        }
        ValidationErrorKind::AdditionalProperties { unexpected } => {
            let property = unexpected
                .first()
                .cloned()
                .unwrap_or_else(|| "arguments".to_owned());
            let pointer = error.instance_path().to_string();
            let parent = parameter_path(&pointer);
            let param =
                parent.map_or_else(|| property.clone(), |parent| format!("{parent}.{property}"));
            ToolError::InvalidParam {
                reason: format!("{name} does not accept a '{param}' argument"),
                param,
            }
        }
        _ => {
            let pointer = error.instance_path().to_string();
            let param = parameter_path(&pointer).unwrap_or_else(|| "arguments".to_owned());
            ToolError::InvalidParam {
                param,
                reason: error.masked().to_string(),
            }
        }
    }
}

fn parameter_path(pointer: &str) -> Option<String> {
    pointer
        .strip_prefix('/')
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('/', "."))
}

fn undeclared_parameter(schema: &Value, instance: &Value, parameter: &str) -> Option<String> {
    if let Some(object) = instance.as_object() {
        let properties = schema["properties"].as_object();
        if schema["additionalProperties"] == Value::Bool(false) {
            for name in object.keys() {
                if !properties.is_some_and(|properties| properties.contains_key(name)) {
                    return Some(if parameter.is_empty() {
                        name.clone()
                    } else {
                        format!("{parameter}.{name}")
                    });
                }
            }
        }
        if let Some(properties) = properties {
            for (name, child_schema) in properties {
                let Some(child) = object.get(name) else {
                    continue;
                };
                let child_parameter = if parameter.is_empty() {
                    name.clone()
                } else {
                    format!("{parameter}.{name}")
                };
                if let Some(param) = undeclared_parameter(child_schema, child, &child_parameter) {
                    return Some(param);
                }
            }
        }
    }

    if let (Some(item_schema), Some(items)) = (schema.get("items"), instance.as_array()) {
        for (index, item) in items.iter().enumerate() {
            let item_parameter = if parameter.is_empty() {
                index.to_string()
            } else {
                format!("{parameter}.{index}")
            };
            if let Some(param) = undeclared_parameter(item_schema, item, &item_parameter) {
                return Some(param);
            }
        }
    }

    None
}

/// Preserve JSON Schema's mathematical integer semantics for Serde readers.
///
/// Values such as `50.0` satisfy an `integer` schema, but Serde retains their
/// floating representation and `as_u64` rejects them. Normalizing only the
/// schema-approved integer nodes prevents handlers from silently substituting
/// defaults for valid calls.
fn normalize_integer_values(
    schema: &Value,
    instance: &mut Value,
    parameter: &str,
) -> Result<(), ToolError> {
    if schema["type"] == "integer" {
        let Value::Number(number) = instance else {
            return Ok(());
        };
        if number.as_i64().is_some() || number.as_u64().is_some() {
            return Ok(());
        }

        let value = number.as_f64().ok_or_else(|| ToolError::InvalidParam {
            param: parameter.to_owned(),
            reason: "integer cannot be represented by the daemon".into(),
        })?;
        let normalized = if (0.0..18_446_744_073_709_551_616.0).contains(&value) {
            serde_json::Number::from(value as u64)
        } else if (-9_223_372_036_854_775_808.0..0.0).contains(&value) {
            serde_json::Number::from(value as i64)
        } else {
            return Err(ToolError::InvalidParam {
                param: parameter.to_owned(),
                reason: "integer is outside the daemon's supported range".into(),
            });
        };
        *instance = Value::Number(normalized);
        return Ok(());
    }

    if let (Some(properties), Some(instance)) =
        (schema["properties"].as_object(), instance.as_object_mut())
    {
        for (name, child_schema) in properties {
            let Some(child) = instance.get_mut(name) else {
                continue;
            };
            let child_parameter = if parameter.is_empty() {
                name.clone()
            } else {
                format!("{parameter}.{name}")
            };
            normalize_integer_values(child_schema, child, &child_parameter)?;
        }
    }

    if let (Some(item_schema), Some(items)) = (schema.get("items"), instance.as_array_mut()) {
        for (index, item) in items.iter_mut().enumerate() {
            let item_parameter = if parameter.is_empty() {
                index.to_string()
            } else {
                format!("{parameter}.{index}")
            };
            normalize_integer_values(item_schema, item, &item_parameter)?;
        }
    }

    Ok(())
}

pub(super) fn output_schema<T: ToSchema>() -> Value {
    let mut definitions = Vec::new();
    T::schemas(&mut definitions);

    let mut root = serde_json::to_value(T::schema())
        .expect("utoipa output schemas should serialize to JSON values");
    rewrite_schema_refs(&mut root);
    close_typed_objects(&mut root);

    if !definitions.is_empty() {
        let definitions = definitions
            .into_iter()
            .map(|(name, schema)| {
                let mut schema = serde_json::to_value(schema)
                    .expect("utoipa referenced schemas should serialize to JSON values");
                rewrite_schema_refs(&mut schema);
                close_typed_objects(&mut schema);
                (name, schema)
            })
            .collect();
        root.as_object_mut()
            .expect("MCP output schemas must have an object root")
            .insert("$defs".to_owned(), Value::Object(definitions));
    }

    root
}

pub(super) fn serialize_result<T: Serialize>(result: T) -> Result<Value, ToolError> {
    serde_json::to_value(result).map_err(|error| ToolError::Internal(error.to_string()))
}

fn rewrite_schema_refs(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if let Some(reference) = object.get_mut("$ref")
                && let Some(value) = reference.as_str()
                && let Some(name) = value.strip_prefix("#/components/schemas/")
            {
                *reference = Value::String(format!("#/$defs/{name}"));
            }
            for value in object.values_mut() {
                rewrite_schema_refs(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_schema_refs(item);
            }
        }
        _ => {}
    }
}

fn close_typed_objects(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if object.contains_key("properties") && !object.contains_key("additionalProperties") {
                object.insert("additionalProperties".to_owned(), Value::Bool(false));
            }
            for value in object.values_mut() {
                close_typed_objects(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                close_typed_objects(item);
            }
        }
        _ => {}
    }
}

/// Execute a tool with live daemon state access.
pub async fn execute_tool_with_state(
    name: &str,
    params: &Value,
    state: &AppState,
) -> Result<Value, ToolError> {
    let params = validate_params(name, params)?;
    let result = match name {
        "set_effect" => effects::handle_set_effect_with_state(&params, state).await,
        "list_effects" => effects::handle_list_effects_with_state(&params, state).await,
        "set_output_power" => system::handle_set_output_power_with_state(&params, state).await,
        "clear_zone" => scenes::handle_clear_zone_with_state(&params, state).await,
        "adjust_controls" => scenes::handle_adjust_controls_with_state(&params, state).await,
        "set_color" => effects::handle_set_color_with_state(&params, state).await,
        "get_devices" => devices::handle_get_devices_with_state(&params, state).await,
        "set_brightness" => devices::handle_set_brightness_with_state(&params, state).await,
        "get_status" => system::handle_get_status_with_state(state).await,
        "activate_scene" => scenes::handle_activate_scene_with_state(&params, state).await,
        "list_scenes" => scenes::handle_list_scenes_with_state(&params, state).await,
        "create_scene" => scenes::handle_create_scene_with_state(&params, state).await,
        "get_audio_state" => system::handle_get_audio_state_with_state(state),
        "get_sensor_data" => system::handle_get_sensor_data_with_state(&params, state).await,
        "set_display_face" => displays::handle_set_display_face_with_state(&params, state).await,
        "get_layout" => system::handle_get_layout_with_state(state).await,
        "diagnose" => system::handle_diagnose_with_state(&params, state).await,
        _ => Err(ToolError::NotFound(name.to_owned())),
    }?;
    validate_result(name, &result)?;
    Ok(result)
}

fn validate_result(name: &str, result: &Value) -> Result<(), ToolError> {
    let contracts = OUTPUT_CONTRACTS
        .as_ref()
        .map_err(|error| ToolError::Internal(error.clone()))?;
    let Some(contract) = contracts.get(name) else {
        return Ok(());
    };
    if let Some(error) = contract.iter_errors(result).next() {
        tracing::error!(tool = name, %error, "MCP result violated its schema");
        return Err(ToolError::Internal(format!(
            "{name} produced a result outside its declared schema"
        )));
    }
    Ok(())
}

/// Errors that can occur during tool execution.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// Tool name not recognized.
    #[error("tool not found: {0}")]
    NotFound(String),
    /// Required parameter missing.
    #[error("missing required parameter: {0}")]
    MissingParam(String),
    /// Parameter has wrong type or invalid value.
    #[error("invalid parameter '{param}': {reason}")]
    InvalidParam {
        /// Parameter name.
        param: String,
        /// What was wrong with it.
        reason: String,
    },
    /// A human-friendly resource selector did not resolve uniquely.
    #[error("invalid parameter '{param}': {source}")]
    InvalidSelector {
        /// Parameter name.
        param: String,
        /// Structured selector failure.
        source: SelectorError,
    },
    /// Current daemon state rejects the requested mutation.
    #[error("operation conflict: {0}")]
    Conflict(String),
    /// Internal execution error.
    #[error("execution error: {0}")]
    Internal(String),
}

impl ToolError {
    /// JSON-RPC error code for this error type.
    pub const fn error_code(&self) -> i64 {
        match self {
            Self::NotFound(_) => -32601, // Method not found
            Self::MissingParam(_) | Self::InvalidParam { .. } | Self::InvalidSelector { .. } => {
                -32602
            } // Invalid params
            Self::Conflict(_) => -32000, // Server error / state conflict
            Self::Internal(_) => -32603, // Internal error
        }
    }

    /// Build an invalid-parameter error from the shared selector policy.
    pub fn selector(param: impl Into<String>, source: SelectorError) -> Self {
        Self::InvalidSelector {
            param: param.into(),
            source,
        }
    }

    /// Structured details rendered alongside the MCP error code and message.
    #[must_use]
    pub fn details(&self) -> Option<Value> {
        match self {
            Self::MissingParam(parameter) => Some(json!({ "parameter": parameter })),
            Self::InvalidParam { param, .. } => Some(json!({ "parameter": param })),
            Self::InvalidSelector { param, source } => Some(json!({
                "kind": source.kind(),
                "parameter": param,
                "query": source.query(),
                "candidates": source.candidates(),
            })),
            Self::NotFound(_) | Self::Conflict(_) | Self::Internal(_) => None,
        }
    }
}

pub(super) async fn find_effect_metadata(
    state: &AppState,
    primary_name: &str,
    fallback_name: &str,
) -> Option<hypercolor_types::effect::EffectMetadata> {
    state
        .domains
        .effects
        .all_metadata()
        .await
        .into_iter()
        .find(|metadata| {
            metadata.name.eq_ignore_ascii_case(primary_name)
                || metadata.name.eq_ignore_ascii_case(fallback_name)
        })
}

pub(super) async fn resolve_effect_selector(
    state: &AppState,
    parameter: &str,
    query: &str,
) -> Result<hypercolor_types::effect::EffectMetadata, ToolError> {
    let candidates = state
        .domains
        .effects
        .all_metadata()
        .await
        .into_iter()
        .map(|metadata| {
            crate::mcp::selector::SelectorCandidate::named(
                metadata.id.to_string(),
                metadata.name.clone(),
                metadata,
            )
        })
        .collect();
    crate::mcp::selector::resolve(query, candidates)
        .map_err(|error| ToolError::selector(parameter, error))
}

/// Convert a 0.0–1.0 brightness float to a 0–100 percentage. The
/// output service owns the conversion; MCP re-exports it so the two
/// surfaces cannot drift.
pub(crate) use crate::domain::output::brightness_percent;

/// Compute theoretical render capacity, capped at the target tier rate.
///
/// The EWMA frame time measures render *work* only (excluding the sleep between
/// frames), so `1/avg_frame_time` gives theoretical throughput. The real delivery
/// rate is bounded by the FPS tier.
pub(crate) fn render_capacity_fps(stats: &hypercolor_core::engine::RenderLoopStats) -> f32 {
    let avg_secs = stats.avg_frame_time.as_secs_f32();
    if avg_secs <= 0.0 {
        return 0.0;
    }
    let throughput = 1.0 / avg_secs;
    #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
    let target = stats.tier.fps() as f32;
    throughput.min(target)
}
