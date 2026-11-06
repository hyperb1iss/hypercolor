//! Bounded line framing and writes (Spec 94 §4.1).

use std::io::ErrorKind;
use std::time::Duration;

use hypercolor_driver_yeelight::framing::{LineReader, MAX_LINE_BYTES, write_bounded};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn lines_split_on_crlf_and_lf() {
    let (mut writer, reader) = tokio::io::duplex(256);
    writer
        .write_all(b"{\"id\":1,\"result\":[\"ok\"]}\r\n{\"method\":\"props\"}\nlast")
        .await
        .expect("write");
    drop(writer);
    let mut lines = LineReader::new(reader);
    assert_eq!(
        lines.next_line().await.expect("line").as_deref(),
        Some("{\"id\":1,\"result\":[\"ok\"]}")
    );
    assert_eq!(
        lines.next_line().await.expect("line").as_deref(),
        Some("{\"method\":\"props\"}")
    );
    // An unterminated tail at end of stream is not a line.
    assert_eq!(lines.next_line().await.expect("eof"), None);
}

#[tokio::test]
async fn a_line_without_a_newline_stops_at_the_limit() {
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let flood = tokio::spawn(async move {
        let chunk = vec![b'a'; 1024];
        for _ in 0..64 {
            if writer.write_all(&chunk).await.is_err() {
                break;
            }
        }
    });
    let mut lines = LineReader::new(reader);
    let error = lines.next_line().await.expect_err("oversized line fails");
    assert_eq!(error.kind(), ErrorKind::InvalidData);
    flood.abort();
}

#[tokio::test]
async fn a_line_at_the_limit_is_accepted() {
    let (mut writer, reader) = tokio::io::duplex(16 * 1024);
    let mut line = vec![b'x'; MAX_LINE_BYTES];
    line.extend_from_slice(b"\r\n");
    writer.write_all(&line).await.expect("write");
    let mut lines = LineReader::new(reader);
    assert_eq!(
        lines
            .next_line()
            .await
            .expect("line")
            .map(|line| line.len()),
        Some(MAX_LINE_BYTES)
    );
}

#[tokio::test]
async fn writes_to_a_peer_that_stops_reading_time_out() {
    let (mut writer, _reader) = tokio::io::duplex(16);
    let error = write_bounded(&mut writer, &[b'x'; 256], Duration::from_millis(50))
        .await
        .expect_err("stalled write times out");
    assert_eq!(error.kind(), ErrorKind::TimedOut);
    write_bounded(&mut writer, b"", Duration::from_millis(50))
        .await
        .expect("an empty write completes");
}
