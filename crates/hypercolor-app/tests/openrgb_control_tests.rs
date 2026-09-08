use hypercolor_app::supervisor::{openrgb::OpenRgbSupervisor, openrgb_control::dispatch};
use hypercolor_openrgb_host::ControlRequest;
use serde_json::Value;

#[tokio::test]
async fn local_control_reports_idle_status_without_claiming_an_external_process() {
    let supervisor = OpenRgbSupervisor::default();
    for action in ["status", "stop"] {
        let reply = dispatch(
            &supervisor,
            &ControlRequest {
                action: action.to_owned(),
                payload: Value::Null,
            },
            "http://127.0.0.1:9420",
        )
        .await;
        assert!(reply.error.is_none());
        assert!(reply.status["managed_pid"].is_null());
        if action == "stop" {
            assert_eq!(reply.status["stopped"], false);
        }
    }
}

#[tokio::test]
async fn malformed_start_and_unknown_operations_do_not_reach_hardware() {
    let supervisor = OpenRgbSupervisor::default();
    for action in ["start", "unknown"] {
        let reply = dispatch(
            &supervisor,
            &ControlRequest {
                action: action.to_owned(),
                payload: Value::Null,
            },
            "http://127.0.0.1:1",
        )
        .await;
        assert!(reply.error.is_some());
        assert!(supervisor.managed_pid().is_none());
    }
}

fn connection(
    revision: u64,
    url: Option<&str>,
) -> hypercolor_app::supervisor::VerifiedDaemonConnectionSnapshot {
    use hypercolor_app::supervisor::{VerifiedDaemonConnection, VerifiedDaemonConnectionSnapshot};
    VerifiedDaemonConnectionSnapshot {
        revision,
        connection: url.map(|url| VerifiedDaemonConnection {
            base_url: url.to_owned(),
            server_session_id: None,
            protected_control_credential: None,
        }),
    }
}

#[tokio::test]
async fn registration_recovers_on_the_same_connection_after_a_transient_failure() {
    use hypercolor_app::supervisor::openrgb_control::serve_registration;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (sender, receiver) =
        tokio::sync::watch::channel(connection(1, Some("http://127.0.0.1:9420")));
    let (registered, observed) = tokio::sync::oneshot::channel();
    let mut registered = Some(registered);
    let attempts = Arc::new(AtomicUsize::new(0));
    let calls = attempts.clone();
    let task = tokio::spawn(serve_registration(receiver, move |_| {
        let attempt = calls.fetch_add(1, Ordering::SeqCst);
        let ready = if attempt == 0 {
            None
        } else {
            registered.take()
        };
        async move {
            if attempt == 0 {
                anyhow::bail!("temporary registration failure");
            }
            if let Some(ready) = ready {
                let _ = ready.send(());
            }
            std::future::pending().await
        }
    }));
    tokio::time::timeout(std::time::Duration::from_secs(3), observed)
        .await
        .expect("registration retries")
        .expect("registered");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    drop(sender);
    task.await
        .expect("registration task")
        .expect("clean shutdown");
}

#[tokio::test]
async fn registration_withdraws_and_replaces_a_session_even_at_the_same_url() {
    use hypercolor_app::supervisor::openrgb_control::serve_registration;
    let (sender, receiver) =
        tokio::sync::watch::channel(connection(1, Some("http://127.0.0.1:9420")));
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(serve_registration(receiver, move |url| {
        let started = started.clone();
        async move {
            started.send(url).expect("observer remains");
            std::future::pending().await
        }
    }));
    observed.recv().await.expect("first registration");
    sender.send(connection(2, None)).expect("withdraw session");
    sender
        .send(connection(3, Some("http://127.0.0.1:9420")))
        .expect("new session");
    tokio::time::timeout(std::time::Duration::from_secs(1), observed.recv())
        .await
        .expect("replacement is prompt")
        .expect("new registration");
    drop(sender);
    task.await
        .expect("registration task")
        .expect("clean shutdown");
}

#[tokio::test]
async fn remote_daemon_does_not_schedule_local_registration_retries() {
    use hypercolor_app::supervisor::openrgb_control::serve_registration;
    let (sender, receiver) =
        tokio::sync::watch::channel(connection(1, Some("http://192.0.2.1:9420")));
    let (started, mut observed) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(serve_registration(receiver, move |url| {
        let started = started.clone();
        async move {
            started.send(url).expect("observer remains");
            std::future::pending().await
        }
    }));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), observed.recv())
            .await
            .is_err()
    );
    sender
        .send(connection(2, Some("http://127.0.0.1:9420")))
        .expect("select local daemon");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), observed.recv())
            .await
            .expect("local registration")
            .as_deref(),
        Some("http://127.0.0.1:9420")
    );
    drop(sender);
    task.await
        .expect("registration task")
        .expect("clean shutdown");
}

#[test]
fn holds_offer_prose_without_treating_an_existing_start_as_an_error() {
    use hypercolor_app::supervisor::openrgb::OpenRgbPlanSummary;
    assert_eq!(
        OpenRgbPlanSummary::HoldNotInstalled.hold_message(),
        Some("Install OpenRGB before starting its server")
    );
    assert!(
        OpenRgbPlanSummary::HoldPermissionsMissing
            .hold_message()
            .expect("permission remedy")
            .contains("permissions")
    );
    assert!(
        OpenRgbPlanSummary::HoldBridgeDisabled
            .hold_message()
            .expect("enable remedy")
            .contains("Enable")
    );
    assert!(
        OpenRgbPlanSummary::HoldPortOwnedByUnknown
            .hold_message()
            .expect("port remedy")
            .contains("another service")
    );
    for summary in [
        OpenRgbPlanSummary::Adopt,
        OpenRgbPlanSummary::Spawn,
        OpenRgbPlanSummary::HoldStarting,
        OpenRgbPlanSummary::HoldOtherOwner,
    ] {
        assert!(summary.hold_message().is_none());
    }
}
