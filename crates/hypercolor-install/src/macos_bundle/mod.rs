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

mod executor;
mod launchd;
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

pub use executor::MacosBundleExecutor;
pub use launchd::{
    MACOS_BUNDLE_START_DEADLINE, MacosBundleAgent, MacosBundleAgentJob, MacosBundleLaunchdService,
};
pub use model::{
    MACOS_BUNDLE_DISCARD_SUFFIX, MACOS_BUNDLE_NAME, MACOS_BUNDLE_PARTIAL_SUFFIX,
    MACOS_BUNDLE_STAGING_MARGIN_BYTES, MACOS_BUNDLE_STAGING_ROOT_NAME, MacosBundleConfig,
    MacosBundleIdentity, MacosBundleMaterialization, MacosBundleMember, MacosBundleMemberKind,
    MacosBundleObservation, MacosBundleRunningDaemon, MacosBundleServiceObservation,
    MacosBundleSlot, MacosBundleUnit,
};
#[cfg(target_os = "macos")]
pub use native::MacosBundleNativeExecutor;
#[cfg(target_os = "macos")]
pub use native_agent::MacosBundleNativeAgent;
pub use platform::MacosBundleInstallPlatform;
pub use service::MacosBundleService;
