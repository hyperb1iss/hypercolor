use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use hypercolor_ui::api::client::{
    ApiError, Expired, Fence, HttpTransportInstallError, MutationOutcome, fetch_json,
    fetch_json_optional, head_status, install_http_transport, install_verified_daemon_connection,
    post_json, post_multipart, send_json_keyed, send_json_versioned, within,
};
use hypercolor_ui::api::http_transport::{
    HttpHeader, HttpMethod, HttpMultipartPart, HttpRequest, HttpRequestBody, HttpResponse,
    HttpTransport, HttpTransportError, HttpTransportFuture,
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

fn ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
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

fn meta() -> serde_json::Value {
    serde_json::json!({
        "api_version": "1.0",
        "request_id": "req_transport_test",
        "timestamp": "1970-01-01T00:00:00Z"
    })
}

/// The daemon's canonical success envelope, which the transport must carry
/// through untouched.
fn data_response(status: u16, data: serde_json::Value) -> Result<HttpResponse, HttpTransportError> {
    response(status, serde_json::json!({ "data": data, "meta": meta() }))
}

/// The daemon's canonical error envelope, including the stable machine
/// code and the structured details a stale mutation rebases on.
fn error_response(
    status: u16,
    code: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> Result<HttpResponse, HttpTransportError> {
    let mut error = serde_json::json!({ "code": code, "message": message });
    if let Some(details) = details {
        error["details"] = details;
    }
    response(
        status,
        serde_json::json!({ "error": error, "meta": meta() }),
    )
}

#[test]
fn injected_transport_preserves_the_logical_http_contract() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let responses = Rc::new(RefCell::new(VecDeque::from([
        data_response(200, serde_json::json!({ "value": 1 })),
        data_response(200, serde_json::json!({ "created": true })),
        error_response(
            412,
            "precondition_failed",
            "stale",
            Some(serde_json::json!({ "expected": 16, "current": 17 })),
        ),
        error_response(404, "not_found", "absent", None),
        error_response(404, "not_found", "required", None),
        error_response(409, "scene_locked", "Scene is locked", None),
        Err(HttpTransportError {
            message: "encrypted channel closed".to_owned(),
        }),
        response(204, serde_json::Value::Null),
        data_response(200, serde_json::json!({ "uploaded": true })),
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
    let required = ready(fetch_json::<serde_json::Value>("/api/v1/required"))
        .expect_err("404 without an optional wrapper is an error");
    assert!(matches!(
        &required,
        ApiError::Http {
            status: 404,
            message: Some(message),
            ..
        } if message == "required"
    ));
    assert_eq!(required.code(), Some("not_found"));
    let locked =
        ready(fetch_json::<serde_json::Value>("/api/v1/locked")).expect_err("409 is an error");
    assert!(matches!(
        &locked,
        ApiError::Http {
            status: 409,
            message: Some(message),
            ..
        } if message == "Scene is locked"
    ));
    assert_eq!(locked.code(), Some("scene_locked"));
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
    assert!(ready(fetch_json::<serde_json::Value>("/\\attacker.example/steal")).is_err());
    assert!(
        ready(fetch_json::<serde_json::Value>(
            "/\n/attacker.example/steal"
        ))
        .is_err()
    );
    assert!(
        ready(fetch_json::<serde_json::Value>(
            "/\t/attacker.example/steal"
        ))
        .is_err()
    );
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

#[test]
fn a_keyed_write_goes_out_again_exactly_as_it_first_went() {
    let requests = Rc::new(RefCell::new(Vec::new()));
    let mut answered =
        data_response(200, serde_json::json!({ "id": "zone" })).expect("a test response");
    answered.headers = vec![
        HttpHeader {
            name: "Hypercolor-Run-Epoch".to_owned(),
            value: "run-a".to_owned(),
        },
        HttpHeader {
            name: "ETag".to_owned(),
            value: "\"8\"".to_owned(),
        },
    ];
    let responses = Rc::new(RefCell::new(VecDeque::from([
        Err(HttpTransportError {
            message: "connection reset".to_owned(),
        }),
        Ok(answered),
    ])));
    install_verified_daemon_connection("http://127.0.0.1:9420", None);
    install_http_transport(Rc::new(FakeHttpTransport {
        requests: Rc::clone(&requests),
        responses,
    }))
    .expect("first install succeeds");

    let body = serde_json::json!({ "placements": [{ "member": "a", "position": { "x": 0.25, "y": 0.5 } }] });
    let fence = Fence {
        revision: 7,
        run: "run-a".to_owned(),
    };
    let send = || {
        send_json_keyed::<_, serde_json::Value>(
            HttpMethod::Put,
            "/api/v1/scene/zones/zone/layout",
            Some(&body),
            Some(&fence),
            "write-key",
        )
    };
    let lost = ready(send());
    assert_eq!(lost.run, None, "no response names no run");
    assert_eq!(lost.status, None);
    let answer = ready(send());
    assert_eq!(answer.run.as_deref(), Some("run-a"));
    assert_eq!(answer.status, Some(200));
    assert!(matches!(
        answer.outcome,
        Ok(MutationOutcome::Applied((_, Some(8))))
    ));

    // The daemon fingerprints method, path, If-Match, Content-Type and body,
    // so the retry must be the first request again, byte for byte.
    let requests = requests.borrow();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
    let header = |name: &str| {
        requests[0]
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.clone())
    };
    assert_eq!(header("idempotency-key").as_deref(), Some("write-key"));
    assert_eq!(header("If-Match"), Some(fence.if_match()));
    assert_eq!(fence.if_match(), "7", "a fence is spelled by its revision");
    assert_eq!(header("Content-Type").as_deref(), Some("application/json"));
}

#[test]
fn a_deadline_gives_up_on_work_that_never_finishes() {
    assert_eq!(
        ready(within(std::future::pending::<u8>(), std::future::ready(()))),
        Err(Expired)
    );
    assert_eq!(
        ready(within(std::future::ready(3_u8), std::future::pending())),
        Ok(3)
    );
}
