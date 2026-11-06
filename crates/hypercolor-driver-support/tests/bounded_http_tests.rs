//! The read budget: oversized bodies and slow peers fail at the bound,
//! whatever `Content-Length` says.

use std::future::Future;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use hypercolor_driver_support::http::{BoundedHttpClient, HttpError, ReadBudget, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const HEAD: &[u8] = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
const LONG: Duration = Duration::from_secs(5);

/// Serve one connection with `reply`, after reading the request.
async fn serve<F, R>(reply: F) -> TestResult<String>
where
    F: FnOnce(TcpStream) -> R + Send + 'static,
    R: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await;
            reply(stream).await;
        }
    });
    Ok(format!("http://127.0.0.1:{port}/"))
}

/// Serve one connection with a canned response.
async fn serve_bytes(response: Vec<u8>) -> TestResult<String> {
    serve(move |mut stream| async move {
        let _ = stream.write_all(&response).await;
        let _ = stream.shutdown().await;
    })
    .await
}

fn close_delimited(body: &[u8]) -> Vec<u8> {
    [HEAD, body].concat()
}

async fn read(url: &str, budget: ReadBudget) -> Result<Vec<u8>, HttpError> {
    BoundedHttpClient::new()?
        .get(url)
        .send(budget)
        .await?
        .bytes()
        .await
}

#[tokio::test]
async fn a_body_at_the_cap_reads_whole() -> TestResult {
    let url = serve_bytes(close_delimited(&[b'x'; 1024])).await?;
    let body = read(&url, ReadBudget::new(1024, LONG)).await?;
    assert_eq!(body.len(), 1024);
    Ok(())
}

#[tokio::test]
async fn a_close_delimited_body_one_byte_over_the_cap_is_refused() -> TestResult {
    let url = serve_bytes(close_delimited(&[b'x'; 1025])).await?;
    let error = read(&url, ReadBudget::new(1024, LONG))
        .await
        .expect_err("oversized body");
    assert!(matches!(error, HttpError::BodyTooLarge { limit: 1024 }));
    Ok(())
}

#[tokio::test]
async fn a_chunked_body_is_counted_as_it_arrives() -> TestResult {
    let chunk = [b"400\r\n".as_slice(), &[b'x'; 1024], b"\r\n"].concat();
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    for _ in 0..8 {
        response.extend_from_slice(&chunk);
    }
    response.extend_from_slice(b"0\r\n\r\n");
    let url = serve_bytes(response).await?;
    let error = read(&url, ReadBudget::new(4096, LONG))
        .await
        .expect_err("oversized chunked body");
    assert!(matches!(error, HttpError::BodyTooLarge { limit: 4096 }));
    Ok(())
}

#[tokio::test]
async fn a_declared_length_over_the_cap_is_refused_before_any_body_arrives() -> TestResult {
    let url = serve(|mut stream| async move {
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\n\r\n";
        let _ = stream.write_all(head).await;
        tokio::time::sleep(LONG * 2).await;
    })
    .await?;
    let started = Instant::now();
    let error = read(&url, ReadBudget::new(1024, LONG))
        .await
        .expect_err("declared length over the cap");
    assert!(matches!(error, HttpError::BodyTooLarge { .. }));
    assert!(started.elapsed() < LONG, "refused without waiting");
    Ok(())
}

#[tokio::test]
async fn a_slow_drip_cannot_extend_the_deadline() -> TestResult {
    let url = serve(|mut stream| async move {
        let _ = stream.write_all(HEAD).await;
        // Always making progress, never finishing.
        while stream.write_all(b"x").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let started = Instant::now();
    let error = read(
        &url,
        ReadBudget::new(1024 * 1024, Duration::from_millis(300)),
    )
    .await
    .expect_err("dripped body");
    assert!(matches!(error, HttpError::DeadlineExceeded), "{error}");
    let elapsed = started.elapsed();
    assert!(
        (Duration::from_millis(250)..Duration::from_secs(2)).contains(&elapsed),
        "the deadline is absolute: {elapsed:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_peer_that_never_answers_fails_at_the_deadline() -> TestResult {
    let url = serve(|stream| async move {
        tokio::time::sleep(LONG * 2).await;
        drop(stream);
    })
    .await?;
    let started = Instant::now();
    let error = read(&url, ReadBudget::new(1024, Duration::from_millis(200)))
        .await
        .expect_err("silent peer");
    assert!(matches!(error, HttpError::DeadlineExceeded), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[tokio::test]
async fn the_clock_runs_from_the_budget_not_from_the_response() -> TestResult {
    let url = serve(|mut stream| async move {
        let _ = stream.write_all(HEAD).await;
        tokio::time::sleep(LONG * 2).await;
    })
    .await?;
    let budget = ReadBudget::new(1024, Duration::from_millis(250));
    let response = BoundedHttpClient::new()?.get(url).send(budget).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    let error = response.bytes().await.expect_err("expired budget");
    assert!(matches!(error, HttpError::DeadlineExceeded), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "nothing is left to wait for"
    );
    Ok(())
}

#[tokio::test]
async fn statuses_stay_with_the_caller_until_it_asks_for_an_error() -> TestResult {
    let url = serve_bytes(
        b"HTTP/1.1 503 Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    )
    .await?;
    let response = BoundedHttpClient::new()?
        .get(url)
        .send(ReadBudget::new(1024, LONG))
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error = response.error_for_status().expect_err("503 is an error");
    assert!(matches!(
        error,
        HttpError::Status(StatusCode::SERVICE_UNAVAILABLE)
    ));
    Ok(())
}

#[tokio::test]
async fn json_decodes_inside_the_budget() -> TestResult {
    let url = serve_bytes(close_delimited(br#"{"ok":true}"#)).await?;
    let value: serde_json::Value = BoundedHttpClient::new()?
        .get(url)
        .send(ReadBudget::new(1024, LONG))
        .await?
        .json()
        .await?;
    assert_eq!(value["ok"], true);

    let url = serve_bytes(close_delimited(b"not json")).await?;
    let error = BoundedHttpClient::new()?
        .get(url)
        .send(ReadBudget::new(1024, LONG))
        .await?
        .json::<serde_json::Value>()
        .await
        .expect_err("malformed JSON");
    assert!(matches!(error, HttpError::Json(_)));
    Ok(())
}

#[tokio::test]
async fn no_redirects_answers_with_the_redirect_itself() -> TestResult {
    let url = serve_bytes(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
    )
    .await?;
    let response = BoundedHttpClient::builder()
        .no_redirects()
        .no_proxy()
        .build()?
        .get(url)
        .send(ReadBudget::new(1024, LONG))
        .await?;
    assert_eq!(response.status(), StatusCode::FOUND);
    Ok(())
}
