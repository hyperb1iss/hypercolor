//! Completion rule for multi-packet logical reads.

use std::cell::RefCell;
use std::time::Duration;

use hypercolor_hal::transport::{
    DEFAULT_PACKET_GAP_TIMEOUT, TransportError, accumulate_logical_reply,
};

const PACKET: usize = 64;
const FIRST_TIMEOUT: Duration = Duration::from_millis(200);

/// A dongle reply is a 4-byte header plus 42 bytes per device record, so the
/// record count decides whether the reply ends on a short packet or has to go
/// quiet on an exact packet boundary.
fn reply_len(records: usize) -> usize {
    4 + records * 42
}

struct ScriptedPackets {
    packets: RefCell<Vec<Result<Vec<u8>, TransportError>>>,
    timeouts: RefCell<Vec<Duration>>,
}

impl ScriptedPackets {
    fn new(packets: Vec<Result<Vec<u8>, TransportError>>) -> Self {
        Self {
            packets: RefCell::new(packets),
            timeouts: RefCell::new(Vec::new()),
        }
    }

    /// Split `len` bytes into full packets plus whatever remains, then answer
    /// a timeout once the device has nothing left to say.
    fn from_reply(len: usize) -> Self {
        let mut packets: Vec<Result<Vec<u8>, TransportError>> = Vec::new();
        let mut written = 0;
        while written < len {
            let take = PACKET.min(len - written);
            packets.push(Ok(vec![
                u8::try_from(packets.len() % 251).unwrap_or(0);
                take
            ]));
            written += take;
        }
        packets.push(Err(TransportError::Timeout { timeout_ms: 20 }));
        Self::new(packets)
    }

    fn read(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.timeouts.borrow_mut().push(timeout);
        if self.packets.borrow().is_empty() {
            return Err(TransportError::Timeout { timeout_ms: 20 });
        }
        self.packets.borrow_mut().remove(0)
    }

    fn timeouts(&self) -> Vec<Duration> {
        self.timeouts.borrow().clone()
    }
}

#[test]
fn a_short_packet_ends_the_reply() {
    let source = ScriptedPackets::from_reply(reply_len(2));
    let capacity = reply_len(12);

    let reply = accumulate_logical_reply(
        PACKET,
        capacity,
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect("a two-record reply should accumulate");

    assert_eq!(reply.len(), reply_len(2), "88 bytes: one full, one short");
    assert_eq!(
        source.timeouts(),
        vec![FIRST_TIMEOUT, DEFAULT_PACKET_GAP_TIMEOUT],
        "the short packet ends the read with no gap wait after it"
    );
}

#[test]
fn a_reply_on_an_exact_packet_boundary_ends_on_the_gap_timeout() {
    let source = ScriptedPackets::from_reply(reply_len(6));
    let capacity = reply_len(12);

    let reply = accumulate_logical_reply(
        PACKET,
        capacity,
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect("a gap-terminated reply is a normal reply, not an error");

    assert_eq!(reply.len(), 256, "six records land on 4 x 64 bytes exactly");
    assert_eq!(
        source.timeouts(),
        vec![
            FIRST_TIMEOUT,
            DEFAULT_PACKET_GAP_TIMEOUT,
            DEFAULT_PACKET_GAP_TIMEOUT,
            DEFAULT_PACKET_GAP_TIMEOUT,
            DEFAULT_PACKET_GAP_TIMEOUT,
        ],
        "four packets plus the one gap wait that ends it"
    );
}

#[test]
fn reaching_capacity_stops_reading() {
    let source = ScriptedPackets::new(vec![
        Ok(vec![0x01; PACKET]),
        Ok(vec![0x02; PACKET]),
        Ok(vec![0x03; PACKET]),
    ]);

    let reply = accumulate_logical_reply(
        PACKET,
        PACKET * 2,
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect("two full packets fill the capacity");

    assert_eq!(reply.len(), PACKET * 2);
    assert_eq!(
        source.timeouts().len(),
        2,
        "capacity ends the read without a further packet"
    );
}

#[test]
fn a_reply_longer_than_capacity_is_truncated_not_overrun() {
    let source = ScriptedPackets::new(vec![Ok(vec![0xAB; PACKET])]);

    let reply = accumulate_logical_reply(
        PACKET,
        40,
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect("an oversized packet should be clipped to the declared capacity");

    assert_eq!(reply.len(), 40);
}

#[test]
fn a_timeout_before_any_bytes_is_an_error() {
    let source = ScriptedPackets::new(vec![Err(TransportError::Timeout { timeout_ms: 200 })]);

    let error = accumulate_logical_reply(
        PACKET,
        reply_len(12),
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect_err("a reply that never starts is a failed read");

    assert!(
        matches!(error, TransportError::Timeout { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn a_non_timeout_error_mid_reply_is_not_swallowed() {
    let source = ScriptedPackets::new(vec![
        Ok(vec![0x01; PACKET]),
        Err(TransportError::Disconnected {
            detail: "cable yanked".to_owned(),
        }),
    ]);

    let error = accumulate_logical_reply(
        PACKET,
        reply_len(12),
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect_err("a disconnect must not pass as a completed reply");

    assert!(
        matches!(error, TransportError::Disconnected { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn a_zero_capacity_read_touches_the_device_not_at_all() {
    let source = ScriptedPackets::new(vec![Ok(vec![0xFF; PACKET])]);

    let reply = accumulate_logical_reply(
        PACKET,
        0,
        FIRST_TIMEOUT,
        DEFAULT_PACKET_GAP_TIMEOUT,
        |timeout| source.read(timeout),
    )
    .expect("a zero capacity read is empty, not an error");

    assert!(reply.is_empty());
    assert!(source.timeouts().is_empty());
}
