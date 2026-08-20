use super::super::{InstallPlatformError, PlatformOwnerReceipt, PlatformTransactionRecord};
use super::model::{
    LINUX_RECEIPT_SCHEMA_VERSION, LINUX_RECORD_SCHEMA_VERSION, LinuxOwnerReceipt, LinuxRecord,
    error,
};

pub(super) fn decode_record(
    record: &PlatformTransactionRecord,
) -> Result<LinuxRecord, InstallPlatformError> {
    let PlatformTransactionRecord::Linux {
        schema_version,
        payload,
    } = record
    else {
        return Err(error("Linux adapter received a non-Linux platform record"));
    };
    if *schema_version != LINUX_RECORD_SCHEMA_VERSION {
        return Err(error("unsupported Linux platform record schema"));
    }
    serde_json::from_slice(payload)
        .map_err(|source| error(format!("invalid strict Linux platform record: {source}")))
}

pub(super) fn decode_receipt(
    receipt: Option<&PlatformOwnerReceipt>,
) -> Result<Option<LinuxOwnerReceipt>, InstallPlatformError> {
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let PlatformOwnerReceipt::Linux {
        schema_version,
        payload,
    } = receipt
    else {
        return Err(error("Linux adapter received a non-Linux owner receipt"));
    };
    if *schema_version != LINUX_RECEIPT_SCHEMA_VERSION {
        return Err(error("unsupported Linux owner receipt schema"));
    }
    serde_json::from_slice(payload)
        .map(Some)
        .map_err(|source| error(format!("invalid strict Linux owner receipt: {source}")))
}
