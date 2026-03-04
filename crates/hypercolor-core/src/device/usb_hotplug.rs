//! USB hotplug event channel.

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;

use anyhow::{Context, Result};
use futures_core::Stream;
use hypercolor_hal::database::{DeviceDescriptor, ProtocolDatabase};
use nusb::hotplug::HotplugEvent;
use tracing::{debug, warn};

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

/// Background hotplug watcher task.
///
/// Dropping this handle aborts the watcher.
pub struct UsbHotplugTask {
    handle: tokio::task::JoinHandle<()>,
}

impl UsbHotplugTask {
    /// Abort the watcher task immediately.
    pub fn abort(&self) {
        self.handle.abort();
    }
}

impl Drop for UsbHotplugTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// USB hotplug monitor wrapper.
///
/// Uses `nusb::watch_devices()` and emits HAL-filtered arrival/removal events.
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

    /// Start background USB hotplug monitoring.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform hotplug watcher cannot be created.
    pub fn start(&self) -> Result<UsbHotplugTask> {
        let watch = nusb::watch_devices().context("failed to start USB hotplug watcher")?;
        let event_tx = self.event_tx.clone();
        let handle = tokio::spawn(async move {
            run_hotplug_loop(event_tx, watch).await;
        });
        Ok(UsbHotplugTask { handle })
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

async fn run_hotplug_loop(
    event_tx: tokio::sync::broadcast::Sender<UsbHotplugEvent>,
    mut watch: nusb::hotplug::HotplugWatch,
) {
    let mut known_devices = enumerate_known_devices().await;

    while let Some(event) = next_hotplug_event(&mut watch).await {
        match event {
            HotplugEvent::Connected(device) => {
                let vendor_id = device.vendor_id();
                let product_id = device.product_id();

                known_devices.insert(device.id(), (vendor_id, product_id));

                if let Some(descriptor) = ProtocolDatabase::lookup(vendor_id, product_id) {
                    let _ = event_tx.send(UsbHotplugEvent::Arrived {
                        vendor_id,
                        product_id,
                        descriptor,
                    });
                }
            }
            HotplugEvent::Disconnected(device_id) => {
                let Some((vendor_id, product_id)) = known_devices.remove(&device_id) else {
                    continue;
                };

                if ProtocolDatabase::lookup(vendor_id, product_id).is_some() {
                    let _ = event_tx.send(UsbHotplugEvent::Removed {
                        vendor_id,
                        product_id,
                    });
                }
            }
        }
    }

    debug!("USB hotplug watcher exited");
}

async fn enumerate_known_devices() -> HashMap<nusb::DeviceId, (u16, u16)> {
    let mut known_devices = HashMap::new();
    match nusb::list_devices().await {
        Ok(devices) => {
            for device in devices {
                let vendor_id = device.vendor_id();
                let product_id = device.product_id();
                if ProtocolDatabase::lookup(vendor_id, product_id).is_some() {
                    known_devices.insert(device.id(), (vendor_id, product_id));
                }
            }
        }
        Err(error) => {
            warn!(error = %error, "failed to enumerate USB devices for hotplug baseline");
        }
    }
    known_devices
}

async fn next_hotplug_event(watch: &mut nusb::hotplug::HotplugWatch) -> Option<HotplugEvent> {
    poll_fn(|cx| Pin::new(&mut *watch).poll_next(cx)).await
}
