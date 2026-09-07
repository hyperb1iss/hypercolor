use std::path::PathBuf;

use crate::model::MacosHandoverPhase;

/// Typed durable owner-store failure.
#[derive(Debug, thiserror::Error)]
pub enum MacosOwnerStoreError {
    /// The explicit data directory could not be created.
    #[error("failed to create macOS owner data directory {path}: {source}")]
    CreateDirectory {
        /// Data directory.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The stable coordination lock could not be opened.
    #[error("failed to open macOS owner coordination lock {path}: {source}")]
    OpenCoordinationLock {
        /// Lock path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The stable coordination lock could not be acquired.
    #[error("failed to acquire macOS owner coordination lock {path}: {source}")]
    AcquireCoordinationLock {
        /// Lock path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A durable artifact could not be read.
    #[error("failed to read macOS {artifact} at {path}: {source}")]
    Read {
        /// Artifact kind.
        artifact: &'static str,
        /// Artifact path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A durable artifact could not be decoded.
    #[error("failed to decode macOS {artifact}: {source}")]
    Decode {
        /// Artifact kind.
        artifact: &'static str,
        /// JSON failure.
        #[source]
        source: serde_json::Error,
    },
    /// A durable artifact has an unsupported schema version.
    #[error("unsupported macOS {artifact} schema version {found}; expected {expected}")]
    UnsupportedVersion {
        /// Artifact kind.
        artifact: &'static str,
        /// Version found on disk.
        found: u32,
        /// Version supported by this build.
        expected: u32,
    },
    /// A durable artifact violates a semantic invariant.
    #[error("invalid macOS {artifact}: {detail}")]
    InvalidArtifact {
        /// Artifact kind.
        artifact: &'static str,
        /// Stable validation detail.
        detail: &'static str,
    },
    /// JSON serialization failed before any bytes were replaced.
    #[error("failed to serialize macOS {artifact}: {source}")]
    Encode {
        /// Artifact kind.
        artifact: &'static str,
        /// JSON failure.
        #[source]
        source: serde_json::Error,
    },
    /// A same-directory temporary file could not be created.
    #[error("failed to create temporary file beside {path}: {source}")]
    CreateTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A complete temporary artifact could not be written.
    #[error("failed to write temporary file for {path}: {source}")]
    WriteTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// Temporary artifact contents could not be synced.
    #[error("failed to sync temporary file for {path}: {source}")]
    SyncTemporary {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The durable destination could not be atomically replaced.
    #[error("failed to atomically replace {path}: {source}")]
    Replace {
        /// Destination path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The parent directory could not be synced after replacement.
    #[cfg(unix)]
    #[error("failed to sync parent directory {path}: {source}")]
    SyncDirectory {
        /// Parent directory.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A matching daemon-session attestation could not be removed.
    #[error("failed to remove macOS daemon session attestation at {path}: {source}")]
    RemoveSessionAttestation {
        /// Attestation path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// No owner record exists for the requested mutation.
    #[error("macOS owner record does not exist")]
    MissingOwnerRecord,
    /// The owner acquisition epoch cannot advance further.
    #[error("macOS owner epoch overflow")]
    OwnerEpochOverflow,
    /// A nonterminal handover journal must be recovered first.
    #[error("macOS handover {transaction_id} is still pending")]
    HandoverAlreadyPending {
        /// Existing transaction identifier.
        transaction_id: String,
    },
    /// No handover journal exists for the requested mutation.
    #[error("macOS handover journal does not exist")]
    MissingHandoverJournal,
    /// A caller attempted to advance a different transaction.
    #[error("macOS handover transaction does not match the durable journal")]
    HandoverTransactionMismatch,
    /// A concurrent recovery participant already advanced the journal.
    #[error("macOS handover phase changed from {expected:?} to {found:?}")]
    HandoverPhaseChanged {
        /// Phase expected by the caller.
        expected: MacosHandoverPhase,
        /// Current durable phase.
        found: MacosHandoverPhase,
    },
    /// The handover journal revision cannot advance further.
    #[error("macOS handover journal revision overflow")]
    JournalRevisionOverflow,
    /// A transaction identifier is not a bounded path-free token.
    #[error("macOS handover transaction ID must be 1-64 ASCII letters, digits, '_' or '-'")]
    InvalidTransactionId,
    /// An owner identity field is empty, oversized, or structurally invalid.
    #[error("invalid macOS owner identity field {field}: {detail}")]
    InvalidOwnerIdentity {
        /// Invalid identity field.
        field: &'static str,
        /// Stable validation detail.
        detail: &'static str,
    },
    /// A durable artifact exceeds the bounded decoder input size.
    #[error("macOS {artifact} exceeds the {maximum_bytes}-byte limit")]
    ArtifactTooLarge {
        /// Artifact kind.
        artifact: &'static str,
        /// Maximum accepted byte length.
        maximum_bytes: usize,
    },
    /// A completed or rolled-back transaction cannot be advanced.
    #[error("terminal macOS handover {transaction_id} cannot advance")]
    TerminalHandover {
        /// Completed transaction identifier.
        transaction_id: String,
    },
}
