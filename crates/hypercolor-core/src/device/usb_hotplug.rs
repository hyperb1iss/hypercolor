//! USB hotplug event channel.

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;

use anyhow::{Context, Result};
use futures_core::Stream;
use hypercolor_hal::database::{DeviceDescriptor, ProtocolDatabase};
use nusb::hotplug::HotplugEvent;
use tracing::{debug, warn};

use super::unclaimed::UnclaimedDeviceStore;
use super::usb_scanner::usb_observation;

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
    unclaimed: Option<UnclaimedDeviceStore>,
}

impl UsbHotplugMonitor {
    /// Create a monitor with a bounded event channel.
    #[must_use]
    pub fn new(buffer: usize) -> Self {
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(buffer.max(1));
        Self {
            event_tx,
            unclaimed: None,
        }
    }

    /// Patch `store` on every arrival and removal, claimed or not.
    #[must_use]
    pub fn with_unclaimed_store(mut self, store: UnclaimedDeviceStore) -> Self {
        self.unclaimed = Some(store);
        self
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
        let unclaimed = self.unclaimed.clone();
        let handle = tokio::spawn(async move {
            run_hotplug_loop(event_tx, unclaimed, watch).await;
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

/// What the watcher remembers about one attached device, so a removal
/// (which only carries the platform id) can still name what left.
struct SeenDevice {
    vendor_id: u16,
    product_id: u16,
    /// [`UsbObservation::key`](super::unclaimed::UsbObservation::key) for
    /// the unclaimed inventory patch on removal.
    observation_key: String,
}

async fn run_hotplug_loop(
    event_tx: tokio::sync::broadcast::Sender<UsbHotplugEvent>,
    unclaimed: Option<UnclaimedDeviceStore>,
    mut watch: nusb::hotplug::HotplugWatch,
) {
    let mut known_devices = enumerate_known_devices().await;

    while let Some(event) = next_hotplug_event(&mut watch).await {
        match event {
            HotplugEvent::Connected(device) => {
                let vendor_id = device.vendor_id();
                let product_id = device.product_id();
                let descriptor = ProtocolDatabase::lookup(vendor_id, product_id);
                let observation = usb_observation(&device, descriptor);

                known_devices.insert(
                    device.id(),
                    SeenDevice {
                        vendor_id,
                        product_id,
                        observation_key: observation.key(),
                    },
                );
                if let Some(store) = &unclaimed {
                    store.upsert(observation);
                }

                if let Some(descriptor) = descriptor {
                    let _ = event_tx.send(UsbHotplugEvent::Arrived {
                        vendor_id,
                        product_id,
                        descriptor,
                    });
                }
            }
            HotplugEvent::Disconnected(device_id) => {
                let Some(seen) = known_devices.remove(&device_id) else {
                    continue;
                };
                if let Some(store) = &unclaimed {
                    store.remove(&seen.observation_key);
                }

                if ProtocolDatabase::lookup(seen.vendor_id, seen.product_id).is_some() {
                    let _ = event_tx.send(UsbHotplugEvent::Removed {
                        vendor_id: seen.vendor_id,
                        product_id: seen.product_id,
                    });
                }
            }
        }
    }

    debug!("USB hotplug watcher exited");
}

/// Every attached device at watcher start, claimed or not, so a later
/// removal of an unclaimed device can still patch the inventory.
async fn enumerate_known_devices() -> HashMap<nusb::DeviceId, SeenDevice> {
    let mut known_devices = HashMap::new();
    match nusb::list_devices().await {
        Ok(devices) => {
            for device in devices {
                let vendor_id = device.vendor_id();
                let product_id = device.product_id();
                let descriptor = ProtocolDatabase::lookup(vendor_id, product_id);
                known_devices.insert(
                    device.id(),
                    SeenDevice {
                        vendor_id,
                        product_id,
                        observation_key: usb_observation(&device, descriptor).key(),
                    },
                );
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
