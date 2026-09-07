//! Locate an OpenRGB installation on the host.
//!
//! Detection walks `PATH`, then the per-platform install locations, then
//! asks `flatpak` about the Flathub build. Version reads shell out to the
//! binary with a hard timeout so a wedged OpenRGB never stalls the caller.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, trace};

use crate::types::{BinaryKind, OpenRgbBinary};

/// Flatpak application id of the Flathub OpenRGB build.
pub const FLATPAK_APP_ID: &str = "org.openrgb.OpenRGB";

/// Upper bound on any subprocess this module spawns while detecting.
pub const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(3);

/// Executable names to look for on `PATH`, most specific first.
#[must_use]
pub const fn executable_names() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &["OpenRGB.exe", "openrgb.exe"]
    }
    #[cfg(not(target_os = "windows"))]
    {
        &["openrgb", "OpenRGB"]
    }
}

/// Find the first executable named by `names` in a `PATH`-style value.
///
/// `path_value` is the raw `PATH` string (or `None` for an unset variable),
/// so callers and tests can inject a value instead of reading the process
/// environment.
#[must_use]
pub fn find_in_path(names: &[&str], path_value: Option<&OsStr>) -> Option<PathBuf> {
    let path_value = path_value?;
    for dir in std::env::split_paths(path_value) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in names {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                trace!(path = %candidate.display(), "found executable on PATH");
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether `path` names a regular file the current user could execute.
#[must_use]
pub fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Classify a discovered executable by its packaging.
#[must_use]
pub fn classify_binary(path: &Path) -> BinaryKind {
    let is_appimage = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("appimage"));
    if is_appimage {
        BinaryKind::AppImage
    } else {
        BinaryKind::Native
    }
}

/// Well-known install locations for the current platform, in priority order.
///
/// Entries are candidates, not guarantees: callers still check existence.
#[must_use]
pub fn known_locations() -> Vec<PathBuf> {
    let home = home_dir();
    #[cfg(target_os = "windows")]
    {
        let mut locations = Vec::new();
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(variable) {
                locations.push(PathBuf::from(root).join("OpenRGB").join("OpenRGB.exe"));
            }
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            locations.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("OpenRGB")
                    .join("OpenRGB.exe"),
            );
        }
        let _ = home;
        locations
    }
    #[cfg(target_os = "macos")]
    {
        let bundle_suffix = Path::new("OpenRGB.app/Contents/MacOS/OpenRGB");
        let mut locations = vec![PathBuf::from("/Applications").join(bundle_suffix)];
        if let Some(home) = home {
            locations.push(home.join("Applications").join(bundle_suffix));
        }
        locations.push(PathBuf::from("/opt/homebrew/bin/openrgb"));
        locations.push(PathBuf::from("/usr/local/bin/openrgb"));
        locations
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut locations = vec![
            PathBuf::from("/usr/bin/openrgb"),
            PathBuf::from("/usr/local/bin/openrgb"),
        ];
        if let Some(home) = home {
            locations.push(home.join(".local/bin/openrgb"));
        }
        locations
    }
}

/// Directories scanned for a portable `OpenRGB*.AppImage` on Linux.
#[must_use]
pub fn appimage_search_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        home_dir()
            .map(|home| {
                vec![
                    home.join("Applications"),
                    home.join(".local/bin"),
                    home.join("bin"),
                ]
            })
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Find an `OpenRGB*.AppImage` in one directory.
///
/// When several match, the newest wins: by the version parsed from the file
/// name (`OpenRGB_1.0rc3_x86_64.AppImage` beats `openrgb_0.9_x86_64.appimage`
/// regardless of case), then by modification time, then by name.
#[must_use]
pub fn find_appimage_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut matches: Vec<AppImageCandidate> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_ascii_lowercase();
            (name.starts_with("openrgb")
                && name.ends_with(".appimage")
                && is_executable_file(&path))
            .then(|| AppImageCandidate {
                version: appimage_version_key(&name),
                modified: std::fs::metadata(&path).and_then(|m| m.modified()).ok(),
                name,
                path,
            })
        })
        .collect();
    matches.sort();
    matches.pop().map(|candidate| candidate.path)
}

/// Sort key for AppImage candidates: field order is the precedence order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AppImageCandidate {
    version: Option<Vec<u64>>,
    modified: Option<std::time::SystemTime>,
    name: String,
    path: PathBuf,
}

/// Numeric version components parsed from an AppImage file name.
///
/// `openrgb_1.0rc3_x86_64.appimage` yields `[1, 0, 3]`; a name without a
/// dotted version segment (`openrgb_x86_64.appimage`) yields `None`.
#[must_use]
pub fn appimage_version_key(lowercase_name: &str) -> Option<Vec<u64>> {
    let stem = lowercase_name.strip_suffix(".appimage")?;
    let segment = stem.split(['_', '-']).skip(1).find(|segment| {
        segment.starts_with(|c: char| c.is_ascii_digit()) && segment.contains('.')
    })?;
    let numbers: Vec<u64> = segment
        .split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .filter_map(|run| run.parse().ok())
        .collect();
    (!numbers.is_empty()).then_some(numbers)
}

/// Detect an OpenRGB installation, preferring native binaries over Flatpak.
///
/// Order: `PATH`, platform install locations, portable AppImages, then the
/// Flathub build when `flatpak` is available. The filesystem walk runs on
/// tokio's blocking pool so it never stalls the async runtime; the version
/// is read with [`SUBPROCESS_TIMEOUT`] and left `None` when OpenRGB does not
/// answer.
pub async fn detect_binary() -> Option<OpenRgbBinary> {
    let (native, flatpak) = match tokio::task::spawn_blocking(|| {
        let path = std::env::var_os("PATH");
        (
            find_native_binary(),
            find_in_path(&["flatpak"], path.as_deref()),
        )
    })
    .await
    {
        Ok(found) => found,
        Err(error) => {
            debug!(%error, "openrgb detection task failed to join");
            return None;
        }
    };

    if let Some(path) = native {
        let kind = classify_binary(&path);
        let version = read_version(&path).await;
        debug!(path = %path.display(), ?kind, ?version, "detected OpenRGB binary");
        return Some(OpenRgbBinary {
            path,
            kind,
            version,
        });
    }

    let flatpak = flatpak?;
    let version = flatpak_app_version(&flatpak).await?;
    debug!(?version, "detected OpenRGB Flatpak");
    Some(OpenRgbBinary {
        path: flatpak,
        kind: BinaryKind::Flatpak,
        version,
    })
}

/// Locate a native or AppImage OpenRGB executable without spawning anything.
///
/// This walks the filesystem synchronously; call it from a blocking context
/// or through `spawn_blocking` (as [`detect_binary`] does).
#[must_use]
pub fn find_native_binary() -> Option<PathBuf> {
    if let Some(path) = find_in_path(executable_names(), std::env::var_os("PATH").as_deref()) {
        return Some(path);
    }
    if let Some(path) = known_locations()
        .into_iter()
        .find(|path| is_executable_file(path))
    {
        return Some(path);
    }
    appimage_search_dirs()
        .iter()
        .find_map(|dir| find_appimage_in(dir))
}

/// Run `<binary> --version` and parse the reported version.
pub async fn read_version(binary: &Path) -> Option<String> {
    let output = run_with_timeout(Command::new(binary).arg("--version")).await?;
    parse_version_output(&output)
}

/// Query `flatpak info` for the OpenRGB app.
///
/// Returns `None` when the app is not installed, `Some(version)` otherwise
/// (with an inner `None` when the version line is missing).
pub async fn flatpak_app_version(flatpak: &Path) -> Option<Option<String>> {
    let output = run_with_timeout(Command::new(flatpak).args(["info", FLATPAK_APP_ID])).await?;
    Some(parse_flatpak_info_version(&output))
}

/// Extract the version from OpenRGB's `--version` output.
///
/// OpenRGB 1.0rc3 prints `OpenRGB 0.9+ (1.0rc3), for controlling RGB
/// lighting.` followed by `Version:\t\t 0.9+ (1.0rc3)`. The parenthesised
/// release tag is the version users recognise, so it wins when present; the
/// first numeric token (`0.9`, `0.9+`) is the fallback for older builds.
#[must_use]
pub fn parse_version_output(output: &str) -> Option<String> {
    let version_line = output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Version:"))
        .or_else(|| output.lines().find(|line| line.contains("OpenRGB")))
        .or_else(|| output.lines().next())?;
    parse_version_line(version_line)
}

fn parse_version_line(line: &str) -> Option<String> {
    let release_tag = line
        .split('(')
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .map(str::trim)
        .find(|tag| tag.starts_with(|c: char| c.is_ascii_digit()));
    if let Some(tag) = release_tag {
        return Some(tag.to_owned());
    }
    line.split_whitespace()
        .map(|token| token.trim_matches(|c: char| c == ',' || c == '(' || c == ')'))
        .find(|token| token.starts_with(|c: char| c.is_ascii_digit()) && token.contains('.'))
        .map(str::to_owned)
}

/// Extract the `Version:` field from `flatpak info` output.
#[must_use]
pub fn parse_flatpak_info_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Version:")?;
        let version = rest.trim();
        (!version.is_empty()).then(|| version.to_owned())
    })
}

async fn run_with_timeout(command: &mut Command) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let program = format!("{command:?}");
    match timeout(SUBPROCESS_TIMEOUT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            Some(text)
        }
        Ok(Ok(output)) => {
            debug!(%program, status = ?output.status, "subprocess exited unsuccessfully");
            None
        }
        Ok(Err(error)) => {
            debug!(%program, %error, "subprocess failed to spawn");
            None
        }
        Err(_) => {
            debug!(%program, ?SUBPROCESS_TIMEOUT, "subprocess timed out");
            None
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let variable = "USERPROFILE";
    #[cfg(not(target_os = "windows"))]
    let variable = "HOME";
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
