//! Linux HIDRAW transport for HID feature-report devices.
//!
//! This path keeps the kernel `usbhid` driver attached and talks to devices
//! through `/dev/hidraw*`, avoiding exclusive USB interface claims.

use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use hidapi::HidApi;

use crate::transport::{Transport, TransportError};

const DEFAULT_MAX_PACKET_LEN: usize = 90;

/// HIDRAW transport backed by `hidapi` on Linux.
pub struct UsbHidRawTransport {
    device_path: String,
    report_id: u8,
    max_packet_len: usize,
    closed: AtomicBool,
    op_lock: tokio::sync::Mutex<()>,
}

impl UsbHidRawTransport {
    /// Discover and open a HIDRAW device path for one USB interface.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] if no matching hidraw node can be found or
    /// the node cannot be opened.
    pub fn open(
        vendor_id: u16,
        product_id: u16,
        interface_number: u8,
        report_id: u8,
        serial: Option<&str>,
        usb_path: Option<&str>,
    ) -> Result<Self, TransportError> {
        let api = HidApi::new().map_err(|error| map_hidapi_error(&error))?;

        let mut candidates = api
            .device_list()
            .filter(|device| device.vendor_id() == vendor_id && device.product_id() == product_id)
            .collect::<Vec<_>>();

        let requested_interface = i32::from(interface_number);
        candidates.retain(|candidate| candidate.interface_number() == requested_interface);

        if let Some(serial) = serial {
            candidates.retain(|candidate| candidate.serial_number().is_some_and(|s| s == serial));
        }

        if let Some(usb_path) = usb_path {
            candidates.retain(|candidate| {
                hidraw_usb_path(candidate.path())
                    .is_some_and(|candidate_path| candidate_path == usb_path)
            });
        }

        let Some(chosen) = candidates.into_iter().next() else {
            return Err(TransportError::NotFound {
                detail: format!(
                    "hidraw node not found for {:04X}:{:04X} interface {} (serial={}, usb_path={})",
                    vendor_id,
                    product_id,
                    interface_number,
                    serial.unwrap_or("<none>"),
                    usb_path.unwrap_or("<unknown>")
                ),
            });
        };

        let device_path = chosen.path().to_string_lossy().into_owned();
        let c_path = c_string_for_path(&device_path)?;
        let _ = api
            .open_path(&c_path)
            .map_err(|error| map_hidapi_error(&error))?;

        Ok(Self {
            device_path,
            report_id,
            max_packet_len: DEFAULT_MAX_PACKET_LEN,
            closed: AtomicBool::new(false),
            op_lock: tokio::sync::Mutex::new(()),
        })
    }

    fn check_open(&self) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Closed);
        }
        Ok(())
    }
}

#[async_trait]
impl Transport for UsbHidRawTransport {
    fn name(&self) -> &'static str {
        "USB HIDRAW (Feature Report)"
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let device_path = self.device_path.clone();
        let payload = data.to_vec();
        let report_id = self.report_id;

        tokio::task::spawn_blocking(move || {
            let api = HidApi::new().map_err(|error| map_hidapi_error(&error))?;
            let c_path = c_string_for_path(&device_path)?;
            let device = api
                .open_path(&c_path)
                .map_err(|error| map_hidapi_error(&error))?;

            // hidapi expects report ID in the first byte. For report_id=0 devices
            // the payload already starts with 0, so send as-is.
            let mut packet = payload;
            if report_id != 0 && packet.first().copied() != Some(report_id) {
                packet.insert(0, report_id);
            }

            device
                .send_feature_report(&packet)
                .map_err(|error| map_hidapi_error(&error))?;

            Ok(())
        })
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("hidraw send task failed: {error}"),
        })?
    }

    async fn receive(&self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let device_path = self.device_path.clone();
        let report_id = self.report_id;
        let max_packet_len = self.max_packet_len;

        tokio::task::spawn_blocking(move || {
            let api = HidApi::new().map_err(|error| map_hidapi_error(&error))?;
            let c_path = c_string_for_path(&device_path)?;
            let device = api
                .open_path(&c_path)
                .map_err(|error| map_hidapi_error(&error))?;

            let mut buffer = vec![0_u8; max_packet_len];
            if report_id != 0 {
                buffer[0] = report_id;
            }

            let read = device
                .get_feature_report(&mut buffer)
                .map_err(|error| map_hidapi_error(&error))?;
            buffer.truncate(read);

            if report_id != 0 && !buffer.is_empty() && buffer[0] == report_id {
                buffer.remove(0);
            }

            Ok(buffer)
        })
        .await
        .map_err(|error| TransportError::IoError {
            detail: format!("hidraw receive task failed: {error}"),
        })?
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

fn c_string_for_path(path: &str) -> Result<CString, TransportError> {
    CString::new(path).map_err(|error| TransportError::IoError {
        detail: format!("invalid hidraw path '{path}': {error}"),
    })
}

fn map_hidapi_error(error: &hidapi::HidError) -> TransportError {
    let detail = error.to_string();
    if detail.to_ascii_lowercase().contains("permission") {
        return TransportError::PermissionDenied { detail };
    }

    TransportError::IoError { detail }
}

fn hidraw_usb_path(path: &CStr) -> Option<String> {
    let node = Path::new(path.to_str().ok()?).file_name()?.to_str()?;
    let sysfs = Path::new("/sys/class/hidraw").join(node).join("device");
    let canonical = std::fs::canonicalize(sysfs).ok()?;

    for component in canonical.components() {
        let value = component.as_os_str().to_string_lossy();
        if let Some((usb_path, _interface)) = value.split_once(':')
            && usb_path.contains('-')
        {
            return Some(usb_path.to_owned());
        }
    }

    None
}
