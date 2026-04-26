use hypercolor_driver_govee::cloud::{CloudClient, V1Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::test]
async fn list_v1_devices_sends_api_key_and_parses_devices() {
    let body = r#"{
        "code": 200,
        "message": "Success",
        "data": {
            "devices": [{
                "device": "99:E5:A4:C1:38:29:DA:7B",
                "model": "H6159",
                "deviceName": "Desk Strip",
                "controllable": true,
                "retrievable": true,
                "supportCmds": ["turn", "brightness", "color", "colorTem"],
                "properties": { "colorTem": { "range": { "min": 2000, "max": 9000 } } }
            }]
        }
    }"#;
    let (base_url, request) = serve_once(200, body).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");

    let devices = client
        .list_v1_devices()
        .await
        .expect("device-list response should parse");

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].model, "H6159");
    assert_eq!(devices[0].device_name, "Desk Strip");
    assert_eq!(
        devices[0].support_cmds,
        ["turn", "brightness", "color", "colorTem"]
    );

    let request = request.await.expect("server task should join");
    assert!(request.starts_with("GET /v1/devices HTTP/1.1"));
    assert_header_present(&request, "govee-api-key", "test-key");
}

#[tokio::test]
async fn list_v1_devices_rejects_invalid_api_key_status() {
    let (base_url, request) = serve_once(403, r#"{"message":"AuthorizationError"}"#).await;
    let client = CloudClient::with_base_url("bad-key", base_url).expect("base URL should parse");

    let error = client
        .list_v1_devices()
        .await
        .expect_err("403 should reject API key");

    assert!(error.to_string().contains("rejected the API key"));
    let request = request.await.expect("server task should join");
    assert_header_present(&request, "govee-api-key", "bad-key");
}

#[tokio::test]
async fn list_v1_devices_rejects_non_success_api_code() {
    let (base_url, _request) = serve_once(
        200,
        r#"{"code":400,"message":"InvalidParameter","data":{}}"#,
    )
    .await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");

    let error = client
        .list_v1_devices()
        .await
        .expect_err("API code 400 should fail");

    assert!(error.to_string().contains("Govee API returned code 400"));
}

#[tokio::test]
async fn v1_state_sends_device_query_and_parses_properties() {
    let body = r#"{
        "code": 200,
        "message": "Success",
        "data": {
            "device": "99:E5:A4:C1:38:29:DA:7B",
            "model": "H6159",
            "properties": [
                { "online": "true" },
                { "brightness": 82 },
                { "color": { "r": 255, "g": 64, "b": 0 } }
            ]
        }
    }"#;
    let (base_url, request) = serve_once(200, body).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");

    let state = client
        .v1_state("H6159", "99:E5:A4:C1:38:29:DA:7B")
        .await
        .expect("state response should parse");

    assert_eq!(state.model, "H6159");
    assert_eq!(state.properties.len(), 3);
    let request = request.await.expect("server task should join");
    assert!(request.starts_with("GET /v1/devices/state?"));
    assert!(request.contains("device=99%3AE5%3AA4%3AC1%3A38%3A29%3ADA%3A7B"));
    assert!(request.contains("model=H6159"));
    assert_header_present(&request, "govee-api-key", "test-key");
}

#[tokio::test]
async fn v1_control_sends_command_body() {
    let (base_url, request) =
        serve_once(200, r#"{"code":200,"message":"Success","data":{}}"#).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");

    client
        .v1_control(
            "H6159",
            "99:E5:A4:C1:38:29:DA:7B",
            V1Command::Color {
                r: 255,
                g: 64,
                b: 0,
            },
        )
        .await
        .expect("control response should parse");

    let request = request.await.expect("server task should join");
    assert!(request.starts_with("PUT /v1/devices/control HTTP/1.1"));
    assert_header_present(&request, "govee-api-key", "test-key");
    let body = request_json_body(&request);
    assert_eq!(body["device"], "99:E5:A4:C1:38:29:DA:7B");
    assert_eq!(body["model"], "H6159");
    assert_eq!(body["cmd"]["name"], "color");
    assert_eq!(
        body["cmd"]["value"],
        serde_json::json!({ "r": 255, "g": 64, "b": 0 })
    );
}

async fn serve_once(status: u16, body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("test HTTP listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have local address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .expect("test HTTP connection should arrive");
        let request = read_http_request(&mut stream)
            .await
            .expect("test HTTP request should read");
        let response = http_response(status, body);
        stream
            .write_all(response.as_bytes())
            .await
            .expect("test HTTP response should write");
        request
    });

    (format!("http://{address}/v1"), task)
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> std::io::Result<String> {
    let mut request = Vec::new();
    let mut buf = [0_u8; 4096];
    loop {
        let len = stream.read(&mut buf).await?;
        if len == 0 {
            break;
        }
        request.extend_from_slice(&buf[..len]);
        if request_complete(&request) {
            break;
        }
    }

    Ok(String::from_utf8(request).expect("request should be UTF-8"))
}

fn request_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
        .or_else(|| {
            headers
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    request.len() >= header_end + 4 + content_length
}

fn http_response(status: u16, body: &str) -> String {
    let reason = match status {
        200 => "OK",
        403 => "Forbidden",
        _ => "Test",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn assert_header_present(request: &str, name: &str, value: &str) {
    let expected = format!("{name}: {value}");
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&expected)),
        "missing expected header {expected:?} in request:\n{request}"
    );
}

fn request_json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("HTTP request should contain a header/body separator");
    serde_json::from_str(body).expect("HTTP request body should be JSON")
}
