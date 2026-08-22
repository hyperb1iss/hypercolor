use super::{
    CaptureExactCommand, CaptureExactCommandEndpoint, CaptureExactCommandRejected,
    CaptureExactPublicationShared, CaptureExactRuntimeCollection, CaptureExactRuntimeOwner,
    CaptureExactRuntimeStore, CaptureOwnedSource, CapturePublication, CapturePublicationFence,
    CapturePublicationSource, CaptureSessionAuthority, VersionedCaptureSettings,
    begin_capture_exact_retirement, exact_preparation_abort, reap_capture_exact_runtimes,
};
use crate::input::screen::{
    CaptureSourceId, ExactBoxList, PixelExtent, ScreenCaptureDemand, ScreenCommittedState,
    ScreenPublicationHub, ScreenPublicationSlotPolicy, ScreenWorkerBinding,
};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone)]
struct FakeExactEndpoint {
    commands: Arc<Mutex<Vec<CaptureExactCommand>>>,
    wakes: Arc<AtomicUsize>,
    authority: Arc<AtomicU64>,
    reject: bool,
}

impl Default for FakeExactEndpoint {
    fn default() -> Self {
        Self {
            commands: Arc::default(),
            wakes: Arc::default(),
            authority: Arc::new(AtomicU64::new(1)),
            reject: false,
        }
    }
}

impl CaptureExactCommandEndpoint for FakeExactEndpoint {
    const SOURCE_NAME: &'static str = "fake capture";

    fn authority(&self) -> CaptureSessionAuthority {
        CaptureSessionAuthority::new(self.authority.load(Ordering::Acquire))
    }

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

#[test]
fn preparation_abort_reaps_the_authority_that_started_the_transaction() {
    let endpoint = FakeExactEndpoint::default();
    let original_authority = endpoint.authority();
    let cancelled = Arc::new(AtomicBool::new(false));
    let abort =
        exact_preparation_abort(endpoint.clone(), original_authority, Arc::clone(&cancelled));

    endpoint.authority.store(2, Ordering::Release);
    abort();

    assert!(cancelled.load(Ordering::Acquire));
    let command = endpoint
        .commands
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop()
        .expect("preparation abort enqueues one exact command");
    let CaptureExactCommand::Reap {
        authority,
        completion: None,
    } = command
    else {
        panic!("preparation abort enqueues a noncompleting reap command");
    };
    assert_eq!(authority, original_authority);
    assert_eq!(endpoint.wakes.load(Ordering::Relaxed), 1);
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

struct ProbeRuntime {
    source: FakeSource,
    binding: ScreenWorkerBinding,
}

impl CaptureExactRuntimeOwner for ProbeRuntime {
    type Source = FakeSource;

    const BACKEND_NAME: &'static str = "probe capture";
    const ABORTED_BINDING_ERROR: &'static str = "probe binding aborted";

    fn source(&self) -> &Self::Source {
        &self.source
    }

    fn binding(&self) -> &ScreenWorkerBinding {
        &self.binding
    }

    fn bind_routes(&mut self, _authority: &ScreenCommittedState) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn is_bound(&self) -> bool {
        false
    }
}

struct RetainProbeStore {
    runtime_count: usize,
    retain_calls: usize,
}

impl CaptureExactRuntimeCollection<ProbeRuntime> for RetainProbeStore {
    fn iter_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut ProbeRuntime>
    where
        ProbeRuntime: 'a,
    {
        std::iter::empty()
    }
}

impl CaptureExactRuntimeStore<ProbeRuntime> for RetainProbeStore {
    type Prepared = ProbeRuntime;

    fn prepare(runtime: ProbeRuntime) -> Self::Prepared {
        runtime
    }

    fn install(&mut self, _prepared: Self::Prepared) {
        self.runtime_count += 1;
    }

    fn retain(&mut self, _retain: impl FnMut(&ProbeRuntime) -> bool) {
        self.retain_calls += 1;
        self.runtime_count = 0;
    }
}

struct ReentrantOwnedSource {
    id: CaptureSourceId,
    state: Weak<CaptureExactPublicationShared<FakeSource, Self>>,
    dropped: Arc<AtomicBool>,
}

struct OrderedDrop {
    id: u64,
    drops: Arc<Mutex<Vec<u64>>>,
}

impl Drop for OrderedDrop {
    fn drop(&mut self) {
        self.drops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(self.id);
    }
}

impl CaptureOwnedSource for ReentrantOwnedSource {
    fn source_id(&self) -> &CaptureSourceId {
        &self.id
    }

    fn belongs_to_authority(&self, _authority: &ScreenCommittedState) -> bool {
        false
    }
}

impl Drop for ReentrantOwnedSource {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let _ = state.source();
        }
        self.dropped.store(true, Ordering::Release);
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FakeFence {
    source: u64,
    activity: u64,
}

#[test]
fn versioned_settings_commit_one_coherent_config_and_demand_snapshot() {
    let settings =
        VersionedCaptureSettings::new(String::from("initial"), ScreenCaptureDemand::Inactive);

    assert_eq!(settings.revision(), 0);
    let initial = settings.snapshot();
    assert_eq!(initial.config, "initial");
    assert_eq!(initial.demand, ScreenCaptureDemand::Inactive);

    let active = ScreenCaptureDemand::active(
        PixelExtent::new(64, 32).expect("test capture extent is nonzero"),
    );
    let mut values = settings.lock();
    values.config_mut().clone_from(&String::from("committed"));
    assert_eq!(values.config(), "committed");
    *values.demand_mut() = active;
    assert_eq!(values.commit(), 1);

    let committed = settings.snapshot();
    assert_eq!(committed.config, "committed");
    assert_eq!(committed.demand, active);
    assert_eq!(settings.commit_revision(), 2);
    assert_eq!(settings.bump_revision(), 3);
    assert_eq!(settings.revision(), 3);

    settings.lock_config().clone_from(&String::from("direct"));
    assert_eq!(settings.lock_config().as_str(), "direct");
    *settings.lock_demand() = ScreenCaptureDemand::Inactive;
    assert_eq!(
        *settings
            .try_lock_demand()
            .expect("demand lock should be available"),
        ScreenCaptureDemand::Inactive
    );
}

impl CapturePublicationFence<FakeEpoch> for FakeFence {
    fn admits(&self, epoch: &FakeEpoch) -> bool {
        epoch.source == self.source && epoch.activity == self.activity
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
    let mut publication = CapturePublication::<FakeFence, _, _>::default();

    assert_eq!(publication.fence(), &FakeFence::default());
    assert!(
        publication
            .replace_fence_if_changed(FakeFence::default())
            .is_none()
    );
    assert!(publication.activate(initial).is_ok());
    assert!(publication.is_active(&initial));
    assert!(publication.publish(&initial, "first").is_ok());
    assert!(publication.publish(&initial, "latest").is_ok());
    assert_eq!(publication.latest(), Some(&"latest"));
    assert_eq!(
        publication
            .snapshot()
            .expect("latest publication remains visible")
            .revision,
        2
    );

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
        assert!(publication.publish(&stale, "stale").is_err());
        assert_eq!(publication.latest(), Some(&"latest"));
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
    let mut publication = CapturePublication::<FakeFence, _, _>::default();

    assert!(publication.activate(previous).is_ok());
    assert!(publication.publish(&previous, "previous").is_ok());
    let checkpoint = publication.checkpoint();
    let displaced = publication.replace_fence(FakeFence {
        source: 1,
        activity: 1,
    });
    assert_eq!(displaced.latest, Some("previous"));

    assert!(publication.activate(previous).is_err());
    assert!(publication.publish(&previous, "late").is_err());
    assert!(publication.active().is_none());
    assert!(publication.latest().is_none());
    assert!(publication.activate(current).is_ok());
    assert!(matches!(
        publication.restore_checkpoint(Some(&current), checkpoint),
        Ok(None)
    ));
    let snapshot = publication.snapshot().expect("restored value is visible");
    assert_eq!(snapshot.epoch, current);
    assert_eq!(snapshot.revision, 2);
    assert_eq!(snapshot.value, "previous");
    assert!(publication.publish(&current, "current").is_ok());
    assert_eq!(publication.latest(), Some(&"current"));
}

#[test]
fn exact_publication_state_versions_sources_and_reaps_unowned_incarnations() {
    let state = CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default();
    let first_authority = CaptureSessionAuthority::new(1);
    let successor_authority = CaptureSessionAuthority::new(2);
    let first = FakeSource {
        id: CaptureSourceId::new("fake:first").expect("test source id is valid"),
        incarnation: 1,
    };
    let replacement = FakeSource {
        id: CaptureSourceId::new("fake:replacement").expect("test source id is valid"),
        incarnation: 2,
    };

    assert_eq!(state.resolution_revision(), 0);
    drop(state.activate_authority(first_authority));
    assert!(state.is_current_authority(first_authority));
    state.replace_source_if_current(first_authority, Some(first.clone()));
    assert_eq!(state.resolution_revision(), 1);
    state.replace_source_if_current(first_authority, Some(first.clone()));
    assert_eq!(state.resolution_revision(), 1);
    assert!(state.owns_source(&first.id));

    assert!(state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: first.id.clone(),
        }),
    ));
    assert_eq!(state.owned_source_count(), 1);
    assert!(state.retain_owned_sources_if_current(first_authority, |source| source.id == first.id));
    state.replace_source_if_current(first_authority, Some(replacement.clone()));
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
    assert!(state.reap_owned_sources_if_current(first_authority));
    assert!(!state.owns_source(&first.id));
    assert!(state.owns_source(&replacement.id));
    state.replace_source_if_current(first_authority, None);
    assert_eq!(state.resolution_revision(), 3);
    assert!(!state.owns_source(&replacement.id));
    assert!(state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: replacement.id.clone(),
        }),
    ));
    assert_eq!(state.owned_source_count(), 1);
    assert!(state.clear_owned_sources_if_current(first_authority));
    assert_eq!(state.owned_source_count(), 0);
    assert!(!state.owns_source(&replacement.id));

    state.replace_source_if_current(first_authority, Some(first.clone()));
    assert_eq!(state.resolution_revision(), 4);
    drop(state.activate_authority(successor_authority));
    assert!(state.is_current_authority(successor_authority));
    assert_eq!(state.resolution_revision(), 5);
    assert!(!state.replace_source_if_current(first_authority, Some(replacement.clone())));
    assert!(!state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: replacement.id.clone(),
        }),
    ));
    assert!(!state.clear_owned_sources_if_current(first_authority));
    assert_eq!(state.resolution_revision(), 5);
    assert!(state.source().is_none());
    assert_eq!(state.owned_source_count(), 0);
}

#[test]
fn authority_displacement_drops_owned_sources_after_releasing_ledger_locks() {
    let state = Arc::new(CaptureExactPublicationShared::<
        FakeSource,
        ReentrantOwnedSource,
    >::default());
    let first_authority = CaptureSessionAuthority::new(1);
    let successor_authority = CaptureSessionAuthority::new(2);
    drop(state.activate_authority(first_authority));
    let dropped = Arc::new(AtomicBool::new(false));
    assert!(state.register_owned_source_if_current(
        first_authority,
        ExactBoxList::boxed_node(ReentrantOwnedSource {
            id: CaptureSourceId::new("fake:reentrant").expect("test source id is valid"),
            state: Arc::downgrade(&state),
            dropped: Arc::clone(&dropped),
        }),
    ));

    let displaced = state.activate_authority(successor_authority);
    assert!(!dropped.load(Ordering::Acquire));
    drop(displaced);
    assert!(dropped.load(Ordering::Acquire));
    assert!(state.is_current_authority(successor_authority));
}

#[test]
fn extracted_nodes_keep_their_original_destruction_order() {
    let drops = Arc::new(Mutex::new(Vec::new()));
    let mut nodes = ExactBoxList::default();
    for id in [1, 2, 3] {
        nodes.push_boxed(ExactBoxList::boxed_node(OrderedDrop {
            id,
            drops: Arc::clone(&drops),
        }));
    }

    let extracted = nodes.extract_if(|_| true);
    drop(extracted);

    assert_eq!(
        *drops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [3, 2, 1]
    );
}

#[test]
fn stale_reap_leaves_successor_owned_sources_and_runtimes_intact() {
    let state = CaptureExactPublicationShared::<FakeSource, FakeOwnedSource>::default();
    let stale_authority = CaptureSessionAuthority::new(1);
    let successor_authority = CaptureSessionAuthority::new(2);
    drop(state.activate_authority(successor_authority));
    assert!(state.register_owned_source_if_current(
        successor_authority,
        ExactBoxList::boxed_node(FakeOwnedSource {
            id: CaptureSourceId::new("fake:successor").expect("test source id is valid"),
        }),
    ));
    let mut runtimes = RetainProbeStore {
        runtime_count: 1,
        retain_calls: 0,
    };

    reap_capture_exact_runtimes(stale_authority, &mut runtimes, &state);

    assert_eq!(state.owned_source_count(), 1);
    assert_eq!(runtimes.runtime_count, 1);
    assert_eq!(runtimes.retain_calls, 0);

    reap_capture_exact_runtimes(successor_authority, &mut runtimes, &state);
    assert_eq!(state.owned_source_count(), 0);
    assert_eq!(runtimes.runtime_count, 0);
    assert_eq!(runtimes.retain_calls, 1);
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
        ..
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
