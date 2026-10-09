use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use super::{BlocksConnection, MAX_REPLY_LINE};

#[tokio::test]
async fn an_oversized_reply_fails_its_request_and_the_next_request_resyncs() {
    let dir = tempdir().expect("socket directory");
    let path = dir.path().join("blocksd.sock");
    let listener = UnixListener::bind(&path).expect("fake blocksd binds");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut reader = BufReader::new(stream);
        let mut request = String::new();
        // A valid pong both times, padded past the cap the first time.
        for padding in [MAX_REPLY_LINE, 0] {
            request.clear();
            reader.read_line(&mut request).await?;
            assert_eq!(request, "{\"type\":\"ping\",\"id\":\"hc\"}\n");
            let pong = serde_json::json!({
                "type": "pong",
                "version": "0.5.0",
                "uptime_seconds": 1,
                "device_count": 0,
                "pad": "x".repeat(padding),
            });
            reader
                .get_mut()
                .write_all((pong.to_string() + "\n").as_bytes())
                .await?;
        }
        Ok::<(), std::io::Error>(())
    });

    let mut connection = BlocksConnection::connect(&path)
        .await
        .expect("connects to fake blocksd");
    let error = connection
        .ping()
        .await
        .expect_err("an oversized reply fails its request");
    assert!(
        error.to_string().contains("exceeded"),
        "the error names the cap: {error:#}"
    );
    let pong = connection
        .ping()
        .await
        .expect("the next request reads its own reply");
    assert_eq!(pong.version, "0.5.0");
    server
        .await
        .expect("fake blocksd task completes")
        .expect("fake blocksd I/O succeeds");
}
