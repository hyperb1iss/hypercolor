//! This CLI's own candidate validator accepts what the release producer
//! writes, with and without the durable store inventory.
//!
//! `scripts/tests/release-artifact-tests.sh` packages this crate's
//! `hypercolor` binary as the fixture's CLI, so the producer's output meets
//! the Rust validator as well as the script verifier.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;

#[test]
fn the_rust_validator_accepts_what_the_producer_writes() {
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
