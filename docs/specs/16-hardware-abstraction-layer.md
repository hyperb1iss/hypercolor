# 16 -- Hardware Abstraction Layer (HAL)

> Protocol encoding separated from transport I/O, a static device database keyed by VID/PID, and a `UsbBackend` adapter that bridges raw USB devices up to the engine's `DeviceBackend` trait.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Author:** Nova
**Date:** 2026-03-04

---

## Table of Contents

1. [Overview](#1-overview)
2. [Dependency Graph](#2-dependency-graph)
3. [Protocol Trait](#3-protocol-trait)
4. [Transport Trait](#4-transport-trait)
5. [Protocol Database](#5-protocol-database)
6. [UsbBackend Adapter](#6-usbbackend-adapter)
7. [USB Scanner](#7-usb-scanner)
8. [USB Hotplug](#8-usb-hotplug)
9. [Type Additions](#9-type-additions)
10. [Bridge Backend Pattern](#10-bridge-backend-pattern)
11. [Crate Layout](#11-crate-layout)
12. [Integration Flow](#12-integration-flow)
13. [Testing Strategy](#13-testing-strategy)

---

## 1. Overview

The Hardware Abstraction Layer introduces a clean separation between **protocol encoding** (pure byte construction and parsing) and **transport I/O** (async USB communication) for USB HID devices. This is the foundation that all native USB drivers (Razer, Lian Li, future Corsair Phase 2) plug into.

### Why a Separate Crate

The existing engine layer (`hypercolor-core`) defines `DeviceBackend` and `TransportScanner` — high-level traits for device communication and discovery. These are transport-agnostic and work for network backends (WLED, Hue) as well as USB.

For USB devices, we need a lower layer:

- **Protocol** — Pure byte encoding/decoding for a specific device family's wire format. Zero I/O, zero async. Testable with no hardware.
- **Transport** — Async byte I/O over a specific USB transfer type (control, interrupt, vendor). Swappable for mocks in CI.
- **Protocol Database** — Static registry mapping USB VID/PID pairs to device metadata and protocol parameters. Compile-time knowledge of every supported device.

These live in `hypercolor-hal` because:

1. **No upward dependency.** The HAL depends on `hypercolor-types` for shared vocabulary and `nusb` for USB access. It cannot depend on `hypercolor-core` (which defines `DeviceBackend`) without creating a cycle.
2. **Reusable across backends.** The `UsbBackend` adapter in `hypercolor-core` imports HAL traits and wraps them into a `DeviceBackend`. Future crates (e.g., a standalone CLI tool) could use HAL directly.
3. **Driver isolation.** Each driver family (Razer, Lian Li, Corsair) implements `Protocol` and uses a `Transport` impl independently. Adding a new driver never touches existing ones.

### What the HAL Does Not Do

- **Device lifecycle management** — Owned by `DeviceLifecycleManager` in core.
- **Frame routing** — Owned by `BackendManager` in core.
- **Discovery orchestration** — Owned by `DiscoveryOrchestrator` in core.
- **Network protocols** — WLED/DDP and Hue stay in core. The HAL is USB-specific.

---

## 2. Dependency Graph

```
hypercolor-types        (pure data types, zero deps)
       │
       ▼
hypercolor-hal           (Protocol + Transport + Database; depends on types + nusb)
       │
       ▼
hypercolor-core          (DeviceBackend, UsbBackend adapter, engine; depends on hal + types)
       │
       ▼
hypercolor-daemon        (binary: REST API, WebSocket, daemon lifecycle)
hypercolor-cli           (binary: CLI tool)
```

### Crate Dependencies

| Crate | Depends On | Provides |
|-------|-----------|----------|
| `hypercolor-types` | `serde`, `uuid` | `DeviceFamily`, `DeviceIdentifier`, `DeviceInfo`, `DeviceColorFormat` |
| `hypercolor-hal` | `hypercolor-types`, `nusb`, `thiserror`, `tracing` | `Protocol`, `Transport`, `ProtocolDatabase`, driver implementations |
| `hypercolor-core` | `hypercolor-types`, `hypercolor-hal`, `tokio`, `async-trait` | `DeviceBackend`, `UsbBackend`, `UsbScanner`, `UsbHotplugMonitor` |

**Critical constraint:** `hypercolor-hal` MUST NOT depend on `hypercolor-core`. The dependency arrow is strictly `types → hal → core`.

---

## 3. Protocol Trait

The `Protocol` trait represents pure byte encoding and decoding for a device family's wire format. Implementations contain device-specific knowledge (packet layout, CRC algorithms, command vocabularies) but perform zero I/O.

```rust
/// Pure byte-level protocol encoder/decoder.
///
/// Implementations encode Hypercolor commands into device-specific
/// byte sequences and decode device responses back into structured
/// data. All methods are synchronous and infallible for encoding
/// (errors only occur during response parsing).
///
/// # Design Principle
///
/// Protocol is a pure function from (command, state) → bytes and
/// bytes → response. It holds device parameters (matrix size,
/// LED count, protocol version) but never touches I/O. This makes
/// every protocol fully testable without hardware.
pub trait Protocol: Send + Sync {
    /// Human-readable protocol name for logging and diagnostics.
    ///
    /// Examples: `"Razer Extended"`, `"Lian Li SL Infinity"`, `"iCUE LINK"`.
    fn name(&self) -> &str;

    /// Byte sequence(s) to send when first connecting to the device.
    ///
    /// Typically: set software mode, query firmware version, read
    /// device capabilities. Returns an empty vec if no init is needed.
    fn init_sequence(&self) -> Vec<ProtocolCommand>;

    /// Byte sequence(s) to send during clean disconnection.
    ///
    /// Typically: restore hardware mode, set a shutdown color.
    /// Returns an empty vec if no shutdown is needed.
    fn shutdown_sequence(&self) -> Vec<ProtocolCommand>;

    /// Encode a full LED frame into transport-ready byte sequence(s).
    ///
    /// The `colors` slice contains one RGB triplet per LED, ordered
    /// by zone then by LED index within the zone. The implementation
    /// handles color format conversion (RGB → R-B-G for Lian Li),
    /// brightness scaling, multi-packet splitting, CRC computation,
    /// and any activation/commit commands.
    ///
    /// Returns one or more commands that must be sent sequentially
    /// with the inter-command delays specified in each `ProtocolCommand`.
    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand>;

    /// Parse a raw response buffer into a structured response.
    ///
    /// # Errors
    ///
    /// Returns `ProtocolError` if the response is malformed, has an
    /// invalid CRC, or contains an error status code.
    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError>;

    /// Zone descriptors for this device.
    ///
    /// Each zone maps to a contiguous range of LEDs with a name and
    /// topology hint. Used by `UsbBackend` to populate `DeviceInfo::zones`.
    fn zones(&self) -> Vec<ProtocolZone>;

    /// Aggregate device capabilities.
    fn capabilities(&self) -> DeviceCapabilities;

    /// Total LED count across all zones.
    fn total_leds(&self) -> u32;

    /// Minimum interval between frames for this device.
    ///
    /// Derived from the device's USB timing constraints. For example,
    /// a 6-row Razer keyboard with 1ms inter-row delay + activation
    /// needs ~7ms per frame → 143 Hz max. Returns `Duration::ZERO`
    /// if the device has no known timing constraint.
    fn frame_interval(&self) -> std::time::Duration;
}
```

### Supporting Types

```rust
/// A single command to send over the transport.
#[derive(Debug, Clone)]
pub struct ProtocolCommand {
    /// Raw bytes to send.
    pub data: Vec<u8>,

    /// Whether a response should be read after sending this command.
    pub expects_response: bool,

    /// Minimum delay after sending this command before the next one.
    /// `Duration::ZERO` means no delay.
    pub post_delay: std::time::Duration,
}

/// Parsed response from a device.
#[derive(Debug, Clone)]
pub struct ProtocolResponse {
    /// Response status.
    pub status: ResponseStatus,

    /// Parsed payload data (firmware version, serial number, etc.).
    pub data: Vec<u8>,
}

/// Response status codes.
///
/// Variants are protocol-family-agnostic. Individual protocol
/// implementations map device-specific status bytes to these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseStatus {
    /// Command succeeded.
    Ok,
    /// Device is busy processing a previous command.
    Busy,
    /// Command failed (rejected by device).
    Failed,
    /// Communication timed out.
    Timeout,
    /// Command not supported by this device.
    Unsupported,
}

/// Zone descriptor emitted by a protocol.
#[derive(Debug, Clone)]
pub struct ProtocolZone {
    /// Zone name (e.g., "Backlight", "Logo", "Channel 1").
    pub name: String,
    /// Number of LEDs in this zone.
    pub led_count: u32,
    /// Physical topology hint.
    pub topology: DeviceTopologyHint,
    /// Wire-level color format.
    pub color_format: DeviceColorFormat,
}

/// Protocol-level errors.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Response CRC doesn't match computed value.
    #[error("CRC mismatch: expected {expected:#04X}, got {actual:#04X}")]
    CrcMismatch { expected: u8, actual: u8 },

    /// Response buffer is too short or structurally invalid.
    #[error("malformed response: {detail}")]
    MalformedResponse { detail: String },

    /// Device reported an error status.
    #[error("device error: {status:?}")]
    DeviceError { status: ResponseStatus },

    /// Data doesn't fit within protocol constraints.
    #[error("encoding error: {detail}")]
    EncodingError { detail: String },
}
```

---

## 4. Transport Trait

The `Transport` trait is the async I/O boundary. Implementations send and receive raw byte buffers over a specific USB transfer mechanism. Transport knows nothing about protocol semantics — it just moves bytes.

```rust
/// Async byte-level I/O transport.
///
/// Implementations wrap a specific USB transfer type (control
/// transfer, HID interrupt, vendor control) and handle OS-level
/// details: claiming interfaces, setting configurations, managing
/// timeouts.
///
/// Transport is `Send + Sync` and intended for use within a tokio
/// runtime. Long-running USB operations are dispatched to blocking
/// threads internally.
#[async_trait::async_trait]
pub trait Transport: Send + Sync {
    /// Human-readable transport name for logging.
    ///
    /// Examples: `"USB Control (Razer)"`, `"USB HID Interrupt"`,
    /// `"USB Vendor Control"`.
    fn name(&self) -> &str;

    /// Send raw bytes to the device.
    ///
    /// # Errors
    ///
    /// Returns `TransportError` on USB communication failure.
    async fn send(&self, data: &[u8]) -> Result<(), TransportError>;

    /// Receive raw bytes from the device.
    ///
    /// The returned buffer may be shorter than the device's maximum
    /// response size. Returns an empty vec if no data is available
    /// within the timeout.
    ///
    /// # Errors
    ///
    /// Returns `TransportError` on USB communication failure.
    async fn receive(&self, timeout: std::time::Duration) -> Result<Vec<u8>, TransportError>;

    /// Send a command and read the response in one operation.
    ///
    /// Default implementation calls `send()` then `receive()`. Transports
    /// may override for atomicity or performance.
    ///
    /// # Errors
    ///
    /// Returns `TransportError` on USB communication failure.
    async fn send_receive(
        &self,
        data: &[u8],
        timeout: std::time::Duration,
    ) -> Result<Vec<u8>, TransportError> {
        self.send(data).await?;
        self.receive(timeout).await
    }

    /// Close the transport and release OS resources.
    ///
    /// After calling `close()`, all subsequent `send`/`receive` calls
    /// will return `TransportError::Closed`.
    async fn close(&self) -> Result<(), TransportError>;
}

/// Transport-level errors.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// USB device not found or not accessible.
    #[error("device not found: {detail}")]
    NotFound { detail: String },

    /// USB communication error.
    #[error("USB I/O error: {detail}")]
    IoError { detail: String },

    /// Operation timed out.
    #[error("transport timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Transport has been closed.
    #[error("transport closed")]
    Closed,

    /// Permission denied (udev rules, etc.).
    #[error("permission denied: {detail}")]
    PermissionDenied { detail: String },
}
```

### Concrete Transport Implementations

Three transport types cover all USB device families:

#### `UsbControlTransport`

Used by **Razer** devices. Sends/receives 90-byte HID feature reports via USB control transfers.

```rust
/// USB control transfer transport for HID feature reports.
///
/// Used by Razer devices which communicate exclusively through
/// GET_REPORT / SET_REPORT control transfers rather than interrupt
/// endpoints.
///
/// | Operation   | bmRequestType | bRequest | wValue   | wIndex    |
/// |-------------|---------------|----------|----------|-----------|
/// | SET_REPORT  | 0x21          | 0x09     | 0x0300+  | interface |
/// | GET_REPORT  | 0xA1          | 0x01     | 0x0300+  | interface |
pub struct UsbControlTransport {
    /// nusb device handle.
    device: nusb::Device,
    /// Claimed USB interface.
    interface: nusb::Interface,
    /// Interface number for wIndex.
    interface_number: u8,
    /// HID report ID (0x00 for most devices).
    report_id: u8,
}
```

#### `UsbHidTransport`

Used by **Lian Li modern hubs** (`0xA100`+) and **PrismRGB** controllers. Sends HID reports via interrupt OUT endpoint, receives via interrupt IN endpoint.

```rust
/// USB HID interrupt transfer transport.
///
/// Used by Lian Li modern hubs and PrismRGB controllers which
/// communicate through HID interrupt endpoints (usage page 0xFF72
/// for Lian Li, standard HID for PrismRGB).
///
/// Detects endpoints automatically from the HID interface descriptor.
pub struct UsbHidTransport {
    /// nusb device handle.
    device: nusb::Device,
    /// Claimed USB interface.
    interface: nusb::Interface,
    /// Interface number.
    interface_number: u8,
}
```

#### `UsbVendorTransport`

Used by the **Lian Li original hub** (`0x7750`). Sends vendor-specific control transfers with register-addressed `wIndex` values.

```rust
/// USB vendor-specific control transfer transport.
///
/// Used by the original Lian Li Uni Hub which communicates via
/// vendor control transfers with register-addressed wIndex values
/// rather than HID reports.
///
/// | Operation | bmRequestType | bRequest | wValue | wIndex   |
/// |-----------|---------------|----------|--------|----------|
/// | Write     | 0x40          | 0x80     | 0x0000 | register |
/// | Read      | 0xC0          | 0x81     | 0x0000 | register |
pub struct UsbVendorTransport {
    /// nusb device handle.
    device: nusb::Device,
    /// Claimed USB interface.
    interface: nusb::Interface,
}
```

---

## 5. Protocol Database

The protocol database is a static, compile-time registry mapping USB VID/PID pairs to device metadata and protocol construction parameters. Every supported USB device has an entry. No runtime registration is needed since all drivers live in `hypercolor-hal`.

### Device Descriptor

```rust
/// Static metadata for a known USB device.
///
/// Compiled into the binary. Provides everything needed to:
/// 1. Identify a device during USB enumeration (VID/PID match)
/// 2. Select the correct transport type
/// 3. Instantiate the correct protocol with the right parameters
#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    /// USB Vendor ID.
    pub vendor_id: u16,
    /// USB Product ID.
    pub product_id: u16,
    /// Human-readable device name.
    pub name: &'static str,
    /// Device family classification.
    pub family: DeviceFamily,
    /// Transport type required by this device.
    pub transport: TransportType,
    /// Protocol-family-specific parameters.
    pub params: ProtocolParams,
    /// Optional predicate for disambiguating same-PID devices
    /// by firmware version string. `None` means unconditional match.
    pub firmware_predicate: Option<fn(&str) -> bool>,
}
```

### Transport Type

```rust
/// USB transport mechanism for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    /// USB control transfers for HID feature reports (Razer).
    UsbControl {
        /// USB interface number to claim.
        interface: u8,
        /// HID report ID.
        report_id: u8,
    },
    /// USB HID interrupt transfers (Lian Li modern, PrismRGB).
    UsbHid {
        /// USB interface number to claim.
        interface: u8,
    },
    /// USB vendor-specific control transfers (Lian Li original hub).
    UsbVendor,
}
```

### Protocol Parameters

```rust
/// Protocol-family-specific parameters stored in device descriptors.
///
/// Each variant carries the minimum data needed to instantiate the
/// corresponding protocol implementation.
#[derive(Debug, Clone)]
pub enum ProtocolParams {
    /// Razer Chroma protocol parameters.
    Razer {
        /// Protocol generation (determines transaction_id and command class).
        version: RazerProtocolVersion,
        /// Matrix addressing mode.
        matrix_type: RazerMatrixType,
        /// Matrix dimensions (rows, columns).
        matrix_size: (u8, u8),
        /// Primary LED ID.
        led_id: u8,
    },
    /// Lian Li Uni Hub protocol parameters.
    LianLi {
        /// Hub hardware variant (SL, AL, SLV2, SL Infinity, Original).
        variant: LianLiHubVariant,
    },
    /// PrismRGB protocol parameters.
    PrismRgb {
        /// Controller model.
        model: PrismRgbModel,
    },
    /// Corsair iCUE LINK protocol parameters (Phase 2 — future).
    Corsair {
        /// USB interface number.
        interface: u8,
        /// HID usage page.
        usage_page: u16,
        /// HID usage.
        usage: u16,
    },
}
```

### Protocol Database

```rust
/// Static device database with O(1) VID/PID lookup.
///
/// All entries are compiled into the binary. The database is
/// initialized lazily on first access via `std::sync::LazyLock`.
pub struct ProtocolDatabase;

impl ProtocolDatabase {
    /// Look up a device descriptor by VID/PID pair.
    ///
    /// Returns `None` if the device is not in the database.
    pub fn lookup(vendor_id: u16, product_id: u16) -> Option<&'static DeviceDescriptor>;

    /// All known VID/PID pairs, for use by the USB scanner's
    /// enumeration filter.
    pub fn known_vid_pids() -> &'static [(u16, u16)];

    /// All descriptors for a given vendor ID.
    pub fn by_vendor(vendor_id: u16) -> Vec<&'static DeviceDescriptor>;

    /// Total number of registered device descriptors.
    pub fn count() -> usize;
}
```

Internally, the database uses a `HashMap<(u16, u16), Vec<DeviceDescriptor>>` wrapped in `LazyLock`. The map key is `(VID, PID)`, but the value is a `Vec` because some PIDs require secondary disambiguation (see below). The `known_vid_pids()` method returns a pre-computed static slice for fast scanner filtering.

### Firmware-Gated Dispatch

Some Lian Li hubs share a PID but require different drivers based on firmware version read from the USB product string (spec 19 §2.4). The protocol database handles this with a **predicate chain**: `lookup()` returns all descriptors for a VID/PID, and the scanner evaluates an optional `firmware_predicate` on each to select the correct one:

```rust
/// Optional predicate for disambiguating same-PID devices.
///
/// Evaluated against the USB product string during scanner enumeration.
/// If `None`, the descriptor always matches.
pub firmware_predicate: Option<fn(&str) -> bool>,
```

| PID | Firmware | Matched Variant |
|-----|----------|-----------------|
| `0xA101` | contains `"v1.7"` | `LianLiHubVariant::Al` |
| `0xA101` | contains `"v1.0"` | `LianLiHubVariant::Original` (AL10 fallback) |
| `0xA101` | (default) | `LianLiHubVariant::Al` |

Devices without firmware gating (Razer, PrismRGB, most Lian Li variants) have `firmware_predicate: None` and match unconditionally — `lookup()` returns the single entry directly.

### Registration Macros

Each driver family provides a registration macro that expands into a `DeviceDescriptor` const and an entry in the database's static initializer:

```rust
/// Register a Razer device.
///
/// # Example
/// ```
/// razer_device!(HUNTSMAN_V2, 0x026C, "Razer Huntsman V2",
///     Extended, Extended, (6, 22), 3, BACKLIGHT);
/// ```
macro_rules! razer_device {
    ($name:ident, $pid:expr, $display:expr,
     $version:ident, $matrix:ident, ($rows:expr, $cols:expr),
     $interface:expr, $led_id:ident) => { ... };
}

/// Register a Lian Li device.
///
/// # Example
/// ```
/// lianli_device!(UNI_HUB_SL_INFINITY, 0xA102,
///     "Lian Li Uni Hub - SL Infinity", SlInfinity);
/// ```
macro_rules! lianli_device {
    ($name:ident, $pid:expr, $display:expr, $variant:ident) => { ... };
}

/// Register a PrismRGB device.
///
/// # Example
/// ```
/// prismrgb_device!(PRISM_8, 0x16D5, 0x1F01, "PrismRGB Prism 8", Prism8);
/// ```
macro_rules! prismrgb_device {
    ($name:ident, $vid:expr, $pid:expr, $display:expr, $model:ident) => { ... };
}

/// Register a Corsair device (Phase 2 — future).
///
/// # Example
/// ```
/// corsair_device!(ICUE_LINK_SYSTEM_HUB, 0x0C3F,
///     "Corsair iCUE LINK System Hub",
///     interface: 0, usage_page: 0xFF42, usage: 0x01);
/// ```
macro_rules! corsair_device {
    ($name:ident, $pid:expr, $display:expr,
     interface: $iface:expr, usage_page: $up:expr, usage: $u:expr) => { ... };
}
```

### PrismRGB Migration Reference

PrismRGB devices (spec 04) migrate into the HAL with these VID/PID mappings:

| Device | VID | PID | Interface | Model |
|--------|-----|-----|-----------|-------|
| Prism 8 | `0x16D5` | `0x1F01` | 0 | `Prism8` |
| Nollie 8 v2 | `0x16D2` | `0x1F01` | 0 | `Nollie8` |
| Prism S | `0x16D0` | `0x1294` | 2 | `PrismS` |
| Prism Mini | `0x16D0` | `0x1407` | 2 | `PrismMini` |

Note: PrismRGB controllers use VIDs `0x16D5`, `0x16D2`, and `0x16D0` — completely separate from Lian Li Uni Hubs (VID `0x0CF2`). See spec 19 §9 for disambiguation.

---

## 6. UsbBackend Adapter

The `UsbBackend` adapter lives in `hypercolor-core` (not `hypercolor-hal`) and implements the `DeviceBackend` trait. It bridges the HAL's Protocol+Transport pairs up to the engine's device management layer.

### Why It Lives in Core

`DeviceBackend` is defined in `hypercolor-core`. If `UsbBackend` lived in `hypercolor-hal`, the HAL would need to depend on core to access the trait — creating a circular dependency. Instead:

- **HAL exports:** `Protocol`, `Transport`, `ProtocolDatabase`, driver implementations
- **Core imports:** HAL traits and wraps them in `UsbBackend`

```rust
/// USB device backend — bridges HAL Protocol+Transport pairs
/// to the engine's DeviceBackend trait.
///
/// Manages multiple USB devices, each with its own Protocol+Transport
/// pair. Handles the connect → init_sequence → frame routing →
/// shutdown_sequence → disconnect lifecycle for each device.
pub struct UsbBackend {
    /// Active devices keyed by DeviceId.
    devices: HashMap<DeviceId, UsbDevice>,
}

/// One connected USB device with its protocol and transport.
struct UsbDevice {
    /// Protocol encoder/decoder for this device's family.
    protocol: Box<dyn Protocol>,
    /// Transport for USB I/O.
    transport: Box<dyn Transport>,
    /// Device descriptor from the protocol database.
    descriptor: &'static DeviceDescriptor,
}
```

### DeviceBackend Implementation

```rust
#[async_trait::async_trait]
impl DeviceBackend for UsbBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "usb".to_owned(),
            name: "USB HID (HAL)".to_owned(),
            description: "Native USB device control via Protocol+Transport HAL"
                .to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        // Delegates to UsbScanner (see §7)
        // UsbBackend does not own the scanner — discovery is
        // handled externally by DiscoveryOrchestrator
        Ok(Vec::new())
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        // 1. Look up DeviceDescriptor from ProtocolDatabase
        // 2. Open nusb device and construct Transport
        // 3. Construct Protocol from ProtocolParams
        // 4. Execute protocol.init_sequence() via transport
        // 5. Store UsbDevice in self.devices
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        // 1. Execute protocol.shutdown_sequence() via transport
        // 2. Call transport.close()
        // 3. Remove from self.devices
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        // 1. Look up UsbDevice
        // 2. Call protocol.encode_frame(colors) → Vec<ProtocolCommand>
        // 3. For each command:
        //    a. transport.send(command.data)
        //    b. if expects_response: transport.receive(timeout)
        //       → protocol.parse_response() → handle retry on Busy
        //    c. if post_delay > 0: tokio::time::sleep(post_delay)
    }
}
```

### Backend Routing

The daemon's `backend_id_for_family()` function (in `hypercolor-daemon/src/discovery.rs`) maps `DeviceFamily` variants to backend IDs. All HAL-managed USB families route to the single `"usb"` backend:

```rust
fn backend_id_for_family(family: &DeviceFamily) -> String {
    match family {
        DeviceFamily::Wled     => "wled".to_owned(),
        DeviceFamily::Hue      => "hue".to_owned(),
        // HAL-managed USB families → single UsbBackend
        DeviceFamily::Razer    => "usb".to_owned(),
        DeviceFamily::LianLi   => "usb".to_owned(),
        DeviceFamily::PrismRgb => "usb".to_owned(),
        // Bridge backends → dedicated backend IDs
        DeviceFamily::Corsair  => "corsair-bridge".to_owned(),
        DeviceFamily::Custom(name) => name.to_ascii_lowercase(),
    }
}
```

**Key design point:** All native USB devices share one `UsbBackend` instance (backend ID `"usb"`). The `UsbBackend` internally dispatches to the correct Protocol+Transport pair per device based on `DeviceDescriptor`. This is different from Corsair Phase 1, which gets its own bridge backend with ID `"corsair-bridge"`.

When Corsair Phase 2 (native iCUE LINK) is implemented, `DeviceFamily::Corsair` will switch to routing to `"usb"` and the bridge backend will become a fallback option in config.

The `DiscoveryBackend` enum in the daemon must also be extended:

```rust
pub enum DiscoveryBackend {
    Wled,
    Usb,           // new — UsbScanner
    CorsairBridge, // new — HTTP health check + device list
}
```

### Connect Flow Detail

```
connect(device_id)
│
├── ProtocolDatabase::lookup(vid, pid)
│   └── DeviceDescriptor { transport: UsbControl { interface: 3, report_id: 0 }, ... }
│
├── nusb::list_devices()? → find matching device
│   └── device.open()? → nusb::Device
│
├── Match descriptor.transport:
│   ├── UsbControl  → UsbControlTransport::new(device, interface, report_id)
│   ├── UsbHid      → UsbHidTransport::new(device, interface)
│   └── UsbVendor   → UsbVendorTransport::new(device)
│
├── Match descriptor.params:
│   ├── Razer { .. }    → RazerProtocol::new(version, matrix_type, matrix_size, led_id)
│   ├── LianLi { .. }  → LianLiProtocol::new(variant)
│   ├── PrismRgb { .. } → PrismRgbProtocol::new(model)
│   └── Corsair { .. }  → IcueLinkProtocol::new(interface, usage_page, usage)  [Phase 2]
│
├── protocol.init_sequence() → Vec<ProtocolCommand>
│   └── For each cmd: transport.send(cmd.data), handle response
│
└── Store UsbDevice { protocol, transport, descriptor }
```

---

## 7. USB Scanner

`UsbScanner` implements the `TransportScanner` trait from `hypercolor-core`, using `nusb::list_devices()` filtered against the protocol database's known VID/PIDs.

```rust
/// USB device scanner using nusb enumeration.
///
/// Filters the OS USB device list against the protocol database
/// to find supported devices. Runs in <100ms (target) / <500ms (max).
pub struct UsbScanner;

#[async_trait::async_trait]
impl TransportScanner for UsbScanner {
    fn name(&self) -> &str { "USB" }

    async fn scan(&mut self) -> Result<Vec<DiscoveredDevice>> {
        // 1. nusb::list_devices()?
        // 2. For each USB device:
        //    a. ProtocolDatabase::lookup(vid, pid)
        //    b. If found → build DiscoveredDevice from descriptor
        //    c. DeviceIdentifier::UsbHid { vendor_id, product_id, serial, usb_path }
        //    d. DeviceInfo populated from descriptor metadata
        // 3. Return all matched devices
    }
}
```

### Discovery Device Construction

The scanner builds `DiscoveredDevice` entries with:

| Field | Source |
|-------|--------|
| `connection_type` | `ConnectionType::Usb` |
| `name` | `DeviceDescriptor.name` |
| `family` | `DeviceDescriptor.family` |
| `fingerprint` | `DeviceFingerprint` from `DeviceIdentifier::UsbHid` |
| `info.zones` | Protocol instance `zones()` (constructed temporarily for metadata) |
| `info.capabilities` | Protocol instance `capabilities()` |

The scanner instantiates a protocol temporarily during scan to query zone metadata and capabilities. This is cheap since protocols are pure data structures with no I/O.

---

## 8. USB Hotplug

`UsbHotplugMonitor` watches for USB device arrival and removal using `nusb`'s hotplug API, emitting events for targeted rescans and lifecycle updates.

```rust
/// USB hotplug monitor using nusb::watch_devices().
///
/// Runs as a background tokio task. Filters events against the
/// protocol database and emits discovery/removal signals via
/// a broadcast channel.
pub struct UsbHotplugMonitor {
    /// Broadcast sender for hotplug events.
    event_tx: tokio::sync::broadcast::Sender<UsbHotplugEvent>,
}

/// USB hotplug event.
#[derive(Debug, Clone)]
pub enum UsbHotplugEvent {
    /// A known USB device was connected.
    Arrived {
        vendor_id: u16,
        product_id: u16,
        descriptor: &'static DeviceDescriptor,
    },
    /// A USB device was disconnected.
    Removed {
        vendor_id: u16,
        product_id: u16,
    },
}
```

### Event Flow

```
USB device plugged in
│
├── nusb hotplug callback fires
│
├── ProtocolDatabase::lookup(vid, pid)
│   ├── Known → emit UsbHotplugEvent::Arrived
│   └── Unknown → ignore
│
├── Daemon receives Arrived event
│   └── Triggers targeted rescan via DiscoveryOrchestrator
│       (only UsbScanner, not full multi-transport sweep)
│
└── DiscoveryOrchestrator → LifecycleManager::on_discovered()

USB device unplugged
│
├── nusb hotplug callback fires
│
├── Emit UsbHotplugEvent::Removed
│
├── Daemon receives Removed event
│   └── LifecycleManager::on_device_vanished(device_id)
│       → LifecycleAction::Disconnect + Unmap
│
└── UsbBackend::disconnect() handles transport cleanup
```

---

## 9. Type Additions

### DeviceFamily Variants

New variants added to `DeviceFamily` in `hypercolor-types`:

```rust
pub enum DeviceFamily {
    Wled,
    Hue,
    Razer,      // USB VID 0x1532
    Corsair,    // USB VID 0x1B1C
    LianLi,    // USB VID 0x0CF2 (ENE Technology)
    PrismRgb,  // USB VIDs 0x16D5, 0x16D2, 0x16D0
    Custom(String),
}
```

### DeviceIdentifier::Bridge Variant

New variant for bridge-managed devices (Corsair Phase 1 via OpenLinkHub):

```rust
pub enum DeviceIdentifier {
    UsbHid { vendor_id: u16, product_id: u16, serial: Option<String>, usb_path: Option<String> },
    Network { mac_address: String, last_ip: Option<IpAddr>, mdns_hostname: Option<String> },
    HueBridge { bridge_id: String, light_id: String },
    /// Device managed by an external bridge service.
    Bridge {
        /// Bridge service identifier (e.g., "openlinkhub").
        service: String,
        /// Device serial or unique ID within the bridge.
        device_serial: String,
    },
}
```

### DeviceColorFormat Variants

New wire-level color formats for devices that don't use standard RGB or RGBW ordering:

```rust
pub enum DeviceColorFormat {
    Rgb,
    Rgbw,
    /// Green-Red-Blue byte order (some PrismRGB controllers).
    Grb,
    /// Red-Blue-Green byte order (all Lian Li Uni Hub variants).
    Rbg,
}
```

---

## 10. Bridge Backend Pattern

Not all USB devices fit the Protocol+Transport pattern. Corsair Phase 1 uses an external service (OpenLinkHub) that handles all USB communication internally. The bridge backend bypasses the HAL entirely and implements `DeviceBackend` directly in `hypercolor-core`.

### When to Use Bridge vs. HAL

| Pattern | When | Example |
|---------|------|---------|
| **Protocol + Transport (HAL)** | Direct USB access with known wire protocol | Razer, Lian Li, PrismRGB, Corsair Phase 2 |
| **Bridge Backend (core)** | External service handles USB; we talk HTTP/gRPC | Corsair OpenLinkHub, future SignalRGB bridge |

### Bridge Backend Characteristics

- Implements `DeviceBackend` directly (not `Protocol`)
- Lives in `hypercolor-core`, not `hypercolor-hal`
- Uses `DeviceIdentifier::Bridge` for device identity
- Uses `ConnectionType::Bridge` for connection type
- Discovery via HTTP health check + device list endpoint
- No `ProtocolDatabase` entry needed (no VID/PID matching)

```
Bridge flow:            HAL flow:
DeviceBackend           DeviceBackend (UsbBackend)
    │                       │
    ▼                       ▼
HTTP Client             Protocol + Transport
    │                       │         │
    ▼                       ▼         ▼
OpenLinkHub             encode()   send()
    │                       │         │
    ▼                       ▼         ▼
USB (managed            Wire bytes  nusb
externally)
```

---

## 11. Crate Layout

```
crates/hypercolor-hal/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # Re-exports: Protocol, Transport, ProtocolDatabase
│   ├── protocol.rs               # Protocol trait + supporting types
│   ├── transport.rs              # Transport trait + TransportError
│   ├── database.rs               # ProtocolDatabase, DeviceDescriptor, macros
│   │
│   ├── transport/                # Concrete transport implementations
│   │   ├── mod.rs
│   │   ├── control.rs            # UsbControlTransport (Razer)
│   │   ├── hid.rs                # UsbHidTransport (Lian Li modern, PrismRGB)
│   │   └── vendor.rs             # UsbVendorTransport (Lian Li original)
│   │
│   └── drivers/                  # Protocol implementations per device family
│       ├── mod.rs
│       ├── razer/
│       │   ├── mod.rs
│       │   ├── protocol.rs       # RazerProtocol impl
│       │   ├── types.rs          # RazerProtocolVersion, RazerMatrixType, etc.
│       │   ├── crc.rs            # razer_crc() + fast XOR folding
│       │   ├── commands.rs       # Command builders (per class/id)
│       │   └── devices.rs        # razer_device!() registrations
│       │
│       ├── lianli/
│       │   ├── mod.rs
│       │   ├── protocol.rs       # LianLiProtocol impl
│       │   ├── types.rs          # LianLiHubVariant, effect tables
│       │   ├── white_protect.rs  # Brightness limiters
│       │   └── devices.rs        # lianli_device!() registrations
│       │
│       ├── prismrgb/
│       │   ├── mod.rs
│       │   ├── protocol.rs       # PrismRgbProtocol impl (migrated from spec 04)
│       │   ├── types.rs          # PrismRgbModel, channel layouts
│       │   └── devices.rs        # prismrgb_device!() registrations
│       │
│       └── corsair/              # Phase 2 — future native iCUE LINK driver
│           ├── mod.rs
│           ├── protocol.rs       # IcueLinkProtocol impl
│           ├── types.rs          # Downstream device types, endpoint addresses
│           └── devices.rs        # corsair_device!() registrations
```

### Core Additions

```
crates/hypercolor-core/src/device/
├── usb_backend.rs                # UsbBackend: DeviceBackend impl wrapping HAL
├── usb_scanner.rs                # UsbScanner: TransportScanner impl using nusb
└── usb_hotplug.rs                # UsbHotplugMonitor: background hotplug watcher
```

---

## 12. Integration Flow

End-to-end flow from daemon startup through frame rendering to hotplug removal.

### Startup

```
daemon main()
│
├── 1. Load config
│
├── 2. Build DiscoveryOrchestrator
│   ├── WledScanner (existing)
│   └── UsbScanner (new — from hypercolor-core, uses HAL database)
│
├── 3. Build BackendManager
│   ├── WledBackend (existing)
│   ├── UsbBackend (new — from hypercolor-core, wraps HAL Protocol+Transport)
│   └── CorsairBridgeBackend (new — direct DeviceBackend, no HAL)
│
├── 4. Start UsbHotplugMonitor (background task)
│
├── 5. DiscoveryOrchestrator::full_scan()
│   ├── WledScanner: mDNS + UDP broadcast → WLED devices
│   └── UsbScanner: nusb::list_devices() filtered by ProtocolDatabase
│       → Razer, Lian Li, PrismRGB devices
│
├── 6. DiscoveryReport → DeviceLifecycleManager
│   └── on_discovered() → LifecycleAction::Connect for each new device
│
├── 7. Execute Connect actions
│   ├── UsbBackend.connect(device_id)
│   │   ├── ProtocolDatabase::lookup(vid, pid) → descriptor
│   │   ├── Construct Transport from descriptor.transport
│   │   ├── Construct Protocol from descriptor.params
│   │   ├── protocol.init_sequence() → send via transport
│   │   └── Store Protocol+Transport pair
│   │
│   └── CorsairBridgeBackend.connect(device_id)
│       └── HTTP probe + device list from OpenLinkHub
│
└── 8. Start render loop
```

### Connect Failure & Reconnect

```
UsbBackend.connect(device_id) fails
│
├── LifecycleManager::on_connect_failed(device_id)
│   └── State: Known → Reconnecting
│   └── LifecycleAction::SpawnReconnect { device_id, delay: 1s }
│
├── Daemon spawns reconnect task (tokio::spawn)
│   └── sleep(delay) → LifecycleManager::on_reconnect_attempt(device_id)
│       └── LifecycleAction::Connect { ... }
│       └── UsbBackend.connect(device_id) retry
│
├── If retry succeeds:
│   └── LifecycleManager::on_connected(device_id)
│       └── State: Reconnecting → Connected
│
├── If retry fails:
│   └── LifecycleManager::on_reconnect_failed(device_id)
│       ├── Attempts remaining → SpawnReconnect with exponential backoff
│       └── Max attempts exhausted → State: Reconnecting → Known
│           └── CancelReconnect (device waits for next discovery scan)
```

### Write Failure & Recovery

```
UsbBackend::write_colors() fails (transport error)
│
├── OutputQueue worker logs warning, sets last_error
│
├── LifecycleManager::on_comm_error(device_id)
│   └── State: Active → Reconnecting
│   └── Actions: [Disconnect, Unmap, SpawnReconnect]
│
├── Execute Disconnect: UsbBackend.disconnect(device_id)
│   └── protocol.shutdown_sequence() (best-effort)
│   └── transport.close()
│
├── Execute Unmap: BackendManager::unmap_device(layout_device_id)
│   └── OutputQueue torn down (no more frames routed)
│
└── SpawnReconnect: same retry logic as connect failure above
```

### Hotplug Arrival

```
USB device physically plugged in
│
├── UsbHotplugMonitor detects arrival
│   └── ProtocolDatabase::lookup(vid, pid) → known device
│   └── Emits UsbHotplugEvent::Arrived { vid, pid, descriptor }
│
├── Daemon event handler
│   └── Trigger targeted UsbScanner rescan (not full multi-transport sweep)
│       └── DiscoveryOrchestrator runs UsbScanner only
│
├── DiscoveryReport → LifecycleManager::on_discovered()
│   ├── New device → LifecycleAction::Connect
│   └── Reconnecting device → CancelReconnect + Connect
│
└── Normal connect flow (§6 Connect Flow Detail)
```

### Frame Rendering

```
render loop tick (60fps)
│
├── Effect engine produces ZoneColors per zone
│
├── BackendManager::write_frame(zone_colors, layout)
│   ├── Group zones by target device via device_map
│   └── For each (backend_id, device_id):
│       └── OutputQueue::push(colors)
│
├── OutputQueue worker (async task per device)
│   └── UsbBackend::write_colors(device_id, colors)
│       ├── protocol.encode_frame(colors)
│       │   → Vec<ProtocolCommand>
│       │   (e.g., Razer: 6 row packets + 1 activation)
│       │   (e.g., Lian Li: activate + color + commit × N channels)
│       │
│       └── For each command:
│           ├── transport.send(cmd.data)
│           ├── if expects_response:
│           │   ├── transport.receive(timeout)
│           │   └── protocol.parse_response()
│           │       → Busy? → retry (up to 3×)
│           └── tokio::time::sleep(cmd.post_delay)
```

### Hotplug Removal

```
USB device physically unplugged
│
├── UsbHotplugMonitor detects removal
│   └── Emits UsbHotplugEvent::Removed { vid, pid }
│
├── Daemon event handler
│   ├── Match device_id from registry by VID/PID
│   └── LifecycleManager::on_device_vanished(device_id)
│       → [Disconnect, Unmap, CancelReconnect]
│
├── Execute Disconnect action
│   └── UsbBackend::disconnect(device_id)
│       ├── protocol.shutdown_sequence() → send via transport
│       │   (best-effort — device may already be gone)
│       ├── transport.close()
│       └── Remove Protocol+Transport pair
│
├── Execute Unmap action
│   └── BackendManager::unmap_device(layout_device_id)
│
└── Device stops receiving frames
    (zones mapped to this device render black / are skipped)
```

---

## 13. Testing Strategy

### MockTransport

A mock transport that records all outgoing data and returns pre-configured responses:

```rust
/// Mock transport for protocol testing.
///
/// Records all `send()` calls and returns pre-configured responses
/// for `receive()`. Allows protocol tests to verify exact byte
/// sequences without USB hardware.
pub struct MockTransport {
    /// All data passed to send(), in order.
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Pre-configured responses, consumed in order.
    responses: Arc<Mutex<VecDeque<Vec<u8>>>>,
    /// Whether close() has been called.
    closed: AtomicBool,
}

impl MockTransport {
    /// Create a mock with no pre-configured responses.
    pub fn new() -> Self;

    /// Create a mock with the given response queue.
    pub fn with_responses(responses: Vec<Vec<u8>>) -> Self;

    /// Snapshot of all sent data.
    pub fn sent_packets(&self) -> Vec<Vec<u8>>;

    /// Number of send() calls.
    pub fn send_count(&self) -> usize;
}
```

### Test Categories

**Protocol encoding (per driver family):**

- Round-trip: build command → serialize → verify byte layout matches reference implementation
- CRC validation against known-good test vectors (Razer)
- Color format reordering: RGB input → R-B-G wire bytes (Lian Li)
- White color protection: verify brightness limiting (Lian Li)
- Multi-packet frame splitting: keyboards with >25 columns per row (Razer)
- Protocol version dispatch: correct transaction_id, command_class per device

**Protocol decoding:**

- Parse response buffers → `ProtocolResponse` with correct status
- CRC mismatch detection → `ProtocolError::CrcMismatch`
- Malformed response handling → `ProtocolError::MalformedResponse`
- Status code mapping: device-specific bytes → `ResponseStatus` enum

**Transport (integration, requires mock or real hardware):**

- `MockTransport`: verify `UsbBackend` sends correct sequences
- Send/receive round-trip with mock responses
- Timeout handling: `receive()` with empty response queue
- Close semantics: operations after `close()` return `TransportError::Closed`

**Protocol Database:**

- All registered VID/PIDs resolve to valid descriptors
- No duplicate VID/PID entries
- Descriptor → Protocol construction → `total_leds()` matches expected
- `known_vid_pids()` returns the full set
- Vendor filter: `by_vendor(0x1532)` returns only Razer devices

**Packet capture replay:**

For each supported device family, maintain a set of captured USB packets (from vendor software, uchroma, or real hardware) and verify that:
- `encode_frame()` produces byte-identical output for the same input colors
- `parse_response()` correctly decodes captured response packets
- Init/shutdown sequences match reference implementations

**End-to-end (integration):**

- `UsbScanner` with mock `nusb::list_devices()` → correct `DiscoveredDevice` construction
- `UsbBackend` connect → init sequence → frame write → shutdown → disconnect via `MockTransport`
- Hotplug arrival → targeted rescan → lifecycle connect
- Hotplug removal → lifecycle vanish → transport cleanup

---

## References

- Spec 04: USB HID Backend (PrismRGB/Nollie) — **deprecated**, superseded by spec 20
- Spec 20: PrismRGB Protocol Driver — `PrismRgbProtocol`, `PrismRgbModel`, all four controller variants
- Spec 17: Razer Protocol Driver — `RazerProtocol`, `RazerTransport`, device registry
- Spec 18: Corsair Integration — `CorsairBridgeBackend` (Phase 1), `IcueLinkProtocol` (Phase 2)
- Spec 19: Lian Li Uni Hub Driver — `LianLiProtocol`, `LianLiHidTransport`, `LianLiCtrlTransport`
- `crates/hypercolor-types/src/device.rs` — `DeviceFamily`, `DeviceIdentifier`, `DeviceInfo`
- `crates/hypercolor-core/src/device/traits.rs` — `DeviceBackend`, `DevicePlugin`, `BackendInfo`
- `crates/hypercolor-core/src/device/discovery.rs` — `TransportScanner`, `DiscoveryOrchestrator`
- `crates/hypercolor-core/src/device/lifecycle.rs` — `DeviceLifecycleManager`, `LifecycleAction`
- `crates/hypercolor-core/src/device/manager.rs` — `BackendManager`, `OutputQueue`
- `crates/hypercolor-daemon/src/discovery.rs` — `DiscoveryRuntime`, `execute_discovery_scan()`
- `nusb` crate — USB device access for Rust (replaces hidapi dependency)
