#[cfg(unix)]
use std::process::Command;

const CARGO_CONFIG: &str = include_str!("../../../.cargo/config.toml");
const GET_INSTALLER: &str = include_str!("../../../scripts/get-hypercolor.sh");
const HOMEBREW_FORMULA: &str = include_str!("../../../packaging/homebrew/hypercolor.rb");
const HOMEBREW_CASK: &str = include_str!("../../../packaging/homebrew/hypercolor-app.rb");
const CI_WORKFLOW: &str = include_str!("../../../.github/workflows/ci.yml");
const JUSTFILE: &str = include_str!("../../../justfile");
const WINDOWS_INSTALLER_SCRIPT: &str = include_str!("../../../scripts/build-windows-installer.ps1");
const DIST_SH: &str = include_str!("../../../scripts/dist.sh");
const INSTALL_SH: &str = include_str!("../../../scripts/install.sh");
const BUILD_MAC_INSTALLER_SH: &str = include_str!("../../../scripts/build-mac-installer.sh");
const CARGO_CACHE_BUILD_SH: &str = include_str!("../../../scripts/cargo-cache-build.sh");
const CARGO_CACHE_BUILD_PS1: &str = include_str!("../../../scripts/cargo-cache-build.ps1");
const CARGO_TARGET_GC_SH: &str = include_str!("../../../scripts/cargo-target-gc.sh");
const CARGO_TARGET_GC_SERVICE: &str =
    include_str!("../../../packaging/systemd/user/hypercolor-cargo-target-gc.service");
const CARGO_TARGET_GC_TIMER: &str =
    include_str!("../../../packaging/systemd/user/hypercolor-cargo-target-gc.timer");
const BRAND_BUILD_PY: &str = include_str!("../../../assets/brand/build.py");
const DIAGNOSE_WINDOWS_PS1: &str = include_str!("../../../scripts/diagnose-windows.ps1");
const FETCH_PAWNIO_ASSETS_PS1: &str = include_str!("../../../scripts/fetch-pawnio-assets.ps1");
const INSTALL_BUNDLED_PAWNIO_PS1: &str =
    include_str!("../../../scripts/install-bundled-pawnio.ps1");
const INSTALL_RELEASE_SH: &str = include_str!("../../../scripts/install-release.sh");
const INSTALL_PAWNIO_MODULES_PS1: &str =
    include_str!("../../../scripts/install-pawnio-modules.ps1");
const INSTALL_WINDOWS_SERVICE_PS1: &str =
    include_str!("../../../scripts/install-windows-service.ps1");
const INSTALL_WINDOWS_SMBUS_SERVICE_PS1: &str =
    include_str!("../../../scripts/install-windows-smbus-service.ps1");
const PACKAGE_DEB_SH: &str = include_str!("../../../scripts/package-deb.sh");
const VERIFY_DEB_SH: &str = include_str!("../../../scripts/verify-deb-package.sh");
const VERIFY_MACOS_DEPLOYMENT_TARGET_SH: &str =
    include_str!("../../../scripts/verify-macos-deployment-target.sh");
const STAGE_APP_BUNDLE_PS1: &str = include_str!("../../../scripts/stage-app-bundle-assets.ps1");
const STAGE_APP_BUNDLE_SH: &str = include_str!("../../../scripts/stage-app-bundle-assets.sh");
const INSTALLER_HOOKS_NSH: &str = include_str!("../installer-hooks.nsh");
const INSTALL_WINDOWS_HARDWARE_SUPPORT_PS1: &str =
    include_str!("../../../scripts/install-windows-hardware-support.ps1");

const WINDOWS_TOOL_SCRIPTS: &[&str] = &[
    "install-windows-service.ps1",
    "uninstall-windows-service.ps1",
    "diagnose-windows.ps1",
    "install-windows-smbus-service.ps1",
    "install-pawnio-modules.ps1",
    "install-bundled-pawnio.ps1",
    "install-windows-hardware-support.ps1",
];

const REQUIRED_PAWNIO_MODULES: &[&str] = &[
    "SmbusI801.bin",
    "SmbusPIIX4.bin",
    "SmbusNCT6793.bin",
    "IntelMSR.bin",
    "AMDFamily17.bin",
];

#[test]
fn macos_distribution_surfaces_require_15_2() {
    assert!(
        CARGO_CONFIG.contains(r#"MACOSX_DEPLOYMENT_TARGET = { value = "15.2", force = true }"#)
    );
    assert!(HOMEBREW_FORMULA.contains(r#"depends_on macos: ">= :sequoia""#));
    assert!(HOMEBREW_FORMULA.contains("MacOS.version >= Version.new(\"15.2\")"));
    assert!(HOMEBREW_CASK.contains(r#"depends_on macos: ">= :sequoia""#));
    assert!(HOMEBREW_CASK.contains("MacOS.version < Version.new(\"15.2\")"));
    assert!(GET_INSTALLER.contains("require_supported_macos"));
}

#[cfg(unix)]
#[test]
fn curl_installer_compares_macos_versions_by_numeric_component() {
    let (_, function_tail) = GET_INSTALLER
        .split_once("macos_version_supported() {")
        .expect("installer should define macos_version_supported");
    let (function_body, _) = function_tail
        .split_once("# ── Argument Parsing")
        .expect("version helper should precede argument parsing");
    let script =
        format!("macos_version_supported() {{{function_body}\nmacos_version_supported \"$1\"");

    for (version, supported) in [
        ("14.9", false),
        ("15.0", false),
        ("15.1", false),
        ("15.2", true),
        ("15.10", true),
        ("26.0", true),
        ("26.10", true),
    ] {
        let status = Command::new("bash")
            .args(["-c", &script, "--", version])
            .status()
            .expect("bash should execute the installer version helper");
        assert_eq!(
            status.success(),
            supported,
            "unexpected support result for macOS {version}"
        );
    }
}

#[test]
fn macos_distribution_covers_arm64_and_amd64() {
    for expected in ["macos-arm64", "macos-amd64"] {
        assert!(CI_WORKFLOW.contains(&format!("target: {expected}")));
        assert!(GET_INSTALLER.contains(expected));
        assert!(INSTALL_RELEASE_SH.contains(expected));
        assert!(HOMEBREW_FORMULA.contains(expected));
    }

    assert!(CI_WORKFLOW.contains("os: macos-26"));
    assert!(CI_WORKFLOW.contains("os: macos-26-intel"));
    assert!(CI_WORKFLOW.contains("SHA256_MACOS_AMD64"));
    assert!(HOMEBREW_FORMULA.contains("SHA256_MACOS_AMD64"));
    assert!(HOMEBREW_FORMULA.contains("keep_alive successful_exit: false"));
}

#[test]
fn macos_release_verifier_pins_every_macho_to_15_2() {
    assert!(VERIFY_MACOS_DEPLOYMENT_TARGET_SH.contains("xcrun vtool -show-build"));
    assert!(VERIFY_MACOS_DEPLOYMENT_TARGET_SH.contains("LC_BUILD_VERSION minos"));
    assert!(VERIFY_MACOS_DEPLOYMENT_TARGET_SH.contains("expected 15.2"));
    assert!(VERIFY_MACOS_DEPLOYMENT_TARGET_SH.contains("no Mach-O files found"));
}

#[test]
fn ci_qualifies_both_macos_architectures_with_xcode_26() {
    assert!(CI_WORKFLOW.contains("rust-check-macos:"));
    assert!(CI_WORKFLOW.contains("os: macos-26"));
    assert!(CI_WORKFLOW.contains("os: macos-26-intel"));
    assert!(CI_WORKFLOW.contains("XCODE_VERSION: \"26.5\""));
    assert!(CI_WORKFLOW.contains("xcodebuild -version"));
    assert!(CI_WORKFLOW.contains("xcrun --show-sdk-version"));
    assert!(CI_WORKFLOW.contains("test \"${sdk_version%%.*}\" = \"26\""));
}

#[test]
fn ci_audits_pr_and_release_macho_deployment_targets() {
    assert!(CI_WORKFLOW.contains("cargo check --workspace --locked"));
    assert!(CI_WORKFLOW.contains("cargo nextest run --locked -p hypercolor-macos-gpu-interop"));
    assert!(CI_WORKFLOW.contains("cargo build --locked -p hypercolor-cli --bin hypercolor"));
    assert_eq!(
        CI_WORKFLOW
            .matches("./scripts/verify-macos-deployment-target.sh")
            .count(),
        3
    );
}

#[test]
fn homebrew_cask_template_targets_normalized_macos_dmg_names() {
    assert!(HOMEBREW_CASK.contains(r#"cask "hypercolor-app" do"#));
    assert!(HOMEBREW_CASK.contains(r#"arch arm: "arm64", intel: "x86_64""#));
    assert!(HOMEBREW_CASK.contains("VERSION_PLACEHOLDER"));
    assert!(HOMEBREW_CASK.contains("SHA256_MACOS_APP_ARM64"));
    assert!(HOMEBREW_CASK.contains("SHA256_MACOS_APP_X86_64"));
    assert!(
        HOMEBREW_CASK.contains("Hypercolor-#{version}-#{arch}.dmg"),
        "cask URL should use the normalized release DMG name"
    );
    assert!(HOMEBREW_CASK.contains(r#"app "Hypercolor.app""#));
}

#[test]
fn ci_normalizes_macos_dmg_artifacts_for_cask_urls() {
    assert!(CI_WORKFLOW.contains("Normalize macOS DMG artifact name"));
    assert!(CI_WORKFLOW.contains("cask_arch: arm64"));
    assert!(CI_WORKFLOW.contains("cask_arch: x86_64"));
    assert!(CI_WORKFLOW.contains("Hypercolor-$version-$arch.dmg"));
}

#[test]
fn ci_publishes_homebrew_formula_and_cask() {
    assert!(CI_WORKFLOW.contains("packaging/homebrew/hypercolor.rb > hypercolor.rb"));
    assert!(CI_WORKFLOW.contains("packaging/homebrew/hypercolor-app.rb > hypercolor-app.rb"));
    assert!(CI_WORKFLOW.contains("sha256_macos_app_arm64"));
    assert!(CI_WORKFLOW.contains("sha256_macos_app_x86_64"));
    assert!(CI_WORKFLOW.contains("tap/Casks"));
    assert!(CI_WORKFLOW.contains("Casks/hypercolor-app.rb"));
}

#[test]
fn ci_builds_debian_packages_for_linux_release_artifacts() {
    assert!(CI_WORKFLOW.contains("Build Debian package"));
    assert!(CI_WORKFLOW.contains("Verify Debian package"));
    assert!(CI_WORKFLOW.contains("./scripts/package-deb.sh"));
    assert!(CI_WORKFLOW.contains("./scripts/verify-deb-package.sh"));
    assert!(CI_WORKFLOW.contains("hypercolor-deb-${{ steps.version.outputs.version }}"));
}

#[test]
fn ci_windows_native_bundle_applies_windows_overlay() {
    assert!(CI_WORKFLOW.contains("tauri.windows.bundle.conf.json"));
    assert!(CI_WORKFLOW.contains("$env:RUNNER_OS -eq \"Windows\""));
}

#[test]
fn app_bundle_staging_includes_windows_support_helpers() {
    let tauri_config = include_str!("../tauri.conf.json");
    for script in WINDOWS_TOOL_SCRIPTS {
        assert!(
            tauri_config.contains(script),
            "tauri.conf.json should reference workspace tool script {script}"
        );
    }

    assert!(STAGE_APP_BUNDLE_PS1.contains("hypercolor-smbus-service"));
    assert!(STAGE_APP_BUNDLE_PS1.contains("hypercolor-windows-helper"));
    assert!(STAGE_APP_BUNDLE_SH.contains("hypercolor-smbus-service"));
    assert!(STAGE_APP_BUNDLE_SH.contains("hypercolor-windows-helper"));
}

#[test]
fn app_bundle_staging_includes_pawnio_runtime_payloads() {
    assert!(STAGE_APP_BUNDLE_PS1.contains("fetch-pawnio-assets.ps1"));
    assert!(STAGE_APP_BUNDLE_PS1.contains("'pawnio'"));

    assert!(STAGE_APP_BUNDLE_SH.contains("PawnIO_setup.exe"));
    assert!(STAGE_APP_BUNDLE_SH.contains("PawnIO.Modules.zip"));
    assert!(STAGE_APP_BUNDLE_SH.contains("manifest.json"));
    for module in REQUIRED_PAWNIO_MODULES {
        for (script_name, script) in [
            ("diagnose-windows.ps1", DIAGNOSE_WINDOWS_PS1),
            ("fetch-pawnio-assets.ps1", FETCH_PAWNIO_ASSETS_PS1),
            ("install-bundled-pawnio.ps1", INSTALL_BUNDLED_PAWNIO_PS1),
            ("install-pawnio-modules.ps1", INSTALL_PAWNIO_MODULES_PS1),
            ("install-windows-service.ps1", INSTALL_WINDOWS_SERVICE_PS1),
            (
                "install-windows-smbus-service.ps1",
                INSTALL_WINDOWS_SMBUS_SERVICE_PS1,
            ),
            ("stage-app-bundle-assets.sh", STAGE_APP_BUNDLE_SH),
        ] {
            assert!(
                script.contains(module),
                "{script_name} should include PawnIO module {module}"
            );
        }
    }
}

/// 0.2.1 shipped PawnIO without the broker that loads its modules, so CPU
/// package temperature and motherboard SMBus lighting were dead on every
/// Windows install with nothing prompting for the rights to fix it.
#[test]
fn installer_hook_provisions_the_whole_hardware_access_stack() {
    assert!(
        INSTALLER_HOOKS_NSH.contains("install-windows-hardware-support.ps1"),
        "postinstall must run the orchestrator that installs PawnIO *and* the SMBus broker"
    );
    assert!(
        INSTALLER_HOOKS_NSH.contains(r#"-BrokerExe "$INSTDIR\tools\hypercolor-smbus-service.exe""#),
        "the broker must be registered from its Program Files path"
    );
    assert!(
        INSTALLER_HOOKS_NSH.contains("-ReinstallService"),
        "upgrades must not trip the broker installer's existing-registration guard"
    );
}

/// The broker installer rejects service binaries and PawnIO directories under
/// per-user profile paths, since a LocalSystem service must not load code the
/// user can rewrite. Everything the hook hands it therefore lives in $INSTDIR.
#[test]
fn installer_hook_keeps_privileged_paths_administrator_owned() {
    for flag in ["-AssetRoot", "-BrokerExe", "-ModuleDestination"] {
        let value = INSTALLER_HOOKS_NSH
            .split_once(&format!("{flag} \""))
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once('"'))
            .map(|(value, _)| value)
            .unwrap_or_else(|| panic!("installer hook should pass {flag}"));
        assert!(
            value.starts_with("$INSTDIR\\"),
            "{flag} must stay under $INSTDIR, got {value}"
        );
    }
}

/// Splatting an array into a PowerShell *script* binds every element
/// positionally, so `@("-AssetRoot", $path)` puts the literal string
/// "-AssetRoot" in the child's first parameter and drops every switch on the
/// floor. Only native executables parse "-Name value" pairs out of a splatted
/// array; scripts need a hashtable. This silently broke both the installer
/// path and the app's "Install support" button.
#[test]
fn hardware_support_orchestrator_splats_by_hashtable_not_array() {
    for splat in ["$pawnIoArgs", "$serviceArgs"] {
        let opener = format!("{splat} = @");
        let (_, rest) = INSTALL_WINDOWS_HARDWARE_SUPPORT_PS1
            .split_once(opener.as_str())
            .unwrap_or_else(|| panic!("orchestrator should build {splat}"));
        assert!(
            rest.starts_with('{'),
            "{splat} must be a hashtable; an array splat binds positionally \
             and silently mis-assigns every named parameter"
        );
    }
}

#[test]
fn installer_hook_cleans_up_the_broker_on_uninstall() {
    assert!(INSTALLER_HOOKS_NSH.contains("sc.exe stop HypercolorSmBus"));
    assert!(INSTALLER_HOOKS_NSH.contains("sc.exe delete HypercolorSmBus"));
}

#[test]
fn brand_build_pipeline_mirrors_nsis_assets_to_tauri_icons() {
    assert!(BRAND_BUILD_PY.contains("crates\" / \"hypercolor-app\" / \"icons"));
    for asset in ["installer.ico", "nsis-header.bmp", "nsis-sidebar.bmp"] {
        assert!(
            BRAND_BUILD_PY.contains(asset),
            "brand build pipeline should mirror {asset} for Tauri bundling"
        );
    }
    assert!(BRAND_BUILD_PY.contains("shutil.copy2"));
}

#[test]
fn pawnio_scripts_hash_without_requiring_get_file_hash() {
    for script in [
        FETCH_PAWNIO_ASSETS_PS1,
        INSTALL_BUNDLED_PAWNIO_PS1,
        INSTALL_PAWNIO_MODULES_PS1,
    ] {
        assert!(script.contains("function Get-Sha256"));
        assert!(script.contains("Get-Command \"Get-FileHash\""));
        assert!(script.contains("System.Security.Cryptography.SHA256"));
    }

    assert!(FETCH_PAWNIO_ASSETS_PS1.contains("Get-Sha256 $Path"));
    assert!(FETCH_PAWNIO_ASSETS_PS1.contains("Get-Sha256 $modulePath"));
    assert!(INSTALL_BUNDLED_PAWNIO_PS1.contains("Get-Sha256 $Path"));
    assert!(INSTALL_PAWNIO_MODULES_PS1.contains("Get-Sha256 $zip"));
}

#[test]
fn bundled_pawnio_installer_uses_embedded_trust_roots() {
    assert!(INSTALL_BUNDLED_PAWNIO_PS1.contains("$PawnIoSetupSha256 ="));
    assert!(INSTALL_BUNDLED_PAWNIO_PS1.contains("$PawnIoModulesZipSha256 ="));
    assert!(INSTALL_BUNDLED_PAWNIO_PS1.contains(
        "Assert-FileHash (Join-Path $AssetRoot \"PawnIO_setup.exe\") $PawnIoSetupSha256"
    ));
    assert!(INSTALL_BUNDLED_PAWNIO_PS1.contains(
        "Assert-FileHash (Join-Path $AssetRoot \"PawnIO.Modules.zip\") $PawnIoModulesZipSha256"
    ));
    assert!(!INSTALL_BUNDLED_PAWNIO_PS1.contains("ConvertFrom-Json"));
    assert!(!INSTALL_BUNDLED_PAWNIO_PS1.contains("manifest.json"));
}

#[test]
fn justfile_exposes_single_windows_installer_target() {
    assert!(JUSTFILE.contains("windows-installer *args=''"));
    assert!(JUSTFILE.contains("scripts/build-windows-installer.ps1"));
}

#[test]
fn local_build_wrappers_default_to_workspace_target_dir() {
    assert!(CARGO_CACHE_BUILD_SH.contains(r#"TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}""#));
    assert!(CARGO_CACHE_BUILD_SH.contains("unset CARGO_TARGET_DIR"));
    assert!(CARGO_CACHE_BUILD_SH.contains("--target-dir \"$TARGET_DIR\""));
    assert!(CARGO_CACHE_BUILD_SH.contains("SCCACHE_SERVER_UDS"));
    assert!(CARGO_CACHE_BUILD_SH.contains("collect_sccache_basedirs"));
    assert!(CARGO_CACHE_BUILD_PS1.contains("$env:CARGO_TARGET_DIR = Join-Path $RepoRoot 'target'"));
    assert!(STAGE_APP_BUNDLE_SH.contains(r#"STAGE_DIR="${ROOT_DIR}/target/bundle-stage""#));
    assert!(
        STAGE_APP_BUNDLE_PS1.contains("$StageDir = Join-Path $RepoRoot 'target\\bundle-stage'")
    );
}

#[test]
fn ui_cargo_builds_use_the_shared_cache_policy() {
    assert!(JUSTFILE.contains(
        r#"HYPERCOLOR_ITERATE=1 env -u NO_COLOR ../../scripts/cargo-cache-build.sh trunk serve"#
    ));
    assert!(JUSTFILE.contains(
        r#"HYPERCOLOR_FORCE_SCCACHE=1 env -u NO_COLOR ../../scripts/cargo-cache-build.sh trunk build"#
    ));
    assert!(CARGO_CACHE_BUILD_SH.contains("HYPERCOLOR_NESTED_CARGO_TARGET_DIR"));
}

#[test]
fn unix_app_bundles_use_the_shared_cache_policy() {
    assert!(JUSTFILE.contains(
        r#"HYPERCOLOR_FORCE_SCCACHE=1 ../../scripts/cargo-cache-build.sh cargo tauri build"#
    ));
    for script in [DIST_SH, INSTALL_SH, BUILD_MAC_INSTALLER_SH] {
        assert!(script.contains("cargo-cache-build.sh"));
        assert!(script.contains("trunk build --release --locked"));
    }
    assert!(
        BUILD_MAC_INSTALLER_SH
            .contains(r#"HYPERCOLOR_FORCE_SCCACHE=1 "${CARGO_CACHE_BUILD}" cargo"#)
    );
}

#[test]
fn cargo_target_gc_is_pressure_triggered_and_lock_aware() {
    for required in [
        "HYPERCOLOR_GC_HIGH_WATER_BYTES",
        "HYPERCOLOR_GC_LOW_WATER_BYTES",
        "HYPERCOLOR_GC_MIN_AGE_DAYS",
        "HYPERCOLOR_GC_RECLAIM_MIN_AGE_SECONDS",
        "status --porcelain=v1",
        ".cargo-lock",
        "flock -n",
        "clear_profile_preserving_locks",
    ] {
        assert!(
            CARGO_TARGET_GC_SH.contains(required),
            "Cargo target GC should enforce {required}"
        );
    }
    assert!(CARGO_TARGET_GC_SERVICE.contains("Nice=19"));
    assert!(CARGO_TARGET_GC_SERVICE.contains("IOSchedulingClass=idle"));
    assert!(CARGO_TARGET_GC_SERVICE.contains("TimeoutStartSec=45min"));
    assert!(CARGO_TARGET_GC_TIMER.contains("OnStartupSec=30min"));
    assert!(CARGO_TARGET_GC_TIMER.contains("OnUnitActiveSec=1d"));
    assert!(!CARGO_TARGET_GC_TIMER.contains("Persistent=true"));
}

#[test]
fn windows_installer_target_builds_all_bundle_inputs() {
    for required in [
        "cargo tauri --version",
        "Build production UI",
        "Build bundled effects",
        "hypercolor-daemon",
        "hypercolor-cli",
        "hypercolor-windows-pawnio",
        "hypercolor-smbus-service",
        "stage-app-bundle-assets.ps1",
        "\"cargo\"",
        "\"tauri\", \"build\"",
        "--bundles",
        "$env:CARGO_TARGET_DIR",
        "[Environment]::GetFolderPath(\"UserProfile\")",
        ".cache\\hypercolor\\target\\release\\bundle\\nsis",
    ] {
        assert!(
            WINDOWS_INSTALLER_SCRIPT.contains(required),
            "Windows installer script should include {required}"
        );
    }
}

#[test]
fn debian_package_script_maps_release_payload() {
    for required in [
        "linux-amd64) DEB_ARCH=\"amd64\"",
        "linux-arm64) DEB_ARCH=\"arm64\"",
        "install -Dm755 \"${DIST_DIR}/bin/hypercolor-daemon\"",
        "${PACKAGE_ROOT}/usr/lib/systemd/user/hypercolor.service",
        "${PACKAGE_ROOT}/usr/lib/udev/rules.d/99-hypercolor.rules",
        "${PACKAGE_ROOT}/usr/lib/modules-load.d/i2c-dev.conf",
        "libwebkit2gtk-4.1-0",
    ] {
        assert!(
            PACKAGE_DEB_SH.contains(required),
            "Debian package script should include {required}"
        );
    }
}

#[test]
fn debian_verifier_checks_package_payload() {
    for required in [
        "dpkg-deb --field",
        "require_path \"./usr/bin/hypercolor-daemon\"",
        "require_path \"./usr/share/hypercolor/ui/index.html\"",
        "require_path \"./usr/lib/systemd/user/hypercolor.service\"",
        "libwebkit2gtk-4.1-0",
    ] {
        assert!(
            VERIFY_DEB_SH.contains(required),
            "Debian verifier should include {required}"
        );
    }
}
