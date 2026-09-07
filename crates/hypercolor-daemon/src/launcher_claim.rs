//! Neutral launcher identity claims and their corroboration rule.
//!
//! A launcher declares who it is through [`SERVICE_IDENTITY_ENV`]. The
//! daemon treats that declaration as a claim, never as authority: each
//! platform arm measures which launchers positively attest to this
//! process (launchctl pid identity, systemd `MainPID`, the SCM dispatcher,
//! the supervising parent) and the claim must agree with the measurement.
//! Absent metadata resolves through bounded inference to the measured
//! authority, with standalone as the residual. Malformed, ambiguous, or
//! contradicted claims reject startup (spec 77 H1.5).

use std::ffi::OsStr;

use anyhow::{Context, Result};
use hypercolor_types::service::{SERVICE_IDENTITY_ENV, ServiceIdentity};

/// Read the neutral launcher declaration from its environment value.
///
/// # Errors
///
/// Returns an error when the value is present but not UTF-8 or not a
/// well-formed declaration.
pub fn read_service_identity_claim(value: Option<&OsStr>) -> Result<Option<ServiceIdentity>> {
    value
        .map(|value| {
            let value = value
                .to_str()
                .with_context(|| format!("{SERVICE_IDENTITY_ENV} is not UTF-8"))?;
            ServiceIdentity::parse_declaration(value)
                .with_context(|| format!("invalid {SERVICE_IDENTITY_ENV} launcher declaration"))
        })
        .transpose()
}

/// Whether two identities name the same launcher for authority purposes.
///
/// The unit label is diagnostic only, so a claim corroborates when its run
/// mode and manager match the measured authority.
#[must_use]
pub fn same_launcher(left: &ServiceIdentity, right: &ServiceIdentity) -> bool {
    left.run_mode == right.run_mode && left.manager == right.manager
}

/// Resolve the launcher identity from the positively attested launchers.
///
/// `attested` lists every launcher the platform authority measured as
/// owning this process. Exactly one must attest; none at all resolves to
/// the standalone residual. A claim, when present, must name the measured
/// launcher. The measured identity is returned, never the claim, so the
/// unit label always comes from the authority.
///
/// # Errors
///
/// Returns an error when more than one launcher attests or when the claim
/// contradicts the measurement.
pub fn resolve_launcher_identity(
    claim: Option<&ServiceIdentity>,
    attested: &[ServiceIdentity],
) -> Result<ServiceIdentity> {
    let authority = match attested {
        [] => ServiceIdentity::STANDALONE,
        [authority] => authority.clone(),
        _ => anyhow::bail!(
            "daemon launcher authority is ambiguous: {}",
            attested
                .iter()
                .map(ServiceIdentity::declaration)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    if let Some(claim) = claim {
        anyhow::ensure!(
            same_launcher(claim, &authority),
            "daemon launcher claim {claim} is not corroborated by {authority} authority"
        );
    }
    Ok(authority)
}

/// Ensure two launcher declarations agree when both are present.
///
/// Launchers that still publish a legacy platform claim beside the neutral
/// one must keep them equal; disagreement is a configuration fault and
/// rejects startup rather than letting one channel silently win.
///
/// # Errors
///
/// Returns an error naming both declarations when they disagree.
pub fn ensure_claims_agree(
    neutral: Option<&ServiceIdentity>,
    legacy: Option<&ServiceIdentity>,
) -> Result<()> {
    if let (Some(neutral), Some(legacy)) = (neutral, legacy) {
        anyhow::ensure!(
            same_launcher(neutral, legacy),
            "conflicting daemon launcher claims: {SERVICE_IDENTITY_ENV}={neutral} legacy={legacy}"
        );
    }
    Ok(())
}

/// Launchers a Linux authority probe measured as owning the process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxLauncherEvidence {
    /// The supervised-parent claim matches the live parent process.
    pub supervised_child: bool,
    /// The systemd user unit reports this process as its `MainPID`.
    pub systemd_user: bool,
    /// The systemd system unit reports this process as its `MainPID`.
    pub systemd_system: bool,
}

impl LinuxLauncherEvidence {
    /// The identities this evidence positively attests.
    #[must_use]
    pub fn attested(self) -> Vec<ServiceIdentity> {
        let mut attested = Vec::new();
        if self.supervised_child {
            attested.push(ServiceIdentity::APP_SIDECAR);
        }
        if self.systemd_user {
            attested.push(ServiceIdentity::systemd_user());
        }
        if self.systemd_system {
            attested.push(ServiceIdentity::systemd_system());
        }
        attested
    }
}

/// Resolve the corroborated Linux identity from a claim and evidence.
///
/// # Errors
///
/// Returns an error when the evidence is ambiguous or the claim names a
/// launcher the evidence does not attest.
pub fn resolve_linux_launcher_identity(
    claim: Option<&ServiceIdentity>,
    evidence: LinuxLauncherEvidence,
) -> Result<ServiceIdentity> {
    resolve_launcher_identity(claim, &evidence.attested())
}

/// Parse `systemctl show --property=MainPID --value` output.
///
/// Accepts the bare value and the `MainPID=<pid>` form; `0` means the unit
/// has no main process and never corroborates anything.
#[must_use]
pub fn parse_systemd_main_pid(output: &str) -> Option<u32> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let value = line.strip_prefix("MainPID=").unwrap_or(line);
    let pid = value.parse::<u32>().ok()?;
    (pid != 0).then_some(pid)
}
