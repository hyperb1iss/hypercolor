//! UDP request and reply client (Spec 88 §2.4 and §4.4).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};
use tracing::trace;

use crate::protocol::{
    FrameAddress, Message, ProtocolError, Serial, State, StateService, decode_packet, decode_state,
    encode_message, message_type,
};

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
            reply_timeout: Duration::from_millis(300),
            attempts: 3,
        }
    }
}

/// Errors from a request and reply exchange.
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("LIFX socket error: {0}")]
    Io(#[from] io::Error),
    #[error(
        "LIFX device {target} at {addr} did not answer message {message_type} after {attempts} attempts"
    )]
    Timeout {
        addr: SocketAddr,
        target: Serial,
        message_type: u16,
        attempts: u32,
    },
    #[error("LIFX device {target} does not implement message {message_type}")]
    Unsupported { target: Serial, message_type: u16 },
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

/// One StateService reply collected during discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceReply {
    pub serial: Serial,
    /// Reply source IP plus the advertised UDP port.
    pub addr: SocketAddr,
}

/// Pick a client source id. Never 0 or 1 (Spec 88 §2.1).
#[must_use]
pub fn random_source() -> u32 {
    let mixed = RandomState::new().hash_one((std::process::id(), std::time::SystemTime::now()));
    let low = u32::try_from(mixed & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    low.max(2)
}

/// UDP endpoint that speaks request and reply LIFX.
pub struct LifxClient {
    socket: UdpSocket,
    source: u32,
    sequence: u8,
    send_buf: Vec<u8>,
    recv_buf: Vec<u8>,
}

impl LifxClient {
    /// Bind an ephemeral socket in the address family of `peer`.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be bound.
    pub async fn bind_for(peer: &SocketAddr) -> io::Result<Self> {
        let local = match peer {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        Ok(Self::from_socket(
            UdpSocket::bind(local).await?,
            random_source(),
        ))
    }

    /// Wrap an existing socket.
    #[must_use]
    pub fn from_socket(socket: UdpSocket, source: u32) -> Self {
        Self {
            socket,
            source,
            sequence: 0,
            send_buf: Vec::with_capacity(64),
            recv_buf: vec![0; RECEIVE_BUFFER_LEN],
        }
    }

    /// The client's source id.
    #[must_use]
    pub const fn source(&self) -> u32 {
        self.source
    }

    /// Next sequence number the client will use.
    #[must_use]
    pub const fn sequence(&self) -> u8 {
        self.sequence
    }

    /// Give up the socket and sequence counter, for a streaming session.
    #[must_use]
    pub fn into_parts(self) -> (UdpSocket, u32, u8) {
        (self.socket, self.source, self.sequence)
    }

    /// Allow sending to broadcast addresses.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket option cannot be set.
    pub fn enable_broadcast(&self) -> io::Result<()> {
        self.socket.set_broadcast(true)
    }

    fn next_address(&mut self, target: Serial, ack_required: bool) -> FrameAddress {
        let address = FrameAddress {
            source: self.source,
            target,
            ack_required,
            res_required: false,
            sequence: self.sequence,
        };
        self.sequence = self.sequence.wrapping_add(1);
        address
    }

    /// Send GetService to every target, then collect StateService replies
    /// until `window` elapses. Replies are keyed by serial; the first UDP
    /// service seen for a serial wins.
    ///
    /// # Errors
    ///
    /// Returns an error only when encoding fails; per-target send errors
    /// are logged and skipped.
    pub async fn discover(
        &mut self,
        targets: &[SocketAddr],
        window: Duration,
    ) -> Result<Vec<ServiceReply>, TransportError> {
        let address = self.next_address(Serial::BROADCAST, false);
        self.send_buf.clear();
        encode_message(&mut self.send_buf, &address, &Message::GetService)?;
        for target in targets {
            if let Err(error) = self.socket.send_to(&self.send_buf, target).await {
                trace!(%target, %error, "LIFX GetService send failed");
            }
        }

        let deadline = Instant::now() + window;
        let mut replies: HashMap<Serial, ServiceReply> = HashMap::new();
        let mut order = Vec::new();
        loop {
            let Ok(received) =
                timeout_at(deadline, self.socket.recv_from(&mut self.recv_buf)).await
            else {
                break;
            };
            let (len, from) = match received {
                Ok(received) => received,
                Err(error) => {
                    trace!(%error, "LIFX discovery receive failed");
                    continue;
                }
            };
            let Ok(packet) = decode_packet(&self.recv_buf[..len]) else {
                continue;
            };
            if packet.header.source != address.source
                || packet.header.sequence != address.sequence
                || packet.header.message_type != message_type::STATE_SERVICE
            {
                continue;
            }
            let Ok(State::Service(service)) = decode_state(&packet) else {
                continue;
            };
            let serial = packet.header.target;
            if service.service != StateService::UDP
                || service.port == 0
                || serial.is_broadcast()
                || replies.contains_key(&serial)
            {
                continue;
            }
            let Ok(port) = u16::try_from(service.port) else {
                continue;
            };
            replies.insert(
                serial,
                ServiceReply {
                    serial,
                    addr: SocketAddr::new(from.ip(), port),
                },
            );
            order.push(serial);
        }
        Ok(order
            .into_iter()
            .filter_map(|serial| replies.remove(&serial))
            .collect())
    }

    /// Send a request and wait for a reply of one of `expected` types.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Unsupported`] when the device answers
    /// StateUnhandled, [`TransportError::Timeout`] when every attempt goes
    /// unanswered, or an I/O or protocol error.
    pub async fn request(
        &mut self,
        addr: SocketAddr,
        target: Serial,
        message: &Message<'_>,
        expected: &[u16],
        policy: RequestPolicy,
    ) -> Result<State, TransportError> {
        self.exchange(addr, target, message, false, expected, policy)
            .await
    }

    /// Send a Set message with ack_required and wait for the acknowledgement.
    ///
    /// # Errors
    ///
    /// See [`LifxClient::request`].
    pub async fn request_ack(
        &mut self,
        addr: SocketAddr,
        target: Serial,
        message: &Message<'_>,
        policy: RequestPolicy,
    ) -> Result<(), TransportError> {
        self.exchange(
            addr,
            target,
            message,
            true,
            &[message_type::ACKNOWLEDGEMENT],
            policy,
        )
        .await
        .map(|_| ())
    }

    async fn exchange(
        &mut self,
        addr: SocketAddr,
        target: Serial,
        message: &Message<'_>,
        ack_required: bool,
        expected: &[u16],
        policy: RequestPolicy,
    ) -> Result<State, TransportError> {
        let attempts = policy.attempts.max(1);
        for _ in 0..attempts {
            let address = self.next_address(target, ack_required);
            self.send_buf.clear();
            encode_message(&mut self.send_buf, &address, message)?;
            self.socket.send_to(&self.send_buf, addr).await?;

            let deadline = Instant::now() + policy.reply_timeout;
            loop {
                let Ok(received) =
                    timeout_at(deadline, self.socket.recv_from(&mut self.recv_buf)).await
                else {
                    break;
                };
                let Ok((len, _from)) = received else {
                    continue;
                };
                let Ok(packet) = decode_packet(&self.recv_buf[..len]) else {
                    continue;
                };
                let header = packet.header;
                if header.source != address.source
                    || header.sequence != address.sequence
                    || (!target.is_broadcast() && header.target != target)
                {
                    continue;
                }
                if header.message_type == message_type::STATE_UNHANDLED {
                    return Err(TransportError::Unsupported {
                        target,
                        message_type: message.message_type(),
                    });
                }
                if !expected.contains(&header.message_type) {
                    continue;
                }
                return Ok(decode_state(&packet)?);
            }
        }
        Err(TransportError::Timeout {
            addr,
            target,
            message_type: message.message_type(),
            attempts,
        })
    }
}
