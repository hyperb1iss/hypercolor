# Spec 35 — Network Driver Architecture and Externalized Backend Boundary

> Implementation-ready specification for refactoring Hypercolor's network
> backends behind a modular driver architecture with clear host capabilities,
> minimal `#[cfg]` spread, and a future path to external plugins.

**Status:** Draft
**Author:** Nova
**Date:** 2026-03-15
**Crates:** `hypercolor-driver-api`, `hypercolor-network`, `hypercolor-daemon`, `hypercolor-cli`
**Related:** `docs/specs/33-network-device-backends.md`, `docs/specs/34-device-pairing-ui.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Design Principles](#4-design-principles)
5. [Target Architecture](#5-target-architecture)
6. [Driver API](#6-driver-api)
7. [Host Context Boundary](#7-host-context-boundary)
8. [Registration Model](#8-registration-model)
9. [Crate Layout](#9-crate-layout)
10. [Daemon and CLI Integration](#10-daemon-and-cli-integration)
11. [Migration Plan](#11-migration-plan)
12. [Future Dynamic Plugins](#12-future-dynamic-plugins)
13. [Recommendation](#13-recommendation)

---

## 1. Overview

Hypercolor's current network support works, but the composition boundary is too
porous. Startup, discovery, daemon API, and CLI logic all know about specific
network backends like Hue and Nanoleaf by name. As new network backends are
added, that spreads `#[cfg(feature = "...")]`, backend string checks, and
backend-specific routing into more modules than necessary.

This spec introduces:

- a dedicated driver API crate for host-facing traits and shared types
- a `hypercolor-network` orchestration crate that owns driver registration and
  capability dispatch
- one crate per network backend
- a single composition edge for compile-time feature gating
- a capability-based model for discovery, pairing, credentials, and backend
  construction

This is an externalization refactor, not a dynamic plugin system. The primary
goal is to fix the architecture so built-in drivers are modular and the daemon
only talks to stable capabilities.

---

## 2. Problem Statement

Current state:

- backend registration is hard-coded in daemon startup
- discovery knows a closed set of network backend IDs
- pairing/auth summary logic branches on backend strings
- CLI pairing commands are backend-specific
- feature gating is applied in multiple behavior modules instead of one
  composition module

That creates three concrete problems:

1. **High fan-out for every new backend**

   Adding a network backend currently requires touching multiple modules to:

   - register the backend
   - extend discovery backend enums and parsing
   - add auth summary logic
   - add pairing flows
   - add CLI commands

2. **No stable network driver boundary**

   `DeviceBackend` covers connection and frame writes, but discovery, pairing,
   credential lookup, and auth summaries live outside that abstraction. The
   daemon therefore owns protocol-specific logic that belongs with the driver.

3. **`#[cfg]` leaks through the stack**

   Compile-time gating currently affects runtime behavior code. That makes
   modules like discovery and devices API responsible for both host orchestration
   and backend availability.

---

## 3. Goals and Non-Goals

### Goals

- isolate network backend logic behind a small, explicit driver capability API
- move `#[cfg]` checks to one composition edge
- let the daemon discover driver capabilities instead of matching on backend IDs
- support built-in modular driver crates now
- preserve a future path to native or Wasm plugins later
- keep the runtime fast and the hot path allocation-free after initialization

### Non-Goals

- runtime dynamic loading in v1
- replacing the existing `DeviceBackend` render/write contract
- changing wire protocols for WLED, Hue, or Nanoleaf
- solving USB/HID modularization in the same refactor

Note: this spec focuses on network-native drivers first because that is where
pairing, credential storage, and driver-specific orchestration currently leak
into the daemon API.

---

## 4. Design Principles

### 4.1 Composition Edge, Not Logic Edge

Feature flags should decide which driver crates are linked into the binary. They
should not appear in pairing handlers, discovery orchestration, or CLI business
logic.

### 4.2 Capabilities Over Backend IDs

The daemon should ask:

- does this driver support discovery?
- does this driver support pairing?
- how does this driver summarize auth state?

It should not ask:

- is backend ID `"hue"`?
- is backend ID `"nanoleaf"`?

### 4.3 Narrow Host Boundary

Drivers should not receive `AppState`, `DiscoveryRuntime`, or broad mutable
access to daemon internals. They should receive a narrow host context with only
the services they actually need.

### 4.4 Static First, Dynamic Later

Use normal Rust crates and trait objects first. If the host/driver API is clean,
it can later be adapted to native dynamic plugins or Wasm components without
rewriting the daemon again.

---

## 5. Target Architecture

### 5.1 High-Level Shape

```text
hypercolor-daemon
  -> hypercolor-network
       -> DriverRegistry
       -> DriverHost
       -> capability dispatch
  -> hypercolor-driver-api
       -> shared traits and types
  -> hypercolor-driver-wled
  -> hypercolor-driver-hue
  -> hypercolor-driver-nanoleaf
```

The daemon owns runtime state and transport-agnostic orchestration.
`hypercolor-network` owns driver registration and generic dispatch.
Each driver crate owns protocol-specific discovery, pairing, auth summary,
credential semantics, and backend construction.

### 5.2 Ownership Split

| Layer | Responsibility |
|------|----------------|
| `hypercolor-driver-api` | Stable host/driver boundary |
| `hypercolor-network` | Registry, host adapters, generic routing |
| driver crates | Protocol-specific implementation |
| daemon | Runtime wiring, API surface, render loop lifecycle |

---

## 6. Driver API

### 6.1 Core Traits

The driver boundary should be capability-based.

```rust
pub trait NetworkDriverFactory: Send + Sync {
    fn descriptor(&self) -> &'static DriverDescriptor;

    fn build_backend(
        &self,
        host: &DriverHost,
    ) -> anyhow::Result<Option<Box<dyn DeviceBackend>>>;

    fn discovery(&self) -> Option<&dyn DiscoveryCapability> {
        None
    }

    fn pairing(&self) -> Option<&dyn PairingCapability> {
        None
    }
}

pub trait DiscoveryCapability: Send + Sync {
    async fn discover(
        &self,
        host: &DriverHost,
        request: DiscoveryRequest,
    ) -> anyhow::Result<DiscoveryResult>;
}

pub trait PairingCapability: Send + Sync {
    fn auth_summary(&self, device: &TrackedDeviceCtx<'_>) -> Option<DeviceAuthSummary>;

    async fn pair(
        &self,
        device: &TrackedDeviceCtx<'_>,
        request: PairDeviceRequest,
    ) -> anyhow::Result<PairDeviceOutcome>;

    async fn clear_credentials(
        &self,
        device: &TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<()>;
}
```

### 6.2 Descriptor

Each driver must expose metadata the host can use without touching backend
implementation details.

```rust
pub struct DriverDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub transport: DriverTransport,
    pub supports_discovery: bool,
    pub supports_pairing: bool,
}
```

### 6.3 Shared Request / Outcome Types

Keep pairing and discovery request/response types in the API crate so the daemon
and drivers share one vocabulary.

```rust
pub struct PairDeviceRequest {
    pub values: HashMap<String, String>,
    pub activate_after_pair: bool,
}

pub struct PairDeviceOutcome {
    pub status: PairDeviceStatus,
    pub message: String,
    pub auth_state: DeviceAuthState,
}

pub struct DiscoveryRequest {
    pub timeout: Duration,
    pub mdns_enabled: bool,
}
```

### 6.4 What Stays Out of the API

Do not put these in `hypercolor-driver-api`:

- `AppState`
- `DiscoveryRuntime`
- daemon HTTP request/response types
- concrete credential store implementation
- concrete `mdns-sd` or `reqwest` types

The API crate should be as stable and host-agnostic as possible.

---

## 7. Host Context Boundary

### 7.1 DriverHost

`DriverHost` is the daemon-owned adapter passed into driver capabilities.

It should expose only the services network drivers need:

```rust
pub trait DriverHost {
    fn credentials(&self) -> &dyn DriverCredentialStore;
    fn network_clients(&self) -> &dyn DriverNetworkClients;
    fn runtime(&self) -> &dyn DriverRuntimeActions;
}
```

### 7.2 TrackedDeviceCtx

Pairing and auth summary logic needs a read-only device context:

```rust
pub struct TrackedDeviceCtx<'a> {
    pub device_id: DeviceId,
    pub info: &'a DeviceInfo,
    pub metadata: Option<&'a HashMap<String, String>>,
    pub current_state: &'a DeviceState,
}
```

### 7.3 Runtime Actions

The only daemon lifecycle actions drivers should trigger directly are:

- best-effort post-pair activation
- best-effort disconnect after credential removal
- event publication through a narrow callback surface

That keeps driver code decoupled from the daemon's full lifecycle executor.

---

## 8. Registration Model

### 8.1 Phase 1: Explicit Built-In Registry

In v1, use a simple explicit registry module in `hypercolor-network`:

```rust
pub fn register_builtin_drivers(
    registry: &mut DriverRegistry,
    host: &DriverHost,
    config: &HypercolorConfig,
) -> anyhow::Result<()>;
```

This module is the only place that should contain network-driver feature gates:

```rust
#[cfg(feature = "driver-hue")]
registry.register(HueDriverFactory::new(config.hue.clone(), host.clone()));

#[cfg(feature = "driver-nanoleaf")]
registry.register(NanoleafDriverFactory::new(config.nanoleaf.clone(), host.clone()));
```

### 8.2 Why Explicit First

An explicit built-in registry is preferred over self-registration for the first
refactor because it is:

- obvious to read
- easy to test
- deterministic
- low-magic during a large boundary change

### 8.3 Optional Phase 2: Distributed Registration

If the remaining central registry file still feels too manual, a later phase may
replace it with compile-time self-registration via a distributed slice.

That is an implementation choice, not the architectural boundary. The important
part is the driver capability API, not whether built-ins are listed explicitly or
collected automatically.

---

## 9. Crate Layout

### 9.1 New Crates

```text
crates/
  hypercolor-driver-api/
  hypercolor-network/
  hypercolor-driver-wled/
  hypercolor-driver-hue/
  hypercolor-driver-nanoleaf/
```

### 9.2 Dependency Direction

```text
hypercolor-driver-api
  <- hypercolor-network
  <- hypercolor-driver-wled
  <- hypercolor-driver-hue
  <- hypercolor-driver-nanoleaf

hypercolor-core
  <- hypercolor-network
  <- driver crates

hypercolor-daemon
  -> hypercolor-network
```

### 9.3 Practical Rule

Driver crates may depend on `hypercolor-core` for shared runtime primitives like
`DeviceBackend`, shared network helpers, and device types.

The daemon must not depend on driver crate internals directly.

---

## 10. Daemon and CLI Integration

### 10.1 Daemon Startup

`hypercolor-daemon` should no longer manually register Hue/Nanoleaf/WLED
backends. It should create a `DriverRegistry`, call the built-in registration
helper, and then ask each driver whether it contributes a `DeviceBackend`.

### 10.2 Discovery

Replace `DiscoveryBackend` as a closed enum for network drivers.

Preferred model:

- keep transport-agnostic built-ins like `usb`, `smbus`, `blocks` as host-owned
  scan domains for now
- move network discovery enumeration to registry-driven string IDs

That means discovery requests can ask for:

- all registry drivers with discovery capability
- a specific driver ID

without editing a central enum every time a driver is added.

### 10.3 Pairing

`/api/v1/devices/{id}/pair` and `/api/v1/devices/{id}/pair` deletion should:

- resolve device -> backend ID
- look up driver in the registry
- call `pairing().pair(...)` or `pairing().clear_credentials(...)`

The daemon API should not know whether the backend is Hue, Nanoleaf, or future
secured WLED.

### 10.4 CLI

The CLI should move to the generic device-scoped route.

Temporary compatibility strategy:

- keep legacy `hyper devices pair hue|nanoleaf` commands as wrappers
- internally resolve the device and call the generic route
- remove backend-specific CLI subcommands in a later cleanup wave

---

## 11. Migration Plan

### Wave 1: API Extraction

- create `hypercolor-driver-api`
- move pairing/discovery shared types there
- define `DriverDescriptor`, `NetworkDriverFactory`, `DiscoveryCapability`,
  `PairingCapability`, `DriverHost`, and `TrackedDeviceCtx`

### Wave 2: Registry and Host Adapters

- create `hypercolor-network`
- implement `DriverRegistry`
- implement daemon-owned host adapters for credentials, shared network clients,
  and runtime actions
- add explicit built-in driver registration

### Wave 3: Pairing Refactor

- move generic pairing logic out of daemon backend string branches
- make auth summaries driver-owned
- keep daemon API route shape unchanged

### Wave 4: Discovery Refactor

- replace network backend enum handling with registry-driven enumeration
- migrate generic discovery scan orchestration to driver capability dispatch

### Wave 5: Driver Crate Split

- move WLED network code into `hypercolor-driver-wled`
- move Hue code into `hypercolor-driver-hue`
- move Nanoleaf code into `hypercolor-driver-nanoleaf`

### Wave 6: Cleanup

- remove legacy backend-specific pair routes
- collapse CLI wrappers onto the generic route
- remove stale backend string matches from daemon code

---

## 12. Future Dynamic Plugins

### 12.1 Native Dynamic Plugins

If Hypercolor later wants third-party drivers shipped as dynamic libraries, this
driver API can be adapted to a native plugin host.

That future should be considered **optional** and **separate** from this refactor
because it introduces:

- ABI stability concerns
- plugin packaging and versioning
- platform-specific loader complexity
- stricter constraints on boundary types

### 12.2 Wasm Plugins

Wasm components are also a plausible future for untrusted third-party drivers or
cross-language plugins.

They are not recommended for this refactor because:

- built-in drivers need host networking, sockets, mDNS, and credential access
- frame streaming and transport state would require a large host-call surface
- the immediate problem is boundary cleanliness, not sandboxing

### 12.3 Compatibility Goal

The host/driver API in this spec should be designed so a later native or Wasm
plugin host can implement the same capability surface with minimal daemon churn.

That means:

- narrow host context
- stable shared request/response types
- no direct daemon-state access in driver code

---

## 13. Recommendation

Implement a **static modular driver architecture** first:

- separate network drivers into workspace crates
- introduce a driver API crate and registry crate
- make pairing/discovery/auth summary capability-driven
- keep feature gating only in one built-in registration module

Do **not** jump to dynamic loading or Wasm yet.

This solves the actual architectural pain immediately:

- fewer cross-module edits per backend
- no daemon branching on specific network backends
- a cleaner extensibility story for secured WLED and future network devices
- a stable path toward external plugins if Hypercolor later decides it truly
  needs them

The key outcome is simple:

> Hypercolor should know how to talk to drivers, not how Hue or Nanoleaf work.
