//! The journaled record of one Windows transaction.

use serde::{Deserialize, Serialize};

use super::model::{
    WindowsFirewallObservation, WindowsFirewallRule, WindowsRuntime, WindowsUnitBinding, error,
    member_path,
};
use crate::{InstallPlatformError, PlatformOwnerReceipt, PlatformTransactionRecord, UnitId};

const RECORD_SCHEMA_VERSION: u32 = 1;
const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Everything a transaction is prepared against, journaled once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WindowsRecord {
    pub(super) units_root: String,
    pub(super) candidate: WindowsUnitBinding,
    /// The unit that was active, if any was.
    pub(super) prior: Option<WindowsUnitBinding>,
    /// The app and daemon that ran when the transaction was prepared.
    pub(super) baseline: Option<WindowsRuntime>,
    /// The broker as it was registered; `None` when it was not registered.
    pub(super) broker: Option<WindowsBrokerBaseline>,
    /// The firewall rules as they were; `None` when no firewall answered.
    pub(super) firewall: Option<WindowsFirewallObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WindowsBrokerBaseline {
    /// The program its `ImagePath` named, unquoted.
    pub(super) program: String,
    pub(super) running: bool,
}

impl WindowsRecord {
    /// Where the candidate keeps `member`.
    pub(super) fn candidate_path(&self, member: &str) -> String {
        member_path(&self.units_root, &self.candidate.unit, member)
    }

    /// The program the candidate's `rule` allows.
    pub(super) fn candidate_rule(&self, rule: WindowsFirewallRule) -> String {
        self.candidate_path(rule.member())
    }

    /// The binding of `unit`, when it is one of this transaction's two.
    pub(super) fn binding(&self, unit: &UnitId) -> Option<&WindowsUnitBinding> {
        std::iter::once(&self.candidate)
            .chain(self.prior.as_ref())
            .find(|binding| &binding.unit == unit)
    }

    pub(super) fn encode(&self) -> Result<PlatformTransactionRecord, InstallPlatformError> {
        let payload = serde_json::to_vec(self)
            .map_err(|source| error(format!("encode Windows record: {source}")))?;
        PlatformTransactionRecord::windows_machine(RECORD_SCHEMA_VERSION, payload)
            .map_err(|source| error(source.to_string()))
    }

    pub(super) fn decode(record: &PlatformTransactionRecord) -> Result<Self, InstallPlatformError> {
        let PlatformTransactionRecord::WindowsMachine {
            schema_version,
            payload,
        } = record
        else {
            return Err(error(
                "the install journal belongs to another platform than Windows",
            ));
        };
        if *schema_version != RECORD_SCHEMA_VERSION {
            return Err(error(format!(
                "unsupported Windows record schema {schema_version}"
            )));
        }
        let record: Self = serde_json::from_slice(payload)
            .map_err(|source| error(format!("decode Windows record: {source}")))?;
        record.candidate.validate()?;
        if let Some(prior) = &record.prior {
            prior.validate()?;
        }
        Ok(record)
    }
}

pub(super) fn encode_receipt(
    runtime: &WindowsRuntime,
) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
    let payload = serde_json::to_vec(runtime)
        .map_err(|source| error(format!("encode Windows receipt: {source}")))?;
    PlatformOwnerReceipt::windows_machine(RECEIPT_SCHEMA_VERSION, payload)
        .map_err(|source| error(source.to_string()))
}

pub(super) fn decode_receipt(
    receipt: Option<&PlatformOwnerReceipt>,
) -> Result<Option<WindowsRuntime>, InstallPlatformError> {
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let PlatformOwnerReceipt::WindowsMachine {
        schema_version,
        payload,
    } = receipt
    else {
        return Err(error(
            "the owner receipt belongs to another platform than Windows",
        ));
    };
    if *schema_version != RECEIPT_SCHEMA_VERSION {
        return Err(error(format!(
            "unsupported Windows receipt schema {schema_version}"
        )));
    }
    serde_json::from_slice(payload)
        .map(Some)
        .map_err(|source| error(format!("decode Windows receipt: {source}")))
}
