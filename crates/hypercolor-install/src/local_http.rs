//! The bounded HTTP client every platform proves a local daemon with.

use std::io::{Read as _, Write as _};
#[cfg(any(unix, windows))]
use std::net::{SocketAddr, TcpStream};
#[cfg(any(unix, windows))]
use std::time::{Duration, Instant};

use super::InstallPlatformError;

/// Largest proof response body a platform reads.
pub(crate) const MAX_HTTP_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
#[cfg(any(unix, windows))]
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// One response from the daemon's local API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

fn error(detail: impl Into<String>) -> InstallPlatformError {
    InstallPlatformError::new(detail)
}

#[cfg(any(unix, windows))]
pub(crate) fn http_get(
    address: SocketAddr,
    path: &str,
    max_bytes: usize,
) -> Result<LocalHttpResponse, InstallPlatformError> {
    http_get_with_timeout(address, path, max_bytes, HTTP_TIMEOUT)
}

/// Require a healthy daemon on `/health` and `/api/v1/system` that reports
/// exactly `version`, with a non-empty instance identity.
///
/// # Errors
/// Returns an error for any other response.
pub(crate) fn verify_http(
    health: LocalHttpResponse,
    system: LocalHttpResponse,
    version: &str,
) -> Result<(), InstallPlatformError> {
    if health.status != 200
        || system.status != 200
        || health.body.len() > MAX_HTTP_RESPONSE_BYTES
        || system.body.len() > MAX_HTTP_RESPONSE_BYTES
    {
        return Err(error("bounded daemon HTTP proof failed"));
    }
    let health: serde_json::Value = serde_json::from_slice(&health.body)
        .map_err(|_| error("invalid /health identity response"))?;
    let system: serde_json::Value = serde_json::from_slice(&system.body)
        .map_err(|_| error("invalid /api/v1/system identity response"))?;
    if health.get("status").and_then(serde_json::Value::as_str) != Some("healthy")
        || health.get("version").and_then(serde_json::Value::as_str) != Some(version)
        || system
            .pointer("/data/identity/version")
            .and_then(serde_json::Value::as_str)
            != Some(version)
        || system
            .pointer("/data/identity/instance_id")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        || system
            .pointer("/data/identity/instance_name")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(error(
            "daemon HTTP identity does not match the immutable unit",
        ));
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn http_get_with_timeout(
    address: SocketAddr,
    path: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<LocalHttpResponse, InstallPlatformError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| error("HTTP owner proof deadline overflowed"))?;
    let mut stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?).map_err(io_error)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    write_deadline(&mut stream, request.as_bytes(), deadline)?;
    let mut response = read_headers(&mut stream, deadline)?;
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| error("HTTP owner proof response lacks a header boundary"))?;
    let (status, declared) = parse_headers(&response[..separator], max_bytes)?;
    let body_start = separator + 4;
    if response.len() - body_start > declared {
        return Err(error("HTTP owner proof response has trailing framing data"));
    }
    let missing = declared - (response.len() - body_start);
    if missing != 0 {
        let initial = response.len();
        response.resize(initial + missing, 0);
        read_exact_deadline(&mut stream, &mut response[initial..], deadline)?;
    }
    reject_buffered_trailing(&stream)?;
    Ok(LocalHttpResponse {
        status,
        body: response[body_start..].to_vec(),
    })
}

#[cfg(any(unix, windows))]
fn write_deadline(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> Result<(), InstallPlatformError> {
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        let written = stream.write(bytes).map_err(io_error)?;
        if written == 0 {
            return Err(error("HTTP owner proof request write made no progress"));
        }
        bytes = &bytes[written..];
    }
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(io_error)?;
    stream.flush().map_err(io_error)
}

#[cfg(any(unix, windows))]
fn read_headers(
    stream: &mut TcpStream,
    deadline: Instant,
) -> Result<Vec<u8>, InstallPlatformError> {
    let mut response = Vec::new();
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        if response.len() >= MAX_HTTP_HEADER_BYTES + 4 {
            return Err(error("HTTP owner proof headers exceed their byte bound"));
        }
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).map_err(io_error)?;
        if read == 0 {
            return Err(error("HTTP owner proof response ended before its headers"));
        }
        response.extend_from_slice(&chunk[..read]);
    }
    Ok(response)
}

#[cfg(any(unix, windows))]
fn read_exact_deadline(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), InstallPlatformError> {
    while !bytes.is_empty() {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        let read = stream.read(bytes).map_err(io_error)?;
        if read == 0 {
            return Err(error("HTTP owner proof body ended before Content-Length"));
        }
        bytes = &mut bytes[read..];
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn reject_buffered_trailing(stream: &TcpStream) -> Result<(), InstallPlatformError> {
    stream.set_nonblocking(true).map_err(io_error)?;
    let mut byte = [0_u8; 1];
    match stream.peek(&mut byte) {
        Ok(0) => Ok(()),
        Ok(_) => Err(error("HTTP owner proof response has trailing framing data")),
        Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        Err(source) => Err(io_error(source)),
    }
}

#[cfg(any(unix, windows))]
fn parse_headers(headers: &[u8], max_bytes: usize) -> Result<(u16, usize), InstallPlatformError> {
    if headers.len() > MAX_HTTP_HEADER_BYTES {
        return Err(error("HTTP owner proof headers exceed their byte bound"));
    }
    let headers = std::str::from_utf8(headers)
        .map_err(|_| error("HTTP owner proof headers are not UTF-8"))?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.strip_prefix("HTTP/1.1 "))
        .and_then(|line| line.split_once(' '))
        .and_then(|(status, _)| status.parse::<u16>().ok())
        .ok_or_else(|| error("HTTP owner proof status line is malformed"))?;
    let mut content_length = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| error("HTTP owner proof header is malformed"))?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(error("chunked HTTP owner proof responses are unsupported"));
        }
        if name.eq_ignore_ascii_case("content-length")
            && content_length.replace(value.trim()).is_some()
        {
            return Err(error("duplicate HTTP Content-Length header"));
        }
    }
    let declared = content_length
        .ok_or_else(|| error("HTTP owner proof response lacks Content-Length"))?
        .parse::<usize>()
        .map_err(|_| error("HTTP Content-Length is malformed"))?;
    if declared > max_bytes {
        return Err(error("HTTP owner proof body exceeds its byte bound"));
    }
    Ok((status, declared))
}

#[cfg(any(unix, windows))]
fn remaining(deadline: Instant) -> Result<Duration, InstallPlatformError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| error("HTTP owner proof exceeded its absolute deadline"))
}

#[cfg(any(unix, windows))]
fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    use serde_json::json;

    use super::{Duration, Instant, LocalHttpResponse, http_get_with_timeout, verify_http};

    fn response(value: serde_json::Value) -> LocalHttpResponse {
        LocalHttpResponse {
            status: 200,
            body: serde_json::to_vec(&value).expect("serialize identity response fixture"),
        }
    }

    #[test]
    fn owner_proof_uses_the_system_resources_nested_public_identity() {
        let health = response(json!({"status":"healthy", "version":"0.4.0"}));
        let system = response(json!({"data":{"identity":{
            "version":"0.4.0", "instance_id":"daemon-id", "instance_name":"lights"
        },"status":null}}));
        verify_http(health, system, "0.4.0")
            .expect("public system identity without privileged status");
    }

    #[test]
    fn owner_proof_rejects_retired_flat_missing_and_mismatched_system_identity() {
        for system in [
            json!({"data":{"version":"0.4.0", "instance_id":"id", "instance_name":"name"}}),
            json!({"data":{"status":{}}}),
            json!({"data":{"identity":{"version":"0.3.0", "instance_id":"id", "instance_name":"name"}}}),
            json!({"data":{"identity":{"version":"0.4.0", "instance_id":"", "instance_name":"name"}}}),
            json!({"data":{"identity":{"version":"0.4.0", "instance_id":"id", "instance_name":""}}}),
        ] {
            assert!(
                verify_http(
                    response(json!({"status":"healthy", "version":"0.4.0"})),
                    response(system),
                    "0.4.0"
                )
                .is_err()
            );
        }
    }

    #[test]
    fn keep_alive_response_finishes_at_exact_content_length() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let response =
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n{}";
            stream.write_all(response).expect("response");
            thread::sleep(Duration::from_millis(250));
        });
        let started = Instant::now();
        let response = http_get_with_timeout(address, "/health", 16, Duration::from_secs(1))
            .expect("bounded response");
        assert_eq!(response.body, b"{}");
        assert!(started.elapsed() < Duration::from_millis(200));
        server.join().expect("server");
    }

    #[test]
    fn slow_trickle_cannot_reset_the_absolute_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}" {
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        assert!(http_get_with_timeout(address, "/health", 16, Duration::from_millis(100)).is_err());
        assert!(started.elapsed() < Duration::from_millis(300));
        server.join().expect("server");
    }
}
