#![cfg(feature = "ws-core")]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::Channel;
use hypercolor_leptos_ext::ws::transport::{CinderTransport, InMemoryTransport};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn in_memory_transport_roundtrips_bytes() {
    let (mut left, mut right) = InMemoryTransport::pair();

    left.send(Bytes::from_static(b"canvas"))
        .await
        .expect("send succeeds");

    let received = right.recv().await.expect("recv succeeds");

    assert_eq!(received, Some(Bytes::from_static(b"canvas")));
}

#[tokio::test]
async fn in_memory_transport_close_ends_peer_stream() {
    let (mut left, mut right) = InMemoryTransport::pair();

    left.close().await.expect("close succeeds");

    let received = right.recv().await.expect("recv succeeds");

    assert_eq!(received, None);
}

#[tokio::test]
async fn in_memory_transport_poll_ready_blocks_when_capacity_is_full() {
    let (mut left, mut right) = InMemoryTransport::pair_with_capacity(1);

    left.send(Bytes::from_static(b"first"))
        .await
        .expect("first send succeeds");
    left.send(Bytes::from_static(b"second"))
        .await
        .expect("second send succeeds");

    let blocked = timeout(
        Duration::from_millis(25),
        left.send(Bytes::from_static(b"third")),
    )
    .await;
    assert!(blocked.is_err());

    let drained = right.recv().await.expect("recv succeeds");
    assert_eq!(drained, Some(Bytes::from_static(b"first")));
    let drained = right.recv().await.expect("recv succeeds");
    assert_eq!(drained, Some(Bytes::from_static(b"second")));

    left.send(Bytes::from_static(b"third"))
        .await
        .expect("third send succeeds after drain");

    let drained = right.recv().await.expect("recv succeeds");
    assert_eq!(drained, Some(Bytes::from_static(b"third")));
}

#[tokio::test]
async fn channel_wraps_transport_without_changing_bytes() {
    let (left, right) = InMemoryTransport::pair();
    let mut channel = Channel::new(left);
    let mut receiver = Channel::new(right);

    channel
        .send_bytes(Bytes::from_static(b"preview"))
        .await
        .expect("send succeeds");

    let received = receiver.recv_bytes().await.expect("recv succeeds");

    assert_eq!(received, Some(Bytes::from_static(b"preview")));
}
