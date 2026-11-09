//! `hypercolor __install-release` on Windows: the one step an installer runs,
//! from inside the unit it carries.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use clap::{Args, Parser};
use hypercolor_install::UnitId;
use hypercolor_install::windows::{WindowsInstallRequest, install_windows_release};

#[doc(hidden)]
#[derive(Debug, Args)]
pub struct InstallReleaseArgs {
    #[arg(long, value_name = "ABSOLUTE_PATH")]
    install_dir: PathBuf,

    #[arg(long, value_parser = parse_unit)]
    expected_manifest_sha256: UnitId,
}

fn parse_unit(value: &str) -> Result<UnitId, String> {
    UnitId::new(value).map_err(|error| error.to_string())
}

#[derive(Parser)]
#[command(name = "hypercolor __install-release")]
struct InstallReleaseInvocation {
    #[command(flatten)]
    args: InstallReleaseArgs,
}

pub(crate) fn parse_invocation<I, T>(
    raw_args: I,
) -> Option<std::result::Result<InstallReleaseArgs, clap::Error>>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut raw_args = raw_args.into_iter().map(Into::into);
    let executable = raw_args.next()?;
    let command = raw_args.next()?;
    if command != OsStr::new("__install-release") {
        return None;
    }
    Some(
        InstallReleaseInvocation::try_parse_from(std::iter::once(executable).chain(raw_args))
            .map(|invocation| invocation.args),
    )
}

/// Install the unit this program belongs to. The installer unpacks the unit
/// somewhere temporary and runs this program from there.
pub(crate) fn execute(args: &InstallReleaseArgs) -> Result<()> {
    let executable = std::env::current_exe().context("failed to locate this program")?;
    let source = executable
        .parent()
        .ok_or_else(|| anyhow!("{} has no directory", executable.display()))?
        .to_path_buf();
    let notes = install_windows_release(&WindowsInstallRequest {
        install_dir: args.install_dir.clone(),
        source,
        unit: args.expected_manifest_sha256.clone(),
        managed_update: cfg!(feature = "managed-update"),
    })
    .map_err(|error| anyhow!("{error}"))?;
    for note in notes {
        println!("{note}");
    }
    Ok(())
}
