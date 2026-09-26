//! `hypercolor __launch --role <role> [-- <arguments>...]`: the stable
//! launcher a managed Linux installation's units start.
//!
//! The installer copies this CLI to `<release root>/launcher/hypercolor`, and
//! once an install settles with the service starting through it, never
//! rewrites it. Each launch asks the install library which release to run
//! and every path it will use, all beneath that one release, then replaces
//! this process with it, so systemd's main process becomes the selected
//! program and a later `active` swap cannot mix releases. The
//! `prepare-roots` role runs nothing: the service runs it unsandboxed first,
//! to create any recorded root a user deleted.

use std::ffi::OsString;

use crate::install::LinuxLaunchRole;

/// A parsed launch request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchInvocation {
    pub(crate) role: LinuxLaunchRole,
    pub(crate) arguments: Vec<OsString>,
}

const USAGE: &str = "usage: hypercolor __launch --role \
                     <daemon|cli|update-executor|prepare-roots> [-- <arguments>...]";

/// Parse `argv` when it asks for `__launch`, or return `None`.
///
/// The grammar is frozen by launcher contract 1: exactly `--role <role>`,
/// then optionally `--` and the arguments for the selected CLI.
pub(crate) fn parse_launch_invocation<I, T>(raw: I) -> Option<Result<LaunchInvocation, String>>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut raw = raw.into_iter().map(Into::into);
    let _executable = raw.next()?;
    if raw.next()? != crate::install::LINUX_LAUNCH_COMMAND {
        return None;
    }
    Some(parse_rest(raw))
}

fn parse_rest(mut raw: impl Iterator<Item = OsString>) -> Result<LaunchInvocation, String> {
    if raw.next().as_deref() != Some(std::ffi::OsStr::new("--role")) {
        return Err(USAGE.to_owned());
    }
    let role = raw
        .next()
        .and_then(|role| role.to_str().and_then(LinuxLaunchRole::parse))
        .ok_or_else(|| USAGE.to_owned())?;
    let arguments = match raw.next() {
        None => Vec::new(),
        Some(separator) if separator == "--" => raw.collect(),
        Some(_) => return Err(USAGE.to_owned()),
    };
    Ok(LaunchInvocation { role, arguments })
}

/// Plan the launch and replace this process with it. Never returns.
#[cfg(target_os = "linux")]
pub(crate) fn execute(invocation: LaunchInvocation) -> ! {
    use std::os::unix::process::CommandExt as _;

    let fail = |status: i32, message: String| -> ! {
        eprintln!("hypercolor launcher: {message}");
        std::process::exit(status)
    };
    let Some(home) = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|home| home.is_absolute())
    else {
        fail(EX_CONFIG, "HOME must be an absolute path".to_owned())
    };
    let launcher = match std::fs::read_link("/proc/self/exe") {
        Ok(launcher) => launcher,
        Err(error) => fail(
            EX_OSERR,
            format!("cannot resolve the launcher itself: {error}"),
        ),
    };
    let request = crate::install::LinuxLaunchRequest {
        home: &home,
        role: invocation.role,
        arguments: invocation.arguments,
        launcher: &launcher,
    };
    if invocation.role == LinuxLaunchRole::PrepareRoots {
        match crate::install::prepare_linux_launch_roots(&request) {
            Ok(created) => {
                for root in created {
                    eprintln!("hypercolor launcher: created {}", root.display());
                }
                std::process::exit(0)
            }
            Err(error) => fail(EX_CONFIG, error.to_string()),
        }
    }
    let plan = match crate::install::plan_linux_launch(&request) {
        Ok(plan) => plan,
        Err(error) => fail(EX_CONFIG, error.to_string()),
    };
    if invocation.role != LinuxLaunchRole::Cli {
        eprintln!(
            "hypercolor launcher: {} from release {}{}",
            invocation.role.as_str(),
            plan.unit.as_str(),
            match plan.selection {
                crate::install::LinuxLaunchSelection::Active => "",
                crate::install::LinuxLaunchSelection::PendingTransactionPrior => {
                    ", the prior of the unsettled install"
                }
                crate::install::LinuxLaunchSelection::ActiveWithoutRunnablePrior => {
                    ", since the unsettled install has no prior that can run this role"
                }
            }
        );
    }
    let error = std::process::Command::new(&plan.program)
        .args(&plan.arguments)
        .envs(plan.environment)
        .exec();
    fail(
        EX_OSERR,
        format!("cannot run {}: {error}", plan.program.display()),
    )
}

/// Non-Linux hosts have no managed installation to launch.
#[cfg(not(target_os = "linux"))]
pub(crate) fn execute(_invocation: LaunchInvocation) -> ! {
    eprintln!("hypercolor launcher: managed launches exist only on Linux");
    std::process::exit(EX_CONFIG)
}

/// `sysexits.h` `EX_USAGE`: the launch arguments are malformed.
pub(crate) const EX_USAGE: i32 = 64;
/// `sysexits.h` `EX_CONFIG`: the installation cannot be launched.
const EX_CONFIG: i32 = 78;
/// `sysexits.h` `EX_OSERR`: the selected program could not be executed.
#[cfg(target_os = "linux")]
const EX_OSERR: i32 = 71;

#[cfg(test)]
mod tests {
    use super::{LaunchInvocation, parse_launch_invocation};
    use crate::install::LinuxLaunchRole;
    use std::ffi::OsString;

    fn parse(words: &[&str]) -> Option<Result<LaunchInvocation, String>> {
        parse_launch_invocation(words.iter().map(OsString::from))
    }

    #[test]
    fn only_the_launch_command_is_claimed() {
        assert!(parse(&["hypercolor", "status"]).is_none());
        assert!(parse(&["hypercolor"]).is_none());
        assert!(parse(&["hypercolor", "__install-release"]).is_none());
    }

    #[test]
    fn roles_and_passthrough_follow_the_frozen_grammar() {
        assert_eq!(
            parse(&["hypercolor", "__launch", "--role", "daemon"]),
            Some(Ok(LaunchInvocation {
                role: LinuxLaunchRole::Daemon,
                arguments: Vec::new(),
            }))
        );
        assert_eq!(
            parse(&[
                "hypercolor",
                "__launch",
                "--role",
                "update-executor",
                "--",
                "update",
                "activator",
                "recover",
            ]),
            Some(Ok(LaunchInvocation {
                role: LinuxLaunchRole::UpdateExecutor,
                arguments: ["update", "activator", "recover"]
                    .map(OsString::from)
                    .to_vec(),
            }))
        );
        assert_eq!(
            parse(&["hypercolor", "__launch", "--role", "prepare-roots"]),
            Some(Ok(LaunchInvocation {
                role: LinuxLaunchRole::PrepareRoots,
                arguments: Vec::new(),
            }))
        );
        assert_eq!(
            parse(&[
                "hypercolor",
                "__launch",
                "--role",
                "cli",
                "--",
                "--",
                "status"
            ]),
            Some(Ok(LaunchInvocation {
                role: LinuxLaunchRole::Cli,
                arguments: ["--", "status"].map(OsString::from).to_vec(),
            }))
        );
    }

    #[test]
    fn anything_else_is_a_usage_error() {
        for words in [
            &["hypercolor", "__launch"][..],
            &["hypercolor", "__launch", "--role"],
            &["hypercolor", "__launch", "--role", "shell"],
            &[
                "hypercolor",
                "__launch",
                "--role",
                "daemon",
                "--ui-dir",
                "/tmp",
            ],
            &["hypercolor", "__launch", "daemon"],
            &["hypercolor", "__launch", "--role=daemon"],
        ] {
            assert!(
                matches!(parse(words), Some(Err(_))),
                "{words:?} must be refused"
            );
        }
    }
}
