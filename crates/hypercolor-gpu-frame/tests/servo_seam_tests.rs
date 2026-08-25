#![cfg(feature = "servo-context")]

use hypercolor_gpu_frame::servo::ServoGpuImportFailure;

#[derive(Debug, thiserror::Error)]
#[error("platform boom")]
struct PlatformError;

#[test]
fn failure_from_anyhow_keeps_the_platform_error_downcastable() {
    let failure = ServoGpuImportFailure::from(anyhow::Error::from(PlatformError));

    assert!(failure.diagnostics.is_none());
    assert!(
        failure
            .error
            .chain()
            .any(|cause| cause.downcast_ref::<PlatformError>().is_some()),
        "platform error lost in conversion: {:#}",
        failure.error
    );
}
