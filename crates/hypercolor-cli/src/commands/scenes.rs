//! `hyper scenes` -- scene management and automation.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

/// Scene management (automated lighting triggers).
#[derive(Debug, Args)]
pub struct ScenesArgs {
    #[command(subcommand)]
    pub command: SceneCommand,
}

/// Scene subcommands.
#[derive(Debug, Subcommand)]
pub enum SceneCommand {
    /// List configured scenes.
    List,
    /// Create a new scene.
    Create(SceneCreateArgs),
    /// Manually activate a scene.
    Activate(SceneActivateArgs),
    /// Delete a scene.
    Delete(SceneDeleteArgs),
    /// Show detailed scene configuration.
    Info(SceneInfoArgs),
}

/// Arguments for `scenes create`.
#[derive(Debug, Args)]
pub struct SceneCreateArgs {
    /// Scene name.
    pub name: String,

    /// Profile to activate when triggered.
    #[arg(long, required = true)]
    pub profile: String,

    /// Trigger type: schedule, sunset, sunrise, device, audio.
    #[arg(long, required = true)]
    pub trigger: String,

    /// Cron expression (for schedule trigger).
    #[arg(long)]
    pub cron: Option<String>,

    /// Offset in minutes from solar event (for sunset/sunrise).
    #[arg(long, default_value = "0")]
    pub offset: i32,

    /// Transition duration in milliseconds.
    #[arg(long, default_value = "1000")]
    pub transition: u32,

    /// Start enabled.
    #[arg(long, default_value = "true")]
    pub enabled: bool,
}

/// Arguments for `scenes activate`.
#[derive(Debug, Args)]
pub struct SceneActivateArgs {
    /// Scene name or ID.
    pub name: String,

    /// Override transition duration (ms).
    #[arg(long)]
    pub transition: Option<u32>,
}

/// Arguments for `scenes delete`.
#[derive(Debug, Args)]
pub struct SceneDeleteArgs {
    /// Scene name or ID.
    pub name: String,

    /// Skip confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for `scenes info`.
#[derive(Debug, Args)]
pub struct SceneInfoArgs {
    /// Scene name or ID.
    pub name: String,
}

/// Execute the `scenes` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the scene is not found.
pub async fn execute(args: &ScenesArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        SceneCommand::List => execute_list(client, ctx).await,
        SceneCommand::Create(create_args) => execute_create(create_args, client, ctx).await,
        SceneCommand::Activate(activate_args) => execute_activate(activate_args, client, ctx).await,
        SceneCommand::Delete(delete_args) => execute_delete(delete_args, client, ctx).await,
        SceneCommand::Info(info_args) => execute_info(info_args, client, ctx).await,
    }
}

async fn execute_list(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/scenes").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(scenes) = response.get("items").and_then(serde_json::Value::as_array) {
                for scene in scenes {
                    if let Some(name) = scene.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(scenes) = response.get("items").and_then(serde_json::Value::as_array) {
                let headers = ["ID", "Scene", "Priority", "Enabled"];
                let rows: Vec<Vec<String>> = scenes
                    .iter()
                    .map(|s| {
                        vec![
                            extract_str(s, "id"),
                            extract_str(s, "name"),
                            s.get("priority")
                                .and_then(serde_json::Value::as_u64)
                                .map_or_else(|| "?".to_string(), |v| v.to_string()),
                            if s.get("enabled")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                            {
                                "yes".to_string()
                            } else {
                                "no".to_string()
                            },
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                ctx.info(&format!("{} scenes", scenes.len()));
            }
        }
    }

    Ok(())
}

async fn execute_create(
    args: &SceneCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({
        "name": args.name,
        "profile": args.profile,
        "trigger": args.trigger,
        "cron": args.cron,
        "offset_minutes": args.offset,
        "transition_ms": args.transition,
        "enabled": args.enabled,
    });

    let response = client.post("/scenes", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene created: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_activate(
    args: &SceneActivateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/scenes/{}/activate", urlencoded(&args.name));
    let body = serde_json::json!({ "transition_ms": args.transition });
    let response = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene triggered: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_delete(
    args: &SceneDeleteArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if !args.yes {
        ctx.warning(&format!(
            "Use --yes to confirm deletion of scene '{}'",
            args.name
        ));
        return Ok(());
    }

    let path = format!("/scenes/{}", urlencoded(&args.name));
    let response = client.delete(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene deleted: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_info(
    args: &SceneInfoArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/scenes/{}", urlencoded(&args.name));
    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "name"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Scene: {}", extract_str(&response, "name")));
            println!();
            ctx.info(&format!("ID             {}", extract_str(&response, "id")));
            let priority = response
                .get("priority")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "?".to_string(), |v| v.to_string());
            ctx.info(&format!("Priority       {priority}"));
            ctx.info(&format!(
                "Enabled        {}",
                if response
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    "yes"
                } else {
                    "no"
                }
            ));
            println!();
        }
    }

    Ok(())
}
