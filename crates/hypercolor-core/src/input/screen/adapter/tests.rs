use super::{
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureOwnedSource, CapturePublication, CapturePublicationEpoch,
    CapturePublicationSource, begin_capture_exact_retirement,
};
use crate::input::screen::{
    CaptureSourceId, ExactBoxList, ScreenCommittedState, ScreenPublicationHub,
    ScreenPublicationSlotPolicy,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct FakeExactEndpoint {
    commands: Arc<Mutex<Vec<CaptureExactCommand>>>,
    wakes: Arc<AtomicUsize>,
    reject: bool,
}

impl CaptureExactCommandEndpoint for FakeExactEndpoint {
    const SOURCE_NAME: &'static str = "fake capture";

    fn send_exact(&self, command: CaptureExactCommand) -> Result<(), CaptureExactCommandRejected> {
        if self.reject {
            return Err(CaptureExactCommandRejected);
        }
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command);
        Ok(())
    }

    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FakeSource {
    id: CaptureSourceId,
    incarnation: u64,
}

impl CapturePublicationSource for FakeSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.id
    }
}

struct FakeOwnedSource {
    id: CaptureSourceId,
}

impl CaptureOwnedSource for FakeOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.id
    }

    fn belongs_to_authority(&self, _authority: &ScreenCommittedState) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeEpoch {
    source: u64,
    activity: u64,
    session: u64,
    topology: u64,
    resource: u64,
}

impl CapturePublicationEpoch for FakeEpoch {
    fn source_generation(&self) -> u64 {
        self.source
    }

    fn activity_generation(&self) -> u64 {
        self.activity
    }
}

#[test]
fn publication_fences_every_epoch_dimension_and_keeps_only_the_latest_value() {
    let initial = FakeEpoch {
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    let mut publication = CapturePublication::default();

    assert!(publication.activate(initial));
    assert!(publication.publish(&initial, "first"));
    assert!(publication.publish(&initial, "latest"));
    assert_eq!(publication.latest, Some("latest"));

    for stale in [
        FakeEpoch {
            source: 1,
            ..initial
        },
        FakeEpoch {
            activity: 1,
            ..initial
        },
        FakeEpoch {
            session: 4,
            ..initial
        },
        FakeEpoch {
            topology: 5,
            ..initial
        },
        FakeEpoch {
            resource: 6,
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
        source: 0,
        activity: 0,
        session: 1,
        topology: 2,
        resource: 3,
    };
    let current = FakeEpoch {
        source: 1,
        activity: 1,
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

#[test]
fn exact_publication_state_versions_sources_and_reaps_unowned_incarnations() {
    let state = CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default();
    let first = FakeSource {
        id: CaptureSourceId::new("fake:first").expect("test source id is valid"),
        incarnation: 1,
    };
    let replacement = FakeSource {
        id: CaptureSourceId::new("fake:replacement").expect("test source id is valid"),
        incarnation: 2,
    };

    assert_eq!(state.resolution_revision(), 0);
    state.replace_source(Some(first.clone()));
    assert_eq!(state.resolution_revision(), 1);
    state.replace_source(Some(first.clone()));
    assert_eq!(state.resolution_revision(), 1);
    assert!(state.owns_source(&first.id));

    state.register_owned_source(ExactBoxList::boxed_node(FakeOwnedSource {
        id: first.id.clone(),
    }));
    assert_eq!(state.owned_source_count(), 1);
    state.retain_owned_sources(|source| source.id == first.id);
    state.replace_source(Some(replacement.clone()));
    assert_eq!(state.resolution_revision(), 2);
    assert!(state.owns_source(&first.id));
    assert!(state.owns_source(&replacement.id));

    let hub = Arc::new(ScreenPublicationHub::new(
        ScreenPublicationSlotPolicy::default(),
    ));
    state.install_hub(Arc::clone(&hub));
    assert!(Arc::ptr_eq(
        &state.hub().expect("installed hub remains visible"),
        &hub
    ));
    state.reap_owned_sources();
    assert!(!state.owns_source(&first.id));
    assert!(state.owns_source(&replacement.id));
    state.replace_source(None);
    assert_eq!(state.resolution_revision(), 3);
    assert!(!state.owns_source(&replacement.id));
    state.register_owned_source(ExactBoxList::boxed_node(FakeOwnedSource {
        id: replacement.id.clone(),
    }));
    assert_eq!(state.owned_source_count(), 1);
    state.clear_owned_sources();
    assert_eq!(state.owned_source_count(), 0);
    assert!(!state.owns_source(&replacement.id));
}

#[tokio::test]
async fn exact_retirement_wakes_the_endpoint_and_reports_completion() {
    let endpoint = FakeExactEndpoint::default();
    let retirement = begin_capture_exact_retirement(&endpoint);
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 1);
    let command = endpoint
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .expect("retirement enqueues one exact command");
    let CaptureExactCommand::Reap {
        completion: Some(completion),
    } = command
    else {
        panic!("retirement enqueues a completing reap command");
    };
    completion
        .send(Ok(()))
        .expect("test retirement receiver remains live");
    retirement
        .complete()
        .await
        .expect("completed exact retirement succeeds");
}

#[tokio::test]
async fn rejected_exact_retirement_fails_without_waking_the_endpoint() {
    let endpoint = FakeExactEndpoint {
        reject: true,
        ..FakeExactEndpoint::default()
    };
    let error = begin_capture_exact_retirement(&endpoint)
        .complete()
        .await
        .expect_err("rejected exact retirement fails");
    assert!(error.to_string().contains("fake capture worker rejected"));
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 0);
}
