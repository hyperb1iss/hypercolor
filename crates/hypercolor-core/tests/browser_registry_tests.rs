use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use hypercolor_core::input::{
    BrowserConnectionIncarnation, BrowserInputChildKey, BrowserInputChildSlot, BrowserInputEdge,
    BrowserInputHandle, BrowserInputRegistryError, BrowserPreviewId, INPUT_EVENT_RING_CAPACITY,
    InputData,
};
use hypercolor_types::event::{InputButtonState, InputEvent};
use hypercolor_types::event::{PointerScrollPhase, PointerScrollUnit};

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

fn line_scroll(delta_line120: i64) -> BrowserInputEdge {
    BrowserInputEdge::Scroll {
        delta_x_q16_16: 0,
        delta_y_q16_16: delta_line120 << 16,
        unit: PointerScrollUnit::Line120,
        phase: PointerScrollPhase::None,
        momentum_phase: PointerScrollPhase::None,
    }
}

#[test]
fn connection_wide_compatibility_lane_stays_deleted() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/input/browser.rs"),
    )
    .expect("browser input module should read");
    let retired_symbols = [
        "attach_legacy",
        "release_legacy",
        "pub fn inject(&self, source_id",
        "release_source",
        "retired_legacy",
        "BROWSER_RETIRED_LEGACY_CAPACITY",
        "BrowserConnectionNamespace",
    ];
    let offenders = retired_symbols
        .into_iter()
        .filter(|symbol| source.contains(symbol))
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "browser input must use addressed preview attachments: {offenders:#?}"
    );
}

#[test]
fn connections_and_previews_publish_independent_children() {
    let handle = BrowserInputHandle::new();
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
    let handle = BrowserInputHandle::new();
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
    let handle = BrowserInputHandle::new();
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
    let handle = BrowserInputHandle::new();
    let attachment = handle.attach(child_key(4, "events")).expect("attach");
    let slot = attachment.slot();
    attachment
        .inject((0..3).map(line_scroll))
        .expect("initial inject");

    let mut fast_events = Vec::new();
    let fast_cursor = slot.read_events_since(0, &mut fast_events).next_cursor;
    assert_eq!(fast_cursor, 3);
    attachment
        .inject(
            (0..INPUT_EVENT_RING_CAPACITY + 5)
                .map(|index| line_scroll(i64::try_from(index).expect("test index fits i64"))),
        )
        .expect("overflow inject");

    let mut slow_events = Vec::new();
    let slow = slot.read_events_since(0, &mut slow_events);
    assert_eq!(slow_events.len(), INPUT_EVENT_RING_CAPACITY);
    assert_eq!(slow.dropped, 8);

    fast_events.clear();
    let fast = slot.read_events_since(fast_cursor, &mut fast_events);
    assert_eq!(fast_events.len(), INPUT_EVENT_RING_CAPACITY);
    assert_eq!(fast.dropped, 5);

    let mut replay = Vec::new();
    let replay_read = slot.read_events_since(0, &mut replay);
    assert_eq!(replay, slow_events);
    assert_eq!(replay_read, slow);
}

#[test]
fn child_publication_accumulates_motion_without_a_sampled_owner() {
    let handle = BrowserInputHandle::new();
    let attachment = handle.attach(child_key(13, "motion")).expect("attach");
    let slot = attachment.slot();
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

    let mut events = Vec::new();
    let publication = slot.read_publication_since(0, &mut events);
    let InputData::Interaction(_sample) = publication
        .sample
        .expect("child publication")
        .as_ref()
        .clone()
    else {
        panic!("expected interaction sample");
    };
    assert!(events.is_empty());
    assert!((publication.interaction_transients.dx - 0.3).abs() < 1e-6);
    assert!((publication.interaction_transients.dy - 0.4).abs() < 1e-6);
    assert!((publication.interaction_transients.distance - 0.7).abs() < 1e-6);
}

#[test]
fn final_attachment_owner_retires_the_child() {
    let handle = BrowserInputHandle::new();
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
fn child_publication_reads_held_state_and_edges_from_one_revision() {
    let handle = BrowserInputHandle::new();
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
    let handle = BrowserInputHandle::new();
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
