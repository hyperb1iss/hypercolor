//! The Windows machine layout of the release store (RFC 57, B1.4.1).
//!
//! Releases live as immutable units under `%ProgramFiles%\Hypercolor\units`,
//! and the transaction state under `%ProgramData%\Hypercolor\update`: the
//! journal, the lock and `active.json`, the pointer that names the active
//! unit. Both roots are machine-wide. [`WindowsOwnershipPolicy`] proves that
//! only the principals it trusts can change them, or replace anything on
//! the path to them, before a transaction relies on either.

mod ownership;
pub(crate) mod store;

pub use ownership::{WindowsDirectoryRefusal, WindowsOwnershipPolicy};
