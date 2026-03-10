//! Linux HIDRAW transport for HID feature-report devices.
//!
//! This path keeps the kernel `usbhid` driver attached and talks to devices
//! through `/dev/hidraw*` via `async-hid`, avoiding exclusive USB interface
//! claims while still using native Linux HID APIs.

use std::cmp::min;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_hid::{
    AsyncHidFeatureHandle, AsyncHidRead, AsyncHidWrite, Device, DeviceFeatureHandle, DeviceId,
    DeviceReader, DeviceWriter, HidBackend, HidError,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use tracing::{debug, trace};

use crate::registry::HidRawReportMode;
use crate::transport::{Transport, TransportError};

const DEFAULT_MAX_PACKET_LEN: usize = 90;

struct HidrawCandidate {
    device: Device,
    device_path: String,
    interface_number: u8,
    serial: Option<String>,
    usb_path: Option<String>,
    usage_page: u16,
    usage: u16,
    summary: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct FeatureReportRequestState {
    transaction_id: Option<u8>,
}

enum HidrawHandleState {
    Feature(DeviceFeatureHandle),
    OutputInput {
        reader: DeviceReader,
        writer: DeviceWriter,
    },
}

/// Linux HIDRAW transport backed by `async-hid`.
pub struct UsbHidRawTransport {
    device_path: String,
    report_id: u8,
    report_mode: HidRawReportMode,
    max_packet_len: usize,
    handles: tokio::sync::Mutex<HidrawHandleState>,
    feature_report_state: Mutex<FeatureReportRequestState>,
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
    #[expect(
        clippy::too_many_arguments,
        reason = "HID device selection needs transport metadata, identity filters, and collection filters together"
    )]
    #[allow(
        clippy::too_many_lines,
        reason = "HID device matching logic is sequential with clear stages — splitting would obscure the flow"
    )]
    pub async fn open(
        vendor_id: u16,
        product_id: u16,
        interface_number: u8,
        report_id: u8,
        report_mode: HidRawReportMode,
        serial: Option<&str>,
        usb_path: Option<&str>,
        usage_page: Option<u16>,
        usage: Option<u16>,
    ) -> Result<Self, TransportError> {
        let backend = HidBackend::default();
        let mut devices = backend.enumerate().await.map_err(map_async_hid_error)?;
        let mut candidates = Vec::new();

        while let Some(device) = devices.next().await {
            if device.vendor_id != vendor_id || device.product_id != product_id {
                continue;
            }

            let device_path = hidraw_sysfs_path(&device.id).display().to_string();
            let candidate_usb_path = hidraw_usb_path(&device.id);
            let Some(candidate_interface_number) = hidraw_interface_number(&device.id) else {
                continue;
            };
            let candidate_serial = device.serial_number.clone();
            let candidate_usage_page = device.usage_page;
            let candidate_usage = device.usage_id;

            let summary = format!(
                "{}(if={}, usage_page=0x{:04X}, usage=0x{:04X}, serial={}, usb_path={})",
                device_path,
                candidate_interface_number,
                candidate_usage_page,
                candidate_usage,
                candidate_serial.as_deref().unwrap_or("<none>"),
                candidate_usb_path.as_deref().unwrap_or("<unknown>")
            );

            candidates.push(HidrawCandidate {
                device,
                device_path,
                interface_number: candidate_interface_number,
                serial: candidate_serial,
                usb_path: candidate_usb_path,
                usage_page: candidate_usage_page,
                usage: candidate_usage,
                summary,
            });
        }

        let original_candidates = candidates
            .iter()
            .take(6)
            .map(|candidate| candidate.summary.clone())
            .collect::<Vec<_>>()
            .join(", ");

        candidates.retain(|candidate| candidate.interface_number == interface_number);

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

        if let Some(usage_page) = usage_page {
            candidates.retain(|candidate| candidate.usage_page == usage_page);
        }

        if let Some(usage) = usage {
            candidates.retain(|candidate| candidate.usage == usage);
        }

        let Some(chosen) = candidates.into_iter().next() else {
            return Err(TransportError::NotFound {
                detail: format!(
                    "hidraw node not found for {:04X}:{:04X} interface {} (serial={}, usb_path={}, usage_page={}, usage={}); candidates=[{}]",
                    vendor_id,
                    product_id,
                    interface_number,
                    serial.unwrap_or("<none>"),
                    usb_path.unwrap_or("<unknown>"),
                    usage_page.map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                    usage.map_or_else(|| "<any>".to_owned(), |value| format!("0x{value:04X}")),
                    original_candidates
                ),
            });
        };

        let selected_usage_page = chosen.usage_page;
        let selected_usage = chosen.usage;
        let handles = open_handles(&chosen.device, report_mode).await?;

        debug!(
            vendor_id = format_args!("{vendor_id:04X}"),
            product_id = format_args!("{product_id:04X}"),
            interface_number,
            report_id = format_args!("0x{report_id:02X}"),
            report_mode = ?report_mode,
            device_path = %chosen.device_path,
            serial = serial.unwrap_or("<none>"),
            usb_path = usb_path.unwrap_or("<unknown>"),
            usage_page = format_args!("0x{selected_usage_page:04X}"),
            usage = format_args!("0x{selected_usage:04X}"),
            "opened hidraw transport"
        );

        Ok(Self {
            device_path: chosen.device_path,
            report_id,
            report_mode,
            max_packet_len: DEFAULT_MAX_PACKET_LEN,
            handles: tokio::sync::Mutex::new(handles),
            feature_report_state: Mutex::new(FeatureReportRequestState::default()),
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
        match self.report_mode {
            HidRawReportMode::FeatureReport => "USB HIDRAW (Feature Report)",
            HidRawReportMode::OutputReport => "USB HIDRAW (Output/Input Report)",
        }
    }

    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let mut handles = self.handles.lock().await;
        let packet = encode_feature_report_packet(data, self.report_id);
        store_feature_report_transaction_id(&self.feature_report_state, &packet);

        match (&mut *handles, self.report_mode) {
            (HidrawHandleState::Feature(handle), HidRawReportMode::FeatureReport) => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidraw feature report send"
                );

                handle
                    .write_feature_report(&packet)
                    .await
                    .map_err(map_async_hid_error)
            }
            (HidrawHandleState::OutputInput { writer, .. }, HidRawReportMode::OutputReport) => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidraw output report send"
                );

                writer
                    .write_output_report(&packet)
                    .await
                    .map_err(map_async_hid_error)
            }
            _ => Err(invalid_handle_state(self.report_mode)),
        }
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let mut handles = self.handles.lock().await;

        match (&mut *handles, self.report_mode) {
            (HidrawHandleState::Feature(handle), HidRawReportMode::FeatureReport) => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    max_packet_len = self.max_packet_len,
                    "hidraw feature report receive requested"
                );

                let transaction_id = load_feature_report_transaction_id(&self.feature_report_state);
                let mut buffer = encode_feature_report_request_buffer(
                    self.report_id,
                    self.max_packet_len,
                    transaction_id,
                );
                let read = handle
                    .read_feature_report(&mut buffer)
                    .await
                    .map_err(map_async_hid_error)?;
                buffer.truncate(read);
                let response =
                    decode_feature_report_packet(&buffer, self.report_id, self.max_packet_len);

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    response_len = response.len(),
                    response_hex = %format_hex_preview(&response, 32),
                    "hidraw feature report received"
                );

                Ok(response)
            }
            (HidrawHandleState::OutputInput { reader, .. }, HidRawReportMode::OutputReport) => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    max_packet_len = self.max_packet_len,
                    timeout_ms = timeout.as_millis(),
                    "hidraw input report receive requested"
                );

                let mut buffer = vec![0_u8; self.max_packet_len.saturating_add(1)];
                let read = tokio::time::timeout(timeout, reader.read_input_report(&mut buffer))
                    .await
                    .map_err(|_| TransportError::Timeout {
                        timeout_ms: saturating_timeout_ms(timeout),
                    })?
                    .map_err(map_async_hid_error)?;
                buffer.truncate(read);

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    response_len = buffer.len(),
                    response_hex = %format_hex_preview(&buffer, 32),
                    "hidraw input report received"
                );

                Ok(buffer)
            }
            _ => Err(invalid_handle_state(self.report_mode)),
        }
    }

    async fn send_receive(
        &self,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.check_open()?;

        let _guard = self.op_lock.lock().await;
        let mut handles = self.handles.lock().await;
        let packet = encode_feature_report_packet(data, self.report_id);
        store_feature_report_transaction_id(&self.feature_report_state, &packet);

        match (&mut *handles, self.report_mode) {
            (HidrawHandleState::Feature(handle), HidRawReportMode::FeatureReport) => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidraw feature report send"
                );

                handle
                    .write_feature_report(&packet)
                    .await
                    .map_err(map_async_hid_error)?;

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    max_packet_len = self.max_packet_len,
                    "hidraw feature report receive requested"
                );

                let transaction_id = load_feature_report_transaction_id(&self.feature_report_state);
                let mut buffer = encode_feature_report_request_buffer(
                    self.report_id,
                    self.max_packet_len,
                    transaction_id,
                );
                let read = handle
                    .read_feature_report(&mut buffer)
                    .await
                    .map_err(map_async_hid_error)?;
                buffer.truncate(read);
                let response =
                    decode_feature_report_packet(&buffer, self.report_id, self.max_packet_len);

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    response_len = response.len(),
                    response_hex = %format_hex_preview(&response, 32),
                    "hidraw feature report received"
                );

                Ok(response)
            }
            (HidrawHandleState::OutputInput { reader, writer }, HidRawReportMode::OutputReport) => {
                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    packet_len = packet.len(),
                    packet_hex = %format_hex_preview(&packet, 32),
                    "hidraw output report send"
                );

                writer
                    .write_output_report(&packet)
                    .await
                    .map_err(map_async_hid_error)?;

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    max_packet_len = self.max_packet_len,
                    timeout_ms = timeout.as_millis(),
                    "hidraw input report receive requested"
                );

                let mut buffer = vec![0_u8; self.max_packet_len.saturating_add(1)];
                let read = tokio::time::timeout(timeout, reader.read_input_report(&mut buffer))
                    .await
                    .map_err(|_| TransportError::Timeout {
                        timeout_ms: saturating_timeout_ms(timeout),
                    })?
                    .map_err(map_async_hid_error)?;
                buffer.truncate(read);

                trace!(
                    device_path = %self.device_path,
                    report_id = format_args!("0x{:02X}", self.report_id),
                    response_len = buffer.len(),
                    response_hex = %format_hex_preview(&buffer, 32),
                    "hidraw input report received"
                );

                Ok(buffer)
            }
            _ => Err(invalid_handle_state(self.report_mode)),
        }
    }

    async fn close(&self) -> Result<(), TransportError> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

async fn open_handles(
    device: &Device,
    report_mode: HidRawReportMode,
) -> Result<HidrawHandleState, TransportError> {
    match report_mode {
        HidRawReportMode::FeatureReport => device
            .open_feature_handle()
            .await
            .map(HidrawHandleState::Feature)
            .map_err(map_async_hid_error),
        HidRawReportMode::OutputReport => {
            let (reader, writer) = device.open().await.map_err(map_async_hid_error)?;
            Ok(HidrawHandleState::OutputInput { reader, writer })
        }
    }
}

fn invalid_handle_state(report_mode: HidRawReportMode) -> TransportError {
    TransportError::IoError {
        detail: format!("hidraw handle state does not match report mode {report_mode:?}"),
    }
}

fn store_feature_report_transaction_id(state: &Mutex<FeatureReportRequestState>, packet: &[u8]) {
    if let Ok(mut state) = state.lock() {
        state.transaction_id = packet.get(2).copied();
    }
}

fn load_feature_report_transaction_id(state: &Mutex<FeatureReportRequestState>) -> Option<u8> {
    state.lock().ok().and_then(|state| state.transaction_id)
}

fn map_async_hid_error(error: HidError) -> TransportError {
    match error {
        HidError::NotConnected => TransportError::NotFound {
            detail: "hidraw device is not connected".to_owned(),
        },
        HidError::Disconnected => TransportError::IoError {
            detail: "hidraw device disconnected".to_owned(),
        },
        HidError::Message(message) => map_error_detail(message.into_owned()),
        HidError::Other(error) => map_error_detail(error.to_string()),
    }
}

fn map_error_detail(detail: String) -> TransportError {
    if detail.to_ascii_lowercase().contains("permission") {
        TransportError::PermissionDenied { detail }
    } else {
        TransportError::IoError { detail }
    }
}

fn hidraw_sysfs_path(device_id: &DeviceId) -> &Path {
    let DeviceId::DevPath(path) = device_id else {
        unreachable!("only DevPath variant exists on Linux")
    };
    path.as_path()
}

fn hidraw_usb_path(device_id: &DeviceId) -> Option<String> {
    hidraw_usb_identity(hidraw_sysfs_path(device_id)).map(|(usb_path, _)| usb_path)
}

fn hidraw_interface_number(device_id: &DeviceId) -> Option<u8> {
    hidraw_usb_identity(hidraw_sysfs_path(device_id)).map(|(_, interface)| interface)
}

fn hidraw_usb_identity(path: &Path) -> Option<(String, u8)> {
    for component in path.components() {
        let value = component.as_os_str().to_string_lossy();
        let Some((usb_path, interface_suffix)) = value.split_once(':') else {
            continue;
        };
        if !usb_path.contains('-') {
            continue;
        }
        let Some((_, interface)) = interface_suffix.rsplit_once('.') else {
            continue;
        };
        let Ok(interface_number) = interface.parse::<u8>() else {
            continue;
        };
        return Some((usb_path.to_owned(), interface_number));
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

#[doc(hidden)]
#[must_use]
pub fn usb_paths_match_for_testing(candidate: &str, requested: &str) -> bool {
    usb_paths_match(candidate, requested)
}

#[doc(hidden)]
#[must_use]
pub fn hidraw_usb_identity_for_testing(path: &str) -> (Option<String>, Option<u8>) {
    match hidraw_usb_identity(Path::new(path)) {
        Some((usb_path, interface)) => (Some(usb_path), Some(interface)),
        None => (None, None),
    }
}

fn normalize_usb_path(path: &str) -> Option<String> {
    let (bus, ports) = path.split_once('-')?;
    let bus = bus.parse::<u16>().ok()?;
    Some(format!("{bus}-{ports}"))
}

fn saturating_timeout_ms(timeout: Duration) -> u64 {
    u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
}

#[doc(hidden)]
#[must_use]
pub fn encode_feature_report_packet(payload: &[u8], report_id: u8) -> Vec<u8> {
    let mut packet = Vec::with_capacity(payload.len().saturating_add(1));
    packet.push(report_id);
    packet.extend_from_slice(payload);
    packet
}

#[doc(hidden)]
#[must_use]
pub fn encode_feature_report_request_buffer(
    report_id: u8,
    max_packet_len: usize,
    transaction_id: Option<u8>,
) -> Vec<u8> {
    let mut buffer = vec![0_u8; max_packet_len.saturating_add(1)];
    buffer[0] = report_id;
    if let Some(transaction_id) = transaction_id {
        if buffer.len() > 2 {
            buffer[2] = transaction_id;
        }
    }
    buffer
}

#[doc(hidden)]
#[must_use]
pub fn decode_feature_report_packet(
    buffer: &[u8],
    report_id: u8,
    expected_payload_len: usize,
) -> Vec<u8> {
    if buffer.len() == expected_payload_len.saturating_add(1)
        && buffer.first().copied() == Some(report_id)
    {
        return buffer[1..].to_vec();
    }

    buffer.to_vec()
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
