#![cfg(feature = "ws-core")]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::{
    BackpressureQueue, BlockOnFull, DropNewest, DropOldest, Latest, OverflowAction, Queue,
};

#[test]
fn drop_oldest_replaces_front_frame_when_full() {
    let mut queue = BackpressureQueue::<DropOldest<2>>::new();

    assert_eq!(
        queue.push(Bytes::from_static(b"one")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"two")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"three")),
        OverflowAction::Dropped { dropped_frames: 1 }
    );

    assert_eq!(queue.dropped_frames(), 1);
    assert_eq!(queue.pop_front(), Some(Bytes::from_static(b"two")));
    assert_eq!(queue.pop_front(), Some(Bytes::from_static(b"three")));
}

#[test]
fn drop_newest_keeps_existing_queue_when_full() {
    let mut queue = BackpressureQueue::<DropNewest<2>>::new();

    assert_eq!(
        queue.push(Bytes::from_static(b"one")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"two")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"three")),
        OverflowAction::Dropped { dropped_frames: 1 }
    );

    assert_eq!(queue.dropped_frames(), 1);
    assert_eq!(queue.pop_front(), Some(Bytes::from_static(b"one")));
    assert_eq!(queue.pop_front(), Some(Bytes::from_static(b"two")));
    assert_eq!(queue.pop_front(), None);
}

#[test]
fn latest_keeps_only_the_newest_frame() {
    let mut queue = BackpressureQueue::<Latest>::new();

    assert_eq!(
        queue.push(Bytes::from_static(b"one")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"two")),
        OverflowAction::Dropped { dropped_frames: 1 }
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"three")),
        OverflowAction::Dropped { dropped_frames: 1 }
    );

    assert_eq!(queue.dropped_frames(), 2);
    assert_eq!(queue.pop_front(), Some(Bytes::from_static(b"three")));
    assert_eq!(queue.pop_front(), None);
}

#[test]
fn queue_blocks_when_full() {
    let mut queue = BackpressureQueue::<Queue<2>>::new();

    assert_eq!(
        queue.push(Bytes::from_static(b"one")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"two")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"three")),
        OverflowAction::Block
    );

    assert_eq!(queue.dropped_frames(), 0);
    assert_eq!(queue.pending_len(), 2);
}

#[test]
fn block_on_full_never_drops_frames() {
    let mut queue = BackpressureQueue::<BlockOnFull<1>>::new();

    assert_eq!(
        queue.push(Bytes::from_static(b"one")),
        OverflowAction::Accepted
    );
    assert_eq!(
        queue.push(Bytes::from_static(b"two")),
        OverflowAction::Block
    );

    assert_eq!(queue.dropped_frames(), 0);
    assert_eq!(queue.pop_front(), Some(Bytes::from_static(b"one")));
}
