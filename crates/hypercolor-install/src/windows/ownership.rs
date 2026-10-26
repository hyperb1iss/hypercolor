//! Which principals may change the directories a Windows install relies on.
//!
//! The store roots and the units directory are the installer's own: only a
//! trusted principal may own them or hold any write-class right on them.
//! Their ancestors are shared system directories, where other principals
//! may add children (as `C:\ProgramData` lets every user) but must not be
//! able to delete, rename or replace an existing entry or take over the
//! directory's security. Deny entries are never credited, so a refusal can
//! be too strict but never too lenient.

use hypercolor_platform_fs::{WindowsSecurity, WindowsSid};

/// The local system account.
const LOCAL_SYSTEM: &str = "S-1-5-18";
/// The built-in Administrators group.
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// `NT SERVICE\TrustedInstaller`, the owner of the system's own directories.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

/// Why a directory cannot be trusted by a Windows install.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowsDirectoryRefusal {
    #[error("it has no DACL, which lets everyone change it")]
    NullDacl,
    #[error("it is owned by {0}, which the installation does not trust")]
    UntrustedOwner(String),
    #[error("{0} may change it")]
    UntrustedWriter(String),
    #[error("{0} may delete, rename or replace it or its entries")]
    UntrustedReplacer(String),
}

/// The principals a Windows install trusts with its directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsOwnershipPolicy {
    trusted: Vec<WindowsSid>,
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
        let mut trusted = [LOCAL_SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER]
            .into_iter()
            .map(|sid| WindowsSid::parse(sid).expect("well-known SIDs parse"))
            .collect::<Vec<_>>();
        for sid in extra {
            if !trusted.contains(&sid) {
                trusted.push(sid);
            }
        }
        Self { trusted }
    }

    /// Require that only trusted principals own or can change a directory
    /// the installer manages.
    ///
    /// # Errors
    /// Returns the first reason an untrusted principal could change it.
    pub(crate) fn require_installer_owned(
        &self,
        security: &WindowsSecurity,
    ) -> Result<(), WindowsDirectoryRefusal> {
        self.require_trusted_owner(security)?;
        match security
            .grants
            .iter()
            .find(|grant| grant.writes() && !self.trusted.contains(&grant.sid))
        {
            Some(grant) => Err(WindowsDirectoryRefusal::UntrustedWriter(
                grant.sid.to_string_form(),
            )),
            None => Ok(()),
        }
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
            .grants
            .iter()
            .find(|grant| grant.replaces() && !self.trusted.contains(&grant.sid))
        {
            Some(grant) => Err(WindowsDirectoryRefusal::UntrustedReplacer(
                grant.sid.to_string_form(),
            )),
            None => Ok(()),
        }
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
        if !self.trusted.contains(&security.owner) {
            return Err(WindowsDirectoryRefusal::UntrustedOwner(
                security.owner.to_string_form(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hypercolor_platform_fs::{WindowsGrant, WindowsSecurity, WindowsSid};

    use super::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};

    const USERS: &str = "S-1-5-32-545";
    const READ_EXECUTE: u32 = 0x0012_00A9;
    const ADD_CHILDREN: u32 = 0x0000_0006;
    const FULL_CONTROL: u32 = 0x001F_01FF;

    fn sid(text: &str) -> WindowsSid {
        WindowsSid::parse(text).expect("sid")
    }

    fn security(owner: &str, grants: &[(&str, u32)]) -> WindowsSecurity {
        WindowsSecurity {
            owner: sid(owner),
            null_dacl: false,
            grants: grants
                .iter()
                .map(|(principal, mask)| WindowsGrant {
                    sid: sid(principal),
                    mask: *mask,
                })
                .collect(),
        }
    }

    #[test]
    fn a_machine_directory_admits_only_machine_writers() {
        let policy = WindowsOwnershipPolicy::machine();
        let program_files = security(
            super::TRUSTED_INSTALLER,
            &[
                (super::LOCAL_SYSTEM, FULL_CONTROL),
                (super::ADMINISTRATORS, FULL_CONTROL),
                (USERS, READ_EXECUTE),
            ],
        );
        assert_eq!(policy.require_installer_owned(&program_files), Ok(()));
        assert_eq!(policy.require_ancestor(&program_files), Ok(()));

        let user_writable = security(
            super::ADMINISTRATORS,
            &[(super::ADMINISTRATORS, FULL_CONTROL), (USERS, ADD_CHILDREN)],
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

        let user_replaceable = security(super::LOCAL_SYSTEM, &[(USERS, FULL_CONTROL)]);
        assert_eq!(
            policy.require_ancestor(&user_replaceable),
            Err(WindowsDirectoryRefusal::UntrustedReplacer(USERS.to_owned()))
        );
    }

    #[test]
    fn an_untrusted_owner_or_a_null_dacl_is_refused_everywhere() {
        let policy = WindowsOwnershipPolicy::machine();
        let user = "S-1-5-21-1-2-3-1001";
        let owned_by_user = security(user, &[(super::ADMINISTRATORS, FULL_CONTROL)]);
        for result in [
            policy.require_installer_owned(&owned_by_user),
            policy.require_ancestor(&owned_by_user),
        ] {
            assert_eq!(
                result,
                Err(WindowsDirectoryRefusal::UntrustedOwner(user.to_owned()))
            );
        }
        let mut open = security(super::LOCAL_SYSTEM, &[]);
        open.null_dacl = true;
        assert_eq!(
            policy.require_ancestor(&open),
            Err(WindowsDirectoryRefusal::NullDacl)
        );

        let trusting = WindowsOwnershipPolicy::trusting([sid(user)]);
        assert_eq!(trusting.require_installer_owned(&owned_by_user), Ok(()));
    }
}
