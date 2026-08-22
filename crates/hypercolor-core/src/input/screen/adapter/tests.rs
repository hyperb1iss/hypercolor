use super::{CapturePublication, CapturePublicationEpoch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeEpoch {
    source_generation: u64,
    activity_generation: u64,
    session_generation: u64,
    topology_generation: u64,
    resource_generation: u64,
}

impl CapturePublicationEpoch for FakeEpoch {
    fn source_generation(&self) -> u64 {
        self.source_generation
    }

    fn activity_generation(&self) -> u64 {
        self.activity_generation
    }
}

#[test]
fn publication_fences_every_epoch_dimension_and_keeps_only_the_latest_value() {
    let initial = FakeEpoch {
        source_generation: 0,
        activity_generation: 0,
        session_generation: 1,
        topology_generation: 2,
        resource_generation: 3,
    };
    let mut publication = CapturePublication::default();

    assert!(publication.activate(initial));
    assert!(publication.publish(&initial, "first"));
    assert!(publication.publish(&initial, "latest"));
    assert_eq!(publication.latest, Some("latest"));

    for stale in [
        FakeEpoch {
            source_generation: 1,
            ..initial
        },
        FakeEpoch {
            activity_generation: 1,
            ..initial
        },
        FakeEpoch {
            session_generation: 4,
            ..initial
        },
        FakeEpoch {
            topology_generation: 5,
            ..initial
        },
        FakeEpoch {
            resource_generation: 6,
            ..initial
        },
    ] {
        assert!(!publication.publish(&stale, "stale"));
        assert_eq!(publication.latest, Some("latest"));
    }
}

#[test]
fn publication_requires_current_source_and_activity_before_reactivation() {
    let previous = FakeEpoch {
        source_generation: 0,
        activity_generation: 0,
        session_generation: 1,
        topology_generation: 2,
        resource_generation: 3,
    };
    let current = FakeEpoch {
        source_generation: 1,
        activity_generation: 1,
        ..previous
    };
    let mut publication = CapturePublication::default();

    assert!(publication.activate(previous));
    assert!(publication.publish(&previous, "previous"));
    publication.fence_source(1);
    publication.fence_activity(1);

    assert!(!publication.activate(previous));
    assert!(!publication.publish(&previous, "late"));
    assert!(publication.active.is_none());
    assert!(publication.latest.is_none());
    assert!(publication.activate(current));
    assert!(publication.publish(&current, "current"));
    assert_eq!(publication.latest, Some("current"));
}
