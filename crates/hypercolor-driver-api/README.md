# hypercolor-driver-api

*Stable capability boundary between the Hypercolor daemon and modular driver implementations.*

This crate is the contract every driver depends on. It exports the trait surface
for discovery, pairing, output, controls, and protocol catalogs, along with the
supporting request/response types and host-service abstractions. Drivers import
only this crate; they never reach into daemon internals. The `DRIVER_API_SCHEMA_VERSION`
constant enforces compatibility at registration time: any driver compiled against a
different schema version is rejected by the registry.

## Position in the Workspace

- Depends on: `hypercolor-types`, `serde`, `serde_json`, `tracing`, `anyhow`,
  `async-trait`, `utoipa`, `tokio`, `mdns-sd`, `aes-gcm`, `rand`
- Consumed by: every network driver crate (`hypercolor-driver-hue`,
  `hypercolor-driver-nanoleaf`, `hypercolor-driver-wled`, `hypercolor-driver-govee`),
  HAL catalog wrappers in `hypercolor-driver-builtin`, `hypercolor-network`, and
  `hypercolor-daemon`
- Does NOT depend on `hypercolor-core` — this is the deliberate stable boundary

## Key Public Surface

**Root capability traits**

- `DriverModule` — root trait; optional sub-traits are returned as `Option<&dyn …>`
- `DeviceBackend`, `DeviceFrameSink`, `DeviceDisplaySink` — hardware output interfaces
- `DiscoveryCapability`, `PairingCapability`, `DriverControlProvider`,
  `DriverRuntimeCacheProvider`, `DriverProtocolCatalog`, `DriverPresentationProvider`

**Host-side service interfaces (injected into driver calls)**

- `DriverHost`, `DriverCredentialStore`, `DriverRuntimeActions`, `DriverDiscoveryState`,
  `DriverControlHost`

**Protocol types**

- `DriverDescriptor`, `DRIVER_API_SCHEMA_VERSION`
- `DeviceAuthState`, `DeviceAuthSummary`, `PairingDescriptor`, `PairDeviceRequest`,
  `PairDeviceOutcome`, `ClearPairingOutcome`
- `DriverDiscoveredDevice`, `DiscoveryRequest`, `DiscoveryResult`
- `ValidatedControlChanges`, `DriverConfigProvider`, `DriverConfigView`
- `CredentialStore` (re-exported from `net::credentials`), `MdnsBrowser`, `MdnsService`

**Support utilities**

- `support` module: `activate_if_requested`, `disconnect_after_unpair`,
  `metadata_value`, `network_ip_from_metadata`, `network_port_from_metadata`
- `validation` module: IP and port sanitization helpers

## Cargo Features

None. All types are unconditionally available.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
