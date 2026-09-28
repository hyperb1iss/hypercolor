//! A wedged L-Wireless dongle: how the controller's transport tells a half
//! that stopped taking commands from a slow reply, resets it through its
//! partner the way L-Connect does, and ends the session instead of writing
//! on into a dead endpoint.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hypercolor_hal::drivers::lianli::wireless::frame::{
    DEFAULT_CHANNEL, DONGLE_RESET, TX_RESET, USB_CMD_RESET_PARTNER, USB_PACKET_LEN, get_mac_query,
};
use hypercolor_hal::drivers::lianli::wireless::health::{
    ControllerHealth, DongleHalf, controller_health,
};
use hypercolor_hal::drivers::lianli::wireless::transport::{
    WirelessControllerTransport, partner_reset_packet,
};
use hypercolor_hal::protocol::TransferType;
use hypercolor_hal::transport::{Transport, TransportError};

/// What the transport's one-second write budget reports when it runs out.
const STALL: TransportError = TransportError::Timeout { timeout_ms: 1000 };

/// A scripted TX/RX pair: each half either takes commands or stalls. A TX
/// that takes a command queues its reply, the way the real TX answers every
/// command, unless it has gone mute.
#[derive(Default)]
struct Pair {
    tx_stalled: AtomicBool,
    tx_mute: AtomicBool,
    rx_stalled: AtomicBool,
    tx_gone: AtomicBool,
    reset_refused: AtomicBool,
    sends: Mutex<Vec<(TransferType, Vec<u8>)>>,
    tx_replies: Mutex<VecDeque<Vec<u8>>>,
}

impl Pair {
    fn stalled_tx() -> Arc<Self> {
        let pair = Arc::new(Self::default());
        pair.tx_stalled.store(true, Ordering::Relaxed);
        pair
    }

    fn sends(&self) -> Vec<(TransferType, Vec<u8>)> {
        self.sends.lock().expect("send log").clone()
    }

    fn stalled(&self, transfer_type: TransferType) -> bool {
        match transfer_type {
            TransferType::Companion => self.rx_stalled.load(Ordering::Relaxed),
            _ => self.tx_stalled.load(Ordering::Relaxed),
        }
    }
}

/// The pair as the watcher sees it; the test keeps the other handle.
struct Handle(Arc<Pair>);

#[async_trait]
impl Transport for Handle {
    fn name(&self) -> &'static str {
        "scripted pair"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        self.0
            .sends
            .lock()
            .expect("send log")
            .push((transfer_type, data.to_vec()));
        if data.first() == Some(&USB_CMD_RESET_PARTNER)
            && self.0.reset_refused.load(Ordering::Relaxed)
        {
            return Err(TransportError::IoError {
                detail: "partner refused the reset".to_owned(),
            });
        }
        if transfer_type != TransferType::Companion && self.0.tx_gone.load(Ordering::Relaxed) {
            return Err(TransportError::Disconnected {
                detail: "TX left the bus".to_owned(),
            });
        }
        if self.0.stalled(transfer_type) {
            return Err(STALL);
        }
        if transfer_type != TransferType::Companion && !self.0.tx_mute.load(Ordering::Relaxed) {
            let mut reply = vec![0_u8; USB_PACKET_LEN];
            reply[0] = data[0];
            self.0
                .tx_replies
                .lock()
                .expect("reply queue")
                .push_back(reply);
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
            self.0.tx_replies.lock().expect("reply queue").pop_front()
        };
        // Nothing queued: a reply that never comes.
        queued.ok_or(TransportError::Timeout {
            timeout_ms: u64::try_from(timeout.as_millis()).expect("short timeout"),
        })
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_with_type(data, transfer_type).await?;
        if self.0.stalled(transfer_type) {
            return Err(TransportError::Timeout {
                timeout_ms: u64::try_from(timeout.as_millis()).expect("short timeout"),
            });
        }
        let mut reply = vec![0_u8; USB_PACKET_LEN];
        reply[0] = data[0];
        Ok(reply)
    }

    async fn close(&self) -> Result<(), TransportError> {
        Ok(())
    }
}

fn watch(pair: &Arc<Pair>, controller: &str) -> WirelessControllerTransport {
    WirelessControllerTransport::new(Box::new(Handle(Arc::clone(pair))), controller)
}

fn rgb_slice() -> Vec<u8> {
    let mut packet = vec![0_u8; USB_PACKET_LEN];
    packet[..4].copy_from_slice(&[0x10, 0, DEFAULT_CHANNEL, 3]);
    packet
}

fn wedge_of(controller: &str) -> hypercolor_hal::drivers::lianli::wireless::health::Wedge {
    match controller_health(controller) {
        ControllerHealth::Wedged(wedge) => wedge,
        ControllerHealth::Healthy => panic!("{controller} should be wedged"),
    }
}

#[test]
fn the_partner_reset_is_command_0x15_and_nothing_else() {
    assert_eq!(DONGLE_RESET, [0x15]);
    let packet = partner_reset_packet();
    assert_eq!(packet.len(), USB_PACKET_LEN);
    assert_eq!(packet[0], 0x15);
    assert!(packet[1..].iter().all(|&byte| byte == 0));
}

#[test]
fn the_reference_reset_is_the_master_query_on_the_default_channel() {
    let query = get_mac_query(DEFAULT_CHANNEL);
    assert_eq!(&TX_RESET[..], &query[..TX_RESET.len()]);
}

#[tokio::test]
async fn a_stalled_tx_write_is_a_wedge_that_resets_it_through_the_rx() {
    let controller = "wedge-test-tx-write";
    let pair = Pair::stalled_tx();
    let transport = watch(&pair, controller);

    let error = transport
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await
        .expect_err("a stalled TX fails the write");

    let TransportError::Disconnected { detail } = &error else {
        panic!("a stall must end the session, not read as a transient timeout: {error:?}");
    };
    assert!(detail.contains("L-Wireless TX wedged"), "{detail}");
    assert!(detail.contains("reset sent through the RX"), "{detail}");
    assert_eq!(
        pair.sends(),
        vec![
            (TransferType::Primary, rgb_slice()),
            (TransferType::Companion, partner_reset_packet()),
        ],
        "the reset goes to the RX, right after the stalled write"
    );
    let wedge = wedge_of(controller);
    assert_eq!(wedge.half, DongleHalf::Tx);
    assert_eq!((wedge.stalls, wedge.resets_sent), (1, 1));
}

#[tokio::test]
async fn a_master_query_the_tx_refuses_is_a_wedge() {
    let controller = "wedge-test-master-query-refused";
    let pair = Pair::stalled_tx();
    let transport = watch(&pair, controller);

    let error = transport
        .send_receive_logical(
            &get_mac_query(DEFAULT_CHANNEL),
            Duration::from_secs(1),
            TransferType::Primary,
            None,
        )
        .await
        .expect_err("a TX that refuses the query fails the connect");

    assert!(
        matches!(error, TransportError::Disconnected { .. }),
        "{error:?}"
    );
    assert_eq!(
        pair.sends().last(),
        Some(&(TransferType::Companion, partner_reset_packet()))
    );
    assert_eq!(wedge_of(controller).half, DongleHalf::Tx);
}

#[tokio::test]
async fn a_master_query_the_tx_takes_but_never_answers_is_not_a_wedge() {
    let controller = "wedge-test-master-query-mute";
    let pair = Arc::new(Pair::default());
    pair.tx_mute.store(true, Ordering::Relaxed);
    let transport = watch(&pair, controller);

    let error = transport
        .send_receive_logical(
            &get_mac_query(DEFAULT_CHANNEL),
            Duration::from_secs(1),
            TransferType::Primary,
            None,
        )
        .await
        .expect_err("no reply fails the query");

    assert!(
        matches!(error, TransportError::Timeout { .. }),
        "a missing reply is not a refused write, so it stays a timeout: {error:?}"
    );
    assert_eq!(
        pair.sends(),
        vec![(TransferType::Primary, get_mac_query(DEFAULT_CHANNEL))],
        "no reset for a TX that took the write"
    );
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}

#[tokio::test]
async fn a_master_query_reads_the_reply_the_tx_queued() {
    let controller = "wedge-test-master-query-answered";
    let pair = Arc::new(Pair::default());
    let transport = watch(&pair, controller);

    let reply = transport
        .send_receive_logical(
            &get_mac_query(DEFAULT_CHANNEL),
            Duration::from_secs(1),
            TransferType::Primary,
            None,
        )
        .await
        .expect("a live TX answers");

    assert_eq!(reply[0], 0x11);
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}

#[tokio::test]
async fn a_stalled_rx_write_is_reset_through_the_tx() {
    let controller = "wedge-test-rx-write";
    let pair = Arc::new(Pair::default());
    pair.rx_stalled.store(true, Ordering::Relaxed);
    let transport = watch(&pair, controller);

    let error = transport
        .send_with_type(&[0x10, 0x01, 0x04, 0x30], TransferType::Companion)
        .await
        .expect_err("a stalled RX fails the write");

    assert!(
        matches!(error, TransportError::Disconnected { .. }),
        "{error:?}"
    );
    assert_eq!(
        pair.sends().last(),
        Some(&(TransferType::Primary, partner_reset_packet())),
        "the RX is reset through the TX"
    );
    assert_eq!(wedge_of(controller).half, DongleHalf::Rx);
}

#[tokio::test]
async fn a_table_poll_that_times_out_is_not_a_wedge() {
    let controller = "wedge-test-rx-poll";
    let pair = Arc::new(Pair::default());
    pair.rx_stalled.store(true, Ordering::Relaxed);
    let transport = watch(&pair, controller);

    let error = transport
        .send_receive_logical(
            &[0x10, 0x02],
            Duration::from_millis(500),
            TransferType::Companion,
            Some(1024),
        )
        .await
        .expect_err("the poll still fails");

    assert!(
        matches!(error, TransportError::Timeout { .. }),
        "a slow table reply cannot be told from a stalled poll, so it stays a timeout: {error:?}"
    );
    assert!(
        pair.sends()
            .iter()
            .all(|(_, data)| data[0] != USB_CMD_RESET_PARTNER),
        "no reset for a missed reply"
    );
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}

#[tokio::test]
async fn read_timeouts_never_count_as_stalls() {
    let controller = "wedge-test-reads";
    let pair = Arc::new(Pair::default());
    let transport = watch(&pair, controller);

    let error = transport
        .receive_logical(Duration::from_millis(20), TransferType::Primary, None)
        .await
        .expect_err("nothing queued");

    assert!(matches!(error, TransportError::Timeout { .. }), "{error:?}");
    assert!(pair.sends().is_empty());
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}

#[tokio::test]
async fn a_failed_partner_reset_still_reports_the_wedge() {
    let controller = "wedge-test-reset-refused";
    let pair = Pair::stalled_tx();
    pair.reset_refused.store(true, Ordering::Relaxed);
    let transport = watch(&pair, controller);

    let error = transport
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await
        .expect_err("a stalled TX fails the write");

    let TransportError::Disconnected { detail } = &error else {
        panic!("{error:?}");
    };
    assert!(
        detail.contains("reset through the RX failed"),
        "the error must not claim a reset went out: {detail}"
    );
    let wedge = wedge_of(controller);
    assert_eq!((wedge.stalls, wedge.resets_sent), (1, 0));
}

#[tokio::test]
async fn other_failures_pass_through_without_a_reset() {
    let controller = "wedge-test-gone";
    let pair = Arc::new(Pair::default());
    pair.tx_gone.store(true, Ordering::Relaxed);
    let transport = watch(&pair, controller);

    let error = transport
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await
        .expect_err("the TX left the bus");

    let TransportError::Disconnected { detail } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(detail, "TX left the bus");
    assert_eq!(pair.sends().len(), 1, "no reset for a device that is gone");
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}

#[tokio::test]
async fn a_wedge_outlives_the_session_and_counts_every_stall() {
    let controller = "wedge-test-repeat";
    let first = Pair::stalled_tx();
    let _ = watch(&first, controller)
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await;
    let since = wedge_of(controller).since;

    let second = Pair::stalled_tx();
    let _ = watch(&second, controller)
        .send_receive_logical(
            &TX_RESET,
            Duration::from_secs(1),
            TransferType::Primary,
            None,
        )
        .await;

    let wedge = wedge_of(controller);
    assert_eq!((wedge.stalls, wedge.resets_sent), (2, 2));
    assert_eq!(wedge.since, since, "one wedge, from its first stall");
}

#[tokio::test]
async fn the_first_command_a_wedged_tx_takes_closes_the_wedge() {
    let controller = "wedge-test-recovery";
    let stalled = Pair::stalled_tx();
    let _ = watch(&stalled, controller)
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await;

    let recovered = Arc::new(Pair::default());
    let transport = watch(&recovered, controller);
    transport
        .send_with_type(&[0x10, 0x01, 0x04, 0x30], TransferType::Companion)
        .await
        .expect("the RX takes its command");
    assert_eq!(
        wedge_of(controller).half,
        DongleHalf::Tx,
        "the RX answering says nothing about the TX"
    );

    transport
        .send_receive_logical(
            &TX_RESET,
            Duration::from_secs(1),
            TransferType::Primary,
            None,
        )
        .await
        .expect("the TX answers again");
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}

#[tokio::test]
async fn a_command_through_a_session_opened_before_the_wedge_closes_it() {
    let controller = "wedge-test-overlapping-sessions";
    let stalled = Pair::stalled_tx();
    let recovered = Arc::new(Pair::default());
    let first = watch(&stalled, controller);
    let second = watch(&recovered, controller);

    let _ = first
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await;
    assert_eq!(wedge_of(controller).half, DongleHalf::Tx);

    second
        .send_with_type(&rgb_slice(), TransferType::Primary)
        .await
        .expect("the other session's TX takes the write");
    assert_eq!(controller_health(controller), ControllerHealth::Healthy);
}
