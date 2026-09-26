//! Transactional release installer for Hypercolor.
//!
//! One durable install journal drives every install, upgrade, rollback and
//! recovery through [`InstallCoordinator`]. Platforms apply and prove each
//! step: [`LinuxInstallPlatform`] drives a per-user systemd service, and the
//! `macos` module a launchd agent. The raw `hypercolor __install-release`
//! command is one host of this library; others (an update activator, a
//! daemon that observes its own installation) link it directly.
#![cfg(unix)]

mod coordinator;
#[cfg(unix)]
mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
mod model;
#[cfg(unix)]
mod ownership;
#[cfg(unix)]
mod payload;
mod store;

pub use coordinator::{
    InstallCoordinator, InstallCoordinatorError, InstallPlatform, InstallPlatformError,
};
#[cfg(unix)]
pub use linux::{
    DEFAULT_PROBATION_WINDOW, InstallLocationError, LINUX_ACTIVATOR_DIRECTORY,
    LINUX_COORDINATOR_DIRECTORY, LINUX_DIRECTORY_ITEMS, LINUX_LAUNCH_COMMAND,
    LINUX_LAUNCHER_DIRECTORY, LINUX_LAUNCHER_PROGRAM, LINUX_LAYOUT_ITEMS, LinuxAdoption,
    LinuxAdoptionError, LinuxDirectoryItem, LinuxDirectoryState, LinuxExactEntry,
    LinuxFilePublication, LinuxHttpResponse, LinuxInstallAuthority, LinuxInstallCheckpoint,
    LinuxInstallCommandError, LinuxInstallConfig, LinuxInstallElection, LinuxInstallExecutor,
    LinuxInstallHost, LinuxInstallLocation, LinuxInstallLocator, LinuxInstallObservation,
    LinuxInstallPlatform, LinuxInstallRecords, LinuxInstallRequest, LinuxInstallRun,
    LinuxLaunchError, LinuxLaunchPlan, LinuxLaunchRequest, LinuxLaunchRole, LinuxLaunchSelection,
    LinuxLauncherProgram, LinuxLayoutItem, LinuxLayoutPublication, LinuxLegacyFile,
    LinuxLegacySnapshot, LinuxLocatorError, LinuxManagedAuthority, LinuxNativeExecutor,
    LinuxObservationError, LinuxPlatformInputs, LinuxProcessExecutable, LinuxPublicEntry,
    LinuxPublicTree, LinuxRuntimeSettlement, LinuxServiceIdentity, LinuxServicePhase,
    LinuxServiceWatch, LinuxSystemdConnection, LinuxSystemdObservation, LinuxUninstallCheckpoint,
    LinuxUninstallHost, LinuxUninstallRun, MAX_PROBATION_WINDOW, RetainedLinuxInstallLocation,
    bind_linux_platform, bind_linux_retained_unit, elect_linux_installation,
    elect_linux_installation_with, ensure_linux_launcher, ensure_linux_update_directories,
    inspect_linux_launcher, linux_launcher_path, linux_layout_directories,
    observe_linux_installation, parse_systemd_show, plan_linux_launch, retain_linux_unit,
    run_linux_install, run_linux_uninstall,
};
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
    CompatibilityDecision, CompatibilityRefusal, DeclaredCompatibility, DurableStoreDeclaration,
    LINUX_USER_TARBALL_OWNER, MANAGED_LAUNCHER_CONTRACT, MANAGED_PACKAGE_SCHEMA_VERSION,
    MAX_DURABLE_STORES, MAX_RELEASE_MANIFEST_BYTES, MAX_RELEASE_MEMBER_BYTES, MAX_RELEASE_MEMBERS,
    MAX_RELEASE_PATH_BYTES, MAX_RELEASE_PAYLOAD_BYTES, ManagedComponent, ManagedPackage,
    MigrationMode, ObservedStores, ReleasePayloadError, copy_installed_release_unit,
    declared_compatibility_from_manifest, evaluate_data_compatibility, read_declared_compatibility,
    stage_release_payload, stage_release_payload_from_authority, validate_release_payload,
    validate_release_payload_from_authority,
};
#[cfg(target_os = "macos")]
pub use payload::{MacosReleaseProvenance, bind_macos_release_provenance};
pub use store::{InstallLock, InstallStore, InstallStoreError, UnitCollection};
