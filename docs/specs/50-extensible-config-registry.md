# Spec 50 — Extensible Config Registry

> A driver-owned configuration model for Hypercolor that keeps core config
> typed, moves extension settings into registries, and prepares the host
> boundary for future Wasm-loaded drivers.

**Status:** Implemented
**Author:** Nova
**Date:** 2026-04-26
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-driver-api`, `hypercolor-network`, `hypercolor-daemon`
**Related:** `docs/specs/12-configuration.md`, `docs/specs/51-unified-driver-module-api.md`, `docs/specs/35-network-driver-architecture.md`, `docs/specs/33-network-device-backends.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Design Principles](#4-design-principles)
5. [Target TOML Shape](#5-target-toml-shape)
6. [Rust Types](#6-rust-types)
7. [Driver Config Contract](#7-driver-config-contract)
8. [Config Loading](#8-config-loading)
9. [Daemon Integration](#9-daemon-integration)
10. [API and UI Introspection](#10-api-and-ui-introspection)
11. [Future Wasm Drivers](#11-future-wasm-drivers)
12. [Implementation Plan](#12-implementation-plan)
13. [Verification](#13-verification)
14. [Recommendation](#14-recommendation)

---

## 1. Overview

Hypercolor's main config currently contains driver-specific sections such as
`wled`, `hue`, and `nanoleaf` directly on `HypercolorConfig`. That was fine
while these drivers were built-in experiments, but it does not scale to a
modular driver architecture. Every new driver forces a core type change, a
schema version bump, daemon enablement branches, and UI/API awareness.

This spec replaces top-level driver-specific config fields with an extensible
registry:

```toml
[drivers.wled]
enabled = true
known_ips = ["192.168.1.42"]
default_protocol = "ddp"
realtime_http_enabled = true
dedup_threshold = 2

[drivers.hue]
enabled = true
bridge_ips = ["192.168.1.50"]
use_cie_xy = true
```

Core owns the registry storage and lifecycle. Drivers own the meaning,
defaults, validation, and migration of their own settings.

The first implementation pass targets native Rust drivers only. The API shape
must still map cleanly to a future Wasm Component Model driver contract.

---

## 2. Problem Statement

Original state:

- `HypercolorConfig` had top-level `wled`, `hue`, and `nanoleaf` fields.
- `DiscoveryConfig` had per-driver booleans such as `wled_scan`, `hue_scan`,
  and `nanoleaf_scan`.
- Some driver factories received broad config snapshots rather than a narrow
  driver-owned slice.
- The config API could only mutate known paths in the statically typed root.
- Future dynamic drivers had nowhere to declare config defaults or validation
  without modifying core crates.

That creates three problems.

1. **Core type pollution**

   Core config becomes a catalog of every driver Hypercolor has ever learned
   about. This violates the driver boundary introduced by Spec 35.

2. **Closed-world assumptions**

   The daemon can only validate and expose config paths known at compile time.
   This blocks out-of-tree native drivers and Wasm-loaded drivers.

3. **Duplicated enablement**

   Discovery enablement lives in `discovery`, driver settings live in separate
   top-level sections, and backend registration has to know both.

---

## 3. Goals and Non-Goals

### Goals

- Keep core daemon config strongly typed where Hypercolor owns the semantics.
- Move extension-owned settings under stable registries such as `drivers`.
- Give each driver a narrow config slice instead of the full root config.
- Replace per-driver discovery booleans with generic driver enablement.
- Keep runtime loading canonical v4 only, with local config updated in place
  during the transition.
- Expose enough metadata for CLI/UI/API introspection.
- Shape the contract so a Wasm driver can provide the same metadata later.

### Non-Goals

- Dynamic Wasm driver loading in the first pass.
- JSON Schema completeness in the first pass.
- Moving USB/HID protocol tables into this config system.
- Replacing profile, scene, layout, or device settings storage.
- Runtime hot-reload for all driver config fields.

---

## 4. Design Principles

### 4.1 Typed Core, Untyped Extension Boundary

Core-owned sections remain typed Rust structs:

- `daemon`
- `web`
- `mcp`
- `effect_engine`
- `audio`
- `capture`
- `discovery`
- `network`
- `dbus`
- `tui`
- `session`
- `features`

Extension-owned sections are stored as structured values keyed by stable IDs.
Drivers parse those values into private typed structs at their boundary.

### 4.2 Core Validates Ownership, Drivers Validate Meaning

The config manager validates that registry entries are well-formed, stable,
and attached to legal IDs. Drivers validate protocol-specific meaning:

- IP lists
- enum values
- timing ranges
- feature compatibility
- deprecated keys inside their own section

### 4.3 Defaults Come From Owners

The default root config comes from Hypercolor. Driver defaults come from the
registered driver. A missing `drivers.<id>` entry means "use the driver's
default entry", not "driver has no config".

### 4.4 Forward Compatibility Preserves Unknowns

Unknown driver entries must roundtrip without loss. A daemon without a driver
installed should not delete that driver's config.

### 4.5 Native First, Wasm-Shaped

The first pass only wires native Rust drivers. The driver config contract uses
value-based inputs and serializable metadata so it can later become WIT exports
without redesigning the config model.

---

## 5. Target TOML Shape

### 5.1 Root Config

```toml
schema_version = 4
include = ["hypercolor.local.toml"]

[daemon]
listen_address = "127.0.0.1"
port = 9420

[discovery]
mdns_enabled = true
scan_interval_secs = 300
blocks_scan = true

[drivers.wled]
enabled = true
known_ips = ["192.168.1.42"]
default_protocol = "ddp"
realtime_http_enabled = true
dedup_threshold = 2

[drivers.hue]
enabled = true
entertainment_config = "Living Room"
bridge_ips = ["192.168.1.50"]
use_cie_xy = true

[drivers.nanoleaf]
enabled = true
device_ips = ["192.168.1.60"]
transition_time = 1
```

### 5.2 Driver Registry Rules

- Keys under `[drivers]` are driver IDs.
- Driver IDs must be ASCII lowercase kebab-case or snake_case.
- `enabled` is reserved and interpreted by the host.
- Every other key belongs to the driver.
- Unknown driver IDs are preserved.
- Unknown keys inside a known driver section are passed to the driver for
  validation.

### 5.3 Enablement Semantics

`drivers.<id>.enabled` controls registration and discovery for a driver.

If the entry is missing:

- built-in drivers use their default enablement from the descriptor
- out-of-tree drivers use their descriptor default
- unknown drivers are inert until their driver is available

If the entry exists but `enabled = false`, the daemon does not register that
driver's backend, does not run discovery, and does not expose pairing actions.

### 5.4 Shared Discovery Settings

`[discovery]` keeps only host-level discovery behavior:

```toml
[discovery]
mdns_enabled = true
scan_interval_secs = 300
blocks_scan = true
blocks_socket_path = "/run/user/1000/blocksd.sock"
```

Driver-specific scanning flags move to `[drivers.<id>]`.

---

## 6. Rust Types

### 6.1 Root Config

```rust
pub struct HypercolorConfig {
    pub schema_version: u32,
    pub include: Vec<String>,
    pub daemon: DaemonConfig,
    pub web: WebConfig,
    pub mcp: McpConfig,
    pub effect_engine: EffectEngineConfig,
    pub audio: AudioConfig,
    pub capture: CaptureConfig,
    pub discovery: DiscoveryConfig,
    pub network: NetworkConfig,
    pub drivers: DriverConfigs,
    pub dbus: DbusConfig,
    pub tui: TuiConfig,
    pub session: SessionConfig,
    pub features: FeatureFlags,
}
```

The old `wled`, `hue`, and `nanoleaf` fields are removed from the canonical
Rust type after migration support exists.

### 6.2 Registry Storage

```rust
pub type DriverConfigs = BTreeMap<String, DriverConfigEntry>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverConfigEntry {
    #[serde(default = "defaults::bool_true")]
    pub enabled: bool,

    #[serde(flatten)]
    pub settings: BTreeMap<String, serde_json::Value>,
}
```

`serde_json::Value` keeps the extension boundary format-neutral and avoids a
runtime TOML dependency in `hypercolor-types`. TOML loading still preserves the
scalar, list, and object shapes drivers need through serde.

### 6.3 Driver Config View

```rust
pub struct DriverConfigView<'a> {
    pub driver_id: &'a str,
    pub enabled: bool,
    pub settings: &'a BTreeMap<String, serde_json::Value>,
}
```

Factories receive `DriverConfigView`, not `HypercolorConfig`.

---

## 7. Driver Config Contract

### 7.1 Native Rust Contract

Add a config capability to `hypercolor-driver-api`:

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

`DriverConfigProvider` hangs off the unified `DriverModule` contract:

```rust
pub trait DriverModule: Send + Sync {
    fn descriptor(&self) -> &'static DriverDescriptor;

    fn config(&self) -> Option<&dyn DriverConfigProvider> {
        None
    }

    fn build_output_backend(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<Box<dyn DeviceBackend>>>;

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> { None }

    fn pairing(&self) -> Option<&dyn PairingCapability> { None }
}
```

Discovery receives the same config view:

```rust
pub trait DiscoveryCapability: Send + Sync {
    async fn discover(
        &self,
        host: &dyn DriverHost,
        request: &DiscoveryRequest,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<DiscoveryResult>;
}
```

### 7.2 Descriptor Additions

```rust
pub struct DriverDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub transport: DriverTransport,
    pub supports_discovery: bool,
    pub supports_pairing: bool,
    pub default_enabled: bool,
    pub schema_version: u32,
    pub config_version: u32,
}
```

`schema_version` describes the driver API contract. `config_version` describes
the driver's private config settings.

### 7.3 Config Schema Metadata

The first pass uses a small internal schema instead of full JSON Schema:

```rust
pub struct DriverConfigSchema {
    pub version: u32,
    pub fields: Vec<DriverConfigField>,
}

pub struct DriverConfigField {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub value_type: DriverConfigValueType,
    pub default_value: serde_json::Value,
    pub required: bool,
    pub secret: bool,
    pub restart_required: bool,
}

pub enum DriverConfigValueType {
    Bool,
    Integer { min: Option<i64>, max: Option<i64> },
    Float { min: Option<f64>, max: Option<f64> },
    String,
    IpAddress,
    StringList,
    IpAddressList,
    Enum { values: Vec<String> },
}
```

This is enough for API documentation, CLI display, and a future generic UI.

---

## 8. Config Loading

### 8.1 Schema Version

Set `CURRENT_SCHEMA_VERSION` to `4`.

Main config v4 shape:

- add `drivers`
- use `[drivers.wled]`, `[drivers.hue]`, and `[drivers.nanoleaf]`
- use `drivers.<id>.enabled` for network driver discovery enablement
- keep `[discovery]` for shared discovery behavior only

### 8.2 Legacy Config Handling

The first implementation does not carry a runtime migration layer. Existing
local config files are updated in place to the v4 shape, and core code rejects
legacy top-level driver fields by omission from `HypercolorConfig`.

Manual update rules:

- `[wled]` becomes `[drivers.wled]`
- `[hue]` becomes `[drivers.hue]`
- `[nanoleaf]` becomes `[drivers.nanoleaf]`
- `discovery.wled_scan` becomes `drivers.wled.enabled`
- `discovery.hue_scan` becomes `drivers.hue.enabled`
- `discovery.nanoleaf_scan` becomes `drivers.nanoleaf.enabled`
- `schema_version` becomes `4`

### 8.3 Include Merging

`include` files deep-merge driver entries by driver ID. Included settings patch
only the keys they specify:

```toml
# hypercolor.toml
[drivers.wled]
enabled = true
default_protocol = "ddp"

# hypercolor.local.toml
[drivers.wled]
known_ips = ["192.168.1.42"]
```

The effective entry is:

```toml
[drivers.wled]
enabled = true
default_protocol = "ddp"
known_ips = ["192.168.1.42"]
```

---

## 9. Daemon Integration

### 9.1 Registry Construction

Built-in driver registration changes from:

```rust
registry.register(WledDriverFactory::new(config.clone()))?;
```

to:

```rust
registry.register(WledDriverFactory::new())?;
```

Factories no longer capture root config at construction time.

### 9.2 Backend Registration

Backend registration resolves a driver config view at the call site:

```rust
for driver_id in registry.ids() {
    let Some(driver) = registry.get(&driver_id) else {
        continue;
    };
    let config = resolve_driver_config(root_config, driver.as_ref())?;
    if !config.enabled {
        continue;
    }
    let Some(backend) = driver.build_output_backend(host, config)? else {
        continue;
    };
    backend_manager.register_backend(backend);
}
```

### 9.3 Discovery Resolution

Discovery backend resolution becomes registry-driven:

- `DiscoveryBackend::Network(driver_id)` is valid if the registry contains the
  driver and the driver supports discovery.
- The backend is enabled if `drivers.<id>.enabled` resolves true.
- Explicit requests for disabled drivers return a config error naming
  `drivers.<id>.enabled`.

### 9.4 Runtime Config Changes

Driver config changes are persisted immediately. Live application is explicit:

- fields marked `restart_required = true` do not hot-apply
- discovery-only fields can affect the next scan
- backend construction fields require backend restart unless the driver
  advertises a future live-reconfigure capability

The first pass does not need a live driver reconfigure trait.

---

## 10. API and UI Introspection

### 10.1 Config API Paths

The config API accepts canonical v4 keys only. Driver settings use
`drivers.<id>.<key>` and driver enablement uses `drivers.<id>.enabled`.

### 10.2 Driver Listing

Add or extend an endpoint:

```
GET /api/v1/drivers
```

Response:

```json
{
  "items": [
    {
      "id": "wled",
      "display_name": "WLED",
      "transport": "network",
      "enabled": true,
      "supports_discovery": true,
      "supports_pairing": false,
      "config_version": 1
    }
  ]
}
```

### 10.3 Driver Config Metadata

Add:

```
GET /api/v1/drivers/{id}/config
```

Response:

```json
{
  "id": "wled",
  "enabled": true,
  "config": {
    "known_ips": ["192.168.1.42"],
    "default_protocol": "ddp"
  },
  "schema": {
    "version": 1,
    "fields": [
      {
        "key": "default_protocol",
        "label": "Default protocol",
        "value_type": { "enum": ["ddp", "e131"] },
        "default_value": "ddp",
        "restart_required": true
      }
    ]
  }
}
```

This metadata is sufficient for a generic settings panel later.

---

## 11. Future Wasm Drivers

Dynamic Wasm driver loading is deferred. The native contract should still map
to a future WIT world with equivalent exports:

```wit
package hypercolor:driver@1.0.0;

world network-driver {
  export descriptor: func() -> driver-descriptor;
  export default-config: func() -> config-entry;
  export config-schema: func() -> config-schema;
  export validate-config: func(config: config-entry) -> result<_, string>;
  export discover: func(request: discovery-request, config: config-entry) -> result<discovery-result, string>;
}
```

Host-owned services remain imports:

- credential lookup and storage
- discovery cache reads and writes
- device lifecycle actions
- logging
- bounded network access, if allowed

The Wasm component does not read `hypercolor.toml` directly. The daemon reads,
merges, migrates, and validates ownership. The component receives only its
driver config entry.

This preserves a single config source of truth while allowing untrusted drivers
to be sandboxed later.

---

## 12. Implementation Plan

### Phase 1: Add Registry Types

Files:

- `crates/hypercolor-types/src/config.rs`
- `crates/hypercolor-types/tests/config_tests.rs`
- `crates/hypercolor-core/tests/config_tests.rs`

Work:

- add `DriverConfigEntry` and `DriverConfigs`
- add `drivers` to `HypercolorConfig`
- set schema version to 4
- keep legacy serde compatibility for first pass
- add tests for v4 minimal and full TOML

Verify:

- `cargo test -p hypercolor-types config`
- `cargo test -p hypercolor-core config`

### Phase 2: Migrate Built-In Driver Configs

Files:

- `crates/hypercolor-driver-api/src/lib.rs`
- `crates/hypercolor-driver-wled/src/lib.rs`
- `crates/hypercolor-driver-hue/src/lib.rs`
- `crates/hypercolor-driver-nanoleaf/src/lib.rs`
- driver crate tests

Work:

- add driver config provider trait
- move WLED/Hue/Nanoleaf config structs into driver crates or driver-owned
  modules
- parse `DriverConfigEntry` into typed private config
- validate driver settings at factory/backend/discovery boundary

Verify:

- `cargo test -p hypercolor-driver-api`
- `cargo test -p hypercolor-driver-wled`
- `cargo test -p hypercolor-driver-hue`
- `cargo test -p hypercolor-driver-nanoleaf`

### Phase 3: Update Daemon Registry and Discovery

Files:

- `crates/hypercolor-daemon/src/network.rs`
- `crates/hypercolor-daemon/src/discovery/mod.rs`
- `crates/hypercolor-daemon/src/discovery/scan.rs`
- `crates/hypercolor-daemon/src/startup/discovery_worker.rs`
- daemon discovery tests

Work:

- stop passing `HypercolorConfig` into driver factories
- resolve per-driver config at call sites
- replace `driver_enabled` branches with generic registry lookup
- update explicit disabled-driver errors to use `drivers.<id>.enabled`
- update WLED retry logic to read `drivers.wled.known_ips`

Verify:

- `cargo test -p hypercolor-daemon discovery`
- `cargo test -p hypercolor-daemon list_devices_includes_hue_auth_summary_when_configured`

### Phase 4: Config API and Introspection

Files:

- `crates/hypercolor-daemon/src/api/config.rs`
- driver API route module, new or existing
- `crates/hypercolor-daemon/tests/api_tests.rs`

Work:

- map legacy config keys to canonical driver keys
- add driver list/config metadata endpoint
- ensure unknown driver config roundtrips
- return canonical keys in config set responses

Verify:

- `cargo test -p hypercolor-daemon config`
- targeted API tests for `/api/v1/drivers`

### Phase 5: Docs and Cleanup

Files:

- `docs/specs/12-configuration.md`
- `docs/specs/35-network-driver-architecture.md`
- example config snippets

Work:

- update canonical config examples to v4 shape
- remove legacy top-level driver sections from current docs
- document the local one-time config edit separately from runtime loading

Verify:

- `just verify`

---

## 13. Verification

Minimum acceptance checks:

- v4 config with `[drivers.wled]`, `[drivers.hue]`, and
  `[drivers.nanoleaf]` loads.
- unknown `[drivers.example]` entry survives load/save.
- WLED known IPs still seed discovery and backend cache behavior.
- Hue and Nanoleaf pairing/auth summaries still work.
- explicit discovery request for disabled WLED returns an error naming
  `drivers.wled.enabled`.
- config API canonical driver registry keys update driver config.
- `just verify` passes.

Independent verification is required before completion because this crosses
more than three files and changes backend/API behavior.

---

## 14. Recommendation

Implement the registry model in one migration-focused pass for native drivers,
but do not build dynamic Wasm loading yet. The load-bearing decision is to make
driver config a value contract owned by drivers and stored by core. That removes
driver pollution from `hypercolor-types`, simplifies enablement, gives the UI
metadata to render unknown drivers later, and leaves a clean path to Wasm
Component Model drivers without committing to runtime plugin complexity now.
