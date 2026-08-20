use std::time::Duration;

use thiserror::Error;

pub use hypercolor_types::device::ErrorRecoverability;

/// Errors produced while configuring or constructing a driver capability.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DriverError {
    /// Driver configuration is invalid.
    #[error("invalid driver configuration: {message}")]
    Configuration {
        /// Validation failure detail.
        message: String,
    },
    /// Driver construction exceeded its deadline.
    #[error("driver operation timed out after {after:?}")]
    Timeout {
        /// Elapsed deadline.
        after: Duration,
    },
    /// The driver capability graph violates the host contract.
    #[error("driver contract violation: {message}")]
    Contract {
        /// Contract failure detail.
        message: String,
    },
    /// Backend construction failed.
    #[error("driver backend construction failed: {source}")]
    BackendConstruction {
        /// Construction failure from the concrete driver.
        #[source]
        source: anyhow::Error,
    },
}

impl DriverError {
    /// Classify the recovery action for this failure.
    #[must_use]
    pub const fn recoverability(&self) -> ErrorRecoverability {
        match self {
            Self::Timeout { .. } => ErrorRecoverability::Retry,
            Self::Configuration { .. }
            | Self::Contract { .. }
            | Self::BackendConstruction { .. } => ErrorRecoverability::Permanent,
        }
    }
}

impl From<anyhow::Error> for DriverError {
    fn from(source: anyhow::Error) -> Self {
        Self::BackendConstruction { source }
    }
}
