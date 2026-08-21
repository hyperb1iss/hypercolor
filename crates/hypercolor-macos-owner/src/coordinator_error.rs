use crate::error::MacosOwnerStoreError;
use crate::model::{MacosDaemonOwner, MacosHandoverOperation};

/// Bounded failure returned by a typed local operation executor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct MacosOwnerExecutionError {
    detail: String,
}

impl MacosOwnerExecutionError {
    /// Construct an executor failure from a bounded operational detail.
    pub fn new(detail: impl Into<String>) -> Self {
        let mut detail = detail.into();
        detail.truncate(4_096);
        Self { detail }
    }
}

/// Typed local coordinator failure.
#[derive(Debug, thiserror::Error)]
pub enum MacosOwnerCoordinatorError {
    /// Durable owner or journal I/O failed.
    #[error(transparent)]
    Store(#[from] MacosOwnerStoreError),
    /// A typed local operation failed before rollback could finish.
    #[error("macOS daemon owner operation {operation:?} failed: {source}")]
    Operation {
        operation: MacosHandoverOperation,
        #[source]
        source: MacosOwnerExecutionError,
    },
    /// Inspecting one launcher's current installed state failed.
    #[error("failed to inspect {owner:?} autostart state: {source}")]
    InspectAutostart {
        owner: MacosDaemonOwner,
        #[source]
        source: MacosOwnerExecutionError,
    },
    /// The durable owner record does not exist.
    #[error("macOS daemon owner selection requires an active owner record")]
    MissingActiveOwner,
    /// Standalone is observable but has no local launcher to select.
    #[error("standalone daemon ownership cannot be selected by a coordinator")]
    StandaloneCannotBeSelected,
    /// Recovery attempted an operation absent from the validated journal.
    #[error("macOS handover journal does not authorize operation {operation:?}")]
    UnauthorizedOperation { operation: MacosHandoverOperation },
    /// A managed owner did not release the guard within ten seconds.
    #[error("macOS daemon guard did not release within the managed handover timeout")]
    GuardReleaseTimeout,
    /// A requested owner did not publish a matching owner epoch in time.
    #[error("requested macOS daemon owner did not publish before startup timeout")]
    OwnerStartupTimeout,
}
