use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::super::{InstallPlatformError, PlatformOwnerReceipt, PlatformTransactionRecord, UnitId};
use super::model::{MacosBundleIdentity, MacosBundleRunningDaemon, MacosBundleUnit, error};
use super::swap::SwapPlan;

const RECORD_SCHEMA_VERSION: u32 = 1;
const RECEIPT_SCHEMA_VERSION: u32 = 1;

/// Everything a bundle transaction is prepared against, journaled once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BundleRecord {
    pub(super) installed_bundle: PathBuf,
    /// The candidate's digest, which names its staged copy.
    pub(super) staging_name: String,
    pub(super) candidate: BundleBinding,
    /// The unit whose bundle was installed, if any was.
    pub(super) prior: Option<BundleBinding>,
    /// The daemon that ran when the transaction was prepared.
    pub(super) baseline: Option<MacosBundleRunningDaemon>,
    /// Whether launchd could load the agent when the transaction was
    /// prepared. A transaction closes that gate around each swap and ends
    /// with it back here.
    pub(super) autostart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BundleBinding {
    pub(super) unit: UnitId,
    pub(super) version: String,
    pub(super) identity: MacosBundleIdentity,
    pub(super) daemon_sha256: String,
}

impl BundleBinding {
    pub(super) fn of(unit: &MacosBundleUnit) -> Self {
        Self {
            unit: unit.id().clone(),
            version: unit.version().to_owned(),
            identity: unit.identity().clone(),
            daemon_sha256: unit.daemon_sha256().to_owned(),
        }
    }
}

impl BundleRecord {
    pub(super) fn plan(&self) -> SwapPlan<'_> {
        SwapPlan {
            prior: self.prior.as_ref().map(|prior| &prior.identity),
            candidate: &self.candidate.identity,
        }
    }

    /// The binding of `unit`, when it is one of this transaction's two.
    pub(super) fn binding(&self, unit: &UnitId) -> Option<&BundleBinding> {
        std::iter::once(&self.candidate)
            .chain(self.prior.as_ref())
            .find(|binding| &binding.unit == unit)
    }

    pub(super) fn encode(&self) -> Result<PlatformTransactionRecord, InstallPlatformError> {
        self.validate()?;
        let payload = serde_json::to_vec(self)
            .map_err(|source| error(format!("encode macOS bundle record: {source}")))?;
        PlatformTransactionRecord::macos_app_bundle(RECORD_SCHEMA_VERSION, payload)
            .map_err(|source| error(source.to_string()))
    }

    pub(super) fn decode(record: &PlatformTransactionRecord) -> Result<Self, InstallPlatformError> {
        let PlatformTransactionRecord::MacosAppBundle {
            schema_version,
            payload,
        } = record
        else {
            return Err(error(
                "the install journal belongs to another platform than the macOS app bundle",
            ));
        };
        if *schema_version != RECORD_SCHEMA_VERSION {
            return Err(error(format!(
                "macOS bundle record schema {schema_version} is unsupported"
            )));
        }
        let decoded: Self = serde_json::from_slice(payload)
            .map_err(|source| error(format!("decode macOS bundle record: {source}")))?;
        decoded.validate()?;
        Ok(decoded)
    }

    fn validate(&self) -> Result<(), InstallPlatformError> {
        if self.staging_name != self.candidate.unit.as_str()
            || self.staging_name.starts_with("legacy-")
        {
            return Err(error(
                "a macOS bundle record must stage under its candidate's digest",
            ));
        }
        self.candidate.identity.validate()?;
        if let Some(prior) = &self.prior {
            prior.identity.validate()?;
            if prior.identity.daemon == self.candidate.identity.daemon
                || prior.unit == self.candidate.unit
            {
                return Err(error(
                    "a macOS bundle record cannot tell its two daemons apart",
                ));
            }
        }
        if let Some(baseline) = &self.baseline {
            baseline.validate()?;
            if self.prior.is_none() {
                return Err(error(
                    "a macOS bundle record names a running daemon without an installed prior",
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn encode_receipt(
    daemon: &MacosBundleRunningDaemon,
) -> Result<PlatformOwnerReceipt, InstallPlatformError> {
    daemon.validate()?;
    let payload = serde_json::to_vec(daemon)
        .map_err(|source| error(format!("encode macOS bundle receipt: {source}")))?;
    PlatformOwnerReceipt::macos_app_bundle(RECEIPT_SCHEMA_VERSION, payload)
        .map_err(|source| error(source.to_string()))
}

pub(super) fn decode_receipt(
    receipt: Option<&PlatformOwnerReceipt>,
) -> Result<Option<MacosBundleRunningDaemon>, InstallPlatformError> {
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    let PlatformOwnerReceipt::MacosAppBundle {
        schema_version,
        payload,
    } = receipt
    else {
        return Err(error(
            "the candidate receipt belongs to another platform than the macOS app bundle",
        ));
    };
    if *schema_version != RECEIPT_SCHEMA_VERSION {
        return Err(error(format!(
            "macOS bundle receipt schema {schema_version} is unsupported"
        )));
    }
    let daemon: MacosBundleRunningDaemon = serde_json::from_slice(payload)
        .map_err(|source| error(format!("decode macOS bundle receipt: {source}")))?;
    daemon.validate()?;
    Ok(Some(daemon))
}
