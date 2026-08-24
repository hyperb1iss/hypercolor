use std::path::Path;

#[cfg(feature = "screen-capture")]
use std::process::Command;
#[cfg(feature = "screen-capture")]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
#[cfg(feature = "screen-capture")]
use core_foundation::{base::TCFType, data::CFData};
use hypercolor_macos_owner::MacosDaemonOwner;
#[cfg(feature = "screen-capture")]
use security_framework::os::macos::code_signing::{
    Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
};
use sha2::{Digest, Sha256};
#[cfg(feature = "screen-capture")]
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

use super::receipts::{MacosTccCanaryLauncherEvidence, MacosTccCanarySigningEvidence};

pub(super) fn live_signing_identity_is_valid(
    audit_token: &str,
    expected_path: &Path,
    signing: &MacosTccCanarySigningEvidence,
) -> bool {
    let Some(bytes) = audit_token_bytes(audit_token) else {
        return false;
    };
    if audit_token_identity(audit_token).map(|identity| identity.pid)
        != Some(signing.process_bound_pid)
    {
        return false;
    }
    let token_data = CFData::from_buffer(&bytes);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(token_data.as_concrete_TypeRef());
    let Ok(code) = SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE)
    else {
        return false;
    };
    let Some(path) = code
        .path(CodeSigningFlags::NONE)
        .ok()
        .and_then(|url| url.to_path())
    else {
        return false;
    };
    let Ok(requirement) = signing.designated_requirement.parse::<SecRequirement>() else {
        return false;
    };
    let Ok(cdhash_requirement) =
        format!("cdhash H\"{}\"", signing.cdhash).parse::<SecRequirement>()
    else {
        return false;
    };
    path == expected_path
        && code
            .check_validity(CodeSigningFlags::STRICT_VALIDATE, &requirement)
            .is_ok()
        && code
            .check_validity(CodeSigningFlags::STRICT_VALIDATE, &cdhash_requirement)
            .is_ok()
}

#[cfg(feature = "screen-capture")]
pub(super) fn inspect_signing(
    executable: &Path,
    pid: u32,
    process_fingerprint: &str,
    audit_token: Option<&str>,
) -> Result<MacosTccCanarySigningEvidence> {
    let static_details = bounded_command(
        "/usr/bin/codesign",
        &["-d", "--verbose=4", path_arg(executable)?],
    )?;
    let dynamic_target = format!("+{pid}");
    let dynamic_details =
        bounded_command("/usr/bin/codesign", &["-d", "--verbose=4", &dynamic_target])?;
    let requirement = bounded_command("/usr/bin/codesign", &["-d", "-r-", path_arg(executable)?])?;
    let static_verification = bounded_command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "--verbose=4", path_arg(executable)?],
    )?;
    let dynamic_verification = bounded_command(
        "/usr/bin/codesign",
        &dynamic_codesign_verification_args(&dynamic_target),
    )?;
    let entitlements = bounded_command(
        "/usr/bin/codesign",
        &["-d", "--entitlements", ":-", path_arg(executable)?],
    )?;
    let spctl = bounded_command(
        "/usr/sbin/spctl",
        &[
            "--assess",
            "--type",
            "execute",
            "--verbose=4",
            path_arg(executable)?,
        ],
    )?;
    let static_detail_text = bounded_utf8(&static_details.stderr, "static codesign details")?;
    let dynamic_detail_text = bounded_utf8(&dynamic_details.stderr, "dynamic codesign details")?;
    let static_cdhash = details_value(static_detail_text, "CDHash=")?.to_ascii_lowercase();
    let dynamic_cdhash = details_value(dynamic_detail_text, "CDHash=")?.to_ascii_lowercase();
    let requirement_text = bounded_utf8(&requirement.stdout, "codesign requirement")?;
    let designated_requirement = requirement_text
        .lines()
        .find_map(|line| {
            line.strip_prefix("designated => ")
                .or_else(|| line.strip_prefix("# designated => "))
        })
        .context("codesign omitted designated requirement")?
        .to_owned();
    let requirement_digest = hex_digest(designated_requirement.as_bytes());
    let entitlement_text = bounded_utf8(&entitlements.stdout, "codesign entitlements")?;
    let mut evidence = MacosTccCanarySigningEvidence {
        bundle_identifier: details_value(dynamic_detail_text, "Identifier=")?.to_owned(),
        team_identifier: details_value(dynamic_detail_text, "TeamIdentifier=")?.to_owned(),
        designated_requirement,
        designated_requirement_sha256: requirement_digest,
        cdhash: dynamic_cdhash.clone(),
        process_bound_pid: pid,
        process_bound_fingerprint: process_fingerprint.to_owned(),
        process_bound_valid: dynamic_details.success
            && dynamic_verification.success
            && static_cdhash == dynamic_cdhash,
        audit_token_bound_valid: false,
        authorities: dynamic_detail_text
            .lines()
            .filter_map(|line| line.strip_prefix("Authority="))
            .map(str::to_owned)
            .collect(),
        entitlement_keys: plist_true_keys(entitlement_text)?,
        codesign_strict_valid: static_details.success
            && requirement.success
            && static_verification.success
            && entitlements.success,
        hardened_runtime: dynamic_detail_text
            .lines()
            .find(|line| line.starts_with("flags="))
            .is_some_and(|line| line.contains("runtime")),
        secure_timestamp: dynamic_detail_text
            .lines()
            .any(|line| line.starts_with("Timestamp=")),
        spctl_accepted: spctl.success,
    };
    evidence.audit_token_bound_valid = audit_token
        .is_some_and(|token| live_signing_identity_is_valid(token, executable, &evidence));
    Ok(evidence)
}

#[cfg(feature = "screen-capture")]
pub(super) fn plist_true_keys(xml: &str) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut remaining = xml;
    while let Some(key_start) = remaining.find("<key>") {
        remaining = &remaining[key_start + "<key>".len()..];
        let key_end = remaining
            .find("</key>")
            .context("entitlement key is not terminated")?;
        let key = &remaining[..key_end];
        remaining = &remaining[key_end + "</key>".len()..];
        let value = remaining.trim_start();
        anyhow::ensure!(
            value.starts_with("<true/>") || value.starts_with("<true />"),
            "entitlement {key} is not true"
        );
        keys.push(key.to_owned());
    }
    anyhow::ensure!(
        !keys.is_empty(),
        "codesign returned no Boolean entitlements"
    );
    keys.sort();
    Ok(keys)
}

#[cfg(feature = "screen-capture")]
pub(super) fn inspect_launcher(
    topology: MacosDaemonOwner,
    daemon_signing: &MacosTccCanarySigningEvidence,
) -> Result<MacosTccCanaryLauncherEvidence> {
    let pid = std::process::id();
    let mut system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    system.refresh_processes(ProcessesToUpdate::All, true);
    let current = system
        .process(Pid::from_u32(pid))
        .context("current process is missing from the process table")?;
    let parent_pid = current.parent().map(Pid::as_u32);
    let parent_executable_path = current
        .parent()
        .and_then(|parent| system.process(parent))
        .and_then(|parent| parent.exe())
        .map(Path::to_path_buf);
    let mut parent_signing = None;
    let (actual_launcher, expected_label, launchctl_pid_matches, verified) = match topology {
        MacosDaemonOwner::AppSidecar => {
            parent_signing =
                parent_pid
                    .zip(parent_executable_path.as_deref())
                    .and_then(|(parent_pid, path)| {
                        process_fingerprint(parent_pid)
                            .and_then(|fingerprint| {
                                inspect_signing(path, parent_pid, &fingerprint, None)
                            })
                            .ok()
                    });
            let parent_verified = parent_signing.as_ref().is_some_and(|signing| {
                signing.bundle_identifier == "tech.hyperbliss.hypercolor"
                    && signing.team_identifier == daemon_signing.team_identifier
                    && signing.codesign_strict_valid
                    && signing.hardened_runtime
                    && signing.secure_timestamp
                    && signing.spctl_accepted
            });
            (
                "packaged_app_supervisor".to_owned(),
                None,
                None,
                parent_verified,
            )
        }
        MacosDaemonOwner::DirectLaunchd => {
            let label = "tech.hyperbliss.hypercolor";
            let matches = launchctl_service_pid(label)? == Some(pid);
            (
                "direct_launchd".to_owned(),
                Some(label.to_owned()),
                Some(matches),
                matches,
            )
        }
        MacosDaemonOwner::Homebrew => {
            let label = "homebrew.mxcl.hypercolor";
            let matches = launchctl_service_pid(label)? == Some(pid);
            (
                "homebrew_services".to_owned(),
                Some(label.to_owned()),
                Some(matches),
                matches,
            )
        }
        MacosDaemonOwner::Standalone => {
            let direct = launchctl_service_pid("tech.hyperbliss.hypercolor")? == Some(pid);
            let homebrew = launchctl_service_pid("homebrew.mxcl.hypercolor")? == Some(pid);
            let terminal_parent = parent_executable_path
                .as_deref()
                .is_some_and(terminal_parent_is_valid);
            (
                "terminal_parent".to_owned(),
                None,
                None,
                terminal_parent && !direct && !homebrew,
            )
        }
    };
    Ok(MacosTccCanaryLauncherEvidence {
        actual_launcher,
        expected_label,
        parent_pid,
        parent_executable_path,
        parent_signing,
        launchctl_pid_matches,
        verified,
    })
}

#[cfg(feature = "screen-capture")]
fn launchctl_service_pid(label: &str) -> Result<Option<u32>> {
    let uid = bounded_command_text("/usr/bin/id", &["-u"])?;
    let target = format!("gui/{uid}/{label}");
    let output = bounded_command("/bin/launchctl", &["print", &target])?;
    if !output.success {
        return Ok(None);
    }
    let output = bounded_utf8(&output.stdout, "launchctl output")?;
    output
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("pid = "))
        .map(str::parse)
        .transpose()
        .context("launchctl returned an invalid service pid")
}

#[cfg(feature = "screen-capture")]
pub(super) fn host_architecture() -> Result<String> {
    if sysctl_flag("hw.optional.arm64")? || sysctl_flag("sysctl.proc_translated")? {
        Ok("apple_silicon".to_owned())
    } else {
        Ok("intel".to_owned())
    }
}

#[cfg(feature = "screen-capture")]
pub(super) fn process_fingerprint(pid: u32) -> Result<String> {
    let pid = pid.to_string();
    let identity =
        bounded_command_text("/bin/ps", &["-p", &pid, "-o", "lstart=", "-o", "command="])?;
    let identity = identity.split_whitespace().collect::<Vec<_>>().join(" ");
    anyhow::ensure!(!identity.is_empty(), "process identity is empty");
    Ok(hex_digest(identity.as_bytes()))
}

#[cfg(feature = "screen-capture")]
pub(super) fn sysctl_flag(name: &str) -> Result<bool> {
    let output = bounded_command("/usr/sbin/sysctl", &["-in", name])?;
    if !output.success {
        return Ok(false);
    }
    match bounded_utf8(&output.stdout, "sysctl output")?.trim() {
        "" | "0" => Ok(false),
        "1" => Ok(true),
        value => anyhow::bail!("sysctl {name} returned unexpected value {value:?}"),
    }
}

#[cfg(feature = "screen-capture")]
struct BoundedCommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(feature = "screen-capture")]
pub(super) fn dynamic_codesign_verification_args(dynamic_target: &str) -> [&str; 2] {
    ["--verify", dynamic_target]
}

#[cfg(feature = "screen-capture")]
fn bounded_command(program: &str, args: &[&str]) -> Result<BoundedCommandOutput> {
    const MAX_OUTPUT: usize = 64 * 1024;
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    anyhow::ensure!(
        output.stdout.len() <= MAX_OUTPUT && output.stderr.len() <= MAX_OUTPUT,
        "{program} output exceeds 64 KiB"
    );
    Ok(BoundedCommandOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[cfg(feature = "screen-capture")]
pub(super) fn bounded_command_text(program: &str, args: &[&str]) -> Result<String> {
    let output = bounded_command(program, args)?;
    anyhow::ensure!(output.success, "{program} exited unsuccessfully");
    Ok(bounded_utf8(&output.stdout, "command output")?
        .trim()
        .to_owned())
}

#[cfg(feature = "screen-capture")]
fn bounded_utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes).with_context(|| format!("{label} is not UTF-8"))
}

#[cfg(feature = "screen-capture")]
fn details_value<'a>(details: &'a str, prefix: &str) -> Result<&'a str> {
    details
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .filter(|value| !value.is_empty())
        .with_context(|| format!("codesign omitted {prefix}"))
}

#[cfg(feature = "screen-capture")]
fn path_arg(path: &Path) -> Result<&str> {
    path.to_str().context("process path is not valid UTF-8")
}

#[cfg(feature = "screen-capture")]
pub(super) fn hex_digest(bytes: &[u8]) -> String {
    hex_bytes(&Sha256::digest(bytes))
}

pub(super) fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(
        String::with_capacity(bytes.len().saturating_mul(2)),
        |mut digest, byte| {
            write!(&mut digest, "{byte:02x}").expect("writing into a String cannot fail");
            digest
        },
    )
}

#[cfg(feature = "screen-capture")]
pub(super) fn unix_time_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates the Unix epoch")?
        .as_millis()
        .try_into()
        .context("system time exceeds u64 milliseconds")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AuditTokenIdentity {
    pub(super) pid: u32,
    pidversion: u32,
}

pub(super) fn audit_token_identity(identity: &str) -> Option<AuditTokenIdentity> {
    let words = identity.split(':').collect::<Vec<_>>();
    if words.len() != 8
        || words
            .iter()
            .any(|word| word.len() != 8 || !word.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(AuditTokenIdentity {
        pid: u32::from_str_radix(words[5], 16).ok()?,
        pidversion: u32::from_str_radix(words[7], 16).ok()?,
    })
}

#[cfg(feature = "screen-capture")]
pub(super) fn audit_token_bytes(identity: &str) -> Option<[u8; 32]> {
    let words = identity.split(':').collect::<Vec<_>>();
    if words.len() != 8 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, word) in words.into_iter().enumerate() {
        let parsed = u32::from_str_radix(word, 16).ok()?;
        bytes[index * 4..index * 4 + 4].copy_from_slice(&parsed.to_ne_bytes());
    }
    Some(bytes)
}

pub(super) const fn topology_key(topology: MacosDaemonOwner) -> u8 {
    match topology {
        MacosDaemonOwner::AppSidecar => 0,
        MacosDaemonOwner::DirectLaunchd => 1,
        MacosDaemonOwner::Homebrew => 2,
        MacosDaemonOwner::Standalone => 3,
    }
}

pub(super) fn terminal_parent_is_valid(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "bash" | "dash" | "fish" | "nu" | "sh" | "tcsh" | "zsh"
            )
        })
}
