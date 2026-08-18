//! Sanity checks for the Tauri config file shipped with hypercolor-app.
//!
//! These tests ensure the bundled `tauri.conf.json` parses as valid JSON
//! and carries the metadata the Tauri runtime expects at startup. They do
//! not spawn a Tauri app; they only read the file from the manifest dir.

use std::collections::BTreeMap;
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

fn repository_root() -> PathBuf {
    manifest_dir().join("../..")
}

fn plist_string_entries(plist: &str) -> BTreeMap<&str, &str> {
    let lines: Vec<_> = plist.lines().map(str::trim).collect();
    lines
        .windows(2)
        .filter_map(|pair| {
            let key = pair[0].strip_prefix("<key>")?.strip_suffix("</key>")?;
            let value = pair[1]
                .strip_prefix("<string>")?
                .strip_suffix("</string>")?;
            Some((key, value))
        })
        .collect()
}

fn plist_boolean_entries(plist: &str) -> BTreeMap<&str, bool> {
    let mut lines = plist.lines().map(str::trim);
    let mut entries = BTreeMap::new();

    while let Some(line) = lines.next() {
        let Some(key) = line
            .strip_prefix("<key>")
            .and_then(|key| key.strip_suffix("</key>"))
        else {
            continue;
        };
        let value_line = lines
            .next()
            .expect("plist keys should have a following value");
        let value = match value_line {
            "<true/>" => true,
            "<false/>" => false,
            other => panic!("plist key {key} should have a Boolean value, got {other}"),
        };
        assert!(
            entries.insert(key, value).is_none(),
            "plist key should be unique: {key}"
        );
    }

    entries
}

fn signing_manifest_entries(manifest: &str) -> BTreeMap<(&str, &str), (&str, &str)> {
    let mut entries = BTreeMap::new();

    for (line_index, line) in manifest.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split('\t');
        let scope = fields.next().expect("manifest rows should have a scope");
        let path = fields.next().expect("manifest rows should have a path");
        let identifier = fields
            .next()
            .expect("manifest rows should have an identifier");
        let entitlements = fields
            .next()
            .expect("manifest rows should have an entitlements profile");
        assert!(
            fields.next().is_none(),
            "manifest line {} should have exactly four fields",
            line_index + 1
        );
        assert!(
            entries
                .insert((scope, path), (identifier, entitlements))
                .is_none(),
            "manifest scope and path should be unique: {scope}/{path}"
        );
    }

    entries
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
fn default_capability_rejects_every_remote_ipc_origin() {
    let capability = default_capability();
    assert!(
        capability.get("remote").is_none(),
        "bundled-origin commands must not be authorized for any remote document"
    );
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
fn tauri_config_uses_per_machine_nsis_installs() {
    let config = tauri_config();
    let install_mode = config
        .get("bundle")
        .and_then(|bundle| bundle.get("windows"))
        .and_then(|windows| windows.get("nsis"))
        .and_then(|nsis| nsis.get("installMode"))
        .and_then(serde_json::Value::as_str);

    // Installer hooks perform one-shot PawnIO setup and firewall rule creation,
    // so NSIS must run elevated.
    assert_eq!(install_mode, Some("perMachine"));
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
fn tauri_config_requires_macos_15_2() {
    let config = tauri_config();
    let minimum_system_version = config
        .get("bundle")
        .and_then(|bundle| bundle.get("macOS"))
        .and_then(|macos| macos.get("minimumSystemVersion"))
        .and_then(serde_json::Value::as_str);

    assert_eq!(minimum_system_version, Some("15.2"));
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

    let expected_privacy_entries = BTreeMap::from([
        (
            "NSMicrophoneUsageDescription",
            "Hypercolor uses your microphone for audio-reactive lighting effects.",
        ),
        (
            "NSScreenCaptureUsageDescription",
            "Hypercolor captures your screen to create screen-reactive lighting effects.",
        ),
    ]);
    assert_eq!(
        plist_string_entries(&info_plist),
        expected_privacy_entries,
        "Info.plist should declare only the required privacy purpose strings"
    );
    assert!(
        !info_plist.contains("NSAppleEventsUsageDescription"),
        "Info.plist should not request unrelated Apple Events permission"
    );
}

#[test]
fn macos_daemon_signing_contract_is_exact() {
    let root = repository_root();
    let manifest = fs::read_to_string(root.join("packaging/macos/signing-manifest.tsv"))
        .expect("macOS signing manifest should be readable");
    let entitlements = fs::read_to_string(root.join("packaging/macos/daemon.entitlements.plist"))
        .expect("macOS daemon entitlements should be readable");

    let expected_manifest = BTreeMap::from([
        (
            ("app", "Contents/MacOS/Hypercolor"),
            (
                "tech.hyperbliss.hypercolor",
                "crates/hypercolor-app/entitlements.plist",
            ),
        ),
        (
            ("app", "Contents/MacOS/hypercolor-daemon-{target}"),
            (
                "tech.hyperbliss.hypercolor.sidecar",
                "packaging/macos/daemon.entitlements.plist",
            ),
        ),
        (
            ("app", "Contents/MacOS/hypercolor-{target}"),
            ("tech.hyperbliss.hypercolor.cli", "none"),
        ),
        (
            ("standalone", "bin/hypercolor-daemon"),
            (
                "tech.hyperbliss.hypercolor.daemon",
                "packaging/macos/daemon.entitlements.plist",
            ),
        ),
        (
            ("standalone", "bin/hypercolor"),
            ("tech.hyperbliss.hypercolor.cli", "none"),
        ),
        (
            ("standalone", "bin/hypercolor-app"),
            (
                "tech.hyperbliss.hypercolor.app-host",
                "crates/hypercolor-app/entitlements.plist",
            ),
        ),
        (
            ("standalone", "bin/hypercolor-tray"),
            ("tech.hyperbliss.hypercolor.tray", "none"),
        ),
    ]);
    assert_eq!(signing_manifest_entries(&manifest), expected_manifest);

    let expected_entitlements = BTreeMap::from([
        ("com.apple.security.cs.allow-jit", true),
        (
            "com.apple.security.cs.allow-unsigned-executable-memory",
            true,
        ),
        ("com.apple.security.cs.disable-library-validation", true),
        ("com.apple.security.device.audio-input", true),
        ("com.apple.security.device.usb", true),
        ("com.apple.security.network.client", true),
        ("com.apple.security.network.server", true),
    ]);
    assert_eq!(plist_boolean_entries(&entitlements), expected_entitlements);
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
fn tauri_bundle_config_declares_staged_web_resources() {
    let config = tauri_bundle_config();
    let resources = config
        .get("bundle")
        .and_then(|bundle| bundle.get("resources"))
        .and_then(serde_json::Value::as_object)
        .expect("bundle resources should be a map");

    for (source, target) in [
        ("../../target/bundle-stage/ui/", "ui/"),
        ("../../target/bundle-stage/effects/", "effects/bundled/"),
    ] {
        assert_eq!(
            resources.get(source).and_then(serde_json::Value::as_str),
            Some(target),
            "bundle resources should map {source} -> {target}"
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
            "../../target/bundle-stage/tools/hypercolor-windows-helper.exe",
            "tools/hypercolor-windows-helper.exe",
        ),
        ("../../target/bundle-stage/tools/pawnio/", "tools/pawnio/"),
        ("../../target/bundle-stage/dlls/libEGL.dll", "libEGL.dll"),
        (
            "../../target/bundle-stage/dlls/libGLESv2.dll",
            "libGLESv2.dll",
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
fn tauri_windows_bundle_config_uses_branded_nsis_assets() {
    let config = config_json("tauri.windows.bundle.conf.json");
    let nsis = config
        .get("bundle")
        .and_then(|bundle| bundle.get("windows"))
        .and_then(|windows| windows.get("nsis"))
        .and_then(serde_json::Value::as_object)
        .expect("windows nsis config should be a map");
    let root = manifest_dir();

    for (key, file_name) in [
        ("installerIcon", "icons/installer.ico"),
        ("uninstallerIcon", "icons/installer.ico"),
        ("headerImage", "icons/nsis-header.bmp"),
        ("sidebarImage", "icons/nsis-sidebar.bmp"),
    ] {
        assert_eq!(
            nsis.get(key).and_then(serde_json::Value::as_str),
            Some(file_name),
            "NSIS config should set {key}"
        );
        assert!(
            root.join(file_name).exists(),
            "configured NSIS asset should exist: {file_name}"
        );
    }

    assert_eq!(
        bitmap_dimensions(&root.join("icons/nsis-header.bmp")),
        (150, 57)
    );
    assert_eq!(
        bitmap_dimensions(&root.join("icons/nsis-sidebar.bmp")),
        (164, 314)
    );
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

fn bitmap_dimensions(path: &Path) -> (i32, i32) {
    let bytes = fs::read(path).expect("bitmap should be readable");
    assert!(bytes.len() >= 26, "bitmap header should be present");
    let width = i32::from_le_bytes(bytes[18..22].try_into().expect("width bytes"));
    let height = i32::from_le_bytes(bytes[22..26].try_into().expect("height bytes"));
    (width, height)
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

#[test]
fn tauri_config_exposes_global_tauri_api_for_bundled_ui_bridge() {
    let config = tauri_config();
    let with_global_tauri = config
        .get("app")
        .and_then(|app| app.get("withGlobalTauri"))
        .and_then(serde_json::Value::as_bool);

    assert_eq!(with_global_tauri, Some(true));
}
