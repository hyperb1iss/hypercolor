//! Razer HID packet construction and response parsing.

use std::time::Duration;

use tracing::warn;
use zerocopy::{FromBytes, FromZeros, IntoBytes};

use crate::protocol::{
    CommandBuffer, ProtocolCommand, ProtocolError, ProtocolResponse, ResponseStatus, TransferType,
};

use super::crc::{RAZER_REPORT_LEN, RazerReport, razer_crc};

pub(super) const REPORT_ARGS_LEN: usize = 80;

const RESPONSE_HEADER_LEN: usize = 8;
const RESPONSE_DATA_SIZE_OFFSET: usize = 5;
const RESPONSE_ARGS_OFFSET: usize = 8;

#[derive(Debug, Clone, Copy)]
pub(super) struct CommandTiming {
    pub expects_response: bool,
    pub response_delay: Duration,
    pub post_delay: Duration,
}

impl CommandTiming {
    pub(super) fn new(
        expects_response: bool,
        response_delay: Duration,
        post_delay: Duration,
    ) -> Self {
        Self {
            expects_response,
            response_delay: if expects_response {
                response_delay
            } else {
                Duration::ZERO
            },
            post_delay,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CommandSpec<'a> {
    pub transaction_id: u8,
    pub command_class: u8,
    pub command_id: u8,
    pub args: &'a [u8],
    pub declared_data_size: Option<u8>,
    pub timing: CommandTiming,
}

pub(super) fn build_command(spec: CommandSpec<'_>) -> Option<ProtocolCommand> {
    let report = build_report(spec)?;

    Some(ProtocolCommand {
        data: report.as_bytes().to_vec(),
        expects_response: spec.timing.expects_response,
        response_delay: spec.timing.response_delay,
        post_delay: spec.timing.post_delay,
        transfer_type: TransferType::Primary,
    })
}

pub(super) fn push_command(encoder: &mut CommandBuffer<'_>, spec: CommandSpec<'_>) {
    let Some(report) = build_report(spec) else {
        return;
    };

    encoder.push_struct(
        &report,
        spec.timing.expects_response,
        spec.timing.response_delay,
        spec.timing.post_delay,
        TransferType::Primary,
    );
}

pub(super) fn parse_response(data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
    if data.len() < RAZER_REPORT_LEN {
        return parse_short_response(data);
    }

    // HID transports can leave a report ID prefix attached on some platforms.
    let (report, _remainder) =
        RazerReport::read_from_prefix(data).map_err(|_| ProtocolError::MalformedResponse {
            detail: format!(
                "expected at least {} bytes, got {}",
                RAZER_REPORT_LEN,
                data.len()
            ),
        })?;

    let status = map_status(report.status);
    if status == ResponseStatus::Failed {
        return Err(ProtocolError::DeviceError { status });
    }

    let data_size = usize::from(report.data_size);
    if data_size > REPORT_ARGS_LEN {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("data size exceeds arguments field: {data_size}"),
        });
    }

    Ok(ProtocolResponse {
        status,
        data: report.args[..data_size].to_vec(),
    })
}

fn build_report(spec: CommandSpec<'_>) -> Option<RazerReport> {
    if spec.args.len() > REPORT_ARGS_LEN {
        warn!(
            args_len = spec.args.len(),
            "razer command payload exceeds argument field, dropping packet"
        );
        return None;
    }

    let data_size = spec
        .declared_data_size
        .unwrap_or_else(|| u8::try_from(spec.args.len()).unwrap_or(0));
    if usize::from(data_size) > REPORT_ARGS_LEN {
        warn!(
            data_size,
            "razer command declared data size exceeds argument field, dropping packet"
        );
        return None;
    }

    if spec.args.len() > usize::from(data_size) {
        warn!(
            args_len = spec.args.len(),
            data_size, "razer command arguments exceed declared data size, dropping packet"
        );
        return None;
    }

    let mut report = RazerReport::new_zeroed();
    report.transaction_id = spec.transaction_id;
    report.data_size = data_size;
    report.command_class = spec.command_class;
    report.command_id = spec.command_id;
    report.args[..spec.args.len()].copy_from_slice(spec.args);
    report.crc = razer_crc(&report);

    Some(report)
}

fn parse_short_response(data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
    if data.len() < RESPONSE_HEADER_LEN {
        return Err(ProtocolError::MalformedResponse {
            detail: format!(
                "expected at least {} bytes, got {}",
                RESPONSE_HEADER_LEN,
                data.len()
            ),
        });
    }

    let status = map_status(data[0]);
    if status == ResponseStatus::Failed {
        return Err(ProtocolError::DeviceError { status });
    }

    let data_size = usize::from(data[RESPONSE_DATA_SIZE_OFFSET]);
    if data_size > REPORT_ARGS_LEN {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("data size exceeds arguments field: {data_size}"),
        });
    }

    let payload_end = RESPONSE_ARGS_OFFSET.checked_add(data_size).ok_or_else(|| {
        ProtocolError::MalformedResponse {
            detail: format!("data size exceeds arguments field: {data_size}"),
        }
    })?;
    if data.len() < payload_end {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("expected at least {payload_end} bytes, got {}", data.len()),
        });
    }

    Ok(ProtocolResponse {
        status,
        data: data[RESPONSE_ARGS_OFFSET..payload_end].to_vec(),
    })
}

fn map_status(byte: u8) -> ResponseStatus {
    match byte {
        0x01 => ResponseStatus::Busy,
        0x02 => ResponseStatus::Ok,
        0x04 => ResponseStatus::Timeout,
        0x05 => ResponseStatus::Unsupported,
        _ => ResponseStatus::Failed,
    }
}
