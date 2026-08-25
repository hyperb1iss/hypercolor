use anyhow::Context;
use async_trait::async_trait;
use hypercolor_core::session::SessionMonitor;
use hypercolor_types::session::SessionEvent;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::decode::{ScmNotification, decode_scm_notification};

/// Non-blocking adapter for Windows Service Control Manager callbacks.
#[derive(Clone)]
pub struct ScmSessionEventAdapter {
    tx: mpsc::UnboundedSender<SessionEvent>,
}

impl ScmSessionEventAdapter {
    /// Publish a decoded SCM notification to its session monitor.
    ///
    /// Returns `true` when the notification represented a session event.
    /// Delivery is best-effort because SCM acknowledgment must not depend on
    /// whether session policy is enabled.
    pub fn publish(&self, notification: ScmNotification) -> bool {
        let Some(event) = decode_scm_notification(notification) else {
            return false;
        };
        let _ = self.tx.send(event);
        true
    }

    /// Publish a native Windows service control callback.
    ///
    /// Returns `true` when the callback represented a session event.
    #[cfg(target_os = "windows")]
    pub fn publish_service_control(
        &self,
        control: &windows_service::service::ServiceControl,
    ) -> bool {
        self.publish(crate::decode::notification_from_service_control(control))
    }
}

/// Session monitor fed by the Windows service control handler.
pub struct ScmSessionMonitor {
    rx: mpsc::UnboundedReceiver<SessionEvent>,
}

/// Create the paired SCM callback adapter and session monitor.
#[must_use]
pub fn scm_session_monitor() -> (ScmSessionEventAdapter, ScmSessionMonitor) {
    let (tx, rx) = mpsc::unbounded_channel();
    (ScmSessionEventAdapter { tx }, ScmSessionMonitor { rx })
}

#[async_trait]
impl SessionMonitor for ScmSessionMonitor {
    fn name(&self) -> &'static str {
        "windows-scm"
    }

    async fn run(
        mut self: Box<Self>,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                maybe_event = self.rx.recv() => {
                    let Some(event) = maybe_event else {
                        return Ok(());
                    };
                    tx.send(event)
                        .await
                        .context("core session stream closed while SCM monitor was active")?;
                }
            }
        }
    }
}
