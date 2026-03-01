//! `hyper profiles` -- profile save, apply, and management.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Profile management (save, apply, delete).
#[derive(Debug, Args)]
pub struct ProfilesArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

/// Profile subcommands.
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List all saved profiles.
    List,
    /// Create a new profile from current state.
    Create(ProfileCreateArgs),
    /// Apply a saved profile.
    Apply(ProfileApplyArgs),
    /// Delete a profile.
    Delete(ProfileDeleteArgs),
    /// Show detailed profile contents.
    Info(ProfileInfoArgs),
}

/// Arguments for `profiles create`.
#[derive(Debug, Args)]
pub struct ProfileCreateArgs {
    /// Profile name.
    pub name: String,

    /// Profile description.
    #[arg(long)]
    pub description: Option<String>,

    /// Overwrite if profile already exists.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `profiles apply`.
#[derive(Debug, Args)]
pub struct ProfileApplyArgs {
    /// Profile name or ID (fuzzy-matched).
    pub name: String,

    /// Crossfade transition duration in milliseconds.
    #[arg(long, default_value = "500")]
    pub transition: u32,
}

/// Arguments for `profiles delete`.
#[derive(Debug, Args)]
pub struct ProfileDeleteArgs {
    /// Profile name or ID.
    pub name: String,

    /// Skip confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for `profiles info`.
#[derive(Debug, Args)]
pub struct ProfileInfoArgs {
    /// Profile name or ID.
    pub name: String,
}

/// Execute the `profiles` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the profile is not found.
pub async fn execute(
    args: &ProfilesArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        ProfileCommand::List => execute_list(client, ctx).await,
        ProfileCommand::Create(create_args) => execute_create(create_args, client, ctx).await,
        ProfileCommand::Apply(apply_args) => execute_apply(apply_args, client, ctx).await,
        ProfileCommand::Delete(delete_args) => execute_delete(delete_args, client, ctx).await,
        ProfileCommand::Info(info_args) => execute_info(info_args, client, ctx).await,
    }
}

async fn execute_list(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/profiles").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            if let Some(profiles) = response.as_array() {
                for profile in profiles {
                    if let Some(name) = profile.get("name").and_then(serde_json::Value::as_str) {
                        println!("{name}");
                    }
                }
            }
        }
        OutputFormat::Table => {
            if let Some(profiles) = response.as_array() {
                let headers = ["Profile", "Effect", "Active", "Last Applied"];
                let rows: Vec<Vec<String>> = profiles
                    .iter()
                    .map(|p| {
                        vec![
                            extract_str(p, "name"),
                            extract_str(p, "effect"),
                            if p.get("active")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                            {
                                "yes".to_string()
                            } else {
                                String::new()
                            },
                            extract_str(p, "last_applied"),
                        ]
                    })
                    .collect();

                ctx.print_table(&headers, &rows);
                println!();
                ctx.info(&format!("{} profiles", profiles.len()));
            }
        }
    }

    Ok(())
}

async fn execute_create(
    args: &ProfileCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = serde_json::json!({
        "name": args.name,
        "description": args.description,
        "force": args.force,
    });

    let response = client.post("/profiles", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Profile saved: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_apply(
    args: &ProfileApplyArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/profiles/{}/apply", urlencoded(&args.name));
    let body = serde_json::json!({ "transition_ms": args.transition });
    let response = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Profile applied: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_delete(
    args: &ProfileDeleteArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if !args.yes && !ctx.quiet {
        ctx.warning(&format!(
            "Use --yes to confirm deletion of profile '{}'",
            args.name
        ));
        return Ok(());
    }

    let path = format!("/profiles/{}", urlencoded(&args.name));
    let response = client.delete(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Profile deleted: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_info(
    args: &ProfileInfoArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/profiles/{}", urlencoded(&args.name));
    let response = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "name"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Profile: {}", extract_str(&response, "name")));
            println!();
            ctx.info(&format!(
                "Effect        {}",
                extract_str(&response, "effect")
            ));
            if let Some(desc) = response
                .get("description")
                .and_then(serde_json::Value::as_str)
            {
                ctx.info(&format!("Description   {desc}"));
            }
            ctx.info(&format!(
                "Last Applied  {}",
                extract_str(&response, "last_applied")
            ));
            println!();
        }
    }

    Ok(())
}

fn extract_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?")
        .to_string()
}

fn urlencoded(s: &str) -> String {
    s.replace(' ', "%20")
}
