//! The discovery contract: observations, coverage, and what each one means.

use std::io;

use hypercolor_driver_api::{CoverageGap, DiscoveryBatch, DiscoveryCoverage, DiscoveryObservation};
use hypercolor_types::device::DeviceFingerprint;

fn send_gap() -> CoverageGap {
    CoverageGap::SendFailed {
        target: "192.0.2.1:9".to_owned(),
    }
}

#[test]
fn coverage_is_complete_only_without_gaps() {
    assert_eq!(
        DiscoveryCoverage::from_gaps(Vec::new()),
        DiscoveryCoverage::Complete
    );
    assert_eq!(
        DiscoveryCoverage::from_gaps(vec![send_gap()]),
        DiscoveryCoverage::Partial(vec![send_gap()])
    );
}

#[test]
fn coverage_lists_its_gaps() {
    assert!(DiscoveryCoverage::Complete.gaps().is_empty());
    assert_eq!(
        DiscoveryCoverage::Partial(vec![send_gap()]).gaps(),
        [send_gap()]
    );
}

#[test]
fn a_complete_batch_carries_no_gaps() {
    let batch = DiscoveryBatch::complete([]);
    assert!(batch.observations.is_empty());
    assert_eq!(batch.coverage, DiscoveryCoverage::Complete);
}

#[test]
fn a_batch_earns_its_coverage_from_its_gaps() {
    let present = DiscoveryObservation::Present(DeviceFingerprint::from_persisted(
        "net:fixture:lamp".to_owned(),
    ));

    let batch = DiscoveryBatch::new(vec![present], vec![send_gap()]);

    assert_eq!(batch.observations.len(), 1);
    assert_eq!(batch.coverage, DiscoveryCoverage::Partial(vec![send_gap()]));
}

#[test]
fn a_failing_socket_leaves_a_gap_but_a_peer_saying_nothing_listens_does_not() {
    assert!(matches!(
        CoverageGap::from_receive_error(&io::Error::from(io::ErrorKind::PermissionDenied)),
        Some(CoverageGap::ReceiveFailed { .. })
    ));
    for silent in [
        io::ErrorKind::ConnectionReset,
        io::ErrorKind::ConnectionRefused,
    ] {
        assert_eq!(
            CoverageGap::from_receive_error(&io::Error::from(silent)),
            None
        );
    }
}

#[test]
fn only_a_device_gap_names_the_device_it_holds() {
    let fingerprint = DeviceFingerprint::from_persisted("net:fixture:lamp".to_owned());
    let held = CoverageGap::DeviceUnverified {
        fingerprint: fingerprint.clone(),
        reason: "did not answer".to_owned(),
    };

    assert_eq!(held.device(), Some(&fingerprint));
    assert_eq!(send_gap().device(), None);
    assert_eq!(CoverageGap::AdmissionFull { refused: 1 }.device(), None);
    assert_eq!(
        held.to_string(),
        "net:fixture:lamp could not be verified: did not answer"
    );
}

#[test]
fn gaps_read_as_sentences() {
    assert_eq!(send_gap().to_string(), "send to 192.0.2.1:9 failed");
    assert_eq!(
        CoverageGap::AdmissionFull { refused: 3 }.to_string(),
        "admission refused 3 candidates past its limit"
    );
    assert_eq!(
        CoverageGap::ProbesCancelled { unfinished: 2 }.to_string(),
        "2 probes did not finish before the scan deadline"
    );
}
