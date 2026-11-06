//! Windows Firewall program rules, through `INetFwPolicy2`.
//!
//! `netsh` prints localized output, so a rule's program cannot be read back
//! from it on every system; the firewall's COM API answers in plain fields
//! and updates a rule's program in one call.

use std::io;

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_SERVICE_DISABLED, ERROR_SERVICE_NOT_ACTIVE, RPC_E_CHANGED_MODE,
    VARIANT_TRUE,
};
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule, INetFwRules, NET_FW_ACTION_ALLOW, NET_FW_PROFILE2_ALL,
    NET_FW_RULE_DIR_IN, NetFwPolicy2, NetFwRule,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{BSTR, Error, HRESULT};

/// `EPT_S_NOT_REGISTERED`: the firewall service has no endpoint to reach.
const NO_ENDPOINT: HRESULT = HRESULT(0x8007_06D9_u32.cast_signed());
/// `RPC_S_SERVER_UNAVAILABLE`.
const NO_SERVER: HRESULT = HRESULT(0x8007_06BA_u32.cast_signed());

/// What the firewall holds under one rule name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirewallRule {
    /// The firewall service is stopped or disabled, so it enforces no rule.
    Unavailable,
    /// No rule has the name.
    Absent,
    /// A rule has the name and allows this program.
    Program(String),
}

fn os_error(error: &Error) -> io::Error {
    io::Error::from_raw_os_error(error.code().0)
}

/// Whether `error` means the firewall service is not there to ask.
fn is_unavailable(error: &Error) -> bool {
    let code = error.code();
    code == ERROR_SERVICE_DISABLED.to_hresult()
        || code == ERROR_SERVICE_NOT_ACTIVE.to_hresult()
        || code == NO_ENDPOINT
        || code == NO_SERVER
}

fn is_absent(error: &Error) -> bool {
    error.code() == ERROR_FILE_NOT_FOUND.to_hresult()
}

/// COM for the current thread, released when dropped.
struct ComThread {
    release: bool,
}

impl ComThread {
    fn join() -> io::Result<Self> {
        // SAFETY: The call takes a reserved null and plain flags.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if joined == RPC_E_CHANGED_MODE {
            // The thread already joined another apartment, which outlives
            // this use and suits these calls as well.
            return Ok(Self { release: false });
        }
        joined.ok().map_err(|error| os_error(&error))?;
        Ok(Self { release: true })
    }
}

impl Drop for ComThread {
    fn drop(&mut self) {
        if self.release {
            // SAFETY: This balances the successful `CoInitializeEx` of
            // `join`. The guard never leaves the call that made it, so it
            // drops on the thread that joined.
            unsafe { CoUninitialize() };
        }
    }
}

fn rules() -> windows::core::Result<INetFwRules> {
    // SAFETY: The class and interface identifiers come from the binding, and
    // no aggregate is passed.
    let policy: INetFwPolicy2 =
        unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }?;
    // SAFETY: `policy` is a live interface pointer.
    unsafe { policy.Rules() }
}

fn named_rule(rules: &INetFwRules, name: &BSTR) -> windows::core::Result<Option<INetFwRule>> {
    // SAFETY: `rules` is a live interface pointer and `name` a valid BSTR.
    match unsafe { rules.Item(name) } {
        Ok(rule) => Ok(Some(rule)),
        Err(error) if is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// What the firewall holds under `name`.
///
/// # Errors
///
/// Returns the operating-system error when the firewall cannot be asked,
/// other than because its service is stopped.
pub fn firewall_rule(name: &str) -> io::Result<FirewallRule> {
    let _com = ComThread::join()?;
    let read = || -> windows::core::Result<FirewallRule> {
        let rules = rules()?;
        let Some(rule) = named_rule(&rules, &BSTR::from(name))? else {
            return Ok(FirewallRule::Absent);
        };
        // SAFETY: `rule` is a live interface pointer.
        let program = unsafe { rule.ApplicationName() }?;
        Ok(FirewallRule::Program(program.to_string()))
    };
    match read() {
        Ok(rule) => Ok(rule),
        Err(error) if is_unavailable(&error) => Ok(FirewallRule::Unavailable),
        Err(error) => Err(os_error(&error)),
    }
}

/// Make the rule under `name` allow `program` inbound on every profile, or
/// remove it when `program` is `None`.
///
/// An existing rule keeps everything but its program. Repeating a call
/// changes nothing.
///
/// # Errors
///
/// Returns the operating-system error when the firewall refuses, as it
/// does a caller without administrator rights.
pub fn set_firewall_rule_program(name: &str, program: Option<&str>) -> io::Result<()> {
    let _com = ComThread::join()?;
    let change = || -> windows::core::Result<()> {
        let rules = rules()?;
        let name = BSTR::from(name);
        let existing = named_rule(&rules, &name)?;
        match (existing, program) {
            (None, None) => Ok(()),
            (Some(_), None) => {
                // SAFETY: `rules` is live and `name` a valid BSTR.
                unsafe { rules.Remove(&name) }
            }
            (Some(rule), Some(program)) => {
                // SAFETY: `rule` is live and the argument a valid BSTR.
                unsafe { rule.SetApplicationName(&BSTR::from(program)) }
            }
            (None, Some(program)) => {
                // SAFETY: The class and interface identifiers come from the
                // binding, and no aggregate is passed.
                let rule: INetFwRule =
                    unsafe { CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER) }?;
                // SAFETY: `rule` and `rules` are live interface pointers, and
                // every argument is a valid BSTR or a plain value.
                unsafe {
                    rule.SetName(&name)?;
                    rule.SetApplicationName(&BSTR::from(program))?;
                    rule.SetDirection(NET_FW_RULE_DIR_IN)?;
                    rule.SetAction(NET_FW_ACTION_ALLOW)?;
                    rule.SetProfiles(NET_FW_PROFILE2_ALL.0)?;
                    rule.SetEnabled(VARIANT_TRUE)?;
                    rules.Add(&rule)
                }
            }
        }
    };
    change().map_err(|error| os_error(&error))
}

#[cfg(test)]
mod tests {
    use super::{FirewallRule, firewall_rule, set_firewall_rule_program};

    /// Removes the rule even when an assertion fails first.
    struct Rule(String);

    impl Drop for Rule {
        fn drop(&mut self) {
            drop(set_firewall_rule_program(&self.0, None));
        }
    }

    #[test]
    fn a_missing_rule_reads_as_absent() {
        let rule = firewall_rule("Hypercolor test rule that nothing ever creates")
            .expect("ask the firewall");
        assert!(
            matches!(rule, FirewallRule::Absent | FirewallRule::Unavailable),
            "{rule:?}"
        );
    }

    /// Needs administrator rights, so it runs in the service manager step of
    /// the Windows CI lane.
    #[test]
    #[ignore = "changes the machine's firewall; needs administrator rights"]
    fn a_rule_moves_between_programs_and_goes_away() {
        let rule = Rule(format!("Hypercolor test rule {}", std::process::id()));
        let first = r"C:\Program Files\Hypercolor\units\aaaa\hypercolor-daemon.exe";
        let second = r"C:\Program Files\Hypercolor\units\bbbb\hypercolor-daemon.exe";

        assert_eq!(
            firewall_rule(&rule.0).expect("before"),
            FirewallRule::Absent
        );
        set_firewall_rule_program(&rule.0, Some(first)).expect("create");
        assert_eq!(
            firewall_rule(&rule.0).expect("created"),
            FirewallRule::Program(first.to_owned())
        );
        set_firewall_rule_program(&rule.0, Some(second)).expect("move");
        set_firewall_rule_program(&rule.0, Some(second)).expect("repeat");
        assert_eq!(
            firewall_rule(&rule.0).expect("moved"),
            FirewallRule::Program(second.to_owned())
        );
        set_firewall_rule_program(&rule.0, None).expect("remove");
        set_firewall_rule_program(&rule.0, None).expect("repeat");
        assert_eq!(
            firewall_rule(&rule.0).expect("removed"),
            FirewallRule::Absent
        );
    }
}
