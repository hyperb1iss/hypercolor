//! Produce a Windows release unit from a build's outputs.
//!
//! ```text
//! hypercolor-windows-unit --version <version> --rust-target <triple>
//!     --bin-dir <dir> --stage-dir <dir> --source-root <dir> --output <dir>
//!     [--durable-stores <file>]... [--third-party-notices <dir>]
//! ```
//!
//! `--bin-dir` holds the six signed release executables, `--stage-dir` the
//! assets `scripts/stage-app-bundle-assets.ps1` stages, and `--source-root`
//! the engine checkout. The unit lands at `--output`, which must not exist,
//! and the unit digest is printed on standard output.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use hypercolor_install::windows::{WindowsUnitSources, produce_windows_unit};

const USAGE: &str = "usage: hypercolor-windows-unit --version <version> --rust-target <triple> \
    --bin-dir <dir> --stage-dir <dir> --source-root <dir> --output <dir> \
    [--durable-stores <file>]... [--third-party-notices <dir>]";

fn main() -> ExitCode {
    let (sources, output) = match parse(std::env::args_os().skip(1)) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match produce_windows_unit(&sources, &output) {
        Ok(unit) => {
            println!("{}", unit.unit_id().as_str());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("hypercolor-windows-unit: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse(
    mut arguments: impl Iterator<Item = OsString>,
) -> Result<(WindowsUnitSources, PathBuf), String> {
    let mut version = None;
    let mut rust_target = None;
    let mut bin_dir = None;
    let mut stage_dir = None;
    let mut source_root = None;
    let mut output = None;
    let mut overlays = Vec::new();
    let mut notices = None;
    while let Some(flag) = arguments.next() {
        let flag = flag
            .into_string()
            .map_err(|flag| format!("unknown argument {}", flag.to_string_lossy()))?;
        if flag == "-h" || flag == "--help" {
            return Err("produce a Windows release unit".to_owned());
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        let text = || {
            value
                .clone()
                .into_string()
                .map_err(|_| format!("{flag} must be Unicode"))
        };
        let once = |slot: &mut Option<PathBuf>| {
            if slot.replace(PathBuf::from(&value)).is_some() {
                Err(format!("{flag} may be given once"))
            } else {
                Ok(())
            }
        };
        match flag.as_str() {
            "--version" => version = Some(text()?),
            "--rust-target" => rust_target = Some(text()?),
            "--bin-dir" => once(&mut bin_dir)?,
            "--stage-dir" => once(&mut stage_dir)?,
            "--source-root" => once(&mut source_root)?,
            "--output" => once(&mut output)?,
            "--third-party-notices" => once(&mut notices)?,
            "--durable-stores" => overlays.push(PathBuf::from(&value)),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }
    let required = |value: Option<PathBuf>, flag: &str| value.ok_or(format!("{flag} is required"));
    let sources = WindowsUnitSources {
        version: version.ok_or("--version is required")?,
        rust_target: rust_target.ok_or("--rust-target is required")?,
        bin_dir: required(bin_dir, "--bin-dir")?,
        stage_dir: required(stage_dir, "--stage-dir")?,
        source_root: required(source_root, "--source-root")?,
        durable_store_overlays: overlays,
        third_party_notices: notices,
    };
    Ok((sources, required(output, "--output")?))
}
