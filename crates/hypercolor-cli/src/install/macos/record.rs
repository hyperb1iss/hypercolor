use super::super::{InstallPlatformError, PlatformOwnerReceipt, PlatformTransactionRecord};
use super::model::{
    MACOS_RECEIPT_SCHEMA_VERSION, MACOS_RECORD_SCHEMA_VERSION, MacosOwnerReceipt, MacosRecord,
    error,
};

pub(super) fn decode_record(
    record: &PlatformTransactionRecord,
) -> Result<MacosRecord, InstallPlatformError> {
    let PlatformTransactionRecord::Macos {
        schema_version,
        payload,
    } = record
    else {
        return Err(error("macOS adapter received a non-macOS platform record"));
    };
    if *schema_version != MACOS_RECORD_SCHEMA_VERSION {
        return Err(error("unsupported macOS platform record schema"));
    }
    serde_json::from_slice(payload)
        .map_err(|source| error(format!("invalid strict macOS platform record: {source}")))
}

pub(super) fn decode_receipt(
    receipt: Option<&PlatformOwnerReceipt>,
) -> Result<Option<MacosOwnerReceipt>, InstallPlatformError> {
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let PlatformOwnerReceipt::Macos {
        schema_version,
        payload,
    } = receipt
    else {
        return Err(error("macOS adapter received a non-macOS owner receipt"));
    };
    if *schema_version != MACOS_RECEIPT_SCHEMA_VERSION {
        return Err(error("unsupported macOS owner receipt schema"));
    }
    serde_json::from_slice(payload)
        .map(Some)
        .map_err(|source| error(format!("invalid strict macOS owner receipt: {source}")))
}
