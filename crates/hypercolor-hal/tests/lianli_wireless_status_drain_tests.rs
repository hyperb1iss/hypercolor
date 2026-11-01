//! The L-Wireless TX answers every command with a status packet. Nothing
//! reads them while frames stream, so the controller's transport drains
//! the queue once a second the way L-Connect does, after a frame that is
//! not RGB, and never where a caller is waiting for a reply.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_hal::drivers::lianli::wireless::frame::{
    USB_PACKET_LEN, WallClock, clock_payload, clock_sync_envelope, get_mac_query, rgb_transfer,
};
use hypercolor_hal::drivers::lianli::wireless::transport::{
    TX_STATUS_DRAIN_INTERVAL, TX_STATUS_DRAIN_LIMIT, WirelessControllerTransport,
};
use hypercolor_hal::protocol::TransferType;
use hypercolor_hal::transport::{Transport, TransportError};

const MASTER: [u8; 6] = [0xA0, 0x71, 0xAE, 0x72, 0xAB, 0x3C];

/// A TX that queues one status packet per command it takes, echoing the
/// command byte, and an RX that takes everything silently.
#[derive(Default)]
struct Tx {
    statuses: Mutex<VecDeque<Vec<u8>>>,
}

impl Tx {
    fn queued(&self) -> usize {
        self.statuses.lock().expect("status queue").len()
    }
}

struct Handle(Arc<Tx>);

#[async_trait]
impl Transport for Handle {
    fn name(&self) -> &'static str {
        "status-queueing pair"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        if transfer_type != TransferType::Companion {
            let mut status = vec![0_u8; USB_PACKET_LEN];
            status[0] = data[0];
            status[1..7].copy_from_slice(&MASTER);
            self.0
                .statuses
                .lock()
                .expect("status queue")
                .push_back(status);
        }
        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.receive_with_type(timeout, TransferType::Primary).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        let queued = if transfer_type == TransferType::Companion {
            None
        } else {
            self.0.statuses.lock().expect("status queue").pop_front()
        };
        if let Some(status) = queued {
            return Ok(status);
        }
        tokio::time::sleep(timeout).await;
        Err(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        self.receive(timeout).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

fn watched() -> (WirelessControllerTransport, Arc<Tx>) {
    let tx = Arc::new(Tx::default());
    let transport =
        WirelessControllerTransport::new(Box::new(Handle(Arc::clone(&tx))), "drain-test");
    (transport, tx)
}

async fn send_all(transport: &WirelessControllerTransport, packets: &[[u8; 64]]) {
    for packet in packets {
        transport
            .send_with_type(packet, TransferType::Primary)
            .await
            .expect("the TX takes it");
    }
}

fn clock_packets() -> [[u8; 64]; 4] {
    clock_sync_envelope(MASTER, &clock_payload(WallClock::default())).usb_packets(8, 0xFF)
}

fn rgb_packets() -> Vec<[u8; 64]> {
    let transfer = rgb_transfer([0x11; 6], MASTER, [0x80, 0, 0, 1], 3, 1, 5000, &[7; 9]);
    let mut packets = Vec::new();
    for envelope in [&transfer.header, &transfer.header]
        .into_iter()
        .chain(&transfer.data)
    {
        packets.extend(envelope.usb_packets(8, 1));
    }
    packets
}

#[tokio::test(start_paused = true)]
async fn the_clock_drains_what_the_tx_queued_and_rgb_never_does() {
    let (transport, tx) = watched();
    let rgb = rgb_packets();
    send_all(&transport, &rgb).await;
    assert_eq!(tx.queued(), rgb.len(), "no drain inside or after RGB");

    send_all(&transport, &clock_packets()).await;
    assert_eq!(
        tx.queued(),
        rgb.len() + 4 - TX_STATUS_DRAIN_LIMIT.min(rgb.len() + 4),
        "the clock's last slice drains the queue, up to the limit"
    );
}

#[tokio::test(start_paused = true)]
async fn a_drain_runs_at_most_once_an_interval() {
    let (transport, tx) = watched();
    send_all(&transport, &clock_packets()).await;
    assert_eq!(tx.queued(), 0, "the first clock of a session drains");

    tokio::time::sleep(TX_STATUS_DRAIN_INTERVAL / 2).await;
    send_all(&transport, &clock_packets()).await;
    assert_eq!(tx.queued(), 4, "half an interval later, no drain");

    tokio::time::sleep(TX_STATUS_DRAIN_INTERVAL).await;
    send_all(&transport, &clock_packets()).await;
    assert_eq!(tx.queued(), 0, "an interval on, the queue is drained again");
}

#[tokio::test(start_paused = true)]
async fn a_drain_stops_at_its_limit() {
    let (transport, tx) = watched();
    let rgb = rgb_packets();
    for _ in 0..3 {
        send_all(&transport, &rgb).await;
    }
    send_all(&transport, &clock_packets()).await;
    assert_eq!(tx.queued(), 3 * rgb.len() + 4 - TX_STATUS_DRAIN_LIMIT);
}

#[tokio::test(start_paused = true)]
async fn a_query_reply_is_left_for_the_caller() {
    let (transport, _tx) = watched();
    send_all(&transport, &clock_packets()).await;
    let query = get_mac_query(8);
    transport
        .send_with_type(&query, TransferType::Primary)
        .await
        .expect("the TX takes the query");
    let reply = transport
        .receive_logical(Duration::from_millis(50), TransferType::Primary, None)
        .await
        .expect("the reply is still there");
    assert_eq!(reply[0], query[0], "the query's own reply");
}
