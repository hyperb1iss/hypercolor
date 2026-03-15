use std::sync::{Arc, Mutex};

use hypercolor_core::device::hue::HueBridgeClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn bridge_client_pair_with_status_returns_none_when_button_not_pressed() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept request");
        let request = read_http_request(&mut stream)
            .await
            .expect("read HTTP request");
        assert!(request.starts_with("POST /api HTTP/1.1"));

        stream
            .write_all(
                json_response(
                    r#"[{"error":{"type":101,"description":"link button not pressed"}}]"#,
                )
                .as_slice(),
            )
            .await
            .expect("write HTTP response");
    });

    let client = HueBridgeClient::with_port("127.0.0.1".parse()?, port);
    assert!(client.pair_with_status("hypercolor").await?.is_none());

    server_task.await?;
    Ok(())
}

#[tokio::test]
async fn bridge_client_fetches_identity_lights_and_entertainment_configs() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let actions = Arc::new(Mutex::new(Vec::<String>::new()));
    let actions_for_server = Arc::clone(&actions);
    let config_id = "12345678-1234-1234-1234-123456789abc";
    let server_task = tokio::spawn(async move {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let request = read_http_request(&mut stream)
                .await
                .expect("read HTTP request");
            let response = if request.starts_with("GET /api/config HTTP/1.1") {
                json_response(
                    r#"{"bridgeid":"test-bridge","name":"Living Room Bridge","modelid":"BSB002","swversion":"1968096020"}"#,
                )
            } else if request.starts_with("GET /clip/v2/resource/light HTTP/1.1") {
                json_response(
                    r#"{"data":[{"id":"light-left","metadata":{"name":"Left Bulb"},"product_data":{"model_id":"LCA001"},"color":{"gamut_type":"C","gamut":{"red":{"x":0.6915,"y":0.3083},"green":{"x":0.17,"y":0.7},"blue":{"x":0.1532,"y":0.0475}}}}]}"#,
                )
            } else if request
                .starts_with("GET /clip/v2/resource/entertainment_configuration HTTP/1.1")
            {
                json_response(&format!(
                    r#"{{"data":[{{"id":"{config_id}","metadata":{{"name":"Living Room"}},"configuration_type":"screen","channels":[{{"channel_id":1,"position":{{"x":-0.5,"y":0.0,"z":0.0}},"members":[{{"service":{{"rid":"light-left","rtype":"light"}}}}]}}]}}]}}"#
                ))
            } else if request.starts_with(&format!(
                "PUT /clip/v2/resource/entertainment_configuration/{config_id} HTTP/1.1"
            )) {
                let action = if request.contains(r#""action":"start""#) {
                    "start"
                } else if request.contains(r#""action":"stop""#) {
                    "stop"
                } else {
                    "unknown"
                };
                actions_for_server
                    .lock()
                    .expect("lock actions")
                    .push(action.to_owned());
                json_response(r#"{"data":[{"success":true}]}"#)
            } else {
                not_found_response()
            };

            stream
                .write_all(response.as_slice())
                .await
                .expect("write HTTP response");
        }
    });

    let client = HueBridgeClient::authenticated_with_port(
        "127.0.0.1".parse()?,
        port,
        "test-api-key".to_owned(),
    );
    let identity = client.bridge_identity().await?;
    assert_eq!(identity.bridge_id, "test-bridge");
    assert_eq!(identity.name, "Living Room Bridge");

    let lights = client.lights().await?;
    assert_eq!(lights.len(), 1);
    assert_eq!(lights[0].name, "Left Bulb");

    let configs = client.entertainment_configs().await?;
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].name, "Living Room");
    assert_eq!(configs[0].channels.len(), 1);

    client.start_streaming(config_id).await?;
    client.stop_streaming(config_id).await?;
    assert_eq!(
        actions.lock().expect("lock actions").as_slice(),
        &["start".to_owned(), "stop".to_owned()]
    );

    server_task.await?;
    Ok(())
}

fn json_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

fn not_found_response() -> Vec<u8> {
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut buf = vec![0_u8; 4096];
    let mut total = 0_usize;
    let mut header_end = None;

    loop {
        let read = stream.read(&mut buf[total..]).await?;
        if read == 0 {
            break;
        }
        total += read;

        if header_end.is_none() {
            header_end = buf[..total]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4);
        }

        if let Some(header_end) = header_end {
            let headers = String::from_utf8_lossy(&buf[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if total >= header_end + content_length {
                break;
            }
        }

        if total == buf.len() {
            buf.resize(buf.len() * 2, 0);
        }
    }

    Ok(String::from_utf8_lossy(&buf[..total]).into_owned())
}
