//! Daemon clock fields on JSON server messages.
//!
//! Every server message that reports when it was published carries two
//! stamps side by side: the wall-clock `timestamp`, which follows the
//! host clock through NTP slews and manual steps, and `mono_us`, read
//! from one monotonic clock per daemon run. The `hello` names that run's
//! clock with an opaque `clock_epoch` and the clock's reading at the
//! handshake, so a client can line daemon stamps up with its own
//! monotonic clock and knows to discard the alignment when the epoch
//! changes.
//!
//! Command `response` messages carry `mono_us` alone: the clock's reading
//! after the command finished, which falls inside the client's round trip
//! and bounds the offset between the two clocks.
//!
//! Every type here decodes from the whole message object: serde ignores
//! the fields around it, so a client reads the stamps without modeling
//! the rest of the message.

use serde::{Deserialize, Serialize};

/// When the daemon published one timestamped server message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationStamp {
    /// Wall-clock publication time, ISO 8601 UTC with millisecond
    /// precision.
    pub timestamp: String,
    /// Publication time in microseconds on the monotonic clock the
    /// `hello` names. Never moves backwards within one `clock_epoch` and
    /// does not jump when the wall clock is stepped, but its rate can
    /// follow NTP frequency corrections, so a long session recalibrates
    /// against command response stamps instead of assuming a fixed rate.
    pub mono_us: u64,
}

/// The daemon run's monotonic clock, announced once per connection in
/// the `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloClock {
    /// Opaque identity of this daemon run's monotonic clock. Compare by
    /// equality only: a different value means a different run, and
    /// `mono_us` stamps from the two runs share no origin.
    pub clock_epoch: String,
    /// The monotonic clock's reading, in microseconds, when the daemon
    /// built the `hello`.
    pub mono_us: u64,
}

/// When the daemon finished one WebSocket command, carried on its
/// `response`.
///
/// A client that sent the command at its own monotonic time `t0` and
/// received the response at `t1` knows `t0 - mono_us <= offset <=
/// t1 - mono_us`, where `offset` is client time minus daemon time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseStamp {
    /// The monotonic clock's reading, in microseconds, taken after the
    /// command finished and immediately before the daemon wrote the
    /// `response` to the socket. `None` from daemons that predate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_us: Option<u64>,
}
