//! `hypercolor completions` -- generate shell completion scripts.

use std::io;

use clap::{Args, Command, ValueEnum};
use clap_complete::{Shell, generate};

/// Generate shell completion scripts.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    pub shell: CompletionShell,
}

/// Supported shells for completion generation.
#[derive(Debug, Clone, ValueEnum)]
pub enum CompletionShell {
    /// Bash shell.
    Bash,
    /// Zsh shell.
    Zsh,
    /// Fish shell.
    Fish,
    /// Windows `PowerShell`.
    Powershell,
}

impl CompletionShell {
    /// Convert to the `clap_complete` `Shell` enum.
    fn to_clap_shell(&self) -> Shell {
        match self {
            Self::Bash => Shell::Bash,
            Self::Zsh => Shell::Zsh,
            Self::Fish => Shell::Fish,
            Self::Powershell => Shell::PowerShell,
        }
    }
}

/// Execute the `completions` subcommand.
///
/// Generates shell completion scripts for the specified shell and writes
/// them to stdout.
pub fn execute(args: &CompletionsArgs, cmd: &mut Command) {
    let shell = args.shell.to_clap_shell();
    generate(shell, cmd, "hypercolor", &mut io::stdout());
}
