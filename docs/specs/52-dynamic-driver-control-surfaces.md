# Spec 52 - Dynamic Driver Control Surfaces

> Typed, dynamically-applied driver and device configuration surfaces that can
> be consumed generically by the daemon API, UI, CLI, TUI, and future Wasm
> driver modules.

**Status:** Draft
**Author:** Nova
**Date:** 2026-04-26
**Crates:** `hypercolor-types`, `hypercolor-driver-api`, `hypercolor-network`, `hypercolor-core`, `hypercolor-daemon`, `hypercolor-ui`, `hypercolor-tui`, `hypercolor-cli`
**Related:** `docs/specs/50-extensible-config-registry.md`, `docs/specs/51-unified-driver-module-api.md`, `docs/specs/40-device-pairing-ui.md`, `docs/specs/47-device-metrics.md`

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Use Cases](#3-use-cases)
4. [Goals and Non-Goals](#4-goals-and-non-goals)
5. [Design Principles](#5-design-principles)
6. [Terminology](#6-terminology)
7. [Control Surface Model](#7-control-surface-model)
8. [Typed Value System](#8-typed-value-system)
9. [Descriptor Types](#9-descriptor-types)
10. [Dynamic Apply Contract](#10-dynamic-apply-contract)
11. [Driver Module Contract](#11-driver-module-contract)
12. [Persistence Model](#12-persistence-model)
13. [Daemon API](#13-daemon-api)
14. [WebSocket Events](#14-websocket-events)
15. [UI Consumption Layer](#15-ui-consumption-layer)
16. [CLI and TUI Consumption](#16-cli-and-tui-consumption)
17. [Future Wasm Shape](#17-future-wasm-shape)
18. [Security and Safety](#18-security-and-safety)
19. [Implementation Plan](#19-implementation-plan)
20. [Verification](#20-verification)
21. [Recommendation](#21-recommendation)

---

## 1. Overview

Spec 50 moved driver-owned daemon configuration under the generic
`drivers.<id>` registry. Spec 51 introduces a unified driver-module layer for
network drivers, HAL protocols, built-in backends, and future Wasm extensions.

This spec defines the next layer: a typed **control surface** system.

A control surface is the API-facing description of settings, live controls,
actions, state, constraints, and apply semantics owned by a driver module or a
specific device. The daemon exposes these surfaces through stable typed API
documents. The UI renders them without hardcoding WLED, Hue, Nanoleaf, Nollie,
PrismRGB, or any future driver.

The target mental model:

```text
Driver module
  owns semantics, validation, defaults, dynamic apply

Daemon
  owns persistence, API envelopes, auth, eventing, optimistic state

UI / CLI / TUI
  consume typed control-surface documents and render appropriate controls
```

The key rule:

> Runtime controls should never require a daemon restart. If a change cannot be
> applied in place, the driver must expose the smallest dynamic impact needed,
> such as device reconnect, backend rebind, or discovery rescan.

---

## 2. Problem Statement

Hypercolor currently has several kinds of driver-specific settings, but no
single way to describe them above the driver implementation.

Examples:

- WLED has protocol and realtime transport settings.
- Hue has bridge, entertainment, color-space, and pairing state.
- Nanoleaf has device IPs, tokens, transition timing, and topology concerns.
- HAL devices have model-specific LED counts, zones, packet formats, color
  orders, firmware quirks, and topology options.
- Some devices need immediate commands such as identify, reboot, re-pair, save
  to hardware, sync clock, or refresh topology.
- The UI needs to show all of this without growing a hardcoded panel for every
  driver.

Without a generic typed control layer, Hypercolor gets three bad outcomes.

1. **UI hardcoding creeps back in**

   Every new driver requires UI-specific conditionals, labels, toggles,
   validation, and action buttons.

2. **Config and live controls blur together**

   Some values are persistent daemon settings. Some are per-device desired
   state. Some are live-only hardware commands. Treating all of them as config
   makes persistence, undo, profiles, and apply behavior murky.

3. **Dynamic drivers cannot feel native**

   A Wasm-loaded driver must be able to expose its controls with the same
   fidelity as built-in Rust drivers. Raw JSON blobs are not enough.

---

## 3. Use Cases

### 3.1 Driver-Level Settings

Driver-level settings affect the module as a whole.

- Enable or disable a driver module.
- Configure discovery seed addresses.
- Choose a default protocol for newly discovered devices.
- Tune polling intervals and discovery timeouts.
- Select default color encoding or transport behavior.
- Configure pairing strategy, credential namespace, or bridge preference.
- Enable optional protocol features.

Examples:

- `drivers.wled.default_protocol = "ddp"`
- `drivers.hue.use_cie_xy = true`
- `drivers.nanoleaf.transition_time = 1`
- `drivers.nollie.discovery_mode = "vid_pid_database"`

### 3.2 Device-Level Persistent Controls

Device-level persistent controls describe desired behavior for a specific
physical device.

- Color order: RGB, RBG, GRB, GBR, BRG, BGR.
- White-channel strategy: disabled, separate, auto, warm-only.
- Gamma curve or color correction profile.
- Brightness floor and ceiling.
- Power limit or current limit.
- LED count, segment lengths, or matrix dimensions.
- Zone labels, strip direction, panel orientation, and logical rotation.
- Preferred realtime protocol for that device.
- Device output FPS cap.
- Per-device transition smoothing.

These values should survive daemon restart and may participate in profiles or
device backup/export later.

### 3.3 Device-Level Live Controls

Live controls affect the current hardware session but may not need persistence.

- Identify or blink device.
- Set temporary test color.
- Enter hardware setup mode.
- Toggle a device display overlay.
- Preview a changed LED count or color order before committing it.
- Force refresh topology.
- Enable a temporary diagnostics stream.

Live controls must still be typed. They should not become arbitrary command
strings.

### 3.4 Device Actions

Actions are one-shot commands with optional typed inputs.

- Reboot controller.
- Save current state to hardware memory.
- Factory reset user settings.
- Re-pair or forget credentials.
- Sync clock.
- Refresh firmware metadata.
- Run a self-test.
- Calibrate white point.

Actions are not fields. They have input descriptors, confirmation metadata,
progress, result payloads, and error reporting.

### 3.5 Read-Only Device State

Drivers can expose typed read-only values:

- Firmware version.
- Hardware revision.
- Serial number.
- Protocol mode currently active on hardware.
- Battery level.
- Thermal state.
- Link quality.
- Packet loss.
- Last protocol error.
- Last successful frame time.

This overlaps with metrics, but the UI needs a typed source of truth for
driver-owned state that belongs near controls.

### 3.6 Conditional Controls

Some controls only exist when another value or capability is present.

- Hue entertainment settings only show when an entertainment area exists.
- RGBW strategy only shows when the device has a white channel.
- E1.31 universe fields only show when WLED protocol is E1.31.
- Matrix orientation only shows for panel or matrix topologies.
- Fan curves only show for devices with fan sensors.

The UI should not hardcode these rules. Drivers should expose typed
availability conditions.

---

## 4. Goals and Non-Goals

### Goals

- Define one typed control-surface model for drivers and devices.
- Separate driver config, device controls, actions, and read-only state.
- Keep the UI data-driven without prescribing visual design.
- Avoid daemon restart requirements for driver-owned changes.
- Let drivers dynamically apply changes through explicit transactions.
- Support optimistic UI updates with server-confirmed reconciliation.
- Preserve strong typing at the API boundary.
- Keep future Wasm drivers first-class by using value records and variants.
- Support built-in Rust drivers before dynamic Wasm loading.

### Non-Goals

- Build the UI in this spec.
- Define exact visual layout, colors, icons, copy, or component styling.
- Replace profiles, scenes, layouts, or effect controls.
- Replace high-frequency render controls in the hot path.
- Require full JSON Schema.
- Require every driver to expose controls immediately.
- Support arbitrary script execution from control descriptors.

---

## 5. Design Principles

### 5.1 Typed At The Boundary

Control descriptors and values must use a closed type system owned by
Hypercolor. The API may serialize through JSON, but consumers should never need
to infer types from loose `serde_json::Value` payloads.

Drivers can keep rich private Rust structs internally. The public surface uses
stable serializable value types.

### 5.2 Dynamic Apply By Default

There is no `restart_required` flag in this model.

Every mutable control advertises an `ApplyImpact`, not a restart requirement.
The impact describes what the daemon may do dynamically:

- update driver state in place
- rescan discovery
- reconnect one device
- rebind one backend
- rebuild topology
- clear a runtime cache

The UI can communicate impact without telling the user to restart Hypercolor.

### 5.3 Separate Fields From Actions

A field represents state. An action represents a command. A reboot button is
not a boolean. A topology refresh is not a timestamp field. This keeps undo,
profiles, audit logs, and permissions sane.

### 5.4 The UI Renders Documents, Not Drivers

The UI consumes a `ControlSurfaceDocument` and a typed value map. It may choose
visual presentation, grouping, and interaction patterns, but it must not branch
on driver IDs for generic controls.

### 5.5 Drivers Own Meaning, Host Owns Storage

Drivers define defaults, validation, availability, dynamic apply, and action
behavior. The daemon owns persistence, transport, API shape, auth, eventing,
and conflict handling.

### 5.6 Hot Path Isolation

Applying controls must not block the render path. Drivers perform slow I/O
through async control operations, backend rebinds, or device lifecycle tasks.
Frame output continues through `BackendManager` and `DeviceBackend`.

### 5.7 Stable IDs Over Labels

Control IDs are stable machine identifiers. Labels are presentation metadata.
Profiles, config files, API calls, logs, and migrations use IDs, never labels.

---

## 6. Terminology

### Control Surface

A typed document describing controls, actions, groups, values, availability,
and apply semantics for a driver module or device.

### Driver Setting

A persistent module-level setting stored under `drivers.<id>` and applied
dynamically to the driver module.

### Device Control

A typed per-device field owned by the device's driver module.

### Device Action

A one-shot command owned by the device's driver module.

### Control Value

A typed scalar, collection, object, or secret reference that matches a
descriptor.

### Apply Transaction

The daemon-mediated process of validating, applying, persisting, and publishing
one or more control changes.

### Apply Impact

The dynamic operational effect required to apply a change.

### Availability

Typed conditions that determine whether a control is visible, enabled,
read-only, or unsupported for a specific driver or device context.

---

## 7. Control Surface Model

### 7.1 Surface Scopes

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSurfaceScope {
    Driver { driver_id: String },
    Device { device_id: DeviceId, driver_id: String },
}
```

Driver surfaces are module-level. Device surfaces are contextual and may vary
by model, firmware, topology, credentials, and discovered capabilities.

### 7.2 Surface Document

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlSurfaceDocument {
    pub surface_id: ControlSurfaceId,
    pub scope: ControlSurfaceScope,
    pub schema_version: u32,
    pub revision: ControlSurfaceRevision,
    pub groups: Vec<ControlGroupDescriptor>,
    pub fields: Vec<ControlFieldDescriptor>,
    pub actions: Vec<ControlActionDescriptor>,
    pub values: ControlValueMap,
    pub availability: ControlAvailabilityMap,
    pub action_availability: ControlActionAvailabilityMap,
}
```

`revision` changes whenever descriptors, availability, or current values
change. The UI can use it for caching and optimistic reconciliation.
`availability` is keyed by field ID. `action_availability` is keyed by action
ID and defaults to an empty map for older payloads.

### 7.3 Surface IDs

```rust
pub type ControlSurfaceId = String;
pub type ControlFieldId = String;
pub type ControlActionId = String;
pub type ControlGroupId = String;
pub type ControlSurfaceRevision = u64;
```

Recommended IDs:

- driver surface: `driver:wled`
- device surface: `device:018f...`
- field ID: `color_order`
- action ID: `reboot`

Field and action IDs are scoped to a surface. API paths include the surface
identity, so field IDs do not need global uniqueness.

### 7.4 Surface Sources

Control surfaces can be composed from several sources:

1. driver module descriptors
2. host-owned common controls
3. persisted device overrides
4. live hardware state
5. credentials and pairing state
6. runtime capability probes

The daemon returns one merged document. Consumers should not assemble the
surface by calling every subsystem individually.

---

## 8. Typed Value System

### 8.1 Value Type

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlValueType {
    Bool,
    Integer { min: Option<i64>, max: Option<i64>, step: Option<i64> },
    Float { min: Option<f64>, max: Option<f64>, step: Option<f64> },
    String { min_len: Option<u16>, max_len: Option<u16>, pattern: Option<String> },
    Secret,
    ColorRgb,
    ColorRgba,
    IpAddress,
    MacAddress,
    DurationMs { min: Option<u64>, max: Option<u64>, step: Option<u64> },
    Enum { options: Vec<ControlEnumOption> },
    Flags { options: Vec<ControlEnumOption> },
    List { item_type: Box<ControlValueType>, min_items: Option<u16>, max_items: Option<u16> },
    Object { fields: Vec<ControlObjectField> },
}
```

This should live in `hypercolor-types` so API, CLI, TUI, UI, and driver
contracts share the same vocabulary.

### 8.2 Value

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ControlValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    SecretRef(String),
    ColorRgb([u8; 3]),
    ColorRgba([u8; 4]),
    IpAddress(String),
    MacAddress(String),
    DurationMs(u64),
    Enum(String),
    Flags(Vec<String>),
    List(Vec<ControlValue>),
    Object(BTreeMap<String, ControlValue>),
}
```

The API may serialize this as JSON, but the discriminant is required. Consumers
must not infer `1` as either integer, float, duration, enum index, or boolean.

### 8.3 Enum Options

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlEnumOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
    pub deprecated: bool,
}
```

Enum values are stable IDs. Labels are display text.

### 8.4 Object Fields

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlObjectField {
    pub id: String,
    pub label: String,
    pub value_type: ControlValueType,
    pub required: bool,
    pub default_value: Option<ControlValue>,
}
```

Object values are for small structured controls such as coordinates, matrix
dimensions, universe/channel tuples, or calibration points. Large nested
documents should become dedicated resources instead of deeply nested controls.

### 8.5 Value Map

```rust
pub type ControlValueMap = BTreeMap<ControlFieldId, ControlValue>;
```

Values are keyed by field ID.

---

## 9. Descriptor Types

### 9.1 Field Descriptor

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlFieldDescriptor {
    pub id: ControlFieldId,
    pub owner: ControlOwner,
    pub group_id: Option<ControlGroupId>,
    pub label: String,
    pub description: Option<String>,
    pub value_type: ControlValueType,
    pub default_value: Option<ControlValue>,
    pub access: ControlAccess,
    pub persistence: ControlPersistence,
    pub apply_impact: ApplyImpact,
    pub visibility: ControlVisibility,
    pub availability: ControlAvailabilityExpr,
    pub ordering: i32,
}
```

Descriptors are durable enough for UI rendering and CLI/TUI display, but they
do not prescribe visual components.

### 9.2 Group Descriptor

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlGroupDescriptor {
    pub id: ControlGroupId,
    pub label: String,
    pub description: Option<String>,
    pub kind: ControlGroupKind,
    pub ordering: i32,
}
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlGroupKind {
    General,
    Connection,
    Output,
    Color,
    Topology,
    Performance,
    Diagnostics,
    Advanced,
    Danger,
    Custom,
}
```

Groups are semantic. The UI decides layout.

### 9.3 Access

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAccess {
    ReadOnly,
    ReadWrite,
    WriteOnly,
}
```

Secrets usually appear as `WriteOnly` or `ReadWrite` with a `SecretRef` value.
Raw secret material must not be returned to clients.

### 9.4 Persistence

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPersistence {
    DriverConfig,
    DeviceConfig,
    ProfileOverride,
    RuntimeOnly,
    HardwareStored,
}
```

`HardwareStored` means the driver can write state into the physical device.
The daemon still records desired state when needed for reconciliation.

### 9.5 Visibility

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlVisibility {
    Standard,
    Advanced,
    Diagnostics,
    Hidden,
}
```

Hidden fields can exist for compatibility or driver state, but generic UI
surfaces should not render them by default.

### 9.6 Availability

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlAvailabilityExpr {
    Always,
    Never { reason: String },
    WhenFieldEquals { field_id: ControlFieldId, value: ControlValue },
    WhenCapability { capability: String },
    All { expressions: Vec<ControlAvailabilityExpr> },
    Any { expressions: Vec<ControlAvailabilityExpr> },
}
```

The daemon evaluates availability for the current context and returns a
resolved map:

```rust
pub type ControlAvailabilityMap = BTreeMap<ControlFieldId, ControlAvailability>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlAvailability {
    pub state: ControlAvailabilityState,
    pub reason: Option<String>,
}
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlAvailabilityState {
    Available,
    Disabled,
    ReadOnly,
    Unsupported,
    Hidden,
}
```

The UI can choose whether to hide, disable, or explain unavailable controls.

### 9.7 Action Descriptor

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlActionDescriptor {
    pub id: ControlActionId,
    pub group_id: Option<ControlGroupId>,
    pub label: String,
    pub description: Option<String>,
    pub input_fields: Vec<ControlObjectField>,
    pub result_type: Option<ControlValueType>,
    pub confirmation: Option<ActionConfirmation>,
    pub apply_impact: ApplyImpact,
    pub availability: ControlAvailabilityExpr,
    pub ordering: i32,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfirmation {
    pub level: ActionConfirmationLevel,
    pub message: String,
}
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionConfirmationLevel {
    Normal,
    Destructive,
    HardwarePersistent,
}
```

Actions support typed inputs and typed results. They are not config keys.

---

## 10. Dynamic Apply Contract

### 10.1 Apply Impact

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyImpact {
    None,
    Live,
    DiscoveryRescan,
    DeviceReconnect,
    BackendRebind,
    TopologyRebuild,
    HardwarePersist,
    Custom(String),
}
```

There is intentionally no `DaemonRestart` variant.

If a driver cannot apply a change without restarting Hypercolor, that is a
driver limitation to fix, not a first-class UX contract. The driver may return
`ControlApplyError::UnsupportedDynamicApply` while the feature is incomplete,
but the target architecture remains dynamically configurable.

### 10.2 Apply Request

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyControlChangesRequest {
    pub surface_id: ControlSurfaceId,
    pub expected_revision: Option<ControlSurfaceRevision>,
    pub changes: Vec<ControlChange>,
    pub dry_run: bool,
}
```

`dry_run` defaults to `false` when omitted by clients.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlChange {
    pub field_id: ControlFieldId,
    pub value: ControlValue,
}
```

The daemon should support multi-field transactions so related values can be
validated together. Examples:

- matrix width and height
- E1.31 universe and channel
- RGBW strategy and white-channel calibration
- LED count and segment list

### 10.3 Apply Response

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyControlChangesResponse {
    pub surface_id: ControlSurfaceId,
    pub previous_revision: ControlSurfaceRevision,
    pub revision: ControlSurfaceRevision,
    pub accepted: Vec<AppliedControlChange>,
    pub rejected: Vec<RejectedControlChange>,
    pub impacts: Vec<ApplyImpact>,
    pub values: ControlValueMap,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedControlChange {
    pub field_id: ControlFieldId,
    pub value: ControlValue,
}
```

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedControlChange {
    pub field_id: ControlFieldId,
    pub attempted_value: ControlValue,
    pub error: ControlApplyError,
}
```

### 10.4 Apply Errors

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlApplyError {
    UnknownField,
    TypeMismatch { expected: ControlValueType },
    OutOfRange,
    InvalidValue { message: String },
    Unavailable { reason: String },
    Conflict { current_revision: ControlSurfaceRevision },
    Unauthorized,
    DeviceOffline,
    UnsupportedDynamicApply { message: String },
    DriverError { message: String },
}
```

Errors are typed enough for clients to react without parsing prose.

### 10.5 Transaction Rules

- Validate all changes before mutating state.
- Reject stale revisions unless the request opts into last-write-wins later.
- Persist host-owned state before publishing success.
- Apply slow hardware changes through async lifecycle tasks.
- Publish resulting values through WebSocket after commit.
- Return driver-normalized values, not just attempted values.
- If dynamic hardware apply fails after persistence, record drift and surface
  the failure in device state.

---

## 11. Driver Module Contract

### 11.1 Control Provider Capability

Spec 51 should add a `controls` capability:

```rust
pub trait DriverModule: Send + Sync {
    fn controls(&self) -> Option<&dyn DriverControlProvider> {
        None
    }
}
```

```rust
#[async_trait]
pub trait DriverControlProvider: Send + Sync {
    async fn driver_surface(
        &self,
        host: &dyn DriverHost,
        config: DriverConfigView<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>>;

    async fn device_surface(
        &self,
        host: &dyn DriverHost,
        device: &TrackedDeviceCtx<'_>,
    ) -> anyhow::Result<Option<ControlSurfaceDocument>>;

    async fn validate_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: &[ControlChange],
    ) -> anyhow::Result<ValidatedControlChanges>;

    async fn apply_changes(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        changes: ValidatedControlChanges,
    ) -> anyhow::Result<ApplyControlChangesResponse>;

    async fn invoke_action(
        &self,
        host: &dyn DriverHost,
        target: &ControlApplyTarget<'_>,
        action_id: &str,
        input: ControlValueMap,
    ) -> anyhow::Result<ControlActionResult>;
}
```

### 11.2 Apply Target

```rust
pub enum ControlApplyTarget<'a> {
    Driver {
        driver_id: &'a str,
        config: DriverConfigView<'a>,
    },
    Device {
        device: &'a TrackedDeviceCtx<'a>,
    },
}
```

### 11.3 Host Services

`DriverHost` needs control-specific services:

```rust
pub trait DriverControlHost: Send + Sync {
    fn device_config_store(&self) -> &dyn DeviceControlStore;
    fn driver_config_store(&self) -> &dyn DriverConfigStore;
    fn lifecycle(&self) -> &dyn DriverLifecycleActions;
    fn backend_rebind(&self) -> &dyn BackendRebindActions;
    fn publish_control_event(&self, event: ControlSurfaceEvent);
}
```

The driver asks the host for dynamic operations. It does not reach into
`AppState` or `BackendManager` directly.

### 11.4 Built-In Common Controls

Some controls are host-owned and should appear for many devices:

- display name override
- enabled/disabled
- brightness limit
- layout attachment hints
- identify action, if backend supports it

The daemon may compose these into the same document, but ownership must remain
explicit so updates route to the right subsystem.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlOwner {
    Host,
    Driver { driver_id: String },
}
```

Field descriptors should include `owner: ControlOwner`.

### 11.5 Action Result

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlActionResult {
    pub surface_id: ControlSurfaceId,
    pub action_id: ControlActionId,
    pub status: ControlActionStatus,
    pub result: Option<ControlValue>,
    pub revision: ControlSurfaceRevision,
}
```

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionStatus {
    Accepted,
    Running,
    Completed,
    Failed,
}
```

---

## 12. Persistence Model

### 12.1 Driver Config

Driver-level persistent fields with `ControlPersistence::DriverConfig` map to
`drivers.<id>.<field_id>` unless the driver provides an explicit storage key.

This keeps Spec 50 as the source of truth for module-level config while adding
typed descriptors and dynamic apply.

### 12.2 Device Config

Device-level persistent fields should live in a daemon-owned device control
store keyed by stable device fingerprint or `DeviceId` plus driver ID.

Recommended shape:

```toml
[device_controls."fingerprint-or-device-id".wled]
color_order = "grb"
max_fps = 60
power_limit_ma = 2500
```

This should not be added to `HypercolorConfig` as top-level driver-specific
typed structs. It should use a generic value map with typed validation from
the driver.

### 12.3 Runtime-Only Values

Runtime-only values are held in memory and optionally exposed through
WebSocket snapshots. They are not written to disk.

Examples:

- identify active
- diagnostic stream enabled
- last action result
- temporary test color

### 12.4 Hardware-Stored Values

For hardware-stored controls, the daemon records desired state when it matters
for reconciliation. The driver action performs the hardware write.

Examples:

- save current WLED settings to flash
- write fan curve to device EEPROM
- persist calibration table

The UI should distinguish "desired state saved by Hypercolor" from "written to
hardware" using action result and state fields, not by guessing.

---

## 13. Daemon API

### 13.1 List Driver Surfaces

```http
GET /api/v1/drivers/{driver_id}/controls
```

Returns the driver-level `ControlSurfaceDocument`.

### 13.2 List Device Surfaces

```http
GET /api/v1/devices/{device_id}/controls
```

Returns the device-level `ControlSurfaceDocument`.

### 13.3 Apply Control Changes

```http
PATCH /api/v1/control-surfaces/{surface_id}/values
```

Request:

```json
{
  "expected_revision": 12,
  "dry_run": false,
  "changes": [
    {
      "field_id": "color_order",
      "value": { "kind": "enum", "value": "grb" }
    }
  ]
}
```

Response:

```json
{
  "surface_id": "device:018f...",
  "previous_revision": 12,
  "revision": 13,
  "accepted": [
    {
      "field_id": "color_order",
      "value": { "kind": "enum", "value": "grb" }
    }
  ],
  "rejected": [],
  "impacts": ["device_reconnect"],
  "values": {
    "color_order": { "kind": "enum", "value": "grb" }
  }
}
```

### 13.4 Invoke Action

```http
POST /api/v1/control-surfaces/{surface_id}/actions/{action_id}
```

Request:

```json
{
  "input": {
    "duration_ms": { "kind": "duration_ms", "value": 1500 }
  }
}
```

Response:

```json
{
  "surface_id": "device:018f...",
  "action_id": "identify",
  "status": "completed",
  "result": null,
  "revision": 14
}
```

### 13.5 Batched Surface Fetch

The UI should have an efficient route for device pages:

```http
GET /api/v1/control-surfaces?device_id=...&include_driver=true
```

Returns both the driver surface and device surface when needed.

### 13.6 Error Envelope

API errors should use the existing response envelope and include typed control
errors in `data` or an equivalent structured error field. Clients should not
parse human text to learn whether a value was out of range or unavailable.

Current structured `error.details.kind` values include:

- `control_surface_revision_conflict`
- `control_surface_mismatch`
- `empty_control_changes`
- `duplicate_control_field`
- `unknown_control_field`
- `control_value_type_mismatch`
- `control_value_out_of_range`
- `invalid_control_value`
- `driver_control_validation_failed`
- `driver_device_control_validation_failed`
- `control_action_failed`

---

## 14. WebSocket Events

Control surfaces need watch-style updates because availability and values can
change after discovery, pairing, firmware probing, reconnect, or action
completion.

Events:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ControlSurfaceEvent {
    SurfaceChanged {
        surface_id: ControlSurfaceId,
        revision: ControlSurfaceRevision,
    },
    ValuesChanged {
        surface_id: ControlSurfaceId,
        revision: ControlSurfaceRevision,
        values: ControlValueMap,
    },
    AvailabilityChanged {
        surface_id: ControlSurfaceId,
        revision: ControlSurfaceRevision,
        availability: ControlAvailabilityMap,
    },
    ActionAvailabilityChanged {
        surface_id: ControlSurfaceId,
        revision: ControlSurfaceRevision,
        availability: ControlActionAvailabilityMap,
    },
    ActionProgress {
        surface_id: ControlSurfaceId,
        action_id: ControlActionId,
        status: ControlActionStatus,
        progress: Option<f32>,
    },
}
```

The UI can optimistically update local state after `PATCH`, then reconcile when
the authoritative event arrives.

---

## 15. UI Consumption Layer

### 15.1 UI Contract

The UI should consume `ControlSurfaceDocument` as a view model. It should not
need driver-specific Rust or TypeScript code to render common controls.

The UI layer needs generated TypeScript types for:

- `ControlSurfaceDocument`
- `ControlFieldDescriptor`
- `ControlActionDescriptor`
- `ControlValueType`
- `ControlValue`
- `ApplyControlChangesRequest`
- `ApplyControlChangesResponse`
- `ControlApplyError`
- `ControlSurfaceEvent`

These types should be generated from the Rust API type source or an OpenAPI
schema derived from those Rust types.

### 15.2 UI Responsibilities

The UI is responsible for:

- rendering fields based on `ControlValueType`
- grouping fields using `ControlGroupDescriptor`
- preserving unknown groups and custom types with a generic fallback
- validating obvious client-side constraints before submit
- sending typed values, not loose JSON
- handling availability states
- showing action confirmation when requested
- batching related field edits when the surface supports it
- reconciling optimistic state with server revisions
- avoiding driver-ID conditionals for generic behavior

### 15.3 UI Non-Responsibilities

The UI is not responsible for:

- knowing driver-specific validation rules
- knowing how to apply hardware changes
- deciding persistence target
- deciding whether reconnect or rebind is needed
- inventing labels for driver-specific controls
- parsing raw metadata blobs

### 15.4 UI Component Mapping

This spec does not prescribe visual design. It does define semantic component
expectations:

| Value Type | Generic UI Capability |
| ---------- | --------------------- |
| `Bool` | binary toggle or checkbox |
| `Integer`, `Float`, `DurationMs` | numeric input, slider, or stepper based on range |
| `String` | text input |
| `Secret` | secret entry with set/clear state |
| `ColorRgb`, `ColorRgba` | color picker or swatch input |
| `IpAddress`, `MacAddress` | validated text input |
| `Enum` | select, segmented control, or radio group |
| `Flags` | multi-select or checkbox list |
| `List` | repeated item editor |
| `Object` | grouped sub-fields |

The UI decides which visual component fits available space and product style.

### 15.5 Device Page Integration

The device page should be able to request:

1. device summary
2. driver presentation
3. device control surface
4. optional driver control surface
5. latest metrics/state

The page can then organize generic sections such as connection, output,
topology, color, performance, diagnostics, and actions without hardcoding
drivers.

### 15.6 Dirty State and Revisions

The UI should track edits against a surface revision:

- If revision matches, submit normally.
- If revision changed, refetch or merge local edits when possible.
- If a field disappears or becomes unavailable, keep the local draft but block
  submit with the server-provided reason.

### 15.7 Unknown Future Types

If the UI receives a future `ControlValueType` variant it does not understand,
it should show a non-editable fallback with the label, description, current
value summary, and unsupported-type reason. It should not crash or hide the
entire surface.

---

## 16. CLI and TUI Consumption

The same surface documents should power CLI and TUI flows.

CLI examples:

```bash
hypercolor devices controls DEVICE_ID
hypercolor devices set-control DEVICE_ID color_order enum:grb
hypercolor devices action DEVICE_ID identify duration_ms:1500
hypercolor drivers controls wled
hypercolor drivers set-control wled default_protocol enum:ddp
```

TUI can render the same semantic groups and value types using terminal-native
widgets. It should not need driver-specific panels for common fields.

---

## 17. Future Wasm Shape

The control model should map to WIT records and variants.

Sketch:

```wit
variant control-value {
  null,
  bool(bool),
  integer(s64),
  float(float64),
  string(string),
  secret-ref(string),
  color-rgb(tuple<u8, u8, u8>),
  color-rgba(tuple<u8, u8, u8, u8>),
  ip-address(string),
  mac-address(string),
  duration-ms(u64),
  enum(string),
  flags(list<string>),
  list(list<control-value>),
  object(list<tuple<string, control-value>>),
}

interface controls {
  driver-surface: func(config: config-entry) -> result<option<surface-document>, string>;
  device-surface: func(device: tracked-device) -> result<option<surface-document>, string>;
  validate-changes: func(target: apply-target, changes: list<control-change>) -> result<validated-changes, string>;
  apply-changes: func(target: apply-target, changes: validated-changes) -> result<apply-response, string>;
  invoke-action: func(target: apply-target, action-id: string, input: list<tuple<string, control-value>>) -> result<action-result, string>;
}
```

Wasm drivers should not receive direct file-system config access. They receive
typed config/control values from the daemon host.

---

## 18. Security and Safety

### 18.1 Secrets

Secret controls never return raw secret values. They return `SecretRef`,
presence state, or redacted summaries. Writes go through the credential store.

### 18.2 Dangerous Actions

Actions with destructive or hardware-persistent effects must include
`ActionConfirmation`. The API should still enforce permission checks even if a
client ignores confirmation metadata.

### 18.3 Bounds

Drivers must validate all values server-side. Client-side validation is only a
convenience.

### 18.4 Sandboxed Drivers

Future Wasm drivers can expose descriptors and validate/apply requests, but
host imports decide what I/O, credentials, cache, and lifecycle operations are
allowed.

---

## 19. Implementation Plan

### Phase 1: Shared Types

Files:

- `crates/hypercolor-types/src/controls.rs`
- `crates/hypercolor-types/src/lib.rs`
- `crates/hypercolor-types/tests/control_surface_tests.rs`

Work:

- add `ControlValueType` and `ControlValue`
- add field, group, action, availability, and apply descriptors
- add serde roundtrip tests
- add validation helpers for type compatibility

Verify:

- `cargo test -p hypercolor-types control`

### Phase 2: Driver API Capability

Files:

- `crates/hypercolor-driver-api/src/lib.rs`
- driver API tests

Work:

- add `DriverControlProvider`
- add `ControlApplyTarget`
- add host service traits for persistence and dynamic operations
- extend `DriverCapabilitySet` with `controls`

Verify:

- `cargo test -p hypercolor-driver-api`

### Phase 3: Daemon Control Service

Files:

- `crates/hypercolor-daemon/src/controls/`
- `crates/hypercolor-daemon/src/api/controls.rs`
- daemon API tests

Work:

- compose driver and device surface documents
- implement revision tracking
- implement apply transaction routing
- implement action invocation routing
- add API routes

Verify:

- `cargo test -p hypercolor-daemon control`

### Phase 4: First Driver Surfaces

Start with WLED because it exercises protocol choice, transport behavior,
known IPs, and device-specific output controls.

Then add Hue and Nanoleaf.

Work:

- expose driver-level WLED controls
- expose device-level WLED controls for protocol, FPS cap, color order, and
  identify
- expose Hue pairing/auth read-only state and entertainment selection
- expose Nanoleaf transition and topology refresh action

Verify:

- driver crate tests
- daemon integration tests for apply and action routing

### Phase 5: UI Data Layer

Files:

- `crates/hypercolor-ui/src/api/controls.rs`
- generated/shared API types
- device page state modules

Work:

- fetch surface documents
- normalize typed values into UI state
- submit typed patches with expected revision
- subscribe to control events
- render generic control groups without driver-specific conditionals

Verify:

- UI wasm check
- component/unit tests where practical
- browser verification once visual components exist

### Phase 6: CLI and TUI

Work:

- add CLI list/set/action commands
- add TUI generic control panel

Verify:

- CLI parser/request tests
- TUI state tests

---

## 20. Verification

Minimum acceptance checks:

- Typed value serde roundtrips preserve discriminants.
- Invalid value kind for a field is rejected before driver apply.
- WLED driver-level protocol control appears through the generic API.
- A WLED device control can be changed without daemon restart.
- A multi-field transaction validates all fields before persisting.
- A stale revision returns a typed conflict error.
- A driver action returns typed progress/result events.
- UI data layer can render a surface with no driver-specific code.
- Unknown future value type does not crash the UI.
- `just verify` passes.

Independent verification should try:

- offline device apply behavior
- unavailable conditional controls
- secret fields
- action confirmation enforcement
- reconnect/rebind impacts

---

## 21. Recommendation

Implement typed control surfaces as a separate layer above config and below UI.
Do not extend `HypercolorConfig` with device-specific typed structs, and do not
let the UI consume raw driver JSON. The daemon should expose one typed,
revisioned document per driver or device surface, and drivers should implement
dynamic apply transactions rather than restart-required settings.

This gives Hypercolor a clean bridge from native built-in drivers to future
Wasm modules while letting the devices page become genuinely generic.
