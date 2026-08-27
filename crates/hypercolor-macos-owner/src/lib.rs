//! Durable macOS daemon ownership and handover state.

mod coordinator;
mod coordinator_error;
mod direct_launchd;
mod effects;
mod error;
mod executor;
#[cfg(target_os = "macos")]
mod guard;
#[cfg(target_os = "macos")]
mod guard_coordinate;
mod journal;
mod launchd_adapter;
mod model;
#[cfg(target_os = "macos")]
mod process_identity;
#[cfg(target_os = "macos")]
mod process_lifetime;
mod publication;
mod store;
mod store_io;
#[cfg(all(test, target_os = "macos"))]
mod tests;
mod validation;

pub use coordinator::{
    MacosOwnerCoordinatorOutcome, MacosOwnerRemedy, choose_daemon_owner, recover_daemon_owner,
    recover_incoming_daemon_owner,
};
pub use coordinator_error::{MacosOwnerCoordinatorError, MacosOwnerExecutionError};
pub use direct_launchd::{
    MACOS_DIRECT_LAUNCHD_LABEL, MacosDirectLaunchdBootstrapExpectation,
    MacosDirectLaunchdBootstrapSource, MacosDirectLaunchdExecutableExpectation,
    MacosDirectLaunchdInspector, MacosDirectLaunchdMutationOutcome, MacosDirectLaunchdMutator,
    MacosDirectLaunchdOwnerProof, MacosDirectLaunchdPublicationExpectation,
    MacosDirectLaunchdState, corroborate_direct_launchd_owner,
    corroborate_newer_direct_launchd_owner, parse_direct_launchd_autostart_state,
    parse_direct_launchd_service_state, wait_for_exact_direct_launchd_publication,
};
#[cfg(target_os = "macos")]
pub use direct_launchd::{
    NativeMacosDirectLaunchdInspector, NativeMacosDirectLaunchdMutator,
    validate_retained_macos_executable,
};
pub use error::MacosOwnerStoreError;
pub use executor::MacosOwnerExecutor;
#[cfg(target_os = "macos")]
pub use guard::{
    MacosDaemonGuard, acquire_macos_daemon_guard, try_acquire_macos_daemon_guard,
    wait_for_macos_guard_release,
};
#[cfg(target_os = "macos")]
pub use guard_coordinate::canonical_macos_daemon_guard_path;
pub use journal::{MacosHandoverJournal, MacosHandoverTransactionId};
pub use launchd_adapter::{
    LAUNCHCTL_PATH, LaunchdAdapter, LaunchdTarget, launch_agent_plist, launchctl_service_disabled,
    parse_launchctl_print_pid,
};
pub use model::{
    MACOS_APP_BUNDLE_BINARY_NAMES, MACOS_APP_BUNDLE_EXECUTABLE_RELATIVE_PATH,
    MACOS_APP_LAUNCH_AGENT_PLIST_FILE_NAME, MACOS_APP_PRODUCT_NAME,
    MACOS_DAEMON_SESSION_ATTESTATION_FILE_NAME, MACOS_DAEMON_SESSION_ATTESTATION_SCHEMA_VERSION,
    MACOS_HANDOVER_JOURNAL_FILE_NAME, MACOS_HANDOVER_JOURNAL_SCHEMA_VERSION,
    MACOS_MANAGED_HANDOVER_TIMEOUT, MACOS_OWNER_COORDINATION_LOCK_FILE_NAME,
    MACOS_OWNER_RECORD_FILE_NAME, MACOS_OWNER_RECORD_SCHEMA_VERSION,
    MACOS_STANDALONE_HANDOVER_TIMEOUT, MAX_MACOS_AUDIT_TOKEN_IDENTITY_BYTES,
    MAX_MACOS_DESIGNATED_REQUIREMENT_HASH_BYTES, MAX_MACOS_EXECUTABLE_PATH_BYTES,
    MAX_MACOS_HANDOVER_OPERATIONS, MAX_MACOS_OWNER_ARTIFACT_BYTES, MacosAutostartStates,
    MacosConflictUpdate, MacosDaemonOwner, MacosDaemonSessionAttestation, MacosExternalOwnerMode,
    MacosHandoverOperation, MacosHandoverPhase, MacosOwnerConflict, MacosOwnerConflictRecord,
    MacosOwnerIdentity, MacosOwnerIncarnation, MacosOwnerRecord, MacosOwnerRecoveryRequired,
    MacosOwnerSnapshot, MacosServerSessionId,
};
#[cfg(target_os = "macos")]
pub use process_identity::{request_macos_child_termination, request_macos_pid_termination};
#[cfg(target_os = "macos")]
pub use process_lifetime::wait_for_process_exit;
pub use publication::wait_for_owner_publication;
pub use store::MacosOwnerStore;

pub(crate) use validation::validate_bounded_identity_text;
