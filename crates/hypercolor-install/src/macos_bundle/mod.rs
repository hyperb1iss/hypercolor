//! The macOS app bundle install platform.
//!
//! A release unit is `manifest.json`, the store inventory and the signed
//! `Hypercolor.app` beside it. Installing one stages a verified clone or
//! durable copy of its bundle next to the installed bundle and exchanges
//! the two with `renamex_np` and `RENAME_SWAP`, so the bundle players
//! launch is always a complete signed bundle.
//!
//! Every decision lives here and compiles on every unix host, so it is
//! tested against fakes anywhere. The macOS system calls live behind
//! [`MacosBundleExecutor`] in `native`, and the daemon's service manager
//! behind [`MacosBundleService`], which [`MacosBundleLaunchdService`]
//! implements over [`MacosBundleAgent`], native in `native_agent`.
//!
//! A bundle reaches the platform through [`adopt_macos_bundle`], which turns
//! what the DMG left in Applications into its first unit. The update agents
//! run the launcher, which picks the release that may run, in `launcher`.

mod adoption;
mod agents;
mod executor;
mod installation;
mod launchd;
mod launcher;
mod model;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod native;
#[cfg(target_os = "macos")]
mod native_agent;
mod platform;
mod record;
mod service;
mod swap;

#[cfg(test)]
mod tests;

pub use adoption::{
    MacosBundleAdoptionHost, MacosBundleAdoptionOutcome, MacosBundleAdoptionRequest,
    adopt_macos_bundle, macos_bundle_in_applications,
};
pub use agents::{
    MACOS_BUNDLE_DAEMON_LABEL, MACOS_BUNDLE_UPDATE_ACTIVATE_LABEL,
    MACOS_BUNDLE_UPDATE_PREPARE_LABEL, MACOS_BUNDLE_UPDATE_RECOVER_LABEL,
    MacosBundleAgentDefinition, macos_bundle_daemon_agent, macos_bundle_update_agents,
};
pub use executor::MacosBundleExecutor;
pub use installation::{
    MACOS_BUNDLE_INSTALLATION_FILE, MACOS_BUNDLE_LAUNCHER_NAME, MacosBundleInstallation,
    MacosBundleRoots,
};
pub use launchd::{
    MACOS_BUNDLE_START_DEADLINE, MacosBundleAgent, MacosBundleAgentJob, MacosBundleLaunchdService,
};
pub use launcher::{
    MacosBundleLauncherError, run_macos_bundle_launcher_role, select_macos_bundle_update_executor,
};
pub use model::{
    MACOS_BUNDLE_DISCARD_SUFFIX, MACOS_BUNDLE_INVENTORY, MACOS_BUNDLE_NAME,
    MACOS_BUNDLE_PARTIAL_SUFFIX, MACOS_BUNDLE_STAGING_MARGIN_BYTES, MACOS_BUNDLE_STAGING_ROOT_NAME,
    MacosBundleConfig, MacosBundleIdentity, MacosBundleMaterialization, MacosBundleMember,
    MacosBundleMemberKind, MacosBundleObservation, MacosBundleRunningDaemon,
    MacosBundleServiceObservation, MacosBundleSlot, MacosBundleUnit,
};
#[cfg(target_os = "macos")]
pub use native::MacosBundleNativeExecutor;
#[cfg(target_os = "macos")]
pub use native_agent::MacosBundleNativeAgent;
pub use platform::MacosBundleInstallPlatform;
pub use service::MacosBundleService;
