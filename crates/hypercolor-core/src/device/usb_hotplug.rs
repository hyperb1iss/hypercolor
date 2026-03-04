//! USB hotplug event channel.

use hypercolor_hal::database::DeviceDescriptor;

/// USB hotplug event emitted by the monitor.
#[derive(Debug, Clone)]
pub enum UsbHotplugEvent {
    /// A known USB device arrived.
    Arrived {
        /// Vendor ID.
        vendor_id: u16,
        /// Product ID.
        product_id: u16,
        /// Matched descriptor.
        descriptor: &'static DeviceDescriptor,
    },

    /// A USB device was removed.
    Removed {
        /// Vendor ID.
        vendor_id: u16,
        /// Product ID.
        product_id: u16,
    },
}

/// USB hotplug monitor wrapper.
///
/// The nusb-backed watcher integration is added in a follow-up milestone.
pub struct UsbHotplugMonitor {
    event_tx: tokio::sync::broadcast::Sender<UsbHotplugEvent>,
}

impl UsbHotplugMonitor {
    /// Create a monitor with a bounded event channel.
    #[must_use]
    pub fn new(buffer: usize) -> Self {
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(buffer.max(1));
        Self { event_tx }
    }

    /// Subscribe to hotplug events.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<UsbHotplugEvent> {
        self.event_tx.subscribe()
    }

    /// Emit an arrival event.
    pub fn emit_arrived(
        &self,
        vendor_id: u16,
        product_id: u16,
        descriptor: &'static DeviceDescriptor,
    ) {
        let _ = self.event_tx.send(UsbHotplugEvent::Arrived {
            vendor_id,
            product_id,
            descriptor,
        });
    }

    /// Emit a removal event.
    pub fn emit_removed(&self, vendor_id: u16, product_id: u16) {
        let _ = self.event_tx.send(UsbHotplugEvent::Removed {
            vendor_id,
            product_id,
        });
    }
}
