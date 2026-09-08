//! Lifecycle dispatch uses the daemon's proven local directory and living owners.

use hypercolor_cli::{
    client::DaemonClient,
    commands::openrgb_lifecycle::{execute_start, execute_stop},
};
use hypercolor_openrgb_host::{APP_CONTROL, ControlReply, ControlServer, OWNER_CONTROL};
use hypercolor_types::api::system::{ServerInfo, SystemResource, SystemStatus};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn daemon(
    directory: &std::path::Path,
    identity: &str,
) -> (DaemonClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let port = listener.local_addr().expect("address").port();
    let resource = SystemResource {
        identity: ServerInfo {
            instance_id: identity.to_owned(),
            ..ServerInfo::default()
        },
        status: Some(SystemStatus {
            data_dir: directory.display().to_string(),
            ..SystemStatus::default()
        }),
    };
    let body = json!({"data": resource}).to_string();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut bytes = [0; 4096];
        let len = stream.read(&mut bytes).await.expect("request");
        assert!(String::from_utf8_lossy(&bytes[..len]).starts_with("GET /api/v1/system "));
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("response");
    });
    (DaemonClient::new("127.0.0.1", port, None), task)
}

#[tokio::test]
async fn remote_lifecycle_rejects_before_host_operations() {
    let client = DaemonClient::new("192.0.2.1", 9420, None);
    assert!(
        execute_start(&client)
            .await
            .expect_err("remote start")
            .to_string()
            .contains("daemon's machine")
    );
    assert!(
        execute_stop(&client)
            .await
            .expect_err("remote stop")
            .to_string()
            .contains("daemon's machine")
    );
}

#[tokio::test]
async fn forwarded_identity_rejects_before_start_or_stop() {
    let directory = tempfile::tempdir().expect("directory");
    std::fs::write(directory.path().join("instance_id"), "local").expect("identity");
    for start in [true, false] {
        let (client, task) = daemon(directory.path(), "forwarded").await;
        let result = if start {
            execute_start(&client).await
        } else {
            execute_stop(&client).await
        };
        assert!(result.is_err());
        task.await.expect("fixture");
        assert!(!directory.path().join("control").exists());
    }
}

#[tokio::test]
async fn stop_reaches_retained_owner_after_app_adoption() {
    for (app_stopped, app_error) in [(false, None), (true, None), (false, Some("app failed"))] {
        let directory = tempfile::tempdir().expect("directory");
        std::fs::write(directory.path().join("instance_id"), "local").expect("identity");
        let control = directory.path().join("control");
        let mut app = ControlServer::bind(&control, APP_CONTROL)
            .await
            .expect("app");
        let mut owner = ControlServer::bind(&control, OWNER_CONTROL)
            .await
            .expect("owner");
        let app_task = tokio::spawn(async move {
            let request = app.accept().await.expect("app request");
            assert_eq!(request.request.action, "stop");
            request
                .respond(&ControlReply {
                    status: json!({"stopped": app_stopped}),
                    error: app_error.map(str::to_owned),
                })
                .await
                .expect("app response");
        });
        let owner_task = tokio::spawn(async move {
            let request = owner.accept().await.expect("owner request");
            assert_eq!(request.request.action, "stop");
            request
                .respond(&ControlReply {
                    status: json!({"stopped": !app_stopped}),
                    error: None,
                })
                .await
                .expect("owner response");
        });
        let (client, daemon_task) = daemon(directory.path(), "local").await;
        let response = execute_stop(&client).await;
        if app_error.is_some() {
            assert!(
                response
                    .expect_err("app error preserved")
                    .to_string()
                    .contains("app failed")
            );
        } else {
            assert_eq!(response.expect("stop").status["stopped"], true);
        }
        app_task.await.expect("app");
        owner_task.await.expect("owner");
        daemon_task.await.expect("daemon");
    }
}

#[tokio::test]
async fn explicit_owner_stops_and_releases_registration_without_a_daemon() {
    use hypercolor_cli::commands::openrgb_lifecycle::run_owner;
    use hypercolor_openrgb_host::{ControlRequest, send_control};
    let directory = tempfile::tempdir().expect("directory");
    let owner_dir = directory.path().to_owned();
    let owner = tokio::spawn(run_owner(owner_dir));
    let control = directory.path().join("control");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(reply) = send_control(
            &control,
            OWNER_CONTROL,
            ControlRequest {
                action: "stop".to_owned(),
                payload: json!(null),
            },
        )
        .await
        .expect("control")
        {
            assert!(reply.error.is_none());
            assert_eq!(reply.status["stopped"], false);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "owner must register"
        );
        tokio::task::yield_now().await;
    }
    owner.await.expect("owner task").expect("owner shutdown");
    let _replacement = ControlServer::bind(&control, OWNER_CONTROL)
        .await
        .expect("registration released");
}

#[tokio::test]
async fn start_forwards_current_facts_to_the_registered_app() {
    use hypercolor_openrgb_host::StartFacts;
    use hypercolor_types::{api::drivers::DriverConfigResponse, config::DriverConfigEntry};
    let directory = tempfile::tempdir().expect("directory");
    std::fs::write(directory.path().join("instance_id"), "local").expect("identity");
    let mut server = ControlServer::bind(&directory.path().join("control"), APP_CONTROL)
        .await
        .expect("app");
    let app = tokio::spawn(async move {
        let request = server.accept().await.expect("start request");
        assert_eq!(request.request.action, "start");
        let facts: StartFacts =
            serde_json::from_value(request.request.payload.clone()).expect("current facts");
        assert_eq!(facts.endpoint.to_string(), "127.0.0.1:6789");
        assert!(facts.drivers.is_empty());
        assert!(facts.devices.is_empty());
        request
            .respond(&ControlReply {
                status: json!({"managed_pid": 123}),
                error: None,
            })
            .await
            .expect("reply");
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("daemon");
    let port = listener.local_addr().expect("address").port();
    let system = SystemResource {
        identity: ServerInfo {
            instance_id: "local".to_owned(),
            ..ServerInfo::default()
        },
        status: Some(SystemStatus {
            data_dir: directory.path().display().to_string(),
            ..SystemStatus::default()
        }),
    };
    let mut current = DriverConfigEntry::default();
    current
        .settings
        .insert("endpoints".to_owned(), json!(["127.0.0.1:6789"]));
    let config = DriverConfigResponse {
        driver_id: "openrgb".to_owned(),
        config_key: "openrgb".to_owned(),
        configurable: true,
        current,
        default: None,
    };
    let routes = [
        ("/system", json!({"data": system})),
        ("/drivers", json!({"data": {"items": [], "total": 0}})),
        ("/devices", json!({"data": {"items": [], "total": 0}})),
        ("/drivers/openrgb/config", json!({"data": config})),
    ];
    let daemon = tokio::spawn(async move {
        for (route, body) in routes {
            let (mut stream, _) = listener.accept().await.expect("request");
            let mut bytes = [0; 4096];
            let length = stream.read(&mut bytes).await.expect("read");
            let request = String::from_utf8_lossy(&bytes[..length]);
            let path = request
                .split_whitespace()
                .nth(1)
                .expect("request target")
                .split('?')
                .next()
                .expect("path");
            assert_eq!(path, format!("/api/v1{route}"));
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.expect("response");
        }
    });
    let result = execute_start(&DaemonClient::new("127.0.0.1", port, None))
        .await
        .expect("app start");
    assert_eq!(result.status["managed_pid"], 123);
    daemon.await.expect("daemon fixture");
    app.await.expect("app fixture");
    assert!(
        !directory.path().join("logs").exists(),
        "app delegation must not spawn a holder"
    );
}

#[tokio::test]
async fn concurrent_holders_converge_on_one_registration() {
    use hypercolor_cli::commands::openrgb_lifecycle::run_owner;
    use hypercolor_openrgb_host::{ControlRequest, send_control};
    let directory = tempfile::tempdir().expect("directory");
    let first = tokio::spawn(run_owner(directory.path().to_owned()));
    let second = tokio::spawn(run_owner(directory.path().to_owned()));
    let control = directory.path().join("control");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !first.is_finished() && !second.is_finished() {
            let _ = send_control(
                &control,
                OWNER_CONTROL,
                ControlRequest {
                    action: "status".to_owned(),
                    payload: json!(null),
                },
            )
            .await
            .expect("registration probe");
            tokio::task::yield_now().await;
        }
        let reply = send_control(
            &control,
            OWNER_CONTROL,
            ControlRequest {
                action: "stop".to_owned(),
                payload: json!(null),
            },
        )
        .await
        .expect("stop")
        .expect("one holder");
        assert!(reply.error.is_none());
        first.await.expect("first task").expect("first owner");
        second.await.expect("second task").expect("second owner");
    })
    .await
    .expect("cooperative lock collision must converge");
}
