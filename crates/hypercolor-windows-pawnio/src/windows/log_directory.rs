//! The machine log directory, `%ProgramData%\Hypercolor\logs`.
//!
//! The SMBus broker (a LocalSystem service) and the hardware support setup
//! (an elevated installer step) keep their logs here rather than beside
//! the broker's executable, because an installed release is immutable and
//! nothing may write inside it at run time.
//!
//! Every user may add children to `%ProgramData%`, so a standard user could
//! create `Hypercolor` or `logs` first, or point one at another directory,
//! for a privileged writer to write through. A writer therefore reaches the
//! directory through retained handles that never follow a reparse point,
//! and judges each component before it relies on it:
//!
//! - `%ProgramData%` and `Hypercolor` are shared: other principals may add
//!   children, but only SYSTEM, Administrators or TrustedInstaller may own
//!   them, and no one else may delete, rename or replace their entries or
//!   change their security. `Hypercolor` also holds the update state.
//! - `logs` is the writers' own: only those principals may own it or hold
//!   any right to change it or what is created inside it.
//!
//! A missing `Hypercolor` or `logs` is created owned by Administrators with
//! a protected DACL: full control for SYSTEM and Administrators and read
//! for Users, inherited by everything inside. A component that fails its
//! judgement is refused, never repaired, so a privileged writer never
//! writes through a path a standard user prepared. The uninstaller removes
//! `logs` through the same judged handles, for the same reason.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use hypercolor_platform_fs::{
    WindowsAceKind, WindowsDirectory, WindowsDirectoryAccess, WindowsSecurity, WindowsSid,
};

/// The product directory under `%ProgramData%`.
const PRODUCT_DIRECTORY: &str = "Hypercolor";
/// The log directory under the product directory.
const LOG_DIRECTORY: &str = "logs";
/// Owned by Administrators, whatever the creating token's default owner
/// (an elevated administrator's own account on client editions); full
/// control for SYSTEM and Administrators, read and traverse for Users,
/// inherited by every file and directory inside; and nothing inherited
/// from `%ProgramData%`, which lets every user add children.
pub(super) const LOG_DIRECTORY_SDDL: &str =
    "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";

/// The local system account.
const LOCAL_SYSTEM: &str = "S-1-5-18";
/// The built-in Administrators group.
const ADMINISTRATORS: &str = "S-1-5-32-544";
/// `NT SERVICE\TrustedInstaller`, the owner of the system's own directories.
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
/// `CREATOR OWNER` resolves to the trusted principal creating a child.
const CREATOR_OWNER: &str = "S-1-3-0";

/// How much of a directory other principals may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ownership {
    /// Others may add children, but not replace entries or take it over.
    Shared,
    /// Others may change nothing in it, now or through what it passes on.
    Exclusive,
}

/// `%ProgramData%\Hypercolor\logs`, where the broker and the hardware
/// support setup write their logs. `None` when `ProgramData` is not set
/// to an absolute path.
#[must_use]
pub fn machine_log_directory() -> Option<PathBuf> {
    let program_data = PathBuf::from(std::env::var_os("ProgramData")?);
    program_data
        .is_absolute()
        .then(|| program_data.join(PRODUCT_DIRECTORY).join(LOG_DIRECTORY))
}

/// Open the machine log directory for writing, creating what is missing
/// and refusing any component an untrusted principal could change.
///
/// # Errors
///
/// Returns an error when `ProgramData` is unset, a component is a reparse
/// point or fails its judgement, or a directory cannot be opened or
/// created.
pub(super) fn open_machine_log_directory() -> io::Result<WindowsDirectory> {
    let root = open_program_data()?;
    let product = open_or_create(&root, PRODUCT_DIRECTORY, Ownership::Shared)?;
    open_or_create(&product, LOG_DIRECTORY, Ownership::Exclusive)
}

/// Remove the machine log directory and everything in it, through handles
/// that never follow a reparse point, once every component on the way
/// passes its judgement. A missing directory is already gone. A refused
/// component is left alone, so an elevated uninstaller never deletes
/// through a path a standard user prepared. `Hypercolor` itself stays,
/// locked down, since it also holds the update state.
///
/// # Errors
///
/// Returns an error when `ProgramData` is unset, a component is refused,
/// or the removal fails.
pub(super) fn remove_machine_log_directory() -> io::Result<()> {
    let root = open_program_data()?;
    let product_name = OsStr::new(PRODUCT_DIRECTORY);
    if root.entry_metadata(product_name)?.is_none() {
        return Ok(());
    }
    let product = root.open_child_directory(product_name, WindowsDirectoryAccess::Write)?;
    judge(&product, Ownership::Shared)?;
    let logs_name = OsStr::new(LOG_DIRECTORY);
    if product.entry_metadata(logs_name)?.is_none() {
        return Ok(());
    }
    let logs = product.open_child_directory(logs_name, WindowsDirectoryAccess::Write)?;
    judge(&logs, Ownership::Exclusive)?;
    drop(logs);
    product.remove_tree(logs_name)
}

fn open_program_data() -> io::Result<WindowsDirectory> {
    let program_data = machine_log_directory()
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "ProgramData does not name an absolute directory",
            )
        })?;
    let root = WindowsDirectory::open(&program_data, WindowsDirectoryAccess::Write)?;
    judge(&root, Ownership::Shared)?;
    Ok(root)
}

fn open_or_create(
    parent: &WindowsDirectory,
    name: &str,
    ownership: Ownership,
) -> io::Result<WindowsDirectory> {
    let name = OsStr::new(name);
    let directory = if parent.entry_metadata(name)?.is_some() {
        parent.open_child_directory(name, WindowsDirectoryAccess::Write)?
    } else {
        match parent.create_child_directory_with_security(name, LOG_DIRECTORY_SDDL) {
            Ok(directory) => directory,
            // Another writer created it first; judge what it made.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                parent.open_child_directory(name, WindowsDirectoryAccess::Write)?
            }
            Err(error) => return Err(error),
        }
    };
    judge(&directory, ownership)?;
    Ok(directory)
}

fn judge(directory: &WindowsDirectory, ownership: Ownership) -> io::Result<()> {
    match refusal(&directory.security()?, ownership) {
        None => Ok(()),
        Some(reason) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing to log into {}: {reason}",
                directory.path().display()
            ),
        )),
    }
}

/// Why a directory with `security` cannot hold the machine logs, or `None`
/// when it can. Write judgements never credit a deny entry, so a refusal
/// can be too strict but never too lenient.
pub(super) fn refusal(security: &WindowsSecurity, ownership: Ownership) -> Option<String> {
    if security.null_dacl {
        return Some("it has no DACL, which lets everyone change it".to_owned());
    }
    if !trusted(&security.owner) {
        return Some(format!(
            "it is owned by {}",
            security.owner.to_string_form()
        ));
    }
    let creator = WindowsSid::parse(CREATOR_OWNER).ok();
    for ace in security
        .aces
        .iter()
        .filter(|ace| ace.kind == WindowsAceKind::Allow && !trusted(&ace.sid))
    {
        let sid = ace.sid.to_string_form();
        match ownership {
            Ownership::Shared if ace.applies_to_self && ace.replaces() => {
                return Some(format!("{sid} may delete, rename or replace its entries"));
            }
            Ownership::Exclusive if ace.writes() && ace.applies_to_self => {
                return Some(format!("{sid} may change it"));
            }
            Ownership::Exclusive
                if ace.writes()
                    && (ace.inherits_to_files || ace.inherits_to_directories)
                    && creator.as_ref() != Some(&ace.sid) =>
            {
                return Some(format!(
                    "it passes {sid} a right to change what is created inside it"
                ));
            }
            _ => {}
        }
    }
    None
}

fn trusted(sid: &WindowsSid) -> bool {
    [LOCAL_SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER]
        .iter()
        .filter_map(|text| WindowsSid::parse(text).ok())
        .any(|trusted| &trusted == sid)
}

#[cfg(test)]
#[path = "log_directory_tests.rs"]
mod tests;
