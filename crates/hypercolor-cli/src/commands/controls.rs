//! `hyper controls` -- dynamic driver and device control surfaces.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

/// Dynamic control surface inspection and mutation.
#[derive(Debug, Args)]
pub struct ControlsArgs {
    #[command(subcommand)]
    pub command: ControlCommand,
}

/// Control surface subcommands.
#[derive(Debug, Subcommand)]
pub enum ControlCommand {
    /// List control surfaces for a device or driver.
    List(ControlListArgs),
    /// Show one device-level or driver-level control surface.
    Show(ControlShowArgs),
    /// Apply typed field values to a control surface.
    Set(ControlSetArgs),
    /// Invoke a typed control surface action.
    Action(ControlActionArgs),
}

/// Arguments for `controls list`.
#[derive(Debug, Args)]
pub struct ControlListArgs {
    /// Device name or ID whose controls should be listed.
    #[arg(long)]
    pub device: Option<String>,

    /// Driver ID whose controls should be listed.
    #[arg(long)]
    pub driver: Option<String>,

    /// Include the owning driver surface when listing a device.
    #[arg(long)]
    pub include_driver: bool,
}

/// Arguments for `controls show`.
#[derive(Debug, Args)]
pub struct ControlShowArgs {
    /// Surface ID, driver ID, or device ID.
    pub target: String,

    /// Interpret target as a driver ID.
    #[arg(long, conflicts_with = "device")]
    pub driver: bool,

    /// Interpret target as a device ID or name.
    #[arg(long, conflicts_with = "driver")]
    pub device: bool,
}

/// Arguments for `controls set`.
#[derive(Debug, Args)]
pub struct ControlSetArgs {
    /// Control surface ID.
    pub surface: String,

    /// Field assignment, repeatable. Examples: `power=bool:true`, `ip=ip:10.0.0.2`.
    #[arg(long = "value", short = 'v', required = true)]
    pub values: Vec<String>,

    /// Expected surface revision for optimistic concurrency.
    #[arg(long)]
    pub expected_revision: Option<u64>,

    /// Validate the transaction without applying it.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for `controls action`.
#[derive(Debug, Args)]
pub struct ControlActionArgs {
    /// Control surface ID.
    pub surface: String,

    /// Action ID.
    pub action: String,

    /// Action input assignment, repeatable.
    #[arg(long = "input", short = 'i')]
    pub input: Vec<String>,
}

/// Execute the `controls` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable, a selector is invalid, or a
/// typed value cannot be parsed.
pub async fn execute(
    args: &ControlsArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        ControlCommand::List(list_args) => execute_list(list_args, client, ctx).await,
        ControlCommand::Show(show_args) => execute_show(show_args, client, ctx).await,
        ControlCommand::Set(set_args) => execute_set(set_args, client, ctx).await,
        ControlCommand::Action(action_args) => execute_action(action_args, client, ctx).await,
    }
}

async fn execute_list(
    args: &ControlListArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if args.device.is_none() && args.driver.is_none() {
        bail!("controls list requires --device or --driver");
    }

    let mut query_parts = Vec::new();
    if let Some(device) = &args.device {
        query_parts.push(format!("device_id={}", urlencoded(device)));
    }
    if let Some(driver) = &args.driver {
        query_parts.push(format!("driver_id={}", urlencoded(driver)));
    }
    if args.include_driver {
        query_parts.push("include_driver=true".to_string());
    }

    let response = client
        .get(&format!("/control-surfaces?{}", query_parts.join("&")))
        .await?;
    render_surface_list(&response, ctx)
}

async fn execute_show(
    args: &ControlShowArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = if args.driver || is_bare_driver_surface(&args.target) {
        let driver = args.target.strip_prefix("driver:").unwrap_or(&args.target);
        format!("/drivers/{}/controls", urlencoded(driver))
    } else if args.device || is_bare_device_surface(&args.target) {
        let device = args.target.strip_prefix("device:").unwrap_or(&args.target);
        format!("/devices/{}/controls", urlencoded(device))
    } else {
        bail!("surface target must be driver:<id>, device:<id>, --driver <id>, or --device <id>");
    };

    let response = client.get(&path).await?;
    render_surface(&response, ctx)
}

async fn execute_set(
    args: &ControlSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let changes = assignments_to_changes(&args.values)?;
    let mut body = json!({
        "surface_id": args.surface,
        "changes": changes,
        "dry_run": args.dry_run,
    });
    if let Some(revision) = args.expected_revision {
        body["expected_revision"] = json!(revision);
    }

    let path = format!("/control-surfaces/{}/values", urlencoded(&args.surface));
    let response = client.patch(&path, &body).await?;
    render_apply_response(&response, ctx)
}

async fn execute_action(
    args: &ControlActionArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let input = assignments_to_map(&args.input)?;
    let body = json!({ "input": input });
    let path = format!(
        "/control-surfaces/{}/actions/{}",
        urlencoded(&args.surface),
        urlencoded(&args.action)
    );
    let response = client.post(&path, &body).await?;
    render_action_response(&response, ctx)
}

fn render_surface_list(response: &Value, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain => {
            for surface in response
                .get("surfaces")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                println!("{}", extract_str(surface, "surface_id"));
            }
        }
        OutputFormat::Table => {
            let surfaces = response
                .get("surfaces")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let rows = surfaces
                .iter()
                .map(|surface| surface_row(surface, ctx))
                .collect::<Vec<_>>();
            ctx.print_table(&["Surface", "Scope", "Fields", "Actions", "Rev"], &rows);
        }
    }
    Ok(())
}

fn render_surface(surface: &Value, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(surface)?,
        OutputFormat::Plain => println!("{}", extract_str(surface, "surface_id")),
        OutputFormat::Table => {
            let rows = field_rows(surface, ctx);
            ctx.info(&format!(
                "{} {}",
                ctx.painter.name(&extract_str(surface, "surface_id")),
                ctx.painter.muted(&format!("rev {}", revision(surface)))
            ));
            if !rows.is_empty() {
                println!();
                ctx.print_table(&["Field", "Type", "Access", "Value"], &rows);
            }
            let actions = action_rows(surface, ctx);
            if !actions.is_empty() {
                println!();
                ctx.print_table(&["Action", "Availability"], &actions);
            }
        }
    }
    Ok(())
}

fn render_apply_response(response: &Value, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let accepted = response
                .get("accepted")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let rejected = response
                .get("rejected")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            ctx.success(&format!(
                "Applied {accepted} change(s), rejected {rejected}; revision {}",
                revision(response)
            ));
        }
    }
    Ok(())
}

fn render_action_response(response: &Value, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let action = extract_str(response, "action_id");
            let status = extract_str(response, "status");
            ctx.success(&format!("{action}: {status}"));
        }
    }
    Ok(())
}

fn surface_row(surface: &Value, ctx: &OutputContext) -> Vec<String> {
    vec![
        ctx.painter.name(&extract_str(surface, "surface_id")),
        ctx.painter.muted(&scope_label(surface)),
        ctx.painter
            .number(&array_len(surface, "fields").to_string()),
        ctx.painter
            .number(&array_len(surface, "actions").to_string()),
        ctx.painter.number(&revision(surface).to_string()),
    ]
}

fn field_rows(surface: &Value, ctx: &OutputContext) -> Vec<Vec<String>> {
    surface
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|field| {
            let field_id = extract_str(field, "id");
            let value = surface
                .get("values")
                .and_then(|values| values.get(&field_id))
                .map_or_else(|| "-".to_string(), value_summary);
            vec![
                ctx.painter.name(&field_id),
                ctx.painter.muted(&value_type_label(field)),
                ctx.painter.muted(&extract_str(field, "access")),
                value,
            ]
        })
        .collect()
}

fn action_rows(surface: &Value, ctx: &OutputContext) -> Vec<Vec<String>> {
    surface
        .get("actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|action| {
            let action_id = extract_str(action, "id");
            vec![
                ctx.painter.name(&action_id),
                ctx.painter
                    .muted(&action_availability_label(surface, &action_id)),
            ]
        })
        .collect()
}

fn assignments_to_changes(assignments: &[String]) -> Result<Vec<Value>> {
    assignments
        .iter()
        .map(|assignment| {
            let (field_id, value) = parse_assignment(assignment)?;
            Ok(json!({ "field_id": field_id, "value": value }))
        })
        .collect()
}

fn assignments_to_map(assignments: &[String]) -> Result<Map<String, Value>> {
    let mut input = Map::new();
    for assignment in assignments {
        let (field_id, value) = parse_assignment(assignment)?;
        input.insert(field_id, value);
    }
    Ok(input)
}

fn parse_assignment(assignment: &str) -> Result<(String, Value)> {
    let Some((field_id, raw)) = assignment.split_once('=') else {
        bail!("control assignment must be key=value: {assignment}");
    };
    if field_id.trim().is_empty() {
        bail!("control assignment field cannot be empty");
    }
    Ok((
        field_id.trim().to_string(),
        parse_control_value(raw.trim())?,
    ))
}

fn parse_control_value(raw: &str) -> Result<Value> {
    if raw.eq_ignore_ascii_case("null") {
        return Ok(json!({ "kind": "null" }));
    }

    if let Some((kind, value)) = raw.split_once(':') {
        return typed_control_value(kind.trim(), value.trim());
    }

    if let Ok(value) = raw.parse::<bool>() {
        return Ok(json!({ "kind": "bool", "value": value }));
    }
    if let Ok(value) = raw.parse::<i64>() {
        return Ok(json!({ "kind": "integer", "value": value }));
    }
    if let Ok(value) = raw.parse::<f64>() {
        return Ok(json!({ "kind": "float", "value": value }));
    }
    Ok(json!({ "kind": "string", "value": raw }))
}

fn typed_control_value(kind: &str, value: &str) -> Result<Value> {
    match kind.replace(['-', '_'], "").to_ascii_lowercase().as_str() {
        "null" => Ok(json!({ "kind": "null" })),
        "bool" | "boolean" => Ok(json!({ "kind": "bool", "value": value.parse::<bool>()? })),
        "int" | "integer" => Ok(json!({ "kind": "integer", "value": value.parse::<i64>()? })),
        "float" | "number" => Ok(json!({ "kind": "float", "value": value.parse::<f64>()? })),
        "string" | "str" => Ok(json!({ "kind": "string", "value": value })),
        "secret" | "secretref" => Ok(json!({ "kind": "secret_ref", "value": value })),
        "ip" | "ipaddress" => Ok(json!({ "kind": "ip_address", "value": value })),
        "mac" | "macaddress" => Ok(json!({ "kind": "mac_address", "value": value })),
        "duration" | "durationms" => Ok(json!({
            "kind": "duration_ms",
            "value": value.parse::<u64>()?,
        })),
        "enum" => Ok(json!({ "kind": "enum", "value": value })),
        "flags" => Ok(json!({
            "kind": "flags",
            "value": split_list(value),
        })),
        "rgb" | "colorrgb" => Ok(json!({
            "kind": "color_rgb",
            "value": parse_hex_color(value, 3)?,
        })),
        "rgba" | "colorrgba" => Ok(json!({
            "kind": "color_rgba",
            "value": parse_hex_color(value, 4)?,
        })),
        "json" => json_to_control_value(value),
        _ => bail!("unknown control value kind: {kind}"),
    }
}

fn json_to_control_value(value: &str) -> Result<Value> {
    let parsed: Value = serde_json::from_str(value).context("invalid json control value")?;
    match parsed {
        Value::Array(values) => Ok(json!({
            "kind": "list",
            "value": values.into_iter().map(json_value_to_control_value).collect::<Result<Vec<_>>>()?,
        })),
        Value::Object(values) => Ok(json!({
            "kind": "object",
            "value": values
                .into_iter()
                .map(|(key, value)| Ok((key, json_value_to_control_value(value)?)))
                .collect::<Result<BTreeMap<_, _>>>()?,
        })),
        other => json_value_to_control_value(other),
    }
}

fn json_value_to_control_value(value: Value) -> Result<Value> {
    match value {
        Value::Null => Ok(json!({ "kind": "null" })),
        Value::Bool(value) => Ok(json!({ "kind": "bool", "value": value })),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                Ok(json!({ "kind": "integer", "value": integer }))
            } else if let Some(float) = value.as_f64() {
                Ok(json!({ "kind": "float", "value": float }))
            } else {
                bail!("unsupported JSON number: {value}")
            }
        }
        Value::String(value) => Ok(json!({ "kind": "string", "value": value })),
        Value::Array(values) => Ok(json!({
            "kind": "list",
            "value": values.into_iter().map(json_value_to_control_value).collect::<Result<Vec<_>>>()?,
        })),
        Value::Object(values) => Ok(json!({
            "kind": "object",
            "value": values
                .into_iter()
                .map(|(key, value)| Ok((key, json_value_to_control_value(value)?)))
                .collect::<Result<BTreeMap<_, _>>>()?,
        })),
    }
}

fn parse_hex_color(value: &str, channels: usize) -> Result<Vec<u8>> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    let expected_len = channels.saturating_mul(2);
    if hex.len() != expected_len {
        bail!("{channels}-channel color must have {expected_len} hex digits");
    }
    (0..channels)
        .map(|channel| {
            u8::from_str_radix(&hex[channel * 2..channel * 2 + 2], 16)
                .with_context(|| format!("invalid hex color: {value}"))
        })
        .collect()
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn scope_label(surface: &Value) -> String {
    let Some(scope) = surface.get("scope").and_then(Value::as_object) else {
        return "?".to_string();
    };
    if scope.contains_key("driver") {
        "driver".to_string()
    } else if scope.contains_key("device") {
        "device".to_string()
    } else if let Some(kind) = scope.get("kind").and_then(Value::as_str) {
        kind.to_string()
    } else {
        "?".to_string()
    }
}

fn value_type_label(field: &Value) -> String {
    field
        .get("value_type")
        .and_then(Value::as_object)
        .and_then(|value_type| value_type.get("kind").and_then(Value::as_str))
        .unwrap_or("?")
        .to_string()
}

fn action_availability_label(surface: &Value, action_id: &str) -> String {
    surface
        .get("action_availability")
        .and_then(|availability| availability.get(action_id))
        .and_then(Value::as_object)
        .and_then(|availability| availability.get("state").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_string()
}

fn value_summary(value: &Value) -> String {
    match value.get("value") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Array(values)) => values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "{...}".to_string(),
        Some(Value::Null) | None => "-".to_string(),
    }
}

fn array_len(value: &Value, field: &str) -> usize {
    value
        .get(field)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn revision(value: &Value) -> u64 {
    value.get("revision").and_then(Value::as_u64).unwrap_or(0)
}

fn is_bare_driver_surface(target: &str) -> bool {
    target.starts_with("driver:") && !target.contains(":device:")
}

fn is_bare_device_surface(target: &str) -> bool {
    target.starts_with("device:")
}
