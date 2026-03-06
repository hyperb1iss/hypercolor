//! Linux HIDRAW transport for HID feature-report devices.
//!
//! This path keeps the kernel `usbhid` driver attached and talks to devices
//! through `/dev/hidraw*`, avoiding exclusive USB interface claims.

use std::cmp::min;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use hidapi::HidApi;
use tracing::{debug, trace};

use crate::transport::{Transport, TransportError};

const DEFAULT_MAX_PACKET_LEN: usize = 90;

#[derive(Clone)]
struct HidrawCandidate {
    path: String,
    interface_number: i32,
    serial: Option<String>,
    usb_path: Option<String>,
}

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
            .map(|device| HidrawCandidate {
                path: device.path().to_string_lossy().into_owned(),
                interface_number: device.interface_number(),
                serial: device.serial_number().map(ToOwned::to_owned),
                usb_path: hidraw_usb_path(device.path()),
            })
            .collect::<Vec<_>>();

        let original_candidates = candidates.clone();
        let requested_interface = i32::from(interface_number);
        candidates.retain(|candidate| candidate.interface_number == requested_interface);

        if let Some(serial) = serial {
            candidates.retain(|candidate| candidate.serial.as_deref() == Some(serial));
        }

        if let Some(usb_path) = usb_path {
            candidates.retain(|candidate| {
                candidate
                    .usb_path
                    .as_deref()
                    .is_some_and(|candidate_path| usb_paths_match(candidate_path, usb_path))
            });
        }

        let Some(chosen) = candidates.into_iter().next() else {
            let sample_candidates = original_candidates
                .iter()
                .take(6)
                .map(|candidate| {
                    format!(
                        "{}(if={}, serial={}, usb_path={})",
                        candidate.path,
                        candidate.interface_number,
                        candidate.serial.as_deref().unwrap_or("<none>"),
                        candidate.usb_path.as_deref().unwrap_or("<unknown>")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");

            return Err(TransportError::NotFound {
                detail: format!(
                    "hidraw node not found for {:04X}:{:04X} interface {} (serial={}, usb_path={}); candidates=[{}]",
                    vendor_id,
                    product_id,
                    interface_number,
                    serial.unwrap_or("<none>"),
                    usb_path.unwrap_or("<unknown>"),
                    sample_candidates
                ),
            });
        };

        let device_path = chosen.path;
        let c_path = c_string_for_path(&device_path)?;
        let _ = api
            .open_path(&c_path)
            .map_err(|error| map_hidapi_error(&error))?;

        debug!(
            vendor_id = format_args!("{vendor_id:04X}"),
            product_id = format_args!("{product_id:04X}"),
            interface_number,
            report_id = format_args!("0x{report_id:02X}"),
            device_path = %device_path,
            serial = serial.unwrap_or("<none>"),
            usb_path = usb_path.unwrap_or("<unknown>"),
            "opened hidraw transport"
        );

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
        let report_id = self.report_id;
        let mut packet = data.to_vec();

        // hidapi expects report ID in the first byte. For report_id=0 devices
        // the payload already starts with 0, so send as-is.
        if report_id != 0 && packet.first().copied() != Some(report_id) {
            packet.insert(0, report_id);
        }

        trace!(
            device_path = %self.device_path,
            report_id = format_args!("0x{report_id:02X}"),
            packet_len = packet.len(),
            packet_hex = %format_hex_preview(&packet, 32),
            "hidraw feature report send"
        );

        tokio::task::spawn_blocking(move || {
            let api = HidApi::new().map_err(|error| map_hidapi_error(&error))?;
            let c_path = c_string_for_path(&device_path)?;
            let device = api
                .open_path(&c_path)
                .map_err(|error| map_hidapi_error(&error))?;

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

        debug!(
            device_path = %self.device_path,
            report_id = format_args!("0x{report_id:02X}"),
            max_packet_len,
            "hidraw feature report receive requested"
        );

        let response = tokio::task::spawn_blocking(move || {
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
        })??;

        trace!(
            device_path = %self.device_path,
            report_id = format_args!("0x{report_id:02X}"),
            response_len = response.len(),
            response_hex = %format_hex_preview(&response, 32),
            "hidraw feature report received"
        );

        Ok(response)
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

fn usb_paths_match(candidate: &str, requested: &str) -> bool {
    if candidate == requested {
        return true;
    }

    match (normalize_usb_path(candidate), normalize_usb_path(requested)) {
        (Some(candidate), Some(requested)) => candidate == requested,
        _ => false,
    }
}

fn normalize_usb_path(path: &str) -> Option<String> {
    let (bus, ports) = path.split_once('-')?;
    let bus = bus.parse::<u16>().ok()?;
    Some(format!("{bus}-{ports}"))
}

fn format_hex_preview(bytes: &[u8], max_bytes: usize) -> String {
    let preview_len = min(bytes.len(), max_bytes);
    let mut rendered = bytes
        .iter()
        .take(preview_len)
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ");

    if bytes.len() > preview_len {
        rendered.push_str(&format!(" ... (+{} bytes)", bytes.len() - preview_len));
    }

    if rendered.is_empty() {
        "<empty>".to_owned()
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::usb_paths_match;

    #[test]
    fn usb_paths_match_handles_padded_bus_numbers() {
        assert!(usb_paths_match("3-7", "003-7"));
        assert!(usb_paths_match("003-7", "3-7"));
        assert!(usb_paths_match("03-7.2", "3-7.2"));
    }

    #[test]
    fn usb_paths_match_rejects_different_ports() {
        assert!(!usb_paths_match("3-7", "3-8"));
    }
}
