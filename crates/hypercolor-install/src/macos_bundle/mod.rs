//! The macOS app bundle install platform.
//!
//! A release unit is `manifest.json`, the store inventory and the signed
//! `Hypercolor.app` beside it, with nothing added inside the bundle.

mod model;

#[cfg(test)]
mod tests;

pub use model::{
    MACOS_BUNDLE_DISCARD_SUFFIX, MACOS_BUNDLE_NAME, MACOS_BUNDLE_PARTIAL_SUFFIX,
    MACOS_BUNDLE_STAGING_MARGIN_BYTES, MACOS_BUNDLE_STAGING_ROOT_NAME, MacosBundleConfig,
    MacosBundleIdentity, MacosBundleMaterialization, MacosBundleMember, MacosBundleMemberKind,
    MacosBundleObservation, MacosBundleRunningDaemon, MacosBundleServiceObservation,
    MacosBundleSlot, MacosBundleUnit,
};
