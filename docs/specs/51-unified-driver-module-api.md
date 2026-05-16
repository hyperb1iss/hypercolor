# Spec 51 - Unified Driver Module API

> A single internal driver-module model for network drivers, HAL protocols,
> built-in backends, and future Wasm-loaded extensions.

**Status:** Substantially implemented
**Author:** Nova
**Date:** 2026-04-26
**Crates:** `hypercolor-types`, `hypercolor-driver-api`, `hypercolor-network`, `hypercolor-hal`, `hypercolor-core`, `hypercolor-daemon`, `hypercolor-ui`
**Related:** `docs/specs/35-network-driver-architecture.md`, `docs/specs/50-extensible-config-registry.md`, `docs/specs/45-nollie-protocol-driver.md`, `docs/specs/02-device-backend.md`, `docs/specs/34-device-fingerprints.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Design Principles](#4-design-principles)
5. [Terminology](#5-terminology)
6. [Target Architecture](#6-target-architecture)
7. [Crate Boundaries](#7-crate-boundaries)
8. [Core Types](#8-core-types)
9. [Driver Module Contract](#9-driver-module-contract)
10. [Capabilities](#10-capabilities)
11. [Discovery and Routing](#11-discovery-and-routing)
12. [Network Driver Integration](#12-network-driver-integration)
13. [HAL Driver Integration](#13-hal-driver-integration)
14. [Daemon Integration](#14-daemon-integration)
15. [API and UI Introspection](#15-api-and-ui-introspection)
16. [Render-Path Isolation](#16-render-path-isolation)
17. [Future Wasm Shape](#17-future-wasm-shape)
18. [Migration Plan](#18-migration-plan)
19. [Verification](#19-verification)
20. [Recommendation](#20-recommendation)

---

## 1. Overview

Hypercolor started this track with two driver worlds:

- Network drivers such as WLED, Hue, and Nanoleaf are modular crates with a
  `DriverModule` implementation, discovery and pairing capabilities, config
  slices, and daemon host services.
- HAL drivers such as Nollie, PrismRGB, Lian Li, ASUS, Corsair, Razer, QMK,
  and Dygma are protocol descriptors inside `hypercolor-hal`, discovered
  through static VID/PID databases and routed through shared output backends
  like `usb` or `smbus`.

Those worlds differ for good reasons. Network devices own sockets, pairing,
credentials, mDNS, HTTP probing, and sometimes one backend per vendor. HAL
devices usually share transports and only need to contribute protocol
descriptors, topology, capabilities, and model-specific config.

The problem was not that their internals differ. The problem was that
Hypercolor needed one clean vocabulary above those internals.

This spec introduces a unified **driver module** layer. A driver module is the
unit of extension, metadata, config, and capability discovery. Some modules
build output backends directly. Some modules contribute protocols to a shared
transport backend. Some modules expose pairing. Some modules expose nothing but
metadata and protocol descriptors. The daemon talks to modules through one
capability registry and routes discovered devices through explicit origin
metadata.

The target mental model:

```text
driver_id   = "nollie"          # who owns the device semantics
backend_id  = "usb"             # which output backend writes frames
protocol_id = "nollie/nollie32" # which protocol implementation encodes frames

driver_id   = "wled"
backend_id  = "wled"
protocol_id = "wled/ddp"
```

For WLED, `driver_id` and `backend_id` happen to match. For Nollie, they do
not. That distinction is the whole spell.

### 1.1 Implementation Status

Implemented:

- driver-owned config under `drivers.<id>`
- `DeviceOrigin` on discovered and API-visible devices
- built-in driver registration through `hypercolor-driver-builtin`
- network and HAL modules in one `DriverModuleRegistry`
- driver-scoped runtime cache and encrypted credentials
- driver presentation metadata in `/api/v1/drivers` and device summaries
- UI device cards and pairing surfaces reading driver presentation metadata

Remaining:

- settings/discovery UI should be fully generated from driver control metadata
- future Wasm host services still need value-shaped bindings over the native host adapters

---

## 2. Problem Statement

Current state:

- Built-in registration now flows through driver crates, but
  `hypercolor-driver-api` still re-exports several native boundary traits from
  `hypercolor-core`.
- `DeviceInfo` exposes driver ownership and output routing as separate helpers:
  `driver_id()` and `output_backend_id()`.
- Discovery metadata no longer carries ad hoc `"backend_id"` routing overrides.
- HAL protocol descriptors live behind `ProtocolDatabase`, while network
  drivers live behind `DriverRegistry`.
- Runtime session state now stores driver runtime caches by driver id.
- Native host utilities still expose credential and mDNS helpers through
  `hypercolor-driver-api`; the longer-term Wasm boundary should replace these
  with value-shaped host services.
- Some settings and discovery UI surfaces still need to become fully metadata-driven.
- HAL-specific attachment/profile behavior now flows through host-managed
  attachment profile sync, with protocol-specific config resolved on the HAL side.

That creates five concrete problems.

1. **Identity and routing are conflated**

   `DeviceFamily` should describe hardware identity. It should not decide which
   backend handles I/O. A device can be Nollie-owned and USB-routed, or
   Ableton-owned and MIDI/USB-routed, or WLED-owned and WLED-routed.

2. **The daemon still knows too much**

   The daemon should orchestrate discovery, lifecycle, state, and API surfaces.
   It should not contain logic that knows how Hue credentials are shaped, where
   WLED probe caches live, or when Prism S needs a custom protocol config.

3. **HAL and network drivers do not compose through one registry**

   Network driver discovery and pairing are capability-driven. HAL drivers are
   database-driven. Both should register as driver modules, even if the
   capabilities they expose differ.

4. **The UI cannot become generic**

   A future external driver should be able to provide display name, accent,
   connection type, pairing hints, settings fields, and device class metadata
   without a UI patch.

5. **Future Wasm loading would inherit today's seams**

   If dynamic Wasm drivers are added before this boundary is cleaned up, the
   Wasm host would need to reproduce the current special cases. That would
   fossilize the wrong architecture.

---

## 3. Goals and Non-Goals

### Goals

- Define one internal driver-module API for network and HAL drivers.
- Keep HAL protocol code free of `hypercolor-core` dependencies.
- Preserve `DeviceBackend` as the hot-path output interface.
- Stop using `DeviceFamily` as a runtime routing oracle.
- Represent discovered-device origin explicitly.
- Move driver config, cache, credential, presentation, and metadata ownership
  behind driver capabilities.
- Keep built-in native Rust drivers first.
- Shape all boundary types so they can later map to Wasm Component Model WIT
  exports and host imports.
- Provide a migration plan that can land in small, verifiable waves while other
  config namespace work is active.

### Non-Goals

- Build dynamic Wasm driver loading now.
- Replace `DeviceBackend` frame-output internals.
- Force every HAL protocol into its own crate immediately.
- Make USB, SMBus, MIDI, HID, and network transports identical internally.
- Move low-level transport code out of `hypercolor-hal`.
- Require full JSON Schema in the first pass.
- Remove all backend IDs. Backend IDs are still valid output routing keys.

---

## 4. Design Principles

### 4.1 Driver Module Is The Extension Unit

A driver module owns a stable `driver_id`, metadata, config, and capabilities.
It may own one protocol, many protocols, one backend, or many logical device
families.

Examples:

| Module | Owns | Routes Through |
| ------ | ---- | -------------- |
| `wled` | WLED discovery, config, DDP/E1.31 selection | `wled` backend |
| `hue` | Hue bridge discovery, pairing, Entertainment stream | `hue` backend |
| `nanoleaf` | Nanoleaf discovery, pairing, External Control stream | `nanoleaf` backend |
| `nollie` | Nollie/Prism 8 protocol descriptors and topology | `usb` backend |
| `prismrgb` | Prism S/Mini protocol descriptors and topology | `usb` backend |
| `lianli` | Lian Li HID/vendor-control descriptors | `usb` backend |
| `asus` | ASUS USB and SMBus descriptors | `usb`, `smbus` backends |

### 4.2 Capabilities Over Inheritance

There should not be separate "network driver" and "HAL driver" super-traits
that both pretend to be everything. Modules expose only the capabilities they
support.

Network modules usually expose:

- config
- discovery
- pairing
- backend factory
- credential schema
- runtime cache
- presentation metadata

HAL modules usually expose:

- config
- protocol catalog
- topology/config application
- presentation metadata

### 4.3 Device Origin Is Explicit

Every discovered device should carry explicit origin data:

```rust
pub struct DeviceOrigin {
    pub driver_id: String,
    pub backend_id: String,
    pub transport: DriverTransportKind,
    pub protocol_id: Option<String>,
}
```

The daemon should not infer this from `DeviceFamily`, device name, or scattered
metadata keys.

### 4.4 Values At Boundaries, Rich Types Inside

The unified driver boundary should use serializable value types for metadata,
config, cache, credentials, discovery, and presentation. Native Rust drivers can
parse those values into rich typed structs internally.

This keeps the future Wasm path clean because WIT can express records, variants,
lists, strings, numbers, booleans, and option/result shapes, but it cannot share
Rust trait objects or daemon internals.

### 4.5 Core Owns Runtime, Drivers Own Meaning

The daemon owns:

- config loading and persistence
- encrypted credential storage
- runtime cache storage
- lifecycle execution
- discovery orchestration
- backend registration
- API and WebSocket transport

Driver modules own:

- driver-specific defaults and validation
- device-specific discovery payloads
- pairing/auth semantics
- protocol descriptors and topology meaning
- presentation metadata
- cache payload meaning

### 4.6 Transport Backends Stay Specialized

`DeviceBackend` remains the frame-output path. It is intentionally optimized
for steady-state writing. The unified module layer sits above it; it does not
erase the need for specialized USB, SMBus, Hue, WLED, or Nanoleaf internals.

---

## 5. Terminology

### Driver Module

The extension unit. A module has one stable `driver_id` and a descriptor. It
may expose zero or more capabilities.

### Output Backend

A runtime object implementing `DeviceBackend`. It handles connection and frame
output for one routing domain, such as `usb`, `smbus`, `wled`, `hue`, or
`nanoleaf`.

### Protocol

A HAL-level encoder/decoder that converts color/display/control requests into
transport commands for a device family or model.

### Transport

The low-level I/O mechanism: USB HID, hidraw, control transfer, bulk transfer,
MIDI, serial, SMBus, UDP, HTTP, DTLS, and so on.

### Device Origin

The routing and ownership tuple attached to every discovered device:

- `driver_id` - semantic owner
- `backend_id` - output backend route
- `transport` - physical or network transport class
- `protocol_id` - optional protocol binding

### Backend ID

The key used by `BackendManager` to find the output backend. Backend IDs are
runtime routes, not driver identities.

### Driver ID

The key used by config, capability registry, metadata, cache, credentials, and
UI presentation. Driver IDs identify extension ownership.

---

## 6. Target Architecture

```text
hypercolor-daemon
  -> hypercolor-driver-api
       value types, descriptors, capability traits
  -> hypercolor-network
       DriverModuleRegistry, capability lookup, native module registration
  -> hypercolor-core
       DeviceBackend, BackendManager, lifecycle, discovery orchestrator
  -> hypercolor-hal
       Protocol, Transport, protocol descriptors
  -> built-in driver modules
       network modules: wled, hue, nanoleaf
       HAL modules: nollie, prismrgb, lianli, asus, corsair, razer, qmk, dygma
```

The daemon startup flow becomes:

```text
build DriverModuleRegistry
register built-in native modules
resolve driver config defaults and validation
build host-owned output backends
ask modules for backend factories and protocol catalogs
register DeviceBackend instances with BackendManager
run discovery by module/backend capability
merge devices by fingerprint and DeviceOrigin
connect through DeviceOrigin.backend_id
```

The frame-output flow remains:

```text
spatial layout -> BackendManager -> DeviceBackend -> transport/protocol
```

That hot path does not ask driver modules anything per frame.

---

## 7. Crate Boundaries

### 7.1 `hypercolor-types`

Owns pure serializable data types:

- `DeviceOrigin`
- `DriverTransportKind`
- `DriverPresentation`
- `DriverConfigEntry`
- driver config value/schema records
- API-safe capability summaries

It must not depend on `hypercolor-core`, `hypercolor-hal`, driver crates, or
daemon crates.

`DeviceFamily` remains a device identity enum and must stay out of runtime
routing.

### 7.2 `hypercolor-driver-api`

Owns host/driver boundary traits and value request/response types:

- `DriverModule`
- `DriverModuleDescriptor`
- `DriverCapabilitySet`
- `DriverConfigProvider`
- `DriverDiscoveryCapability`
- `DriverPairingCapability`
- `DriverBackendFactory`
- `DriverProtocolCatalog`
- `DriverRuntimeCache`
- `DriverCredentialSchema`
- `DriverPresentationProvider`
- `DriverHost`

It may depend on `hypercolor-types` and `hypercolor-core` while native boundary
traits are being extracted. The target is for native drivers to import
`DeviceBackend`, `TransportScanner`, credentials, and host services from this
crate without coupling to core.

It should not depend on `hypercolor-hal`, because HAL must remain core-free and
because Wasm-shaped value contracts should not require native transport types.

### 7.3 `hypercolor-hal`

Owns low-level protocol and transport implementation:

- `Protocol`
- `ProtocolCommand`
- `Transport`
- `DeviceDescriptor`
- transport-specific descriptor data
- vendor protocol modules

It must not depend on `hypercolor-core` or `hypercolor-driver-api`.

HAL can expose a pure value catalog that `hypercolor-driver-api` can adapt from
outside the HAL crate.

### 7.4 `hypercolor-network`

Owns registry and dispatch:

- `DriverModuleRegistry`
- native built-in registration helpers
- capability filtering
- descriptor validation

It should not contain vendor-specific protocol logic.

### 7.5 `hypercolor-core`

Owns runtime device output:

- `DeviceBackend`
- `BackendManager`
- discovery orchestrator
- lifecycle state machine
- USB, SMBus, Blocks, and other host-owned output backends

Core may adapt HAL protocol catalogs into output backends, but should not own
driver-specific settings or presentation.

### 7.6 `hypercolor-daemon`

Owns process orchestration:

- builds the registry
- hosts config/cache/credential/lifecycle services
- exposes API and WebSocket surfaces
- coordinates discovery scans
- publishes events

The daemon should not import concrete driver crates directly after the built-in
driver bundle lands.

---

## 8. Core Types

### 8.1 Driver Module Descriptor

```rust
pub struct DriverModuleDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub vendor_name: Option<&'static str>,
    pub module_kind: DriverModuleKind,
    pub transports: &'static [DriverTransportKind],
    pub capabilities: DriverCapabilitySet,
    pub api_schema_version: u32,
    pub config_version: u32,
    pub default_enabled: bool,
}
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverModuleKind {
    Network,
    Hal,
    Host,
    Virtual,
}
```

### 8.2 Transport Kind

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverTransportKind {
    Network,
    Usb,
    Smbus,
    Midi,
    Serial,
    Virtual,
    Custom(String),
}
```

This is not a low-level transport descriptor. It is API-facing and stable
enough for filters, UI, logs, and Wasm boundaries.

### 8.3 Capability Set

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverCapabilitySet {
    pub config: bool,
    pub discovery: bool,
    pub pairing: bool,
    pub output_backend: bool,
    pub protocol_catalog: bool,
    pub runtime_cache: bool,
    pub credentials: bool,
    pub presentation: bool,
}
```

### 8.4 Device Origin

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceOrigin {
    pub driver_id: String,
    pub backend_id: String,
    pub transport: DriverTransportKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_id: Option<String>,
}
```

`DeviceOrigin` should be attached to `DiscoveredDevice`, persisted in device
registry metadata, surfaced in device API responses, and used for all routing.

### 8.5 Driver Presentation

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverPresentation {
    pub label: String,
    pub short_label: Option<String>,
    pub accent_rgb: Option<[u8; 3]>,
    pub secondary_rgb: Option<[u8; 3]>,
    pub icon: Option<String>,
    pub default_device_class: Option<DeviceClassHint>,
}
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceClassHint {
    Keyboard,
    Mouse,
    Hub,
    Controller,
    Light,
    Display,
    Audio,
    Other,
}
```

The UI can still layer local user overrides on top of this, but it should not
hardcode driver IDs for common presentation.

---

## 9. Driver Module Contract

### 9.1 Root Trait

```rust
pub trait DriverModule: Send + Sync {
    fn descriptor(&self) -> &'static DriverModuleDescriptor;

    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        None
    }

    fn discovery(&self) -> Option<&dyn DriverDiscoveryCapability> {
        None
    }

    fn pairing(&self) -> Option<&dyn DriverPairingCapability> {
        None
    }

    fn output_backend(&self) -> Option<&dyn DriverBackendFactory> {
        None
    }

    fn protocol_catalog(&self) -> Option<&dyn DriverProtocolCatalog> {
        None
    }

    fn runtime_cache(&self) -> Option<&dyn DriverRuntimeCacheCapability> {
        None
    }

    fn credentials(&self) -> Option<&dyn DriverCredentialCapability> {
        None
    }

    fn presentation(&self) -> Option<&dyn DriverPresentationProvider> {
        None
    }
}
```

This replaced the old network-only extension point as the top-level concept.
Existing network drivers now implement `DriverModule` directly.

### 9.2 Config Provider

This follows Spec 50.

```rust
pub trait DriverConfigProvider: Send + Sync {
    fn default_config(&self) -> DriverConfigEntry;

    fn config_schema(&self) -> DriverConfigSchema {
        DriverConfigSchema::empty()
    }

    fn validate_config(&self, config: &DriverConfigEntry) -> anyhow::Result<()>;

    fn normalize_config(&self, config: DriverConfigEntry) -> anyhow::Result<DriverConfigEntry> {
        self.validate_config(&config)?;
        Ok(config)
    }
}
```

### 9.3 Host Boundary

```rust
pub trait DriverHost: Send + Sync {
    fn credentials(&self) -> &dyn DriverCredentialStore;
    fn runtime_cache(&self) -> &dyn DriverRuntimeCacheStore;
    fn lifecycle(&self) -> &dyn DriverLifecycleActions;
    fn discovery_state(&self) -> &dyn DriverDiscoveryState;
}
```

The host exposes services. It does not expose `AppState`, `DiscoveryRuntime`,
`BackendManager`, `ConfigManager`, or daemon internals.

---

## 10. Capabilities

### 10.1 Discovery

```rust
#[async_trait]
pub trait DriverDiscoveryCapability: Send + Sync {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DriverDiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<DriverDiscoveryResult>;
}
```

```rust
pub struct DriverDiscoveryRequest {
    pub timeout: Duration,
    pub mdns_enabled: bool,
    pub transport_filter: Option<DriverTransportKind>,
}

pub struct DriverDiscoveryResult {
    pub devices: Vec<DriverDiscoveredDevice>,
}

pub struct DriverDiscoveredDevice {
    pub info: DeviceInfo,
    pub origin: DeviceOrigin,
    pub fingerprint: DeviceFingerprint,
    pub metadata: BTreeMap<String, String>,
    pub connect_behavior: DiscoveryConnectBehavior,
}
```

Network drivers usually implement discovery themselves. HAL modules can either:

- expose protocol catalogs and let host-owned USB/SMBus scanners discover them,
  or
- expose discovery if the driver needs custom probing beyond a shared transport
  scan.

### 10.2 Pairing

Pairing stays generic and device-scoped.

```rust
#[async_trait]
pub trait DriverPairingCapability: Send + Sync {
    async fn auth_summary(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> Option<DeviceAuthSummary>;

    async fn pair(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
        request: &PairDeviceRequest,
    ) -> anyhow::Result<PairDeviceOutcome>;

    async fn clear_credentials(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<ClearPairingOutcome>;
}
```

Pairing is optional. Most HAL modules will not implement it.

### 10.3 Backend Factory

```rust
pub trait DriverBackendFactory: Send + Sync {
    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<Box<dyn DeviceBackend>>>;
}
```

Network modules commonly expose this. HAL modules usually do not, because they
route through host-owned shared backends.

### 10.4 Protocol Catalog

The protocol catalog is the bridge between HAL descriptors and the driver
module API.

```rust
pub trait DriverProtocolCatalog: Send + Sync {
    fn descriptors(&self) -> &[DriverProtocolDescriptor];
}
```

```rust
pub struct DriverProtocolDescriptor {
    pub driver_id: &'static str,
    pub protocol_id: &'static str,
    pub display_name: &'static str,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub family_id: &'static str,
    pub model_id: Option<&'static str>,
    pub transport: DriverTransportKind,
    pub route_backend_id: &'static str,
    pub presentation: Option<DriverPresentation>,
}
```

The descriptor is value-shaped. The native HAL adapter can keep the real
`ProtocolBinding` and `TransportType` internally.

### 10.5 Runtime Cache

```rust
pub trait DriverRuntimeCacheCapability: Send + Sync {
    fn default_cache(&self) -> serde_json::Value {
        serde_json::Value::Object(Default::default())
    }

    fn validate_cache(&self, value: &serde_json::Value) -> anyhow::Result<()>;
}
```

The daemon stores:

```json
{
  "driver_runtime": {
    "wled": {
      "probe_ips": ["192.168.1.42"],
      "probe_targets": []
    }
  }
}
```

Drivers interpret their own cache values.

### 10.6 Credentials

Credential storage becomes driver-scoped JSON.

```rust
#[async_trait]
pub trait DriverCredentialStore: Send + Sync {
    async fn get_json(
        &self,
        driver_id: &str,
        key: &str,
    ) -> anyhow::Result<Option<serde_json::Value>>;

    async fn set_json(
        &self,
        driver_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> anyhow::Result<()>;

    async fn remove(&self, driver_id: &str, key: &str) -> anyhow::Result<()>;
}
```

The key format is no longer parsed by splitting `"hue:..."`. The caller passes
`driver_id` separately.

Credential payloads are opaque driver-owned JSON at the host boundary. Since
Hypercolor has not shipped, stale enum-shaped credential files should be
migrated once and removed rather than kept behind a compatibility adapter.

### 10.7 Presentation

```rust
pub trait DriverPresentationProvider: Send + Sync {
    fn presentation(&self) -> DriverPresentation;

    fn presentation_for_device(
        &self,
        device: &DeviceInfo,
        origin: &DeviceOrigin,
    ) -> Option<DriverPresentation> {
        let _ = (device, origin);
        None
    }
}
```

This feeds `/api/v1/drivers`, device summaries, settings panels, and UI filter
chips.

---

## 11. Discovery and Routing

### 11.1 Backend Resolution

Routing should use this priority:

1. `DiscoveredDevice.origin.backend_id`
2. persisted registry origin for the same fingerprint

Routes must not depend on `DeviceFamily`.

### 11.2 Discovery Scan Loop

Target discovery loop:

```text
resolve requested scan domains
for each requested driver module with discovery capability:
    run module discovery
for each requested host transport domain:
    run shared scanner backed by protocol catalogs
merge all DriverDiscoveredDevice values
persist DeviceOrigin alongside metadata
publish generic discovery events
execute lifecycle actions using origin.backend_id
```

### 11.3 Host-Owned Transport Domains

Some scan domains are not driver IDs:

- `usb`
- `smbus`
- `blocks`
- `simulated`

These are host transport domains. They can discover devices for many driver
modules. Their scanner output still includes `DeviceOrigin.driver_id`.

### 11.4 Network Driver Domains

Network module discovery domains usually match driver IDs:

- `wled`
- `hue`
- `nanoleaf`

This is a convenience, not a rule. A future module may expose multiple network
protocols or route several drivers through one shared backend.

### 11.5 Lifecycle

Lifecycle actions should store and pass `DeviceOrigin` where possible:

```rust
pub enum DeviceLifecycleAction {
    Connect {
        device_id: DeviceId,
        origin: DeviceOrigin,
        layout_device_id: String,
    },
    Disconnect {
        device_id: DeviceId,
        origin: DeviceOrigin,
        will_retry: bool,
    },
}
```

This avoids repeatedly recomputing backend IDs from family and metadata.

---

## 12. Network Driver Integration

The existing WLED/Hue/Nanoleaf driver crates become normal `DriverModule`
implementations.

### 12.1 WLED

Descriptor:

```rust
DriverModuleDescriptor {
    id: "wled",
    display_name: "WLED",
    module_kind: DriverModuleKind::Network,
    transports: &[DriverTransportKind::Network],
    capabilities: DriverCapabilitySet {
        config: true,
        discovery: true,
        output_backend: true,
        runtime_cache: true,
        presentation: true,
        ..DriverCapabilitySet::empty()
    },
    default_enabled: true,
    ..
}
```

Discovery returns:

```rust
DeviceOrigin {
    driver_id: "wled".to_owned(),
    backend_id: "wled".to_owned(),
    transport: DriverTransportKind::Network,
    protocol_id: Some("wled/ddp".to_owned()),
}
```

`protocol_id` may switch to `"wled/e131"` based on config or device state.

### 12.2 Hue

Hue owns pairing and credentials.

```rust
DeviceOrigin {
    driver_id: "hue".to_owned(),
    backend_id: "hue".to_owned(),
    transport: DriverTransportKind::Network,
    protocol_id: Some("hue/entertainment".to_owned()),
}
```

The Entertainment Area Required hint should move from hardcoded UI logic to
driver presentation or auth/device guidance metadata.

### 12.3 Nanoleaf

Nanoleaf is similar to Hue:

```rust
DeviceOrigin {
    driver_id: "nanoleaf".to_owned(),
    backend_id: "nanoleaf".to_owned(),
    transport: DriverTransportKind::Network,
    protocol_id: Some("nanoleaf/external-control".to_owned()),
}
```

### 12.4 Migration Status

The first-pass implementation skipped the temporary adapter layer and moved WLED,
Hue, and Nanoleaf directly to `DriverModule`. That kept the final API cleaner:
there is no legacy adapter layer, and all built-in network drivers now share
the same module boundary as future extension points.

---

## 13. HAL Driver Integration

HAL modules are driver modules too, but they expose protocol catalogs rather
than backend factories.

### 13.1 Nollie

Nollie module owns:

- Nollie OEM SKUs
- Prism 8 rebrand routing through Nollie protocol
- Gen-1 and Gen-2 protocol descriptors
- attachment-aware topology/config defaults
- presentation metadata

Example:

```rust
DriverModuleDescriptor {
    id: "nollie",
    display_name: "Nollie",
    module_kind: DriverModuleKind::Hal,
    transports: &[DriverTransportKind::Usb],
    capabilities: DriverCapabilitySet {
        config: true,
        protocol_catalog: true,
        presentation: true,
        ..DriverCapabilitySet::empty()
    },
    default_enabled: true,
    ..
}
```

Descriptor:

```rust
DriverProtocolDescriptor {
    driver_id: "nollie",
    protocol_id: "nollie/nollie32",
    display_name: "Nollie 32",
    vendor_id: Some(0x3061),
    product_id: Some(0x4714),
    family_id: "nollie",
    model_id: Some("nollie_32"),
    transport: DriverTransportKind::Usb,
    route_backend_id: "usb",
    presentation: None,
}
```

Discovered device origin:

```rust
DeviceOrigin {
    driver_id: "nollie".to_owned(),
    backend_id: "usb".to_owned(),
    transport: DriverTransportKind::Usb,
    protocol_id: Some("nollie/nollie32".to_owned()),
}
```

### 13.2 PrismRGB

PrismRGB should shrink to PrismRGB-exclusive silicon, matching Spec 49.

Prism S dynamic config now resolves through HAL-owned attachment profile helpers:

- PrismRGB module exposes config schema for Prism S attachments/topology.
- Attachment profiles remain daemon-owned persisted data.
- The USB backend consumes a protocol runtime config derived from attachment profiles.
- The daemon no longer has a Prism S branch inside discovery.

### 13.3 Lian Li, ASUS, Corsair, Razer, QMK, Dygma

These start as protocol-catalog modules with presentation metadata. They do not
need pairing, credentials, or custom backend factories in the first pass.

ASUS can expose both USB and SMBus catalog entries. Discovery results for ASUS
SMBus devices should carry `driver_id = "asus"` and `backend_id = "smbus"`.

### 13.4 HAL Database Adaptation

`hypercolor-hal::database::ProtocolDatabase` can remain internally while a new
adapter exports module catalogs.

Transitional shape:

```text
hypercolor-hal
  static protocol descriptors
  ProtocolDatabase lookup by VID/PID

hypercolor-core or driver-bundle adapter
  groups HAL descriptors by driver_id
  registers DriverModule objects with protocol_catalog capability
  builds UsbBackend/SmBusBackend with catalog view
```

Final shape:

```text
each HAL driver module exposes descriptors()
DriverModuleRegistry collects protocol catalogs
UsbScanner/SmBusScanner query protocol catalogs instead of one global database
```

---

## 14. Daemon Integration

### 14.1 Startup

Startup should build a module registry first:

```rust
let mut registry = DriverModuleRegistry::new();
hypercolor_driver_bundle::register_builtin_modules(&mut registry, host_services)?;
```

Then register output backends:

```rust
register_host_backends(&mut backend_manager, &registry, host)?;
register_module_backends(&mut backend_manager, &registry, host, config)?;
```

Host-owned backends:

- simulated display
- mock
- blocks
- smbus
- usb

Module-owned backends:

- wled
- hue
- nanoleaf
- future native or Wasm-backed network modules

### 14.2 Discovery Runtime

`DiscoveryRuntime` should not contain driver-specific protocol config stores.
Instead, it should contain generic host services:

```rust
pub struct DiscoveryRuntime {
    pub device_registry: DeviceRegistry,
    pub backend_manager: Arc<Mutex<BackendManager>>,
    pub lifecycle_manager: Arc<Mutex<DeviceLifecycleManager>>,
    pub driver_modules: Arc<DriverModuleRegistry>,
    pub driver_host: Arc<DaemonDriverHost>,
    ...
}
```

Driver-specific stores should be accessed through `DriverHost` services.

### 14.3 Runtime State

Replace WLED-specific fields:

```rust
pub wled_probe_ips: Vec<IpAddr>,
pub wled_probe_targets: Vec<WledKnownTarget>,
```

with:

```rust
pub driver_runtime_cache: BTreeMap<String, serde_json::Value>,
```

Local pre-release snapshots can be migrated once into the new shape. The
runtime schema should not keep compatibility reads for removed WLED-specific
fields.

### 14.4 Credentials

`DaemonDriverHost` should stop matching on credential enum variants for driver
boundary calls. Driver credentials become opaque encrypted JSON payloads scoped
by `(driver_id, key)`.

Stale `Credentials::HueBridge`-style payloads are pre-release data. Migrate
local files once, then keep the credential store generic.

### 14.5 AppState

This spec does not require splitting `AppState`, but the unified driver module
work should avoid adding new broad `AppState` dependencies. New route modules
should depend on narrower service structs where practical:

- `DriverApiContext`
- `DeviceApiContext`
- `ConfigApiContext`
- `RenderApiContext`

---

## 15. API and UI Introspection

### 15.1 Driver List

Add or extend:

```http
GET /api/v1/drivers
```

Response:

```json
{
  "items": [
    {
      "id": "nollie",
      "display_name": "Nollie",
      "module_kind": "hal",
      "transports": ["usb"],
      "enabled": true,
      "capabilities": {
        "config": true,
        "discovery": false,
        "pairing": false,
        "output_backend": false,
        "protocol_catalog": true,
        "runtime_cache": false,
        "credentials": false,
        "presentation": true
      },
      "presentation": {
        "label": "Nollie",
        "short_label": "NOLLIE",
        "accent_rgb": [225, 53, 255],
        "secondary_rgb": [128, 255, 234],
        "icon": "cable",
        "default_device_class": "controller"
      }
    }
  ]
}
```

### 15.2 Device Summary

Device API responses should include origin:

```json
{
  "id": "018f...",
  "name": "Nollie 32",
  "family": "nollie",
  "origin": {
    "driver_id": "nollie",
    "backend_id": "usb",
    "transport": "usb",
    "protocol_id": "nollie/nollie32"
  },
  "presentation": {
    "label": "Nollie",
    "accent_rgb": [225, 53, 255]
  }
}
```

Output routing is exposed through `origin.backend_id`; device summaries do not
duplicate it as a top-level `backend` field.

### 15.3 Settings UI

Discovery settings should render from driver metadata:

- shared host discovery settings remain under `discovery`
- driver enablement toggles come from `/api/v1/drivers`
- driver-specific fields come from `/api/v1/drivers/{id}/config`

Hardcoded WLED/Hue/Nanoleaf toggles in the UI should become data-driven.

### 15.4 Device Cards

Device cards should classify presentation in this order:

1. user override
2. `device.presentation`
3. `driver.presentation`
4. fallback from `DeviceFamily.vendor_name()`
5. generic

Backend chips should be generated from actual backends present in the device
list, not a static list.

## 16. Render-Path Isolation

The unified driver-module architecture must preserve Hypercolor's most
important output invariant:

> No driver, protocol encoder, transport, network call, USB transaction, Wasm
> guest, pairing flow, discovery probe, credential lookup, or runtime-cache
> operation may block the render path.

### 16.1 Current Invariant

The current `BackendManager` already protects the render loop by routing frames
into per-device latest-value output queues. `write_frame_with_brightness()`
builds per-device color payloads and calls `OutputQueue::push()`. The queue uses
a `watch` channel so a new payload replaces stale work instead of waiting behind
it. The actual `DeviceBackend::write_colors_shared()` call happens in an output
worker task, not inside the render loop.

That means a slow physical write should affect only that output worker's
delivery latency and dropped-frame counters. It should not stall effect render,
spatial sampling, or frame publication.

### 16.2 Required Contract

The driver-module layer must keep this contract explicit:

- render code may only enqueue latest-value payloads
- render code must not call driver module capabilities
- render code must not call Wasm guests
- render code must not perform discovery, pairing, credential, cache, or config
  operations
- driver output workers must use bounded or latest-value queues
- stale output frames are dropped, not accumulated
- per-device failures are recorded as metrics/events, not propagated as render
  loop stalls

### 16.3 Driver and Wasm Boundaries

Future Wasm modules should not be invoked from the render thread. Wasm-backed
discovery, pairing, config validation, presentation metadata, and control
capabilities run outside the render loop.

HAL-style Wasm protocol encoders, if added later, must run in device output
workers or dedicated encoding workers behind latest-value queues. A slow Wasm
encoder may make that device drop frames. It must not block the render loop.

### 16.4 Remaining Risk In The Current Runtime

The current output isolation is good, but one subtle coupling remains: output
queues share an `Arc<Mutex<Box<dyn DeviceBackend>>>` per backend. A slow write
for one device can block other output queues using the same backend while they
wait for that backend mutex. That still should not block the render path, but it
can create cross-device interference inside one backend, such as one slow USB or
network target delaying sibling devices handled by the same backend instance.

The long-term fix is to split backend I/O into a control plane and per-device
data planes.

### 16.5 Per-Device Output Lanes

Each connected physical device should have an independent output lane. A lane is
the only object the render-side output queue talks to during steady-state frame
delivery.

Target shape:

```rust
pub trait DeviceOutputLane: Send + Sync {
    fn target_fps(&self) -> u32;

    fn enqueue_colors(&self, colors: Arc<Vec<[u8; 3]>>) -> OutputEnqueueResult;

    fn enqueue_display_payload(
        &self,
        payload: Arc<OwnedDisplayFramePayload>,
    ) -> OutputEnqueueResult {
        let _ = payload;
        OutputEnqueueResult::Unsupported
    }
}

pub enum OutputEnqueueResult {
    Accepted,
    ReplacedStale,
    DroppedDirectControl,
    Disconnected,
    Unsupported,
}
```

`enqueue_*` is intentionally non-async. It must only update a bounded or
latest-value queue. It must not write to hardware, wait on a backend mutex,
call a driver module, call a Wasm guest, open a socket, open a USB handle, or
perform protocol encoding if encoding can take unbounded time.

The backend control plane remains async:

```rust
#[async_trait]
pub trait DeviceBackend: Send + Sync {
    async fn discover(&mut self) -> Result<Vec<DeviceInfo>>;

    async fn connect(&mut self, id: &DeviceId) -> Result<ConnectedDevice>;

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()>;
}

pub struct ConnectedDevice {
    pub info: DeviceInfo,
    pub origin: DeviceOrigin,
    pub output: Arc<dyn DeviceOutputLane>,
}
```

This is the important split:

| Plane | Allowed To Block? | Examples |
| ----- | ----------------- | -------- |
| control | yes, bounded by lifecycle/API timeouts | discovery, connect, disconnect, pairing, config apply |
| data | no | render frame enqueue, latest-value replacement, per-device pacing |

The render path only sees the data plane.

### 16.6 Per-Device Cadence

Different devices will run at different rates. Cadence belongs to the output
lane, not to the global render loop.

Examples:

| Device | Render Loop | Output Lane |
| ------ | ----------- | ----------- |
| WLED DDP strip | 60 FPS | 42 FPS if device reports lower max |
| Nollie 32 | 60 FPS | 30 or 60 FPS depending protocol/config |
| Hue bridge | 60 FPS | bridge/session cadence |
| Nanoleaf panels | 60 FPS | device External Control cadence |
| LCD display device | 60 FPS | display lane cadence independent from LED lane |

When the render loop produces faster than a lane can send, the lane replaces
the stale frame. It does not queue an unbounded backlog. Slow devices visually
update slower; they do not pull the whole system down.

### 16.7 Backend-Specific Implications

- USB already pushes work into per-device actors after a short backend lock.
- USB should move the actor sender into a `DeviceOutputLane` so normal frame
  enqueue does not need the shared `UsbBackend` mutex at all.
- WLED should have one output lane per IP/device, each with its own UDP/HTTP
  pacing and stale-frame replacement.
- Hue should have one lane per bridge Entertainment session. If multiple
  logical devices share one bridge stream, the bridge lane is the isolation
  boundary because the hardware protocol requires a shared session.
- Nanoleaf should have one output lane per device stream.
- SMBus should isolate by bus/address when possible. If the physical bus is
  inherently serialized, the bus worker is the isolation boundary and should
  still accept latest-value updates per address.
- Display-capable devices should use a separate display lane when display
  payloads can be slower than LED payloads.
- Any future Wasm-backed output path should have one bounded worker lane per
  physical device, bridge session, bus target, or transport session.

### 16.8 Verification

Add regression tests around the invariant:

- a fake backend whose write awaits for seconds must not make
  `BackendManager::write_frame_with_brightness()` take seconds
- repeated frames for a slow device must replace stale queued payloads
- one slow output worker must increment dropped/queue-latency metrics
- two devices on the same backend must not block each other's frame enqueue
- direct control operations may wait for backend I/O, but normal render frames
  must continue enqueueing for unaffected devices

---

## 17. Future Wasm Shape

Dynamic Wasm loading is deferred. The native contract should still map cleanly
to WIT.

The boundary should avoid:

- Rust trait objects in guest-facing shapes
- raw `DeviceBackend` in guest-facing shapes
- direct file, socket, USB, or daemon-state access from guests
- driver reads of `hypercolor.toml`

The host reads config, validates ownership, stores credentials, stores runtime
cache, performs lifecycle actions, and mediates I/O.

Possible WIT shape:

```wit
package hypercolor:driver@1.0.0;

world driver-module {
  export descriptor: func() -> driver-module-descriptor;
  export default-config: func() -> config-entry;
  export config-schema: func() -> config-schema;
  export validate-config: func(config: config-entry) -> result<_, string>;
  export presentation: func() -> option<driver-presentation>;

  export discover: func(
    request: discovery-request,
    config: config-entry
  ) -> result<discovery-result, string>;

  export auth-summary: func(
    device: tracked-device
  ) -> option<device-auth-summary>;

  export pair: func(
    device: tracked-device,
    request: pair-device-request
  ) -> result<pair-device-outcome, string>;

  import host-credentials;
  import host-runtime-cache;
  import host-lifecycle;
  import host-log;
}
```

HAL protocol modules are harder to load as Wasm because frame encoding can be
transport-specific and latency-sensitive. The first Wasm target should be
network-style drivers and discovery/pairing/control extensions. HAL-style Wasm
protocol encoders can be considered later only if they run behind the
render-path isolation contract in Section 16.

Relevant current references:

- Bytecode Alliance Component Model WIT documentation:
  `https://component-model.bytecodealliance.org/design/wit.html`
- Wasmtime Component Model runtime documentation:
  `https://component-model.bytecodealliance.org/running-components/wasmtime.html`
- Wasmtime Rust component API:
  `https://docs.rs/wasmtime/latest/wasmtime/component/`

---

## 18. Migration Plan

### Wave 1: Add Origin and Value Types

Files:

- `crates/hypercolor-types/src/device.rs`
- `crates/hypercolor-types/src/config.rs`
- `crates/hypercolor-types/tests/device_tests.rs`
- `crates/hypercolor-types/tests/config_tests.rs`

Work:

- add `DeviceOrigin`
- add `DriverTransportKind`
- add `DriverPresentation`
- add `DriverCapabilitySet`
- add canonical origin/presentation fields
- remove duplicate device-summary `backend` response fields

Verify:

- `cargo test -p hypercolor-types device`
- `cargo test -p hypercolor-types config`

### Wave 2: Introduce DriverModule Registry

Files:

- `crates/hypercolor-driver-api/src/lib.rs`
- `crates/hypercolor-network/src/lib.rs`
- `crates/hypercolor-network/tests/registry_tests.rs`

Work:

- introduce `DriverModule`
- introduce `DriverModuleRegistry`
- port existing network drivers directly to `DriverModule`
- keep existing network driver tests green

Verify:

- `cargo test -p hypercolor-driver-api`
- `cargo test -p hypercolor-network`

### Wave 3: Move Built-In Registration Out Of Daemon

Files:

- new crate: `crates/hypercolor-driver-builtin`
- `Cargo.toml`
- `crates/hypercolor-daemon/Cargo.toml`
- `crates/hypercolor-daemon/src/network.rs`
- `crates/hypercolor-daemon/tests/network_tests.rs`

Work:

- create one composition edge for built-in modules
- move concrete WLED/Hue/Nanoleaf imports out of daemon
- register network modules through `DriverModuleRegistry`
- leave HAL module registration as adapter-backed stubs if needed

Verify:

- `cargo test -p hypercolor-daemon network`
- `cargo check -p hypercolor-daemon --no-default-features`

### Wave 4: Route Devices By Origin

Files:

- `crates/hypercolor-core/src/device/discovery.rs`
- `crates/hypercolor-core/src/device/usb_scanner.rs`
- `crates/hypercolor-core/src/device/smbus_scanner.rs`
- `crates/hypercolor-core/src/device/lifecycle.rs`
- `crates/hypercolor-daemon/src/discovery/*`
- daemon discovery tests

Work:

- attach origin to discovered devices
- persist origin in registry metadata
- update lifecycle actions to carry origin or resolved backend ID from origin
- keep runtime routing sourced from `DeviceOrigin`

Verify:

- `cargo test -p hypercolor-core lifecycle`
- `cargo test -p hypercolor-daemon discovery`

### Wave 5: Generic Driver Cache And Credentials

Files:

- `crates/hypercolor-daemon/src/runtime_state.rs`
- `crates/hypercolor-daemon/src/network/host.rs`
- `crates/hypercolor-core/src/device/net/credentials.rs`
- WLED/Hue/Nanoleaf driver tests

Work:

- add `driver_runtime: BTreeMap<String, Value>`
- migrate WLED probe cache into `driver_runtime.wled`
- change `DriverCredentialStore` to `(driver_id, key, value)`
- remove compatibility reads for existing credential enum variants after the
  one-time local migration
- remove WLED-specific cache matching from `DaemonDriverHost`

Verify:

- `cargo test -p hypercolor-daemon runtime_state`
- `cargo test -p hypercolor-driver-wled`
- `cargo test -p hypercolor-driver-hue`
- `cargo test -p hypercolor-driver-nanoleaf`

### Wave 6: HAL Protocol Catalog Modules

Files:

- `crates/hypercolor-hal/src/database.rs`
- `crates/hypercolor-hal/src/registry.rs`
- `crates/hypercolor-hal/src/drivers/*/devices.rs`
- `crates/hypercolor-core/src/device/usb_scanner.rs`
- `crates/hypercolor-core/src/device/usb_backend.rs`

Work:

- group HAL descriptors by driver ID
- expose protocol catalog values through adapter modules
- route USB/SMBus scanner discoveries with `DeviceOrigin`
- keep Prism S and Nollie32 dynamic config behind the generic host attachment
  profile service

Verify:

- `cargo test -p hypercolor-hal database`
- `cargo test -p hypercolor-core usb`
- `cargo test -p hypercolor-daemon attachment`

### Wave 7: Driver Metadata API And UI

Files:

- `crates/hypercolor-daemon/src/api/drivers.rs`
- `crates/hypercolor-daemon/src/api/devices/mod.rs`
- `crates/hypercolor-ui/src/api/*`
- `crates/hypercolor-ui/src/components/device_card.rs`
- `crates/hypercolor-ui/src/pages/devices.rs`
- `crates/hypercolor-ui/src/components/settings_sections.rs`

Work:

- add `/api/v1/drivers`
- add `/api/v1/drivers/{id}/config`
- include origin/presentation in device summaries
- render discovery settings from driver metadata
- render driver filter chips from actual device data

Verify:

- `cargo test -p hypercolor-daemon drivers`
- `just ui-test`

### Wave 8: Cleanup

Files:

- `crates/hypercolor-types/src/device.rs`
- `crates/hypercolor-daemon/src/discovery/device_helpers.rs`
- `crates/hypercolor-daemon/src/discovery/scan.rs`
- docs and tests

Work:

- remove any remaining family-derived runtime routing
- remove WLED-specific runtime state fields after migration window
- remove remaining UI backend hardcodes
- update docs to call `driver_id` and `backend_id` separate concepts

Verify:

- `just verify`

---

## 19. Verification

Minimum acceptance checks:

- `cargo test -p hypercolor-types device`
- `cargo test -p hypercolor-driver-api`
- `cargo test -p hypercolor-network`
- `cargo test -p hypercolor-hal database`
- `cargo test -p hypercolor-core lifecycle`
- `cargo test -p hypercolor-daemon discovery`
- `cargo test -p hypercolor-daemon network`
- `just ui-test`
- `just verify`

Behavioral checks:

- WLED discovery still probes configured and cached targets.
- Hue pairing still reports configured/required/error states.
- Nanoleaf pairing still stores and clears credentials.
- USB discovery returns origin with `backend_id = "usb"` and driver-specific
  `driver_id`.
- SMBus discovery returns origin with `backend_id = "smbus"` and
  `driver_id = "asus"` for ASUS devices.
- Prism S attachment changes update protocol config without daemon discovery
  matching on `DeviceFamily::PrismRgb`.
- Device API exposes route ownership through `origin.backend_id` instead of a
  duplicate legacy `backend` string.
- UI shows driver filter chips for actual returned drivers/devices.
- Unknown driver config entries roundtrip through config load/save.
- A disabled driver module does not register backend, discovery, pairing, or UI
  controls except as disabled metadata.
- A slow fake backend write does not block frame enqueue from
  `BackendManager::write_frame_with_brightness()`.

---

## 20. Recommendation

Implement a unified **DriverModule + capabilities** architecture, with
`DeviceOrigin` as the routing contract.

Do not try to make network and HAL internals identical. They are different
because the hardware is different. Make the layer above them identical:

- one module descriptor
- one config shape
- one presentation metadata shape
- one cache/credential host service
- one discovered-device origin shape
- one registry for capability lookup

Keep `DeviceBackend` as the hot path. Keep HAL protocol traits low-level and
core-free. Move the modularity boundary upward so the daemon, API, UI, and
future Wasm host all speak the same language.

The key outcome:

> Hypercolor should know which module owns a device and which backend routes
> frames to it. It should not have to infer either from the device family name.
