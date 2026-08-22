use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::Context;
use arc_swap::ArcSwapOption;
use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::input::{InputManager, SourceCapabilityConflict, TryInputManagerIntent};
use hypercolor_types::event::{
    HypercolorEvent, MacosDaemonHandoverPhaseEvent, MacosDaemonOwnerConflictEvent,
    MacosDaemonOwnerEvent, MacosDaemonOwnerRecoveryRequiredEvent,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tracing::warn;

use crate::macos_owner::{
    MacosDaemonOwner, MacosHandoverPhase, MacosOwnerRecord, MacosOwnerSnapshot, MacosOwnerStore,
};

const WATCH_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

enum WatchSignal {
    Changed,
}

#[derive(Clone)]
pub(crate) struct MacosOwnerPublication {
    snapshot: MacosOwnerSnapshot,
    designated_requirement_hash: Option<Arc<str>>,
}

impl MacosOwnerPublication {
    fn from_record(record: &MacosOwnerRecord, startup_snapshot: MacosOwnerSnapshot) -> Self {
        Self {
            snapshot: snapshot_with_startup_recovery(record, startup_snapshot),
            designated_requirement_hash: Some(Arc::from(
                record.active_identity.designated_requirement_hash.as_str(),
            )),
        }
    }

    pub(crate) fn without_identity(snapshot: MacosOwnerSnapshot) -> Self {
        Self {
            snapshot,
            designated_requirement_hash: None,
        }
    }
}

pub(crate) struct PendingMacosOwnerWatch {
    watcher: RecommendedWatcher,
    signal_tx: SyncSender<WatchSignal>,
    signal_rx: Receiver<WatchSignal>,
    stopping: Arc<AtomicBool>,
    store: MacosOwnerStore,
    snapshots: Arc<ArcSwapOption<MacosOwnerSnapshot>>,
    event_bus: Arc<HypercolorBus>,
    startup_snapshot: MacosOwnerSnapshot,
    reconciled_fingerprint: Option<MacosOwnerIdentityFingerprint>,
}

impl PendingMacosOwnerWatch {
    pub(crate) fn start(
        data_dir: PathBuf,
        snapshots: Arc<ArcSwapOption<MacosOwnerSnapshot>>,
        event_bus: Arc<HypercolorBus>,
        startup_snapshot: MacosOwnerSnapshot,
    ) -> anyhow::Result<Self> {
        let store = MacosOwnerStore::new(&data_dir);
        let (signal_tx, signal_rx) = mpsc::sync_channel(1);
        let callback_tx = signal_tx.clone();
        let callback_owner_record_path = store.owner_record_path();
        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) if event_touches_owner_record(&event, &callback_owner_record_path) => {
                    enqueue_change(&callback_tx);
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "macOS daemon owner watch failed"),
            },
        )
        .context("failed to create the macOS daemon owner watch")?;
        watcher
            .watch(&data_dir, RecursiveMode::NonRecursive)
            .with_context(|| {
                format!(
                    "failed to watch the macOS daemon owner directory {}",
                    data_dir.display()
                )
            })?;
        Ok(Self {
            watcher,
            signal_tx,
            signal_rx,
            stopping: Arc::new(AtomicBool::new(false)),
            store,
            snapshots,
            event_bus,
            startup_snapshot,
            reconciled_fingerprint: None,
        })
    }

    pub(crate) fn reconcile_snapshot(
        &mut self,
        fallback: MacosOwnerSnapshot,
    ) -> Result<MacosOwnerPublication, crate::macos_owner::MacosOwnerStoreError> {
        let Some(record) = self.store.load_owner_record()? else {
            self.reconciled_fingerprint = None;
            return Ok(MacosOwnerPublication::without_identity(fallback));
        };
        self.reconciled_fingerprint = Some(MacosOwnerIdentityFingerprint::from(&record));
        Ok(MacosOwnerPublication::from_record(
            &record,
            self.startup_snapshot,
        ))
    }

    pub(crate) fn attach(self, input_manager: InputManager) -> anyhow::Result<MacosOwnerWatch> {
        let Self {
            watcher,
            signal_tx,
            signal_rx,
            stopping,
            store,
            snapshots,
            event_bus,
            startup_snapshot,
            reconciled_fingerprint,
        } = self;
        let (snapshot_tx, snapshot_rx) = tokio::sync::watch::channel(None);
        let (worker_done_tx, worker_done_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("hypercolor-macos-owner-watch".to_owned())
            .spawn({
                let stopping = Arc::clone(&stopping);
                move || {
                    watch_worker(
                        signal_rx,
                        &stopping,
                        &store,
                        &snapshot_tx,
                        startup_snapshot,
                        reconciled_fingerprint,
                    );
                    let _ = worker_done_tx.try_send(());
                }
            })
            .context("failed to spawn the macOS daemon owner watch worker")?;
        let publisher = tokio::spawn(run_owner_publisher(
            snapshot_rx,
            snapshots,
            input_manager,
            event_bus,
        ));
        Ok(MacosOwnerWatch {
            watcher: Some(watcher),
            signal_tx,
            stopping,
            worker: Some(worker),
            worker_done_rx,
            publisher: Some(publisher),
        })
    }
}

async fn run_owner_publisher(
    mut publications: tokio::sync::watch::Receiver<Option<MacosOwnerPublication>>,
    snapshots: Arc<ArcSwapOption<MacosOwnerSnapshot>>,
    input_manager: InputManager,
    event_bus: Arc<HypercolorBus>,
) {
    let mut pending = None;
    loop {
        if pending.is_none() {
            if publications.changed().await.is_err() {
                return;
            }
            pending = publications.borrow_and_update().clone();
        }
        let Some(publication) = pending.take() else {
            continue;
        };
        let published =
            try_publish_owner_snapshot(&snapshots, &input_manager, &event_bus, publication.clone());
        match published {
            TryInputManagerIntent::Applied(Ok(())) => {}
            TryInputManagerIntent::Applied(Err(error)) => {
                warn!(%error, "failed to publish macOS daemon ownership");
            }
            TryInputManagerIntent::Busy => {
                pending = Some(publication);
                tokio::select! {
                    changed = publications.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        pending = publications.borrow_and_update().clone();
                    }
                    () = input_manager.wait_for_lifecycle_release_after_busy() => {}
                }
            }
            TryInputManagerIntent::Stale => {
                unreachable!("owner capability publication has no stale predicate")
            }
        }
    }
}

pub(crate) struct MacosOwnerWatch {
    watcher: Option<RecommendedWatcher>,
    signal_tx: SyncSender<WatchSignal>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    worker_done_rx: Receiver<()>,
    publisher: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for MacosOwnerWatch {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        drop(self.watcher.take());
        enqueue_change(&self.signal_tx);
        if let Some(publisher) = self.publisher.take() {
            publisher.abort();
        }
        if self
            .worker_done_rx
            .recv_timeout(WATCH_WORKER_SHUTDOWN_TIMEOUT)
            .is_ok()
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
    }
}

fn event_touches_owner_record(event: &notify::Event, owner_record_path: &Path) -> bool {
    event.paths.iter().any(|path| path == owner_record_path)
}

fn enqueue_change(signal_tx: &SyncSender<WatchSignal>) {
    let _ = signal_tx.try_send(WatchSignal::Changed);
}

fn watch_worker(
    signal_rx: Receiver<WatchSignal>,
    stopping: &AtomicBool,
    store: &MacosOwnerStore,
    snapshots: &tokio::sync::watch::Sender<Option<MacosOwnerPublication>>,
    startup_snapshot: MacosOwnerSnapshot,
    mut fingerprint: Option<MacosOwnerIdentityFingerprint>,
) {
    while !stopping.load(Ordering::Acquire) && signal_rx.recv().is_ok() {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) =
            refresh_owner_snapshot(store, snapshots, &mut fingerprint, startup_snapshot)
        {
            warn!(%error, "failed to refresh macOS daemon ownership");
        }
    }
}

pub(crate) fn publish_owner_snapshot(
    snapshots: &ArcSwapOption<MacosOwnerSnapshot>,
    input_manager: &InputManager,
    event_bus: &HypercolorBus,
    publication: MacosOwnerPublication,
) -> anyhow::Result<()> {
    publish_owner_snapshot_with(
        snapshots,
        input_manager,
        publication,
        |published_snapshot| {
            event_bus.publish(owner_event(published_snapshot));
        },
    )
}

fn try_publish_owner_snapshot(
    snapshots: &ArcSwapOption<MacosOwnerSnapshot>,
    input_manager: &InputManager,
    event_bus: &HypercolorBus,
    publication: MacosOwnerPublication,
) -> TryInputManagerIntent<anyhow::Result<()>> {
    let MacosOwnerPublication {
        snapshot,
        designated_requirement_hash,
    } = publication;
    match input_manager.try_set_source_capability_identity(
        capability_owner_id(snapshot.active_owner),
        snapshot.conflict.map(|conflict| SourceCapabilityConflict {
            active: Arc::from(capability_owner_id(conflict.active_owner)),
            contender: Arc::from(capability_owner_id(conflict.contender_owner)),
            observed_at_ms: conflict.observed_at_ms,
        }),
        designated_requirement_hash,
    ) {
        TryInputManagerIntent::Busy => TryInputManagerIntent::Busy,
        TryInputManagerIntent::Stale => TryInputManagerIntent::Stale,
        TryInputManagerIntent::Applied(Err(error)) => TryInputManagerIntent::Applied(Err(error)),
        TryInputManagerIntent::Applied(Ok(())) => {
            snapshots.store(Some(Arc::new(snapshot)));
            event_bus.publish(owner_event(snapshot));
            TryInputManagerIntent::Applied(Ok(()))
        }
    }
}

fn publish_owner_snapshot_with(
    snapshots: &ArcSwapOption<MacosOwnerSnapshot>,
    input_manager: &InputManager,
    publication: MacosOwnerPublication,
    publish_event: impl FnOnce(MacosOwnerSnapshot),
) -> anyhow::Result<()> {
    let MacosOwnerPublication {
        snapshot,
        designated_requirement_hash,
    } = publication;
    input_manager.set_source_capability_identity(
        capability_owner_id(snapshot.active_owner),
        snapshot.conflict.map(|conflict| SourceCapabilityConflict {
            active: Arc::from(capability_owner_id(conflict.active_owner)),
            contender: Arc::from(capability_owner_id(conflict.contender_owner)),
            observed_at_ms: conflict.observed_at_ms,
        }),
        designated_requirement_hash,
    )?;
    snapshots.store(Some(Arc::new(snapshot)));
    publish_event(snapshot);
    Ok(())
}

fn refresh_owner_snapshot(
    store: &MacosOwnerStore,
    snapshots: &tokio::sync::watch::Sender<Option<MacosOwnerPublication>>,
    fingerprint: &mut Option<MacosOwnerIdentityFingerprint>,
    startup_snapshot: MacosOwnerSnapshot,
) -> anyhow::Result<()> {
    let Some(record) = store.load_owner_record()? else {
        return Ok(());
    };
    let next_fingerprint = MacosOwnerIdentityFingerprint::from(&record);
    if fingerprint.as_ref() == Some(&next_fingerprint) {
        return Ok(());
    }
    *fingerprint = Some(next_fingerprint);
    snapshots.send_replace(Some(MacosOwnerPublication::from_record(
        &record,
        startup_snapshot,
    )));
    Ok(())
}

fn snapshot_with_startup_recovery(
    record: &MacosOwnerRecord,
    startup_snapshot: MacosOwnerSnapshot,
) -> MacosOwnerSnapshot {
    let recovery_required = (record.active_owner == startup_snapshot.active_owner
        && record.owner_epoch == startup_snapshot.owner_epoch)
        .then_some(startup_snapshot.recovery_required)
        .flatten();
    record.snapshot().with_recovery_required(recovery_required)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosOwnerIdentityFingerprint {
    active_owner: MacosDaemonOwner,
    owner_epoch: u64,
    active_audit_token_identity: String,
    active_executable_path: PathBuf,
    active_designated_requirement_hash: String,
    active_pid: u32,
    conflict: Option<MacosConflictIdentityFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosConflictIdentityFingerprint {
    active_owner: MacosDaemonOwner,
    active_epoch: u64,
    contender_owner: MacosDaemonOwner,
    audit_token_identity: String,
    executable_path: PathBuf,
    designated_requirement_hash: String,
    pid: u32,
}

impl From<&MacosOwnerRecord> for MacosOwnerIdentityFingerprint {
    fn from(record: &MacosOwnerRecord) -> Self {
        Self {
            active_owner: record.active_owner,
            owner_epoch: record.owner_epoch,
            active_audit_token_identity: record.active_identity.audit_token_identity.clone(),
            active_executable_path: record.active_identity.executable_path.clone(),
            active_designated_requirement_hash: record
                .active_identity
                .designated_requirement_hash
                .clone(),
            active_pid: record.active_identity.pid,
            conflict: record
                .conflict
                .as_ref()
                .map(|conflict| MacosConflictIdentityFingerprint {
                    active_owner: conflict.active_owner,
                    active_epoch: conflict.active_epoch,
                    contender_owner: conflict.contender_owner,
                    audit_token_identity: conflict.contender_identity.audit_token_identity.clone(),
                    executable_path: conflict.contender_identity.executable_path.clone(),
                    designated_requirement_hash: conflict
                        .contender_identity
                        .designated_requirement_hash
                        .clone(),
                    pid: conflict.contender_identity.pid,
                }),
        }
    }
}

const fn capability_owner_id(owner: MacosDaemonOwner) -> &'static str {
    match owner {
        MacosDaemonOwner::AppSidecar => "app_sidecar",
        MacosDaemonOwner::DirectLaunchd => "launchd_service",
        MacosDaemonOwner::Homebrew => "homebrew_service",
        MacosDaemonOwner::Standalone => "standalone",
    }
}

const fn owner_event_owner(owner: MacosDaemonOwner) -> MacosDaemonOwnerEvent {
    match owner {
        MacosDaemonOwner::AppSidecar => MacosDaemonOwnerEvent::AppSidecar,
        MacosDaemonOwner::DirectLaunchd => MacosDaemonOwnerEvent::LaunchdService,
        MacosDaemonOwner::Homebrew => MacosDaemonOwnerEvent::HomebrewService,
        MacosDaemonOwner::Standalone => MacosDaemonOwnerEvent::Standalone,
    }
}

fn owner_event(snapshot: MacosOwnerSnapshot) -> HypercolorEvent {
    HypercolorEvent::MacosDaemonOwnershipChanged {
        active_owner: owner_event_owner(snapshot.active_owner),
        owner_epoch: snapshot.owner_epoch,
        conflict: snapshot
            .conflict
            .map(|conflict| MacosDaemonOwnerConflictEvent {
                active: owner_event_owner(conflict.active_owner),
                contender: owner_event_owner(conflict.contender_owner),
                observed_at_ms: conflict.observed_at_ms,
            }),
        recovery_required: snapshot.recovery_required.map(|recovery| {
            MacosDaemonOwnerRecoveryRequiredEvent {
                requested_owner: owner_event_owner(recovery.requested_owner),
                prior_owner: owner_event_owner(recovery.prior_owner),
                phase: owner_event_phase(recovery.phase),
            }
        }),
    }
}

const fn owner_event_phase(phase: MacosHandoverPhase) -> MacosDaemonHandoverPhaseEvent {
    match phase {
        MacosHandoverPhase::Prepared => MacosDaemonHandoverPhaseEvent::Prepared,
        MacosHandoverPhase::AutostartsConfigured => {
            MacosDaemonHandoverPhaseEvent::AutostartsConfigured
        }
        MacosHandoverPhase::StopRequested => MacosDaemonHandoverPhaseEvent::StopRequested,
        MacosHandoverPhase::OutgoingOwnerStopped => {
            MacosDaemonHandoverPhaseEvent::OutgoingOwnerStopped
        }
        MacosHandoverPhase::AwaitingGuardRelease => {
            MacosDaemonHandoverPhaseEvent::AwaitingGuardRelease
        }
        MacosHandoverPhase::GuardReleased => MacosDaemonHandoverPhaseEvent::GuardReleased,
        MacosHandoverPhase::StartRequested => MacosDaemonHandoverPhaseEvent::StartRequested,
        MacosHandoverPhase::RequestedOwnerStarted => {
            MacosDaemonHandoverPhaseEvent::RequestedOwnerStarted
        }
        MacosHandoverPhase::CommitPending => MacosDaemonHandoverPhaseEvent::CommitPending,
        MacosHandoverPhase::Committed => MacosDaemonHandoverPhaseEvent::Committed,
        MacosHandoverPhase::RollbackPending => MacosDaemonHandoverPhaseEvent::RollbackPending,
        MacosHandoverPhase::RollbackAutostartsRestored => {
            MacosDaemonHandoverPhaseEvent::RollbackAutostartsRestored
        }
        MacosHandoverPhase::RollbackStopRequested => {
            MacosDaemonHandoverPhaseEvent::RollbackStopRequested
        }
        MacosHandoverPhase::RollbackOwnerStopped => {
            MacosDaemonHandoverPhaseEvent::RollbackOwnerStopped
        }
        MacosHandoverPhase::RollbackAwaitingGuardRelease => {
            MacosDaemonHandoverPhaseEvent::RollbackAwaitingGuardRelease
        }
        MacosHandoverPhase::RollbackGuardReleased => {
            MacosDaemonHandoverPhaseEvent::RollbackGuardReleased
        }
        MacosHandoverPhase::RollbackStartRequested => {
            MacosDaemonHandoverPhaseEvent::RollbackStartRequested
        }
        MacosHandoverPhase::PriorOwnerStarted => MacosDaemonHandoverPhaseEvent::PriorOwnerStarted,
        MacosHandoverPhase::RollbackCommitPending => {
            MacosDaemonHandoverPhaseEvent::RollbackCommitPending
        }
        MacosHandoverPhase::RolledBack => MacosDaemonHandoverPhaseEvent::RolledBack,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{
        MacosOwnerIdentityFingerprint, MacosOwnerPublication, PendingMacosOwnerWatch,
        enqueue_change, event_touches_owner_record, owner_event, publish_owner_snapshot_with,
        refresh_owner_snapshot, snapshot_with_startup_recovery, watch_worker,
    };
    use crate::macos_owner::{
        MacosDaemonOwner, MacosHandoverPhase, MacosOwnerIdentity, MacosOwnerRecoveryRequired,
        MacosOwnerStore,
    };
    use arc_swap::ArcSwapOption;
    use hypercolor_core::bus::HypercolorBus;
    use hypercolor_core::input::{
        InputData, InputManager, InputSource, InteractionSource, InteractionSourceRole,
        ManagedSourceRole, SourceCapabilityContext, SourceRoleBinding,
    };
    use notify::{Event, EventKind};

    struct BlockingCapabilitySource {
        armed: Arc<AtomicBool>,
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        updates: Arc<AtomicUsize>,
    }

    impl InputSource for BlockingCapabilitySource {
        fn name(&self) -> &'static str {
            "blocking_capability"
        }

        fn start(&mut self) -> anyhow::Result<()> {
            Ok(())
        }

        fn stop(&mut self) {}

        fn sample(&mut self) -> anyhow::Result<InputData> {
            Ok(InputData::None)
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    impl SourceRoleBinding for BlockingCapabilitySource {
        type Role = InteractionSourceRole;
    }

    impl InteractionSource for BlockingCapabilitySource {
        fn set_capability_context(
            &mut self,
            _context: &SourceCapabilityContext,
        ) -> anyhow::Result<()> {
            self.updates.fetch_add(1, Ordering::AcqRel);
            if self.armed.load(Ordering::Acquire) {
                self.entered
                    .send(())
                    .expect("capability observer should remain connected");
                self.release
                    .recv()
                    .expect("capability release should remain connected");
            }
            Ok(())
        }
    }

    fn identity(path: &std::path::Path, pid: u32) -> MacosOwnerIdentity {
        MacosOwnerIdentity::new("00000001", path, "deadbeef", pid)
            .expect("fixture owner identity is valid")
    }

    #[test]
    fn refresh_coalesces_identical_snapshots_and_publishes_distinct_conflicts() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let active = identity(&directory.path().join("active-daemon"), 10);
        let record = store
            .publish_owner(MacosDaemonOwner::AppSidecar, active)
            .expect("fixture owner should publish");
        let (snapshot_tx, snapshot_rx) = tokio::sync::watch::channel(None);
        let startup_snapshot = record.snapshot();
        let mut fingerprint = Some(MacosOwnerIdentityFingerprint::from(&record));

        refresh_owner_snapshot(&store, &snapshot_tx, &mut fingerprint, startup_snapshot)
            .expect("identical snapshot should coalesce");
        assert!(snapshot_rx.borrow().is_none());

        store
            .record_conflict(
                MacosDaemonOwner::Homebrew,
                identity(&directory.path().join("contender-daemon"), 20),
                42,
            )
            .expect("distinct contender should publish");
        refresh_owner_snapshot(&store, &snapshot_tx, &mut fingerprint, startup_snapshot)
            .expect("distinct conflict should refresh");

        assert_eq!(
            snapshot_rx
                .borrow()
                .as_ref()
                .expect("updated snapshot should remain installed")
                .snapshot
                .conflict
                .expect("updated snapshot should include conflict")
                .observed_at_ms,
            42
        );
    }

    #[test]
    fn private_fingerprint_observes_identity_changes_hidden_from_public_status() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let first = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                identity(&directory.path().join("first-daemon"), 10),
            )
            .expect("fixture owner should publish");
        let mut second = first.clone();
        second.active_identity = identity(&directory.path().join("second-daemon"), 11);

        assert_eq!(first.snapshot(), second.snapshot());
        assert_ne!(
            MacosOwnerIdentityFingerprint::from(&first),
            MacosOwnerIdentityFingerprint::from(&second)
        );
        let public = serde_json::to_string(&second.snapshot()).expect("snapshot should encode");
        assert!(!public.contains("second-daemon"));
        assert!(!public.contains("executable"));
    }

    #[test]
    fn recovery_status_survives_same_epoch_refresh_and_clears_for_a_new_owner() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let record = store
            .publish_owner(
                MacosDaemonOwner::Homebrew,
                identity(&directory.path().join("unrelated-daemon"), 10),
            )
            .expect("fixture owner should publish");
        let recovery = MacosOwnerRecoveryRequired {
            requested_owner: MacosDaemonOwner::DirectLaunchd,
            prior_owner: MacosDaemonOwner::AppSidecar,
            phase: MacosHandoverPhase::Prepared,
        };
        let startup_snapshot = record.snapshot().with_recovery_required(Some(recovery));

        let refreshed = snapshot_with_startup_recovery(&record, startup_snapshot);
        assert_eq!(refreshed.recovery_required, Some(recovery));
        let encoded = serde_json::to_value(owner_event(refreshed))
            .expect("ownership recovery event should serialize");
        assert_eq!(encoded["data"]["recovery_required"]["phase"], "prepared");

        let next = store
            .publish_owner(
                MacosDaemonOwner::DirectLaunchd,
                identity(&directory.path().join("requested-daemon"), 20),
            )
            .expect("replacement owner should publish");
        assert_eq!(
            snapshot_with_startup_recovery(&next, startup_snapshot).recovery_required,
            None
        );
    }

    #[test]
    fn snapshot_store_precedes_event_publication() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let snapshot = MacosOwnerStore::new(directory.path())
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                identity(&directory.path().join("active-daemon"), 10),
            )
            .expect("fixture owner should publish")
            .snapshot();
        let snapshots = ArcSwapOption::empty();
        let mut input_manager = InputManager::new();
        let mut event_published = false;

        publish_owner_snapshot_with(
            &snapshots,
            &mut input_manager,
            MacosOwnerPublication {
                snapshot,
                designated_requirement_hash: Some(Arc::from("deadbeef")),
            },
            |published_snapshot| {
                assert_eq!(snapshots.load_full().as_deref(), Some(&published_snapshot));
                event_published = true;
            },
        )
        .expect("snapshot should publish");

        assert!(event_published);
    }

    #[test]
    fn reconcile_after_watch_registration_closes_the_pre_source_race() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let initial = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                identity(&directory.path().join("active-daemon"), 10),
            )
            .expect("fixture owner should publish")
            .snapshot();
        let snapshots = Arc::new(ArcSwapOption::from(Some(Arc::new(initial))));
        let event_bus = Arc::new(HypercolorBus::new());
        let mut pending = PendingMacosOwnerWatch::start(
            directory.path().to_path_buf(),
            snapshots,
            event_bus,
            initial,
        )
        .expect("owner watch should register");

        store
            .record_conflict(
                MacosDaemonOwner::Homebrew,
                identity(&directory.path().join("contender-daemon"), 20),
                42,
            )
            .expect("conflict should publish after watch registration");
        let reconciled = pending
            .reconcile_snapshot(initial)
            .expect("pre-source reconcile should read the latest record");

        assert_eq!(
            reconciled
                .snapshot
                .conflict
                .expect("reconciled snapshot should include the contender")
                .observed_at_ms,
            42
        );
        assert_eq!(
            reconciled.designated_requirement_hash.as_deref(),
            Some("deadbeef")
        );
        let record = store
            .load_owner_record()
            .expect("owner record should load")
            .expect("owner record should exist");
        assert_eq!(
            pending.reconciled_fingerprint,
            Some(MacosOwnerIdentityFingerprint::from(&record))
        );
    }

    #[test]
    fn watch_signals_are_exact_path_and_latest_value_bounded() {
        let owner_path = std::path::PathBuf::from("/tmp/macos-daemon-owner.json");
        let unrelated =
            Event::new(EventKind::Any).add_path(std::path::PathBuf::from("/tmp/profiles.json"));
        let owner = Event::new(EventKind::Any).add_path(owner_path.clone());
        assert!(!event_touches_owner_record(&unrelated, &owner_path));
        assert!(event_touches_owner_record(&owner, &owner_path));

        let (signal_tx, signal_rx) = mpsc::sync_channel(1);
        enqueue_change(&signal_tx);
        enqueue_change(&signal_tx);

        assert!(signal_rx.recv().is_ok());
        assert!(signal_rx.try_recv().is_err());
    }

    #[test]
    fn stopping_worker_does_not_drain_a_queued_refresh() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let startup_snapshot = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                identity(&directory.path().join("active-daemon"), 10),
            )
            .expect("fixture owner should publish")
            .snapshot();
        store
            .record_conflict(
                MacosDaemonOwner::Homebrew,
                identity(&directory.path().join("contender-daemon"), 20),
                42,
            )
            .expect("conflict should publish");
        let (snapshot_tx, snapshot_rx) = tokio::sync::watch::channel(None);
        let (signal_tx, signal_rx) = mpsc::sync_channel(1);
        enqueue_change(&signal_tx);

        watch_worker(
            signal_rx,
            &AtomicBool::new(true),
            &store,
            &snapshot_tx,
            startup_snapshot,
            None,
        );

        assert!(snapshot_rx.borrow().is_none());
    }

    #[test]
    fn reconciled_fingerprint_coalesces_a_queued_startup_notification() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let record = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                identity(&directory.path().join("active-daemon"), 10),
            )
            .expect("fixture owner should publish");
        let startup_snapshot = record.snapshot();
        let fingerprint = MacosOwnerIdentityFingerprint::from(&record);
        let (snapshot_tx, snapshot_rx) = tokio::sync::watch::channel(None);
        let (signal_tx, signal_rx) = mpsc::sync_channel(1);
        enqueue_change(&signal_tx);
        drop(signal_tx);

        watch_worker(
            signal_rx,
            &AtomicBool::new(false),
            &store,
            &snapshot_tx,
            startup_snapshot,
            Some(fingerprint),
        );

        assert!(snapshot_rx.borrow().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_watch_while_lifecycle_is_busy_prevents_late_owner_publication() {
        let directory = tempfile::tempdir().expect("temporary owner directory should build");
        let store = MacosOwnerStore::new(directory.path());
        let initial = store
            .publish_owner(
                MacosDaemonOwner::AppSidecar,
                identity(&directory.path().join("active-daemon"), 10),
            )
            .expect("fixture owner should publish")
            .snapshot();
        let snapshots = Arc::new(ArcSwapOption::from(Some(Arc::new(initial))));
        let event_bus = Arc::new(HypercolorBus::new());
        let pending = PendingMacosOwnerWatch::start(
            directory.path().to_path_buf(),
            Arc::clone(&snapshots),
            event_bus,
            initial,
        )
        .expect("owner watch should register");
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let armed = Arc::new(AtomicBool::new(false));
        let updates = Arc::new(AtomicUsize::new(0));
        let input_manager = InputManager::new();
        input_manager
            .add_source(ManagedSourceRole::interaction(Box::new(
                BlockingCapabilitySource {
                    armed: Arc::clone(&armed),
                    entered: entered_tx,
                    release: release_rx,
                    updates: Arc::clone(&updates),
                },
            )))
            .expect("blocking capability source should register");
        armed.store(true, Ordering::Release);
        let blocker = {
            let manager = input_manager.clone();
            std::thread::spawn(move || {
                manager.set_source_capability_identity("held-owner", None, None)
            })
        };
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("capability update should own lifecycle state");
        let watch = pending
            .attach(input_manager)
            .expect("owner watch should attach");
        store
            .record_conflict(
                MacosDaemonOwner::Homebrew,
                identity(&directory.path().join("contender-daemon"), 20),
                42,
            )
            .expect("conflict should publish");
        enqueue_change(&watch.signal_tx);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(updates.load(Ordering::Acquire), 2);
        assert_eq!(snapshots.load_full().as_deref(), Some(&initial));

        tokio::time::timeout(
            Duration::from_millis(500),
            tokio::task::spawn_blocking(move || drop(watch)),
        )
        .await
        .expect("watch shutdown must not wait for the input-manager lock")
        .expect("watch shutdown task should finish");
        release_tx
            .send(())
            .expect("blocking capability update should resume");
        blocker
            .join()
            .expect("capability thread should finish")
            .expect("blocking capability update should succeed");
        tokio::task::yield_now().await;

        assert_eq!(updates.load(Ordering::Acquire), 2);
        assert_eq!(snapshots.load_full().as_deref(), Some(&initial));
    }
}
