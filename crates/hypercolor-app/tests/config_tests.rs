//! Sanity checks for the Tauri config file shipped with hypercolor-app.
//!
//! These tests ensure the bundled `tauri.conf.json` parses as valid JSON
//! and carries the metadata the Tauri runtime expects at startup. They do
//! not spawn a Tauri app; they only read the file from the manifest dir.

use std::fs;
use std::path::{Path, PathBuf};

fn tauri_config() -> serde_json::Value {
    config_json("tauri.conf.json")
}

fn tauri_bundle_config() -> serde_json::Value {
    config_json("tauri.bundle.conf.json")
}

fn config_json(file_name: &str) -> serde_json::Value {
    let mut path = manifest_dir();
    path.push(file_name);
    let text = fs::read_to_string(&path).expect("tauri.conf.json should be readable");
    serde_json::from_str(&text).expect("tauri config should be valid JSON")
}

fn default_capability() -> serde_json::Value {
    let path = manifest_dir().join("capabilities").join("default.json");
    let text = fs::read_to_string(&path).expect("default capability should be readable");
    serde_json::from_str(&text).expect("default capability should be valid JSON")
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn default_capability_grants_window_and_autostart_permissions() {
    let capability = default_capability();
    let permissions = capability
        .get("permissions")
        .and_then(serde_json::Value::as_array)
        .expect("permissions should be an array");

    for expected in [
        "core:default",
        "autostart:allow-enable",
        "autostart:allow-disable",
        "autostart:allow-is-enabled",
        "core:tray:default",
        "core:window:allow-show",
        "core:window:allow-hide",
        "core:window:allow-set-focus",
        "core:window:allow-unminimize",
    ] {
        assert!(
            permissions.iter().any(|value| value == expected),
            "capability should include {expected}"
        );
    }
}

#[test]
fn default_capability_allows_local_daemon_remote_ipc() {
    let capability = default_capability();
    let urls = capability
        .get("remote")
        .and_then(|remote| remote.get("urls"))
        .and_then(serde_json::Value::as_array)
        .expect("remote.urls should be configured");

    for expected in ["http://127.0.0.1:9420/*", "http://localhost:9420/*"] {
        assert!(
            urls.iter().any(|value| value == expected),
            "capability should allow IPC from {expected}"
        );
    }
}

#[test]
fn tauri_config_is_valid_json() {
    let _ = tauri_config();
}

#[test]
fn tauri_config_has_product_metadata() {
    let config = tauri_config();
    assert!(
        config.get("productName").is_some(),
        "productName must be set"
    );
    assert!(config.get("version").is_some(), "version must be set");
    assert!(config.get("identifier").is_some(), "identifier must be set");
}

#[test]
fn tauri_config_has_bundle_config() {
    let config = tauri_config();
    assert!(config.get("bundle").is_some(), "bundle config must be set");
}

#[test]
fn tauri_config_declares_installer_targets() {
    let config = tauri_config();
    let targets = config
        .get("bundle")
        .and_then(|bundle| bundle.get("targets"))
        .and_then(serde_json::Value::as_array)
        .expect("bundle.targets should be an array");

    for expected in ["nsis", "dmg", "app"] {
        assert!(
            targets.iter().any(|target| target == expected),
            "bundle.targets should include {expected}"
        );
    }
}

#[test]
fn tauri_config_prefers_current_user_nsis_installs() {
    let config = tauri_config();
    let install_mode = config
        .get("bundle")
        .and_then(|bundle| bundle.get("windows"))
        .and_then(|windows| windows.get("nsis"))
        .and_then(|nsis| nsis.get("installMode"))
        .and_then(serde_json::Value::as_str);

    assert_eq!(install_mode, Some("currentUser"));
}

#[test]
fn tauri_config_declares_dmg_layout() {
    let config = tauri_config();
    let dmg = config
        .get("bundle")
        .and_then(|bundle| bundle.get("macOS"))
        .and_then(|macos| macos.get("dmg"))
        .expect("bundle.macOS.dmg should be configured");

    assert!(dmg.get("windowSize").is_some());
    assert!(dmg.get("appPosition").is_some());
    assert!(dmg.get("applicationFolderPosition").is_some());
}

#[test]
fn tauri_config_declares_macos_hardened_runtime_metadata() {
    let config = tauri_config();
    let macos = config
        .get("bundle")
        .and_then(|bundle| bundle.get("macOS"))
        .expect("bundle.macOS should be configured");

    assert_eq!(
        macos
            .get("hardenedRuntime")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );

    for (key, file_name) in [
        ("entitlements", "entitlements.plist"),
        ("infoPlist", "Info.plist"),
    ] {
        assert_eq!(
            macos.get(key).and_then(serde_json::Value::as_str),
            Some(file_name)
        );
        assert!(
            manifest_dir().join(file_name).exists(),
            "configured macOS bundle file should exist: {file_name}"
        );
    }
}

#[test]
fn macos_bundle_plists_declare_required_permissions() {
    let root = manifest_dir();
    let entitlements = fs::read_to_string(root.join("entitlements.plist"))
        .expect("entitlements.plist should be readable");
    let info_plist =
        fs::read_to_string(root.join("Info.plist")).expect("Info.plist should be readable");

    for key in [
        "com.apple.security.cs.allow-jit",
        "com.apple.security.cs.allow-unsigned-executable-memory",
        "com.apple.security.network.client",
        "com.apple.security.network.server",
        "com.apple.security.device.audio-input",
        "com.apple.security.device.usb",
    ] {
        assert!(
            entitlements.contains(key),
            "entitlements.plist should declare {key}"
        );
    }

    for key in [
        "NSMicrophoneUsageDescription",
        "NSAppleEventsUsageDescription",
    ] {
        assert!(info_plist.contains(key), "Info.plist should declare {key}");
    }
}

#[test]
fn tauri_config_declares_sidecar_binaries() {
    let config = tauri_bundle_config();
    let external_bins = config
        .get("bundle")
        .and_then(|bundle| bundle.get("externalBin"))
        .and_then(serde_json::Value::as_array)
        .expect("bundle.externalBin should be an array");

    for expected in [
        "../../target/bundle-stage/binaries/hypercolor-daemon",
        "../../target/bundle-stage/binaries/hypercolor",
    ] {
        assert!(
            external_bins.iter().any(|bin| bin == expected),
            "bundle.externalBin should include {expected}"
        );
    }
}

#[test]
fn tauri_config_declares_workspace_resources() {
    let config = tauri_config();
    let resources = config
        .get("bundle")
        .and_then(|bundle| bundle.get("resources"))
        .and_then(serde_json::Value::as_object)
        .expect("bundle.resources should be a map");
    let root = manifest_dir();

    let ui_target = resources
        .get("../../crates/hypercolor-ui/dist/")
        .and_then(serde_json::Value::as_str);
    assert_eq!(ui_target, Some("ui/"));

    let effects_target = resources
        .get("../../effects/hypercolor/")
        .and_then(serde_json::Value::as_str);
    assert_eq!(effects_target, Some("effects/bundled/"));

    for script in [
        "install-windows-service.ps1",
        "uninstall-windows-service.ps1",
        "diagnose-windows.ps1",
        "install-windows-smbus-service.ps1",
        "install-pawnio-modules.ps1",
        "install-bundled-pawnio.ps1",
        "install-windows-hardware-support.ps1",
    ] {
        let source = format!("../../scripts/{script}");
        let target = format!("tools/{script}");
        assert_eq!(
            resources.get(&source).and_then(serde_json::Value::as_str),
            Some(target.as_str()),
            "bundle.resources should map {source} -> {target}"
        );
        assert!(
            root.join(Path::new(&source)).exists(),
            "tool script should exist on disk: {source}"
        );
    }
}

#[test]
fn tauri_windows_bundle_config_layers_pawnio_resources() {
    let config = config_json("tauri.windows.bundle.conf.json");
    let resources = config
        .get("bundle")
        .and_then(|bundle| bundle.get("resources"))
        .and_then(serde_json::Value::as_object)
        .expect("windows bundle resources should be a map");

    for (source, target) in [
        (
            "../../target/bundle-stage/tools/hypercolor-smbus-service.exe",
            "tools/hypercolor-smbus-service.exe",
        ),
        (
            "../../target/bundle-stage/tools/pawnio/",
            "tools/pawnio/",
        ),
    ] {
        assert_eq!(
            resources.get(source).and_then(serde_json::Value::as_str),
            Some(target),
            "windows bundle should map {source} -> {target}"
        );
    }
}

#[test]
fn tauri_config_icon_files_exist() {
    let config = tauri_config();
    let icons = config
        .get("bundle")
        .and_then(|bundle| bundle.get("icon"))
        .and_then(serde_json::Value::as_array)
        .expect("bundle.icon should be an array");
    let root = manifest_dir();

    for icon in icons {
        let icon = icon
            .as_str()
            .expect("bundle icon entries should be strings");
        let path = root.join(Path::new(icon));
        assert!(path.exists(), "configured icon should exist: {icon}");
    }
}

#[test]
fn tauri_config_identifier_is_reverse_dns() {
    let config = tauri_config();
    let identifier = config
        .get("identifier")
        .and_then(|v| v.as_str())
        .expect("identifier should be a string");
    assert!(
        identifier.contains('.'),
        "identifier should use reverse-DNS form, got {identifier}"
    );
}

#[test]
fn tauri_config_has_app_section() {
    let config = tauri_config();
    assert!(
        config.get("app").is_some(),
        "app section must be set for window/security configuration"
    );
}
