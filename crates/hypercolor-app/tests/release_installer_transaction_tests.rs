const INSTALL_RELEASE_SH: &str = include_str!("../../../scripts/install-release.sh");

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

fn function_body(name: &str, next_name: &str) -> &'static str {
    let (_, tail) = INSTALL_RELEASE_SH
        .split_once(&format!("{name}() {{"))
        .unwrap_or_else(|| panic!("missing {name} function"));
    let (body, _) = tail
        .split_once(&format!("{next_name}() {{"))
        .unwrap_or_else(|| panic!("missing {next_name} function after {name}"));
    body
}

#[test]
fn raw_release_install_hands_the_archive_to_the_hardened_candidate_verifier() {
    let handoff = function_body("install_release_candidate", "setup_tmpdir");
    for argument in [
        "--install-candidate",
        "--archive",
        "--checksum",
        "--install-prefix",
        "--install-dir",
        "--no-service",
    ] {
        assert!(
            handoff.contains(argument),
            "missing verifier argument {argument}"
        );
    }
    assert!(handoff.contains("bash \"$RELEASE_VERIFIER\""));

    let download = function_body("download_release_artifact", "download_release_verifier");
    assert!(!download.contains("tar -xzf"));
    assert!(!download.contains("verify_release_artifact"));
}

#[test]
fn raw_install_flow_contains_no_shell_release_or_service_mutation() {
    assert!(!INSTALL_RELEASE_SH.contains("install_systemd_service()"));
    assert!(!INSTALL_RELEASE_SH.contains("stop_service_if_running()"));
    assert!(!INSTALL_RELEASE_SH.contains("prompt_udev_rules()"));

    let install = function_body("do_install", "do_uninstall");
    assert!(install.contains("download_release_verifier"));
    assert!(install.contains("install_release_candidate"));
    for forbidden in [
        "tar -xzf",
        "systemctl",
        "launchctl",
        "install -D",
        "cp -R",
        "rm -f",
        "rm -rf",
        "hypercolor.service",
        "tech.hyperbliss.hypercolor.plist",
        "prompt_udev_rules",
        "install_macos_release_payload",
        "install_launchd_agent",
    ] {
        assert!(
            !install.contains(forbidden),
            "raw install flow retains shell mutation: {forbidden}"
        );
    }
}

#[test]
fn installer_help_describes_transactional_preserve_policy() {
    assert!(INSTALL_RELEASE_SH.contains("--no-service      Preserve raw-direct service state"));
    let install = function_body("do_install", "do_uninstall");
    assert!(!install.contains("Darwin)"));
    assert!(!install.contains("Linux)"));
}

#[cfg(unix)]
#[test]
fn raw_shell_handoff_forwards_exact_arguments_without_legacy_mutation() {
    let temp = tempfile::tempdir().expect("temporary shell handoff fixture");
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir(&fake_bin).expect("create fake command directory");
    write_executable(
        &fake_bin.join("uname"),
        "#!/usr/bin/env bash\ncase \"$1\" in -s) echo \"$HYPERCOLOR_TEST_OS\";; -m) echo \"$HYPERCOLOR_TEST_ARCH\";; *) exit 2;; esac\n",
    );
    write_executable(
        &fake_bin.join("mktemp"),
        "#!/usr/bin/env bash\n[[ \"$1\" == -d ]] || exit 2\nmkdir \"$HYPERCOLOR_TEST_MKTEMP\"\nprintf '%s\\n' \"$HYPERCOLOR_TEST_MKTEMP\"\n",
    );
    write_executable(
        &fake_bin.join("curl"),
        r#"#!/usr/bin/env bash
set -euo pipefail
destination=""
url=""
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        -o) destination="$2"; shift 2 ;;
        -fsSL|--progress-bar) shift ;;
        *) url="$1"; shift ;;
    esac
done
[[ -n "$destination" && -n "$url" ]] || exit 2
if [[ "$url" == */scripts/verify-release-artifact.sh ]]; then
    cat > "$destination" <<'SCRIPT'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$HYPERCOLOR_TEST_VERIFIER_ARGS"
exit "$HYPERCOLOR_TEST_VERIFIER_EXIT"
SCRIPT
else
    printf 'downloaded fixture\n' > "$destination"
fi
"#,
    );
    let forbidden_witness = temp.path().join("forbidden-command");
    let etc_witness = temp.path().join("etc-mutation");
    for command in [
        "cp",
        "install",
        "launchctl",
        "lsmod",
        "modprobe",
        "systemctl",
        "tar",
        "udevadm",
    ] {
        write_executable(
            &fake_bin.join(command),
            &format!(
                "#!/usr/bin/env bash\nprintf '%s\\n' {command} >> \"$HYPERCOLOR_TEST_FORBIDDEN\"\nexit 97\n"
            ),
        );
    }
    write_executable(
        &fake_bin.join("sudo"),
        "#!/usr/bin/env bash\nprintf '%s\n' sudo >> \"$HYPERCOLOR_TEST_FORBIDDEN\"\nprintf '%s\n' \"$*\" > \"$HYPERCOLOR_TEST_ETC_WITNESS\"\nexit 97\n",
    );

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        inherited_path.to_string_lossy()
    );
    let installer = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install-release.sh");
    for (os, arch, suffix) in [
        ("Linux", "x86_64", "linux-amd64"),
        ("Darwin", "arm64", "macos-arm64"),
    ] {
        for expected_exit in [0, 23] {
            let case = format!("{}-{expected_exit}", os.to_ascii_lowercase());
            let home = temp.path().join(format!("home-{case}"));
            let prefix = home.join(".local");
            let install_dir = prefix.join("bin");
            let install_tmp = temp.path().join(format!("install-tmp-{case}"));
            let args_witness = temp.path().join(format!("verifier-args-{case}"));
            let legacy_binary = install_dir.join("hypercolor");
            let legacy_plist = home.join("Library/LaunchAgents/tech.hyperbliss.hypercolor.plist");
            let legacy_ui = prefix.join("share/hypercolor/ui/index.html");
            write_fixture(&legacy_binary, b"legacy binary");
            write_fixture(&legacy_plist, b"legacy plist");
            write_fixture(&legacy_ui, b"legacy ui");

            let active = prefix.join("lib/hypercolor/active");
            fs::create_dir_all(active.join("lib/udev/rules.d"))
                .expect("create active udev fixture");
            fs::create_dir_all(active.join("etc/modules-load.d"))
                .expect("create active module fixture");
            fs::write(
                active.join("lib/udev/rules.d/99-hypercolor.rules"),
                b"udev fixture",
            )
            .expect("write active udev fixture");
            fs::write(active.join("etc/modules-load.d/i2c-dev.conf"), b"i2c-dev\n")
                .expect("write active module fixture");
            let output = Command::new("bash")
                .arg(&installer)
                .args(["--version", "v1.2.3", "--yes", "--no-service"])
                .env("PATH", &path)
                .env("HOME", &home)
                .env("NO_COLOR", "1")
                .env("HYPERCOLOR_INSTALL_PREFIX", &prefix)
                .env("HYPERCOLOR_INSTALL_DIR", &install_dir)
                .env("HYPERCOLOR_TEST_OS", os)
                .env("HYPERCOLOR_TEST_ARCH", arch)
                .env("HYPERCOLOR_TEST_MKTEMP", &install_tmp)
                .env("HYPERCOLOR_TEST_VERIFIER_ARGS", &args_witness)
                .env("HYPERCOLOR_TEST_VERIFIER_EXIT", expected_exit.to_string())
                .env("HYPERCOLOR_TEST_FORBIDDEN", &forbidden_witness)
                .env("HYPERCOLOR_TEST_ETC_WITNESS", &etc_witness)
                .output()
                .expect("execute raw shell handoff");
            assert_eq!(
                output.status.code(),
                Some(expected_exit),
                "unexpected {os} shell status: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let archive = format!("{}/hypercolor-1.2.3-{suffix}.tar.gz", install_tmp.display());
            let checksum = format!("{archive}.sha256");
            let expected_args = vec![
                "--install-candidate".to_owned(),
                "--archive".to_owned(),
                archive,
                "--checksum".to_owned(),
                checksum,
                "--install-prefix".to_owned(),
                prefix.to_str().expect("UTF-8 prefix").to_owned(),
                "--install-dir".to_owned(),
                install_dir
                    .to_str()
                    .expect("UTF-8 install directory")
                    .to_owned(),
                "--no-service".to_owned(),
            ];
            assert_eq!(
                fs::read_to_string(&args_witness)
                    .expect("read verifier arguments")
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                expected_args
            );
            if expected_exit == 0 {
                assert!(String::from_utf8_lossy(&output.stdout).contains("installed successfully"));
            } else {
                assert!(
                    !String::from_utf8_lossy(&output.stdout).contains("installed successfully")
                );
            }
            assert_eq!(
                fs::read(&legacy_binary).expect("legacy binary"),
                b"legacy binary"
            );
            assert_eq!(
                fs::read(&legacy_plist).expect("legacy plist"),
                b"legacy plist"
            );
            assert_eq!(fs::read(&legacy_ui).expect("legacy UI"), b"legacy ui");
        }
    }
    assert!(
        !forbidden_witness.exists(),
        "raw shell invoked a forbidden mutation command"
    );
    assert!(
        !etc_witness.exists(),
        "raw shell attempted a privileged /etc mutation"
    );
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write fake command");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make fake command executable");
}

#[cfg(unix)]
fn write_fixture(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
    fs::write(path, contents).expect("write fixture");
}
