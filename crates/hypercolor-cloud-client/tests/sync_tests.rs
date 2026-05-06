use std::time::Duration;

use hypercolor_cloud_client::{
    CloudClient, CloudClientConfig, CloudClientError, Etag, SyncEntityKind, SyncPutRequest,
};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn list_sync_entities_uses_kind_path_and_bearer_auth() {
    let server = OneShotServer::bind().await;
    let base_url = server.base_url();
    let task = server.spawn(|request| {
        assert!(request.starts_with("GET /v1/sync/scenes HTTP/1.1"));
        assert!(request.contains("authorization: Bearer access-token"));

        json_response(
            200,
            json!({
                "data": [sync_entity_json("scene", "scene-1", 7)],
                "meta": meta_json()
            }),
        )
    });
    let client = CloudClient::new(CloudClientConfig::new(base_url).expect("base url should parse"));

    let entities = client
        .list_sync_entities("access-token", SyncEntityKind::Scene)
        .await
        .expect("sync entities should fetch");

    wait_server(task).await;
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].id, "scene-1");
    assert_eq!(entities[0].etag, Etag(7));
    assert_eq!(entities[0].kind, SyncEntityKind::Scene);
}

#[tokio::test]
async fn fetch_sync_changes_uses_since_cursor() {
    let server = OneShotServer::bind().await;
    let base_url = server.base_url();
    let task = server.spawn(|request| {
        assert!(request.starts_with("GET /v1/sync/changes?since=41 HTTP/1.1"));
        assert!(request.contains("authorization: Bearer access-token"));

        json_response(
            200,
            json!({
                "data": [{
                    "seq": 42,
                    "op": "put",
                    "entity_kind": "scene",
                    "entity_id": "scene-1",
                    "entity": sync_entity_json("scene", "scene-1", 8)
                }],
                "next_seq": 42,
                "has_more": false
            }),
        )
    });
    let client = CloudClient::new(CloudClientConfig::new(base_url).expect("base url should parse"));

    let changes = client
        .fetch_sync_changes("access-token", 41)
        .await
        .expect("sync changes should fetch");

    wait_server(task).await;
    assert_eq!(changes.next_seq, 42);
    assert!(!changes.has_more);
    assert_eq!(changes.changes[0].seq, 42);
    assert_eq!(changes.changes[0].entity_kind, SyncEntityKind::Scene);
}

#[tokio::test]
async fn put_sync_entity_sends_if_match_and_surfaces_conflict() {
    let server = OneShotServer::bind().await;
    let base_url = server.base_url();
    let task = server.spawn(|request| {
        assert!(request.starts_with("PUT /v1/sync/scenes/scene%2Fone%3Fdraft HTTP/1.1"));
        assert!(request.contains("authorization: Bearer access-token"));
        assert!(request.contains("if-match: 7"));
        assert!(request.contains(r#""schema_version":1"#));
        assert!(request.contains(r#""value""#));
        assert!(request.contains(r#""name":"Desk Glow""#));

        json_response(
            412,
            json!({
                "error": "stale",
                "current_etag": 8,
                "current": sync_entity_json("scene", "scene/one?draft", 8)
            }),
        )
    });
    let client = CloudClient::new(CloudClientConfig::new(base_url).expect("base url should parse"));
    let request = SyncPutRequest {
        schema_version: 1,
        value: json!({"name": "Desk Glow", "definition": {"groups": []}}),
    };

    let error = client
        .put_sync_entity(
            "access-token",
            SyncEntityKind::Scene,
            "scene/one?draft",
            Etag(7),
            &request,
        )
        .await
        .expect_err("stale sync write should conflict");

    wait_server(task).await;
    let CloudClientError::SyncConflict {
        current_etag,
        current,
    } = error
    else {
        panic!("expected sync conflict");
    };
    assert_eq!(current_etag, Etag(8));
    assert_eq!(
        current.expect("current entity should be present").id,
        "scene/one?draft"
    );
}

#[tokio::test]
async fn delete_sync_entity_sends_if_match_and_returns_tombstone() {
    let server = OneShotServer::bind().await;
    let base_url = server.base_url();
    let task = server.spawn(|request| {
        assert!(request.starts_with("DELETE /v1/sync/favorites/effect-xyz HTTP/1.1"));
        assert!(request.contains("authorization: Bearer access-token"));
        assert!(request.contains("if-match: 3"));

        json_response(
            200,
            json!({
                "kind": "favorite",
                "id": "effect-xyz",
                "etag": 4,
                "schema_version": 1,
                "value": {},
                "deleted_at": "2026-05-15T17:00:00Z",
                "updated_at": "2026-05-15T17:00:00Z"
            }),
        )
    });
    let client = CloudClient::new(CloudClientConfig::new(base_url).expect("base url should parse"));

    let entity = client
        .delete_sync_entity(
            "access-token",
            SyncEntityKind::Favorite,
            "effect-xyz",
            Etag(3),
        )
        .await
        .expect("delete tombstone should return entity");

    wait_server(task).await;
    assert_eq!(entity.kind, SyncEntityKind::Favorite);
    assert_eq!(entity.id, "effect-xyz");
    assert_eq!(entity.etag, Etag(4));
    assert!(entity.deleted_at.is_some());
}

#[tokio::test]
async fn delete_sync_entity_surfaces_conflict() {
    let server = OneShotServer::bind().await;
    let base_url = server.base_url();
    let task = server.spawn(|request| {
        assert!(request.starts_with("DELETE /v1/sync/scenes/scene-1 HTTP/1.1"));
        assert!(request.contains("if-match: 7"));

        json_response(
            412,
            json!({
                "error": "stale",
                "current_etag": 8,
                "current": sync_entity_json("scene", "scene-1", 8)
            }),
        )
    });
    let client = CloudClient::new(CloudClientConfig::new(base_url).expect("base url should parse"));

    let error = client
        .delete_sync_entity("access-token", SyncEntityKind::Scene, "scene-1", Etag(7))
        .await
        .expect_err("stale delete should conflict");

    wait_server(task).await;
    assert!(matches!(
        error,
        CloudClientError::SyncConflict {
            current_etag: Etag(8),
            ..
        }
    ));
}

struct OneShotServer {
    listener: tokio::net::TcpListener,
}

impl OneShotServer {
    async fn bind() -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");

        Self { listener }
    }

    fn base_url(&self) -> String {
        format!(
            "http://{}",
            self.listener
                .local_addr()
                .expect("test server address should resolve")
        )
    }

    fn spawn(
        self,
        handler: impl FnOnce(String) -> String + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let (mut socket, _) = self
                .listener
                .accept()
                .await
                .expect("request should connect");
            let mut buffer = vec![0_u8; 8192];
            let mut request = Vec::new();
            loop {
                let read = socket.read(&mut buffer).await.expect("request should read");
                assert!(read > 0, "request should not close before body is read");
                request.extend_from_slice(&buffer[..read]);
                if request_complete(&request) {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request).into_owned();
            let response = handler(request);
            socket
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
        })
    }
}

fn request_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);

    request.len() >= header_end + 4 + content_length
}

async fn wait_server(task: tokio::task::JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("test server should finish")
        .expect("test server should not panic");
}

fn json_response(status: u16, body: serde_json::Value) -> String {
    let status_text = match status {
        200 => "OK",
        412 => "Precondition Failed",
        _ => "OK",
    };
    let body = serde_json::to_string(&body).expect("response should serialize");
    format!(
        "HTTP/1.1 {status} {status_text}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn sync_entity_json(kind: &str, id: &str, etag: u64) -> serde_json::Value {
    json!({
        "kind": kind,
        "id": id,
        "etag": etag,
        "schema_version": 1,
        "value": {"name": "Desk Glow", "definition": {"groups": []}},
        "updated_at": "2026-05-15T17:00:00Z"
    })
}

fn meta_json() -> serde_json::Value {
    json!({
        "api_version": "1.0",
        "request_id": "req_01JTEST",
        "timestamp": "2026-05-15T17:00:00Z"
    })
}
