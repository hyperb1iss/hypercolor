//! Cross-platform HIDAPI transport for HID feature-report and output-report devices.
//!
//! This path talks to devices through the OS HID stack instead of claiming USB
//! interfaces directly, which keeps input devices such as mice and keyboards
//! usable while Hypercolor sends lighting commands.

use std::cmp::min;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use hidapi::{HidApi, HidDevice};
use tracing::{debug, trace};

use crate::registry::HidRawReportMode;
use crate::transport::{Transport, TransportError};

#[cfg(target_os = "linux")]
use std::path::Path;

#[derive(Clone)]
struct HidApiCandidate {
    path: String,
    interface_number: i32,
    serial: Option<String>,
    usb_path: Option<String>,
    usage_page: u16,
    usage: u16,
}

#[derive(Clone, Copy, Debug, Default)]
struct FeatureReportRequestState {
    transaction_id: Option<u8>,
}

/// HIDAPI transport backed by `hidapi`.
pub struct UsbHidApiTransport {
    device_path: String,
    report_id: u8,
    report_mode: HidRawReportMode,
    max_report_len: usize,
    device: Arc<Mutex<HidDevice>>,
    feature_report_state: Arc<Mutex<FeatureReportRequestState>>,
    closed: AtomicBool,
    op_lock: tokio::sync::Mutex<()>,
}

impl UsbHidApiTransport {
    /// Discover and open a HID device path for one USB interface/collection.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] if no matching HID device can be found or
    /// the device cannot be opened.
    #[expect(
        clippy::too_many_arguments,
        reason = "HID device selection needs transport metadata, identity filters, and collection filters together"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "Device discovery, filtering, and diagnostic reporting stay together so probe failures are debuggable"
    )]
    pub fn open(
        vendor_id: u16,
        product_id: u16,
        interface_number: Option<u8>,
        report_id: u8,
        report_mode: HidRawReportMode,
        max_report_len: usize,
        serial: Option<&str>,
        usb_path: Option<&str>,
        usage_page: Option<u16>,
        usage: Option<u16>,
    ) -> Result<Self, TransportError> {
        let api = HidApi::new().map_err(|error| map_hidapi_error(&error))?;

        let mut candidates = api
            .device_list()
            .filter(|device| device.vendor_id() == vendor_id && device.product_id() == product_id)
            .map(|device| HidApiCandidate {
                path: device.path().to_string_lossy().into_owned(),
                interface_number: device.interface_number(),
                serial: device.serial_number().map(ToOwned::to_owned),
                usb_path: hidapi_usb_path(device.path()),
                usage_page: device.usage_page(),
                usage: device.usage(),
            })
            .collect::<Vec<_>>();

        let original_candidates = candidates.clone();
        if let Some(interface_number) = interface_number {
            let requested_interface = i32::from(interface_number);
            candidates.retain(|candidate| candidate.interface_number == requested_interface);
        }

        if let Some(serial) = serial {
            candidates.retain(|candidate| candidate.serial.as_deref() == Some(serial));
        }

        if let Some(usb_path) = usb_path {
            let any_usb_paths = candidates
                .iter()
                .any(|candidate| candidate.usb_path.is_some());
            if any_usb_paths {
                candidates.retain(|candidate| {
                    candidate
                        .usb_path
                        .as_deref()
                        .is_some_and(|candidate_path| usb_paths_match(candidate_path, usb_path))
                });
            }
        }

        if let Some(usage_page) = usage_page {
            candidates.retain(|candidate| candidate.usage_page == usage_page);
        }

        if let Some(usage) = usage {
            candidates.retain(|candidate| candidate.usage == usage);
        }

        let Some(chosen) = candidates.into_iter().next() else {
            let sample_candidates = original_candidates
                .iter()
                .take(6)
                .map(|candidate| {
                    format!(
                        "{}(if={}, usage_page=0x{:04X}, usage=0x{:04X}, serial={}, usb_path={})",
                        candidate.path,
                        candidate.interface_number,
                        candidate.usage_page,
                        candidate.usage,
                        candidate.serial.as_deref().unwrap_or("<none>"),
                        candidate.usb_path.as_deref().unwrap_or("<unknown>")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");

            return Err(TransportError::NotFound {
                detail: format!(
                    "hidapi device not found for {:04X}:{:04X} interface {} (serial={}, usb_path={}, usage_page={}, usage={}); candidates=[{}]",
                    vendor_id,
                    product_id,
                    interface_number.map_or_else(|| "<any>".to_owned(), |value| value.to_string()),
                    serial.unwrap_or("<none>"),
                    usb_path.unwrap_or("<unknown>"),
                    usage_page.map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                    usage.map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                    sample_candidates
                ),
            });
        };

        let device_path = chosen.path;
        let selected_usage_page = chosen.usage_page;
        let selected_usage = chosen.usage;
        let c_path = c_string_for_path(&device_path)?;
        let device = api
            .open_path(&c_path)
            .map_err(|error| map_hidapi_error(&error))?;

        debug!(
            vendor_id = format_args!("{vendor_id:04X}"),
            product_id = format_args!("{product_id:04X}"),
            interface_number = interface_number
                .map_or_else(|| "<any>".to_owned(), |value| value.to_string()),
            report_id = format_args!("0x{report_id:02X}"),
            report_mode = ?report_mode,
            device_path = %device_path,
            serial = serial.unwrap_or("<none>"),
            usb_path = usb_path.unwrap_or("<unknown>"),
            usage_page = format_args!("0x{selected_usage_page:04X}"),
            usage = format_args!("0x{selected_usage:04X}"),
            "opened hidapi transport"
        );

        Ok(Self {
            device_path,
            report_id,
            report_mode,
            max_report_len,
            device: Arc::new(Mutex::new(device)),
            feature_report_state: Arc::new(Mutex::new(FeatureReportRequestState::default())),
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
impl Transport for UsbHidApiTransport {
    fn name(&self) -> &'static str {
        match self.report_mode {
            HidRawReportMode::FeatureReport => "USB HIDAPI (Feature Report)",
            HidRawReportMode::OutputReport => "USB HIDAPI (Output/Input Report)",
            HidRawReportMode::FeatureReportWithReportId => {
                "USB HIDAPI (Feature Report, report ID in payload)"
            }
            HidRawReportMode::OutputReportWithReportId => {
                "USB HIDAPI (Output/Input Report, report ID in payload)"
            }
        }
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_mode(data, self.report_mode).await
    }

    async fn send_with_type(
        &self,
        data: &[u8],
        transfer_type: crate::protocol::TransferType,
    ) -> Result<(), TransportError> {
        self.send_with_mode(data, self.mode_for_transfer(transfer_type))
            .await
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.receive_with_mode(timeout, self.report_mode).await
    }

    async fn receive_with_type(
        &self,
        timeout: Duration,
        transfer_type: crate::protocol::TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.receive_with_mode(timeout, self.mode_for_transfer(transfer_type))
            .await
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_receive_with_mode(data, timeout, self.report_mode)
            .await
    }

    async fn send_receive_with_type(
        &self,
        data: &[u8],
        timeout: Duration,
        transfer_type: crate::protocol::TransferType,
    ) -> Result<Vec<u8>, TransportError> {
        self.send_receive_with_mode(data, timeout, self.mode_for_transfer(transfer_type))
            .await
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

impl UsbHidApiTransport {
    fn mode_for_transfer(&self, transfer_type: crate::protocol::TransferType) -> HidRawReportMode {
        match transfer_type {
            crate::protocol::TransferType::Primary | crate::protocol::TransferType::Bulk => {
                self.report_mode
            }
            crate::protocol::TransferType::HidReport => {
                if report_mode_payload_includes_report_id(self.report_mode) {
                    HidRawReportMode::FeatureReportWithReportId
                } else {
                    HidRawReportMode::FeatureReport
                }
            }
        }
    }

    async fn send_with_mode(
        &self,
        data: &[u8],
        report_mode: HidRawReportMode,
    ) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let device = Arc::clone(&self.device);
        let feature_report_state = Arc::clone(&self.feature_report_state);
        let report_id = self.report_id;
        let packet = encode_hidapi_packet(data, report_id, report_mode);

        match report_mode {
            HidRawReportMode::FeatureReport | HidRawReportMode::FeatureReportWithReportId => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidapi feature report send"
                );

                tokio::task::spawn_blocking(move || {
                    store_feature_report_transaction_id(feature_report_state.as_ref(), &packet);
                    send_feature_report_locked(device.as_ref(), &packet)
                })
                .await
                .map_err(|error| TransportError::IoError {
                    detail: format!("hidapi send task failed: {error}"),
                })?
            }
            HidRawReportMode::OutputReport | HidRawReportMode::OutputReportWithReportId => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidapi output report send"
                );

                tokio::task::spawn_blocking(move || {
                    store_feature_report_transaction_id(feature_report_state.as_ref(), &packet);
                    send_output_report_locked(device.as_ref(), &packet)
                })
                .await
                .map_err(|error| TransportError::IoError {
                    detail: format!("hidapi send task failed: {error}"),
                })?
            }
        }
    }

    async fn receive_with_mode(
        &self,
        timeout: Duration,
        report_mode: HidRawReportMode,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let device = Arc::clone(&self.device);
        let feature_report_state = Arc::clone(&self.feature_report_state);
        let report_id = self.report_id;
        let max_report_len = self.max_report_len;

        match report_mode {
            HidRawReportMode::FeatureReport | HidRawReportMode::FeatureReportWithReportId => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    max_report_len,
                    "hidapi feature report receive requested"
                );

                let response = tokio::task::spawn_blocking(move || {
                    let transaction_id =
                        load_feature_report_transaction_id(feature_report_state.as_ref());
                    receive_feature_report_locked(
                        device.as_ref(),
                        report_id,
                        max_report_len,
                        transaction_id,
                        report_mode_payload_includes_report_id(report_mode),
                    )
                })
                .await
                .map_err(|error| TransportError::IoError {
                    detail: format!("hidapi receive task failed: {error}"),
                })??;

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    response_len = response.len(),
                    response_hex = %format_hex_preview(&response, 32),
                    "hidapi feature report received"
                );

                Ok(response)
            }
            HidRawReportMode::OutputReport | HidRawReportMode::OutputReportWithReportId => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    max_report_len,
                    timeout_ms = timeout.as_millis(),
                    "hidapi input report receive requested"
                );

                let response = tokio::task::spawn_blocking(move || {
                    receive_input_report_locked(device.as_ref(), max_report_len, timeout)
                })
                .await
                .map_err(|error| TransportError::IoError {
                    detail: format!("hidapi receive task failed: {error}"),
                })??;

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    response_len = response.len(),
                    response_hex = %format_hex_preview(&response, 32),
                    "hidapi input report received"
                );

                Ok(response)
            }
        }
    }

    async fn send_receive_with_mode(
        &self,
        data: &[u8],
        timeout: Duration,
        report_mode: HidRawReportMode,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let device = Arc::clone(&self.device);
        let report_id = self.report_id;
        let max_report_len = self.max_report_len;
        let feature_report_state = Arc::clone(&self.feature_report_state);
        let packet = encode_hidapi_packet(data, report_id, report_mode);

        match report_mode {
            HidRawReportMode::FeatureReport | HidRawReportMode::FeatureReportWithReportId => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidapi feature report send"
                );
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    max_report_len,
                    "hidapi feature report receive requested"
                );

                let response = tokio::task::spawn_blocking(move || {
                    send_receive_feature_report_locked(
                        device.as_ref(),
                        &packet,
                        report_id,
                        max_report_len,
                        feature_report_state.as_ref(),
                        report_mode_payload_includes_report_id(report_mode),
                    )
                })
                .await
                .map_err(|error| TransportError::IoError {
                    detail: format!("hidapi send/receive task failed: {error}"),
                })??;

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    response_len = response.len(),
                    response_hex = %format_hex_preview(&response, 32),
                    "hidapi feature report received"
                );

                Ok(response)
            }
            HidRawReportMode::OutputReport | HidRawReportMode::OutputReportWithReportId => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidapi output report send"
                );
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    max_report_len,
                    timeout_ms = timeout.as_millis(),
                    "hidapi input report receive requested"
                );

                let response = tokio::task::spawn_blocking(move || {
                    send_receive_output_report_locked(
                        device.as_ref(),
                        &packet,
                        max_report_len,
                        timeout,
                    )
                })
                .await
                .map_err(|error| TransportError::IoError {
                    detail: format!("hidapi send/receive task failed: {error}"),
                })??;

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{report_id:02X}"),
                    response_len = response.len(),
                    response_hex = %format_hex_preview(&response, 32),
                    "hidapi input report received"
                );

                Ok(response)
            }
        }
    }
}

fn lock_hidapi_device(
    device: &Mutex<HidDevice>,
) -> Result<std::sync::MutexGuard<'_, HidDevice>, TransportError> {
    device.lock().map_err(|_| TransportError::IoError {
        detail: "hidapi device lock poisoned".to_owned(),
    })
}

fn send_feature_report_locked(
    device: &Mutex<HidDevice>,
    packet: &[u8],
) -> Result<(), TransportError> {
    let device = lock_hidapi_device(device)?;
    device
        .send_feature_report(packet)
        .map_err(|error| map_hidapi_error(&error))
}

fn send_output_report_locked(
    device: &Mutex<HidDevice>,
    packet: &[u8],
) -> Result<(), TransportError> {
    let device = lock_hidapi_device(device)?;
    let written = device
        .write(packet)
        .map_err(|error| map_hidapi_error(&error))?;
    if written == packet.len() {
        Ok(())
    } else {
        Err(TransportError::IoError {
            detail: format!(
                "short hidapi output write: wrote {written} of {} bytes",
                packet.len()
            ),
        })
    }
}

fn receive_feature_report_locked(
    device: &Mutex<HidDevice>,
    report_id: u8,
    max_report_len: usize,
    transaction_id: Option<u8>,
    payload_includes_report_id: bool,
) -> Result<Vec<u8>, TransportError> {
    let device = lock_hidapi_device(device)?;
    let mut buffer =
        encode_feature_report_request_buffer(report_id, max_report_len, transaction_id);

    let read = device
        .get_feature_report(&mut buffer)
        .map_err(|error| map_hidapi_error(&error))?;
    buffer.truncate(read);

    Ok(decode_feature_report_packet(
        &buffer,
        report_id,
        max_report_len,
        payload_includes_report_id,
    ))
}

fn send_receive_feature_report_locked(
    device: &Mutex<HidDevice>,
    packet: &[u8],
    report_id: u8,
    max_report_len: usize,
    feature_report_state: &Mutex<FeatureReportRequestState>,
    payload_includes_report_id: bool,
) -> Result<Vec<u8>, TransportError> {
    let device = lock_hidapi_device(device)?;
    store_feature_report_transaction_id(feature_report_state, packet);
    device
        .send_feature_report(packet)
        .map_err(|error| map_hidapi_error(&error))?;

    let transaction_id = load_feature_report_transaction_id(feature_report_state);
    let mut buffer =
        encode_feature_report_request_buffer(report_id, max_report_len, transaction_id);

    let read = device
        .get_feature_report(&mut buffer)
        .map_err(|error| map_hidapi_error(&error))?;
    buffer.truncate(read);

    Ok(decode_feature_report_packet(
        &buffer,
        report_id,
        max_report_len,
        payload_includes_report_id,
    ))
}

fn receive_input_report_locked(
    device: &Mutex<HidDevice>,
    max_report_len: usize,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    let device = lock_hidapi_device(device)?;
    let timeout_ms = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    let mut buffer = vec![0_u8; max_report_len];
    let read = device
        .read_timeout(&mut buffer, timeout_ms)
        .map_err(|error| map_hidapi_error(&error))?;
    buffer.truncate(read);
    Ok(buffer)
}

fn send_receive_output_report_locked(
    device: &Mutex<HidDevice>,
    packet: &[u8],
    max_report_len: usize,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    send_output_report_locked(device, packet)?;
    receive_input_report_locked(device, max_report_len, timeout)
}

fn c_string_for_path(path: &str) -> Result<CString, TransportError> {
    CString::new(path).map_err(|error| TransportError::IoError {
        detail: format!("invalid hidapi path '{path}': {error}"),
    })
}

fn encode_hidapi_packet(payload: &[u8], report_id: u8, report_mode: HidRawReportMode) -> Vec<u8> {
    if report_mode_payload_includes_report_id(report_mode) {
        if payload.is_empty() {
            return vec![report_id];
        }
        return payload.to_vec();
    }

    let mut packet = Vec::with_capacity(payload.len().saturating_add(1));
    packet.push(report_id);
    packet.extend_from_slice(payload);
    packet
}

#[doc(hidden)]
#[must_use]
pub fn encode_hidapi_packet_for_testing(
    payload: &[u8],
    report_id: u8,
    report_mode: HidRawReportMode,
) -> Vec<u8> {
    encode_hidapi_packet(payload, report_id, report_mode)
}

fn encode_feature_report_request_buffer(
    report_id: u8,
    max_report_len: usize,
    transaction_id: Option<u8>,
) -> Vec<u8> {
    let mut buffer = vec![0_u8; max_report_len];
    if buffer.is_empty() {
        return buffer;
    }
    buffer[0] = report_id;
    if let Some(transaction_id) = transaction_id
        && buffer.len() > 2
    {
        buffer[2] = transaction_id;
    }
    buffer
}

#[doc(hidden)]
#[must_use]
pub fn encode_feature_report_request_buffer_for_testing(
    report_id: u8,
    max_report_len: usize,
    transaction_id: Option<u8>,
) -> Vec<u8> {
    encode_feature_report_request_buffer(report_id, max_report_len, transaction_id)
}

fn store_feature_report_transaction_id(state: &Mutex<FeatureReportRequestState>, packet: &[u8]) {
    if let Ok(mut state) = state.lock() {
        state.transaction_id = packet.get(2).copied();
    }
}

fn load_feature_report_transaction_id(state: &Mutex<FeatureReportRequestState>) -> Option<u8> {
    state.lock().ok().and_then(|state| state.transaction_id)
}

fn decode_feature_report_packet(
    buffer: &[u8],
    report_id: u8,
    expected_report_len: usize,
    payload_includes_report_id: bool,
) -> Vec<u8> {
    if !payload_includes_report_id
        && buffer.len() == expected_report_len
        && buffer.first().copied() == Some(report_id)
    {
        return buffer[1..].to_vec();
    }

    buffer.to_vec()
}

#[doc(hidden)]
#[must_use]
pub fn decode_feature_report_packet_for_testing(
    buffer: &[u8],
    report_id: u8,
    expected_report_len: usize,
    payload_includes_report_id: bool,
) -> Vec<u8> {
    decode_feature_report_packet(
        buffer,
        report_id,
        expected_report_len,
        payload_includes_report_id,
    )
}

fn report_mode_payload_includes_report_id(report_mode: HidRawReportMode) -> bool {
    matches!(
        report_mode,
        HidRawReportMode::FeatureReportWithReportId | HidRawReportMode::OutputReportWithReportId
    )
}

fn map_hidapi_error(error: &hidapi::HidError) -> TransportError {
    let detail = error.to_string();
    if detail.to_ascii_lowercase().contains("permission") {
        return TransportError::PermissionDenied { detail };
    }

    TransportError::IoError { detail }
}

#[cfg(target_os = "linux")]
fn hidapi_usb_path(path: &CStr) -> Option<String> {
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

#[cfg(not(target_os = "linux"))]
fn hidapi_usb_path(_path: &CStr) -> Option<String> {
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
        use std::fmt::Write;
        let _ = write!(rendered, " ... (+{} bytes)", bytes.len() - preview_len);
    }

    if rendered.is_empty() {
        "<empty>".to_owned()
    } else {
        rendered
    }
}
