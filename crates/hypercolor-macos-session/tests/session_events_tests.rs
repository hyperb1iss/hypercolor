use hypercolor_core::session::{SessionMonitor, SessionWatcher, SleepPolicy};
use hypercolor_macos_session::{MacosSessionNotification, decode_session_notification, monitors};
use hypercolor_types::session::SessionEvent;
use hypercolor_types::session::{SessionConfig, SleepAction, WakeAction};

#[test]
fn macos_notifications_emit_the_neutral_session_stream() {
    let events = [
        MacosSessionNotification::SystemWillSleep,
        MacosSessionNotification::SystemPoweredOn,
        MacosSessionNotification::SessionResigned,
        MacosSessionNotification::SessionBecameActive,
    ]
    .map(|notification| {
        decode_session_notification(notification).expect("fixture should be recognized")
    });

    assert_eq!(
        events,
        [
            SessionEvent::Suspending,
            SessionEvent::Resumed,
            SessionEvent::SessionInactive,
            SessionEvent::SessionActive,
        ]
    );
}

#[test]
fn repeated_native_notifications_preserve_transition_evidence() {
    let events = [
        MacosSessionNotification::SystemWillSleep,
        MacosSessionNotification::SystemWillSleep,
        MacosSessionNotification::SystemPoweredOn,
        MacosSessionNotification::SystemPoweredOn,
        MacosSessionNotification::SessionResigned,
        MacosSessionNotification::SessionResigned,
        MacosSessionNotification::SessionBecameActive,
        MacosSessionNotification::SessionBecameActive,
    ]
    .map(decode_session_notification);

    assert_eq!(
        events,
        [
            Some(SessionEvent::Suspending),
            Some(SessionEvent::Suspending),
            Some(SessionEvent::Resumed),
            Some(SessionEvent::Resumed),
            Some(SessionEvent::SessionInactive),
            Some(SessionEvent::SessionInactive),
            Some(SessionEvent::SessionActive),
            Some(SessionEvent::SessionActive),
        ]
    );
}

#[test]
fn unrelated_notifications_are_ignored() {
    assert_eq!(
        decode_session_notification(MacosSessionNotification::Other),
        None
    );
}

#[test]
fn monitor_set_matches_host_support() {
    let monitors = monitors();

    #[cfg(target_os = "macos")]
    assert_eq!(
        monitors
            .iter()
            .map(|monitor| monitor.name())
            .collect::<Vec<_>>(),
        ["macos-workspace-iokit"]
    );

    #[cfg(not(target_os = "macos"))]
    assert!(monitors.is_empty());
}

struct NotificationFixture {
    release: std::sync::Arc<tokio::sync::Notify>,
    notifications: Vec<MacosSessionNotification>,
}

#[async_trait::async_trait]
impl SessionMonitor for NotificationFixture {
    fn name(&self) -> &'static str {
        "macos-notification-fixture"
    }

    async fn run(
        self: Box<Self>,
        tx: tokio::sync::mpsc::Sender<SessionEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<()> {
        tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            () = self.release.notified() => {}
        }

        for notification in self.notifications {
            if let Some(event) = decode_session_notification(notification) {
                tx.send(event)
                    .await
                    .map_err(|_| anyhow::anyhow!("session watcher closed fixture stream"))?;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn macos_duplicate_evidence_uses_the_shared_deduplication_boundary() {
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let monitor = NotificationFixture {
        release: release.clone(),
        notifications: vec![
            MacosSessionNotification::SystemWillSleep,
            MacosSessionNotification::SystemWillSleep,
            MacosSessionNotification::SystemPoweredOn,
            MacosSessionNotification::SystemPoweredOn,
            MacosSessionNotification::SessionResigned,
            MacosSessionNotification::SessionResigned,
            MacosSessionNotification::SessionBecameActive,
            MacosSessionNotification::SessionBecameActive,
        ],
    };
    let watcher = SessionWatcher::start(&SessionConfig::default(), vec![Box::new(monitor)]);
    let mut events = watcher.subscribe();
    release.notify_one();

    for expected in [
        SessionEvent::Suspending,
        SessionEvent::Resumed,
        SessionEvent::SessionInactive,
        SessionEvent::SessionActive,
    ] {
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                .await
                .expect("fixture event should arrive")
                .expect("fixture stream should remain open"),
            expected
        );
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );

    watcher.shutdown().await;
}

#[test]
fn macos_events_feed_the_shared_sleep_policy() {
    let policy = SleepPolicy::new(SessionConfig::default());
    let suspending = decode_session_notification(MacosSessionNotification::SystemWillSleep)
        .expect("system sleep should normalize");
    let resumed = decode_session_notification(MacosSessionNotification::SystemPoweredOn)
        .expect("system wake should normalize");
    let inactive = decode_session_notification(MacosSessionNotification::SessionResigned)
        .expect("session resign should normalize");
    let active = decode_session_notification(MacosSessionNotification::SessionBecameActive)
        .expect("session activation should normalize");

    assert!(matches!(
        policy.sleep_action(&suspending),
        Some(SleepAction::Off { fade_ms: 300, .. })
    ));
    assert_eq!(
        policy.wake_action(&resumed),
        Some(WakeAction::Restore { fade_ms: 150 })
    );
    assert_eq!(policy.sleep_action(&inactive), Some(SleepAction::Ignore));
    assert_eq!(
        policy.wake_action(&active),
        Some(WakeAction::Restore { fade_ms: 500 })
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_monitor_teardown_is_bounded_and_restartable() {
    use std::time::Duration;

    use hypercolor_core::session::SessionMonitor;
    use hypercolor_macos_session::MacosSessionMonitor;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    for _ in 0..2 {
        let (tx, _rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(Box::new(MacosSessionMonitor::new()).run(tx, cancel.clone()));

        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("native monitor teardown should be bounded")
            .expect("native monitor task should join")
            .expect("native monitor should stop cleanly");
    }
}
