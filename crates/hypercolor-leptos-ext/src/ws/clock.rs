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
//! Both types decode from the whole message object: serde ignores the
//! fields around them, so a client reads the stamps without modeling the
//! rest of the message.

use serde::{Deserialize, Serialize};

/// When the daemon published one timestamped server message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationStamp {
    /// Wall-clock publication time, ISO 8601 UTC with millisecond
    /// precision.
    pub timestamp: String,
    /// Publication time in microseconds on the monotonic clock the
    /// `hello` names. Never moves backwards within one `clock_epoch` and
    /// ignores wall-clock adjustments.
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
