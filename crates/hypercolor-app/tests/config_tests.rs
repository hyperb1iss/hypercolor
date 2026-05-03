//! Sanity checks for the Tauri config file shipped with hypercolor-app.
//!
//! These tests ensure the bundled `tauri.conf.json` parses as valid JSON
//! and carries the metadata the Tauri runtime expects at startup. They do
//! not spawn a Tauri app; they only read the file from the manifest dir.

use std::fs;
use std::path::{Path, PathBuf};

fn tauri_config() -> serde_json::Value {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tauri.conf.json");
    let text = fs::read_to_string(&path).expect("tauri.conf.json should be readable");
    serde_json::from_str(&text).expect("tauri.conf.json should be valid JSON")
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
