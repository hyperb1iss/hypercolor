const HOMEBREW_CASK: &str = include_str!("../../../packaging/homebrew/hypercolor-app.rb");
const CI_WORKFLOW: &str = include_str!("../../../.github/workflows/ci.yml");

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
