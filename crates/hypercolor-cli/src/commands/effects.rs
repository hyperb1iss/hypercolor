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
    #[arg(long, default_value = "500")]
    pub transition: u32,
}

/// Arguments for `effects info`.
#[derive(Debug, Args)]
pub struct EffectInfoArgs {
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
                            extract_str(e, "name"),
                            extract_str(e, "category"),
                            extract_str(e, "author"),
                            extract_str(e, "version"),
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                ctx.info(&format!("{} effects", effects.len()));
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
        controls.insert(key.clone(), serde_json::Value::String(value.clone()));
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
            "type": "crossfade",
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
