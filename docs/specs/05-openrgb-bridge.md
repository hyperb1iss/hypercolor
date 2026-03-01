# 05 — OpenRGB Bridge

> License-isolated process bridge between Hypercolor and OpenRGB's SDK server.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [License Boundary](#2-license-boundary)
3. [OpenRGB SDK Protocol](#3-openrgb-sdk-protocol)
4. [gRPC Service Definition](#4-grpc-service-definition)
5. [Controller-to-DeviceInfo Mapping](#5-controller-to-deviceinfo-mapping)
6. [LED Update Batching](#6-led-update-batching)
7. [Auto-Detection](#7-auto-detection)
8. [Bridge Lifecycle](#8-bridge-lifecycle)
9. [Error Handling](#9-error-handling)
10. [Configuration](#10-configuration)
11. [Rust Types: Bridge Side (GPL-2.0)](#11-rust-types-bridge-side-gpl-20)
12. [Rust Types: Daemon Side (MIT/Apache-2.0)](#12-rust-types-daemon-side-mitapache-20)
13. [Complete Protobuf Definition](#13-complete-protobuf-definition)

---

## 1. Architecture Overview

The OpenRGB bridge is a **separate binary** (`hypercolor-openrgb-bridge`) that sits between the Hypercolor daemon and an OpenRGB server instance. It exists for one reason: the `openrgb2` Rust crate is GPL-2.0, and linking it into the MIT/Apache-2.0 Hypercolor daemon would contaminate the entire binary with GPL obligations.

The bridge solves this by running as an independent process, communicating with the daemon over gRPC via a Unix domain socket. The process boundary is the license firewall.

```
hypercolor-daemon (MIT/Apache-2.0)
    |
    | gRPC over Unix socket
    | /run/hypercolor/openrgb.sock
    |
    v
hypercolor-openrgb-bridge (GPL-2.0)
    |
    | OpenRGB SDK binary protocol
    | TCP port 6742
    |
    v
OpenRGB server (GPL-2.0)
    |
    v
Hardware controllers (mobo, GPU, RAM, Razer, Corsair, etc.)
```

**Binary identity:**

| Property | Value |
|---|---|
| Crate name | `hypercolor-openrgb-bridge` |
| Binary name | `hypercolor-openrgb-bridge` |
| Install path | `/usr/bin/hypercolor-openrgb-bridge` or alongside daemon |
| License | GPL-2.0-only |
| Cargo workspace | Same workspace, separate crate under `crates/bridges/` |
| Runtime | tokio async, single-threaded sufficient |
| Dependencies | `openrgb2`, `tonic` (gRPC server), `tokio`, `tracing`, `clap` |

The daemon never imports, links, or loads any GPL code. It speaks to the bridge exclusively through serialized protobuf messages over a socket -- the same mechanism used for any out-of-process gRPC plugin.

---

## 2. License Boundary

This section is the legal load-bearing wall. Get it right or the whole structure collapses.

### What is GPL-2.0

| Component | License | Location |
|---|---|---|
| `hypercolor-openrgb-bridge` binary | **GPL-2.0-only** | `crates/bridges/hypercolor-openrgb-bridge/` |
| `openrgb2` crate (dependency) | **GPL-2.0** | External crate from crates.io |
| Bridge-side gRPC server impl | **GPL-2.0** (derived work) | Same crate |
| `.proto` files (interface definition) | **MIT/Apache-2.0** | `proto/hypercolor/` (shared) |

### What is MIT/Apache-2.0

| Component | License | Location |
|---|---|---|
| `hypercolor-daemon` binary | MIT/Apache-2.0 | `crates/hypercolor-daemon/` |
| `hypercolor-core` library | MIT/Apache-2.0 | `crates/hypercolor-core/` |
| `hypercolor-bridge-sdk` library | MIT/Apache-2.0 | `crates/hypercolor-bridge-sdk/` |
| Daemon-side gRPC client (`OpenRgbBridgeBackend`) | MIT/Apache-2.0 | `crates/hypercolor-core/src/device/` |
| Protobuf definitions | MIT/Apache-2.0 | `proto/` |
| Generated protobuf Rust code | MIT/Apache-2.0 | Both sides generate independently |

### The Boundary

```
          MIT/Apache-2.0 territory          |        GPL-2.0 territory
                                            |
  hypercolor-daemon                         |    hypercolor-openrgb-bridge
  ├── hypercolor-core                       |    ├── openrgb2 (GPL dep)
  │   └── device/openrgb_bridge_client.rs   |    ├── bridge server impl
  ├── tonic gRPC client ─── Unix socket ────┼──► tonic gRPC server
  └── hypercolor-bridge-sdk (shared types)  |    └── OpenRGB SDK TCP client
                                            |
  proto/hypercolor/bridge/v1/bridge.proto   |    (same .proto, independently compiled)
  proto/hypercolor/bridge/v1/device.proto   |
```

**Key principle:** The `.proto` files define the interface contract. Both sides compile them independently. The daemon side generates Rust code under MIT/Apache-2.0. The bridge side generates Rust code that becomes part of its GPL-2.0 binary. The protobuf IDL itself is MIT/Apache-2.0 -- interface definitions are not copylightable in the same way as implementation code, and we explicitly license them permissively to enable community bridges in any language.

**What this means for users:** The bridge is an optional companion binary. Users who don't use OpenRGB never touch GPL code. The daemon ships and runs perfectly without it. When OpenRGB support is desired, the bridge is installed separately (packaged as its own `.deb`/AUR package/binary).

---

## 3. OpenRGB SDK Protocol

The OpenRGB server exposes a binary TCP protocol on port **6742** (spells "ORGB" on a phone keypad). The bridge acts as an SDK client to this server.

### 3.1 Packet Header

Every message (both directions) begins with a 16-byte header:

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|      'O'      |      'R'      |      'G'      |      'B'      |  Magic
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Device Index                           |  u32 LE
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Command ID                             |  u32 LE
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                        Payload Size                           |  u32 LE
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

- **Magic:** ASCII `"ORGB"` (4 bytes) -- packet validator
- **Device Index:** Which controller this packet targets (0-based). Set to 0 for global commands.
- **Command ID:** Operation identifier (see command table below)
- **Payload Size:** Byte length of the data following the header. Can be 0.

All multi-byte integers are **little-endian**.

### 3.2 String Encoding ("bstring")

Strings in the protocol use a length-prefixed, null-terminated format:

```
| Length (u16 LE) | UTF-8 bytes... | 0x00 |
```

The length field includes the null terminator. An empty string is encoded as `\x01\x00\x00`.

### 3.3 Color Encoding

Colors are 4 bytes: `R G B padding` (the fourth byte is unused, always 0). This means each color in an array occupies 4 bytes, not 3.

### 3.4 Command Table

| ID | Name | Direction | Payload |
|---|---|---|---|
| **0** | `REQUEST_CONTROLLER_COUNT` | Bidirectional | Request: none. Response: `u32` count |
| **1** | `REQUEST_CONTROLLER_DATA` | Bidirectional | Request: protocol version (`u32`). Response: full controller struct |
| **40** | `REQUEST_PROTOCOL_VERSION` | Bidirectional | Request: client's max version (`u32`). Response: server's version (`u32`) |
| **50** | `SET_CLIENT_NAME` | Client -> Server | Null-terminated name string (no length prefix) |
| **100** | `DEVICE_LIST_UPDATED` | Server -> Client | Notification (no payload). Re-enumerate controllers. |
| **150** | `REQUEST_PROFILE_LIST` | Bidirectional | Response: list of profile name strings |
| **151** | `REQUEST_SAVE_PROFILE` | Client -> Server | Profile name string |
| **152** | `REQUEST_LOAD_PROFILE` | Client -> Server | Profile name string |
| **153** | `REQUEST_DELETE_PROFILE` | Client -> Server | Profile name string |
| **1000** | `RGBCONTROLLER_RESIZEZONE` | Client -> Server | Zone index (`u32`) + new size (`u32`) |
| **1050** | `RGBCONTROLLER_UPDATELEDS` | Client -> Server | Data size (`u32`) + LED count (`u16`) + colors (`4 * count` bytes) |
| **1051** | `RGBCONTROLLER_UPDATEZONELEDS` | Client -> Server | Zone index (`u32`) + LED count (`u16`) + colors (`4 * count` bytes) |
| **1052** | `RGBCONTROLLER_UPDATESINGLELED` | Client -> Server | LED index (`u32`) + color (4 bytes) |
| **1100** | `RGBCONTROLLER_SETCUSTOMMODE` | Client -> Server | No payload. Switches controller to "Direct" mode. |
| **1101** | `RGBCONTROLLER_UPDATEMODE` | Client -> Server | Mode struct data |
| **1102** | `RGBCONTROLLER_SAVEMODE` | Client -> Server | Mode struct data (v3+) |

### 3.5 Protocol Versions

| Version | OpenRGB Release | Notable Changes |
|---|---|---|
| 0 | 0.3 | Initial unversioned protocol |
| 1 | 0.5 | Added `REQUEST_PROTOCOL_VERSION`, vendor string in controller data |
| 2 | 0.6 | Profile management commands (150-153) |
| 3 | 0.7+ | Brightness field in modes, `SAVEMODE` command |
| 4 | 0.9+ | Segment support, zone flags |

The bridge negotiates to the highest mutually supported version using `REQUEST_PROTOCOL_VERSION` (ID 40). The server responds with `min(client_version, server_version)`. The negotiated version determines which fields appear in controller data structures.

### 3.6 Controller Data Structure (Response to ID 1)

The `REQUEST_CONTROLLER_DATA` response is a variable-length structure:

```
Controller:
  data_size:     u32          // Total payload size
  type:          u32          // DeviceType enum (motherboard, GPU, RAM, keyboard, etc.)
  name:          bstring      // "ASUS Aura LED Controller"
  vendor:        bstring      // "ASUS" (v1+)
  description:   bstring      // Human-readable description
  version:       bstring      // Firmware/driver version
  serial:        bstring      // Serial number
  location:      bstring      // "HID: /dev/hidraw3" or "I2C: /dev/i2c-1, address 0x68"
  num_modes:     u16
  active_mode:   u32
  modes:         Mode[num_modes]
  num_zones:     u16
  zones:         Zone[num_zones]
  num_leds:      u16
  leds:          Led[num_leds]
  num_colors:    u16
  colors:        Color[num_colors]   // 4 bytes each (RGBX)

Mode:
  name:          bstring
  value:         u32          // Internal mode ID
  flags:         u32          // MODE_FLAG_HAS_SPEED, MODE_FLAG_HAS_DIRECTION, etc.
  speed_min:     u32
  speed_max:     u32
  brightness_min: u32         // v3+
  brightness_max: u32         // v3+
  colors_min:    u32
  colors_max:    u32
  speed:         u32
  brightness:    u32          // v3+
  direction:     u32          // 0 = left, 1 = right, 2 = up, 3 = down, etc.
  color_mode:    u32          // 0 = none, 1 = per-LED, 2 = mode-specific, 3 = random
  num_colors:    u16
  colors:        Color[num_colors]

Zone:
  name:          bstring      // "Mainboard", "GPU", "DRAM"
  type:          u32          // 0 = single, 1 = linear, 2 = matrix
  leds_min:      u32          // Minimum LED count (resize bounds)
  leds_max:      u32
  leds_count:    u32          // Current LED count
  matrix_len:    u16          // 0 if not a matrix
  // If matrix_len > 0:
  matrix_height: u32
  matrix_width:  u32
  matrix_data:   u32[height * width]  // LED index at each cell (-1 = empty)

Led:
  name:          bstring      // "LED 1", "Key: Escape"
  value:         u32          // HW-specific value
```

### 3.7 Commands the Bridge Uses

The bridge needs exactly these operations for Hypercolor's render loop:

1. **Connect:** TCP connect to `host:6742`
2. **Handshake:** `SET_CLIENT_NAME("Hypercolor")` + `REQUEST_PROTOCOL_VERSION(4)`
3. **Enumerate:** `REQUEST_CONTROLLER_COUNT` -> for each index, `REQUEST_CONTROLLER_DATA`
4. **Prepare:** `RGBCONTROLLER_SETCUSTOMMODE` per controller (switches to Direct mode)
5. **Stream frames:** `RGBCONTROLLER_UPDATELEDS` per controller, 60 times per second
6. **React to hot-plug:** Listen for `DEVICE_LIST_UPDATED` notifications, re-enumerate

That's it. The bridge doesn't manage profiles, modes, or zone resizing -- it treats OpenRGB as a dumb color pipe.

---

## 4. gRPC Service Definition

The bridge exposes a gRPC service over a Unix domain socket. The daemon connects as a client. This is the same `DeviceBackendPlugin` service defined in the plugin ecosystem spec (design doc 09), specialized for the OpenRGB bridge with additional RPCs for OpenRGB-specific operations.

### Service Overview

```protobuf
// Hypercolor <-> OpenRGB Bridge gRPC interface
//
// The bridge implements this service. The daemon is the client.
// Transport: Unix domain socket at /run/hypercolor/openrgb.sock
//
// License: MIT/Apache-2.0 (interface definition only)
// The bridge's implementation of this service is GPL-2.0.
// The daemon's client code is MIT/Apache-2.0.
```

**RPCs:**

| RPC | Purpose | Frequency |
|---|---|---|
| `GetBridgeInfo` | Bridge metadata + OpenRGB connection status | Once on connect |
| `Discover` | Enumerate all OpenRGB controllers as `DeviceInfo` | On startup + hot-plug |
| `Connect` | Enable a specific controller for frame streaming | Once per device |
| `PushFrame` | Send LED colors to a single controller | 60 fps per controller |
| `PushFrameBatch` | Send LED colors to multiple controllers in one call | 60 fps (preferred) |
| `Disconnect` | Release a controller | On device removal |
| `SubscribeEvents` | Server-streaming RPC for hot-plug / disconnect events | Persistent stream |
| `GetControllerDetail` | Full OpenRGB controller data (modes, zones, segments) | On demand |
| `Shutdown` | Graceful bridge shutdown | Once |
| `HealthCheck` | Liveness probe | Periodic (5s) |

The complete `.proto` definition is in [Section 13](#13-complete-protobuf-definition).

---

## 5. Controller-to-DeviceInfo Mapping

The bridge translates OpenRGB's controller data model into Hypercolor's `DeviceInfo` abstraction. This mapping preserves the zone/LED topology that the spatial layout engine needs.

### Mapping Rules

```
OpenRGB Controller          -->  Hypercolor DeviceInfo
─────────────────────────        ─────────────────────
controller.name             -->  device_info.name
controller.vendor           -->  device_info.manufacturer
controller.type             -->  device_info.device_type (mapped enum)
controller.serial           -->  device_info.serial
controller.location         -->  device_info.location
"openrgb:{index}"           -->  device_info.id (stable identifier)
sum(zone.leds_count)        -->  device_info.total_led_count
controller.zones[]          -->  device_info.zones[]
```

### Zone Mapping

```
OpenRGB Zone                -->  Hypercolor ZoneInfo
────────────                     ────────────────────
zone.name                   -->  zone_info.name
zone.leds_count             -->  zone_info.led_count
zone.type == 0 (single)     -->  zone_info.topology = Custom (1 LED)
zone.type == 1 (linear)     -->  zone_info.topology = Strip
zone.type == 2 (matrix)     -->  zone_info.topology = Matrix { width, height }
zone.leds_min/max           -->  zone_info.resizable (min != max)
```

### LED Mapping

LEDs in OpenRGB are a flat array across the entire controller. Zones partition this array: zone 0 owns LEDs `[0..zone_0.leds_count)`, zone 1 owns `[zone_0.leds_count..zone_0.leds_count + zone_1.leds_count)`, and so on.

When Hypercolor pushes colors per-zone, the bridge reconstructs the flat LED array:

```
Zone "Mainboard"  (4 LEDs):  [R,G,B,W]
Zone "GPU"        (3 LEDs):  [C,M,Y]
Zone "DRAM"       (4 LEDs):  [A,B,C,D]
                                |
                    Bridge assembles flat array:
                                |
                                v
RGBCONTROLLER_UPDATELEDS: [R,G,B,W,C,M,Y,A,B,C,D]  (11 LEDs)
```

### Device Type Mapping

| OpenRGB `device_type` | Value | Hypercolor `DeviceType` |
|---|---|---|
| `DEVICE_TYPE_MOTHERBOARD` | 0 | `Motherboard` |
| `DEVICE_TYPE_DRAM` | 1 | `Memory` |
| `DEVICE_TYPE_GPU` | 2 | `Gpu` |
| `DEVICE_TYPE_COOLER` | 3 | `Cooler` |
| `DEVICE_TYPE_LEDSTRIP` | 4 | `LedStrip` |
| `DEVICE_TYPE_KEYBOARD` | 5 | `Keyboard` |
| `DEVICE_TYPE_MOUSE` | 6 | `Mouse` |
| `DEVICE_TYPE_MOUSEMAT` | 7 | `Mousepad` |
| `DEVICE_TYPE_HEADSET` | 8 | `Headset` |
| `DEVICE_TYPE_HEADSET_STAND` | 9 | `HeadsetStand` |
| `DEVICE_TYPE_GAMEPAD` | 10 | `Gamepad` |
| `DEVICE_TYPE_LIGHT` | 11 | `Light` |
| `DEVICE_TYPE_SPEAKER` | 12 | `Speaker` |
| `DEVICE_TYPE_VIRTUAL` | 13 | `Virtual` |
| `DEVICE_TYPE_STORAGE` | 14 | `Storage` |
| `DEVICE_TYPE_CASE` | 15 | `Case` |
| `DEVICE_TYPE_MICROPHONE` | 16 | `Microphone` |
| Other / unknown | * | `Unknown` |

---

## 6. LED Update Batching

### The Problem

At 60 fps, the daemon calls `PushFrame` for each controller. With 8 OpenRGB controllers, that's 8 gRPC round-trips per frame (~0.5ms each = 4ms of IPC overhead). Add the actual SDK `UPDATELEDS` call per controller, and we're eating a significant chunk of the 16.6ms frame budget.

### The Solution: `PushFrameBatch`

The `PushFrameBatch` RPC accepts colors for **multiple controllers** in a single gRPC call. The bridge processes them sequentially on the OpenRGB SDK side (TCP is serial anyway), but the daemon pays only one IPC round-trip.

```
Daemon                          Bridge                      OpenRGB
  |                               |                           |
  |-- PushFrameBatch ------------>|                           |
  |   [ctrl_0: 120 LEDs,         |-- UPDATELEDS(0, 120) ---->|
  |    ctrl_1: 48 LEDs,          |<-- ack -------------------|
  |    ctrl_2: 16 LEDs]          |-- UPDATELEDS(1, 48) ----->|
  |                              |<-- ack -------------------|
  |                              |-- UPDATELEDS(2, 16) ----->|
  |                              |<-- ack -------------------|
  |<-- BatchResponse ------------|                           |
  |   (all succeeded)            |                           |
```

**Latency comparison:**

| Strategy | 8 controllers | IPC overhead |
|---|---|---|
| Individual `PushFrame` calls | 8 round-trips | ~4ms |
| `PushFrameBatch` | 1 round-trip | ~0.5ms |

### Bridge-Side SDK Batching

The `openrgb2` crate supports command batching natively via its `Command`/`CommandGroup` API. The bridge leverages this to send all controller updates through a single TCP write where possible:

```rust
// Bridge-side pseudo-code (GPL-2.0)
async fn handle_batch(&self, batch: PushFrameBatchRequest) -> Result<()> {
    // Build a CommandGroup for all controllers in the batch
    for entry in &batch.entries {
        let controller = self.controllers.get(&entry.device_id)?;
        let colors = decode_rgb_bytes(&entry.rgb_data);
        controller.set_leds(&colors);
    }

    // Execute all pending commands in one async call
    self.client.execute().await?;
    Ok(())
}
```

### Zone-Level Updates

When Hypercolor has per-zone color data (e.g., only one zone changed), the bridge can use `RGBCONTROLLER_UPDATEZONELEDS` (SDK command 1051) instead of updating the entire controller. The gRPC message includes optional zone targeting:

```protobuf
message FrameEntry {
    string device_id = 1;
    bytes rgb_data = 2;
    uint32 led_count = 3;
    optional string zone_name = 4;  // If set, update only this zone
}
```

---

## 7. Auto-Detection

The bridge needs to find a running OpenRGB instance without user configuration wherever possible.

### Detection Strategy (ordered)

1. **Configuration file** -- If `openrgb_host` and `openrgb_port` are set in bridge config, use those directly. No probing.

2. **Default local** -- Attempt TCP connect to `127.0.0.1:6742`. This covers the most common case: OpenRGB running on the same machine with default settings.

3. **OpenRGB config file** -- Parse `~/.config/OpenRGB/OpenRGB.json` to discover if a non-default port is configured.

4. **D-Bus** -- Query the system D-Bus for a running OpenRGB service. OpenRGB registers as `org.openrgb.OpenRGB` when launched with `--server`.

5. **Process scan** -- Check if an `openrgb` process is running (via `/proc` on Linux). If found but the SDK server isn't responding, the bridge logs a warning suggesting the user enable the SDK server (`openrgb --server`).

### Connection Flow

```
Bridge starts
    |
    v
Config has explicit host:port? --yes--> Connect to configured address
    |
    no
    v
TCP probe 127.0.0.1:6742 --success--> Connected
    |
    fail
    v
Parse ~/.config/OpenRGB/OpenRGB.json for custom port --found--> TCP probe
    |
    not found or fail
    v
Check D-Bus for org.openrgb.OpenRGB --found--> Query for SDK port --> TCP probe
    |
    not found
    v
Check if openrgb process exists --yes--> Log: "OpenRGB running but SDK server
    |                                       not enabled. Start with --server"
    no
    v
Log: "OpenRGB not detected. Waiting for connection..."
Enter retry loop (exponential backoff: 1s, 2s, 4s, 8s, max 30s)
```

### Auto-Launch (Optional, Off by Default)

If configured, the bridge can attempt to launch OpenRGB with the SDK server enabled:

```toml
[openrgb]
auto_launch = true
launch_command = "openrgb --server --server-port 6742"
launch_timeout_ms = 10000
```

This is **disabled by default** because launching OpenRGB touches hardware and should be an explicit user decision.

---

## 8. Bridge Lifecycle

### 8.1 Startup Sequence

```
 1. Parse CLI args and config file
 2. Initialize tracing/logging
 3. Bind gRPC server to Unix socket (/run/hypercolor/openrgb.sock)
 4. Signal readiness (stdout: "READY", or systemd notify)
 5. Auto-detect OpenRGB instance (Section 7)
 6. If found:
    a. TCP connect to OpenRGB SDK
    b. SET_CLIENT_NAME("Hypercolor OpenRGB Bridge")
    c. REQUEST_PROTOCOL_VERSION(4) -- negotiate to highest mutual version
    d. REQUEST_CONTROLLER_COUNT
    e. For each controller: REQUEST_CONTROLLER_DATA
    f. RGBCONTROLLER_SETCUSTOMMODE for each controller (enter Direct mode)
    g. Cache controller data, build DeviceInfo mappings
    h. Emit BridgeEvent::Connected to all SubscribeEvents streams
 7. If not found:
    a. Enter retry loop (see auto-detection)
    b. Bridge is "alive" on gRPC but reports no devices
```

### 8.2 Steady-State Operation

```
 Loop (per gRPC request from daemon):
   PushFrame/PushFrameBatch:
     1. Look up controller by device_id
     2. Translate Hypercolor RGB bytes to OpenRGB color array
     3. Send RGBCONTROLLER_UPDATELEDS (or UPDATEZONELEDS)
     4. Return success/error

 Background task:
   1. Listen for DEVICE_LIST_UPDATED from OpenRGB (command 100)
   2. On notification: re-enumerate controllers
   3. Diff against cached controller list
   4. Emit BridgeEvent::DeviceAdded / DeviceRemoved to subscribers
   5. Update internal state
```

### 8.3 Shutdown Sequence

```
 1. Receive Shutdown RPC (or SIGTERM/SIGINT)
 2. Stop accepting new gRPC requests
 3. Drain in-flight PushFrame calls (max 100ms grace period)
 4. For each controller in Direct mode:
    - Optionally restore previous mode (if configured)
    - Or leave LEDs at last color (default, less disruptive)
 5. Close TCP connection to OpenRGB
 6. Close Unix socket
 7. Exit cleanly
```

### 8.4 Daemon-Managed Lifecycle

The Hypercolor daemon's `BridgeManager` handles the bridge process:

```toml
# ~/.config/hypercolor/config.toml

[[bridge]]
id = "openrgb"
command = "hypercolor-openrgb-bridge"
args = ["--config", "/etc/hypercolor/openrgb-bridge.toml"]
socket = "/run/hypercolor/openrgb.sock"
restart_policy = "always"       # "always" | "on-failure" | "never"
restart_delay_ms = 1000         # Initial delay, doubles on consecutive failures
max_restarts = 10               # 0 = unlimited
health_interval_ms = 5000       # How often to call HealthCheck
startup_timeout_ms = 10000      # Time to wait for READY signal
```

The daemon:
- Spawns the bridge process on startup
- Waits for the `READY` signal before connecting the gRPC client
- Monitors via periodic `HealthCheck` RPCs
- Restarts with exponential backoff on crash (1s, 2s, 4s, ... capped at 30s)
- Logs all bridge lifecycle events to the Hypercolor event bus
- Surfaces bridge status in the web UI and TUI device panels

---

## 9. Error Handling

### 9.1 Error Categories

| Category | Symptoms | Bridge Behavior |
|---|---|---|
| **OpenRGB not running** | TCP connect refused | Retry with backoff. Report `NoConnection` to daemon. |
| **Connection lost** | TCP read/write returns error | Reconnect with backoff. Emit `DeviceDisconnected` for all controllers. |
| **Controller hot-unplug** | `DEVICE_LIST_UPDATED` received | Re-enumerate. Emit `DeviceRemoved` for missing controllers. |
| **Controller hot-plug** | `DEVICE_LIST_UPDATED` received | Re-enumerate. Emit `DeviceAdded` for new controllers. `SETCUSTOMMODE` on new controller. |
| **LED update rejected** | SDK returns error | Log warning. Return error in gRPC response. Daemon skips this device for the frame. |
| **Protocol mismatch** | Version negotiation fails | Fall back to protocol v0. Log warning if critical features unavailable. |
| **Bridge OOM / panic** | Process exits | Daemon's `BridgeManager` restarts it. Devices show as disconnected until reconnect. |
| **Socket permission error** | Bind fails | Exit with clear error message. Systemd `RuntimeDirectory=hypercolor` solves this. |

### 9.2 Reconnection Strategy

```rust
// Bridge-side reconnection logic (GPL-2.0)
pub struct ReconnectionPolicy {
    pub initial_delay: Duration,     // 1 second
    pub max_delay: Duration,         // 30 seconds
    pub backoff_factor: f64,         // 2.0
    pub max_attempts: Option<u32>,   // None = unlimited
    pub jitter: bool,                // true (prevents thundering herd)
}

impl ReconnectionPolicy {
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base = self.initial_delay.as_secs_f64() * self.backoff_factor.powi(attempt as i32);
        let clamped = base.min(self.max_delay.as_secs_f64());
        if self.jitter {
            Duration::from_secs_f64(clamped * rand::thread_rng().gen_range(0.5..1.0))
        } else {
            Duration::from_secs_f64(clamped)
        }
    }
}
```

### 9.3 Graceful Degradation

When the OpenRGB connection drops mid-frame:

1. The current `PushFrame`/`PushFrameBatch` returns `Err(ConnectionLost)`
2. The daemon marks all OpenRGB devices as `Disconnected`
3. The render loop continues -- non-OpenRGB devices (WLED, HID) keep receiving frames
4. The bridge enters reconnection mode
5. On reconnection, the bridge re-enumerates and emits `DeviceAdded` events
6. The daemon automatically reconnects and resumes frame pushing

No user intervention required. The UI shows a transient "OpenRGB: reconnecting..." status that clears on recovery.

---

## 10. Configuration

### 10.1 Bridge Configuration File

Location: `~/.config/hypercolor/openrgb-bridge.toml` (or specified via `--config`)

```toml
# OpenRGB Bridge Configuration

[openrgb]
# OpenRGB server address. Default: auto-detect (see Section 7).
# host = "127.0.0.1"
# port = 6742

# Client name sent to OpenRGB (appears in OpenRGB's SDK client list)
client_name = "Hypercolor"

# Protocol version to request. Bridge negotiates down if server is older.
protocol_version = 4

# Whether to auto-launch OpenRGB if not detected. Default: false.
auto_launch = false
# launch_command = "openrgb --server --server-port 6742"
# launch_timeout_ms = 10000

[bridge]
# Unix socket path for gRPC server.
# Default: /run/hypercolor/openrgb.sock
socket = "/run/hypercolor/openrgb.sock"

# Log level for the bridge process.
# Values: trace, debug, info, warn, error
log_level = "info"

[reconnection]
# Initial delay before first reconnection attempt.
initial_delay_ms = 1000

# Maximum delay between reconnection attempts.
max_delay_ms = 30000

# Backoff multiplier.
backoff_factor = 2.0

# Maximum reconnection attempts (0 = unlimited).
max_attempts = 0

# Add random jitter to prevent thundering herd.
jitter = true

[behavior]
# What to do with controllers on bridge shutdown.
# "hold" = leave LEDs at last color (default)
# "restore" = restore the mode that was active before the bridge took over
# "off" = turn all LEDs off
on_shutdown = "hold"

# Automatically switch controllers to Direct/Custom mode on discovery.
# If false, the daemon must explicitly call Connect(device_id) first.
auto_direct_mode = true

# Filter which controllers the bridge exposes to the daemon.
# Empty = expose all. Supports glob patterns.
# include_controllers = ["*ASUS*", "*Razer*"]
# exclude_controllers = ["*Virtual*"]
```

### 10.2 CLI Arguments

```
hypercolor-openrgb-bridge [OPTIONS]

Options:
    --config <PATH>         Path to config file [default: ~/.config/hypercolor/openrgb-bridge.toml]
    --socket <PATH>         Unix socket path [overrides config]
    --openrgb-host <HOST>   OpenRGB server host [overrides config]
    --openrgb-port <PORT>   OpenRGB server port [overrides config]
    --log-level <LEVEL>     Log level [overrides config]
    --version               Print version and exit
    --help                  Print help
```

CLI arguments override config file values. Config file values override defaults.

---

## 11. Rust Types: Bridge Side (GPL-2.0)

These types live in the `hypercolor-openrgb-bridge` crate. They are GPL-2.0 because they depend on `openrgb2`.

```rust
//! hypercolor-openrgb-bridge -- GPL-2.0-only
//!
//! This binary links against `openrgb2` (GPL-2.0) and is therefore
//! licensed under GPL-2.0-only. It communicates with the Hypercolor
//! daemon exclusively via gRPC over a Unix domain socket.

use openrgb2::{OpenRgbClient, data::Controller, data::Color as OrgbColor};
use tonic::{Request, Response, Status};
use tokio::sync::{broadcast, RwLock};
use std::collections::HashMap;
use std::sync::Arc;

/// Top-level bridge state, shared across gRPC handlers.
pub struct OpenRgbBridge {
    /// Connection to the OpenRGB SDK server.
    client: Arc<RwLock<Option<OpenRgbClient>>>,

    /// Cached controller data, keyed by "openrgb:{index}".
    controllers: Arc<RwLock<HashMap<String, CachedController>>>,

    /// Event broadcast for hot-plug notifications.
    events: broadcast::Sender<BridgeEvent>,

    /// Bridge configuration.
    config: BridgeConfig,

    /// Reconnection state.
    reconnect: Arc<RwLock<ReconnectionState>>,
}

/// Cached controller with its OpenRGB index and translated DeviceInfo.
pub struct CachedController {
    /// OpenRGB controller index (0-based).
    pub index: u32,

    /// Raw controller data from OpenRGB.
    pub controller: Controller,

    /// Pre-computed zone LED offset table.
    /// zone_offsets[i] = starting LED index for zone i in the flat LED array.
    pub zone_offsets: Vec<u32>,

    /// Whether this controller is in Direct/Custom mode.
    pub direct_mode_active: bool,

    /// Negotiated protocol version for this session.
    pub protocol_version: u32,
}

/// Events emitted by the bridge to gRPC subscribers.
#[derive(Clone, Debug)]
pub enum BridgeEvent {
    /// OpenRGB server connection established.
    Connected { protocol_version: u32, controller_count: u32 },

    /// OpenRGB server connection lost.
    Disconnected { reason: String },

    /// New controller appeared (hot-plug).
    DeviceAdded { device_id: String, name: String, led_count: u32 },

    /// Controller disappeared (hot-unplug).
    DeviceRemoved { device_id: String, name: String },

    /// Controller data changed (mode/zone change from another SDK client).
    DeviceUpdated { device_id: String },
}

/// Reconnection state machine.
pub struct ReconnectionState {
    pub attempt: u32,
    pub last_attempt: Option<tokio::time::Instant>,
    pub policy: ReconnectionPolicy,
    pub connected: bool,
}

/// Configuration parsed from TOML.
#[derive(Clone, Debug)]
pub struct BridgeConfig {
    pub openrgb_host: Option<String>,
    pub openrgb_port: Option<u16>,
    pub client_name: String,
    pub protocol_version: u32,
    pub auto_launch: bool,
    pub launch_command: Option<String>,
    pub socket_path: String,
    pub log_level: String,
    pub reconnection: ReconnectionPolicy,
    pub on_shutdown: ShutdownBehavior,
    pub auto_direct_mode: bool,
    pub include_controllers: Vec<String>,
    pub exclude_controllers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ReconnectionPolicy {
    pub initial_delay: std::time::Duration,
    pub max_delay: std::time::Duration,
    pub backoff_factor: f64,
    pub max_attempts: u32, // 0 = unlimited
    pub jitter: bool,
}

#[derive(Clone, Debug)]
pub enum ShutdownBehavior {
    Hold,    // Leave LEDs at last color
    Restore, // Restore previous mode
    Off,     // Turn all LEDs off
}

impl OpenRgbBridge {
    /// Connect to OpenRGB and enumerate controllers.
    pub async fn connect(&self) -> Result<(), BridgeError> {
        let (host, port) = self.resolve_openrgb_address().await?;
        let client = OpenRgbClient::connect(&host, port).await?;

        // Handshake
        client.set_client_name(&self.config.client_name).await?;
        let version = client.get_protocol_version(self.config.protocol_version).await?;

        // Enumerate
        let count = client.get_controller_count().await?;
        let mut controllers = HashMap::new();

        for i in 0..count {
            let ctrl = client.get_controller(i).await?;

            // Compute zone offsets
            let mut offsets = Vec::with_capacity(ctrl.zones.len());
            let mut offset = 0u32;
            for zone in &ctrl.zones {
                offsets.push(offset);
                offset += zone.leds_count;
            }

            let device_id = format!("openrgb:{i}");

            // Apply include/exclude filters
            if !self.matches_filter(&ctrl.name) {
                continue;
            }

            // Switch to Direct mode
            if self.config.auto_direct_mode {
                client.set_custom_mode(i).await?;
            }

            controllers.insert(device_id.clone(), CachedController {
                index: i,
                controller: ctrl,
                zone_offsets: offsets,
                direct_mode_active: self.config.auto_direct_mode,
                protocol_version: version,
            });
        }

        *self.client.write().await = Some(client);
        *self.controllers.write().await = controllers;

        let _ = self.events.send(BridgeEvent::Connected {
            protocol_version: version,
            controller_count: count,
        });

        Ok(())
    }

    /// Push a frame of LED colors to a single controller.
    pub async fn push_frame(
        &self,
        device_id: &str,
        rgb_data: &[u8],
        zone_name: Option<&str>,
    ) -> Result<(), BridgeError> {
        let client = self.client.read().await;
        let client = client.as_ref().ok_or(BridgeError::NotConnected)?;
        let controllers = self.controllers.read().await;
        let cached = controllers.get(device_id)
            .ok_or(BridgeError::DeviceNotFound(device_id.to_string()))?;

        // Convert packed RGB bytes to OpenRGB colors
        let colors: Vec<OrgbColor> = rgb_data
            .chunks_exact(3)
            .map(|c| OrgbColor { r: c[0], g: c[1], b: c[2] })
            .collect();

        match zone_name {
            Some(name) => {
                // Find zone index and update just that zone
                let zone_idx = cached.controller.zones.iter()
                    .position(|z| z.name == name)
                    .ok_or(BridgeError::ZoneNotFound(name.to_string()))?;
                client.update_zone_leds(cached.index, zone_idx as u32, &colors).await?;
            }
            None => {
                // Update all LEDs on the controller
                client.update_leds(cached.index, &colors).await?;
            }
        }

        Ok(())
    }

    /// Listen for DEVICE_LIST_UPDATED notifications and re-enumerate.
    pub async fn watch_hot_plug(&self) {
        loop {
            // The openrgb2 client emits events when it receives
            // DEVICE_LIST_UPDATED (command 100) from the server
            let notification = {
                let client = self.client.read().await;
                match client.as_ref() {
                    Some(c) => c.wait_for_device_update().await,
                    None => {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                }
            };

            if notification.is_ok() {
                // Re-enumerate and diff
                let _ = self.re_enumerate().await;
            }
        }
    }
}

/// Bridge-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Not connected to OpenRGB")]
    NotConnected,

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Zone not found: {0}")]
    ZoneNotFound(String),

    #[error("OpenRGB SDK error: {0}")]
    SdkError(#[from] openrgb2::OpenRgbError),

    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Protocol version {0} not supported (server supports up to {1})")]
    ProtocolMismatch(u32, u32),
}
```

---

## 12. Rust Types: Daemon Side (MIT/Apache-2.0)

These types live in `hypercolor-core` and `hypercolor-bridge-sdk`. They have **zero** GPL dependencies.

```rust
//! hypercolor-core/src/device/openrgb_bridge_client.rs
//! License: MIT/Apache-2.0
//!
//! gRPC client that speaks to the OpenRGB bridge process.
//! Implements the DeviceBackend trait so the render loop
//! doesn't know or care that OpenRGB lives in another process.

use crate::device::traits::{DeviceBackend, DeviceInfo, DeviceHandle, ZoneInfo, ZoneTopology};
use hypercolor_bridge_sdk::proto::bridge::v1::{
    bridge_service_client::BridgeServiceClient,
    DiscoverRequest, ConnectRequest, PushFrameRequest,
    PushFrameBatchRequest, FrameEntry, SubscribeEventsRequest,
};
use tonic::transport::{Channel, Endpoint, Uri};
use tokio::sync::watch;
use tower::service_fn;

/// DeviceBackend implementation that delegates to the OpenRGB bridge process.
pub struct OpenRgbBridgeBackend {
    /// gRPC client channel to the bridge.
    client: BridgeServiceClient<Channel>,

    /// Cached device info from the last Discover call.
    devices: Vec<DeviceInfo>,

    /// Bridge connection status.
    status: watch::Receiver<BridgeStatus>,
}

#[derive(Clone, Debug)]
pub enum BridgeStatus {
    /// Bridge process not running.
    NotRunning,

    /// Bridge running but OpenRGB not connected.
    Disconnected,

    /// Bridge connected to OpenRGB, N controllers available.
    Connected { controller_count: u32 },

    /// Bridge reconnecting to OpenRGB.
    Reconnecting { attempt: u32 },
}

impl OpenRgbBridgeBackend {
    /// Connect to the bridge over a Unix domain socket.
    pub async fn connect(socket_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Connect to Unix socket via tonic
        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| {
                tokio::net::UnixStream::connect(socket_path.to_string())
            }))
            .await?;

        let client = BridgeServiceClient::new(channel);

        let (status_tx, status_rx) = watch::channel(BridgeStatus::NotRunning);

        let mut backend = Self {
            client,
            devices: Vec::new(),
            status: status_rx,
        };

        // Start event subscription in background
        backend.subscribe_events(status_tx).await;

        Ok(backend)
    }

    /// Subscribe to bridge events and update status.
    async fn subscribe_events(&mut self, status_tx: watch::Sender<BridgeStatus>) {
        let mut client = self.client.clone();
        tokio::spawn(async move {
            let stream = client
                .subscribe_events(SubscribeEventsRequest {})
                .await;

            if let Ok(response) = stream {
                let mut stream = response.into_inner();
                while let Ok(Some(event)) = stream.message().await {
                    match event.event_type.as_str() {
                        "connected" => {
                            let _ = status_tx.send(BridgeStatus::Connected {
                                controller_count: event.controller_count,
                            });
                        }
                        "disconnected" => {
                            let _ = status_tx.send(BridgeStatus::Disconnected);
                        }
                        "device_added" | "device_removed" => {
                            // Daemon should re-discover
                        }
                        _ => {}
                    }
                }
            }
        });
    }
}

#[async_trait::async_trait]
impl DeviceBackend for OpenRgbBridgeBackend {
    fn name(&self) -> &str {
        "OpenRGB (Bridge)"
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>, crate::Error> {
        let response = self.client
            .discover(DiscoverRequest {})
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenRGB bridge: {e}")))?;

        let devices: Vec<DeviceInfo> = response.into_inner().devices
            .into_iter()
            .map(|d| DeviceInfo {
                id: d.id,
                name: d.name,
                manufacturer: d.manufacturer,
                device_type: map_device_type(d.device_type),
                total_led_count: d.total_led_count,
                zones: d.zones.into_iter().map(|z| ZoneInfo {
                    name: z.name,
                    led_count: z.led_count,
                    topology: match z.topology.as_str() {
                        "strip" => ZoneTopology::Strip { count: z.led_count },
                        "matrix" => ZoneTopology::Matrix {
                            width: z.matrix_width,
                            height: z.matrix_height,
                        },
                        "ring" => ZoneTopology::Ring { count: z.led_count },
                        _ => ZoneTopology::Custom,
                    },
                    resizable: z.resizable,
                }).collect(),
                serial: d.serial,
                location: d.location,
                backend: "openrgb".to_string(),
            })
            .collect();

        self.devices = devices.clone();
        Ok(devices)
    }

    async fn connect(&mut self, device: &DeviceInfo) -> Result<DeviceHandle, crate::Error> {
        let response = self.client
            .connect(ConnectRequest {
                device_id: device.id.clone(),
            })
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenRGB bridge: {e}")))?;

        Ok(DeviceHandle {
            id: device.id.clone(),
            connected: response.into_inner().success,
        })
    }

    async fn push_frame(
        &mut self,
        handle: &DeviceHandle,
        colors: &[rgb::RGB8],
    ) -> Result<(), crate::Error> {
        // Pack RGB into bytes (3 bytes per LED)
        let rgb_data: Vec<u8> = colors.iter()
            .flat_map(|c| [c.r, c.g, c.b])
            .collect();

        self.client
            .push_frame(PushFrameRequest {
                device_id: handle.id.clone(),
                rgb_data,
                led_count: colors.len() as u32,
                zone_name: None,
            })
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenRGB bridge: {e}")))?;

        Ok(())
    }

    async fn push_frame_batch(
        &mut self,
        frames: &[(DeviceHandle, Vec<rgb::RGB8>)],
    ) -> Result<(), crate::Error> {
        let entries: Vec<FrameEntry> = frames.iter()
            .map(|(handle, colors)| FrameEntry {
                device_id: handle.id.clone(),
                rgb_data: colors.iter().flat_map(|c| [c.r, c.g, c.b]).collect(),
                led_count: colors.len() as u32,
                zone_name: None,
            })
            .collect();

        self.client
            .push_frame_batch(PushFrameBatchRequest { entries })
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenRGB bridge: {e}")))?;

        Ok(())
    }

    async fn disconnect(&mut self, handle: DeviceHandle) -> Result<(), crate::Error> {
        self.client
            .disconnect(hypercolor_bridge_sdk::proto::bridge::v1::DisconnectRequest {
                device_id: handle.id,
            })
            .await
            .map_err(|e| crate::Error::Backend(format!("OpenRGB bridge: {e}")))?;

        Ok(())
    }
}

/// Map protobuf device type string to Hypercolor's DeviceType enum.
fn map_device_type(proto_type: String) -> crate::device::DeviceType {
    match proto_type.as_str() {
        "motherboard" => crate::device::DeviceType::Motherboard,
        "memory" => crate::device::DeviceType::Memory,
        "gpu" => crate::device::DeviceType::Gpu,
        "cooler" => crate::device::DeviceType::Cooler,
        "led_strip" => crate::device::DeviceType::LedStrip,
        "keyboard" => crate::device::DeviceType::Keyboard,
        "mouse" => crate::device::DeviceType::Mouse,
        "mousepad" => crate::device::DeviceType::Mousepad,
        "headset" => crate::device::DeviceType::Headset,
        "headset_stand" => crate::device::DeviceType::HeadsetStand,
        "gamepad" => crate::device::DeviceType::Gamepad,
        "light" => crate::device::DeviceType::Light,
        "speaker" => crate::device::DeviceType::Speaker,
        "virtual" => crate::device::DeviceType::Virtual,
        "storage" => crate::device::DeviceType::Storage,
        "case" => crate::device::DeviceType::Case,
        "microphone" => crate::device::DeviceType::Microphone,
        _ => crate::device::DeviceType::Unknown,
    }
}
```

---

## 13. Complete Protobuf Definition

This is the authoritative `.proto` file for the bridge. It lives at `proto/hypercolor/bridge/v1/bridge.proto` and is compiled independently by both the bridge (GPL-2.0 binary) and the daemon (MIT/Apache-2.0 binary).

```protobuf
// proto/hypercolor/bridge/v1/bridge.proto
//
// Hypercolor OpenRGB Bridge gRPC Service Definition
//
// License: MIT/Apache-2.0 (this interface definition only)
//
// This .proto file defines the contract between the Hypercolor daemon
// and the OpenRGB bridge process. Both sides compile it independently.
// The daemon-side generated code is MIT/Apache-2.0.
// The bridge-side generated code is part of the GPL-2.0 bridge binary.

syntax = "proto3";

package hypercolor.bridge.v1;

option java_package = "dev.hypercolor.bridge.v1";
option go_package = "github.com/hyperbliss/hypercolor/bridge/v1";

// ─────────────────────────────────────────────────────────────
// Service
// ─────────────────────────────────────────────────────────────

service BridgeService {
    // ── Metadata ──

    // Returns bridge metadata and OpenRGB connection status.
    rpc GetBridgeInfo(GetBridgeInfoRequest) returns (GetBridgeInfoResponse);

    // ── Device Lifecycle ──

    // Enumerate all OpenRGB controllers as Hypercolor DeviceInfo.
    // Returns the current snapshot. Call again after a DeviceAdded/Removed event.
    rpc Discover(DiscoverRequest) returns (DiscoverResponse);

    // Enable a controller for frame streaming.
    // The bridge switches the controller to Direct/Custom mode.
    rpc Connect(ConnectRequest) returns (ConnectResponse);

    // Release a controller. Optionally restore its previous mode.
    rpc Disconnect(DisconnectRequest) returns (DisconnectResponse);

    // ── Frame Streaming ──

    // Push LED colors to a single controller. Called at 60fps per device.
    rpc PushFrame(PushFrameRequest) returns (PushFrameResponse);

    // Push LED colors to multiple controllers in a single round-trip.
    // Preferred over individual PushFrame calls for multi-device setups.
    rpc PushFrameBatch(PushFrameBatchRequest) returns (PushFrameBatchResponse);

    // ── Events ──

    // Server-streaming RPC. The bridge pushes events when:
    //   - OpenRGB connection state changes (connected/disconnected)
    //   - Controllers are added or removed (hot-plug)
    //   - Controller data changes (mode/zone updates from other clients)
    rpc SubscribeEvents(SubscribeEventsRequest) returns (stream BridgeEvent);

    // ── Diagnostics ──

    // Return full OpenRGB controller data for a single device.
    // Used for the web UI's device detail panel, not for the render loop.
    rpc GetControllerDetail(GetControllerDetailRequest) returns (GetControllerDetailResponse);

    // Health check. Returns current bridge and OpenRGB connection status.
    rpc HealthCheck(HealthCheckRequest) returns (HealthCheckResponse);

    // ── Lifecycle ──

    // Graceful shutdown. The bridge drains in-flight requests and exits.
    rpc Shutdown(ShutdownRequest) returns (ShutdownResponse);
}

// ─────────────────────────────────────────────────────────────
// Bridge Info
// ─────────────────────────────────────────────────────────────

message GetBridgeInfoRequest {}

message GetBridgeInfoResponse {
    // Bridge version (semver).
    string version = 1;

    // OpenRGB connection status.
    ConnectionStatus openrgb_status = 2;

    // OpenRGB server version string (if connected).
    string openrgb_version = 3;

    // Negotiated protocol version (0-4).
    uint32 protocol_version = 4;

    // Number of controllers currently available.
    uint32 controller_count = 5;

    // OpenRGB server address.
    string openrgb_host = 6;
    uint32 openrgb_port = 7;
}

enum ConnectionStatus {
    CONNECTION_STATUS_UNSPECIFIED = 0;
    CONNECTION_STATUS_CONNECTED = 1;
    CONNECTION_STATUS_DISCONNECTED = 2;
    CONNECTION_STATUS_RECONNECTING = 3;
    CONNECTION_STATUS_ERROR = 4;
}

// ─────────────────────────────────────────────────────────────
// Device Discovery
// ─────────────────────────────────────────────────────────────

message DiscoverRequest {}

message DiscoverResponse {
    repeated DeviceInfo devices = 1;
}

message DeviceInfo {
    // Stable identifier: "openrgb:{controller_index}"
    string id = 1;

    // Human-readable name from OpenRGB (e.g., "ASUS Aura LED Controller").
    string name = 2;

    // Manufacturer/vendor string.
    string manufacturer = 3;

    // Device category (see DeviceType enum).
    string device_type = 4;

    // Total LED count across all zones.
    uint32 total_led_count = 5;

    // Zone breakdown.
    repeated ZoneInfo zones = 6;

    // Hardware serial number (may be empty).
    string serial = 7;

    // OpenRGB location string (e.g., "HID: /dev/hidraw3").
    string location = 8;

    // OpenRGB controller index (for diagnostics).
    uint32 controller_index = 9;
}

message ZoneInfo {
    // Zone name from OpenRGB (e.g., "Mainboard", "GPU Backplate").
    string name = 1;

    // Number of LEDs in this zone.
    uint32 led_count = 2;

    // Topology hint for the spatial layout engine.
    // Values: "strip", "matrix", "ring", "single", "custom"
    string topology = 3;

    // Matrix dimensions (only meaningful when topology == "matrix").
    uint32 matrix_width = 4;
    uint32 matrix_height = 5;

    // Whether this zone supports LED count resizing.
    bool resizable = 6;

    // Resize bounds (only meaningful when resizable == true).
    uint32 leds_min = 7;
    uint32 leds_max = 8;
}

// ─────────────────────────────────────────────────────────────
// Device Connection
// ─────────────────────────────────────────────────────────────

message ConnectRequest {
    string device_id = 1;
}

message ConnectResponse {
    bool success = 1;
    string error = 2;
}

message DisconnectRequest {
    string device_id = 1;

    // What to do with the controller on disconnect.
    // "hold" = leave LEDs as-is (default)
    // "restore" = restore previous mode
    // "off" = set all LEDs to black
    string on_disconnect = 2;
}

message DisconnectResponse {
    bool success = 1;
}

// ─────────────────────────────────────────────────────────────
// Frame Streaming
// ─────────────────────────────────────────────────────────────

message PushFrameRequest {
    // Target controller.
    string device_id = 1;

    // Packed RGB bytes: [R0, G0, B0, R1, G1, B1, ...], 3 bytes per LED.
    bytes rgb_data = 2;

    // Number of LEDs (rgb_data.len() / 3). Redundant but useful for validation.
    uint32 led_count = 3;

    // Optional: update only a specific zone instead of the whole controller.
    // If empty, updates all LEDs (RGBCONTROLLER_UPDATELEDS).
    // If set, updates only this zone (RGBCONTROLLER_UPDATEZONELEDS).
    optional string zone_name = 4;
}

message PushFrameResponse {
    bool success = 1;
    string error = 2;

    // Time spent in the OpenRGB SDK call (microseconds). For performance monitoring.
    uint64 sdk_latency_us = 3;
}

message PushFrameBatchRequest {
    repeated FrameEntry entries = 1;
}

message FrameEntry {
    string device_id = 1;
    bytes rgb_data = 2;
    uint32 led_count = 3;
    optional string zone_name = 4;
}

message PushFrameBatchResponse {
    // Per-entry results, same order as the request.
    repeated FrameResult results = 1;

    // Total time for all SDK calls (microseconds).
    uint64 total_sdk_latency_us = 2;
}

message FrameResult {
    string device_id = 1;
    bool success = 2;
    string error = 3;
}

// ─────────────────────────────────────────────────────────────
// Events
// ─────────────────────────────────────────────────────────────

message SubscribeEventsRequest {}

message BridgeEvent {
    // Event type: "connected", "disconnected", "reconnecting",
    //             "device_added", "device_removed", "device_updated"
    string event_type = 1;

    // Event timestamp (Unix millis).
    uint64 timestamp_ms = 2;

    // Payload fields (populated based on event_type).

    // For "connected": number of controllers.
    uint32 controller_count = 3;

    // For "connected": negotiated protocol version.
    uint32 protocol_version = 4;

    // For "disconnected"/"reconnecting": reason string.
    string reason = 5;

    // For "device_added"/"device_removed"/"device_updated": device identifier.
    string device_id = 6;

    // For "device_added": full device info.
    DeviceInfo device_info = 7;

    // For "reconnecting": current attempt number.
    uint32 reconnect_attempt = 8;
}

// ─────────────────────────────────────────────────────────────
// Diagnostics
// ─────────────────────────────────────────────────────────────

message GetControllerDetailRequest {
    string device_id = 1;
}

message GetControllerDetailResponse {
    // Full device info (same as Discover response).
    DeviceInfo device = 1;

    // OpenRGB-specific details not exposed in the standard DeviceInfo.
    ControllerDetail detail = 2;
}

message ControllerDetail {
    // Controller description string.
    string description = 1;

    // Firmware/driver version.
    string version = 2;

    // Available modes.
    repeated ModeInfo modes = 3;

    // Index of the currently active mode.
    uint32 active_mode = 4;

    // Per-LED names (for keyboards with per-key naming).
    repeated string led_names = 5;

    // Current colors (for reading back device state).
    bytes current_colors = 6;
}

message ModeInfo {
    string name = 1;
    uint32 value = 2;
    uint32 flags = 3;

    // Speed range and current value.
    uint32 speed = 4;
    uint32 speed_min = 5;
    uint32 speed_max = 6;

    // Brightness range and current value (protocol v3+).
    uint32 brightness = 7;
    uint32 brightness_min = 8;
    uint32 brightness_max = 9;

    // Direction (0=left, 1=right, 2=up, 3=down, 4=horizontal, 5=vertical).
    uint32 direction = 10;

    // Color mode (0=none, 1=per-LED, 2=mode-specific, 3=random).
    uint32 color_mode = 11;

    // Mode-specific colors.
    bytes colors = 12;
}

// ─────────────────────────────────────────────────────────────
// Health & Lifecycle
// ─────────────────────────────────────────────────────────────

message HealthCheckRequest {}

message HealthCheckResponse {
    // Overall bridge health.
    HealthStatus status = 1;

    // OpenRGB connection state.
    ConnectionStatus openrgb_status = 2;

    // Uptime in seconds.
    uint64 uptime_seconds = 3;

    // Frames pushed since last health check.
    uint64 frames_since_last_check = 4;

    // Average frame push latency (microseconds).
    uint64 avg_frame_latency_us = 5;

    // Number of errors since last health check.
    uint32 errors_since_last_check = 6;
}

enum HealthStatus {
    HEALTH_STATUS_UNSPECIFIED = 0;

    // Everything is working.
    HEALTH_STATUS_HEALTHY = 1;

    // Bridge is running but OpenRGB is disconnected or experiencing errors.
    HEALTH_STATUS_DEGRADED = 2;

    // Bridge is in an error state (should be restarted).
    HEALTH_STATUS_UNHEALTHY = 3;
}

message ShutdownRequest {
    // Grace period for draining in-flight requests (milliseconds).
    // Default: 100ms.
    uint32 grace_period_ms = 1;
}

message ShutdownResponse {
    bool success = 1;
}
```

---

## Appendix A: Cargo Workspace Integration

```
hypercolor/
├── Cargo.toml                              # Workspace root
├── proto/
│   └── hypercolor/
│       └── bridge/
│           └── v1/
│               └── bridge.proto            # MIT/Apache-2.0
│
├── crates/
│   ├── hypercolor-core/                    # MIT/Apache-2.0
│   │   └── src/device/
│   │       └── openrgb_bridge_client.rs    # DeviceBackend impl (gRPC client)
│   │
│   ├── hypercolor-bridge-sdk/              # MIT/Apache-2.0
│   │   ├── Cargo.toml                      # tonic, prost, prost-build
│   │   ├── build.rs                        # Compiles bridge.proto
│   │   └── src/lib.rs                      # Re-exports generated types
│   │
│   └── bridges/
│       └── hypercolor-openrgb-bridge/      # GPL-2.0-only
│           ├── Cargo.toml                  # openrgb2, tonic, tokio, clap
│           ├── LICENSE-GPL-2.0             # Explicit GPL license file
│           ├── build.rs                    # Compiles same bridge.proto
│           └── src/
│               ├── main.rs                 # Entry point, CLI args, gRPC server bind
│               ├── bridge.rs               # OpenRgbBridge struct, SDK client logic
│               ├── service.rs              # tonic BridgeService impl
│               ├── config.rs               # TOML config parsing
│               ├── detection.rs            # Auto-detection logic
│               └── mapping.rs              # Controller -> DeviceInfo translation
```

## Appendix B: systemd Integration

```ini
# /usr/lib/systemd/system/hypercolor-openrgb-bridge.service
[Unit]
Description=Hypercolor OpenRGB Bridge
After=network.target openrgb.service
Wants=openrgb.service
PartOf=hypercolor.service

[Service]
Type=notify
ExecStart=/usr/bin/hypercolor-openrgb-bridge \
    --config /etc/hypercolor/openrgb-bridge.toml
RuntimeDirectory=hypercolor
User=hypercolor
Group=hypercolor
Restart=on-failure
RestartSec=2

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=true

[Install]
WantedBy=hypercolor.service
```

Alternatively, the daemon can manage the bridge process directly (see Section 8.4), in which case no separate systemd unit is needed.

## Appendix C: Quick Reference

### Protocol Command IDs Used by the Bridge

| ID | Constant | When |
|---|---|---|
| 40 | `REQUEST_PROTOCOL_VERSION` | Handshake |
| 50 | `SET_CLIENT_NAME` | Handshake |
| 0 | `REQUEST_CONTROLLER_COUNT` | Enumeration |
| 1 | `REQUEST_CONTROLLER_DATA` | Enumeration |
| 100 | `DEVICE_LIST_UPDATED` | Hot-plug (received) |
| 1100 | `RGBCONTROLLER_SETCUSTOMMODE` | Before streaming |
| 1050 | `RGBCONTROLLER_UPDATELEDS` | Every frame |
| 1051 | `RGBCONTROLLER_UPDATEZONELEDS` | Per-zone updates |

### gRPC Endpoints Summary

| RPC | Hot Path? | Typical Latency |
|---|---|---|
| `PushFrame` | Yes (60fps) | <1ms IPC + <1ms SDK |
| `PushFrameBatch` | Yes (60fps) | <1ms IPC + N * <1ms SDK |
| `Discover` | No | 50-200ms (SDK enumeration) |
| `SubscribeEvents` | No (streaming) | N/A |
| `HealthCheck` | No (5s interval) | <1ms |
| `GetControllerDetail` | No (on demand) | 10-50ms |
