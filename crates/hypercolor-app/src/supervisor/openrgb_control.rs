//! Local CLI requests routed to the live desktop supervisor.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use hypercolor_openrgb_host::{
    APP_CONTROL, ControlReply, ControlRequest, ControlServer, StartFacts, verify_instance_directory,
};
use hypercolor_types::api::envelope::ApiResponse;
use hypercolor_types::api::system::SystemResource;
use serde_json::Value;
use tauri::{Listener, Manager, Runtime};

use super::openrgb::{OpenRgbPlanSummary, OpenRgbSupervisor};
use super::{
    SupervisorState, VERIFIED_DAEMON_CONNECTION_CHANGED_EVENT, VerifiedDaemonConnectionSnapshot,
};

/// Resolve the configured daemon to an actual local instance before touching
/// local configuration or registering its app control endpoint.
pub async fn local_directory(http: &reqwest::Client, base_url: &str) -> Result<PathBuf> {
    ensure!(
        is_local_daemon(base_url),
        "OpenRGB control requires a local daemon"
    );
    let response: ApiResponse<SystemResource> = http
        .get(format!("{}/api/v1/system", base_url.trim_end_matches('/')))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let status = response
        .data
        .status
        .context("authorized local daemon status is required")?;
    let directory = PathBuf::from(status.data_dir);
    verify_instance_directory(&directory, &response.data.identity.instance_id)?;
    Ok(directory)
}

fn is_local_daemon(base_url: &str) -> bool {
    url::Url::parse(base_url).ok().is_some_and(|url| {
        matches!(url.host(), Some(url::Host::Domain("localhost")))
            || matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
            || matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback())
    })
}

/// Follow the supervisor's verified connection events. Registration begins
/// after daemon readiness, and is withdrawn when its connection proof clears.
pub async fn serve<R: Runtime>(app: tauri::AppHandle<R>) -> Result<()> {
    let state = app.state::<SupervisorState>().inner().clone();
    let (sender, receiver) =
        tokio::sync::watch::channel(VerifiedDaemonConnectionSnapshot::default());
    let event_state = state.clone();
    let event_sender = sender.clone();
    let listener = app.listen(VERIFIED_DAEMON_CONNECTION_CHANGED_EVENT, move |_| {
        publish_connection(&event_sender, event_state.verified_daemon_connection());
    });
    // Subscribe before the snapshot; the revision guard prevents an older
    // snapshot overwriting a concurrent connection event.
    publish_connection(&sender, state.verified_daemon_connection());
    let result =
        serve_registration(receiver, |base_url| serve_connection(app.clone(), base_url)).await;
    app.unlisten(listener);
    result
}

fn publish_connection(
    sender: &tokio::sync::watch::Sender<VerifiedDaemonConnectionSnapshot>,
    next: VerifiedDaemonConnectionSnapshot,
) {
    sender.send_if_modified(|current| {
        if next.revision <= current.revision {
            return false;
        }
        *current = next;
        true
    });
}

/// Maintain control registration while the verified daemon session remains
/// current. Transient registration failures retry without requiring a new URL;
/// session changes cancel the old listener immediately, including during retry.
pub async fn serve_registration<F, Fut>(
    mut receiver: tokio::sync::watch::Receiver<VerifiedDaemonConnectionSnapshot>,
    mut register: F,
) -> Result<()>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    loop {
        let current = receiver.borrow_and_update().clone();
        if let Some(connection) = current
            .connection
            .filter(|connection| is_local_daemon(&connection.base_url))
        {
            tokio::select! {
                result = register(connection.base_url) => {
                    if let Err(error) = result {
                        tracing::warn!(%error, "OpenRGB CLI registration unavailable; retrying");
                    }
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                        changed = receiver.changed() => { if changed.is_err() { return Ok(()); } }
                    }
                }
                changed = receiver.changed() => { if changed.is_err() { return Ok(()); } }
            }
        } else if receiver.changed().await.is_err() {
            return Ok(());
        }
    }
}

async fn serve_connection<R: Runtime>(app: tauri::AppHandle<R>, base_url: String) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let directory = local_directory(&http, &base_url).await?;
    let mut server = ControlServer::bind(&directory.join("control"), APP_CONTROL).await?;
    loop {
        let pending = server.accept().await?;
        let supervisor = app.state::<OpenRgbSupervisor>();
        let reply = dispatch(&supervisor, &pending.request, &base_url).await;
        if let Err(error) = pending.respond(&reply).await {
            tracing::debug!(%error, "OpenRGB CLI requester disconnected");
        }
    }
}

/// Dispatch an authenticated request without exposing a daemon credential.
pub async fn dispatch(
    supervisor: &OpenRgbSupervisor,
    request: &ControlRequest,
    base_url: &str,
) -> ControlReply {
    let result: Result<Value> = match request.action.as_str() {
        "start" => match serde_json::from_value::<StartFacts>(request.payload.clone()) {
            Ok(facts) => {
                match supervisor
                    .start_for_endpoint(base_url, facts.endpoint)
                    .await
                {
                    Ok(status) => {
                        if let Some(message) = status
                            .plan_summary
                            .and_then(OpenRgbPlanSummary::hold_message)
                        {
                            return ControlReply {
                                error: Some(message.to_owned()),
                                status: serde_json::to_value(status).unwrap_or(Value::Null),
                            };
                        }
                        serde_json::to_value(status).map_err(Into::into)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error.into()),
        },
        "stop" => {
            let supervisor = supervisor.clone();
            let stopped = tokio::task::spawn_blocking(move || {
                let owned = supervisor.managed_pid().is_some();
                supervisor.stop_managed();
                (owned, supervisor.status())
            })
            .await;
            match stopped {
                Ok((owned, snapshot)) => serde_json::to_value(snapshot)
                    .map(|mut status| {
                        status["stopped"] = owned.into();
                        status
                    })
                    .map_err(Into::into),
                Err(error) => Err(anyhow::anyhow!("OpenRGB stop task failed: {error}")),
            }
        }
        "status" => serde_json::to_value(supervisor.status()).map_err(Into::into),
        _ => Err(anyhow::anyhow!("unknown OpenRGB control action")),
    };
    match result {
        Ok(status) => ControlReply {
            status,
            error: None,
        },
        Err(error) => ControlReply {
            status: Value::Null,
            error: Some(error.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::publish_connection;
    use crate::supervisor::VerifiedDaemonConnectionSnapshot;

    #[test]
    fn late_initial_snapshot_cannot_replace_a_newer_connection_event() {
        let (sender, receiver) =
            tokio::sync::watch::channel(VerifiedDaemonConnectionSnapshot::default());
        publish_connection(
            &sender,
            VerifiedDaemonConnectionSnapshot {
                revision: 2,
                connection: None,
            },
        );
        publish_connection(
            &sender,
            VerifiedDaemonConnectionSnapshot {
                revision: 1,
                connection: None,
            },
        );
        assert_eq!(receiver.borrow().revision, 2);
    }
}
