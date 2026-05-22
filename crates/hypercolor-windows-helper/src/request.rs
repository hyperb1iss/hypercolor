//! Request file structure and validation.
//!
//! Per §7.4 the caller writes a JSON file to
//! `%LOCALAPPDATA%\hypercolor\helper-requests\<nonce>.json` with owner-only
//! ACL, then passes the absolute path to the elevated helper. This module
//! loads + parses the file and runs the cheap structural validation;
//! deeper checks (owner SID, install attestation, nonce monotonicity) live
//! in dedicated modules added in follow-up commits.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::verbs::Verb;

/// Body of a helper request file.
#[derive(Debug, Clone, Deserialize)]
#[expect(
    dead_code,
    reason = "paths/flags consumed by verb handlers in follow-up commits"
)]
pub struct Request {
    /// Allowlisted verb to perform.
    pub verb: Verb,
    /// Verb-specific path arguments (canonicalized + constraint-checked
    /// in follow-up commits — see `auth::paths`).
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// Per-install monotonic nonce; helper persists last-seen in
    /// `%PROGRAMDATA%\hypercolor\helper.state` to prevent replay.
    pub nonce: u64,
    /// Unix-epoch milliseconds when the request was issued; helper rejects
    /// requests outside `±60s` of its own clock.
    pub issued_at_ms: u64,
    /// Verb-specific flags (e.g. `also_remove_pawnio: true` for the
    /// `uninstall-hardware-support` composite verb).
    #[serde(default)]
    pub flags: serde_json::Map<String, serde_json::Value>,
}

/// Load a request file from `path`, parse the JSON body, and run structural
/// validation. Does **not** yet perform owner-SID, install-attestation, or
/// nonce checks — those land in follow-up commits as the protocol is
/// fleshed out per §7.4.
pub fn load_and_validate(path: &Path) -> Result<Request> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read request file `{}`", path.display()))?;
    let request: Request = serde_json::from_str(&raw)
        .with_context(|| format!("parse request file `{}`", path.display()))?;

    if request.issued_at_ms == 0 {
        bail!("request `issued_at_ms` must be non-zero");
    }
    if request.nonce == 0 {
        bail!("request `nonce` must be non-zero");
    }

    Ok(request)
}
