use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::{DeduplicationState, SessionMonitor, SessionWatcher};
use crate::types::session::{SessionConfig, SessionEvent};

struct FixtureMonitor {
    release: Arc<Notify>,
    events: Vec<SessionEvent>,
}

#[async_trait::async_trait]
impl SessionMonitor for FixtureMonitor {
    fn name(&self) -> &'static str {
        "fixture"
    }

    async fn run(
        self: Box<Self>,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            () = self.release.notified() => {}
        }

        for event in self.events {
            tx.send(event)
                .await
                .map_err(|_| anyhow::anyhow!("session watcher closed fixture stream"))?;
        }

        Ok(())
    }
}

struct StartProbe {
    started: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl SessionMonitor for StartProbe {
    fn name(&self) -> &'static str {
        "start-probe"
    }

    async fn run(
        self: Box<Self>,
        _tx: mpsc::Sender<SessionEvent>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        self.started.store(true, Ordering::Release);
        Ok(())
    }
}

#[tokio::test]
async fn injected_monitor_events_share_the_neutral_dedup_stream() {
    let release = Arc::new(Notify::new());
    let monitor = FixtureMonitor {
        release: release.clone(),
        events: vec![
            SessionEvent::ScreenLocked,
            SessionEvent::ScreenLocked,
            SessionEvent::ScreenUnlocked,
            SessionEvent::ScreenUnlocked,
        ],
    };
    let watcher = SessionWatcher::start(&SessionConfig::default(), vec![Box::new(monitor)]);
    let mut events = watcher.subscribe();

    release.notify_one();

    assert_eq!(receive_event(&mut events).await, SessionEvent::ScreenLocked);
    assert_eq!(
        receive_event(&mut events).await,
        SessionEvent::ScreenUnlocked
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );

    watcher.shutdown().await;
}

#[tokio::test]
async fn disabled_session_config_does_not_start_injected_monitors() {
    let started = Arc::new(AtomicBool::new(false));
    let config = SessionConfig {
        enabled: false,
        ..SessionConfig::default()
    };
    let watcher = SessionWatcher::start(
        &config,
        vec![Box::new(StartProbe {
            started: started.clone(),
        })],
    );

    tokio::task::yield_now().await;
    assert!(!started.load(Ordering::Acquire));

    watcher.shutdown().await;
}

#[test]
fn deduplication_preserves_every_state_transition_pair() {
    let transitions = [
        (SessionEvent::ScreenLocked, SessionEvent::ScreenUnlocked),
        (SessionEvent::SessionInactive, SessionEvent::SessionActive),
        (SessionEvent::Suspending, SessionEvent::Resumed),
        (
            SessionEvent::IdleEntered {
                idle_duration: Duration::from_secs(120),
            },
            SessionEvent::IdleExited,
        ),
        (SessionEvent::LidClosed, SessionEvent::LidOpened),
    ];

    for (entered, exited) in transitions {
        let mut dedup = DeduplicationState::default();
        assert!(dedup.should_forward(&entered));
        assert!(!dedup.should_forward(&entered));
        assert!(dedup.should_forward(&exited));
        assert!(!dedup.should_forward(&exited));
    }
}

async fn receive_event(events: &mut broadcast::Receiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("fixture event should arrive before the timeout")
        .expect("session watcher should remain open")
}
