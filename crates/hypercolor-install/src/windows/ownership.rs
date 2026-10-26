//! Which principals may change the directories a Windows install relies on.
//!
//! The store roots, the units directory and every unit are the installer's
//! own: only a trusted principal may own them or hold any write-class right
//! on them, and no entry they pass to new children may grant one either.
//! Their ancestors are shared system directories, where other principals
//! may add children (as `C:\ProgramData` lets every user) but must not be
//! able to delete, rename or replace an existing entry or take over the
//! directory's security. Write judgements never credit a deny entry, so a
//! refusal can be too strict but never too lenient; whether a standard user
//! can run the release is judged by evaluating allow and deny entries in
//! order, the way an access check does.

use std::fmt::Write as _;

use hypercolor_platform_fs::{WindowsAceKind, WindowsSecurity, WindowsSid};

/// The local system account.
const LOCAL_SYSTEM: &str = "S-1-5-18";
/// The built-in Administrators group.
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// `NT SERVICE\TrustedInstaller`, the owner of the system's own directories.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
/// `CREATOR OWNER` and `CREATOR GROUP`: inheritable entries naming them
/// resolve to whoever creates the child, which only a trusted principal can.
const CREATOR_PLACEHOLDERS: [&str; 2] = ["S-1-3-0", "S-1-3-1"];
/// The groups every signed-in standard user's token holds: Users,
/// Everyone, Authenticated Users and Interactive.
const STANDARD_USER_TOKEN: [&str; 4] = ["S-1-5-32-545", "S-1-1-0", "S-1-5-11", "S-1-5-4"];
/// Read, list, traverse and execute, as `units\` grants Users.
const USERS_READ_EXECUTE: &str = "0x1200a9";

/// Why a directory or file cannot be trusted by a Windows install.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowsDirectoryRefusal {
    #[error("it has no DACL, which lets everyone change it")]
    NullDacl,
    #[error("it is owned by {0}, which the installation does not trust")]
    UntrustedOwner(String),
    #[error("{0} may change it")]
    UntrustedWriter(String),
    #[error("it passes {0} a right to change what is created inside it")]
    UntrustedInheritedWriter(String),
    #[error("{0} may delete, rename or replace it or its entries")]
    UntrustedReplacer(String),
    #[error("it inherits its DACL from its parent instead of carrying its own")]
    InheritsDacl,
    #[error("a standard user cannot {0} it")]
    UnusableByUsers(&'static str),
}

/// The principals a Windows install trusts with its directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsOwnershipPolicy {
    trusted: Vec<WindowsSid>,
    users: Vec<WindowsSid>,
}

impl WindowsOwnershipPolicy {
    /// Trust the local system, Administrators and TrustedInstaller, which
    /// are the only principals that can change a machine-wide install.
    #[must_use]
    pub fn machine() -> Self {
        Self::trusting(std::iter::empty())
    }

    /// Trust the machine principals plus `extra`, as a test harness running
    /// beneath its own profile must.
    #[must_use]
    pub fn trusting(extra: impl IntoIterator<Item = WindowsSid>) -> Self {
        let parse = |sid: &str| WindowsSid::parse(sid).expect("well-known SIDs parse");
        let mut trusted = [LOCAL_SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER]
            .into_iter()
            .map(parse)
            .collect::<Vec<_>>();
        for sid in extra {
            if !trusted.contains(&sid) {
                trusted.push(sid);
            }
        }
        Self {
            trusted,
            users: STANDARD_USER_TOKEN.into_iter().map(parse).collect(),
        }
    }

    /// Require that only trusted principals own or can change an entry the
    /// installer manages, now or through what it passes to new children.
    ///
    /// # Errors
    /// Returns the first reason an untrusted principal could change it.
    pub(crate) fn require_installer_owned(
        &self,
        security: &WindowsSecurity,
    ) -> Result<(), WindowsDirectoryRefusal> {
        self.require_trusted_owner(security)?;
        for ace in security
            .aces
            .iter()
            .filter(|ace| ace.kind == WindowsAceKind::Allow && ace.writes())
        {
            if self.trusts(&ace.sid) {
                continue;
            }
            if ace.applies_to_self {
                return Err(WindowsDirectoryRefusal::UntrustedWriter(
                    ace.sid.to_string_form(),
                ));
            }
            let inherits = ace.inherits_to_files || ace.inherits_to_directories;
            let creator = CREATOR_PLACEHOLDERS
                .iter()
                .any(|placeholder| ace.sid.to_string_form() == *placeholder);
            if inherits && !creator {
                return Err(WindowsDirectoryRefusal::UntrustedInheritedWriter(
                    ace.sid.to_string_form(),
                ));
            }
        }
        Ok(())
    }

    /// Require that no untrusted principal can replace anything in a shared
    /// ancestor, though it may add new children.
    ///
    /// # Errors
    /// Returns the first reason an untrusted principal could replace an
    /// entry on the path to the store.
    pub(crate) fn require_ancestor(
        &self,
        security: &WindowsSecurity,
    ) -> Result<(), WindowsDirectoryRefusal> {
        self.require_trusted_owner(security)?;
        match security
            .grants()
            .find(|ace| ace.replaces() && !self.trusts(&ace.sid))
        {
            Some(ace) => Err(WindowsDirectoryRefusal::UntrustedReplacer(
                ace.sid.to_string_form(),
            )),
            None => Ok(()),
        }
    }

    /// Require the units directory's own DACL: installer-owned, not
    /// inherited from the release root, and letting standard users read and
    /// traverse it.
    ///
    /// # Errors
    /// Returns the first way the directory departs from that DACL.
    pub(crate) fn require_units_directory(
        &self,
        security: &WindowsSecurity,
    ) -> Result<(), WindowsDirectoryRefusal> {
        self.require_installer_owned(security)?;
        if !security.dacl_protected {
            return Err(WindowsDirectoryRefusal::InheritsDacl);
        }
        self.require_usable(security, true)
    }

    /// Require that a standard user can read an entry, and execute it (or
    /// traverse it, for a directory) when `execute` asks.
    ///
    /// # Errors
    /// Returns the use a standard user is refused.
    pub(crate) fn require_usable(
        &self,
        security: &WindowsSecurity,
        execute: bool,
    ) -> Result<(), WindowsDirectoryRefusal> {
        if execute && !security.grants_read_execute(&self.users) {
            return Err(WindowsDirectoryRefusal::UnusableByUsers("read and execute"));
        }
        if !security.grants_read(&self.users) {
            return Err(WindowsDirectoryRefusal::UnusableByUsers("read"));
        }
        Ok(())
    }

    /// The DACL a new units directory carries: full control for every
    /// trusted principal, read and execute for Users, all inherited by the
    /// units inside, and nothing inherited from the release root.
    pub(crate) fn units_sddl(&self) -> String {
        let mut sddl = String::from("D:P");
        for sid in &self.trusted {
            let _ = write!(sddl, "(A;OICI;FA;;;{})", sid.to_string_form());
        }
        let _ = write!(sddl, "(A;OICI;{USERS_READ_EXECUTE};;;BU)");
        sddl
    }

    /// Whether the policy trusts `sid` to change the installation.
    pub(crate) fn trusts(&self, sid: &WindowsSid) -> bool {
        self.trusted.contains(sid)
    }

    fn require_trusted_owner(
        &self,
        security: &WindowsSecurity,
    ) -> Result<(), WindowsDirectoryRefusal> {
        if security.null_dacl {
            return Err(WindowsDirectoryRefusal::NullDacl);
        }
        if !self.trusts(&security.owner) {
            return Err(WindowsDirectoryRefusal::UntrustedOwner(
                security.owner.to_string_form(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_platform_fs::{WindowsAce, WindowsAceKind, WindowsSecurity, WindowsSid};

    use super::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};

    const USERS: &str = "S-1-5-32-545";
    const EVERYONE: &str = "S-1-1-0";
    const READ_EXECUTE: u32 = 0x0012_00A9;
    const ADD_CHILDREN: u32 = 0x0000_0006;
    const FULL_CONTROL: u32 = 0x001F_01FF;
    const EXECUTE: u32 = 0x0000_0020;

    fn sid(text: &str) -> WindowsSid {
        WindowsSid::parse(text).expect("sid")
    }

    fn allow(principal: &str, mask: u32) -> WindowsAce {
        WindowsAce {
            kind: WindowsAceKind::Allow,
            sid: sid(principal),
            mask,
            applies_to_self: true,
            inherits_to_files: false,
            inherits_to_directories: false,
        }
    }

    fn security(owner: &str, aces: Vec<WindowsAce>) -> WindowsSecurity {
        WindowsSecurity {
            owner: sid(owner),
            null_dacl: false,
            dacl_protected: false,
            aces,
        }
    }

    #[test]
    fn a_machine_directory_admits_only_machine_writers() {
        let policy = WindowsOwnershipPolicy::machine();
        let program_files = security(
            super::TRUSTED_INSTALLER,
            vec![
                allow(super::LOCAL_SYSTEM, FULL_CONTROL),
                allow(super::ADMINISTRATORS, FULL_CONTROL),
                allow(USERS, READ_EXECUTE),
            ],
        );
        assert_eq!(policy.require_installer_owned(&program_files), Ok(()));
        assert_eq!(policy.require_ancestor(&program_files), Ok(()));

        let user_writable = security(
            super::ADMINISTRATORS,
            vec![
                allow(super::ADMINISTRATORS, FULL_CONTROL),
                allow(USERS, ADD_CHILDREN),
            ],
        );
        assert_eq!(
            policy.require_installer_owned(&user_writable),
            Err(WindowsDirectoryRefusal::UntrustedWriter(USERS.to_owned()))
        );
        assert_eq!(
            policy.require_ancestor(&user_writable),
            Ok(()),
            "adding children to a shared ancestor replaces nothing"
        );

        let user_replaceable = security(super::LOCAL_SYSTEM, vec![allow(USERS, FULL_CONTROL)]);
        assert_eq!(
            policy.require_ancestor(&user_replaceable),
            Err(WindowsDirectoryRefusal::UntrustedReplacer(USERS.to_owned()))
        );
    }

    #[test]
    fn a_write_right_passed_to_children_is_refused_unless_it_names_the_creator() {
        let policy = WindowsOwnershipPolicy::machine();
        let mut inherited = allow(USERS, FULL_CONTROL);
        inherited.applies_to_self = false;
        inherited.inherits_to_files = true;
        let state = security(
            super::LOCAL_SYSTEM,
            vec![
                allow(super::LOCAL_SYSTEM, FULL_CONTROL),
                allow(USERS, READ_EXECUTE),
                inherited,
            ],
        );
        assert_eq!(
            policy.require_installer_owned(&state),
            Err(WindowsDirectoryRefusal::UntrustedInheritedWriter(
                USERS.to_owned()
            ))
        );

        let mut creator = allow("S-1-3-0", FULL_CONTROL);
        creator.applies_to_self = false;
        creator.inherits_to_files = true;
        creator.inherits_to_directories = true;
        let program_data = security(
            super::LOCAL_SYSTEM,
            vec![allow(super::LOCAL_SYSTEM, FULL_CONTROL), creator],
        );
        assert_eq!(policy.require_installer_owned(&program_data), Ok(()));
    }

    #[test]
    fn units_carry_their_own_dacl_that_users_can_run() {
        let policy = WindowsOwnershipPolicy::machine();
        let mut units = security(
            super::LOCAL_SYSTEM,
            vec![
                allow(super::LOCAL_SYSTEM, FULL_CONTROL),
                allow(USERS, READ_EXECUTE),
            ],
        );
        assert_eq!(
            policy.require_units_directory(&units),
            Err(WindowsDirectoryRefusal::InheritsDacl)
        );
        units.dacl_protected = true;
        assert_eq!(policy.require_units_directory(&units), Ok(()));

        let mut execute_denied = units.clone();
        execute_denied.aces.insert(
            0,
            WindowsAce {
                kind: WindowsAceKind::Deny,
                ..allow(EVERYONE, EXECUTE)
            },
        );
        assert_eq!(
            policy.require_units_directory(&execute_denied),
            Err(WindowsDirectoryRefusal::UnusableByUsers("read and execute"))
        );
        assert_eq!(policy.require_usable(&execute_denied, false), Ok(()));

        let unreadable = security(
            super::LOCAL_SYSTEM,
            vec![allow(super::LOCAL_SYSTEM, FULL_CONTROL)],
        );
        assert_eq!(
            policy.require_usable(&unreadable, false),
            Err(WindowsDirectoryRefusal::UnusableByUsers("read"))
        );

        let sddl = WindowsOwnershipPolicy::trusting([sid("S-1-5-21-1-2-3-1001")]).units_sddl();
        assert!(sddl.starts_with("D:P(A;OICI;FA;;;S-1-5-18)"));
        assert!(sddl.contains("(A;OICI;FA;;;S-1-5-21-1-2-3-1001)"));
        assert!(sddl.ends_with("(A;OICI;0x1200a9;;;BU)"));
    }

    #[test]
    fn an_untrusted_owner_or_a_null_dacl_is_refused_everywhere() {
        let policy = WindowsOwnershipPolicy::machine();
        let user = "S-1-5-21-1-2-3-1001";
        let owned_by_user = security(user, vec![allow(super::ADMINISTRATORS, FULL_CONTROL)]);
        for result in [
            policy.require_installer_owned(&owned_by_user),
            policy.require_ancestor(&owned_by_user),
        ] {
            assert_eq!(
                result,
                Err(WindowsDirectoryRefusal::UntrustedOwner(user.to_owned()))
            );
        }
        let mut open = security(super::LOCAL_SYSTEM, vec![]);
        open.null_dacl = true;
        assert_eq!(
            policy.require_ancestor(&open),
            Err(WindowsDirectoryRefusal::NullDacl)
        );

        let trusting = WindowsOwnershipPolicy::trusting([sid(user)]);
        assert_eq!(trusting.require_installer_owned(&owned_by_user), Ok(()));
    }
}
