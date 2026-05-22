//! One-click diagnostic bundle export for support.
//!
//! Gathers the app + daemon logs, system identification, PawnIO support
//! status, and (on Windows) the output of `diagnose-windows.ps1` into a
//! single timestamped zip on the user's Desktop. Designed to be invoked
//! from the tray menu — no daemon API roundtrip required.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::Local;

use crate::logging;
use crate::supervisor;

/// Build a diagnostics zip on the user's Desktop and return its path.
///
/// # Errors
///
/// Returns an error if the staging directory cannot be created, no
/// destination directory is writable, or the platform's archive tool
/// fails to produce the zip.
pub fn export_to_desktop() -> Result<PathBuf> {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let stage_root = std::env::temp_dir().join(format!("hypercolor-diagnostics-{stamp}"));
    if stage_root.exists() {
        let _ = fs::remove_dir_all(&stage_root);
    }
    fs::create_dir_all(&stage_root)
        .with_context(|| format!("create diagnostics stage `{}`", stage_root.display()))?;

    // Pull in log directories — both files we wrote and platform log dirs.
    copy_log_files(&stage_root)?;

    // Gather system identification (always cheap, always useful).
    let system_info = collect_system_info();
    write_text(&stage_root.join("system-info.txt"), &system_info)?;

    // Run the platform diagnostics probe, if any.
    if let Some(probe_output) = run_platform_probe() {
        write_text(&stage_root.join("platform-probe.txt"), &probe_output)?;
    }

    // Resolve destination — Desktop preferred, then user data dir.
    let destination = preferred_destination_dir()?;
    let zip_path = destination.join(format!("hypercolor-diagnostics-{stamp}.zip"));

    create_zip(&stage_root, &zip_path)?;

    // Best-effort cleanup of the staging directory.
    let _ = fs::remove_dir_all(&stage_root);

    Ok(zip_path)
}

fn copy_log_files(stage_root: &Path) -> Result<()> {
    let logs_dir = stage_root.join("logs");
    fs::create_dir_all(&logs_dir)
        .with_context(|| format!("create staged logs dir `{}`", logs_dir.display()))?;

    let app_log_dir = logging::log_dir();
    if app_log_dir.is_dir() {
        copy_dir_recent(&app_log_dir, &logs_dir, 10)?;
    }

    // Daemon-supervised log lives alongside the other app logs (see
    // supervisor::daemon_log_file) but is appended to across spawn cycles —
    // still in the same dir, no separate copy needed.

    // PawnIO helper log (Phase 1.0 helper, when wired) lives in ProgramData.
    if cfg!(target_os = "windows") {
        if let Ok(program_data) = std::env::var("PROGRAMDATA") {
            let helper_log = PathBuf::from(program_data).join("hypercolor/helper.log");
            if helper_log.is_file() {
                let _ = fs::copy(&helper_log, logs_dir.join("helper.log"));
            }
        }
    }

    Ok(())
}

/// Copy the N most-recently-modified files from `src` into `dst`.
/// Skips subdirectories — we only want the rolling log files themselves.
fn copy_dir_recent(src: &Path, dst: &Path, limit: usize) -> Result<()> {
    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = fs::read_dir(src)
        .with_context(|| format!("read log dir `{}`", src.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();
    entries.sort_by(|left, right| right.1.cmp(&left.1));
    for (path, _) in entries.into_iter().take(limit) {
        if let Some(name) = path.file_name() {
            let _ = fs::copy(&path, dst.join(name));
        }
    }
    Ok(())
}

fn collect_system_info() -> String {
    let mut info = String::new();
    let _ = writeln!(info, "Hypercolor diagnostics bundle");
    let _ = writeln!(info, "Generated: {}", Local::now().to_rfc3339());
    let _ = writeln!(info, "App version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(info, "Daemon URL (default): {}", crate::DEFAULT_DAEMON_URL);
    let _ = writeln!(info, "Daemon executable: {}", supervisor::daemon_executable_name());
    let _ = writeln!(info, "Target OS: {}", std::env::consts::OS);
    let _ = writeln!(info, "Target arch: {}", std::env::consts::ARCH);
    if let Some(pid) = std::env::var_os("PROCESSOR_IDENTIFIER") {
        let _ = writeln!(info, "Processor: {}", pid.to_string_lossy());
    }
    if let Ok(motherboard) = serde_json::to_string_pretty(&hypercolor_core::system::motherboard_info()) {
        let _ = writeln!(info, "\nMotherboard:\n{motherboard}");
    }
    info
}

#[cfg(target_os = "windows")]
fn run_platform_probe() -> Option<String> {
    // Locate the bundled diagnose-windows.ps1 the same way detect_pawnio_support
    // resolves helper assets: relative to the app exe, or the repo for `just dev`.
    let exe = std::env::current_exe().ok()?;
    let candidates = [
        exe.parent()?.join("tools").join("diagnose-windows.ps1"),
        exe.parent()?.join("scripts").join("diagnose-windows.ps1"),
        exe.parent()?.parent()?.join("scripts").join("diagnose-windows.ps1"),
        // Repo layout: target/debug/hypercolor-app.exe → repo/scripts/...
        exe.parent()?.parent()?.parent()?.join("scripts").join("diagnose-windows.ps1"),
    ];
    let script = candidates.iter().find(|path| path.is_file())?;

    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .output()
        .ok()?;

    let mut buf = String::new();
    let _ = writeln!(buf, "$ powershell.exe -File {}", script.display());
    let _ = writeln!(buf, "exit code: {}", output.status.code().unwrap_or(-1));
    let _ = writeln!(buf, "\n--- stdout ---");
    buf.push_str(&String::from_utf8_lossy(&output.stdout));
    let _ = writeln!(buf, "\n--- stderr ---");
    buf.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(buf)
}

#[cfg(not(target_os = "windows"))]
fn run_platform_probe() -> Option<String> {
    None
}

fn write_text(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content)
        .with_context(|| format!("write `{}`", path.display()))?;
    Ok(())
}

fn preferred_destination_dir() -> Result<PathBuf> {
    if let Some(desktop) = dirs::desktop_dir()
        && desktop.is_dir()
    {
        return Ok(desktop);
    }
    if let Some(home) = dirs::home_dir() {
        return Ok(home);
    }
    bail!("could not locate Desktop or home directory for diagnostics output");
}

#[cfg(target_os = "windows")]
fn create_zip(stage_root: &Path, zip_path: &Path) -> Result<()> {
    // PowerShell's Compress-Archive is built-in on Windows 10+ and avoids
    // pulling in a Rust zip-crate dep just for this one path.
    let pattern = format!("{}\\*", stage_root.display());
    let status = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(format!(
            "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
            pattern,
            zip_path.display()
        ))
        .status()
        .with_context(|| "spawn powershell Compress-Archive")?;
    if !status.success() {
        bail!(
            "Compress-Archive exited with status {}",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn create_zip(stage_root: &Path, zip_path: &Path) -> Result<()> {
    // tar is universally available on Linux/macOS dev machines; produces
    // a .zip equivalent for support-bundle purposes.
    let status = Command::new("zip")
        .arg("-r")
        .arg(zip_path)
        .arg(".")
        .current_dir(stage_root)
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => bail!("zip exited with status {}", status.code().unwrap_or(-1)),
        Err(error) => bail!("spawn zip: {error}"),
    }
}
