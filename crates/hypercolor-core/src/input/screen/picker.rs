//! Picker-driven source selection, decoded from source diagnostics.
//!
//! Backends that resolve their capture source through an interactive picker
//! report the accepted choice as a revisioned selection in their diagnostics
//! envelope. This module turns that platform payload into one neutral
//! snapshot so the daemon can persist the accepted choice without naming
//! the backend. Backends without a picker selection report nothing, and the
//! caller treats the absence as "no persistence observer is needed".

use hypercolor_macos_capture::{MacosCaptureSelection, screen_selection_snapshot};
use hypercolor_types::source_status::SourceDiagnosticsEnvelope;

/// One revisioned picker selection as reported by a capture backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerSelectionSnapshot {
    /// Monotonic selection revision; each accepted pick advances it.
    pub revision: u64,
    /// Source string to persist for this selection, or `None` when the
    /// backend currently has nothing selected.
    pub persisted_source: Option<String>,
}

/// What a picker persistence observer should do with a new snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerPersistenceDecision {
    /// The selection has not moved past the baseline yet.
    Wait,
    /// A strictly newer selection arrived; persist this source string.
    Persist(String),
    /// A strictly newer revision cleared the selection; stop observing.
    Cancel,
}

/// Decode the picker selection a backend reports through its diagnostics.
///
/// Returns `None` when the envelope does not carry a picker selection, which
/// is the case for every backend that resolves sources without a picker or
/// that persists its own restore token.
#[must_use]
pub fn picker_selection_snapshot(
    envelope: &SourceDiagnosticsEnvelope,
) -> Option<PickerSelectionSnapshot> {
    let snapshot = screen_selection_snapshot(envelope).ok()??;
    let persisted_source = match snapshot.selection {
        MacosCaptureSelection::None => None,
        MacosCaptureSelection::Display { source_id } => Some(source_id.to_string()),
        MacosCaptureSelection::SessionScoped { .. } => Some("session_scoped".to_owned()),
    };
    Some(PickerSelectionSnapshot {
        revision: snapshot.revision,
        persisted_source,
    })
}

/// Decide whether a snapshot observed after a picker dispatch should persist.
///
/// Only a revision strictly newer than the baseline captured before the
/// picker opened counts, so a stale snapshot can never replay an old choice.
#[must_use]
pub fn picker_persistence_decision(
    baseline_revision: u64,
    snapshot: &PickerSelectionSnapshot,
) -> PickerPersistenceDecision {
    if snapshot.revision <= baseline_revision {
        return PickerPersistenceDecision::Wait;
    }
    snapshot.persisted_source.clone().map_or(
        PickerPersistenceDecision::Cancel,
        PickerPersistenceDecision::Persist,
    )
}

#[cfg(test)]
mod tests {
    use super::{PickerPersistenceDecision, PickerSelectionSnapshot, picker_persistence_decision};

    #[test]
    fn persistence_requires_a_strictly_newer_accepted_selection() {
        let display = PickerSelectionSnapshot {
            revision: 7,
            persisted_source: Some("display:7a3f4954-3d72-47a6-a914-16ef68d02122".to_owned()),
        };
        assert_eq!(
            picker_persistence_decision(7, &display),
            PickerPersistenceDecision::Wait
        );
        assert_eq!(
            picker_persistence_decision(
                6,
                &PickerSelectionSnapshot {
                    revision: 8,
                    ..display.clone()
                }
            ),
            PickerPersistenceDecision::Persist(
                "display:7a3f4954-3d72-47a6-a914-16ef68d02122".to_owned()
            )
        );
        assert_eq!(
            picker_persistence_decision(
                7,
                &PickerSelectionSnapshot {
                    revision: 8,
                    persisted_source: None,
                }
            ),
            PickerPersistenceDecision::Cancel
        );
    }
}
