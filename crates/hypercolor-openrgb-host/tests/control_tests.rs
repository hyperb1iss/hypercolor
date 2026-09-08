use hypercolor_openrgb_host::{ControlReply, ControlRequest, ControlServer, send_control};
use serde_json::json;

#[tokio::test]
async fn control_owner_authenticates_and_returns_the_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut server = ControlServer::bind(dir.path(), "test-owner")
        .await
        .expect("bind");
    let duplicate = ControlServer::bind(dir.path(), "test-owner").await;
    assert!(duplicate.is_err());
    let task = tokio::spawn(async move {
        let request = server.accept().await.expect("accept");
        assert_eq!(request.request.action, "start");
        assert_eq!(request.request.payload, json!({"facts": 4}));
        request
            .respond(&ControlReply {
                status: json!({"managed_pid": 123}),
                error: None,
            })
            .await
            .expect("respond");
        server
    });
    let response = send_control(
        dir.path(),
        "test-owner",
        ControlRequest {
            action: "start".to_owned(),
            payload: json!({"facts": 4}),
        },
    )
    .await
    .expect("request")
    .expect("owner");
    assert_eq!(response.status["managed_pid"], 123);
    let server = task.await.expect("join");
    drop(server);
    assert!(
        send_control(
            dir.path(),
            "test-owner",
            ControlRequest {
                action: "stop".to_owned(),
                payload: json!(null)
            }
        )
        .await
        .expect("no owner")
        .is_none()
    );
}

#[tokio::test]
async fn stale_record_does_not_authorize_control() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("test-owner.json"),
        br#"{"address":"127.0.0.1:1","token":"stale"}"#,
    )
    .expect("stale record");
    assert!(
        send_control(
            dir.path(),
            "test-owner",
            ControlRequest {
                action: "stop".to_owned(),
                payload: json!(null)
            }
        )
        .await
        .expect("no owner")
        .is_none()
    );
}

#[tokio::test]
async fn unauthenticated_or_stalled_clients_do_not_block_control() {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut server = ControlServer::bind(dir.path(), "test-owner")
        .await
        .expect("bind");
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("test-owner.json")).expect("record"))
            .expect("decode");
    let address = record["address"].as_str().expect("address");
    let stalled = tokio::net::TcpStream::connect(address)
        .await
        .expect("stalled connection");
    let mut wrong = tokio::net::TcpStream::connect(address)
        .await
        .expect("wrong token connection");
    wrong
        .write_all(b"{\"token\":\"wrong\",\"request\":{\"action\":\"stop\",\"payload\":null}}\n")
        .await
        .expect("write");
    let task = tokio::spawn(async move {
        let request = server.accept().await.expect("accept");
        assert_eq!(request.request.action, "status");
        request
            .respond(&ControlReply {
                status: json!({"ok": true}),
                error: None,
            })
            .await
            .expect("respond");
    });
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        send_control(
            dir.path(),
            "test-owner",
            ControlRequest {
                action: "status".to_owned(),
                payload: json!(null),
            },
        ),
    )
    .await
    .expect("stalled authentication must not block another client")
    .expect("request")
    .expect("owner");
    assert_eq!(result.status["ok"], true);
    drop(stalled);
    task.await.expect("join");
}

#[cfg(unix)]
#[tokio::test]
async fn control_secret_is_readable_only_by_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let _server = ControlServer::bind(dir.path(), "test-owner")
        .await
        .expect("bind");
    let mode = std::fs::metadata(dir.path().join("test-owner.json"))
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn control_replaces_an_existing_public_secret_with_private_record() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("directory");
    let record = dir.path().join("test-owner.json");
    std::fs::write(&record, "stale").expect("stale record");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).expect("mode");
    let _server = ControlServer::bind(dir.path(), "test-owner")
        .await
        .expect("bind");
    assert_eq!(
        std::fs::metadata(record)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[tokio::test]
async fn requests_wait_for_registration_while_owner_holds_lock() {
    let dir = tempfile::tempdir().expect("directory");
    let mut server = ControlServer::bind(dir.path(), "test-owner")
        .await
        .expect("bind");
    let record = dir.path().join("test-owner.json");
    let bytes = std::fs::read(&record).expect("record");
    std::fs::remove_file(&record).expect("delay publication");
    let task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        std::fs::write(record, bytes).expect("publish");
        let pending = server.accept().await.expect("request");
        pending
            .respond(&ControlReply {
                status: json!({"ready": true}),
                error: None,
            })
            .await
            .expect("respond");
    });
    let result = send_control(
        dir.path(),
        "test-owner",
        ControlRequest {
            action: "status".to_owned(),
            payload: json!(null),
        },
    )
    .await
    .expect("registration handshake")
    .expect("living owner");
    assert_eq!(result.status["ready"], true);
    task.await.expect("server");
}

#[test]
fn app_cannot_claim_a_server_while_cli_child_is_still_starting() {
    use hypercolor_openrgb_host::try_claim_server;
    let directory = tempfile::tempdir().expect("directory");
    let endpoint = "127.0.0.1:6742".parse().expect("endpoint");
    let cli = try_claim_server(directory.path(), endpoint)
        .expect("claim")
        .expect("CLI wins");
    assert!(
        try_claim_server(directory.path(), endpoint)
            .expect("app claim")
            .is_none()
    );
    let alternate = "[::1]:6742".parse().expect("alternate loopback");
    assert!(
        try_claim_server(directory.path(), alternate)
            .expect("alternate claim")
            .is_none()
    );
    drop(cli);
    assert!(
        try_claim_server(directory.path(), endpoint)
            .expect("after stop")
            .is_some()
    );
}
