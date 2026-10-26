//! The reservation's promise, under injected write failures: no revision is
//! ever handed out unless the ceiling above it is already on disk, a
//! failing disk is refused loudly and retried at most once per backoff,
//! and the render path rejects rather than waits.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hypercolor_core::bus::HypercolorBus;
use hypercolor_core::scene::SceneManager;
use hypercolor_core::spatial::SpatialEngine;
use hypercolor_types::spatial::{EdgeBehavior, SamplingMode, SpatialLayout};

use super::*;
use crate::domain::DomainError;
use crate::domain::commit::SceneCommitSequencer;
use crate::domain::scene::SceneService;
use crate::domain::spatial::SpatialService;
use crate::scene_transactions::LayoutTransactionRejection;

const NOW_SECS: u64 = 1_000;

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

fn on_disk(dir: &Path) -> u64 {
    read_reservation(&dir.join(SCENE_REVISIONS_FILE))
        .ok()
        .flatten()
        .expect("the reservation is on disk")
}

fn service(run: SceneRun) -> SceneService {
    SceneService::in_memory_with_run(
        SceneManager::with_default(),
        Arc::new(HypercolorBus::new()),
        run,
    )
}

async fn commit(scenes: &SceneService) -> Result<u64, DomainError> {
    let mutation = scenes.begin_mutation().await;
    scenes
        .commit_mutation(mutation)
        .await
        .map(|commit| commit.revision())
}

#[test]
fn an_uncovered_revision_is_refused_rather_than_served() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = SceneRun::resume_with_block(dir.path(), now(), 8);
    let reservation = run.reservation_for_test().expect("a resumed run reserves");
    let ceiling = run.reserved_until().expect("ceiling");
    reservation.fail_writes_for_test(true);
    let (_, start, reservation) = run.into_parts();
    let sequencer = Arc::new(SceneCommitSequencer::resuming(start, reservation));
    let bus = Arc::new(HypercolorBus::new());

    let mut last = start;
    while let Ok(ticket) = sequencer.admit(Arc::clone(&bus)) {
        last = ticket.generation();
        ticket.release(Vec::new());
    }
    assert_eq!(
        last,
        ceiling - 1,
        "every revision below the durable ceiling is used"
    );
    assert_eq!(
        sequencer.revision(),
        ceiling - 1,
        "the refusal changed nothing"
    );
    assert!(!sequencer.covers_next());

    // A crash now, then a restart with the clock rolled back to the epoch,
    // still starts above everything the failing run handed out.
    let next = SceneRun::resume_with_block(dir.path(), UNIX_EPOCH, 8);
    assert!(next.start_revision() > last);
}

#[test]
fn a_run_whose_first_reservation_fails_refuses_every_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A directory where the file belongs: unreadable and unwritable.
    std::fs::create_dir_all(dir.path().join(SCENE_REVISIONS_FILE)).expect("blocker");
    let run = SceneRun::resume_with_block(dir.path(), now(), 8);
    assert_eq!(run.start_revision(), NOW_SECS * 1_000_000);
    assert_eq!(run.reserved_until(), Some(0), "nothing is durable");

    let (_, start, reservation) = run.into_parts();
    let sequencer = Arc::new(SceneCommitSequencer::resuming(start, reservation));
    assert!(sequencer.admit(Arc::new(HypercolorBus::new())).is_err());
    assert_eq!(sequencer.revision(), start);
}

/// Each committed revision is below the ceiling on disk once its commit
/// returns; that it was below at hand-out is admission's own refusal,
/// which the failure tests above pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commits_wait_for_their_reservation_and_stay_below_the_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scenes = service(SceneRun::resume_with_block(dir.path(), now(), 8));
    for _ in 0..40 {
        let revision = commit(&scenes).await.expect("a healthy disk never refuses");
        assert!(
            on_disk(dir.path()) > revision,
            "revision {revision} was committed at or above the ceiling on disk"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_failing_disk_refuses_commits_until_it_recovers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = SceneRun::resume_with_block(dir.path(), now(), 8);
    let reservation = run.reservation_for_test().expect("a resumed run reserves");
    let ceiling = run.reserved_until().expect("ceiling");
    reservation.fail_writes_for_test(true);
    let scenes = service(run);

    let mut last = scenes.revision();
    let refusal = loop {
        match commit(&scenes).await {
            Ok(revision) => last = revision,
            Err(error) => break error,
        }
    };
    assert!(
        matches!(refusal, DomainError::ServiceUnavailable { .. }),
        "{refusal:?}"
    );
    assert_eq!(last, ceiling - 1);
    assert_eq!(
        scenes.revision(),
        last,
        "a refused commit leaves the scene alone"
    );
    let attempts = reservation.write_attempts_for_test();
    assert!(commit(&scenes).await.is_err());
    assert_eq!(
        reservation.write_attempts_for_test(),
        attempts,
        "inside the backoff a commit is refused without touching the disk"
    );

    reservation.fail_writes_for_test(false);
    tokio::time::advance(RESERVATION_RETRY_BACKOFF + Duration::from_millis(1)).await;
    let resumed = commit(&scenes)
        .await
        .expect("a recovered disk admits again");
    assert_eq!(resumed, last + 1, "no revision was skipped or repeated");
    assert!(on_disk(dir.path()) > resumed);
}

fn layout(id: &str) -> SpatialLayout {
    SpatialLayout {
        id: id.to_owned(),
        name: id.to_owned(),
        description: None,
        canvas_width: 320,
        canvas_height: 200,
        zones: Vec::new(),
        default_sampling_mode: SamplingMode::Bilinear,
        default_edge_behavior: EdgeBehavior::Clamp,
        version: 1,
    }
}

#[tokio::test]
async fn the_render_path_rejects_an_uncovered_publication_without_publishing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let initial = layout("initial");
    let run = SceneRun::resume_with_block(dir.path(), now(), 8);
    let reservation = run.reservation_for_test().expect("a resumed run reserves");
    reservation.fail_writes_for_test(true);
    let scenes = SceneService::in_memory_with_run(
        SceneManager::with_default_layout(initial.clone()),
        Arc::new(HypercolorBus::new()),
        run,
    );
    while commit(&scenes).await.is_ok() {}
    let before = scenes.revision();
    let spatial = SpatialService::new(SpatialEngine::try_new(initial.clone()).expect("layout"));
    let manager = scenes.snapshot().await;

    let mut renderer_published = false;
    let result = scenes
        .publish_layout_activation(
            &spatial,
            SpatialEngine::try_new(layout("candidate")).expect("candidate"),
            &initial,
            manager.active_scene_id().copied(),
            manager.resolved_zones_revision(),
            |_| {
                renderer_published = true;
                Ok(())
            },
        )
        .await;

    assert_eq!(
        result,
        Err(LayoutTransactionRejection::RevisionsUnavailable)
    );
    assert!(!renderer_published, "nothing reached the renderer");
    assert_eq!(spatial.layout().id, initial.id);
    assert_eq!(scenes.revision(), before);
}

#[test]
fn the_store_never_lowers_the_ceiling_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ReservationStore::new(dir.path().join(SCENE_REVISIONS_FILE), 0);
    store.persist_at_least(10).expect("write");
    store
        .persist_at_least(5)
        .expect("a lower ceiling is a no-op");
    assert_eq!(on_disk(dir.path()), 10);
    assert_eq!(store.persisted(), 10);
}
