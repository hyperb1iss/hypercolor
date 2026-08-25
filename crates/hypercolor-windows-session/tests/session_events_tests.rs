use hypercolor_core::session::SessionMonitor;
use hypercolor_types::session::SessionEvent;
use hypercolor_windows_session::{
    PBT_APMRESUMEAUTOMATIC_NOTIFICATION, PBT_APMRESUMECRITICAL_NOTIFICATION,
    PBT_APMRESUMESUSPEND_NOTIFICATION, PBT_APMSUSPEND_NOTIFICATION, ScmNotification,
    WM_POWERBROADCAST_MESSAGE, WM_WTSSESSION_CHANGE_MESSAGE, WTS_SESSION_LOCK_NOTIFICATION,
    WTS_SESSION_UNLOCK_NOTIFICATION, decode_scm_notification, decode_window_message,
    scm_session_monitor, standalone_monitors,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[test]
fn standalone_and_scm_transports_emit_the_same_neutral_stream() {
    let standalone = [
        (WM_POWERBROADCAST_MESSAGE, PBT_APMSUSPEND_NOTIFICATION),
        (
            WM_POWERBROADCAST_MESSAGE,
            PBT_APMRESUMEAUTOMATIC_NOTIFICATION,
        ),
        (WM_WTSSESSION_CHANGE_MESSAGE, WTS_SESSION_LOCK_NOTIFICATION),
        (
            WM_WTSSESSION_CHANGE_MESSAGE,
            WTS_SESSION_UNLOCK_NOTIFICATION,
        ),
    ]
    .map(|(message, notification)| {
        decode_window_message(message, notification)
            .expect("fixture should be a recognized standalone event")
    });
    let scm = [
        ScmNotification::Suspend,
        ScmNotification::ResumeAutomatic,
        ScmNotification::SessionLocked,
        ScmNotification::SessionUnlocked,
    ]
    .map(|notification| {
        decode_scm_notification(notification).expect("fixture should be a recognized SCM event")
    });

    assert_eq!(
        standalone,
        [
            SessionEvent::Suspending,
            SessionEvent::Resumed,
            SessionEvent::ScreenLocked,
            SessionEvent::ScreenUnlocked,
        ]
    );
    assert_eq!(standalone, scm);
}

#[test]
fn all_windows_resume_notifications_share_one_event() {
    for notification in [
        PBT_APMRESUMEAUTOMATIC_NOTIFICATION,
        PBT_APMRESUMESUSPEND_NOTIFICATION,
        PBT_APMRESUMECRITICAL_NOTIFICATION,
    ] {
        assert_eq!(
            decode_window_message(WM_POWERBROADCAST_MESSAGE, notification),
            Some(SessionEvent::Resumed)
        );
    }

    for notification in [
        ScmNotification::ResumeAutomatic,
        ScmNotification::ResumeInteractive,
        ScmNotification::ResumeCritical,
    ] {
        assert_eq!(
            decode_scm_notification(notification),
            Some(SessionEvent::Resumed)
        );
    }
}

#[test]
fn unrelated_notifications_are_ignored() {
    assert_eq!(decode_window_message(0xFFFF, 0xFFFF), None);
    assert_eq!(decode_scm_notification(ScmNotification::Other), None);
}

#[tokio::test]
async fn scm_adapter_forwards_callbacks_through_the_monitor() {
    let (adapter, monitor) = scm_session_monitor();
    let (tx, mut rx) = mpsc::channel(8);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(Box::new(monitor).run(tx, cancel.clone()));

    for notification in [
        ScmNotification::Suspend,
        ScmNotification::ResumeAutomatic,
        ScmNotification::SessionLocked,
        ScmNotification::SessionUnlocked,
    ] {
        assert!(adapter.publish(notification));
    }

    for expected in [
        SessionEvent::Suspending,
        SessionEvent::Resumed,
        SessionEvent::ScreenLocked,
        SessionEvent::ScreenUnlocked,
    ] {
        assert_eq!(rx.recv().await, Some(expected));
    }

    cancel.cancel();
    task.await
        .expect("SCM monitor task should join")
        .expect("SCM monitor should stop cleanly");
}

#[test]
fn scm_adapter_acknowledges_recognized_events_after_monitor_shutdown() {
    let (adapter, monitor) = scm_session_monitor();
    drop(monitor);

    assert!(adapter.publish(ScmNotification::Suspend));
    assert!(adapter.publish(ScmNotification::SessionLocked));
    assert!(!adapter.publish(ScmNotification::Other));
}

#[test]
fn standalone_monitor_set_matches_host_support() {
    let monitors = standalone_monitors();

    #[cfg(target_os = "windows")]
    assert_eq!(
        monitors
            .iter()
            .map(|monitor| monitor.name())
            .collect::<Vec<_>>(),
        ["windows-message-window"]
    );

    #[cfg(not(target_os = "windows"))]
    assert!(monitors.is_empty());
}
