const HOMEBREW_CASK: &str = include_str!("../../../packaging/homebrew/hypercolor-app.rb");
const CI_WORKFLOW: &str = include_str!("../../../.github/workflows/ci.yml");
const JUSTFILE: &str = include_str!("../../../justfile");
const WINDOWS_INSTALLER_SCRIPT: &str = include_str!("../../../scripts/build-windows-installer.ps1");
const FETCH_PAWNIO_ASSETS_PS1: &str = include_str!("../../../scripts/fetch-pawnio-assets.ps1");
const INSTALL_BUNDLED_PAWNIO_PS1: &str =
    include_str!("../../../scripts/install-bundled-pawnio.ps1");
const INSTALL_PAWNIO_MODULES_PS1: &str =
    include_str!("../../../scripts/install-pawnio-modules.ps1");
const STAGE_APP_BUNDLE_PS1: &str = include_str!("../../../scripts/stage-app-bundle-assets.ps1");
const STAGE_APP_BUNDLE_SH: &str = include_str!("../../../scripts/stage-app-bundle-assets.sh");

const WINDOWS_TOOL_SCRIPTS: &[&str] = &[
    "install-windows-service.ps1",
    "uninstall-windows-service.ps1",
    "diagnose-windows.ps1",
    "install-windows-smbus-service.ps1",
    "install-pawnio-modules.ps1",
    "install-bundled-pawnio.ps1",
    "install-windows-hardware-support.ps1",
];

const REQUIRED_PAWNIO_MODULES: &[&str] = &["SmbusI801.bin", "SmbusPIIX4.bin", "SmbusNCT6793.bin"];

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
fn app_bundle_staging_includes_windows_support_helpers() {
    for script in WINDOWS_TOOL_SCRIPTS {
        assert!(
            STAGE_APP_BUNDLE_PS1.contains(script),
            "PowerShell staging should bundle {script}"
        );
        assert!(
            STAGE_APP_BUNDLE_SH.contains(script),
            "Bash staging should bundle {script}"
        );
    }

    assert!(STAGE_APP_BUNDLE_PS1.contains("hypercolor-smbus-service"));
    assert!(STAGE_APP_BUNDLE_SH.contains("hypercolor-smbus-service"));
}

#[test]
fn app_bundle_staging_includes_pawnio_runtime_payloads() {
    assert!(STAGE_APP_BUNDLE_PS1.contains("fetch-pawnio-assets.ps1"));
    assert!(STAGE_APP_BUNDLE_PS1.contains("'pawnio'"));

    assert!(STAGE_APP_BUNDLE_SH.contains("PawnIO_setup.exe"));
    assert!(STAGE_APP_BUNDLE_SH.contains("manifest.json"));
    for module in REQUIRED_PAWNIO_MODULES {
        assert!(
            STAGE_APP_BUNDLE_SH.contains(module),
            "Bash staging should bundle PawnIO module {module}"
        );
    }
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
fn justfile_exposes_single_windows_installer_target() {
    assert!(JUSTFILE.contains("windows-installer *args=''"));
    assert!(JUSTFILE.contains("scripts/build-windows-installer.ps1"));
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
    ] {
        assert!(
            WINDOWS_INSTALLER_SCRIPT.contains(required),
            "Windows installer script should include {required}"
        );
    }
}
