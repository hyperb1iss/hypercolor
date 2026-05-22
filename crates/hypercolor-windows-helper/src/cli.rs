//! Minimal CLI: the helper accepts only `--request-file <ABSOLUTE-PATH>`.
//!
//! No other flags, no positional args, no implicit verbs. All operation
//! parameters live inside the request file (see §7.4 of the roadmap).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

const FLAG: &str = "--request-file";

/// Parsed CLI invocation.
pub struct Invocation {
    pub request_file_path: PathBuf,
}

/// Parse `std::env::args` into an [`Invocation`], rejecting any input that
/// doesn't match the exact `--request-file <PATH>` shape.
pub fn parse() -> Result<Invocation> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        bail!(
            "expected exactly two arguments: `{FLAG} <ABSOLUTE-PATH>` (got {} args)",
            args.len()
        );
    }
    if args[0] != FLAG {
        bail!("first argument must be `{FLAG}` (got `{}`)", args[0]);
    }

    let request_file_path = PathBuf::from(&args[1])
        .canonicalize()
        .with_context(|| format!("could not canonicalize request file path `{}`", args[1]))?;

    Ok(Invocation { request_file_path })
}
