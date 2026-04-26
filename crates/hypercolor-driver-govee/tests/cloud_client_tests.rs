use hypercolor_driver_govee::cloud::CloudClient;
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
    let (base_url, _request) =
        serve_once(200, r#"{"code":400,"message":"InvalidParameter","data":{}}"#).await;
    let client = CloudClient::with_base_url("test-key", base_url).expect("base URL should parse");

    let error = client
        .list_v1_devices()
        .await
        .expect_err("API code 400 should fail");

    assert!(error.to_string().contains("Govee API returned code 400"));
}

async fn serve_once(
    status: u16,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<String>) {
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
        let mut buf = [0_u8; 4096];
        let len = stream
            .read(&mut buf)
            .await
            .expect("test HTTP request should read");
        let request = String::from_utf8(buf[..len].to_vec()).expect("request should be UTF-8");
        let response = http_response(status, body);
        stream
            .write_all(response.as_bytes())
            .await
            .expect("test HTTP response should write");
        request
    });

    (format!("http://{address}/v1"), task)
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
