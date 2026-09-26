//! Writer policy for directories the raw installer relies on.
//!
//! Directories the installer creates and owns (the release store, the state
//! store and the historical locator root) must never be writable by another
//! principal. Ancestors and shared application directories that it does not
//! own (HOME, the XDG base directories, and the daemon's data and
//! configuration directories) may additionally be group-writable, but only
//! when that group is provably the installing user's private group. Every
//! other state that lets another principal write fails closed.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};

use hypercolor_platform_fs::{
    DirectoryAuthority, DirectoryEntryKind, DirectoryEntryMetadata, PublicDirectoryAuthority,
    ReadOnlyDirectoryAuthority,
};

#[cfg(target_os = "linux")]
#[path = "ownership_nss.rs"]
mod nss;

const OWNER_ACCESS: u32 = 0o700;
const GROUP_WRITE: u32 = 0o020;
const OTHER_WRITE: u32 = 0o002;

/// How the installer relates to one directory it validates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectoryRole {
    /// Created and exclusively managed by the installer.
    InstallerOwned,
    /// An ancestor or shared application directory the installer does not own.
    Ancestor,
}

/// A retained directory handle whose access ACL can be probed.
///
/// Platforms without the probe report an ACL as present, which keeps the
/// group-write exception unavailable there.
pub(crate) trait AclProbe {
    fn extended_access_acl(&self) -> io::Result<bool>;
}

macro_rules! acl_probe {
    ($authority:ty) => {
        impl AclProbe for $authority {
            fn extended_access_acl(&self) -> io::Result<bool> {
                #[cfg(target_os = "linux")]
                {
                    self.has_extended_access_acl()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Ok(true)
                }
            }
        }
    };
}

acl_probe!(PublicDirectoryAuthority);
acl_probe!(ReadOnlyDirectoryAuthority);
acl_probe!(DirectoryAuthority);

/// Why a directory cannot be trusted as writable only by the installing user.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirectoryRefusal {
    #[error("it is not a directory")]
    NotDirectory,
    #[error("it is owned by uid {0}, not the installing user")]
    ForeignOwner(u32),
    #[error("it is owned by uid {actual}, but the installation records uid {recorded}")]
    RecordedOwnerMismatch { recorded: u32, actual: u32 },
    #[error("it lacks owner read, write and search permission")]
    OwnerAccess,
    #[error("it is writable by every user")]
    WorldWritable,
    #[error("installer-owned directories must not be group-writable")]
    InstallerOwnedGroupWritable,
    #[error("it carries an extended access ACL that may grant other principals write")]
    ExtendedAcl,
    #[error("it is group-writable by gid {gid}, which is not proven private: {reason}")]
    SharedGroup { gid: u32, reason: String },
}

/// One user record from the system principal database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalUser {
    pub name: String,
    pub uid: u32,
    pub primary_gid: u32,
}

/// One group record from the system principal database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalGroup {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
}

/// Read access to the user and group databases used for private-group proof.
///
/// Implementations must report lookup and enumeration failures as errors
/// rather than empty results; any error refuses the group-write exception.
pub trait PrincipalDatabase: fmt::Debug + Send + Sync {
    /// Look up one user by numeric ID.
    ///
    /// # Errors
    /// Returns an error when the database cannot answer authoritatively.
    fn user_by_uid(&self, uid: u32) -> io::Result<Option<PrincipalUser>>;

    /// Look up one group by numeric ID.
    ///
    /// # Errors
    /// Returns an error when the database cannot answer authoritatively.
    fn group_by_gid(&self, gid: u32) -> io::Result<Option<PrincipalGroup>>;

    /// Enumerate every user record.
    ///
    /// # Errors
    /// Returns an error when enumeration is unsupported or incomplete.
    fn all_users(&self) -> io::Result<Vec<PrincipalUser>>;

    /// Enumerate every group record.
    ///
    /// # Errors
    /// Returns an error when enumeration is unsupported or incomplete.
    fn all_groups(&self) -> io::Result<Vec<PrincipalGroup>>;
}

/// Decides which principals may write the directories an install relies on.
#[derive(Clone, Debug)]
pub struct OwnershipPolicy {
    private_groups: Option<Arc<PrivateGroups>>,
}

#[derive(Debug)]
struct PrivateGroups {
    database: Arc<dyn PrincipalDatabase>,
    decisions: Mutex<BTreeMap<(u32, u32), Result<(), String>>>,
}

impl OwnershipPolicy {
    /// Refuse every group- or world-writable directory.
    #[must_use]
    pub fn strict() -> Self {
        Self {
            private_groups: None,
        }
    }

    /// Accept group-writable ancestors whose group the database proves private.
    #[must_use]
    pub fn with_private_groups(database: Arc<dyn PrincipalDatabase>) -> Self {
        Self {
            private_groups: Some(Arc::new(PrivateGroups {
                database,
                decisions: Mutex::new(BTreeMap::new()),
            })),
        }
    }

    /// The policy an ordinary install uses on this platform.
    ///
    /// Linux proves private groups through the system NSS databases. Other
    /// platforms keep the strict policy.
    #[must_use]
    pub fn system() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::with_private_groups(Arc::new(nss::NssPrincipalDatabase))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::strict()
        }
    }

    /// Require that only the installing user can change a retained directory.
    ///
    /// `metadata` must come from `directory`, whose ACL is probed only when a
    /// group-write exception is otherwise possible.
    ///
    /// # Errors
    /// Returns the first reason another principal might modify the directory.
    pub(crate) fn require_owner_only(
        &self,
        directory: &impl AclProbe,
        metadata: DirectoryEntryMetadata,
        role: DirectoryRole,
    ) -> Result<(), DirectoryRefusal> {
        self.require_writable_only_by_owner(metadata, role, || directory.extended_access_acl())
    }

    pub(crate) fn require_writable_only_by_owner(
        &self,
        metadata: DirectoryEntryMetadata,
        role: DirectoryRole,
        extended_acl: impl FnOnce() -> io::Result<bool>,
    ) -> Result<(), DirectoryRefusal> {
        if metadata.kind() != DirectoryEntryKind::Directory {
            return Err(DirectoryRefusal::NotDirectory);
        }
        if !metadata.is_owned_by_current_user() {
            return Err(DirectoryRefusal::ForeignOwner(metadata.owner_uid()));
        }
        if metadata.mode() & OWNER_ACCESS != OWNER_ACCESS {
            return Err(DirectoryRefusal::OwnerAccess);
        }
        if metadata.mode() & OTHER_WRITE != 0 {
            return Err(DirectoryRefusal::WorldWritable);
        }
        if metadata.mode() & GROUP_WRITE == 0 {
            return Ok(());
        }
        if role == DirectoryRole::InstallerOwned {
            return Err(DirectoryRefusal::InstallerOwnedGroupWritable);
        }
        let gid = metadata.owner_gid();
        let Some(groups) = &self.private_groups else {
            return Err(DirectoryRefusal::SharedGroup {
                gid,
                reason: "this platform does not accept group-writable ancestors".to_owned(),
            });
        };
        match extended_acl() {
            Ok(false) => {}
            Ok(true) => return Err(DirectoryRefusal::ExtendedAcl),
            Err(error) => {
                return Err(DirectoryRefusal::SharedGroup {
                    gid,
                    reason: format!("its access ACL could not be inspected: {error}"),
                });
            }
        }
        groups
            .prove(metadata.owner_uid(), gid)
            .map_err(|reason| DirectoryRefusal::SharedGroup { gid, reason })
    }
}

impl Default for OwnershipPolicy {
    fn default() -> Self {
        Self::system()
    }
}

impl PrivateGroups {
    fn prove(&self, uid: u32, gid: u32) -> Result<(), String> {
        let Ok(mut decisions) = self.decisions.lock() else {
            return Err("private-group decisions are unavailable".to_owned());
        };
        decisions
            .entry((uid, gid))
            .or_insert_with(|| prove_private_group(self.database.as_ref(), uid, gid))
            .clone()
    }
}

/// Prove that `gid` is `uid`'s private group in the principal database.
///
/// The group must be the user's primary group, list no member other than the
/// user, and be the primary group of no other user. Enumeration must include
/// the user and the group itself; a database that cannot show them cannot
/// prove absence of other principals either.
///
/// # Errors
/// Returns a human-readable reason whenever privacy is not proven.
pub(crate) fn prove_private_group(
    database: &dyn PrincipalDatabase,
    uid: u32,
    gid: u32,
) -> Result<(), String> {
    let user = database
        .user_by_uid(uid)
        .map_err(|error| format!("user {uid} lookup failed: {error}"))?
        .ok_or_else(|| format!("user {uid} is not in the user database"))?;
    if user.uid != uid {
        return Err(format!("user lookup for {uid} returned uid {}", user.uid));
    }
    if user.primary_gid != gid {
        return Err(format!(
            "it is not the primary group ({}) of user {}",
            user.primary_gid, user.name
        ));
    }
    let group = database
        .group_by_gid(gid)
        .map_err(|error| format!("group {gid} lookup failed: {error}"))?
        .ok_or_else(|| format!("group {gid} is not in the group database"))?;
    require_only_member(&group, &user)?;

    let users = database
        .all_users()
        .map_err(|error| format!("user enumeration failed: {error}"))?;
    if !users
        .iter()
        .any(|entry| entry.uid == uid && entry.name == user.name)
    {
        return Err(format!(
            "user enumeration does not include {}, so it cannot prove absence",
            user.name
        ));
    }
    if let Some(other) = users
        .iter()
        .find(|entry| entry.primary_gid == gid && entry.uid != uid)
    {
        return Err(format!(
            "user {} (uid {}) also has primary group {gid}",
            other.name, other.uid
        ));
    }

    let groups = database
        .all_groups()
        .map_err(|error| format!("group enumeration failed: {error}"))?;
    let mut same_gid = groups.iter().filter(|entry| entry.gid == gid).peekable();
    if same_gid.peek().is_none() {
        return Err(format!(
            "group enumeration does not include gid {gid}, so it cannot prove absence"
        ));
    }
    for entry in same_gid {
        require_only_member(entry, &user)?;
    }
    Ok(())
}

fn require_only_member(group: &PrincipalGroup, user: &PrincipalUser) -> Result<(), String> {
    if group.gid != user.primary_gid {
        return Err(format!(
            "group lookup for {} returned gid {}",
            user.primary_gid, group.gid
        ));
    }
    match group.members.iter().find(|member| **member != user.name) {
        Some(member) => Err(format!(
            "group {} ({}) also lists member {member}",
            group.name, group.gid
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod tests;
