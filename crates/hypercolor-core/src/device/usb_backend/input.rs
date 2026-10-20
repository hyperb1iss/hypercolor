//! Input from driver-owned USB transports, published under the host's lease.
//!
//! A transport that decodes device input gets one [`UsbDeviceInput`] when it
//! opens. The backend attaches the host lease only once connect succeeds, so
//! a device that never finishes connecting never shows up as an input
//! source, and retires it at disconnect. Between those points the transport
//! decides when held input can no longer be trusted: an interruption
//! supersedes the lease, and closing input retires it, so the host cancels
//! every hold whose lift will never be seen.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use hypercolor_driver_api::{DeviceInputPublisher, DeviceInputSink};
use hypercolor_hal::transport::input::TransportInputSink;
use hypercolor_types::device::DeviceId;
use hypercolor_types::device_input::DeviceInputEdge;

/// Bridge from one transport's decoded input to the host's device lease.
pub(super) struct UsbDeviceInput {
    sink: Arc<dyn DeviceInputSink>,
    device_id: DeviceId,
    label: Arc<str>,
    lease: Mutex<Lease>,
}

enum Lease {
    /// Connect has not finished, so input is dropped.
    Pending,
    Live(Box<dyn DeviceInputPublisher>),
    /// Input ended or the device disconnected; nothing attaches again.
    Closed,
}

impl UsbDeviceInput {
    pub(super) fn new(sink: Arc<dyn DeviceInputSink>, device_id: DeviceId, label: &str) -> Self {
        Self {
            sink,
            device_id,
            label: Arc::from(label),
            lease: Mutex::new(Lease::Pending),
        }
    }

    /// Start publishing under a fresh lease once the device is connected.
    ///
    /// Does nothing after [`Self::close`], so a transport whose input
    /// already ended cannot bring the lease back.
    pub(super) fn activate(&self) {
        let mut lease = self.lock();
        if matches!(*lease, Lease::Pending) {
            *lease = Lease::Live(self.sink.attach(self.device_id, &self.label));
        }
    }

    /// Retire the lease for good, cancelling whatever the device held.
    pub(super) fn close(&self) {
        let retired = std::mem::replace(&mut *self.lock(), Lease::Closed);
        drop(retired);
    }

    fn lock(&self) -> MutexGuard<'_, Lease> {
        self.lease.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TransportInputSink for UsbDeviceInput {
    fn publish(&self, edges: &[DeviceInputEdge]) {
        let mut lease = self.lock();
        if let Lease::Live(publisher) = &*lease
            && !publisher.publish(edges)
        {
            // A newer connection of the same device owns its input now.
            *lease = Lease::Closed;
        }
    }

    fn interrupted(&self) {
        let mut lease = self.lock();
        let Lease::Live(publisher) = &mut *lease else {
            return;
        };
        // An empty publish only asks whether the lease is still current. A
        // lease a newer connection superseded must not take the device back.
        if publisher.publish(&[]) {
            // Attaching again supersedes this lease, so the host cancels its
            // holds before the replacement publishes anything.
            *publisher = self.sink.attach(self.device_id, &self.label);
        } else {
            *lease = Lease::Closed;
        }
    }

    fn closed(&self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
