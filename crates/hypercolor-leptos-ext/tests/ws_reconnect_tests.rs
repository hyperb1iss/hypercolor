#![cfg(feature = "ws-core")]

use hypercolor_leptos_ext::ws::transport::InMemoryTransport;
use hypercolor_leptos_ext::ws::{
    Connector, ExponentialBackoff, Jitter, ReconnectOutcome, ReconnectPolicy,
};
use std::io;
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
