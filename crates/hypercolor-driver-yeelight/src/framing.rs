//! Bounded line framing and writes for the TCP connections (Spec 94 §4.1).
//!
//! Both directions carry one JSON object per CRLF-terminated line. Reads
//! refuse a line longer than [`MAX_LINE_BYTES`] while it streams in, and
//! writes give up after a deadline, so a misbehaving peer can neither grow
//! memory without bound nor stall the frame path.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// Longest line accepted from a device. The largest legitimate message is
/// a `props` notification or a `get_prop` result of a few hundred bytes.
pub const MAX_LINE_BYTES: usize = 4096;

/// Reads CRLF- or LF-terminated lines with a length cap.
pub struct LineReader<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
    limit: usize,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    #[must_use]
    pub fn new(reader: R) -> Self {
        Self::with_limit(reader, MAX_LINE_BYTES)
    }

    #[must_use]
    pub fn with_limit(reader: R, limit: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
            limit,
        }
    }

    /// The next line without its terminator, or `None` at end of stream.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidData`] when a line exceeds the
    /// limit or is not UTF-8, and any read error.
    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        self.line.clear();
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(None);
            }
            let (chunk, complete) = match available.iter().position(|byte| *byte == b'\n') {
                Some(end) => (&available[..=end], true),
                None => (available, false),
            };
            if self.line.len() + chunk.len() > self.limit + 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line longer than {} bytes", self.limit),
                ));
            }
            self.line.extend_from_slice(chunk);
            let consumed = chunk.len();
            self.reader.consume(consumed);
            if complete {
                while matches!(self.line.last(), Some(b'\n' | b'\r')) {
                    self.line.pop();
                }
                return String::from_utf8(std::mem::take(&mut self.line))
                    .map(Some)
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "line is not UTF-8"));
            }
        }
    }
}

/// Write all of `bytes` or fail after `deadline`. A timed-out write leaves
/// a partial line on the stream, so the caller must drop the connection.
///
/// # Errors
///
/// Returns [`io::ErrorKind::TimedOut`] at the deadline, or the write error.
pub async fn write_bounded<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    deadline: Duration,
) -> io::Result<()> {
    match tokio::time::timeout(deadline, writer.write_all(bytes)).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("write did not finish within {} ms", deadline.as_millis()),
        )),
    }
}
