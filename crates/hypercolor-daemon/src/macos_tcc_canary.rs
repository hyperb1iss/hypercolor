#[cfg(not(target_os = "macos"))]
compile_error!("the macos-tcc-canary feature drives ScreenCaptureKit and only builds on macOS");

mod artifacts;
mod harness_protocol;
mod identity;
mod model;
mod receipts;
mod rows;
#[cfg(all(test, feature = "screen-capture"))]
mod tests;
mod validation;

pub use artifacts::{
    arm_macos_tcc_canary, macos_tcc_canary_directory, macos_tcc_canary_request_path,
    publish_macos_tcc_canary_artifact, validate_macos_tcc_canary_request,
};
#[cfg(feature = "screen-capture")]
pub use harness_protocol::run_armed_macos_tcc_canary;
pub use model::{
    MACOS_TCC_CANARY_SCHEMA_VERSION, MacosTccCanaryCapability, MacosTccCanaryInstallationScenario,
    MacosTccCanaryLifecyclePhase, MacosTccCanaryOutcome, MacosTccCanaryRequest,
};
pub use receipts::{
    MacosTccCanaryCapabilityEvidence, MacosTccCanaryCapabilityQualification,
    MacosTccCanaryLauncherEvidence, MacosTccCanaryReceipt, MacosTccCanarySigningEvidence,
    MacosTccCanaryValidation, MacosTccCanaryWitness, MacosTccCanaryWitnessKind,
};
pub use validation::validate_macos_tcc_canary_receipts;
