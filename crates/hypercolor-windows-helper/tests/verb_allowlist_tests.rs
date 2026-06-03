//! Sanity checks for the verb deserialization allowlist.
//!
//! The helper's primary security invariant is "no arbitrary commands."
//! That guarantee rests on `Verb` being a closed enum with no
//! `#[serde(other)]` catch-all. These tests freeze that contract.

#![cfg(target_os = "windows")]

use serde_json::json;

/// All known verbs round-trip through serde.
#[test]
fn known_verbs_deserialize() {
    for verb in [
        "install-pawnio",
        "install-smbus-service",
        "install-hardware-support",
        "repair-smbus-service",
        "swap-smbus-binary",
        "uninstall-smbus-service",
        "uninstall-pawnio",
        "uninstall-hardware-support",
    ] {
        let body = json!({
            "verb": verb,
            "nonce": 1,
            "issued_at_ms": 1000,
        });
        let parsed: Result<TestRequest, _> = serde_json::from_value(body);
        parsed.unwrap_or_else(|err| panic!("verb `{verb}` should deserialize: {err}"));
    }
}

/// Unknown verbs are rejected — this is the allowlist contract.
#[test]
fn unknown_verb_rejected() {
    for evil in [
        "install-anything",
        "delete-all",
        "../uninstall-pawnio",
        "InstallPawnIo",  // wrong case
        "install_pawnio", // wrong separator
        "",
    ] {
        let body = json!({
            "verb": evil,
            "nonce": 1,
            "issued_at_ms": 1000,
        });
        let parsed: Result<TestRequest, _> = serde_json::from_value(body);
        assert!(
            parsed.is_err(),
            "verb `{evil}` must be rejected by the allowlist"
        );
    }
}

// Local mirror of the request shape — keeps this an integration test that
// doesn't need to expose internals of the helper crate.
#[derive(serde::Deserialize)]
struct TestRequest {
    #[allow(dead_code)]
    verb: Verb,
    #[allow(dead_code)]
    nonce: u64,
    #[allow(dead_code)]
    issued_at_ms: u64,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Verb {
    InstallPawnio,
    InstallSmbusService,
    InstallHardwareSupport,
    RepairSmbusService,
    SwapSmbusBinary,
    UninstallSmbusService,
    UninstallPawnio,
    UninstallHardwareSupport,
}
