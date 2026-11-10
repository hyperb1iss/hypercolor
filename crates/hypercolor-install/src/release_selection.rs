//! Which release a program outside any unit must start.
//!
//! A launcher and a recovery bootstrap read the journal before the
//! pointer. While a transaction is open the pointer may already name its
//! candidate, which nothing has proven yet, so they take the transaction's
//! prior instead. Neither program ever holds the install lock, and only
//! the journal says what the pointer means, so the journal is read again
//! after the pointer: a transaction that opened in between would otherwise
//! leave a stale "nothing is pending" beside a candidate's pointer.
//!
//! The two part ways once the journal records the candidate's proof. All
//! that is left then is the commit record, which recovery writes without
//! starting or stopping anything, so the app the launcher starts from that
//! point on is the candidate's: a prior started at sign-in would keep
//! running under a commit that names the candidate. Recovery itself still
//! runs from the prior until the journal is terminal, since the prior's
//! code reads the journal it wrote.
//!
//! Both programs outlive the releases they select. An update replaces
//! neither the launcher nor, as a matter of course, the bootstrap, so they
//! read only the journal fields that decide this, not its whole schema.
//! Platforms store the pointer their own way; Windows keeps `active.json`,
//! and each [`ReleaseRecords`] decodes what it reads.

use std::io;

use serde::{Deserialize, Serialize};

use crate::model::{InstallAction, InstallDisposition, UnitId};

const ACTIVE_POINTER_SCHEMA_VERSION: u32 = 1;
/// A journal only changes between two reads when a transaction opens, and
/// the next round then finds it pending, so more rounds mean records that
/// are not being written by the transaction protocol at all.
const SELECTION_ATTEMPTS: usize = 3;

/// The pointer that names the active unit, `active.json`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivePointer {
    schema_version: u32,
    unit: UnitId,
}

/// Encode the pointer to `unit`.
#[cfg(any(windows, test))]
pub(crate) fn encode_active_pointer(unit: &UnitId) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&ActivePointer {
        schema_version: ACTIVE_POINTER_SCHEMA_VERSION,
        unit: unit.clone(),
    })
}

/// Decode a pointer, with the reason it is invalid.
pub(crate) fn decode_active_pointer(bytes: &[u8]) -> Result<UnitId, String> {
    let pointer: ActivePointer =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if pointer.schema_version != ACTIVE_POINTER_SCHEMA_VERSION {
        return Err(format!(
            "unsupported pointer schema {}",
            pointer.schema_version
        ));
    }
    Ok(pointer.unit)
}

/// The fields of the journal that decide what runs.
#[derive(Debug, Deserialize)]
struct JournalDecision {
    disposition: InstallDisposition,
    prior_active_unit: Option<UnitId>,
    #[serde(default)]
    candidate_unit: Option<UnitId>,
    /// Kept as written, so an action this program does not know reads as
    /// "not the commit" rather than as a journal it refuses.
    #[serde(default)]
    next_action: Option<serde_json::Value>,
}

impl JournalDecision {
    const fn is_pending(&self) -> bool {
        matches!(
            self.disposition,
            InstallDisposition::Forward | InstallDisposition::Rollback
        )
    }

    /// The candidate, once the journal records its proof and only the
    /// commit record is left to write.
    fn proven_candidate(&self) -> Option<&UnitId> {
        let commit_is_next = self.disposition == InstallDisposition::Forward
            && self
                .next_action
                .as_ref()
                .and_then(|action| InstallAction::deserialize(action).ok())
                == Some(InstallAction::Commit);
        self.candidate_unit.as_ref().filter(|_| commit_is_next)
    }
}

/// Who a selection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Selecting {
    /// The code that drives a transaction to its end.
    Recovery,
    /// The app a person, or their sign-in, starts.
    App,
}

/// The two records selection reads, each as one consistent snapshot.
pub trait ReleaseRecords {
    /// The journal's bytes, or `None` when no transaction ever ran.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the journal cannot be read or trusted.
    fn read_journal(&mut self) -> io::Result<Option<Vec<u8>>>;

    /// The pointer's bytes, or `None` before the first commit.
    ///
    /// # Errors
    ///
    /// Returns the host's error when the pointer cannot be read or trusted.
    fn read_pointer(&mut self) -> io::Result<Option<Vec<u8>>>;

    /// The unit `bytes`, as [`Self::read_pointer`] returned them, name.
    ///
    /// # Errors
    ///
    /// Returns why the pointer is invalid.
    fn decode_pointer(&self, bytes: &[u8]) -> Result<UnitId, String> {
        decode_active_pointer(bytes)
    }
}

/// Why no release can be named.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseSelectionError {
    #[error("could not read the installation's records: {0}")]
    Read(#[source] io::Error),
    #[error("the install journal is invalid: {0}")]
    InvalidJournal(#[source] serde_json::Error),
    #[error("the active pointer is invalid: {0}")]
    InvalidPointer(String),
    /// A first install is still open, so its candidate is all there is.
    #[error("a transaction is open and has no prior release")]
    NoPrior,
    #[error("the installation has no active release")]
    NoActive,
    #[error("the installation's records would not hold still")]
    Unsettled,
}

/// The unit whose code runs now: the transaction's prior while a journal
/// is nonterminal, otherwise the active unit. Never a candidate that has
/// not committed.
///
/// # Errors
///
/// Returns an error for records that cannot be read or decoded, and when
/// neither rule names a unit. It never settles for another release.
pub fn select_release<R: ReleaseRecords>(records: &mut R) -> Result<UnitId, ReleaseSelectionError> {
    select(records, Selecting::Recovery)
}

/// The unit whose app a launcher starts now: as [`select_release`], except
/// that a candidate whose proof the journal records is the one to run.
///
/// # Errors
///
/// As [`select_release`].
pub fn select_app_release<R: ReleaseRecords>(
    records: &mut R,
) -> Result<UnitId, ReleaseSelectionError> {
    select(records, Selecting::App)
}

fn select<R: ReleaseRecords>(
    records: &mut R,
    selecting: Selecting,
) -> Result<UnitId, ReleaseSelectionError> {
    for _ in 0..SELECTION_ATTEMPTS {
        let journal = records
            .read_journal()
            .map_err(ReleaseSelectionError::Read)?;
        if let Some(bytes) = &journal {
            let decision: JournalDecision =
                serde_json::from_slice(bytes).map_err(ReleaseSelectionError::InvalidJournal)?;
            if decision.is_pending() {
                if selecting == Selecting::App
                    && let Some(candidate) = decision.proven_candidate()
                {
                    return Ok(candidate.clone());
                }
                return decision
                    .prior_active_unit
                    .ok_or(ReleaseSelectionError::NoPrior);
            }
        }
        let pointer = records
            .read_pointer()
            .map_err(ReleaseSelectionError::Read)?;
        let confirmed = records
            .read_journal()
            .map_err(ReleaseSelectionError::Read)?;
        if confirmed == journal {
            let bytes = pointer.ok_or(ReleaseSelectionError::NoActive)?;
            return records
                .decode_pointer(&bytes)
                .map_err(ReleaseSelectionError::InvalidPointer);
        }
    }
    Err(ReleaseSelectionError::Unsettled)
}

#[cfg(test)]
#[path = "release_selection_tests.rs"]
mod tests;
