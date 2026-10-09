//! Newline framing with a length cap for blocksd's NDJSON connections.

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// What [`read_line_capped`] found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LineRead {
    /// The line is in the buffer, without its newline. A last line the peer
    /// closed the stream on without terminating also counts.
    Line,
    /// The line ran past the limit and was discarded through its newline,
    /// so the reader sits at the start of the next line. The buffer is
    /// empty.
    Oversized,
    /// The peer closed the stream before sending another byte.
    Closed,
}

/// Read the next newline-delimited line into `line`, buffering at most
/// `limit` bytes of it.
///
/// A longer line is skipped through its newline without being buffered, so
/// a peer that never sends a newline cannot grow `line` without bound, and
/// the stream stays framed for the line after it.
///
/// The read is not cancel safe: bytes consumed before the future is dropped
/// are gone and the reader is left mid-line, so a caller that abandons a
/// read, on a timeout say, must abandon the connection with it.
///
/// # Errors
///
/// Fails when the underlying read fails.
pub(super) async fn read_line_capped<R>(
    reader: &mut R,
    line: &mut Vec<u8>,
    limit: usize,
) -> io::Result<LineRead>
where
    R: AsyncBufRead + Unpin,
{
    line.clear();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() && !oversized {
                return Ok(LineRead::Closed);
            }
            break;
        }
        let newline = available.iter().position(|&byte| byte == b'\n');
        let content = newline.unwrap_or(available.len());
        if !oversized {
            if line.len() + content > limit {
                oversized = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..content]);
            }
        }
        reader.consume(newline.map_or(content, |at| at + 1));
        if newline.is_some() {
            break;
        }
    }
    Ok(if oversized {
        LineRead::Oversized
    } else {
        LineRead::Line
    })
}

#[cfg(test)]
mod tests;
