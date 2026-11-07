//! Transactional release installer for Hypercolor.
//!
//! One durable install journal drives every install, upgrade, rollback and
//! recovery through [`InstallCoordinator`], on every platform. Platforms
//! apply and prove each step: `LinuxInstallPlatform` drives a per-user
//! systemd service, the `macos` module a launchd agent over a standalone
//! tree, the `macos_bundle` module a signed app bundle, and the `windows`
//! module a per-machine installation whose app runs in its user's session.
//! The raw `hypercolor __install-release` command is one host of this
//! library; others (an update activator, a daemon that observes its own
//! installation) link it directly.

mod coordinator;
#[cfg(unix)]
mod linux;
mod local_http;
#[cfg(unix)]
mod macho_cdhash;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(unix)]
pub mod macos_bundle;
mod model;
#[cfg(unix)]
mod ownership;
#[cfg(unix)]
mod payload;
mod release_selection;
#[cfg(unix)]
mod store;
pub mod windows;
#[cfg(windows)]
use windows::store;

pub use coordinator::{
    InstallCoordinator, InstallCoordinatorError, InstallPlatform, InstallPlatformError,
};
#[cfg(unix)]
pub use linux::{
    DEFAULT_PROBATION_WINDOW, InstallLocationError, LINUX_DIRECTORY_ITEMS, LINUX_LAYOUT_ITEMS,
    LINUX_PUBLIC_SERVICE_CONTRACT, LinuxAdoption, LinuxAdoptionError, LinuxDirectoryItem,
    LinuxDirectoryState, LinuxExactEntry, LinuxFilePublication, LinuxInstallAuthority,
    LinuxInstallCheckpoint, LinuxInstallCommandError, LinuxInstallConfig, LinuxInstallElection,
    LinuxInstallExecutor, LinuxInstallHost, LinuxInstallLocation, LinuxInstallLocator,
    LinuxInstallObservation, LinuxInstallPlatform, LinuxInstallRecords, LinuxInstallRequest,
    LinuxInstallRun, LinuxLayoutItem, LinuxLayoutPublication, LinuxLegacyFile, LinuxLegacySnapshot,
    LinuxLocatorError, LinuxManagedAuthority, LinuxNativeExecutor, LinuxObservationError,
    LinuxPlatformInputs, LinuxProcessExecutable, LinuxPublicEntry, LinuxPublicTree,
    LinuxRenderedService, LinuxRuntimeSettlement, LinuxServiceIdentity, LinuxServiceInput,
    LinuxServicePhase, LinuxServiceRenderer, LinuxServiceWatch, LinuxSystemdConnection,
    LinuxSystemdObservation, LinuxUninstallCheckpoint, LinuxUninstallHost, LinuxUninstallRun,
    MAX_PROBATION_WINDOW, RetainedLinuxInstallLocation, bind_linux_platform,
    bind_linux_retained_unit, elect_linux_installation, elect_linux_installation_with,
    observe_linux_installation, parse_systemd_show, retain_linux_unit, run_linux_install,
    run_linux_uninstall,
};
pub use local_http::LocalHttpResponse;
pub use model::{
    INSTALL_JOURNAL_SCHEMA_VERSION, InstallAction, InstallDisposition, InstallJournalV1,
    InstallModelError, InstallOutcome, InstallRequest, InstallTargetPolicy, InstallTransactionId,
    InstallationState, MAX_INSTALL_JOURNAL_BYTES, MAX_LINUX_TRANSACTION_RECORD_BYTES,
    MAX_MANAGED_INSTALL_JOURNAL_BYTES, MAX_PLATFORM_OWNER_RECEIPT_BYTES,
    MAX_PLATFORM_TRANSACTION_RECORD_BYTES, PlatformCheckpoint, PlatformOwnerReceipt, PlatformState,
    PlatformTransactionRecord, PlatformTransitionStates, PreparedPlatformTransaction,
    RestoredRelease, UnitId, UnitRecord,
};
#[cfg(unix)]
pub use ownership::{
    DirectoryRefusal, OwnershipPolicy, PrincipalDatabase, PrincipalGroup, PrincipalUser,
};
#[cfg(unix)]
pub use payload::{
    MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES, ReleasePayloadError, bind_macos_bundle_unit,
    copy_installed_release_unit, describe_macos_bundle_unit, retain_macos_bundle_unit,
    stage_macos_bundle_unit, stage_macos_bundle_unit_from_authority, stage_release_payload,
    stage_release_payload_from_authority, validate_release_payload,
    validate_release_payload_from_authority, verified_macos_bundle_cli,
};
#[cfg(target_os = "macos")]
pub use payload::{MacosReleaseProvenance, bind_macos_release_provenance};
pub use store::{InstallLock, InstallStore, InstallStoreError, UnitCollection};
#[cfg(windows)]
pub use windows::{
    MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES, ReleasePayloadError, stage_release_payload,
    validate_release_payload,
};
