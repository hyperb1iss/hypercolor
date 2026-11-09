//! SSE subscriptions bound each event without expiring a quiet stream.

use std::time::Duration;

use hypercolor_driver_support::http::{BoundedHttpClient, EventStream, HttpError, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const TIMEOUT: Duration = Duration::from_millis(200);
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn stream_with_body(
    body: &'static [u8],
    max_bytes: usize,
) -> TestResult<(EventStream, oneshot::Sender<()>, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (close, closed) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept SSE client");
        read_request(&mut socket).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("response headers");
        socket.write_all(body).await.expect("response body");
        let _ = closed.await;
    });
    let stream = BoundedHttpClient::new()?
        .get(format!("http://{address}/events"))
        .subscribe(max_bytes, TIMEOUT)
        .await?;
    Ok((stream, close, server))
}

async fn read_request(socket: &mut TcpStream) {
    let mut buffer = [0; 1024];
    let mut read = 0;
    while !buffer[..read]
        .windows(4)
        .any(|window| window == b"\r\n\r\n")
    {
        read += socket
            .read(&mut buffer[read..])
            .await
            .expect("read request");
    }
}

#[tokio::test]
async fn events_do_not_share_one_body_cap_and_all_sse_line_endings_finish_events() -> TestResult {
    let (mut stream, close, server) =
        stream_with_body(b"data: a\n\ndata: b\r\n\r\ndata: c\r\r", 12).await?;
    assert_eq!(stream.status(), StatusCode::OK);
    close.send(()).expect("close SSE peer");
    let mut chunks = 0;
    while stream.receive().await? {
        chunks += 1;
    }
    assert!(chunks > 0);
    server.await?;
    Ok(())
}

#[tokio::test]
async fn quiet_stream_has_no_idle_deadline_and_cancelling_receive_preserves_it() -> TestResult {
    let (mut stream, close, server) = stream_with_body(b"", 32).await?;
    assert!(
        tokio::time::timeout(TIMEOUT * 2, stream.receive())
            .await
            .is_err()
    );
    close.send(()).expect("close SSE peer");
    assert!(!stream.receive().await?);
    server.await?;
    Ok(())
}

#[tokio::test]
async fn oversized_event_fails_without_collecting_the_payload() -> TestResult {
    let (mut stream, close, server) = stream_with_body(b"data: oversized-event\n\n", 8).await?;
    assert!(matches!(
        stream.receive().await,
        Err(HttpError::BodyTooLarge { limit: 8 })
    ));
    close.send(()).expect("close SSE peer");
    server.await?;
    Ok(())
}

#[tokio::test]
async fn partial_event_has_a_fixed_deadline() -> TestResult {
    let (mut stream, close, server) = stream_with_body(b"data: partial", 32).await?;
    assert!(stream.receive().await?);
    assert!(matches!(
        stream.receive().await,
        Err(HttpError::DeadlineExceeded)
    ));
    close.send(()).expect("close SSE peer");
    server.await?;
    Ok(())
}

#[tokio::test]
async fn waiting_for_headers_is_bounded() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept SSE client");
        read_request(&mut socket).await;
        let mut byte = [0];
        let _ = socket.read(&mut byte).await;
    });
    let result = BoundedHttpClient::new()?
        .get(format!("http://{address}/secret-token/events"))
        .subscribe(32, TIMEOUT)
        .await;
    assert!(matches!(result, Err(HttpError::DeadlineExceeded)));
    server.await?;
    Ok(())
}

#[tokio::test]
async fn subscription_transport_errors_hide_the_request_url() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    drop(listener);
    let error = BoundedHttpClient::new()?
        .get(format!("http://{address}/secret-token/events"))
        .subscribe(32, TIMEOUT)
        .await
        .expect_err("connection is refused");
    assert!(!format!("{error} {error:?}").contains("secret-token"));
    Ok(())
}

#[tokio::test]
async fn cancelled_receive_keeps_partial_event_size_and_deadline() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (next, next_chunk) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept SSE client");
        read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n7\r\ndata: a\r\n").await.expect("first partial event");
        next_chunk.await.expect("send remaining payload");
        socket
            .write_all(b"5\r\nbbbb\n\r\n0\r\n\r\n")
            .await
            .expect("remaining event");
    });
    let mut stream = BoundedHttpClient::new()?
        .get(format!("http://{address}/events"))
        .subscribe(10, Duration::from_secs(2))
        .await?;
    assert!(stream.receive().await?);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), stream.receive())
            .await
            .is_err()
    );
    next.send(()).expect("release second chunk");
    assert!(matches!(
        stream.receive().await,
        Err(HttpError::BodyTooLarge { limit: 10 })
    ));
    server.await?;
    Ok(())
}

#[tokio::test]
async fn event_cap_counts_every_crlf_byte_including_the_terminator() -> TestResult {
    const LF: &[u8] = b"data:a\ndata:b\n\n";
    const CRLF: &[u8] = b"data:a\r\ndata:b\r\n\r\n";
    for (body, cap, accepted) in [
        (LF, LF.len(), true),
        (CRLF, CRLF.len() - 1, false),
        (CRLF, CRLF.len(), true),
    ] {
        let (mut stream, close, server) = stream_with_body(body, cap).await?;
        close.send(()).expect("close SSE peer");
        let mut exceeded = false;
        loop {
            match stream.receive().await {
                Ok(true) => {}
                Ok(false) => break,
                Err(HttpError::BodyTooLarge { limit }) => {
                    assert_eq!(limit, cap);
                    exceeded = true;
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        assert_eq!(!exceeded, accepted);
        server.await?;
    }
    Ok(())
}

#[tokio::test]
async fn split_terminal_crlf_counts_lf_without_starting_an_idle_deadline() -> TestResult {
    const PREFIX: &[u8] = b"data:a\r\ndata:b\r\n\r";
    for cap in [PREFIX.len(), PREFIX.len() + 1] {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (next, next_chunk) = oneshot::channel();
        let (close, closed) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept SSE client");
            read_request(&mut socket).await;
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                PREFIX.len()
            );
            socket.write_all(headers.as_bytes()).await.expect("headers");
            socket.write_all(PREFIX).await.expect("event prefix");
            socket.write_all(b"\r\n").await.expect("chunk boundary");
            next_chunk.await.expect("send terminal LF");
            socket.write_all(b"1\r\n\n\r\n").await.expect("terminal LF");
            let _ = closed.await;
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        let mut stream = BoundedHttpClient::new()?
            .get(format!("http://{address}/events"))
            .subscribe(cap, TIMEOUT)
            .await?;
        assert!(stream.receive().await?);
        assert!(
            tokio::time::timeout(TIMEOUT * 2, stream.receive())
                .await
                .is_err()
        );
        next.send(()).expect("send terminal LF");
        if cap == PREFIX.len() {
            assert!(
                matches!(stream.receive().await, Err(HttpError::BodyTooLarge { limit }) if limit == cap)
            );
        } else {
            assert!(stream.receive().await?);
            assert!(
                tokio::time::timeout(TIMEOUT * 2, stream.receive())
                    .await
                    .is_err()
            );
        }
        close.send(()).expect("close SSE peer");
        server.await?;
    }
    Ok(())
}
