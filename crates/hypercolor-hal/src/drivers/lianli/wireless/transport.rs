//! The controller's transport: the TX bulk device paired with its RX
//! sibling behind the same internal hub, watched for a half that stops
//! taking commands, with the vendor's air gap kept between RF frames.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use tracing::{debug, info, trace, warn};

use super::frame::{
    DONGLE_RESET, Mac, RF_SELECT, RF_SLICES_PER_ENVELOPE, RfSubCommand, USB_CMD_RESET_PARTNER,
    USB_CMD_SEND_RF, USB_PACKET_LEN, USB_RF_HEADER_LEN,
};
use super::health::{self, DongleHalf, WedgeCause};
use crate::protocol::TransferType;
use crate::registry::{UsbTransportFuture, UsbTransportOpenRequest};
use crate::transport::bulk::UsbBulkTransport;
use crate::transport::companion::CompanionTransport;
use crate::transport::{Transport, TransportError};

/// Vendor ID both halves enumerate under.
pub const WIRELESS_VENDOR_ID: u16 = 0x0416;
/// Product ID of the TX half, the device a descriptor binds.
pub const PID_WIRELESS_TX: u16 = 0x8040;
/// Product ID of the RX half, opened as the TX's companion.
pub const PID_WIRELESS_RX: u16 = 0x8041;
/// Both halves claim their only interface.
const WIRELESS_INTERFACE: u8 = 0;
/// Neither half has a HID sideband.
const NO_REPORT_ID: u8 = 0;

/// Open the TX device handed over by discovery, find and open the RX that
/// shares its parent hub, and pair them.
///
/// Without the RX there is no device table and nothing to drive, so a
/// missing sibling fails the open rather than yielding a controller that
/// discovers nothing.
pub fn open_wireless_controller(request: UsbTransportOpenRequest) -> UsbTransportFuture {
    Box::pin(async move {
        let tx = UsbBulkTransport::new(request.device, WIRELESS_INTERFACE, NO_REPORT_ID).await?;
        let rx_info = find_rx_sibling(request.usb_path.as_deref()).await?;
        let rx_device = rx_info
            .open()
            .await
            .map_err(|error| TransportError::IoError {
                detail: format!("opening L-Wireless RX {PID_WIRELESS_RX:04X}: {error}"),
            })?;
        let rx = UsbBulkTransport::new(rx_device, WIRELESS_INTERFACE, NO_REPORT_ID).await?;
        let controller = request
            .usb_path
            .unwrap_or_else(|| UNKNOWN_CONTROLLER.to_owned());
        debug!(
            tx_path = %controller,
            rx_path = %usb_path(&rx_info),
            "paired L-Wireless TX with its RX sibling"
        );
        let pair = CompanionTransport::new(TRANSPORT_NAME, Box::new(tx), Box::new(rx));
        Ok(
            Box::new(WirelessControllerTransport::new(Box::new(pair), controller))
                as Box<dyn Transport>,
        )
    })
}

/// Name both the pair and its watcher report.
const TRANSPORT_NAME: &str = "lianli-wireless";

/// The vendor's air gap after an RGB frame, before a frame to another
/// receiver or of another kind.
pub const AIR_GAP_AFTER_RGB: Duration = Duration::from_millis(10);
/// The air gap after any other frame, before a frame to another receiver
/// or of another kind.
pub const AIR_GAP_BETWEEN_FRAMES: Duration = Duration::from_millis(5);

/// How long an RF frame to `target` carrying `sub` must still wait, when
/// the TX took the last slice of `previous` (its target and sub-command)
/// `since` ago.
///
/// A receiver that hears a frame too soon after another receiver's frame
/// can latch the earlier frame's trailing bytes as its address and duty, so
/// a frame to a new target or of a new kind waits out the gap; frames of
/// one kind to one target (a transfer's header repeats and data) follow
/// each other freely. The gaps are lian-li-linux's (`RfPacer`), which
/// match L-Connect's spacing: 5 ms between devices in its fan-speed loop,
/// 10 ms after an effect upload.
#[must_use]
pub fn air_gap(previous: Option<(Mac, u8)>, since: Duration, target: Mac, sub: u8) -> Duration {
    let Some((last_target, last_sub)) = previous else {
        return Duration::ZERO;
    };
    if last_target == target && last_sub == sub {
        return Duration::ZERO;
    }
    let gap = if last_sub == RfSubCommand::SetRgb as u8 {
        AIR_GAP_AFTER_RGB
    } else {
        AIR_GAP_BETWEEN_FRAMES
    };
    gap.saturating_sub(since)
}

/// The TX answers every command with a status packet, and nothing reads
/// them while frames stream. L-Connect reads every pending one each second
/// (`WinUsb.RfRead` drains through `ReadAll` until a read waits 10 ms);
/// the transport drains them this often, after the last slice of a frame
/// that is not RGB, which in steady state is the 1 Hz clock broadcast.
pub const TX_STATUS_DRAIN_INTERVAL: Duration = Duration::from_secs(1);
/// Status packets one drain reads at most.
pub const TX_STATUS_DRAIN_LIMIT: usize = 16;
/// A drain ends at the first read that waits this long.
pub const TX_STATUS_DRAIN_GAP: Duration = Duration::from_millis(10);

/// Where a TX packet sits in an RF frame.
#[derive(Debug, Clone, Copy)]
enum RfSlice {
    /// The first slice, which names the frame's target and sub-command.
    First { target: Mac, sub: u8 },
    /// A later slice of the frame in progress; `last` for its fourth.
    Later { last: bool },
}

/// Where `packet` sits in an RF frame, if it is a slice of one.
fn rf_slice(packet: &[u8]) -> Option<RfSlice> {
    let envelope = packet.get(USB_RF_HEADER_LEN..)?;
    if packet[0] != USB_CMD_SEND_RF {
        return None;
    }
    match usize::from(packet[1]) {
        0 if envelope.first() == Some(&RF_SELECT) => Some(RfSlice::First {
            target: envelope.get(2..8)?.try_into().ok()?,
            sub: *envelope.get(1)?,
        }),
        index if (1..RF_SLICES_PER_ENVELOPE).contains(&index) => Some(RfSlice::Later {
            last: index + 1 == RF_SLICES_PER_ENVELOPE,
        }),
        _ => None,
    }
}

/// The RF frame the TX took last.
#[derive(Debug, Clone, Copy)]
struct AirFrame {
    target: Mac,
    sub: u8,
    last_slice_at: Instant,
}
/// Health key for a TX the scanner handed over without a USB path.
const UNKNOWN_CONTROLLER: &str = "<unknown>";

/// The TX/RX pair, watched for a half that stops taking commands.
///
/// A write that runs out its whole budget marks its half unresponsive (see
/// [`health`] for why one refused write is treated as the vendor's verdict).
/// The watcher then records the wedge, sends the vendor's partner reset
/// through the other half, and fails the write as a disconnect: frame writes
/// treat a timeout as transient and would keep writing into the refusing
/// endpoint, while a disconnect ends the session so the lifecycle's
/// reconnect backoff paces every later attempt.
///
/// Only writes count, as in the vendor tool. A missing reply is not a
/// refused write: optional replies time out on a healthy dongle, and a TX
/// that takes the master query without answering it is failing differently.
/// So a TX send-and-receive runs as a watched send then an unwatched read.
/// That split is safe because the controller is single-lane: its actor
/// issues one command at a time, so no other reply can land between the two.
/// The RX's table poll stays one exchange and is never counted, because its
/// timeout cannot tell a refused write from a slow reply.
///
/// The protocol has the other verdict: a TX that takes every write while its
/// fans stop confirming frames. It asks for the reset by writing the partner
/// reset itself, and this watcher routes that write through the same path
/// (record, reset, end the session), since a reset half has to be set up
/// again from scratch either way.
pub struct WirelessControllerTransport {
    pair: Box<dyn Transport>,
    controller: String,
    /// The RF frame the TX took last, for the air gap.
    on_air: Mutex<Option<AirFrame>>,
    /// When the TX's status packets were last drained.
    drained_at: Mutex<Option<Instant>>,
}

impl WirelessControllerTransport {
    /// Watch `pair` (TX on the primary path, RX on the companion path) for
    /// the controller identified by `controller`, the TX's USB path.
    #[must_use]
    pub fn new(pair: Box<dyn Transport>, controller: impl Into<String>) -> Self {
        Self {
            pair,
            controller: controller.into(),
            on_air: Mutex::new(None),
            drained_at: Mutex::new(None),
        }
    }

    /// Where a write sits in an RF frame, if it is a TX write of one.
    fn tx_rf_slice(data: &[u8], transfer_type: TransferType) -> Option<RfSlice> {
        (DongleHalf::for_transfer(transfer_type) == DongleHalf::Tx)
            .then(|| rf_slice(data))
            .flatten()
    }

    /// Before a TX write: if it starts an RF frame, wait out whatever is
    /// left of the air gap after the frame before it.
    async fn keep_air_gap(&self, slice: Option<RfSlice>) {
        let Some(RfSlice::First { target, sub }) = slice else {
            return;
        };
        let wait = {
            let on_air = self.on_air.lock().unwrap_or_else(PoisonError::into_inner);
            on_air.map_or(Duration::ZERO, |frame| {
                air_gap(
                    Some((frame.target, frame.sub)),
                    frame.last_slice_at.elapsed(),
                    target,
                    sub,
                )
            })
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// After a TX write the TX took: note the RF frame it belongs to, and
    /// say whether the write finished a frame that is not RGB.
    fn note_on_air(&self, slice: Option<RfSlice>) -> bool {
        let Some(slice) = slice else {
            return false;
        };
        let mut on_air = self.on_air.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        match slice {
            RfSlice::First { target, sub } => {
                *on_air = Some(AirFrame {
                    target,
                    sub,
                    last_slice_at: now,
                });
                false
            }
            RfSlice::Later { last } => on_air.as_mut().is_some_and(|frame| {
                frame.last_slice_at = now;
                last && frame.sub != RfSubCommand::SetRgb as u8
            }),
        }
    }

    /// Once a drain interval has passed, read and discard the status
    /// packets the TX has queued, as L-Connect does every second. Only after
    /// a frame that is not RGB: never inside an RGB transfer, and never
    /// after a command whose reply a caller is about to read. A read that
    /// times out is an empty queue, never a wedge; reads do not count.
    async fn drain_tx_status(&self, frame_done: bool) {
        if !frame_done {
            return;
        }
        {
            let mut drained_at = self
                .drained_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            if drained_at.is_some_and(|at| now.duration_since(at) < TX_STATUS_DRAIN_INTERVAL) {
                return;
            }
            *drained_at = Some(now);
        }
        let mut drained = 0_usize;
        while drained < TX_STATUS_DRAIN_LIMIT {
            match self
                .pair
                .receive_logical(TX_STATUS_DRAIN_GAP, TransferType::Primary, None)
                .await
            {
                Ok(_) => drained += 1,
                Err(_) => break,
            }
        }
        if drained == TX_STATUS_DRAIN_LIMIT {
            debug!(
                controller = %self.controller,
                drained,
                "L-Wireless TX status queue still full after a drain"
            );
        } else {
            trace!(controller = %self.controller, drained, "L-Wireless TX status drained");
        }
    }

    /// Pass a completed write through, reading a stall as a wedge.
    async fn watch_write<T>(
        &self,
        half: DongleHalf,
        result: Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        match result {
            Ok(value) => {
                self.note_alive(half);
                Ok(value)
            }
            Err(TransportError::Timeout { timeout_ms }) => Err(self.wedged(half, timeout_ms).await),
            Err(error) => Err(error),
        }
    }

    fn note_alive(&self, half: DongleHalf) {
        if !health::any_wedge_open() {
            return;
        }
        if let Some(wedge) = health::record_alive(&self.controller, half) {
            let wedged_for_ms =
                u64::try_from(wedge.since.elapsed().as_millis()).unwrap_or(u64::MAX);
            match wedge.cause {
                WedgeCause::RefusedWrite => info!(
                    controller = %self.controller,
                    half = half.name(),
                    wedged_for_ms,
                    stalls = wedge.stalls,
                    resets_sent = wedge.resets_sent,
                    "L-Wireless {} recovered: it is taking commands again",
                    half.name()
                ),
                // Taking writes was never the problem; the delivery report
                // is what shows whether its fans confirm frames again.
                WedgeCause::Undelivered => info!(
                    controller = %self.controller,
                    half = half.name(),
                    wedged_for_ms,
                    stalls = wedge.stalls,
                    resets_sent = wedge.resets_sent,
                    "L-Wireless {} is taking commands again after its reset",
                    half.name()
                ),
            }
        }
    }

    /// Record the wedge, reset the half through its partner, and name the
    /// failure for the caller.
    async fn wedged(&self, half: DongleHalf, timeout_ms: u64) -> TransportError {
        let wedge = health::record_stall(&self.controller, half, WedgeCause::RefusedWrite);
        let partner = half.partner();
        if wedge.stalls == 1 {
            warn!(
                controller = %self.controller,
                half = half.name(),
                stalled_ms = timeout_ms,
                "L-Wireless {} wedged: it refused a write for its whole budget; resetting it through the {}",
                half.name(),
                partner.name()
            );
        } else {
            warn!(
                controller = %self.controller,
                half = half.name(),
                stalled_ms = timeout_ms,
                stalls = wedge.stalls,
                resets_sent = wedge.resets_sent,
                wedged_for_ms = u64::try_from(wedge.since.elapsed().as_millis()).unwrap_or(u64::MAX),
                "L-Wireless {} still wedged; resetting it through the {} again",
                half.name(),
                partner.name()
            );
        }

        let reset = self.reset_through_partner(half).await;
        TransportError::Disconnected {
            detail: format!(
                "L-Wireless {} wedged: it refused a write for {timeout_ms} ms; {reset}. \
                 If it does not come back, replug or power-cycle the controller",
                half.name()
            ),
        }
    }

    /// The protocol's verdict: `half` takes writes but what it sends is no
    /// longer confirmed. Recorded and reset like a refused write.
    async fn undelivered(&self, half: DongleHalf) -> TransportError {
        let wedge = health::record_stall(&self.controller, half, WedgeCause::Undelivered);
        warn!(
            controller = %self.controller,
            half = half.name(),
            stalls = wedge.stalls,
            resets_sent = wedge.resets_sent,
            "L-Wireless {} delivery remains unconfirmed after recovery probes; resetting it through the {}",
            half.name(),
            half.partner().name()
        );
        let reset = self.reset_through_partner(half).await;
        TransportError::Disconnected {
            detail: format!(
                "L-Wireless {} delivery remains unconfirmed after recovery probes; {reset}. \
                 If it does not come back, replug or power-cycle the controller",
                half.name()
            ),
        }
    }

    /// Write the vendor reset for `half` to its partner, and describe the
    /// outcome for the error that ends the session.
    async fn reset_through_partner(&self, half: DongleHalf) -> String {
        let partner = half.partner();
        match self
            .pair
            .send_with_type(&partner_reset_packet(), partner.transfer_type())
            .await
        {
            Ok(()) => {
                health::record_reset_sent(&self.controller, half);
                info!(
                    controller = %self.controller,
                    half = half.name(),
                    "L-Wireless {} reset sent through the {}",
                    half.name(),
                    partner.name()
                );
                format!("reset sent through the {}", partner.name())
            }
            Err(error) => {
                warn!(
                    controller = %self.controller,
                    half = half.name(),
                    error = %error,
                    "L-Wireless {} reset through the {} failed",
                    half.name(),
                    partner.name()
                );
                format!("reset through the {} failed ({error})", partner.name())
            }
        }
    }

    /// A send-and-receive. On the TX it runs as a watched send then an
    /// unwatched read (see the type docs); on the RX it stays one exchange,
    /// where only success is read, as proof the RX is alive.
    async fn exchange(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        match DongleHalf::for_transfer(transfer_type) {
            DongleHalf::Tx => {
                let sent = self.pair.send_with_type(data, transfer_type).await;
                self.watch_write(DongleHalf::Tx, sent).await?;
                self.pair
                    .receive_logical(timeout, transfer_type, capacity)
                    .await
            }
            DongleHalf::Rx => {
                let reply = self
                    .pair
                    .send_receive_logical(data, timeout, transfer_type, capacity)
                    .await?;
                self.note_alive(DongleHalf::Rx);
                Ok(reply)
            }
        }
    }
}

/// Whether `data` is the vendor's partner reset, which no protocol command
/// shares a first byte with.
fn is_partner_reset(data: &[u8]) -> bool {
    data.first() == Some(&USB_CMD_RESET_PARTNER)
}

/// The vendor's partner reset, padded to a controller packet.
#[must_use]
pub fn partner_reset_packet() -> Vec<u8> {
    let mut packet = vec![0_u8; USB_PACKET_LEN];
    packet[..DONGLE_RESET.len()].copy_from_slice(&DONGLE_RESET);
    packet
}

#[async_trait]
impl Transport for WirelessControllerTransport {
    fn name(&self) -> &'static str {
        TRANSPORT_NAME
    }

    fn supports_parallel_transfer_lanes(&self) -> bool {
        self.pair.supports_parallel_transfer_lanes()
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        if is_partner_reset(data) {
            let half = DongleHalf::for_transfer(transfer_type).partner();
            return Err(self.undelivered(half).await);
        }
        let slice = Self::tx_rf_slice(data, transfer_type);
        self.keep_air_gap(slice).await;
        let result = self.pair.send_with_type(data, transfer_type).await;
        self.watch_write(DongleHalf::for_transfer(transfer_type), result)
            .await?;
        let frame_done = self.note_on_air(slice);
        self.drain_tx_status(frame_done).await;
        Ok(())
    }

    async fn send_owned_with_type(
        &self,
        data: Vec<u8>,
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        if is_partner_reset(&data) {
            let half = DongleHalf::for_transfer(transfer_type).partner();
            return Err(self.undelivered(half).await);
        }
        let slice = Self::tx_rf_slice(&data, transfer_type);
        self.keep_air_gap(slice).await;
        let result = self.pair.send_owned_with_type(data, transfer_type).await;
        self.watch_write(DongleHalf::for_transfer(transfer_type), result)
            .await?;
        let frame_done = self.note_on_air(slice);
        self.drain_tx_status(frame_done).await;
        Ok(())
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.pair.receive(timeout).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.pair.receive_with_type(timeout, transfer_type).await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_receive_with_type(data, timeout, TransferType::Primary)
            .await
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.exchange(data, timeout, transfer_type, None).await
    }

    async fn receive_logical(
        &self,
        timeout: Duration,
        transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        self.pair
            .receive_logical(timeout, transfer_type, capacity)
            .await
    }

    async fn send_receive_logical(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: TransferType,
        capacity: Option<usize>,
    ) -> Result<Vec<u8>, TransportError> {
        self.exchange(data, timeout, transfer_type, capacity).await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.pair.close().await
    }
}

/// The RX device under the same parent hub as the TX at `tx_path`.
///
/// Without a resolvable TX path there is no pairing rule, so a lone RX on
/// the system is accepted and more than one is refused.
async fn find_rx_sibling(tx_path: Option<&str>) -> Result<nusb::DeviceInfo, TransportError> {
    let devices = nusb::list_devices()
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("enumerating USB devices for the L-Wireless RX: {error}"),
        })?;
    let mut candidates: Vec<nusb::DeviceInfo> = devices
        .filter(|device| {
            device.vendor_id() == WIRELESS_VENDOR_ID && device.product_id() == PID_WIRELESS_RX
        })
        .collect();

    if let Some(parent) = tx_path.and_then(parent_path) {
        candidates.retain(|device| parent_path(&usb_path(device)).as_deref() == Some(&parent));
    }

    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(TransportError::NotFound {
            detail: format!(
                "no L-Wireless RX ({WIRELESS_VENDOR_ID:04X}:{PID_WIRELESS_RX:04X}) under the TX's hub (tx_path={})",
                tx_path.unwrap_or("<unknown>")
            ),
        }),
        found => Err(TransportError::NotFound {
            detail: format!(
                "{found} L-Wireless RX devices match and the TX path ({}) cannot pick one",
                tx_path.unwrap_or("<unknown>")
            ),
        }),
    }
}

/// The host path the scanner records for a device: `{bus}-{port.port...}`.
fn usb_path(device: &nusb::DeviceInfo) -> String {
    let ports = device
        .port_chain()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(".");
    if ports.is_empty() {
        device.bus_id().to_owned()
    } else {
        format!("{}-{ports}", device.bus_id())
    }
}

/// The path of the hub a device hangs off, or `None` at a root port, where
/// "same parent" would match every device on the bus.
fn parent_path(path: &str) -> Option<String> {
    let (parent, _) = path.rsplit_once('.')?;
    Some(parent.to_owned())
}

#[cfg(test)]
mod tests {
    use super::parent_path;

    #[test]
    fn siblings_share_the_path_above_their_port() {
        assert_eq!(parent_path("1-1.2").as_deref(), Some("1-1"));
        assert_eq!(parent_path("3-4.1.2").as_deref(), Some("3-4.1"));
    }

    #[test]
    fn a_root_port_has_no_pairing_parent() {
        assert_eq!(parent_path("1-3"), None);
        assert_eq!(parent_path("1"), None);
    }
}
