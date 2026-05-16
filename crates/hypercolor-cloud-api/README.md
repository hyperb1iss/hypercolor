# hypercolor-cloud-api

*Shared API contract types for Hypercolor Cloud — pure data, no I/O.*

This crate defines the serialization shapes for everything the Hypercolor daemon
and cloud service exchange over HTTP. It has no runtime logic, no network code,
and no external I/O — only serde-derived structs and enums that both sides of the
connection depend on.

## Role and Position

`hypercolor-cloud-api` sits at the base of the three-crate cloud stack:

```
hypercolor-cloud-api   ← you are here (contract types, no I/O)
       ↓
hypercolor-daemon-link (WebSocket framing and ed25519 identity protocol)
       ↓
hypercolor-cloud-client (daemon-side OAuth, keyring, sync — used by the daemon)
```

Everything above this crate depends on it; nothing here depends on them.
Dependencies are minimal: `chrono`, `serde`, `serde_json`, `ulid`, `uuid`.

## Public Surface

All types are re-exported at the crate root:

- **Auth** — OAuth Device Code flow: `DeviceCodeRequest`, `DeviceCodeResponse`,
  `DeviceTokenRequest`, `DeviceTokenResponse`, `RefreshTokenRequest`,
  `DeviceTokenError`, `DeviceTokenErrorCode`.
- **Devices** — `DeviceRegistrationRequest`, `DeviceRegistrationResponse`,
  `DeviceInstallation`.
- **Entitlements** — `EntitlementClaims`, `EntitlementTokenResponse`,
  `FeatureKey`, `RateLimits`.
- **Envelope** — `ApiEnvelope<T>`, `ApiMeta`, `ProblemDetails` (the standard
  response wrapper used by every REST endpoint).
- **Sync** — `SyncChange`, `SyncEntity`, `SyncEntityKind`, `SyncOp`,
  `ChangesResponse`, `SyncPutRequest`, `SyncConflictResponse`, `Etag`.
- **Updates** — `UpdateManifest`, `ReleaseInfo`, `PlatformArtifact`,
  `ArtifactKind`, `ReleaseChannel`, `RollbackTarget`.

## Cargo Features

None. This crate is always compiled in full.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Apache-2.0.
