const HOMEBREW_CASK: &str = include_str!("../../../packaging/homebrew/hypercolor-app.rb");
const CI_WORKFLOW: &str = include_str!("../../../.github/workflows/ci.yml");
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
