//! Shared Hypercolor Cloud API contract types.
//!
//! Pure-data library: serde-derived request/response types for OAuth Device Code
//! auth, device registration, entitlements, sync, and update manifests. No I/O
//! or logic. Consumed by `hypercolor-daemon-link` and `hypercolor-cloud-client`.

#![forbid(unsafe_code)]

pub mod auth;
pub mod devices;
pub mod entitlements;
pub mod envelope;
pub mod sync;
pub mod updates;

pub use auth::{
    DEVICE_CODE_GRANT_TYPE, DeviceCodeRequest, DeviceCodeResponse, DeviceTokenError,
    DeviceTokenErrorCode, DeviceTokenRequest, DeviceTokenResponse, REFRESH_TOKEN_GRANT_TYPE,
    RefreshTokenRequest,
};
pub use devices::{DeviceInstallation, DeviceRegistrationRequest, DeviceRegistrationResponse};
pub use entitlements::{EntitlementClaims, EntitlementTokenResponse, FeatureKey, RateLimits};
pub use envelope::{ApiEnvelope, ApiMeta, ProblemDetails};
pub use sync::{
    ChangesResponse, Etag, SyncChange, SyncConflictResponse, SyncEntity, SyncEntityKind, SyncOp,
    SyncPutRequest,
};
pub use updates::{
    ArtifactKind, PlatformArtifact, ReleaseChannel, ReleaseInfo, RollbackTarget, UpdateManifest,
};
