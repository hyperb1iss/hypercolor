use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use hypercolor_ui::api::client::{
    ApiError, HttpTransportInstallError, MutationOutcome, fetch_json, fetch_json_optional,
    head_status, install_http_transport, install_verified_daemon_connection, post_json,
    post_multipart, send_json_versioned,
};
use hypercolor_ui::api::http_transport::{
    HttpMethod, HttpMultipartPart, HttpRequest, HttpRequestBody, HttpResponse, HttpTransport,
    HttpTransportError, HttpTransportFuture,
};

struct FakeHttpTransport {
    requests: Rc<RefCell<Vec<HttpRequest>>>,
    responses: Rc<RefCell<VecDeque<Result<HttpResponse, HttpTransportError>>>>,
}

impl HttpTransport for FakeHttpTransport {
    fn send(&self, request: HttpRequest) -> HttpTransportFuture<'_> {
        self.requests.borrow_mut().push(request);
        let response = self
            .responses
            .borrow_mut()
            .pop_front()
            .expect("fake response queue exhausted");
        Box::pin(async move { response })
    }
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("fake transport future unexpectedly yielded"),
    }
}

fn response(status: u16, body: serde_json::Value) -> Result<HttpResponse, HttpTransportError> {
    Ok(HttpResponse {
        status,
        headers: Vec::new(),
        body: serde_json::to_vec(&body).expect("test response serializes"),
    })
}

#[test]
fn injected_transport_preserves_the_logical_http_contract() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let responses = Rc::new(RefCell::new(VecDeque::from([
        response(200, serde_json::json!({ "data": { "value": 1 } })),
        response(200, serde_json::json!({ "data": { "created": true } })),
        response(
            412,
            serde_json::json!({
                "error": {
                    "message": "stale",
                    "details": { "current": 17 }
                }
            }),
        ),
        response(404, serde_json::json!({ "error": { "message": "absent" } })),
        response(
            404,
            serde_json::json!({ "error": { "message": "required" } }),
        ),
        response(
            409,
            serde_json::json!({ "error": { "message": "Scene is locked" } }),
        ),
        Err(HttpTransportError {
            message: "encrypted channel closed".to_owned(),
        }),
        response(204, serde_json::Value::Null),
        response(200, serde_json::json!({ "data": { "uploaded": true } })),
    ])));
    let fake = Rc::new(FakeHttpTransport {
        requests: Rc::clone(&requests),
        responses,
    });

    install_verified_daemon_connection("http://127.0.0.1:9420", Some("local-secret"));
    install_http_transport(fake).expect("first install succeeds");
    assert_eq!(
        install_http_transport(Rc::new(FakeHttpTransport {
            requests: Rc::new(RefCell::new(Vec::new())),
            responses: Rc::new(RefCell::new(VecDeque::new())),
        })),
        Err(HttpTransportInstallError::AlreadyInstalled)
    );

    let fetched =
        ready(fetch_json::<serde_json::Value>("/api/v1/example")).expect("GET response parses");
    assert_eq!(fetched, serde_json::json!({ "value": 1 }));
    assert_eq!(
        install_http_transport(Rc::new(FakeHttpTransport {
            requests: Rc::new(RefCell::new(Vec::new())),
            responses: Rc::new(RefCell::new(VecDeque::new())),
        })),
        Err(HttpTransportInstallError::AlreadyUsed)
    );

    let created = ready(post_json::<_, serde_json::Value>(
        "/api/v1/example",
        &serde_json::json!({ "name": "prism" }),
    ))
    .expect("POST response parses");
    assert_eq!(created, serde_json::json!({ "created": true }));

    let stale = ready(send_json_versioned::<_, serde_json::Value>(
        HttpMethod::Patch,
        "/api/v1/example",
        Some(&serde_json::json!({ "enabled": true })),
        Some(16),
    ))
    .expect("412 response classifies");
    assert_eq!(stale, MutationOutcome::Stale { current: 17 });

    assert_eq!(
        ready(fetch_json_optional::<serde_json::Value>("/api/v1/optional"))
            .expect("optional 404 is not an error"),
        None
    );
    assert!(matches!(
        ready(fetch_json::<serde_json::Value>("/api/v1/required")),
        Err(ApiError::Http {
            status: 404,
            message: Some(message)
        }) if message == "required"
    ));
    assert!(matches!(
        ready(fetch_json::<serde_json::Value>("/api/v1/locked")),
        Err(ApiError::Http {
            status: 409,
            message: Some(message)
        }) if message == "Scene is locked"
    ));
    assert!(matches!(
        ready(fetch_json::<serde_json::Value>("/api/v1/network")),
        Err(ApiError::Network(message)) if message == "encrypted channel closed"
    ));
    assert_eq!(
        ready(head_status("/api/v1/example/cover")).expect("HEAD response returns status"),
        204
    );

    let upload = ready(post_multipart::<serde_json::Value>(
        "/api/v1/assets",
        vec![HttpMultipartPart {
            name: "file".to_owned(),
            file_name: Some("effect.hce".to_owned()),
            content_type: Some("application/octet-stream".to_owned()),
            body: vec![0, 1, 2, 255],
        }],
    ))
    .expect("multipart response parses");
    assert_eq!(upload, serde_json::json!({ "uploaded": true }));

    let before_invalid = requests.borrow().len();
    assert!(
        ready(fetch_json::<serde_json::Value>(
            "https://attacker.example/steal"
        ))
        .is_err()
    );
    assert!(ready(fetch_json::<serde_json::Value>("//attacker.example/steal")).is_err());
    assert_eq!(requests.borrow().len(), before_invalid);

    let requests = requests.borrow();
    assert_eq!(requests.len(), 9);
    assert_eq!(requests[0].method, HttpMethod::Get);
    assert_eq!(requests[0].path, "/api/v1/example");
    assert_eq!(requests[1].method, HttpMethod::Post);
    assert_eq!(
        requests[1].body,
        HttpRequestBody::Bytes(br#"{"name":"prism"}"#.to_vec())
    );
    assert!(requests[1].headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-type") && header.value == "application/json"
    }));
    assert!(
        requests[2]
            .headers
            .iter()
            .any(|header| { header.name.eq_ignore_ascii_case("if-match") && header.value == "16" })
    );
    assert_eq!(requests[7].method, HttpMethod::Head);
    assert_eq!(requests[7].path, "/api/v1/example/cover");
    assert_eq!(
        requests[8].body,
        HttpRequestBody::Multipart(vec![HttpMultipartPart {
            name: "file".to_owned(),
            file_name: Some("effect.hce".to_owned()),
            content_type: Some("application/octet-stream".to_owned()),
            body: vec![0, 1, 2, 255],
        }])
    );
    assert!(requests.iter().all(|request| {
        request
            .headers
            .iter()
            .all(|header| !header.name.eq_ignore_ascii_case("authorization"))
    }));
}
