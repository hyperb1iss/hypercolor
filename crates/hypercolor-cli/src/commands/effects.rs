//! `hyper effects` -- effect browsing, activation, and control.

use anyhow::Result;
use clap::{Args, Subcommand};

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
    /// Show detailed information about an effect.
    Info(EffectInfoArgs),
    /// Patch controls on the currently running effect (without re-applying).
    Patch(EffectPatchArgs),
    /// Reset controls on the currently running effect to defaults.
    Reset,
    /// Rescan the effect library for new or changed effects.
    Rescan,
    /// Manage effect-to-layout associations.
    Layout(EffectLayoutArgs),
}

/// Arguments for `effects list`.
#[derive(Debug, Args)]
pub struct EffectListArgs {
    /// Filter by engine type (native, web, wasm).
    #[arg(long)]
    pub engine: Option<String>,

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

    /// Crossfade transition duration in milliseconds.
    #[arg(long, default_value = "0")]
    pub transition: u32,
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

/// Arguments for `effects layout`.
#[derive(Debug, Args)]
pub struct EffectLayoutArgs {
    #[command(subcommand)]
    pub command: EffectLayoutCommand,
}

/// Effect layout subcommands.
#[derive(Debug, Subcommand)]
pub enum EffectLayoutCommand {
    /// Show the layout associated with an effect.
    Show(EffectLayoutShowArgs),
    /// Associate an effect with a specific layout.
    Set(EffectLayoutSetArgs),
    /// Remove the layout association from an effect.
    Clear(EffectLayoutClearArgs),
}

/// Arguments for `effects layout show`.
#[derive(Debug, Args)]
pub struct EffectLayoutShowArgs {
    /// Effect name or ID.
    pub effect: String,
}

/// Arguments for `effects layout set`.
#[derive(Debug, Args)]
pub struct EffectLayoutSetArgs {
    /// Effect name or ID.
    pub effect: String,
    /// Layout ID to associate.
    pub layout: String,
}

/// Arguments for `effects layout clear`.
#[derive(Debug, Args)]
pub struct EffectLayoutClearArgs {
    /// Effect name or ID.
    pub effect: String,
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
        EffectCommand::Info(info_args) => execute_info(info_args, client, ctx).await,
        EffectCommand::Patch(patch_args) => execute_patch(patch_args, client, ctx).await,
        EffectCommand::Reset => execute_reset(client, ctx).await,
        EffectCommand::Rescan => execute_rescan(client, ctx).await,
        EffectCommand::Layout(layout_args) => execute_layout(layout_args, client, ctx).await,
    }
}

async fn execute_list(
    args: &EffectListArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut path = "/effects".to_string();
    let mut query_parts = Vec::new();

    if let Some(engine) = &args.engine {
        query_parts.push(format!("engine={}", urlencoded(engine)));
    }
    if args.audio {
        query_parts.push("audio=true".to_string());
    }
    if let Some(search) = &args.search {
        query_parts.push(format!("search={}", urlencoded(search)));
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
    let mut controls = serde_json::Map::new();
    for (key, value) in &args.param {
        controls.insert(key.clone(), parse_control_value(value));
    }
    if let Some(speed) = args.speed {
        controls.insert("speed".to_string(), serde_json::Value::from(speed));
    }
    if let Some(intensity) = args.intensity {
        controls.insert("intensity".to_string(), serde_json::Value::from(intensity));
    }

    let body = serde_json::json!({
        "controls": controls,
        "transition": {
            "type": if args.transition == 0 { "cut" } else { "crossfade" },
            "duration_ms": args.transition,
        },
    });

    // The daemon's apply endpoint uses effect IDs in the path.
    // URL-encode the effect name/slug for path-based lookup.
    let path = format!("/effects/{}/apply", urlencoded(&args.effect));
    let response = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let name = response
                .get("effect")
                .and_then(|e| e.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&args.effect);
            ctx.success(&format!("Effect set: {name}"));
        }
    }

    Ok(())
}

async fn execute_stop(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.post("/effects/stop", &serde_json::json!({})).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success("Effect stopped");
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
    let mut controls = serde_json::Map::new();
    for (key, value) in &args.param {
        controls.insert(key.clone(), parse_control_value(value));
    }

    let body = serde_json::json!({ "controls": controls });
    let response = client.patch("/effects/current/controls", &body).await?;

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
    let response = client
        .post("/effects/current/reset", &serde_json::json!({}))
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

async fn execute_layout(
    args: &EffectLayoutArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        EffectLayoutCommand::Show(show_args) => {
            let path = format!("/effects/{}/layout", urlencoded(&show_args.effect));
            let response = client.get(&path).await?;

            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    let layout_id = extract_str(&response, "layout_id");
                    ctx.info(&format!("{}: layout = {layout_id}", show_args.effect));
                }
            }
        }
        EffectLayoutCommand::Set(set_args) => {
            let path = format!("/effects/{}/layout", urlencoded(&set_args.effect));
            let body = serde_json::json!({ "layout_id": set_args.layout });
            let response = client.put(&path, &body).await?;

            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!(
                        "Effect {:?} linked to layout {:?}",
                        set_args.effect, set_args.layout
                    ));
                }
            }
        }
        EffectLayoutCommand::Clear(clear_args) => {
            let path = format!("/effects/{}/layout", urlencoded(&clear_args.effect));
            let response = client.delete(&path).await?;

            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!(
                        "Layout association cleared for {:?}",
                        clear_args.effect
                    ));
                }
            }
        }
    }

    Ok(())
}
