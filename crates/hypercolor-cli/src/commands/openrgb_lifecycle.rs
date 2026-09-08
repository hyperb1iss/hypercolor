//! OpenRGB lifecycle through a living app or an explicit persistent CLI owner.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use hypercolor_openrgb_host::{
    APP_CONTROL, ControlReply, ControlRequest, ControlServer, DeviceFacts, DriverFacts,
    OWNER_CONTROL, OpenRgbOwner, StartFacts, configured_endpoint, send_control, spawn_owner,
};
use hypercolor_types::api::devices::DeviceSummary;
use hypercolor_types::api::drivers::{DriverConfigResponse, DriverSummary};
use serde_json::{Value, json};

use crate::client::DaemonClient;

/// Start the bridge server on the verified local daemon's host. Authentication
/// remains in this foreground client; only nonsecret current facts cross IPC.
pub async fn execute_start(client: &DaemonClient) -> Result<ControlReply> {
    let directory = client.local_data_dir().await?;
    let drivers = client.get_list::<DriverSummary>("/drivers").await?.items;
    let devices = client.get_list::<DeviceSummary>("/devices").await?.items;
    let config: DriverConfigResponse = client.get("/drivers/openrgb/config").await?;
    let endpoint = configured_endpoint(&config.current)?;
    let facts = StartFacts {
        drivers: drivers.iter().map(DriverFacts::from).collect(),
        devices: devices.iter().map(DeviceFacts::from).collect(),
        endpoint,
    };
    let request = ControlRequest {
        action: "start".to_owned(),
        payload: serde_json::to_value(facts)?,
    };
    let control = directory.join("control");
    if let Some(reply) = send_control(&control, APP_CONTROL, request.clone()).await? {
        return checked_reply(reply);
    }
    if let Some(reply) = send_control(&control, OWNER_CONTROL, request.clone()).await? {
        return checked_reply(reply);
    }

    let executable = std::env::current_exe().context("cannot locate the Hypercolor CLI")?;
    let mut owner = spawn_owner(&executable, &directory)
        .context("cannot start the persistent OpenRGB owner")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match send_control(&control, OWNER_CONTROL, request.clone()).await {
            Ok(Some(reply)) => return checked_reply(reply),
            Ok(None) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => return Err(error.into()),
        }
        if tokio::time::Instant::now() >= deadline {
            let exit = owner.try_wait()?;
            bail!("OpenRGB owner did not register (exit: {exit:?}); see logs/openrgb-owner.log");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Stop only an OpenRGB child retained by the app or the CLI owner.
pub async fn execute_stop(client: &DaemonClient) -> Result<ControlReply> {
    let directory = client.local_data_dir().await?;
    let control = directory.join("control");
    let request = ControlRequest {
        action: "stop".to_owned(),
        payload: Value::Null,
    };
    let (app, owner) = tokio::join!(
        send_control(&control, APP_CONTROL, request.clone()),
        send_control(&control, OWNER_CONTROL, request),
    );
    let mut stopped = false;
    let mut errors = Vec::new();
    let mut statuses = Vec::new();
    for (name, result) in [("app", app), ("owner", owner)] {
        match result {
            Ok(Some(reply)) => {
                stopped |= reply
                    .status
                    .get("stopped")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if let Some(error) = reply.error {
                    errors.push(format!("{name}: {error}"));
                }
                statuses.push(reply.status);
            }
            Ok(None) => {}
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    checked_reply(ControlReply {
        status: json!({"managed_pid": null, "stopped": stopped, "owners": statuses}),
        error: (!errors.is_empty()).then(|| {
            format!(
                "OpenRGB stop completed (stopped: {stopped}) with errors: {}",
                errors.join("; ")
            )
        }),
    })
}

fn checked_reply(reply: ControlReply) -> Result<ControlReply> {
    if let Some(error) = &reply.error {
        bail!("{error}");
    }
    Ok(reply)
}

/// Hidden CLI entry point. Remains alive to retain the child handle and answer
/// later explicit requests; a child exit never triggers a restart.
pub async fn run_owner(directory: PathBuf) -> Result<()> {
    serve_owner(&directory).await
}

async fn serve_owner(directory: &Path) -> Result<()> {
    let control = directory.join("control");
    let mut server = loop {
        match ControlServer::bind(&control, OWNER_CONTROL).await {
            Ok(server) => break server,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if send_control(
                    &control,
                    OWNER_CONTROL,
                    ControlRequest {
                        action: "status".to_owned(),
                        payload: Value::Null,
                    },
                )
                .await?
                .is_some()
                {
                    return Ok(());
                }
                tokio::task::yield_now().await;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let mut owner = OpenRgbOwner::new(directory.to_owned());
    let mut reap = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = reap.tick() => {
                if let Err(error) = owner.reap() {
                    tracing::warn!(%error, "OpenRGB child exit observation failed; ownership retained");
                }
            }
            pending = server.accept() => {
                let pending = pending?;
                let result: Result<Value> = match pending.request.action.as_str() {
                    "start" => match serde_json::from_value::<StartFacts>(pending.request.payload.clone()) {
                        Ok(facts) => owner.start(facts).await.map_err(Into::into),
                        Err(error) => Err(error.into()),
                    },
                    "stop" => {
                        let (retained, result) = tokio::task::spawn_blocking(move || {
                            let result = owner.stop();
                            (owner, result)
                        }).await?;
                        owner = retained;
                        result.map_err(Into::into)
                    },
                    "status" => Ok(owner.status()),
                    _ => Err(anyhow::anyhow!("unknown OpenRGB control action")),
                };
                let reply = match result {
                    Ok(status) => ControlReply { status, error: None },
                    Err(error) => ControlReply { status: owner.status(), error: Some(error.to_string()) },
                };
                let shutdown = pending.request.action == "stop" && reply.error.is_none();
                let _ = pending.respond(&reply).await;
                if shutdown { return Ok(()); }
            }
        }
    }
}
