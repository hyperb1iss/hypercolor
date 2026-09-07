use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::super::InstallPlatformError;
use super::model::{LinuxHttpResponse, error};

const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn http_get(
    address: SocketAddr,
    path: &str,
    max_bytes: usize,
) -> Result<LinuxHttpResponse, InstallPlatformError> {
    http_get_with_timeout(address, path, max_bytes, HTTP_TIMEOUT)
}

fn http_get_with_timeout(
    address: SocketAddr,
    path: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<LinuxHttpResponse, InstallPlatformError> {
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
    Ok(LinuxHttpResponse {
        status,
        body: response[body_start..].to_vec(),
    })
}

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

fn remaining(deadline: Instant) -> Result<Duration, InstallPlatformError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| error("HTTP owner proof exceeded its absolute deadline"))
}

fn io_error(source: std::io::Error) -> InstallPlatformError {
    error(source.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread;

    use super::{Duration, Instant, http_get_with_timeout};

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
