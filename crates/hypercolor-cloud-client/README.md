# hypercolor-cloud-client

*Daemon-side client for Hypercolor Cloud — OAuth, keyring, signed identity, and
sync.*

This crate provides everything the daemon needs to authenticate with and
communicate with the Hypercolor Cloud service. It wraps the contract types from
`hypercolor-cloud-api` and the tunnel protocol from `hypercolor-daemon-link` into
a high-level async client: Device Code OAuth flow, platform-native secret storage,
ed25519 daemon identity management, device registration, entitlement fetching, and
sync cursor management.

## Role and Position

`hypercolor-cloud-client` sits at the top of the three-crate cloud stack:

```
hypercolor-cloud-api   (REST contract types — no I/O)
       ↓
hypercolor-daemon-link (WebSocket framing, ed25519 identity protocol)
       ↓
hypercolor-cloud-client ← you are here (OAuth, keyring, sync, REST client)
       ↓
hypercolor-daemon       (feature-gated: `cloud` / `official-cloud`)
```

This crate is consumed exclusively by `hypercolor-daemon` under the `cloud`
feature. It re-exports `hypercolor-cloud-api` as `api` and
`hypercolor-daemon-link` as `daemon_link` for consumers that need both.

Platform keyring backends are selected via `[target]` dependencies:
`dbus-secret-service-keyring-store` on Linux, `apple-native-keyring-store` on
macOS, `windows-native-keyring-store` on Windows.

## Public Surface

All types are re-exported at the crate root:

- **Auth** — `DeviceAuthorizationSession`, `DeviceAuthorizationStatus`,
  `DeviceTokenPoll`, `persist_device_token`, poll-interval constants.
- **Client** — `CloudClient`, `CloudClientConfig`, path constants
  (`OAUTH_TOKEN_PATH`, `DAEMON_CONNECT_PATH`).
- **Connect** — `DaemonConnectRequest`, `DaemonConnectInput`,
  `StoredDaemonConnect`, `StoredDaemonConnectInput`, `connect_authority`.
- **Devices** — `DeviceRegistrationInput`, `signed_device_registration`,
  `DEVICE_REGISTRATION_PATH`.
- **Entitlements** — `ENTITLEMENTS_PATH`.
- **Secrets** — `SecretStore`, `KeyringSecretStore`, `CloudIdentity`,
  `CloudSecretKey`, `RefreshTokenOwner`, identity load/store/delete helpers,
  `KEYRING_SERVICE`.
- **Sync** — `SYNC_PATH`, `SyncCursor`, `SyncCursorError`.
- **Errors** — `CloudClientError`.
- Sync entity types re-exported from `hypercolor-cloud-api`:
  `SyncChange`, `SyncEntity`, `SyncEntityKind`, `SyncOp`, `ChangesResponse`,
  `SyncPutRequest`, `SyncConflictResponse`, `Etag`.

## Cargo Features

None defined. Platform keyring backends are selected automatically via
`[target]` dependencies in `Cargo.toml`.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Apache-2.0.
