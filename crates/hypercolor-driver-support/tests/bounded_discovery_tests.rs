//! Admission and probing bounds, and the gaps they report.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use hypercolor_driver_api::{CoverageGap, DiscoveredDevice, DiscoveryCoverage};
use hypercolor_driver_api::{DiscoveryConnectBehavior, DiscoveryObservation};
use hypercolor_driver_support::discovery::{
    Admission, CandidateAdmission, KnownTarget, ProbeBudget, Probed, ScanOutcome, probe_candidates,
};
use hypercolor_types::device::{
    ConnectionType, DeviceCapabilities, DeviceFamily, DeviceFingerprint, DeviceId, DeviceInfo,
    DeviceOrigin,
};
use tokio::time::Instant;

fn budget(after: Duration, in_flight: usize) -> ProbeBudget {
    ProbeBudget {
        deadline: Instant::now() + after,
        in_flight: NonZeroUsize::new(in_flight).expect("tests probe at least one at a time"),
    }
}

const SOURCE: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));

fn host(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
}

#[test]
fn admission_retains_nothing_past_its_limit() {
    let mut admission = CandidateAdmission::new(2, []);
    assert_eq!(admission.offer(SOURCE, 1, "a"), Admission::Added);
    assert_eq!(admission.offer(SOURCE, 2, "b"), Admission::Added);
    for key in 3..1_000 {
        assert_eq!(admission.offer(SOURCE, key, "flood"), Admission::Full);
    }

    assert_eq!(admission.len(), 2);
    let outcome = admission.finish();
    assert_eq!(outcome.found, ["a", "b"]);
    assert_eq!(outcome.gaps, [CoverageGap::AdmissionFull { refused: 997 }]);
}

#[test]
fn a_full_admission_that_refused_nothing_leaves_no_gap() {
    let mut admission = CandidateAdmission::new(1, []);
    assert_eq!(admission.offer(SOURCE, "only", 1), Admission::Added);
    assert_eq!(admission.offer(SOURCE, "only", 2), Admission::Duplicate);

    let outcome = admission.finish();
    assert_eq!(outcome.found, [1]);
    assert!(
        outcome.gaps.is_empty(),
        "repeats of an admitted key are not refusals"
    );
}

#[test]
fn offers_keep_the_first_value_and_latest_offers_replace_it() {
    let mut admission = CandidateAdmission::new(1, []);
    admission.offer(SOURCE, "key", "first");
    assert_eq!(
        admission.offer(SOURCE, "key", "second"),
        Admission::Duplicate
    );
    assert_eq!(admission.values().copied().collect::<Vec<_>>(), ["first"]);

    assert_eq!(
        admission.offer_latest(SOURCE, "key", "latest"),
        Admission::Updated
    );
    assert_eq!(admission.values().copied().collect::<Vec<_>>(), ["latest"]);
}

#[test]
fn a_flood_cannot_crowd_out_a_tracked_identity() {
    let mut admission = CandidateAdmission::new(2, [KnownTarget::Identity(1_000)]);
    for key in 0..500 {
        admission.offer(SOURCE, key, key);
    }
    assert_eq!(
        admission.offer(SOURCE, 1_000, 1_000),
        Admission::Added,
        "a tracked identity is admitted however full admission is"
    );
    assert!(admission.contains(&1_000));

    let outcome = admission.finish();
    assert_eq!(
        outcome.found,
        [1_000, 0, 1],
        "known targets come first, so they are probed first"
    );
    assert_eq!(outcome.gaps, [CoverageGap::AdmissionFull { refused: 498 }]);
}

#[test]
fn a_flood_cannot_crowd_out_a_configured_address_of_unknown_identity() {
    let configured = host(50);
    let mut admission = CandidateAdmission::new(2, [KnownTarget::Address(configured)]);
    for key in 0..500 {
        admission.offer(SOURCE, key, key);
    }
    assert_eq!(
        admission.offer(configured, 9_000, 9_000),
        Admission::Added,
        "the reply from a configured address has a slot waiting"
    );

    let outcome = admission.finish();
    assert_eq!(outcome.found, [9_000, 0, 1]);
}

#[test]
fn a_configured_address_holds_one_slot_however_many_identities_it_claims() {
    let configured = host(50);
    let mut admission = CandidateAdmission::new(1, [KnownTarget::Address(configured)]);
    for key in 0..500 {
        admission.offer(configured, key, key);
    }

    assert_eq!(admission.len(), 2, "its slot plus the unsolicited limit");
    assert_eq!(
        admission.finish().gaps,
        [CoverageGap::AdmissionFull { refused: 498 }]
    );
}

#[test]
fn known_targets_do_not_use_up_the_unsolicited_limit() {
    let mut admission = CandidateAdmission::new(1, [KnownTarget::Identity("tracked")]);
    assert_eq!(admission.offer(SOURCE, "tracked", 1), Admission::Added);
    assert_eq!(
        admission.offer(SOURCE, "stranger", 2),
        Admission::Added,
        "the limit bounds unsolicited candidates, not the known inventory"
    );
    assert_eq!(admission.offer(SOURCE, "another", 3), Admission::Full);
}

#[tokio::test]
async fn probing_reports_results_and_gaps_apart() {
    let batch = probe_candidates(
        1..=4,
        budget(Duration::from_secs(5), 4),
        |candidate: i32| async move {
            match candidate {
                1 => Ok(Probed::Described(candidate * 10)),
                2 => Ok(Probed::Present(DeviceFingerprint::from_persisted(
                    "net:fixture:lamp".to_owned(),
                ))),
                3 => Ok(Probed::Nothing),
                _ => Err("refused"),
            }
        },
    )
    .await;

    assert_eq!(
        batch.found,
        [10],
        "a probe with nothing to report is silent"
    );
    assert_eq!(batch.present.len(), 1);
    assert_eq!(
        batch.gaps,
        vec![CoverageGap::ProbeFailed {
            candidate: "4".to_owned(),
            reason: "refused".to_owned(),
        }]
    );
}

#[tokio::test]
async fn probing_never_exceeds_its_concurrency() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let batch = probe_candidates(
        0..40,
        budget(Duration::from_secs(5), 3),
        |candidate: u32| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                tokio::time::sleep(Duration::from_millis(2)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, Infallible>(Probed::Described(candidate))
            }
        },
    )
    .await;

    assert_eq!(batch.found.len(), 40);
    assert!(batch.gaps.is_empty());
    assert_eq!(peak.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn candidates_are_probed_in_the_order_given() {
    let started = Arc::new(std::sync::Mutex::new(Vec::new()));
    probe_candidates(
        ["tracked", "stranger"],
        budget(Duration::from_secs(5), 1),
        {
            let started = Arc::clone(&started);
            move |candidate: &'static str| {
                started.lock().expect("lock").push(candidate);
                async { Ok::<Probed<()>, Infallible>(Probed::Nothing) }
            }
        },
    )
    .await;

    assert_eq!(*started.lock().expect("lock"), ["tracked", "stranger"]);
}

/// Sets its flag when dropped, as a cancelled probe's future is.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn the_deadline_cancels_what_is_left_and_reports_it() {
    let dropped = Arc::new(AtomicBool::new(false));
    let batch = probe_candidates(0..5, budget(Duration::from_secs(1), 2), |candidate: u32| {
        let dropped = Arc::clone(&dropped);
        async move {
            if candidate == 0 {
                return Ok::<_, Infallible>(Probed::Described(candidate));
            }
            let _flag = DropFlag(dropped);
            std::future::pending::<()>().await;
            Ok(Probed::Nothing)
        }
    })
    .await;

    assert_eq!(batch.found, [0], "what finished in time still counts");
    assert_eq!(
        batch.gaps,
        vec![CoverageGap::ProbesCancelled { unfinished: 4 }],
        "probes running at the deadline and probes never started both count"
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "cancelled probes are joined before the batch returns"
    );
}

#[tokio::test]
async fn an_expired_budget_starts_nothing_and_reports_every_candidate() {
    let started = Arc::new(AtomicUsize::new(0));
    let batch = probe_candidates(0..5, budget(Duration::ZERO, 2), |candidate: u32| {
        started.fetch_add(1, Ordering::SeqCst);
        async move { Ok::<_, Infallible>(Probed::Described(candidate)) }
    })
    .await;

    assert_eq!(started.load(Ordering::SeqCst), 0, "no probe was created");
    assert!(batch.found.is_empty());
    assert_eq!(
        batch.gaps,
        vec![CoverageGap::ProbesCancelled { unfinished: 5 }]
    );
}

#[tokio::test]
async fn an_expired_budget_with_nothing_to_probe_leaves_no_gap() {
    let batch = probe_candidates(
        Vec::<u32>::new(),
        budget(Duration::ZERO, 2),
        |candidate: u32| async move { Ok::<_, Infallible>(Probed::Described(candidate)) },
    )
    .await;

    assert!(batch.gaps.is_empty());
}

#[tokio::test]
async fn a_panicking_probe_is_a_gap_and_the_rest_still_run() {
    let batch = probe_candidates(
        [0, 1],
        budget(Duration::from_secs(5), 1),
        |candidate: u32| async move {
            assert_ne!(candidate, 0, "this probe panics on purpose");
            Ok::<_, Infallible>(Probed::Described(candidate))
        },
    )
    .await;

    assert_eq!(batch.found, [1]);
    assert!(
        matches!(batch.gaps.as_slice(), [CoverageGap::ProbeFailed { .. }]),
        "{:?}",
        batch.gaps
    );
}

fn described(name: &str) -> DiscoveredDevice {
    DiscoveredDevice {
        fingerprint: DeviceFingerprint::from_persisted(format!("net:fixture:{name}")),
        connect_behavior: DiscoveryConnectBehavior::AutoConnect,
        info: DeviceInfo {
            id: DeviceId::new(),
            name: name.to_owned(),
            vendor: "Fixture".to_owned(),
            family: DeviceFamily::new_static("fixture", "Fixture"),
            model: None,
            connection_type: ConnectionType::Network,
            origin: DeviceOrigin::native("fixture", "fixture", ConnectionType::Network),
            segments: Vec::new(),
            firmware_version: None,
            capabilities: DeviceCapabilities::default(),
        },
        metadata: std::collections::HashMap::new(),
        claim: None,
    }
}

#[test]
fn an_outcome_becomes_a_batch_of_its_finds_and_presence() {
    let tracked = DeviceFingerprint::from_persisted("net:fixture:tracked".to_owned());
    let outcome = ScanOutcome {
        found: vec!["lamp"],
        present: vec![tracked.clone()],
        gaps: Vec::new(),
    };

    let batch = outcome.into_batch(described);

    assert_eq!(batch.coverage, DiscoveryCoverage::Complete);
    assert!(matches!(
        batch.observations.as_slice(),
        [
            DiscoveryObservation::Device(device),
            DiscoveryObservation::Present(fingerprint),
        ] if device.info.name == "lamp" && *fingerprint == tracked
    ));
}

#[test]
fn an_outcome_with_gaps_becomes_a_partial_batch() {
    let gap = CoverageGap::ProbesCancelled { unfinished: 1 };
    let outcome = ScanOutcome::<&str> {
        gaps: vec![gap.clone()],
        ..ScanOutcome::default()
    };

    let batch = outcome.into_batch(described);

    assert!(batch.observations.is_empty());
    assert_eq!(batch.coverage, DiscoveryCoverage::Partial(vec![gap]));
}
