//! `hyper controls` -- dynamic driver and device control surfaces.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use hypercolor_color::{Rgb, Rgba};
use hypercolor_types::api::controls::{ControlSurfaceListResponse, InvokeControlActionRequest};
use hypercolor_types::api::scene::PatchControlsRequest;
use hypercolor_types::control::{ControlValue, SecretRef};
use hypercolor_types::controls::{
    ApplyControlChangesResponse, ControlAccess, ControlActionResult, ControlActionStatus,
    ControlAvailabilityState, ControlSurfaceDocument, ControlSurfaceScope, ControlValueMap,
    ControlValueType,
};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, urlencoded};

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

    /// Confirm actions that declare confirmation metadata.
    #[arg(long)]
    pub yes: bool,
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

    let response: ControlSurfaceListResponse = client
        .get(&format!("/control-surfaces?{}", query_parts.join("&")))
        .await?;
    render_surface_list(&response, ctx)
}

async fn execute_show(
    args: &ControlShowArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if is_driver_device_surface(&args.target) {
        let response: ControlSurfaceDocument = client
            .get(&format!("/control-surfaces/{}", urlencoded(&args.target)))
            .await?;
        return render_surface(&response, ctx);
    }

    let path = if args.driver || is_bare_driver_surface(&args.target) {
        let driver = args.target.strip_prefix("driver:").unwrap_or(&args.target);
        format!("/drivers/{}/controls", urlencoded(driver))
    } else if args.device || is_bare_device_surface(&args.target) {
        let device = args.target.strip_prefix("device:").unwrap_or(&args.target);
        format!("/devices/{}/controls", urlencoded(device))
    } else {
        bail!("surface target must be driver:<id>, device:<id>, --driver <id>, or --device <id>");
    };

    let response: ControlSurfaceDocument = client.get(&path).await?;
    render_surface(&response, ctx)
}

async fn execute_set(
    args: &ControlSetArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = PatchControlsRequest {
        values: assignments_to_values(&args.values)?,
        clear_bindings: Vec::new(),
    };

    let path = format!("/control-surfaces/{}/values", urlencoded(&args.surface));
    let response: ApplyControlChangesResponse = client.patch(&path, &body).await?;
    render_apply_response(&response, ctx)
}

async fn execute_action(
    args: &ControlActionArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let surface: ControlSurfaceDocument = client
        .get(&format!("/control-surfaces/{}", urlencoded(&args.surface)))
        .await?;
    ensure_action_confirmed(&surface, &args.action, args.yes, ctx)?;

    let body = InvokeControlActionRequest {
        input: assignments_to_map(&args.input)?,
    };
    let path = format!(
        "/control-surfaces/{}/actions/{}",
        urlencoded(&args.surface),
        urlencoded(&args.action)
    );
    let response: ControlActionResult = client.post(&path, &body).await?;
    render_action_response(&response, ctx)
}

pub(crate) fn ensure_action_confirmed(
    surface: &ControlSurfaceDocument,
    action_id: &str,
    yes: bool,
    ctx: &OutputContext,
) -> Result<()> {
    let Some(confirmation) = surface
        .actions
        .iter()
        .find(|action| action.id == action_id)
        .and_then(|action| action.confirmation.as_ref())
    else {
        return Ok(());
    };
    if yes {
        return Ok(());
    }

    ctx.warning(&confirmation.message);
    bail!("Use --yes to confirm action '{action_id}'");
}

pub(crate) fn render_surface_list(
    response: &ControlSurfaceListResponse,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain => {
            for surface in &response.surfaces {
                println!("{}", surface.surface_id);
            }
        }
        OutputFormat::Table => {
            let rows = response
                .surfaces
                .iter()
                .map(|surface| surface_row(surface, ctx))
                .collect::<Vec<_>>();
            ctx.print_table(&["Surface", "Scope", "Fields", "Actions", "Rev"], &rows);
        }
    }
    Ok(())
}

pub(crate) fn render_surface(surface: &ControlSurfaceDocument, ctx: &OutputContext) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(surface)?,
        OutputFormat::Plain => println!("{}", surface.surface_id),
        OutputFormat::Table => {
            let rows = field_rows(surface, ctx);
            ctx.info(&format!(
                "{} {}",
                ctx.painter.name(&surface.surface_id),
                ctx.painter.muted(&format!("rev {}", surface.revision))
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

pub(crate) fn render_apply_response(
    response: &ApplyControlChangesResponse,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!(
                "Applied {} change(s), rejected {}; revision {}",
                response.accepted.len(),
                response.rejected.len(),
                response.revision
            ));
        }
    }
    Ok(())
}

pub(crate) fn render_action_response(
    response: &ControlActionResult,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => ctx.print_json(response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!(
                "{}: {}",
                response.action_id,
                action_status_label(response.status)
            ));
        }
    }
    Ok(())
}

fn surface_row(surface: &ControlSurfaceDocument, ctx: &OutputContext) -> Vec<String> {
    vec![
        ctx.painter.name(&surface.surface_id),
        ctx.painter.muted(scope_label(&surface.scope)),
        ctx.painter.number(&surface.fields.len().to_string()),
        ctx.painter.number(&surface.actions.len().to_string()),
        ctx.painter.number(&surface.revision.to_string()),
    ]
}

fn field_rows(surface: &ControlSurfaceDocument, ctx: &OutputContext) -> Vec<Vec<String>> {
    surface
        .fields
        .iter()
        .map(|field| {
            let value = surface
                .values
                .get(&field.id)
                .map_or_else(|| "-".to_string(), value_summary);
            vec![
                ctx.painter.name(&field.id),
                ctx.painter.muted(value_type_label(&field.value_type)),
                ctx.painter.muted(access_label(field.access)),
                value,
            ]
        })
        .collect()
}

fn action_rows(surface: &ControlSurfaceDocument, ctx: &OutputContext) -> Vec<Vec<String>> {
    surface
        .actions
        .iter()
        .map(|action| {
            let availability = surface
                .action_availability
                .get(&action.id)
                .map_or("unknown", |availability| {
                    availability_label(availability.state)
                });
            vec![
                ctx.painter.name(&action.id),
                ctx.painter.muted(availability),
            ]
        })
        .collect()
}

pub(crate) fn assignments_to_values(
    assignments: &[String],
) -> Result<BTreeMap<String, ControlValue>> {
    let mut values = BTreeMap::new();
    for assignment in assignments {
        let (field_id, value) = parse_assignment(assignment)?;
        value.validate()?;
        if values.insert(field_id.clone(), value).is_some() {
            bail!("duplicate control value assignment: {field_id}");
        }
    }
    Ok(values)
}

pub(crate) fn assignments_to_map(assignments: &[String]) -> Result<ControlValueMap> {
    let mut input = ControlValueMap::new();
    for assignment in assignments {
        let (field_id, value) = parse_assignment(assignment)?;
        input.insert(field_id, value);
    }
    Ok(input)
}

fn parse_assignment(assignment: &str) -> Result<(String, ControlValue)> {
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

fn parse_control_value(raw: &str) -> Result<ControlValue> {
    if raw.eq_ignore_ascii_case("null") {
        return Ok(ControlValue::Null);
    }

    if let Some((kind, value)) = raw.split_once(':') {
        return typed_control_value(kind.trim(), value.trim());
    }

    if let Ok(value) = raw.parse::<bool>() {
        return Ok(ControlValue::Bool(value));
    }
    if let Ok(value) = raw.parse::<i64>() {
        return Ok(ControlValue::Int(value));
    }
    if let Ok(value) = raw.parse::<f64>() {
        return Ok(ControlValue::Float(value));
    }
    Ok(ControlValue::Text(raw.to_owned()))
}

fn typed_control_value(kind: &str, value: &str) -> Result<ControlValue> {
    match kind.replace(['-', '_'], "").to_ascii_lowercase().as_str() {
        "null" => Ok(ControlValue::Null),
        "bool" | "boolean" => Ok(ControlValue::Bool(value.parse::<bool>()?)),
        "int" | "integer" => Ok(ControlValue::Int(value.parse::<i64>()?)),
        "float" | "number" => Ok(ControlValue::Float(value.parse::<f64>()?)),
        "string" | "str" => Ok(ControlValue::Text(value.to_owned())),
        "secret" | "secretref" => Ok(ControlValue::SecretRef(SecretRef::new(value))),
        "ip" | "ipaddress" => Ok(ControlValue::ip(value)?),
        "mac" | "macaddress" => Ok(ControlValue::mac(value)?),
        "duration" | "durationms" => Ok(ControlValue::Duration(std::time::Duration::from_millis(
            value.parse::<u64>()?,
        ))),
        "enum" => Ok(ControlValue::Enum(value.to_owned())),
        "flags" => Ok(ControlValue::Flags(split_list(value))),
        "rgb" | "colorrgb" => {
            let color =
                Rgb::from_hex(value).with_context(|| format!("invalid rgb color: {value}"))?;
            Ok(ControlValue::ColorRgb(color))
        }
        "rgba" | "colorrgba" => {
            let color =
                Rgba::from_hex(value).with_context(|| format!("invalid rgba color: {value}"))?;
            Ok(ControlValue::ColorRgba(color))
        }
        "json" => parse_tagged_control_value(value),
        _ => bail!("unknown control value kind: {kind}"),
    }
}

fn parse_tagged_control_value(value: &str) -> Result<ControlValue> {
    let parsed = serde_json::from_str::<ControlValue>(value)
        .context("invalid canonical control value JSON")?;
    parsed.validate()?;
    Ok(parsed)
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

const fn scope_label(scope: &ControlSurfaceScope) -> &'static str {
    match scope {
        ControlSurfaceScope::Driver { .. } => "driver",
        ControlSurfaceScope::Device { .. } => "device",
    }
}

const fn value_type_label(value_type: &ControlValueType) -> &'static str {
    match value_type {
        ControlValueType::Bool => "bool",
        ControlValueType::Integer { .. } => "integer",
        ControlValueType::Float { .. } => "float",
        ControlValueType::String { .. } => "string",
        ControlValueType::Secret => "secret",
        ControlValueType::ColorRgb => "color_rgb",
        ControlValueType::ColorRgba => "color_rgba",
        ControlValueType::IpAddress => "ip_address",
        ControlValueType::MacAddress => "mac_address",
        ControlValueType::DurationMs { .. } => "duration_ms",
        ControlValueType::Enum { .. } => "enum",
        ControlValueType::Flags { .. } => "flags",
        ControlValueType::List { .. } => "list",
        ControlValueType::Object { .. } => "object",
        ControlValueType::Unknown => "unknown",
    }
}

const fn access_label(access: ControlAccess) -> &'static str {
    match access {
        ControlAccess::ReadOnly => "read_only",
        ControlAccess::ReadWrite => "read_write",
        ControlAccess::WriteOnly => "write_only",
    }
}

const fn availability_label(state: ControlAvailabilityState) -> &'static str {
    match state {
        ControlAvailabilityState::Available => "available",
        ControlAvailabilityState::Disabled => "disabled",
        ControlAvailabilityState::ReadOnly => "read_only",
        ControlAvailabilityState::Unsupported => "unsupported",
        ControlAvailabilityState::Hidden => "hidden",
    }
}

const fn action_status_label(status: ControlActionStatus) -> &'static str {
    match status {
        ControlActionStatus::Accepted => "accepted",
        ControlActionStatus::Running => "running",
        ControlActionStatus::Completed => "completed",
        ControlActionStatus::Failed => "failed",
    }
}

/// One control value as a single table cell.
///
/// Secret references never render their target, and a value this build
/// does not understand says so rather than printing a decoded shape it
/// cannot vouch for.
fn value_summary(value: &ControlValue) -> String {
    match value {
        ControlValue::SecretRef(_) => "configured".to_string(),
        ControlValue::Unknown => "unsupported value".to_string(),
        ControlValue::Null => "-".to_string(),
        ControlValue::Bool(value) => value.to_string(),
        ControlValue::Int(value) => value.to_string(),
        ControlValue::Float(value) => value.to_string(),
        ControlValue::Text(value) | ControlValue::Enum(value) => value.clone(),
        ControlValue::Ip(value) => value.as_str().to_string(),
        ControlValue::Mac(value) => value.as_str().to_string(),
        ControlValue::Duration(value) => value.as_millis().to_string(),
        ControlValue::Flags(values) => values.join(","),
        ControlValue::List(values) => values
            .iter()
            .map(value_summary)
            .collect::<Vec<_>>()
            .join(","),
        ControlValue::ColorRgb(_)
        | ControlValue::ColorRgba(_)
        | ControlValue::ColorLinear(_)
        | ControlValue::Gradient(_)
        | ControlValue::Rect(_)
        | ControlValue::Map(_) => "{...}".to_string(),
    }
}

fn is_bare_driver_surface(target: &str) -> bool {
    target.starts_with("driver:") && !target.contains(":device:")
}

fn is_bare_device_surface(target: &str) -> bool {
    target.starts_with("device:")
}

fn is_driver_device_surface(target: &str) -> bool {
    target
        .strip_prefix("driver:")
        .and_then(|target| target.split_once(":device:"))
        .is_some_and(|(driver_id, device_id)| !driver_id.is_empty() && !device_id.is_empty())
}

#[cfg(test)]
mod tests {
    use hypercolor_types::control::{ControlValue, SecretRef};

    use super::{parse_tagged_control_value, value_summary};

    #[test]
    fn json_kind_requires_the_canonical_tagged_shape() {
        let value = parse_tagged_control_value(r#"{"kind":"enum","value":"e131"}"#)
            .expect("canonical enum should parse");
        assert_eq!(value, ControlValue::Enum("e131".to_owned()));

        let error = parse_tagged_control_value(r#"{"enum":"e131"}"#)
            .expect_err("retired external tag should fail");
        assert!(
            error
                .to_string()
                .contains("invalid canonical control value JSON")
        );
    }

    #[test]
    fn value_summary_hides_secret_refs() {
        let value = ControlValue::SecretRef(SecretRef::new("driver-owned-secret"));

        assert_eq!(value_summary(&value), "configured");
    }

    #[test]
    fn value_summary_marks_unknown_control_values_unsupported() {
        let value: ControlValue = serde_json::from_value(serde_json::json!({
            "kind": "unknown"
        }))
        .expect("the unknown sentinel is part of the canonical algebra");

        assert_eq!(value, ControlValue::Unknown);
        assert_eq!(value_summary(&value), "unsupported value");
    }
}
