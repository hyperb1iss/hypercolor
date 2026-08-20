mod coordinator;
#[cfg(unix)]
mod linux;
mod model;
#[cfg(unix)]
mod payload;
mod store;

pub use coordinator::{
    InstallCoordinator, InstallCoordinatorError, InstallPlatform, InstallPlatformError,
};
#[cfg(unix)]
pub use linux::{
    LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS, LinuxDirectoryItem, LinuxDirectoryState,
    LinuxExactEntry, LinuxFilePublication, LinuxHttpResponse, LinuxInstallConfig,
    LinuxInstallExecutor, LinuxInstallPlatform, LinuxLayoutItem, LinuxLayoutPublication,
    LinuxLegacyFile, LinuxLegacySnapshot, LinuxNativeExecutor, LinuxProcessExecutable,
    LinuxPublicEntry, LinuxPublicTree, LinuxSystemdConnection, LinuxSystemdObservation,
    bind_linux_retained_unit, parse_systemd_show, retain_linux_unit,
};
pub use model::{
    INSTALL_JOURNAL_SCHEMA_VERSION, InstallAction, InstallDisposition, InstallJournalV1,
    InstallModelError, InstallOutcome, InstallRequest, InstallTargetPolicy, InstallTransactionId,
    InstallationState, MAX_INSTALL_JOURNAL_BYTES, MAX_PLATFORM_OWNER_RECEIPT_BYTES,
    MAX_PLATFORM_TRANSACTION_RECORD_BYTES, PlatformCheckpoint, PlatformOwnerReceipt, PlatformState,
    PlatformTransactionRecord, PlatformTransitionStates, PreparedPlatformTransaction, UnitId,
    UnitRecord,
};
#[cfg(unix)]
pub use payload::{
    MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES, ReleasePayloadError, stage_release_payload,
    stage_release_payload_from_authority, validate_release_payload,
    validate_release_payload_from_authority,
};
#[cfg(target_os = "macos")]
pub use payload::{MacosReleaseProvenance, bind_macos_release_provenance};
pub use store::{InstallLock, InstallStore, InstallStoreError};
