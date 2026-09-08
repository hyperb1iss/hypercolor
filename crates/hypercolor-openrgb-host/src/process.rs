//! Build the headless OpenRGB server launch.

use crate::detect::FLATPAK_APP_ID;
use crate::error::{HostError, Result};
use crate::types::{BinaryKind, ManagedConfigDir, OpenRgbBinary, ProcessSpec};

/// The only host the managed server may bind. The SDK has no authentication,
/// so OpenRGB's `0.0.0.0` default (still the default in 1.0rc3) is never
/// acceptable; the host is always passed explicitly.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// OpenRGB `--loglevel` for the managed server (4 = info without verbose spam).
pub const SERVER_LOG_LEVEL: u8 = 4;

/// Launch spec for a headless OpenRGB SDK server on loopback.
///
/// Arguments, in order: `--server --server-host 127.0.0.1 --server-port
/// <port> --noautoconnect --config <dir> --loglevel 4`. Flatpak builds run
/// through `flatpak run --filesystem=<dir> org.openrgb.OpenRGB` so the
/// sandbox can read and write the managed config directory.
///
/// The supervisor must create the config directory (normally by writing the
/// detector partition) before launching: `flatpak --filesystem=` silently
/// ignores a path that does not exist yet.
///
/// # Errors
///
/// Returns [`HostError::UnsupportedConfigPath`] when the directory is not
/// valid UTF-8 (OpenRGB receives arguments as text) or, for Flatpak, when it
/// contains `:`, which `--filesystem=` treats as an option separator.
pub fn server_command(
    binary: &OpenRgbBinary,
    config_dir: &ManagedConfigDir,
    port: u16,
) -> Result<ProcessSpec> {
    let Some(config_path) = config_dir.root.to_str() else {
        return Err(HostError::UnsupportedConfigPath {
            path: config_dir.root.clone(),
            reason: "path is not valid UTF-8",
        });
    };
    let mut args: Vec<String> = Vec::new();
    if binary.kind == BinaryKind::Flatpak {
        if config_path.contains(':') {
            return Err(HostError::UnsupportedConfigPath {
                path: config_dir.root.clone(),
                reason: "flatpak --filesystem= treats ':' as an option separator",
            });
        }
        args.push("run".to_owned());
        args.push(format!("--filesystem={config_path}"));
        args.push(FLATPAK_APP_ID.to_owned());
    }
    args.extend(server_args(config_path, port));
    Ok(ProcessSpec {
        program: binary.path.clone(),
        args,
        env: std::collections::BTreeMap::default(),
        cwd: None,
    })
}

/// The OpenRGB-side arguments shared by every packaging.
#[must_use]
pub fn server_args(config_path: &str, port: u16) -> Vec<String> {
    vec![
        "--server".to_owned(),
        "--server-host".to_owned(),
        LOOPBACK_HOST.to_owned(),
        "--server-port".to_owned(),
        port.to_string(),
        "--noautoconnect".to_owned(),
        "--config".to_owned(),
        config_path.to_owned(),
        "--loglevel".to_owned(),
        SERVER_LOG_LEVEL.to_string(),
    ]
}
