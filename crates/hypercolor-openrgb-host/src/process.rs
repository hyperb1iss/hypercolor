//! Build the headless OpenRGB server launch.

use crate::detect::FLATPAK_APP_ID;
use crate::types::{BinaryKind, ManagedConfigDir, OpenRgbBinary, ProcessSpec};

/// The only host the managed server may bind. The SDK has no authentication,
/// so OpenRGB's `0.0.0.0` default is never acceptable.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// OpenRGB `--loglevel` for the managed server (4 = info without verbose spam).
pub const SERVER_LOG_LEVEL: u8 = 4;

/// Launch spec for a headless OpenRGB SDK server on loopback.
///
/// Arguments, in order: `--server --server-host 127.0.0.1 --server-port
/// <port> --noautoconnect --config <dir> --loglevel 4`. Flatpak builds run
/// through `flatpak run --filesystem=<dir> org.openrgb.OpenRGB` so the
/// sandbox can read and write the managed config directory.
#[must_use]
pub fn server_command(
    binary: &OpenRgbBinary,
    config_dir: &ManagedConfigDir,
    port: u16,
) -> ProcessSpec {
    let config_path = config_dir.root.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::new();
    if binary.kind == BinaryKind::Flatpak {
        args.push("run".to_owned());
        args.push(format!("--filesystem={config_path}"));
        args.push(FLATPAK_APP_ID.to_owned());
    }
    args.extend(server_args(&config_path, port));
    ProcessSpec {
        program: binary.path.clone(),
        args,
        env: std::collections::BTreeMap::default(),
        cwd: None,
    }
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
