//! The controller's transport: the TX bulk device paired with its RX
//! sibling behind the same internal hub.

use tracing::debug;

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
        debug!(
            tx_path = request.usb_path.as_deref().unwrap_or("<unknown>"),
            rx_path = %usb_path(&rx_info),
            "paired L-Wireless TX with its RX sibling"
        );
        Ok(Box::new(CompanionTransport::new(
            "lianli-wireless",
            Box::new(tx),
            Box::new(rx),
        )) as Box<dyn Transport>)
    })
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
