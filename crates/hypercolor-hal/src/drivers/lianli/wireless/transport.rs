//! The controller's transport: the TX bulk device paired with its RX
//! sibling behind the same internal hub, watched for a half that stops
//! taking commands.

use std::time::Duration;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use super::frame::{DONGLE_RESET, USB_PACKET_LEN};
use super::health::{self, DongleHalf};
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
#[must_use]
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
pub struct WirelessControllerTransport {
    pair: Box<dyn Transport>,
    controller: String,
}

impl WirelessControllerTransport {
    /// Watch `pair` (TX on the primary path, RX on the companion path) for
    /// the controller identified by `controller`, the TX's USB path.
    #[must_use]
    pub fn new(pair: Box<dyn Transport>, controller: impl Into<String>) -> Self {
        Self {
            pair,
            controller: controller.into(),
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
            info!(
                controller = %self.controller,
                half = half.name(),
                wedged_for_ms = u64::try_from(wedge.since.elapsed().as_millis()).unwrap_or(u64::MAX),
                stalls = wedge.stalls,
                resets_sent = wedge.resets_sent,
                "L-Wireless {} recovered: it is taking commands again",
                half.name()
            );
        }
    }

    /// Record the wedge, reset the half through its partner, and name the
    /// failure for the caller.
    async fn wedged(&self, half: DongleHalf, timeout_ms: u64) -> TransportError {
        let wedge = health::record_stall(&self.controller, half);
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

        let reset = match self
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
        };

        TransportError::Disconnected {
            detail: format!(
                "L-Wireless {} wedged: it refused a write for {timeout_ms} ms; {reset}. \
                 If it does not come back, replug or power-cycle the controller",
                half.name()
            ),
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
        let result = self.pair.send_with_type(data, transfer_type).await;
        self.watch_write(DongleHalf::for_transfer(transfer_type), result)
            .await
    }

    async fn send_owned_with_type(
        &self,
        data: Vec<u8>,
        transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        let result = self.pair.send_owned_with_type(data, transfer_type).await;
        self.watch_write(DongleHalf::for_transfer(transfer_type), result)
            .await
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
