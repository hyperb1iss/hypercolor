use std::path::PathBuf;
use std::time::Duration;

use hypercolor_macos_owner::{
    MacosDaemonOwner, MacosDirectLaunchdExecutableExpectation, MacosDirectLaunchdInspector,
    MacosDirectLaunchdPublicationExpectation, MacosDirectLaunchdState, MacosOwnerExecutionError,
    MacosOwnerIdentity, MacosOwnerRecord, MacosOwnerStore, corroborate_direct_launchd_owner,
    corroborate_newer_direct_launchd_owner, parse_direct_launchd_service_state,
    wait_for_exact_direct_launchd_publication,
};
use sha2::{Digest as _, Sha256};

const UID: u32 = 501;
const CANDIDATE_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CANDIDATE_REQUIREMENT: &str = "candidate-requirement";

#[derive(Debug)]
struct FakeInspector {
    state: MacosDirectLaunchdState,
    final_state: Option<MacosDirectLaunchdState>,
    state_change_after: usize,
    identity_matches: bool,
    publication_identity_matches: bool,
    state_inspections: usize,
    identity_inspections: usize,
    digest_inspections: usize,
}

impl FakeInspector {
    const fn loaded(pid: u32) -> Self {
        Self {
            state: MacosDirectLaunchdState::Loaded { pid },
            final_state: None,
            state_change_after: usize::MAX,
            identity_matches: true,
            publication_identity_matches: true,
            state_inspections: 0,
            identity_inspections: 0,
            digest_inspections: 0,
        }
    }
}

impl MacosDirectLaunchdInspector for FakeInspector {
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        self.state_inspections += 1;
        Ok(self
            .final_state
            .filter(|_| self.state_inspections > self.state_change_after)
            .unwrap_or(self.state))
    }

    fn live_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError> {
        self.identity_inspections += 1;
        Ok(self.identity_matches)
    }

    fn publication_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
        _executable: &MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError> {
        self.digest_inspections += 1;
        Ok(self.publication_identity_matches)
    }
}

fn identity(label: &str, path: &str, requirement: &str, pid: u32) -> MacosOwnerIdentity {
    MacosOwnerIdentity::new(label, path, hex_digest(requirement.as_bytes()), pid)
        .expect("fixture identity should build")
}

fn record(
    owner: MacosDaemonOwner,
    epoch: u64,
    path: &str,
    requirement: &str,
    pid: u32,
) -> MacosOwnerRecord {
    MacosOwnerRecord {
        schema_version: hypercolor_macos_owner::MACOS_OWNER_RECORD_SCHEMA_VERSION,
        active_owner: owner,
        active_identity: identity("audit", path, requirement, pid),
        owner_epoch: epoch,
        conflict: None,
        selected_external_owner: None,
    }
}

fn expectation(epoch: u64) -> MacosDirectLaunchdPublicationExpectation {
    let executable = MacosDirectLaunchdExecutableExpectation::new(
        "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
        CANDIDATE_REQUIREMENT,
        hex_digest(CANDIDATE_REQUIREMENT.as_bytes()),
        CANDIDATE_SHA256,
        0o555,
        1024,
        1,
        2,
    )
    .expect("fixture executable expectation should build");
    MacosDirectLaunchdPublicationExpectation::new(epoch, executable)
        .expect("fixture expectation should build")
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("write digest");
            output
        })
}

#[test]
fn launchctl_parser_distinguishes_exact_missing_service_from_loaded_owner() {
    let missing = format!(
        "Could not find service \"tech.hyperbliss.hypercolor\" in domain for user gui: {UID}\n"
    );
    assert_eq!(
        parse_direct_launchd_service_state(UID, Some(113), b"", missing.as_bytes())
            .expect("exact missing-service output should parse"),
        MacosDirectLaunchdState::NotLoaded
    );
    assert_eq!(
        parse_direct_launchd_service_state(UID, Some(0), b"state = running\n\tpid = 4242\n", b"",)
            .expect("one positive pid should parse"),
        MacosDirectLaunchdState::Loaded { pid: 4_242 }
    );

    let wrong_missing = b"Could not find service \"other.service\" in domain for user gui: 501\n";
    assert!(parse_direct_launchd_service_state(UID, Some(113), b"", wrong_missing).is_err());
    assert!(
        parse_direct_launchd_service_state(UID, Some(113), b"unexpected", missing.as_bytes())
            .is_err()
    );
    let extra_missing = format!("{missing}unexpected\n");
    assert!(
        parse_direct_launchd_service_state(UID, Some(113), b"", extra_missing.as_bytes()).is_err()
    );
}

#[test]
fn launchctl_parser_rejects_missing_malformed_zero_multiple_and_oversized_pids() {
    for stdout in [
        b"state = running\n".as_slice(),
        b"pid = nope\n".as_slice(),
        b"pid = 0\n".as_slice(),
        b"pid = 41\npid = 42\n".as_slice(),
    ] {
        assert!(parse_direct_launchd_service_state(UID, Some(0), stdout, b"").is_err());
    }

    let oversized = vec![b'x'; 64 * 1024 + 1];
    assert!(parse_direct_launchd_service_state(UID, Some(0), &oversized, b"").is_err());
    assert!(parse_direct_launchd_service_state(UID, Some(0), &[0xff], b"").is_err());
    assert!(parse_direct_launchd_service_state(UID, Some(78), b"", b"failure").is_err());
}

#[test]
fn direct_owner_proof_requires_topology_pid_and_live_identity() {
    let direct = record(
        MacosDaemonOwner::DirectLaunchd,
        7,
        "/opt/hypercolor/daemon",
        "requirement",
        42,
    );

    let mut wrong_owner_inspector = FakeInspector::loaded(42);
    let wrong_owner = record(
        MacosDaemonOwner::Homebrew,
        7,
        "/opt/hypercolor/daemon",
        "requirement",
        42,
    );
    assert!(corroborate_direct_launchd_owner(&wrong_owner, &mut wrong_owner_inspector).is_err());
    assert_eq!(wrong_owner_inspector.state_inspections, 0);

    let mut wrong_pid = FakeInspector::loaded(43);
    assert!(corroborate_direct_launchd_owner(&direct, &mut wrong_pid).is_err());
    assert_eq!(wrong_pid.identity_inspections, 0);

    let mut wrong_identity = FakeInspector::loaded(42);
    wrong_identity.identity_matches = false;
    assert!(corroborate_direct_launchd_owner(&direct, &mut wrong_identity).is_err());

    let mut changed_launchd_owner = FakeInspector::loaded(42);
    changed_launchd_owner.final_state = Some(MacosDirectLaunchdState::Loaded { pid: 43 });
    changed_launchd_owner.state_change_after = 1;
    assert!(corroborate_direct_launchd_owner(&direct, &mut changed_launchd_owner).is_err());

    let mut exact = FakeInspector::loaded(42);
    let proof = corroborate_direct_launchd_owner(&direct, &mut exact)
        .expect("exact loaded identity should corroborate");
    assert_eq!(proof.record(), &direct);
}

#[test]
fn exact_publication_ignores_stale_epoch_and_rejects_newer_identity_drift() {
    let expected = expectation(9);
    let stale = record(
        MacosDaemonOwner::DirectLaunchd,
        9,
        "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
        "candidate-requirement",
        80,
    );
    let mut inspector = FakeInspector::loaded(80);
    assert_eq!(
        corroborate_newer_direct_launchd_owner(&stale, &expected, &mut inspector)
            .expect("stale publication should be ignored"),
        None
    );
    assert_eq!(inspector.state_inspections, 0);

    for drifted in [
        record(
            MacosDaemonOwner::Homebrew,
            10,
            "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
            "candidate-requirement",
            81,
        ),
        record(
            MacosDaemonOwner::DirectLaunchd,
            10,
            "/opt/hypercolor/units/other/bin/hypercolor-daemon",
            "candidate-requirement",
            81,
        ),
        record(
            MacosDaemonOwner::DirectLaunchd,
            10,
            "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
            "other-requirement",
            81,
        ),
    ] {
        let mut inspector = FakeInspector::loaded(81);
        assert!(
            corroborate_newer_direct_launchd_owner(&drifted, &expected, &mut inspector).is_err()
        );
        assert_eq!(inspector.state_inspections, 0);
    }

    let exact_record = record(
        MacosDaemonOwner::DirectLaunchd,
        10,
        "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
        "candidate-requirement",
        81,
    );
    let mut digest_mismatch = FakeInspector::loaded(81);
    digest_mismatch.publication_identity_matches = false;
    assert!(
        corroborate_newer_direct_launchd_owner(&exact_record, &expected, &mut digest_mismatch,)
            .is_err()
    );
    assert_eq!(digest_mismatch.state_inspections, 1);
    assert_eq!(digest_mismatch.identity_inspections, 0);
    assert_eq!(digest_mismatch.digest_inspections, 1);

    let mut post_digest_launchd_drift = FakeInspector::loaded(81);
    post_digest_launchd_drift.final_state = Some(MacosDirectLaunchdState::Loaded { pid: 82 });
    post_digest_launchd_drift.state_change_after = 1;
    assert!(
        corroborate_newer_direct_launchd_owner(
            &exact_record,
            &expected,
            &mut post_digest_launchd_drift,
        )
        .is_err()
    );
    assert_eq!(post_digest_launchd_drift.state_inspections, 2);
    assert_eq!(post_digest_launchd_drift.digest_inspections, 1);
}

#[derive(Debug)]
struct RecordDriftInspector {
    store: MacosOwnerStore,
    pid: u32,
}

impl MacosDirectLaunchdInspector for RecordDriftInspector {
    fn inspect_direct_launchd(
        &mut self,
    ) -> Result<MacosDirectLaunchdState, MacosOwnerExecutionError> {
        Ok(MacosDirectLaunchdState::Loaded { pid: self.pid })
    }

    fn live_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
    ) -> Result<bool, MacosOwnerExecutionError> {
        Ok(true)
    }

    fn publication_identity_matches(
        &mut self,
        _identity: &MacosOwnerIdentity,
        _executable: &MacosDirectLaunchdExecutableExpectation,
    ) -> Result<bool, MacosOwnerExecutionError> {
        self.store
            .publish_owner(
                MacosDaemonOwner::Homebrew,
                identity(
                    "replacement",
                    "/opt/homebrew/bin/hypercolor-daemon",
                    "replacement-requirement",
                    82,
                ),
            )
            .map_err(|error| MacosOwnerExecutionError::new(error.to_string()))?;
        Ok(true)
    }
}

#[test]
fn exact_wait_returns_the_newer_corroborated_record() {
    let directory = tempfile::tempdir().expect("temporary owner directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let prior = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity(
                "prior",
                "/opt/hypercolor/units/prior/bin/hypercolor-daemon",
                "prior-requirement",
                40,
            ),
        )
        .expect("prior owner should publish");
    let expected = expectation(prior.owner_epoch);
    let publisher_store = store.clone();
    let publisher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        publisher_store
            .publish_owner(
                MacosDaemonOwner::DirectLaunchd,
                identity(
                    "candidate",
                    "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
                    "candidate-requirement",
                    81,
                ),
            )
            .expect("candidate owner should publish")
    });

    let mut inspector = FakeInspector::loaded(81);
    let matched = wait_for_exact_direct_launchd_publication(
        &store,
        &expected,
        Duration::from_secs(1),
        &mut inspector,
    )
    .expect("exact wait should succeed")
    .expect("candidate should publish before timeout");
    let published = publisher.join().expect("publisher should finish");
    assert_eq!(matched, published);
    assert_eq!(matched.owner_epoch, prior.owner_epoch + 1);
    assert_eq!(inspector.state_inspections, 2);
    assert_eq!(inspector.identity_inspections, 0);
    assert_eq!(inspector.digest_inspections, 1);
}

#[test]
fn exact_wait_times_out_on_a_stale_publication_without_inspecting_a_pid() {
    let directory = tempfile::tempdir().expect("temporary owner directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let current = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity(
                "candidate",
                "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
                "candidate-requirement",
                81,
            ),
        )
        .expect("current owner should publish");
    let mut inspector = FakeInspector::loaded(81);
    assert_eq!(
        wait_for_exact_direct_launchd_publication(
            &store,
            &expectation(current.owner_epoch),
            Duration::ZERO,
            &mut inspector,
        )
        .expect("zero timeout should be clean"),
        None
    );
    assert_eq!(inspector.state_inspections, 0);
    assert_eq!(inspector.identity_inspections, 0);
}

#[test]
fn exact_wait_rejects_a_record_replaced_during_corroboration() {
    let directory = tempfile::tempdir().expect("temporary owner directory should build");
    let store = MacosOwnerStore::new(directory.path());
    let prior = store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity(
                "prior",
                "/opt/hypercolor/units/prior/bin/hypercolor-daemon",
                "prior-requirement",
                40,
            ),
        )
        .expect("prior owner should publish");
    store
        .publish_owner(
            MacosDaemonOwner::DirectLaunchd,
            identity(
                "candidate",
                "/opt/hypercolor/units/candidate/bin/hypercolor-daemon",
                "candidate-requirement",
                81,
            ),
        )
        .expect("candidate owner should publish");
    let mut inspector = RecordDriftInspector {
        store: store.clone(),
        pid: 81,
    };

    assert_eq!(
        wait_for_exact_direct_launchd_publication(
            &store,
            &expectation(prior.owner_epoch),
            Duration::ZERO,
            &mut inspector,
        )
        .expect("record drift should fail closed without an execution error"),
        None
    );
    assert_eq!(
        store
            .load_owner_record()
            .expect("replacement record should remain readable")
            .expect("replacement record should exist")
            .active_owner,
        MacosDaemonOwner::Homebrew
    );
}

#[test]
fn expectation_rejects_unbounded_or_nonabsolute_identity_fields() {
    assert!(
        MacosDirectLaunchdExecutableExpectation::new(
            PathBuf::from("relative"),
            "hash",
            hex_digest(b"hash"),
            CANDIDATE_SHA256,
            0o555,
            1,
            1,
            1,
        )
        .is_err()
    );
    assert!(
        MacosDirectLaunchdExecutableExpectation::new(
            PathBuf::from("/absolute"),
            "",
            hex_digest(b""),
            CANDIDATE_SHA256,
            0o555,
            1,
            1,
            1,
        )
        .is_err()
    );
    assert!(
        MacosDirectLaunchdExecutableExpectation::new(
            PathBuf::from("/absolute"),
            "requirement",
            hex_digest(b"requirement"),
            "not-a-sha256",
            0o555,
            1,
            1,
            1,
        )
        .is_err()
    );
}
