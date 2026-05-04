#![forbid(unsafe_code)]

pub mod auth;
pub mod devices;
pub mod entitlements;
pub mod envelope;
pub mod sync;
pub mod updates;

pub use auth::{
    DEVICE_CODE_GRANT_TYPE, DeviceCodeRequest, DeviceCodeResponse, DeviceTokenError,
    DeviceTokenErrorCode, DeviceTokenRequest, DeviceTokenResponse,
};
pub use devices::{DeviceInstallation, DeviceRegistrationRequest, DeviceRegistrationResponse};
pub use entitlements::{EntitlementClaims, FeatureKey, RateLimits};
pub use envelope::{ApiEnvelope, ApiMeta, ProblemDetails};
pub use sync::{Etag, SyncChange, SyncEntity, SyncEntityKind, SyncOp};
pub use updates::{
    ArtifactKind, PlatformArtifact, ReleaseChannel, ReleaseInfo, RollbackTarget, UpdateManifest,
};
