#[cfg(unix)]
use std::{env, fs, os::unix::fs::PermissionsExt as _, path::Path, process::Command};

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
const UI_WINDOWS_PS1: &str = include_str!("../../../scripts/ui-windows.ps1");
const CARGO_TARGET_GC_SH: &str = include_str!("../../../scripts/cargo-target-gc.sh");
const CARGO_TARGET_GC_SERVICE: &str =
    include_str!("../../../packaging/systemd/user/hypercolor-cargo-target-gc.service");
const CARGO_TARGET_GC_TIMER: &str =
    include_str!("../../../packaging/systemd/user/hypercolor-cargo-target-gc.timer");
const RUN_MACOS_TCC_CANARY_SH: &str = include_str!("../../../scripts/run-macos-tcc-canary-row.sh");
const SYSTEMD_USER_UNIT: &str = include_str!("../../../packaging/systemd/user/hypercolor.service");
const SYSTEMD_PACKAGED_USER_UNIT: &str =
    include_str!("../../../packaging/systemd/user/hypercolor.service.system");
const WINDOWS_SERVICE_INSTALLER_PS1: &str =
    include_str!("../../../scripts/install-windows-service.ps1");
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
const VERIFY_RELEASE_SH: &str = include_str!("../../../scripts/verify-release-artifact.sh");
const VERIFY_MACOS_DEPLOYMENT_TARGET_SH: &str =
    include_str!("../../../scripts/verify-macos-deployment-target.sh");
const SIGN_MACOS_ARTIFACTS_SH: &str = include_str!("../../../scripts/sign-macos-artifacts.sh");
const MACOS_SIGNING_KEYCHAIN_C: &str = include_str!("../../../scripts/macos-signing-keychain.c");
const MACOS_SIGNING_MANIFEST: &str = include_str!("../../../packaging/macos/signing-manifest.tsv");
const TAURI_CONFIG: &str = include_str!("../tauri.conf.json");
const TAURI_MACOS_CONFIG: &str = include_str!("../tauri.macos.conf.json");
const TAURI_BUNDLE_CONFIG: &str = include_str!("../tauri.bundle.conf.json");
const TAURI_DEFAULT_CAPABILITY: &str = include_str!("../capabilities/default.json");
const TAURI_BUILD_RS: &str = include_str!("../build.rs");
const APP_MAIN_RS: &str = include_str!("../src/main.rs");
const MACOS_DAEMON_ENTITLEMENTS: &str =
    include_str!("../../../packaging/macos/daemon.entitlements.plist");
const MACOS_DAEMON_SIDECAR_ENTITLEMENTS: &str =
    include_str!("../../../packaging/macos/daemon-sidecar.entitlements.plist");
const MACOS_LAUNCHD_PLIST: &str =
    include_str!("../../../packaging/launchd/tech.hyperbliss.hypercolor.plist");
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

fn csp_directives(
    csp: &str,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    csp.split(';')
        .filter_map(|directive| {
            let mut fields = directive.split_whitespace();
            let name = fields.next()?;
            Some((
                name.to_owned(),
                fields
                    .map(str::to_owned)
                    .collect::<std::collections::BTreeSet<_>>(),
            ))
        })
        .collect()
}

fn csp_sources(values: &[&str]) -> std::collections::BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn app_window_is_bundled_and_never_accepts_daemon_document_bytes() {
    let config: serde_json::Value =
        serde_json::from_str(TAURI_CONFIG).expect("Tauri config should parse");
    let bundle_config: serde_json::Value =
        serde_json::from_str(TAURI_BUNDLE_CONFIG).expect("bundle config should parse");

    assert_eq!(config["build"]["frontendDist"], "../hypercolor-ui/dist");
    assert_eq!(
        bundle_config["build"]["frontendDist"],
        "../../target/bundle-stage/ui"
    );
    assert!(APP_MAIN_RS.contains("WebviewUrl::App(\"index.html\".into())"));
    assert!(!APP_MAIN_RS.contains("WebviewUrl::External"));
    assert!(!APP_MAIN_RS.contains("window.navigate"));
    assert!(!APP_MAIN_RS.contains("__HYPERCOLOR_DAEMON_BASE_URL__"));
    assert!(!APP_MAIN_RS.contains("initialization_script(daemon"));
}

#[test]
fn bundled_origin_capability_allows_exact_registered_commands_only() {
    let capability: serde_json::Value =
        serde_json::from_str(TAURI_DEFAULT_CAPABILITY).expect("default capability should parse");
    assert!(capability.get("remote").is_none());
    assert_eq!(capability["windows"], serde_json::json!(["main"]));

    let (_, build_commands) = TAURI_BUILD_RS
        .split_once(".commands(&[")
        .expect("build manifest should enumerate app commands");
    let (build_commands, _) = build_commands
        .split_once("]);")
        .expect("build manifest command list should close");
    let build_commands = build_commands
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix('"'))
        .filter_map(|line| line.strip_suffix("\","))
        .collect::<std::collections::BTreeSet<_>>();

    let (_, handlers) = APP_MAIN_RS
        .split_once("tauri::generate_handler![")
        .expect("app should register a command handler");
    let (handlers, _) = handlers
        .split_once("])")
        .expect("command handler list should close");
    let handlers = handlers
        .split(',')
        .filter_map(|entry| entry.trim().rsplit("::").next())
        .filter(|entry| !entry.is_empty())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(build_commands, handlers);

    let command_permissions = capability["permissions"]
        .as_array()
        .expect("permissions should be an array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|permission| permission.strip_prefix("allow-"))
        .map(|permission| permission.replace('-', "_"))
        .collect::<std::collections::BTreeSet<_>>();
    let build_commands = build_commands
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(command_permissions, build_commands);
}

#[test]
fn bundled_origin_csp_is_exact_and_macos_network_access_is_loopback_only() {
    let config: serde_json::Value =
        serde_json::from_str(TAURI_CONFIG).expect("Tauri config should parse");
    let macos: serde_json::Value =
        serde_json::from_str(TAURI_MACOS_CONFIG).expect("macOS Tauri config should parse");
    let security = &config["app"]["security"];
    let macos_security = &macos["app"]["security"];
    assert!(
        security
            .get("dangerousDisableAssetCspModification")
            .is_none()
    );
    assert!(
        macos_security
            .get("dangerousDisableAssetCspModification")
            .is_none()
    );

    let base = csp_directives(
        security["csp"]
            .as_str()
            .expect("base CSP should be a string"),
    );
    let macos = csp_directives(
        macos_security["csp"]
            .as_str()
            .expect("macOS CSP should be a string"),
    );
    let directive_names = csp_sources(&[
        "base-uri",
        "connect-src",
        "default-src",
        "font-src",
        "form-action",
        "frame-ancestors",
        "frame-src",
        "img-src",
        "media-src",
        "object-src",
        "script-src",
        "style-src",
        "worker-src",
    ]);
    assert_eq!(
        base.keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        directive_names
    );
    assert_eq!(
        macos
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        directive_names
    );

    for directives in [&base, &macos] {
        assert_eq!(directives["default-src"], csp_sources(&["'self'"]));
        assert_eq!(
            directives["script-src"],
            csp_sources(&["'self'", "'wasm-unsafe-eval'"])
        );
        assert_eq!(directives["worker-src"], csp_sources(&["'self'", "blob:"]));
        assert_eq!(
            directives["style-src"],
            csp_sources(&["'self'", "'unsafe-inline'", "https://fonts.bunny.net"])
        );
        assert_eq!(
            directives["font-src"],
            csp_sources(&["'self'", "data:", "https://fonts.bunny.net"])
        );
        for denied in [
            "object-src",
            "base-uri",
            "frame-src",
            "frame-ancestors",
            "form-action",
        ] {
            assert_eq!(directives[denied], csp_sources(&["'none'"]));
        }
    }

    assert_eq!(
        base["connect-src"],
        csp_sources(&[
            "'self'",
            "ipc:",
            "http://ipc.localhost",
            "http:",
            "https:",
            "ws:",
            "wss:",
        ])
    );
    assert_eq!(
        base["img-src"],
        csp_sources(&["'self'", "data:", "blob:", "http:", "https:"])
    );
    assert_eq!(base["media-src"], base["img-src"]);

    let loopback_http = [
        "http://127.0.0.1:*",
        "http://localhost:*",
        "http://[::1]:*",
        "https://127.0.0.1:*",
        "https://localhost:*",
        "https://[::1]:*",
    ];
    let mut macos_connect = csp_sources(&["'self'", "ipc:", "http://ipc.localhost"]);
    macos_connect.extend(loopback_http.iter().map(|source| (*source).to_owned()));
    macos_connect.extend(
        [
            "ws://127.0.0.1:*",
            "ws://localhost:*",
            "ws://[::1]:*",
            "wss://127.0.0.1:*",
            "wss://localhost:*",
            "wss://[::1]:*",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    assert_eq!(macos["connect-src"], macos_connect);
    let mut macos_media = csp_sources(&["'self'", "data:", "blob:"]);
    macos_media.extend(loopback_http.into_iter().map(str::to_owned));
    assert_eq!(macos["img-src"], macos_media);
    assert_eq!(macos["media-src"], macos["img-src"]);
}

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
fn macos_packaging_and_installers_cover_both_architectures() {
    assert!(CI_WORKFLOW.contains("target: macos-arm64"));
    assert!(CI_WORKFLOW.contains("target: macos-x64"));
    assert!(CI_WORKFLOW.contains("rust-target: aarch64-apple-darwin"));
    assert!(CI_WORKFLOW.contains("rust-target: x86_64-apple-darwin"));

    for expected in ["macos-arm64", "macos-amd64"] {
        assert!(GET_INSTALLER.contains(expected));
        assert!(INSTALL_RELEASE_SH.contains(expected));
        assert!(HOMEBREW_FORMULA.contains(expected));
    }

    assert!(CI_WORKFLOW.contains("os: macos-26"));
    assert!(CI_WORKFLOW.contains("os: macos-26-intel"));
    assert!(HOMEBREW_FORMULA.contains("SHA256_MACOS_AMD64"));
    assert!(HOMEBREW_FORMULA.contains("keep_alive successful_exit: false"));
    assert!(HOMEBREW_FORMULA.contains(r#""--macos-owner", "homebrew""#));
}

#[test]
fn macos_launchers_identify_their_daemon_topology() {
    let (_, launchd_arguments) = MACOS_LAUNCHD_PLIST
        .split_once("<key>ProgramArguments</key>")
        .expect("launchd plist should declare program arguments");
    let (launchd_arguments, _) = launchd_arguments
        .split_once("</array>")
        .expect("launchd argument array should close");
    assert_eq!(
        launchd_arguments
            .lines()
            .filter_map(|line| line.trim().strip_prefix("<string>"))
            .filter_map(|line| line.strip_suffix("</string>"))
            .collect::<Vec<_>>(),
        [
            "@BIN_DIR@/hypercolor-daemon",
            "--macos-owner",
            "direct-launchd",
            "--ui-dir",
            "@UI_DIR@",
        ]
    );

    let (_, launchd_environment) = MACOS_LAUNCHD_PLIST
        .split_once("<key>EnvironmentVariables</key>")
        .expect("launchd plist should declare environment variables");
    let (launchd_environment, _) = launchd_environment
        .split_once("</dict>")
        .expect("launchd environment dictionary should close");
    let launchd_environment = launchd_environment
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "<dict>")
        .collect::<Vec<_>>();
    assert_eq!(
        launchd_environment,
        [
            "<key>HYPERCOLOR_MACOS_OWNER</key>",
            "<string>direct-launchd</string>",
            "<key>HYPERCOLOR_SERVICE_IDENTITY</key>",
            "<string>user_service:launchd:tech.hyperbliss.hypercolor</string>",
            "<key>HYPERCOLOR_LOG</key>",
            "<string>info</string>",
            "<key>PATH</key>",
            "<string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:@BIN_DIR@</string>",
        ]
    );

    let (_, homebrew_service) = HOMEBREW_FORMULA
        .split_once("  service do\n")
        .expect("Homebrew formula should declare a service");
    let (homebrew_service, _) = homebrew_service
        .split_once("\n  end")
        .expect("Homebrew service block should close");
    assert_eq!(
        homebrew_service,
        concat!(
            "    run [opt_bin/\"hypercolor-daemon\", \"--macos-owner\", \"homebrew\", ",
            "\"--ui-dir\", share/\"hypercolor/ui\"]\n",
            "    keep_alive successful_exit: false\n",
            "    log_path var/\"log/hypercolor/hypercolor.log\"\n",
            "    error_log_path var/\"log/hypercolor/hypercolor.log\"\n",
            "    environment_variables HYPERCOLOR_LOG: \"info\", ",
            "HYPERCOLOR_MACOS_OWNER: \"homebrew\", ",
            "HYPERCOLOR_SERVICE_IDENTITY: \"user_service:homebrew:homebrew.mxcl.hypercolor\"",
        )
    );
}

#[test]
fn every_launcher_declares_a_neutral_service_identity() {
    assert!(
        SYSTEMD_USER_UNIT.contains(
            "Environment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service\n"
        ),
        "systemd user unit declares the user service identity"
    );
    assert!(
        SYSTEMD_PACKAGED_USER_UNIT.contains(
            "Environment=HYPERCOLOR_SERVICE_IDENTITY=user_service:systemd:hypercolor.service\n"
        ),
        "system-installed systemd user unit declares the user service identity"
    );
    assert!(
        WINDOWS_SERVICE_INSTALLER_PS1.contains(
            "$serviceEnvironment = @(\"HYPERCOLOR_SERVICE_IDENTITY=system_service:windows_scm:$ServiceName\")"
        ),
        "SCM registration declares the Windows service identity"
    );
    assert!(MACOS_LAUNCHD_PLIST.contains(
        "<key>HYPERCOLOR_SERVICE_IDENTITY</key>\n        <string>user_service:launchd:tech.hyperbliss.hypercolor</string>"
    ));
    assert!(HOMEBREW_FORMULA.contains(
        "HYPERCOLOR_SERVICE_IDENTITY: \"user_service:homebrew:homebrew.mxcl.hypercolor\""
    ));
}

const UNINSTALL_SH: &str = include_str!("../../../scripts/uninstall.sh");
const INSTALL_RELEASE_SH_FOR_LAUNCHD: &str = include_str!("../../../scripts/install-release.sh");

#[test]
fn launchd_uses_modern_verbs_and_one_app_agent_filename() {
    for (name, script) in [
        ("uninstall.sh", UNINSTALL_SH),
        ("install-release.sh", INSTALL_RELEASE_SH_FOR_LAUNCHD),
    ] {
        assert!(
            !script.contains("launchctl load") && !script.contains("launchctl unload"),
            "{name} must not use legacy launchctl load/unload"
        );
        assert!(
            script.contains("launchctl bootout \"gui/$(id -u)/"),
            "{name} boots the agent out of the user's gui domain"
        );
    }
    // The cask zaps the agent the CLI actually writes for the app.
    assert!(HOMEBREW_CASK.contains("\"~/Library/LaunchAgents/Hypercolor.plist\""));
    assert!(!HOMEBREW_CASK.contains("tech.hyperbliss.hypercolor.app.plist"));
}

#[test]
fn macos_launchd_conflict_exit_does_not_restart_the_losing_daemon() {
    assert!(MACOS_LAUNCHD_PLIST.contains(
        "<key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>"
    ));
    assert!(CI_WORKFLOW.contains("launchd_managed_contenders_exit_zero_without_respawn"));
}

#[test]
fn app_sidecar_identity_matches_tauri_and_signing_artifacts() {
    let config: serde_json::Value =
        serde_json::from_str(TAURI_CONFIG).expect("Tauri config should parse");
    assert_eq!(
        config["productName"],
        hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME
    );
    assert_eq!(
        hypercolor_macos_owner::MACOS_APP_LAUNCH_AGENT_PLIST_FILE_NAME,
        format!("{}.plist", hypercolor_macos_owner::MACOS_APP_PRODUCT_NAME)
    );
    assert!(MACOS_SIGNING_MANIFEST.lines().any(|line| {
        line.split('\t').nth(1)
            == Some(hypercolor_macos_owner::MACOS_APP_BUNDLE_EXECUTABLE_RELATIVE_PATH)
    }));
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
fn ci_installs_nasm_before_intel_macos_compilation() {
    let (_, macos_job) = CI_WORKFLOW
        .split_once("\n  rust-check-macos:\n")
        .expect("CI should define the macOS check job");
    let (macos_job, _) = macos_job
        .split_once("\n  generated-effects:\n")
        .expect("generated effects should follow the macOS check job");
    let install = macos_job
        .find("- name: Install NASM")
        .expect("Intel macOS checks should install NASM");
    let (_, install_and_after) = macos_job
        .split_once("- name: Install NASM\n")
        .expect("Intel macOS checks should install NASM");
    let (install_step, _) = install_and_after
        .split_once("\n\n      - ")
        .expect("another macOS step should follow NASM installation");
    let qualification = macos_job
        .find("- name: Qualify Intel Metal fixture")
        .expect("Intel macOS checks should qualify native Metal import");
    let workspace = macos_job
        .find("- name: Check macOS workspace")
        .expect("macOS checks should compile the workspace");

    assert!(install_step.contains("if: matrix.expected-arch == 'x86_64'"));
    assert!(install_step.contains("run: brew install nasm"));
    assert!(install < qualification);
    assert!(install < workspace);
}

#[test]
fn ci_runs_inline_and_integration_macos_capture_fixtures() {
    let (_, fixture_steps) = CI_WORKFLOW
        .split_once("- name: Run macOS capture fixtures")
        .expect("CI should run macOS capture fixtures");
    let (fixture_steps, _) = fixture_steps
        .split_once("- name: Run macOS host input and ownership fixtures")
        .expect("host input fixtures should follow capture fixtures");
    let (_, first_command_and_after) = fixture_steps
        .split_once("./scripts/cargo-cache-build.sh")
        .expect("capture fixtures should invoke the Cargo wrapper");
    let (capture_command, _) = first_command_and_after
        .split_once("\n          ./scripts/cargo-cache-build.sh")
        .expect("core capture fixtures should follow crate capture fixtures");

    assert!(capture_command.contains(
        "cargo nextest run --locked \\\n            -p hypercolor-macos-capture --features capture-fixtures"
    ));
    for selector in ["--lib", "--tests", "--test", "-E"] {
        assert!(
            !capture_command.contains(selector),
            "capture crate command must not select a target with {selector}"
        );
    }
    assert!(fixture_steps.contains("--test macos_screen_capture_tests"));
}

#[test]
fn ci_builds_pr_docs_without_widening_deployment_permissions() {
    let (_, docs_jobs) = CI_WORKFLOW
        .split_once("\n  docs-build:\n")
        .expect("CI should define an unprivileged docs build job");
    let (build_job, deploy_and_after) = docs_jobs
        .split_once("\n  docs-deploy:\n")
        .expect("CI should define a separate docs deployment job");
    let (deploy_job, _) = deploy_and_after
        .split_once("\n  web-assets:\n")
        .expect("web assets should follow the docs jobs");
    let (_, build_condition_and_after) = build_job
        .split_once("    if: >-\n")
        .expect("docs build should define a job condition");
    let (build_condition, _) = build_condition_and_after
        .split_once("    runs-on:")
        .expect("docs build condition should precede its runner");
    let (_, upload_and_after) = build_job
        .split_once("      - name: Upload Pages artifact\n")
        .expect("docs build should upload its Pages artifact");
    let upload_step = upload_and_after;
    let (_, upload_condition_and_after) = upload_step
        .split_once("        if: >-\n")
        .expect("Pages upload should define a condition");
    let (upload_condition, _) = upload_condition_and_after
        .split_once("        uses:")
        .expect("Pages upload condition should precede its action");
    let (_, deploy_condition_and_after) = deploy_job
        .split_once("    if: >-\n")
        .expect("docs deploy should define a job condition");
    let (deploy_condition, _) = deploy_condition_and_after
        .split_once("    runs-on:")
        .expect("docs deploy condition should precede its runner");
    let normalize = |condition: &str| condition.split_whitespace().collect::<Vec<_>>().join(" ");
    let expected_build = normalize(
        "(github.event_name == 'pull_request' && needs.changes.outputs.docs == 'true') ||
         (github.ref == 'refs/heads/main' && (
           (github.event_name == 'push' && needs.changes.outputs.docs == 'true') ||
           (github.event_name == 'workflow_dispatch' && inputs.deploy_docs)
         ))",
    );
    let expected_deploy = normalize(
        "github.ref == 'refs/heads/main' && (
           (github.event_name == 'push' && needs.changes.outputs.docs == 'true') ||
           (github.event_name == 'workflow_dispatch' && inputs.deploy_docs)
         )",
    );

    assert_eq!(normalize(build_condition), expected_build);
    assert!(build_job.contains("permissions:\n      contents: read"));
    assert!(build_job.contains("working-directory: docs\n        run: zola build"));
    assert!(!build_job.contains("pages: write"));
    assert!(!build_job.contains("id-token: write"));
    assert!(!build_job.contains("actions/deploy-pages"));
    assert!(upload_step.contains("uses: actions/upload-pages-artifact@v5"));
    assert!(upload_step.contains("path: docs/public"));

    assert!(deploy_job.contains("needs: [changes, docs-build]"));
    assert_eq!(normalize(upload_condition), expected_deploy);
    assert_eq!(normalize(deploy_condition), expected_deploy);
    assert!(deploy_job.contains("pages: write"));
    assert!(deploy_job.contains("id-token: write"));
    assert!(deploy_job.contains("actions/deploy-pages@v5"));
}

#[test]
fn public_ci_audits_pr_and_unsigned_app_macho_deployment_targets() {
    assert!(CI_WORKFLOW.contains("cargo check --workspace --locked"));
    assert!(CI_WORKFLOW.contains("cargo nextest run --locked -p hypercolor-macos-gpu-interop"));
    assert!(CI_WORKFLOW.contains("cargo build --locked -p hypercolor-cli --bin hypercolor"));
    assert_eq!(
        CI_WORKFLOW
            .matches("./scripts/verify-macos-deployment-target.sh")
            .count(),
        2
    );
}

#[test]
fn macos_signing_manifest_assigns_every_stable_identity() {
    for identifier in [
        "tech.hyperbliss.hypercolor",
        "tech.hyperbliss.hypercolor.sidecar",
        "tech.hyperbliss.hypercolor.daemon",
        "tech.hyperbliss.hypercolor.cli",
        "tech.hyperbliss.hypercolor.app-host",
    ] {
        assert!(MACOS_SIGNING_MANIFEST.contains(identifier));
    }
    assert!(MACOS_SIGNING_MANIFEST.contains("hypercolor-daemon-{target}"));
}

#[test]
fn macos_signing_actor_rejects_ad_hoc_and_unlisted_objects() {
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("ad-hoc signing identities are forbidden"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("matched ${matches} signing manifest entries"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("codesign --verify --strict"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("anchor apple generic"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("notarytool submit"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("stapler validate"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("tauri build --bundles app --no-sign"));
}

#[test]
fn macos_daemon_entitlements_preserve_the_seven_key_profile() {
    let keys = [
        "com.apple.security.cs.allow-jit",
        "com.apple.security.cs.allow-unsigned-executable-memory",
        "com.apple.security.cs.disable-library-validation",
        "com.apple.security.device.audio-input",
        "com.apple.security.device.usb",
        "com.apple.security.network.client",
        "com.apple.security.network.server",
    ];
    assert_eq!(
        MACOS_DAEMON_ENTITLEMENTS.matches("<key>").count(),
        keys.len()
    );
    assert_eq!(
        MACOS_DAEMON_ENTITLEMENTS.matches("<true/>").count(),
        keys.len()
    );
    for key in keys {
        assert!(MACOS_DAEMON_ENTITLEMENTS.contains(key));
    }
}

#[test]
fn macos_daemon_sidecar_alone_declares_automation_access() {
    assert_eq!(
        MACOS_DAEMON_SIDECAR_ENTITLEMENTS.matches("<key>").count(),
        MACOS_DAEMON_ENTITLEMENTS.matches("<key>").count() + 1
    );
    assert!(
        MACOS_DAEMON_SIDECAR_ENTITLEMENTS.contains("com.apple.security.automation.apple-events")
    );
    assert!(!MACOS_DAEMON_ENTITLEMENTS.contains("com.apple.security.automation.apple-events"));
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
fn public_ci_builds_unsigned_macos_app_fixtures_only() {
    assert!(CI_WORKFLOW.contains("cask_arch: arm64"));
    assert!(CI_WORKFLOW.contains("cask_arch: x86_64"));
    assert!(CI_WORKFLOW.contains("artifact-kind: unsigned-app"));
    assert!(CI_WORKFLOW.contains("--no-sign"));
    assert!(CI_WORKFLOW.contains(r#"@("--target", $env:RUST_TARGET)"#));
    assert!(CI_WORKFLOW.contains("Upload unsigned macOS packaging fixture"));
    assert!(CI_WORKFLOW.contains("name: oss-ci-${{ steps.version.outputs.version }}"));
    assert!(!CI_WORKFLOW.contains("Build signed and notarized macOS artifacts"));
    assert!(!CI_WORKFLOW.contains("APPLE_SIGNING_IDENTITY"));
    assert!(!CI_WORKFLOW.contains("-name '*.dmg'"));
    assert!(!CI_WORKFLOW.contains("-name '*.notarization.json'"));
}

#[test]
fn proprietary_macos_release_tools_use_the_manifest_signing_actor() {
    assert!(!CI_WORKFLOW.contains(r#"APPLE_SIGNING_IDENTITY: "-""#));
    assert!(!CI_WORKFLOW.contains("./scripts/sign-macos-artifacts.sh"));
    assert!(BUILD_MAC_INSTALLER_SH.contains(r#"--bundles app"#));
    assert!(!BUILD_MAC_INSTALLER_SH.contains("dmg,app"));
    assert!(BUILD_MAC_INSTALLER_SH.contains(r#""${SIGNING_ACTOR}" app"#));
    assert!(DIST_SH.contains(r#""${MACOS_SIGNING_ACTOR}" standalone"#));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("codesign_arch_for_target"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains(r#"codesign -d --arch "${arch}" --verbose=4"#));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("^[0-9a-f]{40}$"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("cdhash: $cdhash"));
}

#[test]
fn macos_signing_secrets_stay_out_of_process_arguments() {
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("printf '%s\\0%s\\0'"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("env -u APPLE_CERTIFICATE_PASSWORD"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("APPLE_NOTARY_KEYCHAIN_PROFILE"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("--keychain-profile"));
    assert!(!SIGN_MACOS_ARTIFACTS_SH.contains("security create-keychain -p"));
    assert!(!SIGN_MACOS_ARTIFACTS_SH.contains("security unlock-keychain -p"));
    assert!(!SIGN_MACOS_ARTIFACTS_SH.contains("security import"));
    assert!(!SIGN_MACOS_ARTIFACTS_SH.contains("set-key-partition-list"));
    assert!(!SIGN_MACOS_ARTIFACTS_SH.contains("--apple-id"));
    assert!(!SIGN_MACOS_ARTIFACTS_SH.contains("--password"));

    assert!(MACOS_SIGNING_KEYCHAIN_C.contains("read_secret_frame"));
    assert!(MACOS_SIGNING_KEYCHAIN_C.contains("SecItemImport"));
    assert!(MACOS_SIGNING_KEYCHAIN_C.contains("SecKeychainItemSetAccessWithPassword"));
    assert!(MACOS_SIGNING_KEYCHAIN_C.contains("memset_s"));

    assert!(!CI_WORKFLOW.contains("APPLE_"));
    assert!(!CI_WORKFLOW.contains("notarytool"));

    assert!(BUILD_MAC_INSTALLER_SH.contains("APPLE_NOTARY_KEYCHAIN_PROFILE"));
    assert!(BUILD_MAC_INSTALLER_SH.contains("raw Apple ID credentials are unsupported"));
}

#[test]
fn signed_macos_builds_can_enable_the_physical_tcc_canary_explicitly() {
    assert!(BUILD_MAC_INSTALLER_SH.contains("--tcc-canary"));
    assert!(BUILD_MAC_INSTALLER_SH.contains("--tcc-canary requires --notarize"));
    assert!(
        BUILD_MAC_INSTALLER_SH.contains(r#"daemon_features="${daemon_features},macos-tcc-canary""#)
    );
    assert!(DIST_SH.contains("--tcc-canary requires a macOS target"));
    assert!(DIST_SH.contains("DAEMON_FEATURE_FLAG=(--features macos-tcc-canary)"));
    assert!(DIST_SH.contains(r#""${MACOS_SIGNING_ACTOR}" standalone"#));
}

#[test]
fn tcc_canary_runner_uses_the_daemon_canonical_data_directory() {
    assert!(
        RUN_MACOS_TCC_CANARY_SH
            .contains(r#"${HOME:?HOME must be set}/Library/Application Support/hypercolor"#)
    );
    assert!(!RUN_MACOS_TCC_CANARY_SH.contains("--data-dir"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("--execute-protected-actions"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("--macos-tcc-canary-check-request"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("regular non-symlink witness"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("request process_replacement_witness_id is invalid"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("login_arbitration_witness_id"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("armed request does not exactly match"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("identifier_is_safe"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#"$1" != "." && "$1" != "..""#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("installed_row_artifacts"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("install_new_artifact"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("--macos-tcc-canary-publish"));
    assert!(!RUN_MACOS_TCC_CANARY_SH.contains(r#"/bin/ln "${source}" "${destination}""#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("require_real_path_ancestors"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("path has a symlink ancestor"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("row_committed=true"));
    assert!(!RUN_MACOS_TCC_CANARY_SH.contains("kill -TERM \"${predecessor_pid}\""));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("--cli PATH"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#""${cli}" service enable"#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#""${cli}" service stop"#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#""${cli}" service start"#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#""${brew}" services start hypercolor"#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#""${brew}" services stop hypercolor"#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(r#""${brew}" services start hypercolor"#));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("app_supervisor_daemon_restart"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("app_quit_then_minimized_launch"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("launchd_login_start"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("brew_services_login_start"));
    assert!(!RUN_MACOS_TCC_CANARY_SH.contains("launchctl kickstart"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("operation_timeout_ms + 999"));
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("minimum_timeout_seconds"));
    assert!(
        RUN_MACOS_TCC_CANARY_SH
            .contains(r#""${daemon}" --macos-owner standalone >/dev/null 2>&1 &"#)
    );
    assert!(RUN_MACOS_TCC_CANARY_SH.contains("ensure_descendant_directory"));
    let predecessor_stop = RUN_MACOS_TCC_CANARY_SH
        .find(r#""${cli}" service stop"#)
        .expect("runner should stop the direct service before replacement");
    let exit_observation = RUN_MACOS_TCC_CANARY_SH
        .find("action_observed_unix_ms=$(( $(date +%s) * 1000 ))")
        .expect("runner should timestamp predecessor exit after waiting");
    let replacement_witness = RUN_MACOS_TCC_CANARY_SH
        .find(r#"kind: "process_replacement""#)
        .expect("runner should record a replacement witness");
    let successor_start = RUN_MACOS_TCC_CANARY_SH
        .rfind(r#""${cli}" service start"#)
        .expect("runner should start the direct successor after its witness");
    assert!(predecessor_stop < exit_observation);
    assert!(exit_observation < replacement_witness);
    assert!(replacement_witness < successor_start);
    let pending_receipt_wait = RUN_MACOS_TCC_CANARY_SH
        .find("a regular atomic pending receipt did not arrive")
        .expect("runner should wait for an atomic pending receipt");
    let receipt_wait = RUN_MACOS_TCC_CANARY_SH
        .find("a regular atomic receipt did not arrive")
        .expect("runner should wait for an atomic receipt");
    let settings_install = RUN_MACOS_TCC_CANARY_SH
        .find(r#"install_witness "${settings_witness_id}" system_settings_identity"#)
        .expect("runner should install the settings witness");
    assert!(pending_receipt_wait < settings_install);
    assert!(settings_install < receipt_wait);
    assert!(RUN_MACOS_TCC_CANARY_SH.contains(".signing.audit_token_bound_valid == true"));
    assert!(
        RUN_MACOS_TCC_CANARY_SH
            .contains(".launcher.parent_signing.audit_token_bound_valid == true")
    );
}

#[test]
fn macos_release_verifier_checks_signatures_and_notarization_provenance() {
    assert!(VERIFY_RELEASE_SH.contains("verify-app"));
    assert!(VERIFY_RELEASE_SH.contains("verify-standalone"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("verify_scope"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("verify_inventory"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("app_notarization.status"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("dmg_notarization.status"));
    assert!(SIGN_MACOS_ARTIFACTS_SH.contains("notarization.status"));
}

#[test]
fn release_archives_bind_every_safe_member_before_installation() {
    for field in ["path", "type", "mode", "size", "sha256"] {
        assert!(DIST_SH.contains(&format!("\"{field}\"")));
    }
    assert!(DIST_SH.contains("release payload contains unsupported member"));
    assert!(VERIFY_RELEASE_SH.contains("archive contains duplicate member"));
    assert!(VERIFY_RELEASE_SH.contains("archive contains unsupported member type"));
    assert!(VERIFY_RELEASE_SH.contains("manifest member set mismatch"));
    assert!(VERIFY_RELEASE_SH.contains("manifest digest mismatch"));
    assert!(VERIFY_RELEASE_SH.contains("manifest mode mismatch"));
    assert!(VERIFY_RELEASE_SH.contains("os.fdopen(descriptor, \"rb\")"));
    assert!(VERIFY_RELEASE_SH.contains("os.O_RDWR | os.O_CREAT | os.O_EXCL"));
    assert!(VERIFY_RELEASE_SH.contains("os.unlink(snapshot_path)"));
    assert!(VERIFY_RELEASE_SH.contains("tarfile.open(fileobj=snapshot"));
}

#[cfg(unix)]
#[test]
fn release_verifier_install_mode_owns_candidate_lifetime_and_exit_status() {
    let temp = tempfile::tempdir().expect("temporary install fixture directory");
    let archive = temp.path().join("hypercolor-0.0.0-linux-amd64.tar.gz");
    let manifest_copy = temp.path().join("manifest.json");
    let builder = r#"
import hashlib
import io
import json
import sys
import tarfile

archive_path, manifest_path = sys.argv[1:]
root_name = "hypercolor-0.0.0-linux-amd64"
candidate = b'''#!/usr/bin/env bash
set -eu
root="$(cd -- "$(dirname -- "$0")/.." && pwd)"
test -f "${root}/manifest.json"
printf '%s\n' "${root}" > "${HYPERCOLOR_TEST_ROOT_WITNESS}"
printf '%s\n' "$@" > "${HYPERCOLOR_TEST_ARGS_WITNESS}"
exit "${HYPERCOLOR_TEST_EXIT:-0}"
'''
files = {
    "LICENSE": (0o644, b"license"),
    "NOTICE": (0o644, b"notice"),
    "README.md": (0o644, b"readme"),
    "bin/hypercolor": (0o755, candidate),
    "bin/hypercolor-daemon": (0o755, b"daemon"),
    "bin/hypercolor-app": (0o755, b"app"),
    "bin/hypercolor-tui": (0o755, b"tui"),
    "bin/hypercolor-open": (0o755, b"open"),
    "share/applications/hypercolor.desktop": (0o644, b"desktop"),
    "share/icons/hicolor/48x48/apps/hypercolor.png": (0o644, b"48"),
    "share/icons/hicolor/128x128/apps/hypercolor.png": (0o644, b"128"),
    "share/icons/hicolor/256x256/apps/hypercolor.png": (0o644, b"256"),
    "share/hypercolor/ui/index.html": (0o644, b"ui"),
    "share/hypercolor/effects/bundled/effect.html": (0o644, b"effect"),
    "share/hypercolor/agents/skills/skill.md": (0o644, b"skill"),
    "share/hypercolor/agents/agents/agent.md": (0o644, b"agent"),
    "lib/systemd/user/hypercolor.service": (0o644, b"service"),
    "lib/udev/rules.d/99-hypercolor.rules": (0o644, b"udev"),
    "etc/modules-load.d/i2c-dev.conf": (0o644, b"i2c-dev"),
}
directories = {"share/hypercolor/docs", "share/hypercolor/site"}
for path in files:
    fields = path.split("/")[:-1]
    for index in range(1, len(fields) + 1):
        directories.add("/".join(fields[:index]))
members = [
    {"path": path, "type": "directory", "mode": 0o755}
    for path in sorted(directories)
]
members.extend(
    {
        "path": path,
        "type": "file",
        "mode": mode,
        "size": len(contents),
        "sha256": hashlib.sha256(contents).hexdigest(),
    }
    for path, (mode, contents) in sorted(files.items())
)
manifest = json.dumps(
    {
        "name": "hypercolor",
        "version": "0.0.0",
        "platform": "linux-amd64",
        "rust_target": "x86_64-unknown-linux-gnu",
        "binaries": [
            "hypercolor-daemon",
            "hypercolor",
            "hypercolor-app",
            "hypercolor-tui",
            "hypercolor-open",
        ],
        "assets": {
            "ui_files": 1,
            "bundled_effect_files": 1,
            "docs_files": 0,
            "skill_files": 1,
            "agent_files": 1,
            "site_files": 0,
        },
        "members": members,
    },
    sort_keys=True,
).encode()
files["manifest.json"] = (0o644, manifest)
with open(manifest_path, "wb") as output:
    output.write(manifest)
with tarfile.open(archive_path, "w:gz") as output:
    root = tarfile.TarInfo(f"{root_name}/")
    root.type = tarfile.DIRTYPE
    root.mode = 0o755
    output.addfile(root)
    for directory in sorted(directories):
        entry = tarfile.TarInfo(f"{root_name}/{directory}/")
        entry.type = tarfile.DIRTYPE
        entry.mode = 0o755
        output.addfile(entry)
    for path, (mode, contents) in sorted(files.items()):
        entry = tarfile.TarInfo(f"{root_name}/{path}")
        entry.mode = mode
        entry.size = len(contents)
        output.addfile(entry, io.BytesIO(contents))
"#;
    let built = Command::new("python3")
        .args(["-c", builder])
        .arg(&archive)
        .arg(&manifest_copy)
        .status()
        .expect("python should create the install fixture");
    assert!(built.success(), "failed to build install fixture");

    let hash_file = |path: &Path| {
        Command::new("sha256sum")
            .arg(path)
            .output()
            .or_else(|_| {
                Command::new("shasum")
                    .args(["-a", "256"])
                    .arg(path)
                    .output()
            })
            .expect("a SHA256 tool should hash the install fixture")
    };
    let digest = hash_file(&archive);
    assert!(digest.status.success(), "failed to hash install fixture");
    let checksum = temp.path().join("archive.sha256");
    fs::write(&checksum, &digest.stdout).expect("write checksum fixture");
    let manifest_digest = hash_file(&manifest_copy);
    assert!(manifest_digest.status.success(), "failed to hash manifest");
    let manifest_digest = String::from_utf8(manifest_digest.stdout)
        .expect("manifest digest is UTF-8")
        .split_whitespace()
        .next()
        .expect("manifest digest field")
        .to_owned();

    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&fake_bin).expect("create fake binary directory");
    let uname = fake_bin.join("uname");
    fs::write(
        &uname,
        "#!/usr/bin/env bash\ncase \"$1\" in -s) echo Linux;; -m) echo x86_64;; *) exit 2;; esac\n",
    )
    .expect("write fake uname");
    fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).expect("make uname executable");
    let inherited_path = env::var_os("PATH").unwrap_or_default();
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        inherited_path.to_string_lossy()
    );
    let verifier =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/verify-release-artifact.sh");

    for expected_exit in [0, 23] {
        let root_witness = temp.path().join(format!("root-{expected_exit}"));
        let args_witness = temp.path().join(format!("args-{expected_exit}"));
        let prefix = temp.path().join(format!("prefix-{expected_exit}"));
        let install_dir = prefix.join("bin");
        let output = Command::new("bash")
            .arg(&verifier)
            .args(["--install-candidate", "--archive"])
            .arg(&archive)
            .arg("--checksum")
            .arg(&checksum)
            .arg("--install-prefix")
            .arg(&prefix)
            .arg("--install-dir")
            .arg(&install_dir)
            .arg("--no-service")
            .env("PATH", &path)
            .env("HYPERCOLOR_TEST_ROOT_WITNESS", &root_witness)
            .env("HYPERCOLOR_TEST_ARGS_WITNESS", &args_witness)
            .env("HYPERCOLOR_TEST_EXIT", expected_exit.to_string())
            .output()
            .expect("release verifier should invoke the candidate");
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "unexpected verifier result: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let extracted_root = fs::read_to_string(&root_witness).expect("candidate root witness");
        assert!(
            !Path::new(extracted_root.trim()).exists(),
            "verified root must be cleaned after candidate exit"
        );
        assert_eq!(
            fs::read_to_string(&args_witness)
                .expect("candidate argument witness")
                .lines()
                .collect::<Vec<_>>(),
            [
                "__install-release",
                "--install-prefix",
                prefix.to_str().expect("UTF-8 prefix"),
                "--install-dir",
                install_dir.to_str().expect("UTF-8 install directory"),
                "--expected-manifest-sha256",
                manifest_digest.as_str(),
                "--no-service",
            ]
        );
    }
}

#[cfg(unix)]
#[test]
fn release_verifier_install_mode_rejects_ambiguous_valued_options() {
    let verifier =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/verify-release-artifact.sh");
    for option in [
        "--archive",
        "--checksum",
        "--install-prefix",
        "--install-dir",
    ] {
        let output = Command::new("bash")
            .arg(&verifier)
            .arg("--install-candidate")
            .arg(option)
            .arg("")
            .arg(option)
            .arg("replacement")
            .output()
            .expect("release verifier should parse duplicate options");
        assert_eq!(output.status.code(), Some(2), "{option} duplicate accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("provided exactly once"),
            "unexpected {option} rejection: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for option in [
        "--archive",
        "--checksum",
        "--install-prefix",
        "--install-dir",
    ] {
        let output = Command::new("bash")
            .arg(&verifier)
            .arg("--install-candidate")
            .arg(option)
            .arg("--no-service")
            .output()
            .expect("release verifier should reject a missing option value");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{option} swallowed the next option"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("provided exactly once"),
            "unexpected missing {option} rejection: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = Command::new("bash")
        .arg(&verifier)
        .args(["--install-candidate", "--no-service", "--no-service"])
        .output()
        .expect("release verifier should reject duplicate no-service");
    assert_eq!(
        output.status.code(),
        Some(2),
        "duplicate no-service accepted"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("provided at most once"),
        "unexpected duplicate no-service rejection: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn release_verifier_rejects_unsafe_archive_members_before_extraction() {
    let temp = tempfile::tempdir().expect("temporary archive directory should be created");
    let verifier =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/verify-release-artifact.sh");
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&fake_bin).expect("create fake binary directory");
    let mktemp = fake_bin.join("mktemp");
    fs::write(
        &mktemp,
        "#!/usr/bin/env bash\n[[ \"$#\" -eq 1 && \"$1\" == -d ]] || exit 2\n[[ ! -e \"${HYPERCOLOR_TEST_MKTEMP_PATH}\" ]] || exit 1\nmkdir \"${HYPERCOLOR_TEST_MKTEMP_PATH}\"\nprintf '%s\\n' \"${HYPERCOLOR_TEST_MKTEMP_PATH}\"\n",
    )
    .expect("write fake mktemp");
    fs::set_permissions(&mktemp, fs::Permissions::from_mode(0o755))
        .expect("make mktemp executable");
    let inherited_path = env::var_os("PATH").unwrap_or_default();
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        inherited_path.to_string_lossy()
    );
    let builder = r#"
import io
import sys
import tarfile

archive_path, case = sys.argv[1:]
root_name = "hypercolor-0.0.0-linux-x86_64"
with tarfile.open(archive_path, "w:gz") as archive:
    root = tarfile.TarInfo(f"{root_name}/")
    root.type = tarfile.DIRTYPE
    root.mode = 0 if case == "directory-mode" else 0o755
    archive.addfile(root)

    name = f"{root_name}/payload"
    if case == "traversal":
        name = "../escape"
    member = tarfile.TarInfo(name)
    member.mode = 0o644
    if case == "symlink":
        member.type = tarfile.SYMTYPE
        member.linkname = "/tmp/escape"
    elif case == "hardlink":
        member.type = tarfile.LNKTYPE
        member.linkname = f"{root_name}/target"
    elif case == "special":
        member.type = tarfile.CHRTYPE
        member.devmajor = 1
        member.devminor = 3
    else:
        if case == "setuid":
            member.mode = 0o4755
        member.size = 1
        archive.addfile(member, io.BytesIO(b"x"))
        if case == "duplicate":
            archive.addfile(member, io.BytesIO(b"x"))
        member = None
    if member is not None:
        archive.addfile(member)
"#;

    for (case, expected_error) in [
        ("traversal", "archive contains unsafe path"),
        ("symlink", "archive contains unsupported member type"),
        ("hardlink", "archive contains unsupported member type"),
        ("special", "archive contains unsupported member type"),
        ("duplicate", "archive contains duplicate member"),
        ("setuid", "archive contains unsupported mode"),
        ("directory-mode", "archive contains unsafe directory mode"),
    ] {
        let archive = temp.path().join(format!("{case}.tar.gz"));
        let status = Command::new("python3")
            .args(["-c", builder])
            .arg(&archive)
            .arg(case)
            .status()
            .expect("python should create the hostile archive fixture");
        assert!(status.success(), "failed to build {case} fixture");

        let digest = Command::new("sha256sum")
            .arg(&archive)
            .output()
            .or_else(|_| {
                Command::new("shasum")
                    .args(["-a", "256"])
                    .arg(&archive)
                    .output()
            })
            .expect("a SHA256 tool should hash the hostile archive fixture");
        assert!(digest.status.success(), "failed to hash {case} fixture");
        let checksum = format!("{}.sha256", archive.display());
        fs::write(&checksum, digest.stdout).expect("checksum fixture should be written");

        let verifier_tmp = temp.path().join(format!("verifier-{case}"));
        let output = Command::new("bash")
            .arg(&verifier)
            .arg(&archive)
            .arg(&checksum)
            .env("PATH", &path)
            .env("HYPERCOLOR_TEST_MKTEMP_PATH", &verifier_tmp)
            .output()
            .expect("release verifier should inspect the hostile archive fixture");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{case} archive was accepted");
        assert!(
            stderr.contains(expected_error),
            "unexpected {case} rejection: {stderr}"
        );
        assert!(!verifier_tmp.exists(), "{case} left verifier temp residue");
    }

    assert!(!temp.path().join("escape").exists());
}

#[cfg(unix)]
#[test]
fn release_verifier_rejects_unbound_and_malformed_manifests() {
    let temp = tempfile::tempdir().expect("temporary archive directory should be created");
    let builder = r#"
import hashlib
import io
import json
import sys
import tarfile

archive_path = sys.argv[1]
case = sys.argv[2]
root_name = "hypercolor-0.0.0-linux-amd64"
files = {
    "LICENSE": (0o644, b"license"),
    "NOTICE": (0o644, b"notice"),
    "README.md": (0o644, b"readme"),
    "share/applications/hypercolor.desktop": (0o644, b"desktop"),
    "share/icons/hicolor/48x48/apps/hypercolor.png": (0o644, b"48"),
    "share/icons/hicolor/128x128/apps/hypercolor.png": (0o644, b"128"),
    "share/icons/hicolor/256x256/apps/hypercolor.png": (0o644, b"256"),
    "share/hypercolor/ui/index.html": (0o644, b"ui"),
    "share/hypercolor/effects/bundled/effect.html": (0o644, b"effect"),
    "share/hypercolor/agents/skills/skill.md": (0o644, b"skill"),
    "share/hypercolor/agents/agents/agent.md": (0o644, b"agent"),
}
if case == "nested-manifest":
    files["extras/manifest.json"] = (0o644, b"unbound")
for binary in [
    "hypercolor-daemon",
    "hypercolor",
    "hypercolor-app",
    "hypercolor-tui",
    "hypercolor-open",
]:
    files[f"bin/{binary}"] = (0o755, b"binary")

directories = {"share/hypercolor/docs", "share/hypercolor/site"}
for path in files:
    fields = path.split("/")[:-1]
    for index in range(1, len(fields) + 1):
        directories.add("/".join(fields[:index]))

members = [
    {"path": path, "type": "directory", "mode": 0o755}
    for path in sorted(directories)
]
for path, (mode, contents) in sorted(files.items()):
    if case == "nested-manifest" and path == "extras/manifest.json":
        continue
    members.append(
        {
            "path": path,
            "type": "file",
            "mode": mode,
            "size": len(contents),
            "sha256": hashlib.sha256(contents).hexdigest(),
        }
    )
manifest = {
    "name": "hypercolor",
    "version": "0.0.0",
    "platform": "linux-amd64",
    "rust_target": "x86_64-unknown-linux-gnu",
    "binaries": [
        "hypercolor-daemon",
        "hypercolor",
        "hypercolor-app",
        "hypercolor-tui",
        "hypercolor-open",
    ],
    "assets": {
        "ui_files": 1,
        "bundled_effect_files": 1,
        "docs_files": 0,
        "skill_files": 1,
        "agent_files": 1,
        "site_files": 0,
    },
    "members": members,
}
if case == "binaries-object":
    manifest["binaries"] = {binary: True for binary in manifest["binaries"]}
elif case == "binaries-duplicates":
    manifest["binaries"].append("hypercolor")
elif case == "docs-count-string":
    manifest["assets"]["docs_files"] = "zero"
files["manifest.json"] = (0o644, json.dumps(manifest).encode())

with tarfile.open(archive_path, "w:gz") as output:
    root = tarfile.TarInfo(f"{root_name}/")
    root.type = tarfile.DIRTYPE
    root.mode = 0o755
    output.addfile(root)
    for directory in sorted(directories):
        entry = tarfile.TarInfo(f"{root_name}/{directory}/")
        entry.type = tarfile.DIRTYPE
        entry.mode = 0o755
        output.addfile(entry)
    for path, (mode, contents) in sorted(files.items()):
        entry = tarfile.TarInfo(f"{root_name}/{path}")
        entry.mode = mode
        entry.size = len(contents)
        output.addfile(entry, io.BytesIO(contents))
"#;
    let verifier =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/verify-release-artifact.sh");
    for (case, expected_error) in [
        ("nested-manifest", "manifest member set mismatch"),
        (
            "binaries-object",
            "manifest binaries do not match the release payload",
        ),
        (
            "binaries-duplicates",
            "manifest binaries do not match the release payload",
        ),
        ("docs-count-string", "manifest assets.docs_files is invalid"),
    ] {
        let archive = temp.path().join(format!("{case}.tar.gz"));
        let status = Command::new("python3")
            .args(["-c", builder])
            .arg(&archive)
            .arg(case)
            .status()
            .expect("python should create the malformed manifest fixture");
        assert!(status.success(), "failed to build {case} fixture");

        let digest = Command::new("sha256sum")
            .arg(&archive)
            .output()
            .or_else(|_| {
                Command::new("shasum")
                    .args(["-a", "256"])
                    .arg(&archive)
                    .output()
            })
            .expect("a SHA256 tool should hash the malformed manifest fixture");
        assert!(digest.status.success(), "failed to hash {case} fixture");
        let checksum = format!("{}.sha256", archive.display());
        fs::write(&checksum, digest.stdout).expect("checksum fixture should be written");

        let output = Command::new("bash")
            .arg(&verifier)
            .arg(&archive)
            .arg(&checksum)
            .output()
            .expect("release verifier should inspect the malformed manifest fixture");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{case} manifest was accepted");
        assert!(
            stderr.contains(expected_error),
            "unexpected {case} rejection: {stderr}"
        );
    }
}

#[test]
fn public_ci_leaves_homebrew_promotion_to_the_proprietary_release_pipeline() {
    assert!(!CI_WORKFLOW.contains("update-homebrew:"));
    assert!(!CI_WORKFLOW.contains("HOMEBREW_TAP_TOKEN"));
    assert!(!CI_WORKFLOW.contains("tap/Casks"));
    assert!(HOMEBREW_FORMULA.contains("SHA256_MACOS_ARM64"));
    assert!(HOMEBREW_CASK.contains("SHA256_MACOS_APP_ARM64"));
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
    assert!(JUSTFILE.contains("scripts/ui-windows.ps1 -Mode Serve"));
    assert!(JUSTFILE.contains("scripts/ui-windows.ps1 -Mode Build"));
    assert!(JUSTFILE.contains(
        "scripts/cargo-cache-build.ps1 cargo build -p hypercolor-daemon -p hypercolor-cli"
    ));
    assert!(JUSTFILE.contains(
        "scripts/cargo-cache-build.ps1 cargo build -p hypercolor-daemon --no-default-features"
    ));
    assert!(UI_WINDOWS_PS1.contains("cargo-cache-build.ps1"));
    assert!(UI_WINDOWS_PS1.contains("HYPERCOLOR_NO_SCCACHE"));
    assert!(UI_WINDOWS_PS1.contains("HYPERCOLOR_ITERATE"));
    assert!(CARGO_CACHE_BUILD_PS1.contains("$CallerDir = Get-Location"));
    assert!(CARGO_CACHE_BUILD_PS1.contains("Set-Location $CallerDir"));
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
