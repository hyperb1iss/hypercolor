//! `hyper effects` -- effect browsing, activation, and control.

use anyhow::Result;
use clap::{Args, Subcommand};
use std::collections::BTreeMap;

use hypercolor_types::api::effects::EffectDetailResponse;
use hypercolor_types::api::output::{OutputPatchRequest, OutputPowerMode};
use hypercolor_types::api::scene::{
    ApplyEffectRequest, ClearSceneRequest, PatchControlsRequest, ReplaceLayerRequest,
};
use hypercolor_types::control::ControlValue as ApiControlValue;
use hypercolor_types::effect::ControlValue;
use hypercolor_types::layer::{LayerSource, SceneLayer};
use hypercolor_types::scene::{ZoneId, ZoneRole};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

/// Effect browsing and control.
#[derive(Debug, Args)]
pub struct EffectsArgs {
    #[command(subcommand)]
    pub command: EffectCommand,
}

/// Effect subcommands.
#[derive(Debug, Subcommand)]
pub enum EffectCommand {
    /// List available lighting effects.
    List(EffectListArgs),
    /// Activate a lighting effect by name.
    Activate(EffectActivateArgs),
    /// Stop the currently running effect.
    Stop,
    /// Pause all output without discarding the active effect state.
    Pause,
    /// Resume output from the preserved effect state.
    Resume,
    /// Show detailed information about an effect.
    Info(EffectInfoArgs),
    /// Patch controls on the currently running effect (without re-applying).
    Patch(EffectPatchArgs),
    /// Reset controls on the currently running effect to defaults.
    Reset,
    /// Rescan the effect library for new or changed effects.
    Rescan,
}

/// Arguments for `effects list`.
#[derive(Debug, Args)]
pub struct EffectListArgs {
    /// Filter by rendering source (native, html, shader).
    #[arg(long)]
    pub source: Option<String>,

    /// Filter to audio-reactive effects only.
    #[arg(long)]
    pub audio: bool,

    /// Search effects by name or description.
    #[arg(long)]
    pub search: Option<String>,

    /// Filter by category.
    #[arg(long)]
    pub category: Option<String>,
}

/// Arguments for `effects activate`.
#[derive(Debug, Args)]
pub struct EffectActivateArgs {
    /// Effect name or slug (fuzzy-matched).
    pub effect: String,

    /// Set arbitrary control parameters (repeatable, format: key=value).
    #[arg(long, short, value_parser = parse_key_value)]
    pub param: Vec<(String, String)>,

    /// Speed control shorthand (0-100).
    #[arg(long)]
    pub speed: Option<u32>,

    /// Intensity control shorthand (0-100).
    #[arg(long)]
    pub intensity: Option<u32>,
}

/// Arguments for `effects info`.
#[derive(Debug, Args)]
pub struct EffectInfoArgs {
    /// Effect name or ID.
    pub effect: String,
}

/// Arguments for `effects patch`.
#[derive(Debug, Args)]
pub struct EffectPatchArgs {
    /// Control parameters to update (repeatable, format: key=value).
    #[arg(long, short, value_parser = parse_key_value, required = true)]
    pub param: Vec<(String, String)>,
}

/// Parse a `key=value` string.
fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no '=' found in '{s}'"))?;
    Ok((s[..pos].to_string(), s[pos + 1..].to_string()))
}

fn parse_control_value(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()))
}

/// Execute the `effects` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the requested effect
/// is not found.
pub async fn execute(args: &EffectsArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        EffectCommand::List(list_args) => execute_list(list_args, client, ctx).await,
        EffectCommand::Activate(activate_args) => {
            execute_activate(activate_args, client, ctx).await
        }
        EffectCommand::Stop => execute_stop(client, ctx).await,
        EffectCommand::Pause => execute_output_power(client, ctx, OutputPowerMode::Paused).await,
        EffectCommand::Resume => execute_output_power(client, ctx, OutputPowerMode::Running).await,
        EffectCommand::Info(info_args) => execute_info(info_args, client, ctx).await,
        EffectCommand::Patch(patch_args) => execute_patch(patch_args, client, ctx).await,
        EffectCommand::Reset => execute_reset(client, ctx).await,
        EffectCommand::Rescan => execute_rescan(client, ctx).await,
    }
}

async fn execute_list(
    args: &EffectListArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut path = "/effects".to_string();
    let mut query_parts = Vec::new();

    if let Some(source) = &args.source {
        query_parts.push(format!("source={}", urlencoded(source)));
    }
    if args.audio {
        query_parts.push("audio_reactive=true".to_string());
    }
    if let Some(search) = &args.search {
        query_parts.push(format!("q={}", urlencoded(search)));
    }
    if let Some(category) = &args.category {
        query_parts.push(format!("category={}", urlencoded(category)));
    }
    if !query_parts.is_empty() {
        path = format!("{path}?{}", query_parts.join("&"));
    }

    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(effects) = response.get("items").and_then(serde_json::Value::as_array) {
                for effect in effects {
                    if let Some(name) = effect.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(effects) = response.get("items").and_then(serde_json::Value::as_array) {
                let headers = ["Effect", "Category", "Author", "Version"];
                let rows: Vec<Vec<String>> = effects
                    .iter()
                    .map(|e| {
                        vec![
                            ctx.painter.name(&extract_str(e, "name")),
                            ctx.painter.muted(&extract_str(e, "category")),
                            ctx.painter.muted(&extract_str(e, "author")),
                            ctx.painter.number(&extract_str(e, "version")),
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                ctx.info(&format!(
                    "{} effects",
                    ctx.painter.number(&effects.len().to_string())
                ));
            }
        }
    }

    Ok(())
}

async fn execute_activate(
    args: &EffectActivateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut controls = BTreeMap::new();
    for (key, value) in &args.param {
        controls.insert(
            key.clone(),
            control_value_from_json(parse_control_value(value))?,
        );
    }
    if let Some(speed) = args.speed {
        controls.insert(
            "speed".to_string(),
            ControlValue::Integer(i32::try_from(speed)?),
        );
    }
    if let Some(intensity) = args.intensity {
        controls.insert(
            "intensity".to_string(),
            ControlValue::Integer(i32::try_from(intensity)?),
        );
    }

    let controls = controls
        .into_iter()
        .map(|(name, value)| ApiControlValue::try_from(value).map(|value| (name, value)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    let body = ApplyEffectRequest {
        controls: (!controls.is_empty()).then_some(controls),
        ..ApplyEffectRequest::default()
    };

    // The daemon's apply endpoint uses effect IDs in the path.
    // URL-encode the effect name/slug for path-based lookup.
    let path = format!("/effects/{}/apply", urlencoded(&args.effect));
    let response = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Effect set: {}", args.effect));
        }
    }

    Ok(())
}

async fn execute_stop(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client
        .post("/scene/clear", &ClearSceneRequest::default())
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success("Effect stopped");
        }
    }

    Ok(())
}

async fn execute_output_power(
    client: &DaemonClient,
    ctx: &OutputContext,
    state: OutputPowerMode,
) -> Result<()> {
    let response = client
        .patch(
            "/output",
            &OutputPatchRequest {
                power: Some(state),
                brightness: None,
            },
        )
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(match state {
                OutputPowerMode::Paused => "Output paused",
                OutputPowerMode::Running => "Output resumed",
            });
        }
    }

    Ok(())
}

async fn execute_info(
    args: &EffectInfoArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/effects/{}", urlencoded(&args.effect));
    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "name"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&extract_str(&response, "name"));
            println!();
            ctx.info(&format!(
                "Author       {}",
                extract_str(&response, "author")
            ));
            ctx.info(&format!(
                "Category     {}",
                extract_str(&response, "category")
            ));
            if let Some(desc) = response
                .get("description")
                .and_then(serde_json::Value::as_str)
            {
                ctx.info(&format!("Description  {desc}"));
            }
            println!();
        }
    }

    Ok(())
}

async fn execute_patch(
    args: &EffectPatchArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut values = BTreeMap::new();
    for (key, value) in &args.param {
        values.insert(
            key.clone(),
            ApiControlValue::try_from(control_value_from_json(parse_control_value(value))?)?,
        );
    }

    let (zone, layer, _, _) = active_effect_layer(client).await?;
    let path = format!("/scene/zones/{zone}/layers/{}/controls", layer.id);
    let response = client
        .patch(
            &path,
            &PatchControlsRequest {
                values,
                clear_bindings: Vec::new(),
            },
        )
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let count = args.param.len();
            ctx.success(&format!("Patched {count} control(s)"));
        }
    }

    Ok(())
}

async fn execute_reset(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let (zone, layer, effect_id, _) = active_effect_layer(client).await?;
    let detail: EffectDetailResponse =
        serde_json::from_value(client.get(&format!("/effects/{effect_id}")).await?)?;
    let values: std::collections::HashMap<_, _> = detail
        .controls
        .into_iter()
        .map(|control| (control.control_id().to_owned(), control.default_value))
        .collect();
    let LayerSource::Effect {
        effect_id,
        control_bindings,
        ..
    } = &layer.source
    else {
        anyhow::bail!("The active zone has no effect layer");
    };
    let response = client
        .put(
            &format!("/scene/zones/{zone}/layers/{}", layer.id),
            &ReplaceLayerRequest {
                source: LayerSource::Effect {
                    effect_id: *effect_id,
                    controls: values,
                    control_bindings: control_bindings.clone(),
                    preset_id: None,
                },
                name: layer.name.clone(),
                blend: Some(layer.blend),
                opacity: Some(layer.opacity),
                transform: Some(layer.transform),
                adjust: Some(layer.adjust),
                bindings: Some(layer.bindings.clone()),
                enabled: Some(layer.enabled),
            },
        )
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success("Controls reset to defaults");
        }
    }

    Ok(())
}

async fn execute_rescan(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client
        .post("/effects/rescan", &serde_json::json!({}))
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let count = response
                .get("count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            ctx.success(&format!("Rescanned: {count} effects found"));
        }
    }

    Ok(())
}

async fn active_effect_layer(
    client: &DaemonClient,
) -> Result<(ZoneId, SceneLayer, String, Vec<String>)> {
    let scene: hypercolor_types::api::scene::SceneDocument =
        serde_json::from_value(client.get("/scene").await?)?;
    let zone = scene
        .zones
        .iter()
        .find(|zone| zone.role == ZoneRole::Primary)
        .or_else(|| scene.zones.first())
        .ok_or_else(|| anyhow::anyhow!("The active scene has no zones"))?;
    let layer = zone
        .layers
        .iter()
        .rev()
        .find_map(|layer| match &layer.source {
            LayerSource::Effect {
                effect_id,
                control_bindings,
                ..
            } => Some((
                layer.clone(),
                effect_id.to_string(),
                control_bindings.keys().cloned().collect(),
            )),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("The active zone has no effect layer"))?;
    Ok((zone.id, layer.0, layer.1, layer.2))
}

fn control_value_from_json(value: serde_json::Value) -> Result<ControlValue> {
    if let Some(value) = value.as_i64() {
        return Ok(ControlValue::Integer(i32::try_from(value)?));
    }
    if value.is_number() {
        return Ok(ControlValue::Float(serde_json::from_value(value)?));
    }
    if let Some(value) = value.as_bool() {
        return Ok(ControlValue::Boolean(value));
    }
    if let Some(value) = value.as_str() {
        return Ok(ControlValue::Text(value.to_owned()));
    }
    if let Ok(color) = serde_json::from_value::<[f32; 4]>(value.clone()) {
        return Ok(ControlValue::Color(color));
    }
    if let Ok(rect) = serde_json::from_value(value) {
        return Ok(ControlValue::Rect(rect));
    }
    anyhow::bail!("Unsupported effect control value")
}
