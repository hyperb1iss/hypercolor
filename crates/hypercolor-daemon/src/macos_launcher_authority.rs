use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use hypercolor_macos_owner::{MACOS_APP_BUNDLE_BINARY_NAMES, MacosDaemonOwner};
use security_framework::os::macos::code_signing::{
    Flags as CodeSigningFlags, GuestAttributes, SecCode, SecRequirement,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

pub const MACOS_OWNER_ENV: &str = "HYPERCOLOR_MACOS_OWNER";

const DIRECT_LAUNCHD_LABEL: &str = "tech.hyperbliss.hypercolor";
const HOMEBREW_LAUNCHD_LABEL: &str = "homebrew.mxcl.hypercolor";
const SIDECAR_REQUIREMENT_PREFIX: &str = "identifier \"tech.hyperbliss.hypercolor.sidecar\" and ";
const APP_REQUIREMENT_PREFIX: &str = "identifier \"tech.hyperbliss.hypercolor\" and ";
const MAX_COMMAND_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MacosLauncherAuthorityEvidence {
    pub app_sidecar: bool,
    pub direct_launchd: bool,
    pub homebrew: bool,
    pub standalone: bool,
}

impl MacosLauncherAuthorityEvidence {
    fn exact_owner(self) -> Result<MacosDaemonOwner> {
        let candidates = [
            (self.app_sidecar, MacosDaemonOwner::AppSidecar),
            (self.direct_launchd, MacosDaemonOwner::DirectLaunchd),
            (self.homebrew, MacosDaemonOwner::Homebrew),
            (self.standalone, MacosDaemonOwner::Standalone),
        ]
        .into_iter()
        .filter_map(|(matches, owner)| matches.then_some(owner))
        .collect::<Vec<_>>();

        match candidates.as_slice() {
            [owner] => Ok(*owner),
            [] => anyhow::bail!("macOS daemon launcher authority is missing"),
            _ => anyhow::bail!("macOS daemon launcher authority is ambiguous"),
        }
    }
}

pub fn parse_macos_owner_claim(value: &str) -> Result<MacosDaemonOwner> {
    match value {
        "app-sidecar" => Ok(MacosDaemonOwner::AppSidecar),
        "direct-launchd" => Ok(MacosDaemonOwner::DirectLaunchd),
        "homebrew" => Ok(MacosDaemonOwner::Homebrew),
        "standalone" => Ok(MacosDaemonOwner::Standalone),
        _ => anyhow::bail!("invalid macOS daemon launcher claim {value:?}"),
    }
}

pub fn resolve_macos_launcher_owner(
    environment_claim: Option<&OsStr>,
    argument_claim: Option<MacosDaemonOwner>,
    evidence: MacosLauncherAuthorityEvidence,
) -> Result<MacosDaemonOwner> {
    let environment_claim = environment_claim
        .map(|value| {
            value
                .to_str()
                .context("macOS daemon launcher environment claim is not UTF-8")
                .and_then(parse_macos_owner_claim)
        })
        .transpose()?;
    if let (Some(environment), Some(argument)) = (environment_claim, argument_claim) {
        anyhow::ensure!(
            environment == argument,
            "conflicting macOS daemon launcher claims: environment={} argument={}",
            owner_name(environment),
            owner_name(argument)
        );
    }

    let claim = environment_claim.or(argument_claim);
    let authority = evidence.exact_owner()?;
    if let Some(claim) = claim {
        anyhow::ensure!(
            claim == authority,
            "macOS daemon launcher claim {} is not corroborated by {} authority",
            owner_name(claim),
            owner_name(authority)
        );
    }
    Ok(authority)
}

pub fn inspect_macos_launcher_authority(
    current_executable: &Path,
    current_designated_requirement: &str,
) -> Result<MacosLauncherAuthorityEvidence> {
    let current_pid = std::process::id();
    let (parent_pid, parent_executable) = current_process_parent(current_pid)?;
    let direct_launchd = launchctl_service_pid(DIRECT_LAUNCHD_LABEL)? == Some(current_pid);
    let homebrew = launchctl_service_pid(HOMEBREW_LAUNCHD_LABEL)? == Some(current_pid);
    let mut app_sidecar = app_sidecar_parent_is_valid(
        current_executable,
        current_designated_requirement,
        parent_pid,
        &parent_executable,
    )?;
    if app_sidecar {
        let (rechecked_parent_pid, rechecked_parent_executable) =
            current_process_parent(current_pid)?;
        app_sidecar = rechecked_parent_pid == parent_pid
            && paths_are_equal(&rechecked_parent_executable, &parent_executable);
    }
    // Standalone is the user-directed residual: no positively attested
    // manager owns this process. Gating it on the parent binary's name
    // (shell allowlists) adds no authority (any launcher can interpose
    // `sh -c`) and breaks cargo, sudo, and supervisor launches.
    let standalone = !app_sidecar && !direct_launchd && !homebrew;

    Ok(MacosLauncherAuthorityEvidence {
        app_sidecar,
        direct_launchd,
        homebrew,
        standalone,
    })
}

const fn owner_name(owner: MacosDaemonOwner) -> &'static str {
    match owner {
        MacosDaemonOwner::AppSidecar => "app-sidecar",
        MacosDaemonOwner::DirectLaunchd => "direct-launchd",
        MacosDaemonOwner::Homebrew => "homebrew",
        MacosDaemonOwner::Standalone => "standalone",
    }
}

fn current_process_parent(current_pid: u32) -> Result<(u32, PathBuf)> {
    let mut system = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    system.refresh_processes(ProcessesToUpdate::All, true);
    let current = system
        .process(Pid::from_u32(current_pid))
        .context("current daemon is missing from the process table")?;
    let parent_pid = current
        .parent()
        .context("current daemon has no inspectable parent process")?;
    let parent = system
        .process(parent_pid)
        .context("daemon parent disappeared during launcher inspection")?;
    let parent_executable = parent
        .exe()
        .context("daemon parent executable is unavailable")?
        .to_path_buf();
    Ok((parent_pid.as_u32(), parent_executable))
}

fn app_sidecar_parent_is_valid(
    current_executable: &Path,
    current_designated_requirement: &str,
    parent_pid: u32,
    parent_executable: &Path,
) -> Result<bool> {
    if !app_sidecar_layout_is_valid(current_executable) {
        return Ok(false);
    }
    let daemon_directory = current_executable
        .parent()
        .expect("validated app sidecar path has a parent");
    if !parent_is_adjacent_app_binary(parent_executable, daemon_directory) {
        return Ok(false);
    }

    let Some(parent_requirement) = app_parent_requirement(current_designated_requirement) else {
        // Unsigned local builds are ad-hoc signed and carry a bare cdhash
        // requirement with no identifier chain to verify, so the signed
        // parent/daemon requirement handshake is impossible. Structural
        // evidence still holds: the launcher must be live code executing
        // the exact app binary adjacent to this daemon inside a bundle
        // layout. A daemon whose own requirement carries the identifier
        // chain never takes this path; for signed builds the full
        // handshake stays mandatory.
        if is_adhoc_requirement(current_designated_requirement) {
            return live_process_path_matches(parent_pid, parent_executable);
        }
        return Ok(false);
    };

    let parent_valid =
        live_process_satisfies_requirement(parent_pid, parent_executable, &parent_requirement)?;
    let daemon_valid =
        current_process_satisfies_requirement(current_executable, current_designated_requirement)?;
    Ok(parent_valid && daemon_valid)
}

fn is_adhoc_requirement(requirement: &str) -> bool {
    requirement.starts_with("cdhash ")
}

/// Whether the observed parent executable is one of the bundle's app
/// binaries sitting in the same `Contents/MacOS` directory as the daemon.
///
/// The canonical file name must match the candidate byte-for-byte: on the
/// default case-insensitive APFS volume, a product-cased candidate would
/// otherwise resolve onto the lowercase CLI binary in the same directory.
fn parent_is_adjacent_app_binary(parent_executable: &Path, daemon_directory: &Path) -> bool {
    MACOS_APP_BUNDLE_BINARY_NAMES.iter().any(|name| {
        paths_are_equal(parent_executable, &daemon_directory.join(name))
            && parent_executable
                .canonicalize()
                .ok()
                .is_some_and(|path| path.file_name() == Some(OsStr::new(name)))
    })
}

fn live_process_path_matches(pid: u32, expected: &Path) -> Result<bool> {
    let pid = i32::try_from(pid).context("launcher pid exceeds the macOS pid range")?;
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(pid);
    let code = SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE)
        .context("failed to inspect the live launcher code identity")?;
    let Some(observed) = code
        .path(CodeSigningFlags::NONE)
        .context("failed to resolve the live launcher code path")?
        .to_path()
    else {
        return Ok(false);
    };
    Ok(code_path_matches(&observed, expected, true))
}

fn app_sidecar_layout_is_valid(current_executable: &Path) -> bool {
    let Some(executable_directory) = current_executable.parent() else {
        return false;
    };
    let is_sidecar_name = current_executable
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "hypercolor-daemon" || name.starts_with("hypercolor-daemon-"));
    let is_bundle_macos_directory = executable_directory
        .file_name()
        .and_then(|name| name.to_str())
        == Some("MacOS")
        && executable_directory
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("Contents");
    is_sidecar_name && is_bundle_macos_directory
}

fn app_parent_requirement(sidecar_requirement: &str) -> Option<String> {
    let tail = sidecar_requirement.strip_prefix(SIDECAR_REQUIREMENT_PREFIX)?;
    (!tail.is_empty()).then(|| format!("{APP_REQUIREMENT_PREFIX}{tail}"))
}

fn current_process_satisfies_requirement(path: &Path, requirement: &str) -> Result<bool> {
    let code = SecCode::for_self(CodeSigningFlags::NONE)
        .context("failed to inspect the live daemon code identity")?;
    code_satisfies_requirement(&code, path, requirement, false)
}

fn live_process_satisfies_requirement(pid: u32, path: &Path, requirement: &str) -> Result<bool> {
    let pid = i32::try_from(pid).context("launcher pid exceeds the macOS pid range")?;
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(pid);
    let code = SecCode::copy_guest_with_attribues(None, &attributes, CodeSigningFlags::NONE)
        .context("failed to inspect the live launcher code identity")?;
    code_satisfies_requirement(&code, path, requirement, true)
}

fn code_satisfies_requirement(
    code: &SecCode,
    path: &Path,
    requirement: &str,
    allow_bundle_root: bool,
) -> Result<bool> {
    let Some(observed_path) = code
        .path(CodeSigningFlags::NONE)
        .context("failed to resolve the live code path")?
        .to_path()
    else {
        return Ok(false);
    };
    if !code_path_matches(&observed_path, path, allow_bundle_root) {
        return Ok(false);
    }
    let compiled_requirement = requirement
        .parse::<SecRequirement>()
        .context("failed to compile the expected code requirement")?;
    Ok(code
        .check_validity(CodeSigningFlags::STRICT_VALIDATE, &compiled_requirement)
        .is_ok())
}

fn code_path_matches(observed: &Path, executable: &Path, allow_bundle_root: bool) -> bool {
    if paths_are_equal(observed, executable) {
        return true;
    }
    if !allow_bundle_root {
        return false;
    }
    let Some(bundle_root) = executable
        .parent()
        .filter(|directory| directory.file_name() == Some(OsStr::new("MacOS")))
        .and_then(Path::parent)
        .filter(|directory| directory.file_name() == Some(OsStr::new("Contents")))
        .and_then(Path::parent)
        .filter(|directory| directory.extension() == Some(OsStr::new("app")))
    else {
        return false;
    };
    paths_are_equal(observed, bundle_root)
}

/// Canonicalize-and-compare that treats resolution failure as non-equality.
/// Every caller gathers launcher evidence, where an unresolvable path means
/// the relationship cannot be attested; errors must demote privilege, never
/// abort daemon startup (standalone and homebrew layouts have no app binary
/// sibling to resolve).
fn paths_are_equal(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn launchctl_service_pid(label: &str) -> Result<Option<u32>> {
    let uid = command_output("/usr/bin/id", &["-u"])?;
    if uid.exit_code != Some(0) {
        anyhow::bail!(
            "id -u failed with status {:?}: {}",
            uid.exit_code,
            bounded_utf8(&uid.stderr, "id error output")?.trim()
        );
    }
    let uid = bounded_utf8(&uid.stdout, "id output")?.trim();
    let uid = uid
        .parse::<u32>()
        .context("id -u returned an invalid user identifier")?;
    let target = format!("gui/{uid}/{label}");
    let output = command_output("/bin/launchctl", &["print", &target])?;
    let missing_service =
        format!("Could not find service \"{label}\" in domain for user gui: {uid}");
    parse_launchctl_service_pid(
        output.exit_code,
        &output.stdout,
        &output.stderr,
        &missing_service,
    )
}

struct CommandOutput {
    exit_code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn command_output(program: &str, args: &[&str]) -> Result<CommandOutput> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    anyhow::ensure!(
        output.stdout.len() <= MAX_COMMAND_OUTPUT_BYTES
            && output.stderr.len() <= MAX_COMMAND_OUTPUT_BYTES,
        "{program} output exceeds 64 KiB"
    );
    Ok(CommandOutput {
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn parse_launchctl_service_pid(
    exit_code: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    expected_missing_service: &str,
) -> Result<Option<u32>> {
    match exit_code {
        Some(0) => {}
        Some(113) => {
            let stderr = bounded_utf8(stderr, "launchctl error output")?;
            anyhow::ensure!(
                stderr.lines().any(|line| line == expected_missing_service),
                "launchctl returned status 113 without the exact missing-service diagnostic"
            );
            return Ok(None);
        }
        code => anyhow::bail!(
            "launchctl inspection failed with status {:?}: {}",
            code,
            bounded_utf8(stderr, "launchctl error output")?.trim()
        ),
    }
    let stdout = bounded_utf8(stdout, "launchctl output")?;
    let pids = stdout
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pid = "))
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match pids.as_slice() {
        [] => Ok(None),
        [pid] if *pid > 0 => Ok(Some(*pid)),
        [_] => anyhow::bail!("launchctl returned a zero service pid"),
        _ => anyhow::bail!("launchctl returned ambiguous service pids"),
    }
}

fn bounded_utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes).with_context(|| format!("{label} is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{
        APP_REQUIREMENT_PREFIX, MacosLauncherAuthorityEvidence, SIDECAR_REQUIREMENT_PREFIX,
        app_parent_requirement, app_sidecar_layout_is_valid, app_sidecar_parent_is_valid,
        code_path_matches, is_adhoc_requirement, parent_is_adjacent_app_binary,
        parse_launchctl_service_pid, parse_macos_owner_claim, paths_are_equal,
        resolve_macos_launcher_owner,
    };
    use hypercolor_macos_owner::MacosDaemonOwner;

    fn evidence(owner: MacosDaemonOwner) -> MacosLauncherAuthorityEvidence {
        MacosLauncherAuthorityEvidence {
            app_sidecar: owner == MacosDaemonOwner::AppSidecar,
            direct_launchd: owner == MacosDaemonOwner::DirectLaunchd,
            homebrew: owner == MacosDaemonOwner::Homebrew,
            standalone: owner == MacosDaemonOwner::Standalone,
        }
    }

    #[test]
    fn owner_claim_parser_accepts_only_canonical_legacy_values() {
        for (value, owner) in [
            ("app-sidecar", MacosDaemonOwner::AppSidecar),
            ("direct-launchd", MacosDaemonOwner::DirectLaunchd),
            ("homebrew", MacosDaemonOwner::Homebrew),
            ("standalone", MacosDaemonOwner::Standalone),
        ] {
            assert_eq!(
                parse_macos_owner_claim(value).expect("canonical claim should parse"),
                owner
            );
        }
        for value in ["", "app_sidecar", "APP-SIDECAR", "launchd", "unknown"] {
            assert!(parse_macos_owner_claim(value).is_err());
        }
    }

    #[test]
    fn env_and_argument_compatibility_matrix_requires_exact_authority() {
        for (environment, argument, authority, expected) in [
            (
                None,
                None,
                MacosDaemonOwner::Standalone,
                MacosDaemonOwner::Standalone,
            ),
            (
                Some("app-sidecar"),
                None,
                MacosDaemonOwner::AppSidecar,
                MacosDaemonOwner::AppSidecar,
            ),
            (
                None,
                Some(MacosDaemonOwner::DirectLaunchd),
                MacosDaemonOwner::DirectLaunchd,
                MacosDaemonOwner::DirectLaunchd,
            ),
            (
                Some("homebrew"),
                Some(MacosDaemonOwner::Homebrew),
                MacosDaemonOwner::Homebrew,
                MacosDaemonOwner::Homebrew,
            ),
        ] {
            assert_eq!(
                resolve_macos_launcher_owner(
                    environment.map(OsStr::new),
                    argument,
                    evidence(authority),
                )
                .expect("compatible claims should resolve"),
                expected
            );
        }

        assert!(
            resolve_macos_launcher_owner(
                Some(OsStr::new("homebrew")),
                Some(MacosDaemonOwner::DirectLaunchd),
                evidence(MacosDaemonOwner::Homebrew),
            )
            .is_err()
        );
        assert!(
            resolve_macos_launcher_owner(
                Some(OsStr::new("invalid")),
                Some(MacosDaemonOwner::DirectLaunchd),
                evidence(MacosDaemonOwner::DirectLaunchd),
            )
            .is_err()
        );
        assert!(
            resolve_macos_launcher_owner(
                Some(OsStr::from_bytes(&[0xff])),
                Some(MacosDaemonOwner::DirectLaunchd),
                evidence(MacosDaemonOwner::DirectLaunchd),
            )
            .is_err()
        );
        assert!(
            resolve_macos_launcher_owner(
                Some(OsStr::new("app-sidecar")),
                None,
                evidence(MacosDaemonOwner::Homebrew),
            )
            .is_err()
        );
    }

    #[test]
    fn missing_and_ambiguous_authority_fail_closed() {
        assert!(
            resolve_macos_launcher_owner(None, None, MacosLauncherAuthorityEvidence::default())
                .is_err()
        );
        assert!(
            resolve_macos_launcher_owner(
                None,
                None,
                MacosLauncherAuthorityEvidence {
                    app_sidecar: true,
                    direct_launchd: true,
                    ..MacosLauncherAuthorityEvidence::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn adjacent_app_binary_check_accepts_the_real_bundle_binary_name() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let macos_dir = directory.path().join("Hypercolor.app/Contents/MacOS");
        std::fs::create_dir_all(&macos_dir).expect("bundle directories should build");
        for name in ["hypercolor-app", "hypercolor", "hypercolor-daemon"] {
            std::fs::write(macos_dir.join(name), b"fixture").expect("fixture binary should write");
        }

        // Tauri bundles ship the cargo binary name, not the product name.
        assert!(parent_is_adjacent_app_binary(
            &macos_dir.join("hypercolor-app"),
            &macos_dir
        ));
        // The CLI living in the same directory is not an app launcher.
        assert!(!parent_is_adjacent_app_binary(
            &macos_dir.join("hypercolor"),
            &macos_dir
        ));
        // A same-named binary outside the daemon's directory does not count.
        let elsewhere = directory.path().join("hypercolor-app");
        std::fs::write(&elsewhere, b"fixture").expect("outside fixture should write");
        assert!(!parent_is_adjacent_app_binary(&elsewhere, &macos_dir));
    }

    #[test]
    fn adhoc_requirement_detection_matches_only_bare_cdhash_shapes() {
        assert!(is_adhoc_requirement("cdhash H\"0123456789abcdef\""));
        assert!(!is_adhoc_requirement(
            "identifier \"tech.hyperbliss.hypercolor.sidecar\" and anchor apple generic"
        ));
        assert!(!is_adhoc_requirement(
            "identifier \"x\" and cdhash H\"0123456789abcdef\""
        ));
    }

    #[test]
    fn app_parent_requirement_preserves_the_exact_sidecar_signer_tail() {
        let tail = "anchor apple generic and certificate leaf[subject.OU] = \"TEAMID1234\"";
        let sidecar = format!("{SIDECAR_REQUIREMENT_PREFIX}{tail}");
        let app = app_parent_requirement(&sidecar)
            .expect("sidecar requirement should carry the canonical prefix");
        assert_eq!(app, format!("{APP_REQUIREMENT_PREFIX}{tail}"));
        assert!(app_parent_requirement("cdhash H\"0123456789abcdef\"").is_none());
        assert!(app_parent_requirement(SIDECAR_REQUIREMENT_PREFIX).is_none());
    }

    #[test]
    fn app_sidecar_layout_requires_the_bundle_macos_directory() {
        let daemon = Path::new(
            "/Applications/Hypercolor.app/Contents/MacOS/hypercolor-daemon-aarch64-apple-darwin",
        );
        assert!(app_sidecar_layout_is_valid(daemon));
        assert!(!app_sidecar_layout_is_valid(Path::new(
            "/Applications/Hypercolor.app/Contents/Resources/hypercolor-daemon"
        )));
    }

    #[test]
    fn live_app_code_path_accepts_its_bundle_root() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let bundle = directory.path().join("Hypercolor.app");
        let executable = bundle.join("Contents/MacOS/Hypercolor");
        std::fs::create_dir_all(
            executable
                .parent()
                .expect("executable should have a parent"),
        )
        .expect("bundle directories should build");
        std::fs::write(&executable, b"fixture").expect("fixture executable should write");

        assert!(code_path_matches(&bundle, &executable, true));
        assert!(!code_path_matches(&bundle, &executable, false));
    }

    #[test]
    fn path_comparison_treats_unresolvable_paths_as_unattested() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let present = directory.path().join("present");
        std::fs::write(&present, b"fixture").expect("fixture file should write");
        let missing = directory.path().join("missing");

        assert!(paths_are_equal(&present, &present));
        assert!(!paths_are_equal(&present, &missing));
        assert!(!paths_are_equal(&missing, &missing));
    }

    #[test]
    fn sidecar_parent_check_rejects_non_bundle_layouts_without_error() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let daemon = directory.path().join("hypercolor-daemon");
        std::fs::write(&daemon, b"fixture").expect("fixture daemon should write");
        let parent = directory.path().join("cargo");
        std::fs::write(&parent, b"fixture").expect("fixture parent should write");

        let verdict = app_sidecar_parent_is_valid(&daemon, "unused", 1, &parent)
            .expect("a target/debug style layout must not abort launcher inspection");
        assert!(!verdict);
    }

    #[test]
    fn sidecar_parent_check_rejects_a_bundle_missing_the_app_binary_without_error() {
        let directory = tempfile::tempdir().expect("temporary directory should build");
        let macos_dir = directory.path().join("Hypercolor.app/Contents/MacOS");
        std::fs::create_dir_all(&macos_dir).expect("bundle directories should build");
        let daemon = macos_dir.join("hypercolor-daemon");
        std::fs::write(&daemon, b"fixture").expect("fixture daemon should write");
        let parent = directory.path().join("zsh");
        std::fs::write(&parent, b"fixture").expect("fixture parent should write");

        let verdict = app_sidecar_parent_is_valid(&daemon, "unused", 1, &parent)
            .expect("a bundle without its app binary must not abort launcher inspection");
        assert!(!verdict);
    }

    #[test]
    fn launchctl_pid_parser_rejects_malformed_or_ambiguous_jobs() {
        assert_eq!(
            parse_launchctl_service_pid(Some(0), b"state = running\n\tpid = 42\n", b"", "unused",)
                .expect("one pid should parse"),
            Some(42)
        );
        let missing = "Could not find service \"missing\" in domain for user gui: 501";
        assert_eq!(
            parse_launchctl_service_pid(
                Some(113),
                b"",
                b"Bad request.\nCould not find service \"missing\" in domain for user gui: 501\n",
                missing,
            )
            .expect("the exact missing-service status should be absent"),
            None
        );
        assert!(
            parse_launchctl_service_pid(Some(113), b"", b"permission denied", missing).is_err()
        );
        let error = parse_launchctl_service_pid(Some(1), b"", b"permission denied", "unused")
            .expect_err("arbitrary launchctl failure must remain fatal");
        assert!(error.to_string().contains("permission denied"));
        assert!(parse_launchctl_service_pid(None, b"", b"terminated", "unused").is_err());
        assert!(parse_launchctl_service_pid(Some(0), b"pid = nope\n", b"", "unused").is_err());
        assert!(
            parse_launchctl_service_pid(Some(0), b"pid = 42\npid = 43\n", b"", "unused",).is_err()
        );
        assert!(parse_launchctl_service_pid(Some(0), b"pid = 0\n", b"", "unused").is_err());
        assert!(parse_launchctl_service_pid(Some(0), &[0xff], b"", "unused").is_err());
    }
}
