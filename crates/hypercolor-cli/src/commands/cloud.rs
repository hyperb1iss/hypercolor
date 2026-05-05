//! `hyper cloud` -- Hypercolor Cloud account and daemon-link controls.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str};

#[derive(Debug, Args)]
pub struct CloudArgs {
    #[command(subcommand)]
    pub command: CloudCommand,
}

#[derive(Debug, Subcommand)]
pub enum CloudCommand {
    /// Log this daemon into Hypercolor Cloud.
    Login(CloudLoginArgs),
    /// Log this daemon out of Hypercolor Cloud locally.
    Logout(CloudLogoutArgs),
    /// Show daemon cloud socket readiness.
    Connection,
    /// Show cached cloud entitlement status.
    Entitlement,
    /// Show daemon cloud feature/configuration status.
    Status,
    /// Show local cloud login/session status.
    Session,
    /// Create or show this daemon's cloud identity.
    Identity,
}

#[derive(Debug, Args)]
pub struct CloudLoginArgs {
    /// Maximum seconds to wait for browser approval.
    #[arg(long, default_value_t = 300)]
    pub timeout_seconds: u64,

    /// Do not open the verification URL in a browser.
    #[arg(long)]
    pub no_open: bool,
}

#[derive(Debug, Args)]
pub struct CloudLogoutArgs {
    /// Skip confirmation.
    #[arg(long)]
    pub yes: bool,
}

pub async fn execute(args: &CloudArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        CloudCommand::Login(login_args) => execute_login(login_args, client, ctx).await,
        CloudCommand::Logout(logout_args) => execute_logout(logout_args, client, ctx).await,
        CloudCommand::Connection => execute_connection(client, ctx).await,
        CloudCommand::Entitlement => execute_entitlement(client, ctx).await,
        CloudCommand::Status => execute_status(client, ctx).await,
        CloudCommand::Session => execute_session(client, ctx).await,
        CloudCommand::Identity => execute_identity(client, ctx).await,
    }
}

async fn execute_entitlement(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/cloud/entitlement").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            let cached = response
                .get("cached")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!("{}", if cached { "cached" } else { "missing" });
        }
        OutputFormat::Table => {
            let cached = response
                .get("cached")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let stale = response
                .get("stale")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!();
            ctx.info(&format!("Entitlement {}", ctx.painter.yesno(cached)));
            ctx.info(&format!("Stale       {}", ctx.painter.yesno(stale)));
            ctx.info(&format!(
                "Tier        {}",
                ctx.painter.keyword(&extract_str(&response, "tier"))
            ));
            ctx.info(&format!(
                "Expires     {}",
                extract_str(&response, "expires_at")
            ));
            println!();
        }
    }

    Ok(())
}

async fn execute_connection(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/cloud/connection").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "state"));
        }
        OutputFormat::Table => {
            let can_connect = response
                .get("can_connect")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let connected = response
                .get("connected")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let entitlement_cached = response
                .get("entitlement_cached")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!();
            ctx.info(&format!(
                "Connection  {}",
                ctx.painter.device_state(&extract_str(&response, "state"))
            ));
            ctx.info(&format!("Live        {}", ctx.painter.yesno(connected)));
            ctx.info(&format!("Ready       {}", ctx.painter.yesno(can_connect)));
            ctx.info(&format!(
                "Entitlement {}",
                ctx.painter.yesno(entitlement_cached)
            ));
            ctx.info(&format!(
                "URL         {}",
                ctx.painter.name(&extract_str(&response, "connect_url"))
            ));
            println!();
        }
    }

    Ok(())
}

async fn execute_logout(
    args: &CloudLogoutArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if !args.yes {
        ctx.warning("Use --yes to confirm cloud logout");
        return Ok(());
    }

    let response = client.delete("/cloud/session").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("signed-out");
        }
        OutputFormat::Table => {
            println!();
            ctx.success("Cloud session cleared");
            ctx.info(&format!(
                "Token       {}",
                ctx.painter.yesno(
                    response
                        .get("refresh_token_deleted")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                )
            ));
            ctx.info(&format!(
                "Identity    {}",
                ctx.painter.yesno(
                    response
                        .get("identity_preserved")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                )
            ));
            println!();
        }
    }

    Ok(())
}

async fn execute_login(
    args: &CloudLoginArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let start = client
        .post("/cloud/login/start", &serde_json::json!({}))
        .await?;
    render_login_start(&start, args, ctx)?;

    let login_id = extract_required_str(&start, "login_id")?;
    let deadline = Instant::now() + Duration::from_secs(args.timeout_seconds);
    let mut retry_after = retry_after_duration(&start);

    loop {
        if Instant::now() + retry_after > deadline {
            bail!("Timed out waiting for cloud login approval");
        }
        tokio::time::sleep(retry_after).await;

        let poll = client
            .post(
                &format!("/cloud/login/{login_id}/poll"),
                &serde_json::json!({}),
            )
            .await?;
        match extract_required_str(&poll, "status")? {
            "pending" => {
                retry_after = retry_after_duration(&poll);
                if matches!(ctx.format, OutputFormat::Table) {
                    ctx.info("Waiting for approval...");
                }
            }
            "authorized" => {
                render_login_success(&start, &poll, ctx)?;
                return Ok(());
            }
            "expired" => bail!("Cloud login code expired"),
            "rejected" => bail!("Cloud login was rejected"),
            status => bail!("Daemon returned unknown cloud login status: {status}"),
        }
    }
}

async fn execute_status(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/cloud/status").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            let enabled = response
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!("{}", if enabled { "enabled" } else { "disabled" });
        }
        OutputFormat::Table => {
            let enabled = response
                .get("enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let state = if enabled {
                ctx.painter.success("enabled")
            } else {
                ctx.painter.muted("disabled")
            };
            println!();
            ctx.info(&format!("Cloud       {state}"));
            ctx.info(&format!(
                "API         {}",
                ctx.painter.keyword(&extract_str(&response, "base_url"))
            ));
            ctx.info(&format!(
                "Auth        {}",
                ctx.painter
                    .keyword(&extract_str(&response, "auth_base_url"))
            ));
            ctx.info(&format!(
                "App         {}",
                ctx.painter.keyword(&extract_str(&response, "app_base_url"))
            ));
            ctx.info(&format!(
                "Identity    {}",
                extract_str(&response, "identity_storage")
            ));
            println!();
        }
    }

    Ok(())
}

async fn execute_session(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client.get("/cloud/session").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            let authenticated = response
                .get("authenticated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!(
                "{}",
                if authenticated {
                    "authenticated"
                } else {
                    "signed-out"
                }
            );
        }
        OutputFormat::Table => {
            let authenticated = response
                .get("authenticated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let identity_present = response
                .get("identity_present")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            println!();
            ctx.info(&format!(
                "Session     {}",
                if authenticated {
                    ctx.painter.success("authenticated")
                } else {
                    ctx.painter.muted("signed-out")
                }
            ));
            ctx.info(&format!(
                "Identity    {}",
                ctx.painter.yesno(identity_present)
            ));
            ctx.info(&format!(
                "Daemon ID   {}",
                ctx.painter.id(&extract_str(&response, "daemon_id"))
            ));
            println!();
        }
    }

    Ok(())
}

fn render_login_start(
    start: &serde_json::Value,
    args: &CloudLoginArgs,
    ctx: &OutputContext,
) -> Result<()> {
    let verification_uri = start
        .get("verification_uri_complete")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            start
                .get("verification_uri")
                .and_then(serde_json::Value::as_str)
        })
        .context("daemon response omitted verification_uri")?;

    if !args.no_open
        && let Err(error) = open::that_detached(verification_uri)
    {
        ctx.warning(&format!("Could not open browser: {error}"));
    }

    match ctx.format {
        OutputFormat::Json => {}
        OutputFormat::Plain => {
            println!("{verification_uri}");
            println!("{}", extract_str(start, "user_code"));
        }
        OutputFormat::Table => {
            println!();
            ctx.info("Cloud login started");
            ctx.info(&format!(
                "Code        {}",
                ctx.painter.keyword(&extract_str(start, "user_code"))
            ));
            ctx.info(&format!(
                "Open        {}",
                ctx.painter.name(verification_uri)
            ));
            println!();
        }
    }

    Ok(())
}

fn render_login_success(
    start: &serde_json::Value,
    poll: &serde_json::Value,
    ctx: &OutputContext,
) -> Result<()> {
    match ctx.format {
        OutputFormat::Json => {
            ctx.print_json(&serde_json::json!({
                "start": start,
                "result": poll,
            }))?;
        }
        OutputFormat::Plain => {
            println!("{}", extract_str(poll, "daemon_id"));
        }
        OutputFormat::Table => {
            println!();
            ctx.success("Cloud login complete");
            ctx.info(&format!(
                "Daemon ID   {}",
                ctx.painter.id(&extract_str(poll, "daemon_id"))
            ));
            ctx.info(&format!(
                "Device      {}",
                ctx.painter.id(&extract_str(poll, "device_install_id"))
            ));
            println!();
        }
    }

    Ok(())
}

fn retry_after_duration(value: &serde_json::Value) -> Duration {
    let millis = value
        .get("retry_after_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(5_000)
        .max(100);
    Duration::from_millis(millis)
}

fn extract_required_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("daemon response omitted {key}"))
}

async fn execute_identity(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = client
        .post("/cloud/identity", &serde_json::json!({}))
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", extract_str(&response, "daemon_id"));
        }
        OutputFormat::Table => {
            println!();
            ctx.success("Cloud identity ready");
            ctx.info(&format!(
                "Daemon ID   {}",
                ctx.painter.id(&extract_str(&response, "daemon_id"))
            ));
            ctx.info(&format!(
                "Public key  {}",
                extract_str(&response, "identity_pubkey")
            ));
            println!();
        }
    }

    Ok(())
}
