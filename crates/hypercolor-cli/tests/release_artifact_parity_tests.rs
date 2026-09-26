//! The release producer, the script verifier and this CLI's own candidate
//! validator accept and refuse the same manifests.
//!
//! `scripts/tests/release-artifact-tests.sh` packages this crate's
//! `hypercolor` binary as the fixture's CLI, so every producer output and
//! every manifest the script verifier refuses also meets the Rust
//! validator, each with its own expected reason.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

#[test]
fn producer_script_verifier_and_rust_validator_agree() {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/tests/release-artifact-tests.sh");
    let output = Command::new("bash")
        .arg(&script)
        .env(
            "HYPERCOLOR_RELEASE_TEST_CLI",
            env!("CARGO_BIN_EXE_hypercolor"),
        )
        .output()
        .expect("run the packaging tests");
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{report}");
    assert!(
        report.contains("test_rust_validator_accepts_the_producer_output")
            && !report.contains("skipped 'HYPERCOLOR_RELEASE_TEST_CLI is not set'"),
        "the Rust validator must take part: {report}"
    );
}
