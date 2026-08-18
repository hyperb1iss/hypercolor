use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_core::input::{
    BROWSER_RETIRED_LEGACY_CAPACITY, BrowserConnectionIncarnation, BrowserInputChildKey,
    BrowserInputChildSlot, BrowserInputEdge, BrowserInputHandle, BrowserInputRegistryError,
    BrowserInputSource, BrowserPreviewId, INPUT_EVENT_RING_CAPACITY, InputData, InputSource,
    MotionAggregate,
};
use hypercolor_core::types::event::{InputButtonState, InputEvent};

fn started_source() -> (BrowserInputSource, BrowserInputHandle) {
    let mut source = BrowserInputSource::new();
    source.start().expect("browser source should start");
    let handle = source.handle();
    (source, handle)
}

fn child_key(connection: u64, preview: &str) -> BrowserInputChildKey {
    BrowserInputChildKey::new(
        BrowserConnectionIncarnation::new(connection),
        BrowserPreviewId::new(preview),
    )
}

fn pressed_keys(slot: &BrowserInputChildSlot) -> Vec<String> {
    let sample = slot.latest().expect("child should publish held state");
    let InputData::Interaction(interaction) = sample.as_ref() else {
        panic!("browser child must publish interaction data");
    };
    interaction.keyboard.pressed_keys.clone()
}

fn press(key: &str) -> BrowserInputEdge {
    BrowserInputEdge::Key {
        key: key.to_owned(),
        state: InputButtonState::Pressed,
    }
}

#[test]
fn shared_sampling_reuses_browser_snapshot_pool_and_drains_directly() {
    let (mut source, handle) = started_source();
    let attachment = handle
        .attach(child_key(1, "shared-sample"))
        .expect("preview should attach");
    attachment
        .inject([BrowserInputEdge::Wheel { delta_hi_res: 120 }])
        .expect("wheel should inject");
    let mut events = Vec::with_capacity(4);

    let first = source
        .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
        .expect("shared sample should succeed")
        .expect("running browser source should publish");
    let first_ptr = Arc::as_ptr(&first);
    assert!(matches!(
        events.as_slice(),
        [exact, shadow]
            if matches!(
                exact.event,
                InputEvent::PointerScroll { delta_y_q16_16, .. }
                    if delta_y_q16_16 == 120 * hypercolor_core::input::Q16_16_SCALE
            ) && matches!(
                shadow.event,
                InputEvent::MouseWheel { delta_hi_res: 120, .. }
            )
    ));
    drop(first);

    events.clear();
    let second = source
        .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
        .expect("shared sample should succeed")
        .expect("running browser source should publish");
    assert!(events.is_empty());
    drop(second);

    attachment
        .inject([BrowserInputEdge::Wheel { delta_hi_res: -30 }])
        .expect("second wheel should inject");
    let third = source
        .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
        .expect("shared sample should succeed")
        .expect("running browser source should publish");

    assert_eq!(Arc::as_ptr(&third), first_ptr);
    assert!(matches!(
        events.as_slice(),
        [exact, shadow]
            if matches!(
                exact.event,
                InputEvent::PointerScroll { delta_y_q16_16, .. }
                    if delta_y_q16_16 == -30 * hypercolor_core::input::Q16_16_SCALE
            ) && matches!(
                shadow.event,
                InputEvent::MouseWheel { delta_hi_res: -30, .. }
            )
    ));
}

#[test]
fn shared_snapshot_pool_stays_bounded_under_retained_consumer_pressure() {
    let (mut source, _handle) = started_source();
    let mut events = Vec::new();
    let first = source
        .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
        .expect("first shared sample should succeed")
        .expect("running browser source should publish");
    let second = source
        .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
        .expect("second shared sample should succeed")
        .expect("running browser source should publish");
    let pooled = [Arc::as_ptr(&first), Arc::as_ptr(&second)];
    let pressured = (0..8)
        .map(|_| {
            source
                .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
                .expect("pressure sample should succeed")
                .expect("running browser source should publish")
        })
        .collect::<Vec<_>>();
    let pressure_pointers = pressured.iter().map(Arc::as_ptr).collect::<Vec<_>>();
    assert!(
        pressure_pointers
            .iter()
            .all(|pointer| !pooled.contains(pointer))
    );

    drop(first);
    drop(second);
    drop(pressured);
    for _ in 0..8 {
        let sample = source
            .sample_shared_and_drain_into(1.0 / 60.0, &mut events)
            .expect("recovery sample should succeed")
            .expect("running browser source should publish");
        assert!(pooled.contains(&Arc::as_ptr(&sample)));
        assert!(!pressure_pointers.contains(&Arc::as_ptr(&sample)));
        drop(sample);
    }
}

#[test]
fn connections_and_previews_publish_independent_children() {
    let (_source, handle) = started_source();
    let first = handle
        .attach(child_key(1, "shared-name"))
        .expect("first preview should attach");
    let second = handle
        .attach(child_key(2, "shared-name"))
        .expect("second connection should attach");
    let sibling = handle
        .attach(child_key(1, "sibling"))
        .expect("second preview should attach on one connection");

    first.inject([press("KeyA")]).expect("first inject");
    second.inject([press("KeyB")]).expect("second inject");
    sibling.inject([press("KeyC")]).expect("sibling inject");

    let registry = handle.registry().snapshot();
    assert_eq!(registry.children().len(), 3);
    assert_eq!(pressed_keys(&first.slot()), ["KeyA"]);
    assert_eq!(pressed_keys(&second.slot()), ["KeyB"]);
    assert_eq!(pressed_keys(&sibling.slot()), ["KeyC"]);

    for (attachment, expected) in [(&first, "KeyA"), (&second, "KeyB"), (&sibling, "KeyC")] {
        let mut events = Vec::new();
        let read = attachment.slot().read_events_since(0, &mut events);
        assert_eq!(read.next_cursor, 1);
        assert!(matches!(
            &events[..],
            [event]
                if matches!(
                    &event.event,
                    InputEvent::Key { key, .. } if key == expected
                )
        ));
    }
}

#[test]
fn attach_is_idempotent_and_reconnect_gets_a_fresh_incarnation() {
    let (_source, handle) = started_source();
    let key = child_key(7, "preview");
    let first = handle.attach(key.clone()).expect("first attach");
    let duplicate = handle.attach(key.clone()).expect("idempotent attach");
    assert_eq!(first.publication_id(), duplicate.publication_id());

    assert!(first.close());
    assert!(!duplicate.close());
    assert_eq!(
        duplicate.inject([press("stale")]),
        Err(BrowserInputRegistryError::ChildClosed)
    );

    let replacement = handle.attach(key.clone()).expect("reattach");
    assert_ne!(first.publication_id(), replacement.publication_id());
    assert!(!first.close(), "stale close must not remove replacement");
    replacement.inject([press("fresh")]).expect("fresh inject");
    assert_eq!(pressed_keys(&replacement.slot()), ["fresh"]);
    assert_eq!(
        handle
            .registry()
            .snapshot()
            .child(&key)
            .expect("replacement remains active")
            .publication_id(),
        replacement.publication_id()
    );
}

#[test]
fn close_removes_held_child_without_requiring_a_release_drain() {
    let (_source, handle) = started_source();
    let key = child_key(3, "held");
    let attachment = handle.attach(key.clone()).expect("attach");
    let slot = attachment.slot();
    attachment.inject([press("KeyH")]).expect("inject");
    assert_eq!(pressed_keys(&slot), ["KeyH"]);

    assert!(attachment.close());
    assert!(handle.registry().snapshot().child(&key).is_none());
    assert!(pressed_keys(&slot).is_empty());
    let mut events = Vec::new();
    slot.read_events_since(0, &mut events);
    assert_eq!(
        events.len(),
        1,
        "strict close does not publish a final edge"
    );
    assert!(matches!(
        events[0].event,
        InputEvent::Key {
            state: InputButtonState::Pressed,
            ..
        }
    ));
}

#[test]
fn bounded_child_history_is_non_destructive_for_independent_consumers() {
    let (_source, handle) = started_source();
    let attachment = handle.attach(child_key(4, "events")).expect("attach");
    let slot = attachment.slot();
    attachment
        .inject((0..3).map(|delta_hi_res| BrowserInputEdge::Wheel { delta_hi_res }))
        .expect("initial inject");

    let mut fast_events = Vec::new();
    let fast_cursor = slot.read_events_since(0, &mut fast_events).next_cursor;
    assert_eq!(fast_cursor, 5);
    attachment
        .inject(
            (0..INPUT_EVENT_RING_CAPACITY + 5).map(|index| BrowserInputEdge::Wheel {
                delta_hi_res: i32::try_from(index).expect("test index fits i32"),
            }),
        )
        .expect("overflow inject");

    let mut slow_events = Vec::new();
    let slow = slot.read_events_since(0, &mut slow_events);
    assert_eq!(slow_events.len(), INPUT_EVENT_RING_CAPACITY);
    assert_eq!(slow.dropped, 270);

    fast_events.clear();
    let fast = slot.read_events_since(fast_cursor, &mut fast_events);
    assert_eq!(fast_events.len(), INPUT_EVENT_RING_CAPACITY);
    assert_eq!(fast.dropped, 265);

    let mut replay = Vec::new();
    let replay_read = slot.read_events_since(0, &mut replay);
    assert_eq!(replay, slow_events);
    assert_eq!(replay_read, slow);
}

#[test]
fn compatibility_aggregate_has_its_own_event_cursor() {
    let (mut source, handle) = started_source();
    let attachment = handle.attach(child_key(5, "aggregate")).expect("attach");
    let slot = attachment.slot();
    attachment.inject([press("KeyA")]).expect("inject");

    let (sample, aggregate_events) = source.sample_and_drain_with_delta_secs(0.0);
    let InputData::Interaction(sample) = sample.expect("aggregate sample") else {
        panic!("expected aggregate interaction data");
    };
    assert_eq!(sample.keyboard.pressed_keys, ["KeyA"]);
    assert_eq!(aggregate_events.len(), 1);

    let mut direct_events = Vec::new();
    slot.read_events_since(0, &mut direct_events);
    assert_eq!(direct_events, aggregate_events);
    direct_events.clear();
    slot.read_events_since(0, &mut direct_events);
    assert_eq!(direct_events, aggregate_events);
    assert!(source.drain_events().is_empty());
}

#[test]
fn compatibility_aggregate_accumulates_superseded_motion_publications() {
    let (mut source, handle) = started_source();
    let attachment = handle.attach(child_key(13, "motion")).expect("attach");
    attachment
        .inject([BrowserInputEdge::Move {
            norm_x: 0.1,
            norm_y: 0.1,
        }])
        .expect("initial position");
    attachment
        .inject([BrowserInputEdge::Move {
            norm_x: 0.4,
            norm_y: 0.1,
        }])
        .expect("horizontal motion");
    attachment
        .inject([BrowserInputEdge::Move {
            norm_x: 0.4,
            norm_y: 0.5,
        }])
        .expect("vertical motion");

    let (sample, events) = source.sample_and_drain_with_delta_secs(0.0);
    let InputData::Interaction(sample) = sample.expect("aggregate sample") else {
        panic!("expected interaction sample");
    };
    assert!(events.is_empty());
    assert!((sample.batch.motion.dx - 0.3).abs() < 1e-6);
    assert!((sample.batch.motion.dy - 0.4).abs() < 1e-6);
    assert!((sample.batch.motion.distance - 0.7).abs() < 1e-6);

    let (sample, events) = source.sample_and_drain_with_delta_secs(0.0);
    let InputData::Interaction(sample) = sample.expect("second aggregate sample") else {
        panic!("expected interaction sample");
    };
    assert!(events.is_empty());
    assert_eq!(sample.batch.motion, MotionAggregate::default());
}

#[test]
fn split_legacy_drain_preserves_retired_motion_for_the_next_sample() {
    let (mut source, handle) = started_source();
    handle.inject(
        "legacy-motion",
        [BrowserInputEdge::Move {
            norm_x: 0.2,
            norm_y: 0.2,
        }],
    );
    handle.inject(
        "legacy-motion",
        [BrowserInputEdge::Move {
            norm_x: 0.7,
            norm_y: 0.6,
        }],
    );
    handle.release_source("legacy-motion");

    assert!(source.drain_events().is_empty());
    let InputData::Interaction(sample) = source.sample().expect("aggregate sample") else {
        panic!("expected interaction sample");
    };
    assert!((sample.batch.motion.dx - 0.5).abs() < 1e-6);
    assert!((sample.batch.motion.dy - 0.4).abs() < 1e-6);
    assert!((sample.batch.motion.distance - 0.5_f32.hypot(0.4)).abs() < 1e-6);
}

#[test]
fn fast_aggregate_consumer_is_not_charged_for_replaced_history() {
    let (mut source, handle) = started_source();
    let attachment = handle.attach(child_key(14, "fast")).expect("attach");
    for delta_hi_res in 0..INPUT_EVENT_RING_CAPACITY + 5 {
        attachment
            .inject([BrowserInputEdge::Wheel {
                delta_hi_res: i32::try_from(delta_hi_res).expect("test delta fits i32"),
            }])
            .expect("wheel inject");
        let (sample, events) = source.sample_and_drain_with_delta_secs(0.0);
        let InputData::Interaction(sample) = sample.expect("aggregate sample") else {
            panic!("expected interaction sample");
        };
        assert_eq!(events.len(), if delta_hi_res == 0 { 1 } else { 2 });
        assert_eq!(
            sample.batch.wheel_hi_res,
            i32::try_from(delta_hi_res).expect("test delta fits i32")
        );
        assert_eq!(sample.batch.dropped_events, 0);
    }
}

#[test]
fn source_stop_retires_every_child_and_restart_does_not_resurrect_them() {
    let (mut source, handle) = started_source();
    let key = child_key(6, "restart");
    let old = handle.attach(key.clone()).expect("attach");
    old.inject([press("KeyS")]).expect("inject");

    source.stop();
    assert!(handle.registry().snapshot().children().is_empty());
    assert_eq!(
        old.inject([press("stale")]),
        Err(BrowserInputRegistryError::ChildClosed)
    );
    assert!(matches!(
        handle.attach(key.clone()),
        Err(BrowserInputRegistryError::SourceInactive)
    ));

    source.start().expect("restart");
    let replacement = handle.attach(key).expect("reattach after restart");
    assert_ne!(old.publication_id(), replacement.publication_id());
    assert!(pressed_keys(&replacement.slot()).is_empty());
}

#[test]
fn final_attachment_owner_retires_the_child() {
    let (_source, handle) = started_source();
    let key = child_key(11, "drop-owned");
    let attachment = handle.attach(key.clone()).expect("attach");
    let sibling_owner = attachment.clone();
    let slot = attachment.slot();
    attachment.inject([press("KeyD")]).expect("inject");

    drop(attachment);
    assert!(handle.registry().snapshot().child(&key).is_some());
    sibling_owner
        .inject([press("KeyE")])
        .expect("remaining owner should stay active");

    drop(sibling_owner);
    assert!(handle.registry().snapshot().child(&key).is_none());
    assert!(!slot.is_active());
    assert!(pressed_keys(&slot).is_empty());
}

#[test]
fn legacy_retirement_backlog_is_bounded_without_manager_sampling() {
    let (mut source, handle) = started_source();
    let churn = BROWSER_RETIRED_LEGACY_CAPACITY + 32;
    for index in 0..churn {
        let source_id = format!("legacy-{index}");
        handle.inject(&source_id, [press("KeyL")]);
        handle.release_source(&source_id);
    }

    assert!(handle.registry().snapshot().children().is_empty());
    let events = source.drain_events();
    assert_eq!(events.len(), BROWSER_RETIRED_LEGACY_CAPACITY * 2);
    assert!(source.drain_events().is_empty());
}

#[test]
fn child_publication_reads_held_state_and_edges_from_one_revision() {
    let (_source, handle) = started_source();
    let attachment = handle.attach(child_key(8, "coherent")).expect("attach");
    let slot = attachment.slot();
    attachment.inject([press("KeyA")]).expect("press");
    let mut replay = Vec::new();
    let pressed = slot.read_publication_since(u64::MAX, &mut replay);
    let InputData::Interaction(pressed_sample) =
        pressed.sample.expect("pressed sample").as_ref().clone()
    else {
        panic!("expected interaction sample");
    };
    assert_eq!(pressed_sample.keyboard.pressed_keys, ["KeyA"]);
    assert!(replay.is_empty());
    assert_eq!(pressed.events.next_cursor, 1);

    attachment
        .inject([BrowserInputEdge::Key {
            key: "KeyA".to_owned(),
            state: InputButtonState::Released,
        }])
        .expect("release");

    let mut released_events = Vec::new();
    let released = slot.read_publication_since(pressed.events.next_cursor, &mut released_events);
    let InputData::Interaction(released_sample) =
        released.sample.expect("released sample").as_ref().clone()
    else {
        panic!("expected interaction sample");
    };
    assert!(released_sample.keyboard.pressed_keys.is_empty());
    assert!(matches!(
        released_events.as_slice(),
        [event]
            if matches!(
                event.event,
                InputEvent::Key {
                    state: InputButtonState::Released,
                    ..
                }
            )
    ));
    assert_eq!(released.events.next_cursor, 2);
}

#[test]
fn child_retirement_does_not_hold_the_registry_writer() {
    let (_source, handle) = started_source();
    let blocked = handle.attach(child_key(9, "blocked")).expect("attach");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let injector = {
        let blocked = blocked.clone();
        thread::spawn(move || {
            blocked
                .inject(BlockingEdges::new(entered_tx, release_rx))
                .expect("blocked injection should settle");
        })
    };
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("injector should hold child state");

    let closer = {
        let blocked = blocked.clone();
        thread::spawn(move || blocked.close())
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    while handle.registry().snapshot().child(blocked.key()).is_some() {
        assert!(Instant::now() < deadline, "close should publish removal");
        thread::yield_now();
    }

    let (attached_tx, attached_rx) = mpsc::sync_channel(1);
    let attach_handle = handle.clone();
    let attacher = thread::spawn(move || {
        let result = attach_handle.attach(child_key(10, "unrelated"));
        let _ = attached_tx.send(result);
    });
    let unrelated = attached_rx.recv_timeout(Duration::from_millis(500));
    release_tx.send(()).expect("release injector");
    injector.join().expect("injector thread");
    assert!(closer.join().expect("closer thread"));
    attacher.join().expect("attacher thread");
    assert!(
        unrelated
            .expect("unrelated attach must not wait for retirement")
            .is_ok(),
        "unrelated attach should succeed"
    );
}

#[test]
fn compatibility_aggregate_never_tears_edges_from_held_state() {
    const PUBLICATIONS: usize = INPUT_EVENT_RING_CAPACITY * 16;
    let (mut source, handle) = started_source();
    let attachment = handle
        .attach(child_key(12, "aggregate-race"))
        .expect("attach");
    let start = Arc::new(Barrier::new(2));
    let complete = Arc::new(AtomicBool::new(false));
    let producer = {
        let attachment = attachment.clone();
        let start = Arc::clone(&start);
        let complete = Arc::clone(&complete);
        thread::spawn(move || {
            start.wait();
            for index in 0..PUBLICATIONS {
                let state = if index % 2 == 0 {
                    InputButtonState::Pressed
                } else {
                    InputButtonState::Released
                };
                attachment
                    .inject([BrowserInputEdge::Key {
                        key: "KeyR".to_owned(),
                        state,
                    }])
                    .expect("concurrent injection");
                thread::yield_now();
            }
            complete.store(true, Ordering::Release);
        })
    };

    start.wait();
    let mut held = false;
    let mut observed_completion = false;
    loop {
        let (sample, events) = source.sample_and_drain_with_delta_secs(0.0);
        let InputData::Interaction(sample) = sample.expect("aggregate sample") else {
            panic!("expected interaction sample");
        };
        if sample.batch.dropped_events > 0 {
            held = sample.keyboard.pressed_keys.iter().any(|key| key == "KeyR");
        } else {
            for event in &events {
                let InputEvent::Key { key, state, .. } = &event.event else {
                    continue;
                };
                if key == "KeyR" {
                    held = *state != InputButtonState::Released;
                }
            }
            assert_eq!(
                sample.keyboard.pressed_keys.iter().any(|key| key == "KeyR"),
                held,
                "one aggregate publication must pair edges with the same held revision"
            );
        }

        if complete.load(Ordering::Acquire) {
            if observed_completion && events.is_empty() {
                break;
            }
            observed_completion = true;
        }
    }

    producer.join().expect("aggregate producer");
    assert!(!held, "the even publication count ends released");
}

struct BlockingEdges {
    entered: Option<mpsc::SyncSender<()>>,
    release: mpsc::Receiver<()>,
}

impl BlockingEdges {
    fn new(entered: mpsc::SyncSender<()>, release: mpsc::Receiver<()>) -> Self {
        Self {
            entered: Some(entered),
            release,
        }
    }
}

impl Iterator for BlockingEdges {
    type Item = BrowserInputEdge;

    fn next(&mut self) -> Option<Self::Item> {
        let entered = self.entered.take()?;
        entered.send(()).expect("signal child-state lock");
        self.release.recv().expect("release child-state lock");
        None
    }
}
