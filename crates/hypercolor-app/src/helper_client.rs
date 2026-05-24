//! Invokes `hypercolor-windows-helper` for privileged operations.
//!
//! Writes a per-request JSON file under
//! `%LOCALAPPDATA%\hypercolor\helper-requests\<nonce>.json`, then runs the
//! helper elevated via PowerShell's `Start-Process -Verb RunAs -Wait`. The
//! full request-file authorization protocol (owner-SID check, install
//! attestation, per-install monotonic nonce state file) lives in the
//! helper crate per the Windows experience roadmap §7.4. This client just
//! produces a well-formed request and surfaces the helper's exit code.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const HELPER_BINARY_NAME: &str = "hypercolor-windows-helper.exe";

/// Override the helper binary location. Useful for dev (`just dev` sets
/// this to the freshly-built debug binary) and for tests.
pub const HELPER_PATH_ENV: &str = "HYPERCOLOR_HELPER_PATH";

/// Allowlisted verbs the app can request. Mirrors
/// `hypercolor-windows-helper::verbs::Verb`; duplicated here so this crate
/// doesn't have to take a dependency on the helper crate itself.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Verb {
    /// Stop + start the `HypercolorSmBus` Windows service. Cheap, safe,
    /// and the first verb wired end-to-end.
    RepairSmbusService,
}

/// Outcome surfaced to the UI after the helper exits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HelperOutcome {
    /// Helper process exit code propagated through `Start-Process -Wait
    /// -PassThru`. `None` means PowerShell itself failed to report a code
    /// (rare; UAC consent denial surfaces as a non-zero exit instead).
    pub exit_code: Option<i32>,
}

/// Locate the helper binary across dev and bundle layouts.
#[must_use]
pub fn resolve_helper_path(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(HELPER_PATH_ENV).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(dir) = resource_dir {
        let bundled = dir.join("tools").join(HELPER_BINARY_NAME);
        if bundled.is_file() {
            return Some(bundled);
        }
    }

    // Dev fallback: walk up from the running app exe, checking sibling
    // profile directories at each level. Handles the common `just dev`
    // layout where the app runs from `target/preview/` while the helper
    // lives under `target/debug/`.
    let exe = std::env::current_exe().ok()?;
    let mut probe = exe.parent();
    for _ in 0..6 {
        let parent = probe?;
        let direct = parent.join(HELPER_BINARY_NAME);
        if direct.is_file() {
            return Some(direct);
        }
        for profile in ["debug", "release", "preview"] {
            let candidate = parent.join(profile).join(HELPER_BINARY_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        probe = parent.parent();
    }
    None
}

/// Run the helper with the given verb, blocking until completion.
///
/// Triggers a UAC consent prompt. If the user declines, PowerShell exits
/// non-zero and the returned `HelperOutcome` carries that code so the UI
/// can show an appropriate error toast.
///
/// # Errors
///
/// Returns an error when the platform is unsupported, the helper binary
/// cannot be located, the request file cannot be written, or
/// `powershell.exe` itself fails to launch.
pub fn invoke(resource_dir: Option<&Path>, verb: Verb) -> Result<HelperOutcome> {
    if !cfg!(target_os = "windows") {
        bail!("helper invocation is only available on Windows");
    }

    let helper_path = resolve_helper_path(resource_dir).context(
        "hypercolor-windows-helper binary was not found; \
         build it with `cargo build --bin hypercolor-windows-helper` \
         or set HYPERCOLOR_HELPER_PATH",
    )?;

    let request_path = write_request_file(verb)?;
    let powershell_command = build_elevation_command(&helper_path, &request_path);

    let mut child = Command::new("powershell.exe");
    child
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &powershell_command,
        ])
        .stdin(Stdio::null());
    crate::process_ext::hide_console_window(&mut child);
    let status = child
        .status()
        .context("powershell.exe failed to launch helper")?;

    // Request files are single-use; the helper's per-install nonce state
    // file will reject replays once Phase 1.0 auth lands. Best-effort
    // cleanup keeps the request directory from growing unboundedly.
    let _ = std::fs::remove_file(&request_path);

    Ok(HelperOutcome {
        exit_code: status.code(),
    })
}

fn write_request_file(verb: Verb) -> Result<PathBuf> {
    let nonce = next_nonce();
    let issued_at_ms =
        u64::try_from(chrono::Utc::now().timestamp_millis().max(1)).unwrap_or(u64::MAX);

    let body = serde_json::json!({
        "verb": verb,
        "paths": [],
        "nonce": nonce,
        "issued_at_ms": issued_at_ms,
        "flags": {},
    });

    let dir = request_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{nonce}.json"));
    std::fs::write(&path, body.to_string()).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn request_dir() -> Result<PathBuf> {
    let local = dirs::data_local_dir().context("LOCALAPPDATA is unavailable")?;
    Ok(local.join("hypercolor").join("helper-requests"))
}

fn next_nonce() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let base = u64::try_from(chrono::Utc::now().timestamp_millis().max(1)).unwrap_or(u64::MAX);
    let bump = COUNTER.fetch_add(1, Ordering::Relaxed);
    base.saturating_add(bump)
}

/// PowerShell one-liner that elevates the helper and propagates its exit
/// code back through the outer powershell.exe so `Command::status()` sees
/// the real helper result.
///
/// Paths are interpolated as PowerShell single-quoted strings with any
/// embedded apostrophes doubled. This shuts the door on PS injection via
/// the path even though both inputs are derived from internal sources.
fn build_elevation_command(helper: &Path, request: &Path) -> String {
    let helper_q = ps_quote(&helper.display().to_string());
    let request_q = ps_quote(&request.display().to_string());
    format!(
        "$p = Start-Process -FilePath {helper_q} \
         -ArgumentList @('--request-file',{request_q}) \
         -Verb RunAs -Wait -PassThru -WindowStyle Hidden; \
         exit $p.ExitCode"
    )
}

fn ps_quote(raw: &str) -> String {
    let escaped = raw.replace('\'', "''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_quote_escapes_internal_apostrophes() {
        assert_eq!(ps_quote("C:\\path"), "'C:\\path'");
        assert_eq!(ps_quote("it's"), "'it''s'");
        assert_eq!(ps_quote("two''here"), "'two''''here'");
    }

    #[test]
    fn elevation_command_quotes_both_paths_and_propagates_exit_code() {
        let helper = PathBuf::from(r"C:\Program Files\Hypercolor\hypercolor-windows-helper.exe");
        let request =
            PathBuf::from(r"C:\Users\x\AppData\Local\hypercolor\helper-requests\42.json");
        let cmd = build_elevation_command(&helper, &request);
        assert!(
            cmd.contains("'C:\\Program Files\\Hypercolor\\hypercolor-windows-helper.exe'"),
            "missing quoted helper path: {cmd}"
        );
        assert!(
            cmd.contains("'C:\\Users\\x\\AppData\\Local\\hypercolor\\helper-requests\\42.json'"),
            "missing quoted request path: {cmd}"
        );
        assert!(cmd.contains("-Verb RunAs"), "missing UAC verb: {cmd}");
        assert!(
            cmd.contains("exit $p.ExitCode"),
            "missing exit-code propagation: {cmd}"
        );
    }

    #[test]
    fn resolve_helper_path_prefers_resource_dir_when_env_unset() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tools = temp.path().join("tools");
        std::fs::create_dir_all(&tools).expect("mkdir tools");
        let helper = tools.join(HELPER_BINARY_NAME);
        std::fs::write(&helper, b"stub").expect("write stub");

        let resolved = resolve_helper_path(Some(temp.path()));
        assert_eq!(resolved.as_deref(), Some(helper.as_path()));
    }

    // No "returns None when nothing nearby" test: the dev fallback in
    // resolve_helper_path walks up from the running test exe under
    // `target/.../deps/` and will legitimately find a sibling
    // `hypercolor-windows-helper.exe` whenever the workspace has been
    // built. That's the desired prod-dev behavior; the test premise of
    // "nothing nearby" doesn't survive a normal build.

    #[test]
    fn nonce_is_monotonic_within_process() {
        let a = next_nonce();
        let b = next_nonce();
        assert!(b > a, "expected b > a (got a={a}, b={b})");
    }

    #[test]
    fn verb_serializes_kebab_case_matching_helper_enum() {
        let json = serde_json::to_string(&Verb::RepairSmbusService).expect("serialize");
        assert_eq!(json, "\"repair-smbus-service\"");
    }
}
