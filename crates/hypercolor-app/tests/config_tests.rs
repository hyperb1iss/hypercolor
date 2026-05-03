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

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
