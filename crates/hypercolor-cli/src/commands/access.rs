//! Explicit protected-input and screen-capture actions.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Explicit user actions for protected host input and screen capture.
#[derive(Debug, Args)]
pub struct AccessArgs {
    #[command(subcommand)]
    pub command: AccessCommand,
}

/// Protected-source actions. No command prompts implicitly at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum AccessCommand {
    /// Ask the active macOS owner for Input Monitoring authorization.
    AuthorizeInputMonitoring,
    /// Ask the active macOS owner for Screen Recording authorization.
    AuthorizeScreenRecording,
    /// Present the platform screen-source picker when this owner can show UI.
    ChooseScreenSource,
}

impl AccessCommand {
    const fn route(self) -> &'static str {
        match self {
            Self::AuthorizeInputMonitoring => "/input/authorize",
            Self::AuthorizeScreenRecording => "/capture/authorize",
            Self::ChooseScreenSource => "/capture/source",
        }
    }

    const fn success_message(self) -> &'static str {
        match self {
            Self::AuthorizeInputMonitoring => "Input Monitoring request completed",
            Self::AuthorizeScreenRecording => "Screen Recording request completed",
            Self::ChooseScreenSource => "Screen source picker completed",
        }
    }

    fn human_success_message(self, response: &serde_json::Value) -> String {
        let owner = response
            .get("grant_owner")
            .and_then(serde_json::Value::as_str)
            .map(grant_owner_label)
            .unwrap_or("unavailable from this daemon");
        format!("{}; grant owner: {owner}", self.success_message())
    }
}

fn grant_owner_label(owner: &str) -> &str {
    match owner {
        "app_sidecar" => "Hypercolor.app sidecar",
        "app" => "Hypercolor.app",
        "launchd_service" => "direct launchd service",
        "homebrew_service" => "Homebrew service",
        "broker" => "authenticated app broker",
        "standalone" => "standalone daemon",
        "platform_backend" => "active platform backend",
        _ => "unknown process topology",
    }
}

/// Execute one explicit protected-source action.
///
/// # Errors
///
/// Returns an error when the daemon is unavailable or the active topology
/// cannot execute the requested action. Headless picker failures preserve the
/// daemon's typed `requires_ui` response.
pub async fn execute(args: &AccessArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response = match args.command {
        AccessCommand::ChooseScreenSource => {
            client
                .put(args.command.route(), &serde_json::json!({}))
                .await?
        }
        AccessCommand::AuthorizeInputMonitoring | AccessCommand::AuthorizeScreenRecording => {
            client
                .post(args.command.route(), &serde_json::json!({}))
                .await?
        }
    };
    if ctx.format == OutputFormat::Json {
        ctx.print_json(&response)?;
    } else {
        ctx.success(&args.command.human_success_message(&response));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{AccessCommand, grant_owner_label};
    use crate::{Cli, Commands};

    #[test]
    fn protected_actions_use_only_explicit_daemon_routes() {
        assert_eq!(
            AccessCommand::AuthorizeInputMonitoring.route(),
            "/input/authorize"
        );
        assert_eq!(
            AccessCommand::AuthorizeScreenRecording.route(),
            "/capture/authorize"
        );
        assert_eq!(AccessCommand::ChooseScreenSource.route(), "/capture/source");
    }

    #[test]
    fn protected_actions_parse_as_explicit_subcommands() {
        for (name, expected) in [
            (
                "authorize-input-monitoring",
                AccessCommand::AuthorizeInputMonitoring,
            ),
            (
                "authorize-screen-recording",
                AccessCommand::AuthorizeScreenRecording,
            ),
            ("choose-screen-source", AccessCommand::ChooseScreenSource),
        ] {
            let cli = Cli::try_parse_from(["hypercolor", "access", name])
                .expect("protected-source command should parse");
            let Commands::Access(args) = cli.command else {
                panic!("access command should preserve its top-level group");
            };
            assert_eq!(args.command, expected);
        }
    }

    #[test]
    fn protected_actions_name_the_exact_grant_owner() {
        let response = serde_json::json!({"grant_owner": "broker"});
        assert_eq!(
            AccessCommand::AuthorizeInputMonitoring.human_success_message(&response),
            "Input Monitoring request completed; grant owner: authenticated app broker"
        );
        assert_eq!(grant_owner_label("homebrew_service"), "Homebrew service");
        assert_eq!(
            grant_owner_label("platform_backend"),
            "active platform backend"
        );
        assert_eq!(
            AccessCommand::ChooseScreenSource.human_success_message(&serde_json::json!({})),
            "Screen source picker completed; grant owner: unavailable from this daemon"
        );
    }
}
