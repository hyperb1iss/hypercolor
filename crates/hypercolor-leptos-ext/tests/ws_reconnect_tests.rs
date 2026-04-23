#![cfg(feature = "ws-core")]

use bytes::Bytes;
use hypercolor_leptos_ext::ws::transport::{CinderTransport, InMemoryTransport};
use hypercolor_leptos_ext::ws::{
    Connector, ExponentialBackoff, Jitter, ReconnectError, ReconnectOutcome, ReconnectPolicy,
    Reconnecting,
};
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn default_backoff_matches_hypercolor_schedule_midpoint() {
    let policy = ExponentialBackoff::HYPERCOLOR_DEFAULT;

    assert_eq!(
        policy.delay_for_attempt(0),
        Some(Duration::from_millis(500))
    );
    assert_eq!(policy.delay_for_attempt(1), Some(Duration::from_secs(1)));
    assert_eq!(policy.delay_for_attempt(2), Some(Duration::from_secs(2)));
    assert_eq!(policy.delay_for_attempt(3), Some(Duration::from_secs(4)));
    assert_eq!(policy.delay_for_attempt(4), Some(Duration::from_secs(8)));
    assert_eq!(policy.delay_for_attempt(5), Some(Duration::from_secs(15)));
    assert_eq!(policy.delay_for_attempt(8), Some(Duration::from_secs(15)));
}

#[test]
fn equal_jitter_scales_delay_with_sample() {
    let policy = ExponentialBackoff {
        base: Duration::from_secs(4),
        max: Duration::from_secs(30),
        multiplier: 2.0,
        jitter: Jitter::Equal(0.25),
    };

    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 0.0),
        Some(Duration::from_secs(3))
    );
    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 0.5),
        Some(Duration::from_secs(4))
    );
    assert_eq!(
        policy.delay_for_attempt_with_sample(0, 1.0),
        Some(Duration::from_secs(5))
    );
}

#[test]
fn reconnect_policy_trait_uses_delay_math() {
    let mut policy = ExponentialBackoff {
        base: Duration::from_millis(250),
        max: Duration::from_secs(3),
        multiplier: 2.0,
        jitter: Jitter::None,
    };

    assert_eq!(
        policy.next_delay(2, ReconnectOutcome::SendFailure),
        Some(Duration::from_secs(1))
    );

    policy.reset();

    assert_eq!(
        policy.next_delay(0, ReconnectOutcome::ConnectFailure),
        Some(Duration::from_millis(250))
    );
}

#[tokio::test]
async fn closure_connector_connects_transports() {
    let mut connector = || async {
        let (transport, _) = InMemoryTransport::pair();
        Ok::<_, io::Error>(transport)
    };

    let _transport = connector.connect().await.expect("connect succeeds");
}

#[tokio::test]
async fn reconnecting_recovers_after_remote_close() {
    let peers = Arc::new(Mutex::new(Vec::new()));
    let connector_peers = Arc::clone(&peers);
    let connector = move || {
        let connector_peers = Arc::clone(&connector_peers);
        async move {
            let (client, server) = InMemoryTransport::pair();
            connector_peers
                .lock()
                .expect("peer store lock is not poisoned")
                .push(server);
            Ok::<_, io::Error>(client)
        }
    };

    let policy = ExponentialBackoff {
        base: Duration::from_millis(1),
        max: Duration::from_millis(1),
        multiplier: 1.0,
        jitter: Jitter::None,
    };
    let mut transport = Reconnecting::new(connector, policy);

    transport.connect().await.expect("initial connect succeeds");
    let mut first_peer = peers
        .lock()
        .expect("peer store lock is not poisoned")
        .pop()
        .expect("initial peer is captured");
    first_peer.close().await.expect("peer close succeeds");
    drop(first_peer);

    assert_eq!(
        transport.recv().await.expect("remote close reconnects"),
        None
    );
    assert!(transport.is_connected());
    assert_eq!(transport.last_delay(), Some(Duration::from_millis(1)));

    let mut second_peer = peers
        .lock()
        .expect("peer store lock is not poisoned")
        .pop()
        .expect("reconnected peer is captured");
    transport
        .send(Bytes::from_static(b"after-reconnect"))
        .await
        .expect("send succeeds after reconnect");
    assert_eq!(
        second_peer.recv().await.expect("peer recv succeeds"),
        Some(Bytes::from_static(b"after-reconnect"))
    );
}

#[tokio::test]
async fn reconnecting_reports_exhausted_policy() {
    let connector = || async {
        let (client, _) = InMemoryTransport::pair();
        Ok::<_, io::Error>(client)
    };
    let policy = ExponentialBackoff {
        base: Duration::ZERO,
        max: Duration::from_millis(1),
        multiplier: 1.0,
        jitter: Jitter::None,
    };
    let mut transport = Reconnecting::new(connector, policy);

    let error = transport
        .reconnect(ReconnectOutcome::ConnectFailure)
        .await
        .expect_err("zero base exhausts policy");
    assert!(matches!(
        error,
        ReconnectError::Exhausted { attempt: 0, .. }
    ));
}
