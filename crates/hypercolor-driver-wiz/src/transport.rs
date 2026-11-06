//! UDP request and reply exchange for probes (Spec 93 §2.3).
//!
//! Streaming never waits for replies; only discovery and probing do.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};
use tracing::trace;

use crate::protocol::{ProtocolError, Reply, decode_reply};

const RECEIVE_BUFFER_LEN: usize = 2048;

/// Timeout and retry policy for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPolicy {
    /// How long to wait for a reply to one attempt.
    pub reply_timeout: Duration,
    /// Attempts before giving up, at least one.
    pub attempts: u32,
}

impl Default for RequestPolicy {
    fn default() -> Self {
        Self {
            reply_timeout: Duration::from_millis(400),
            attempts: 3,
        }
    }
}

/// Errors from a request and reply exchange.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("WiZ socket error: {0}")]
    Io(#[from] io::Error),
    #[error("WiZ device at {addr} did not answer {method} after {attempts} attempts")]
    Timeout {
        addr: SocketAddr,
        method: String,
        attempts: u32,
    },
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

/// Bind an ephemeral UDP socket in the address family of `peer`.
///
/// # Errors
///
/// Returns an error if the socket cannot be bound.
pub async fn bind_for(peer: &SocketAddr) -> io::Result<UdpSocket> {
    let local = match peer {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    UdpSocket::bind(local).await
}

/// Send `request` to `addr` and wait for a reply to `method`, retrying on
/// silence. Replies from other addresses or to other methods are ignored.
///
/// # Errors
///
/// Returns [`TransportError::Timeout`] when no matching reply arrives.
pub async fn request(
    socket: &UdpSocket,
    addr: SocketAddr,
    request: &str,
    method: &str,
    policy: RequestPolicy,
) -> Result<Reply, TransportError> {
    let attempts = policy.attempts.max(1);
    let mut buffer = vec![0_u8; RECEIVE_BUFFER_LEN];
    for attempt in 1..=attempts {
        socket.send_to(request.as_bytes(), addr).await?;
        let deadline = Instant::now() + policy.reply_timeout;
        loop {
            match timeout_at(deadline, socket.recv_from(&mut buffer)).await {
                Err(_) => break,
                Ok(Err(error)) => return Err(error.into()),
                Ok(Ok((len, source))) => {
                    if source.ip() != addr.ip() {
                        continue;
                    }
                    match decode_reply(&buffer[..len]) {
                        Ok(reply) if reply.method == method || reply.method.is_empty() => {
                            return Ok(reply);
                        }
                        Ok(reply) => {
                            trace!(%addr, method = %reply.method, "ignoring WiZ reply to another method");
                        }
                        Err(error) => trace!(%addr, %error, "ignoring undecodable WiZ datagram"),
                    }
                }
            }
        }
        trace!(%addr, method, attempt, "WiZ request timed out");
    }
    Err(TransportError::Timeout {
        addr,
        method: method.to_owned(),
        attempts,
    })
}
