//! Bounded framing for byte streams that carry delimited lines or frames.
//!
//! [`BoundedLines`] caps every line and reads on an absolute deadline, so
//! a multi-line reply shares one deadline however slowly the peer drips.
//! [`FramedWriter`] owns its stream and gives it up the moment a frame
//! fails to go out whole, because the peer would read whatever follows a
//! partial frame as the rest of that frame.

use std::io;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::time::Instant;

use crate::deadline::until_deadline;

/// Why [`BoundedLines`] produced no line.
#[derive(Debug, Error)]
pub enum LineError {
    #[error("line longer than {limit} bytes")]
    TooLong { limit: usize },
    #[error("line did not finish before its deadline")]
    TimedOut,
    #[error("line read failed: {0}")]
    Read(#[source] io::Error),
}

/// Reads CRLF- or LF-terminated lines with a length cap.
///
/// A missed deadline keeps the partial line, so a later call resumes it
/// and dropping a call in progress loses nothing. An oversized line is
/// final: its tail must never read as a fresh line, so every later call
/// fails the same way.
pub struct BoundedLines<R> {
    reader: BufReader<R>,
    line: Vec<u8>,
    /// When the line in progress took its first byte. It lives with the
    /// partial line so that a cancelled call cannot renew the line's bound.
    line_started: Option<Instant>,
    max_line_bytes: usize,
    overflowed: bool,
}

impl<R: AsyncRead + Unpin> BoundedLines<R> {
    /// Read lines of at most `max_line_bytes`, terminator excluded.
    #[must_use]
    pub fn new(reader: R, max_line_bytes: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            line: Vec::new(),
            line_started: None,
            max_line_bytes,
            overflowed: false,
        }
    }

    /// The next line without its terminator, or `None` at end of stream.
    /// An unterminated tail at end of stream is not a line.
    ///
    /// Pass one `deadline` to every call of a multi-line reply: it holds
    /// however much data the peer has ready.
    ///
    /// # Errors
    ///
    /// Returns [`LineError::TooLong`] past the cap,
    /// [`LineError::TimedOut`] at the deadline, or [`LineError::Read`].
    pub async fn next(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, LineError> {
        let too_long = LineError::TooLong {
            limit: self.max_line_bytes,
        };
        if self.overflowed {
            return Err(too_long);
        }
        loop {
            let available = until_deadline(deadline, self.reader.fill_buf())
                .await
                .ok_or(LineError::TimedOut)?
                .map_err(LineError::Read)?;
            if available.is_empty() {
                return Ok(None);
            }
            let (chunk, complete) = match available.iter().position(|byte| *byte == b'\n') {
                Some(end) => (&available[..=end], true),
                None => (available, false),
            };
            // Two bytes of slack for the terminator of a line at the cap.
            if self.line.len() + chunk.len() > self.max_line_bytes + 2 {
                self.overflowed = true;
                return Err(too_long);
            }
            self.line.extend_from_slice(chunk);
            self.line_started.get_or_insert_with(Instant::now);
            let consumed = chunk.len();
            self.reader.consume(consumed);
            if complete {
                while matches!(self.line.last(), Some(b'\n' | b'\r')) {
                    self.line.pop();
                }
                if self.line.len() > self.max_line_bytes {
                    self.overflowed = true;
                    return Err(too_long);
                }
                self.line_started = None;
                return Ok(Some(std::mem::take(&mut self.line)));
            }
        }
    }

    /// The next line of a connection that may legitimately stay silent:
    /// wait as long as the peer does, then hold the line it starts to
    /// `completion` from its first byte. That clock stays with the partial
    /// line, so calling again after a cancelled or missed read never
    /// renews it.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::next`].
    pub async fn next_after_idle(
        &mut self,
        completion: Duration,
    ) -> Result<Option<Vec<u8>>, LineError> {
        if self.overflowed {
            return Err(LineError::TooLong {
                limit: self.max_line_bytes,
            });
        }
        let started = if let Some(started) = self.line_started {
            started
        } else {
            let available = self.reader.fill_buf().await.map_err(LineError::Read)?;
            if available.is_empty() {
                return Ok(None);
            }
            *self.line_started.insert(Instant::now())
        };
        self.next(started + completion).await
    }
}

/// Why [`FramedWriter`] did not deliver a frame.
#[derive(Debug, Error)]
pub enum FrameWriteError {
    /// The stream was closed, or an earlier frame invalidated it.
    #[error("the stream is closed")]
    Closed,
    #[error("frame write missed its deadline")]
    TimedOut,
    #[error("frame write failed: {0}")]
    Write(#[source] io::Error),
}

/// Owns a stream and writes whole frames to it on an absolute deadline.
///
/// A frame that fails, times out, or is abandoned mid-write costs the
/// stream: the writer drops it, so a half-written frame can never be
/// followed by anything. Later writes report [`FrameWriteError::Closed`].
///
/// Every frame is flushed, so this is not for serial ports: flushing a tty
/// blocks a runtime thread until the UART drains.
pub struct FramedWriter<W> {
    stream: Option<W>,
}

impl<W: AsyncWrite + Unpin> FramedWriter<W> {
    /// Take ownership of `stream`.
    #[must_use]
    pub const fn new(stream: W) -> Self {
        Self {
            stream: Some(stream),
        }
    }

    /// Write and flush all of `frame`, or give up the stream trying.
    ///
    /// The stream leaves `self` for the duration, so dropping this future
    /// mid-write invalidates it just as a failure would.
    ///
    /// # Errors
    ///
    /// Returns [`FrameWriteError::Closed`] without touching the stream
    /// when it is already gone, [`FrameWriteError::TimedOut`] at the
    /// deadline, or [`FrameWriteError::Write`].
    pub async fn write(&mut self, frame: &[u8], deadline: Instant) -> Result<(), FrameWriteError> {
        let mut stream = self.stream.take().ok_or(FrameWriteError::Closed)?;
        write_frame(&mut stream, frame, deadline).await?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Drop the stream. Later writes report [`FrameWriteError::Closed`].
    pub fn close(&mut self) {
        self.stream = None;
    }
}

/// Drive one write at a time so the clock is read between them: a stream
/// that keeps taking short writes never pends, so nothing else would stop
/// it crossing the deadline.
async fn write_frame<W: AsyncWrite + Unpin>(
    stream: &mut W,
    frame: &[u8],
    deadline: Instant,
) -> Result<(), FrameWriteError> {
    let mut remaining = frame;
    while !remaining.is_empty() {
        let written = until_deadline(deadline, stream.write(remaining))
            .await
            .ok_or(FrameWriteError::TimedOut)?
            .map_err(FrameWriteError::Write)?;
        if written == 0 {
            return Err(FrameWriteError::Write(io::ErrorKind::WriteZero.into()));
        }
        remaining = &remaining[written..];
    }
    until_deadline(deadline, stream.flush())
        .await
        .ok_or(FrameWriteError::TimedOut)?
        .map_err(FrameWriteError::Write)
}
