//! Explicit protected-input and screen-capture actions.

use anyhow::Result;
use clap::{Args, Subcommand};

use hypercolor_types::api::capture::{
    CaptureAuthorizationResponse, CapturePickerResponse, ProtectedSourceGrantOwner,
};

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

    fn human_success_message(self, grant_owner: ProtectedSourceGrantOwner) -> String {
        format!(
            "{}; grant owner: {}",
            self.success_message(),
            grant_owner_label(grant_owner)
        )
    }
}

const fn grant_owner_label(owner: ProtectedSourceGrantOwner) -> &'static str {
    match owner {
        ProtectedSourceGrantOwner::AppSidecar => "Hypercolor.app sidecar",
        ProtectedSourceGrantOwner::App => "Hypercolor.app",
        ProtectedSourceGrantOwner::LaunchdService => "direct launchd service",
        ProtectedSourceGrantOwner::HomebrewService => "Homebrew service",
        ProtectedSourceGrantOwner::Broker => "authenticated app broker",
        ProtectedSourceGrantOwner::Standalone => "standalone daemon",
        ProtectedSourceGrantOwner::PlatformBackend => "active platform backend",
    }
}

/// Execute one explicit protected-source action.
///
/// # Errors
///
/// Returns an error when the daemon is unavailable or the active topology
/// cannot execute the requested action. Headless picker failures preserve the
/// daemon's typed `requires_app_ui` response.
pub async fn execute(args: &AccessArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match args.command {
        AccessCommand::ChooseScreenSource => {
            let response: CapturePickerResponse = client
                .put(args.command.route(), &serde_json::json!({}))
                .await?;
            render(args.command, response.grant_owner, &response, ctx)
        }
        AccessCommand::AuthorizeInputMonitoring | AccessCommand::AuthorizeScreenRecording => {
            let response: CaptureAuthorizationResponse = client
                .post(args.command.route(), &serde_json::json!({}))
                .await?;
            render(args.command, response.grant_owner, &response, ctx)
        }
    }
}

fn render(
    command: AccessCommand,
    grant_owner: ProtectedSourceGrantOwner,
    response: &impl serde::Serialize,
    ctx: &OutputContext,
) -> Result<()> {
    if ctx.format == OutputFormat::Json {
        ctx.print_json(response)?;
    } else {
        ctx.success(&command.human_success_message(grant_owner));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use hypercolor_types::api::capture::ProtectedSourceGrantOwner;

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
        assert_eq!(
            AccessCommand::AuthorizeInputMonitoring
                .human_success_message(ProtectedSourceGrantOwner::Broker),
            "Input Monitoring request completed; grant owner: authenticated app broker"
        );
        assert_eq!(
            grant_owner_label(ProtectedSourceGrantOwner::HomebrewService),
            "Homebrew service"
        );
        assert_eq!(
            grant_owner_label(ProtectedSourceGrantOwner::PlatformBackend),
            "active platform backend"
        );
        assert_eq!(
            AccessCommand::ChooseScreenSource
                .human_success_message(ProtectedSourceGrantOwner::Standalone),
            "Screen source picker completed; grant owner: standalone daemon"
        );
    }
}
