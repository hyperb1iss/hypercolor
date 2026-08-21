#![cfg(target_os = "macos")]

use std::fmt::Write as _;
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::process::Command;
use std::time::Duration;

use hypercolor_macos_owner::{
    MacosDirectLaunchdExecutableExpectation, validate_retained_macos_executable,
};
use sha2::{Digest as _, Sha256};

const LS_REQUIREMENT: &str = "identifier \"com.apple.ls\" and anchor apple";
static VALIDATION_TEST_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn retained_code_validation_ignores_path_replacement_and_rejects_drift() {
    let _guard = VALIDATION_TEST_GATE.lock().expect("test gate should lock");
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let path = directory.path().join("hypercolor-daemon");
    fs::copy("/bin/ls", &path).expect("signed fixture should copy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555))
        .expect("immutable mode should set");
    let retained = File::open(&path).expect("signed fixture should retain");
    let metadata = retained.metadata().expect("retained metadata should read");
    let bytes = fs::read(&path).expect("fixture bytes should read");
    let ls_cdhash = cdhash("/bin/ls");
    let exact = expectation(&path, LS_REQUIREMENT, &ls_cdhash, &bytes, &metadata);

    assert!(
        validate_retained_macos_executable(&retained, &exact, Duration::from_secs(10))
            .expect("exact retained validation should complete")
    );
    let wrong_cdhash = expectation(
        &path,
        LS_REQUIREMENT,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        &bytes,
        &metadata,
    );
    assert!(
        !validate_retained_macos_executable(&retained, &wrong_cdhash, Duration::from_secs(10))
            .expect("wrong CDHash should be rejected")
    );
    let wrong_requirement = expectation(
        &path,
        "identifier \"com.apple.cat\" and anchor apple",
        &ls_cdhash,
        &bytes,
        &metadata,
    );
    assert!(
        !validate_retained_macos_executable(
            &retained,
            &wrong_requirement,
            Duration::from_secs(10),
        )
        .expect("wrong requirement should be rejected")
    );

    let retained_path = directory.path().join("retained-daemon");
    fs::rename(&path, &retained_path).expect("retained path should move");
    fs::copy("/bin/ls", &path).expect("exact path replacement should copy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555))
        .expect("exact replacement mode should set");
    assert!(
        validate_retained_macos_executable(&retained, &exact, Duration::from_secs(10))
            .expect("exact accepted image should ignore pathname inode replacement")
    );

    fs::remove_file(&path).expect("exact path replacement should remove");
    fs::copy("/bin/cat", &path).expect("attacker replacement should copy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555))
        .expect("replacement mode should set");
    assert!(
        !validate_retained_macos_executable(&retained, &exact, Duration::from_secs(10))
            .expect("wrong accepted image should be rejected")
    );

    fs::set_permissions(&retained_path, fs::Permissions::from_mode(0o755))
        .expect("retained inode should become writable");
    fs::write(&retained_path, b"unsigned drift").expect("retained inode should drift");
    assert!(
        !validate_retained_macos_executable(&retained, &exact, Duration::from_secs(10))
            .expect("byte drift should be rejected")
    );
}

#[test]
fn retained_code_validation_rejects_malformed_requirements_and_expired_deadlines() {
    let _guard = VALIDATION_TEST_GATE.lock().expect("test gate should lock");
    let directory = tempfile::tempdir().expect("temporary directory should build");
    let path = directory.path().join("hypercolor-daemon");
    fs::copy("/bin/ls", &path).expect("signed fixture should copy");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o555))
        .expect("immutable mode should set");
    let retained = File::open(&path).expect("signed fixture should retain");
    let metadata = retained.metadata().expect("retained metadata should read");
    let bytes = fs::read(&path).expect("fixture bytes should read");
    let ls_cdhash = cdhash("/bin/ls");
    let malformed = expectation(&path, "not && a requirement", &ls_cdhash, &bytes, &metadata);
    assert!(
        validate_retained_macos_executable(&retained, &malformed, Duration::from_secs(10)).is_err()
    );

    assert!(
        MacosDirectLaunchdExecutableExpectation::new(
            &path,
            LS_REQUIREMENT,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "not-a-cdhash",
            digest(&bytes),
            metadata.mode() & 0o7777,
            metadata.len(),
            metadata.dev(),
            metadata.ino(),
        )
        .is_err()
    );

    let exact = expectation(&path, LS_REQUIREMENT, &ls_cdhash, &bytes, &metadata);
    let error = validate_retained_macos_executable(&retained, &exact, Duration::ZERO)
        .expect_err("zero deadline must reject before validation");
    assert!(error.to_string().contains("deadline"));
}

fn expectation(
    path: &std::path::Path,
    requirement: &str,
    cdhash: &str,
    bytes: &[u8],
    metadata: &std::fs::Metadata,
) -> MacosDirectLaunchdExecutableExpectation {
    MacosDirectLaunchdExecutableExpectation::new(
        path,
        requirement,
        digest(requirement.as_bytes()),
        cdhash,
        digest(bytes),
        metadata.mode() & 0o7777,
        metadata.len(),
        metadata.dev(),
        metadata.ino(),
    )
    .expect("executable expectation should build")
}

fn cdhash(path: &str) -> String {
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4", path])
        .env_clear()
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .output()
        .expect("codesign should inspect the signed fixture");
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("codesign output should be UTF-8");
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))
        .expect("codesign should report CDHash")
        .to_owned()
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("digest should write");
            output
        })
}
