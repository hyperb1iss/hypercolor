# 30 -- ROLI Blocks Backend (blocksd Bridge)

> IPC bridge to blocksd for driving ROLI Lightpad, LUMI Keys, and Seaboard Blocks as pixel-addressable RGB surfaces. The first out-of-process device backend.

**Status:** Draft
**Crate:** `hypercolor-core`
**Module path:** `hypercolor_core::device::blocks`
**Feature flag:** `blocks` (default-enabled)
**Author:** Nova
**Date:** 2026-03-13

---

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture: Why a Bridge](#2-architecture-why-a-bridge)
3. [blocksd API Surface](#3-blocksd-api-surface)
4. [IPC Transport](#4-ipc-transport)
5. [Wire Protocol](#5-wire-protocol)
6. [Device Model](#6-device-model)
7. [Frame Pipeline](#7-frame-pipeline)
8. [Touch & Pressure Input](#8-touch--pressure-input)
9. [BlocksBackend Implementation](#9-blocksbackend-implementation)
10. [Discovery & Lifecycle](#10-discovery--lifecycle)
11. [Configuration](#11-configuration)
12. [Error Handling & Resilience](#12-error-handling--resilience)
13. [Performance Budget](#13-performance-budget)
14. [Testing Strategy](#14-testing-strategy)
15. [Implementation Sequence](#15-implementation-sequence)

---

## 1. Overview

ROLI Blocks are modular music controllers (Lightpad Block, LUMI Keys, Seaboard Block, and others)
that snap together magnetically via a DNA mesh topology. Each block contains a **15×15 RGB LED
grid** (225 pixels) driven via MIDI SysEx over USB. The protocol is non-trivial: 7-bit-safe payload
packing, diff-based heap writes with ACK tracking, and on-device LittleFoot bytecode programs that
repaint the grid at ~25 Hz.

Rather than rewriting this protocol stack in Rust, Hypercolor delegates hardware interaction to
**blocksd** — a dedicated Python daemon that already implements the full ROLI protocol with 275+
tests. Hypercolor communicates with blocksd over a local **Unix domain socket**, making this the
first `ConnectionType::Bridge` backend in the system.

### Supported Hardware

| Device | Serial Prefix | USB PID | LED Grid | Heap Size | Surface |
|--------|--------------|---------|----------|-----------|---------|
| Lightpad Block | LPB | `0x0900` | 15×15 | 7200 B | Pressure-sensitive XY pad |
| Lightpad Block M | LPM | `0x0900` | 15×15 | 7200 B | Same as LPB |
| LUMI Keys Block | LKB | `0x0E00` | 15×15 | 7200 B | 24-key mini keyboard |
| Seaboard Block | SBB | `0x0700` | 15×15 | 7200 B | Continuous pitch surface |
| Live Block | LIC | `0x0B00` | 15×15 | 3000 B | Button matrix |
| Loop Block | LOC | `0x0C00` | 15×15 | 3000 B | Looper controls |
| Touch Block | TCB | `0x0D00` | 15×15 | 3000 B | Blank touch surface |

All devices share the same 15×15 LED grid and RGB565 color encoding. The ROLI USB vendor ID is
`0x2AF4`.

### Relationship to Other Specs

- **Spec 02 (Device Backend):** Defines the `DeviceBackend` trait this driver implements
- **Spec 03 (WLED Backend):** Reference pattern — external-service-backed device driver
- **Spec 06 (Spatial Engine):** Matrix topology mapping for the 15×15 grid
- **Spec 09 (Event Bus):** Touch/pressure events published as reactive inputs
- **Spec 12 (Configuration):** TOML schema for blocksd connection settings

### Prior Art

- [blocksd](https://github.com/hyperb1iss/blocksd) — ROLI Blocks daemon for Linux (Python 3.13+)
- [JUCE BLOCKS SDK](https://github.com/WeAreROLI/BLOCKS-SDK) — ROLI's official C++ SDK (archived)
- ROLI manufacturer ID: `0x00 0x21 0x10 0x77` (SysEx header)

---

## 2. Architecture: Why a Bridge

### 2.1 Integration Options Considered

| Approach | Effort | Maintenance | Touch Input | Multi-device |
|----------|--------|-------------|-------------|--------------|
| **A. Native Rust driver** | ~6 weeks | Dual codebases | Rewrite parser | Rewrite topology |
| **B. FFI to Python** | ~2 weeks | Fragile linking | Possible | Complex |
| **C. IPC bridge to blocksd** | ~1 week | One source of truth | Free | Free |

### 2.2 Why Bridge Wins

The ROLI protocol is uniquely complex among Hypercolor device targets:

1. **7-bit packing** — all MIDI SysEx payloads must be 7-bit-safe, requiring bitwise packing across
   byte boundaries with LSB-first ordering. This is ~300 lines of thoroughly tested Python.

2. **Diff-based heap writes** — LED data isn't sent as full frames. blocksd computes a compact diff
   (SharedDataChange) using skip, set-sequence, and set-repeated commands with continuation bits.
   The encoder coalesces small skips when sending raw bytes is cheaper. Another ~400 lines.

3. **ACK-driven backpressure** — the device ACKs heap writes with a 10-bit packet counter. blocksd
   tracks in-flight bytes (max 200), retransmits after 250ms, and blocks new sends until capacity
   opens. This prevents device buffer overflow.

4. **LittleFoot bytecode** — blocksd uploads a 94-byte VM program to each device that reads
   RGB565 pixel data from the heap and calls `fillPixel()` at ~25 Hz. The assembler and opcode
   table are ~200 lines.

5. **DNA mesh topology** — blocks snap together and form multi-device topologies. Discovery requires
   parsing topology packets (device UIDs, port connections, battery levels) across multiple SysEx
   messages.

Rewriting all of this in Rust would produce a second, undertested implementation of a protocol with
many edge cases. The bridge approach keeps blocksd as the single source of truth for protocol
correctness while Hypercolor focuses on what it does best: spatial mapping, effect rendering, and
multi-device orchestration.

### 2.3 Architectural Boundary

```
┌─────────────────────────────────┐     ┌──────────────────────────────┐
│         Hypercolor              │     │          blocksd             │
│                                 │     │                              │
│  Effect Engine ──▶ Sampler      │     │  blocksd API Server          │
│                     │           │     │    │                         │
│                     ▼           │     │    ▼                         │
│  BlocksBackend ────────────────────▶ Unix Socket ──▶ TopologyManager │
│  (DeviceBackend)    │           │     │    │            │            │
│                     │           │     │    ▼            ▼            │
│  Event Bus ◀────────┘           │     │  DeviceGroup ──▶ MIDI SysEx │
│  (touch/pressure)               │     │    │                  │     │
│                                 │     │    ▼                  ▼     │
└─────────────────────────────────┘     │  RemoteHeap ──▶ USB/MIDI    │
                                        └──────────────────────────────┘
```

Hypercolor sends RGB888 frames over the socket. blocksd converts to RGB565, computes diffs, packs
into 7-bit SysEx, handles ACKs, retransmits, and manages device lifecycle. Clean separation.

---

## 3. blocksd API Surface

blocksd must expose a lightweight API server on a Unix domain socket. This section specifies the
API that blocksd implements and Hypercolor consumes.

### 3.1 Socket Path

```
Default:  $XDG_RUNTIME_DIR/blocksd/blocksd.sock
Fallback: /tmp/blocksd/blocksd.sock
```

The socket directory is created by blocksd on startup with `0o700` permissions.

### 3.2 Protocol Overview

The API uses a **newline-delimited JSON** (NDJSON) protocol over a Unix stream socket. Each message
is a single JSON object terminated by `\n`. This avoids HTTP overhead while remaining debuggable
with standard tools (`socat`, `jq`).

Two message patterns:

1. **Request/Response** — client sends a request, server sends exactly one response
2. **Server-Sent Events** — after subscribing, server pushes events as they occur

Each message has a `type` field that identifies the message kind.

### 3.3 Request Messages (Hypercolor → blocksd)

#### `discover` — List Connected Devices

```json
{
  "type": "discover",
  "id": "req-001"
}
```

Response:

```json
{
  "type": "discover_response",
  "id": "req-001",
  "devices": [
    {
      "uid": 2882400135,
      "serial": "LPB1234567890AB",
      "block_type": "lightpad",
      "name": "Lightpad Block",
      "battery_level": 85,
      "battery_charging": false,
      "grid_width": 15,
      "grid_height": 15,
      "firmware_version": "0.4.2"
    }
  ]
}
```

The `uid` is the device's topology UID — exposed by blocksd as a deterministic 64-bit integer. It is
stable across reconnections for the same physical device and serves as the device address for all
subsequent commands.

#### `frame` — Write RGB Frame Data

```json
{
  "type": "frame",
  "uid": 2882400135,
  "pixels": "<base64-encoded RGB888 data>"
}
```

The `pixels` field contains **675 bytes** of raw RGB888 data (225 pixels × 3 bytes), base64-encoded
to **900 characters**. Row-major order, top-left origin, matching the device's 15×15 grid layout.

Response:

```json
{
  "type": "frame_ack",
  "uid": 2882400135,
  "accepted": true
}
```

If `accepted` is `false`, the daemon rejected the write because the device was unavailable, the
`uid` was unknown, or the payload was malformed. Callers should treat this as retryable while a
device is still coming up.

#### `frame_binary` — Write Frame Data (Binary Fast Path)

For sustained 25+ fps rendering, JSON + base64 adds ~33% overhead and parse latency. The binary
fast path uses a fixed-size binary message format on the same socket:

```
Byte:  0       1       2  3  4  5  6  7  8  9    10 ............ 684
     ┌───────┬───────┬──────────────┬───────────────────┐
     │ Magic │ Type  │   UID (LE)   │ RGB888 × 225      │
     │ 0xBD  │ 0x01  │   8 bytes    │ 675 bytes         │
     └───────┴───────┴──────────────┴───────────────────┘
     Total: 685 bytes
```

- **Magic byte `0xBD`** (for "**B**locks **D**ata") — distinguishes binary messages from JSON
  (which always starts with `{` = `0x7B`). The server peeks at the first byte to determine mode.
- **Type `0x01`** — frame write. Reserved for future binary message types.
- **UID** — 64-bit little-endian device topology UID.
- **Pixels** — 675 bytes of raw RGB888, row-major, no base64 encoding.

Response: single byte `0x01` (accepted) or `0x00` (rejected because the device is unavailable or
the payload is invalid). Once a device is live, blocksd coalesces later frames into the latest
target state rather than surfacing internal heap backpressure as dropped writes.

Both JSON and binary paths are supported simultaneously on the same socket. Hypercolor uses the
binary path for frame writes during rendering and JSON for everything else.

#### `brightness` — Set Global Brightness

```json
{
  "type": "brightness",
  "uid": 2882400135,
  "value": 200
}
```

`value` is 0–255. blocksd applies this as a multiplier before RGB565 conversion:

```python
r = (r * value) // 255
g = (g * value) // 255
b = (b * value) // 255
```

Response:

```json
{
  "type": "brightness_ack",
  "uid": 2882400135,
  "ok": true
}
```

#### `subscribe` — Enable Event Streaming

```json
{
  "type": "subscribe",
  "events": ["device", "touch", "button"]
}
```

After subscription, the server pushes events as they occur (see [§8](#8-touch--pressure-input)).
Multiple event types can be subscribed in one message. Subscribing is idempotent.

Response:

```json
{
  "type": "subscribed",
  "events": ["device", "touch", "button"]
}
```

#### `ping` — Health Check

```json
{
  "type": "ping",
  "id": "health-001"
}
```

Response:

```json
{
  "type": "pong",
  "id": "health-001",
  "version": "0.1.0",
  "uptime_seconds": 3456,
  "device_count": 2
}
```

### 3.4 Server-Sent Events (blocksd → Hypercolor)

After subscribing, the server pushes these events:

#### Device Events

```json
{
  "type": "device_added",
  "device": {
    "uid": 2882400135,
    "serial": "LPB1234567890AB",
    "block_type": "lightpad",
    "name": "Lightpad Block",
    "battery_level": 85,
    "battery_charging": false,
    "grid_width": 15,
    "grid_height": 15,
    "firmware_version": "0.4.2"
  }
}
```

```json
{
  "type": "device_removed",
  "uid": 2882400135,
  "reason": "timeout"
}
```

#### Touch Events

```json
{
  "type": "touch",
  "uid": 2882400135,
  "action": "start",
  "index": 0,
  "x": 0.456,
  "y": 0.789,
  "z": 0.632,
  "vx": 0.12,
  "vy": -0.05,
  "vz": 0.0
}
```

| Field | Type | Range | Description |
|-------|------|-------|-------------|
| `action` | string | `start`, `move`, `end` | Touch lifecycle phase |
| `index` | u8 | 0–15 | Touch finger index (multitouch) |
| `x` | f32 | 0.0–1.0 | Horizontal position (left to right) |
| `y` | f32 | 0.0–1.0 | Vertical position (top to bottom) |
| `z` | f32 | 0.0–1.0 | Pressure (0 = no contact, 1 = max) |
| `vx` | f32 | -1.0–1.0 | Horizontal velocity |
| `vy` | f32 | -1.0–1.0 | Vertical velocity |
| `vz` | f32 | -1.0–1.0 | Pressure velocity |

#### Button Events

```json
{
  "type": "button",
  "uid": 2882400135,
  "action": "press"
}
```

`action` is `press` or `release`. ROLI Blocks have a single mode button.

---

## 4. IPC Transport

### 4.1 Connection Management

```rust
pub struct BlocksConnection {
    stream: UnixStream,
    read_buf: BytesMut,
    write_buf: BytesMut,
    state: ConnectionState,
    reconnect_delay: Duration,
    last_pong: Option<Instant>,
}

enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Subscribing,
    Ready,
}
```

The `BlocksConnection` manages the Unix socket lifecycle:

1. **Connect** — attempt `UnixStream::connect(socket_path)` with 2s timeout
2. **Handshake** — send `ping`, verify `pong` response and version compatibility
3. **Subscribe** — send `subscribe` for desired event types
4. **Ready** — begin frame writes and event processing

### 4.2 Reconnection Strategy

blocksd may restart independently of Hypercolor (systemd watchdog, user `systemctl restart`, etc.).
The backend must handle disconnections gracefully:

```
Disconnect detected
  │
  ├─ Mark all devices as Disconnected
  ├─ Notify DeviceRegistry (triggers layout fallback)
  │
  └─ Reconnect loop:
       Wait 500ms → try connect
       Wait 1s    → try connect
       Wait 2s    → try connect
       Wait 5s    → try connect (cap)
       ... repeat at 5s intervals
       │
       └─ On success:
            Handshake → Subscribe → Discover → Re-register devices
```

Exponential backoff with 5s cap. The backend remains registered in `BackendManager` during
disconnection — it simply returns empty device lists from `discover()` until reconnected.

### 4.3 Message Framing

**JSON messages:** newline-delimited (`\n`). Each message is a complete JSON object. The reader
scans for `\n` boundaries and parses each line independently.

**Binary messages:** length-prefixed by the magic byte + known fixed size. The reader peeks at byte
0: if `0xBD`, read exactly 681 bytes (frame) or parse by type byte; if `{`, read until `\n` and
parse as JSON.

```rust
async fn read_message(stream: &mut UnixStream, buf: &mut BytesMut) -> Result<BlocksMessage> {
    let first_byte = peek_byte(stream).await?;
    match first_byte {
        0xBD => read_binary_message(stream, buf).await,
        _    => read_json_line(stream, buf).await,
    }
}
```

### 4.4 Concurrency Model

The connection runs two tasks:

- **Writer task** — receives `FrameCommand` / `JsonCommand` from a channel, serializes and writes
  to socket. Coalesces multiple pending frames for the same device (only the latest frame matters).
- **Reader task** — reads from socket, dispatches responses to waiting request futures and events
  to the event channel.

Frame writes wait for a 1-byte accept/reject ack when using the binary fast path. JSON
request/response pairs use a `oneshot` channel per request ID.

---

## 5. Wire Protocol

### 5.1 Color Encoding (blocksd Internal)

Hypercolor sends **RGB888** (3 bytes per pixel, 8-bit per channel). blocksd converts to RGB565
for the device heap:

```
RGB888 Input          RGB565 Storage (little-endian)
┌──────────────┐      ┌─────────────────────────────────┐
│ R: 8 bits    │ ──▶  │ Byte 0: [G2 G1 G0 R4 R3 R2 R1 R0] │
│ G: 8 bits    │      │ Byte 1: [B4 B3 B2 B1 B0 G5 G4 G3] │
│ B: 8 bits    │      └─────────────────────────────────┘
└──────────────┘

Conversion:
  r5 = (r8 >> 3) & 0x1F     // 256 levels → 32 levels  (±4 max error)
  g6 = (g8 >> 2) & 0x3F     // 256 levels → 64 levels  (±2 max error)
  b5 = (b8 >> 3) & 0x1F     // 256 levels → 32 levels  (±4 max error)

  byte0 = r5 | (g6 & 0x07) << 5
  byte1 = (g6 >> 3) | b5 << 3
```

This quantization is invisible to Hypercolor — it sends full RGB888 and blocksd handles the lossy
conversion. Effects should be designed knowing that the device has ~32K effective colors, not 16.7M.

### 5.2 Pixel Layout

The 15×15 grid is addressed in **row-major order**, top-left origin:

```
Index:  0   1   2   3  ...  14
       15  16  17  18  ...  29
       30  31  32  33  ...  44
       ...
      210 211 212 213  ... 224
```

In the RGB888 frame buffer:
- Pixel `(x, y)` = byte offset `(y * 15 + x) * 3`
- Total frame size: `15 * 15 * 3 = 675 bytes`

### 5.3 Device Heap Layout (blocksd Internal)

For reference — this is blocksd's internal concern, not Hypercolor's:

```
Offset (bytes)   Content
─────────────    ───────
0 – 93           BitmapLEDProgram bytecode (94 bytes)
94 – 543         LED pixel data (450 bytes, RGB565)
544 – 7199       Unused heap space
```

The BitmapLEDProgram reads pixel data starting at offset 94 and repaints the grid at ~25 Hz. The
program runs on the device's on-chip LittleFoot VM:

```c
void repaint() {
    for (int y = 0; y < 15; ++y)
        for (int x = 0; x < 15; ++x) {
            int bit = (x + y * 15) * 16;
            fillPixel(makeARGB(255,
                getHeapBits(bit, 5) << 3,
                getHeapBits(bit + 5, 6) << 2,
                getHeapBits(bit + 11, 5) << 3),
                x, y);
        }
}
```

### 5.4 SysEx Transport (blocksd Internal)

Also for reference — blocksd handles this entirely:

```
SysEx Frame:
F0 00 21 10 77 [deviceIndex] [7-bit packed payload] [checksum] F7
│  │           │              │                      │
│  │           │              │                      └─ XOR-sum & 0x7F
│  │           │              └─ All payload bytes 7-bit safe
│  │           └─ Lower 6 bits = topology index
│  └─ ROLI manufacturer ID
└─ SysEx start byte

Checksum:
  acc = len(data) & 0xFF
  for byte in data:
      acc = (acc + (acc * 2 + byte)) & 0xFF
  return acc & 0x7F
```

LED updates use **SharedDataChange** messages — diff-based heap writes that only transmit changed
regions, with RLE for repeated values and skip commands for unchanged regions.

---

## 6. Device Model

### 6.1 Hypercolor Type Mapping

```rust
// New DeviceFamily variant
pub enum DeviceFamily {
    // ... existing variants ...
    Roli,
}

impl DeviceFamily {
    pub fn vendor_name(&self) -> &str {
        match self {
            Self::Roli => "ROLI",
            // ...
        }
    }

    pub fn id(&self) -> Cow<'static, str> {
        match self {
            Self::Roli => "roli".into(),
            // ...
        }
    }

    pub fn backend_id(&self) -> &'static str {
        match self {
            Self::Roli => "blocks",
            // ...
        }
    }
}
```

### 6.2 Device Type Enum

```rust
/// ROLI Block hardware variants, identified by serial number prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoliBlockType {
    Lightpad,
    LightpadM,
    LumiKeys,
    Seaboard,
    Live,
    Loop,
    Touch,
    Developer,
}

impl RoliBlockType {
    /// Human-readable device name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Lightpad => "Lightpad Block",
            Self::LightpadM => "Lightpad Block M",
            Self::LumiKeys => "LUMI Keys",
            Self::Seaboard => "Seaboard Block",
            Self::Live => "Live Block",
            Self::Loop => "Loop Block",
            Self::Touch => "Touch Block",
            Self::Developer => "Developer Control Block",
        }
    }

    /// Whether this block type has a pressure-sensitive touch surface.
    pub fn has_touch_surface(&self) -> bool {
        matches!(self, Self::Lightpad | Self::LightpadM | Self::Seaboard | Self::Touch)
    }

    /// Parse from blocksd's `block_type` JSON field.
    pub fn from_api(s: &str) -> Option<Self> {
        match s {
            "lightpad" => Some(Self::Lightpad),
            "lightpad_m" => Some(Self::LightpadM),
            "lumi_keys" => Some(Self::LumiKeys),
            "seaboard" => Some(Self::Seaboard),
            "live" => Some(Self::Live),
            "loop" => Some(Self::Loop),
            "touch" => Some(Self::Touch),
            "developer" => Some(Self::Developer),
            _ => None,
        }
    }
}
```

### 6.3 DeviceInfo Construction

When blocksd reports a device, the backend constructs a `DeviceInfo`:

```rust
fn device_info_from_blocks(dev: &BlocksDeviceResponse) -> DeviceInfo {
    let block_type = RoliBlockType::from_api(&dev.block_type)
        .unwrap_or(RoliBlockType::Lightpad);

    DeviceInfo {
        id: device_id_from_uid(dev.uid),
        name: format!("{} ({})", block_type.display_name(), &dev.serial[..6]),
        vendor: "ROLI".to_owned(),
        family: DeviceFamily::Roli,
        model: Some(block_type.display_name().to_owned()),
        connection_type: ConnectionType::Bridge,
        zones: vec![ZoneInfo {
            name: "Grid".to_owned(),
            led_count: 225,
            topology: DeviceTopologyHint::Matrix { rows: 15, cols: 15 },
            color_format: DeviceColorFormat::Rgb,
        }],
        firmware_version: dev.firmware_version.clone(),
        capabilities: DeviceCapabilities {
            led_count: 225,
            supports_direct: true,
            supports_brightness: true,
            has_display: false,
            display_resolution: None,
            max_fps: 25,
            features: DeviceFeatures::empty(),
        },
    }
}
```

### 6.4 Stable Device IDs

Device IDs must be deterministic so that Hypercolor can persist layout assignments across sessions.
blocksd exposes a deterministic 64-bit UID for each physical device, derived from its serial. We
derive a UUIDv5 from that UID:

```rust
const BLOCKS_NAMESPACE: Uuid = uuid::uuid!("b10c4500-da7a-4001-b10c-4500da7a4001");

fn device_id_from_uid(uid: u64) -> DeviceId {
    let uuid = Uuid::new_v5(&BLOCKS_NAMESPACE, &uid.to_le_bytes());
    DeviceId::from_uuid(uuid)
}
```

This ensures the same physical block always maps to the same `DeviceId` regardless of USB port,
reconnection order, or topology position.

---

## 7. Frame Pipeline

### 7.1 Render Flow

```
Effect Engine (60 fps)
       │
       ▼
  Spatial Sampler
  (maps canvas → 15×15 matrix)
       │
       ▼
  BackendManager::write_colors()
       │
       ▼
  BlocksBackend::write_colors(device_id, &[[u8; 3]; 225])
       │
       ├─ Binary frame message (681 bytes)
       │
       ▼
  Unix Socket ──▶ blocksd
       │
       ├─ RGB888 → RGB565 conversion
       ├─ SharedDataChange diff computation
       ├─ 7-bit SysEx packing
       ├─ ACK tracking + retransmission
       │
       ▼
  MIDI USB ──▶ ROLI Block ──▶ LittleFoot VM ──▶ LEDs (~25 Hz)
```

### 7.2 Frame Rate Governance

The device repaints at ~25 Hz regardless of how fast the host sends data. Sending faster than 25 fps
wastes bandwidth — the device will display the most recent complete frame at each repaint tick.

| Layer | Rate | Governed By |
|-------|------|-------------|
| Effect engine | 60 fps | Hypercolor render loop |
| Backend output | 25 fps | `target_fps()` return value |
| blocksd → device | ~25 fps | ACK backpressure + diff compression |
| Device repaint | ~25 Hz | On-chip LittleFoot VM timer |

`BlocksBackend::target_fps()` returns `Some(25)` for all ROLI devices. The `BackendManager` uses
this to throttle frame dispatch — it drops intermediate frames and sends only the latest at 25 fps.

### 7.3 Frame Coalescing

If the effect engine produces frames faster than blocksd can consume them, the writer task
coalesces: it keeps only the most recent frame per device in its send buffer. When the socket
becomes writable, it sends the latest frame and discards any older ones.

```rust
struct FrameCoalescer {
    pending: HashMap<DeviceId, Vec<u8>>,  // Latest frame per device
}

impl FrameCoalescer {
    fn submit(&mut self, id: DeviceId, frame: Vec<u8>) {
        self.pending.insert(id, frame);  // Overwrites previous
    }

    fn drain(&mut self) -> impl Iterator<Item = (DeviceId, Vec<u8>)> {
        self.pending.drain()
    }
}
```

### 7.4 Color Considerations for Effect Authors

The RGB565 quantization means some colors render differently on ROLI blocks than on WLED strips or
Razer keyboards:

| Color | RGB888 | After RGB565 Roundtrip | Visible? |
|-------|--------|----------------------|----------|
| Pure white | (255,255,255) | (248,252,248) | Barely |
| Deep purple | (128,0,128) | (128,0,128) | No |
| Subtle gray | (40,40,40) | (40,40,40) | No |
| Pastel pink | (255,200,210) | (248,200,208) | Barely |
| Orange-red | (255,69,0) | (248,68,0) | Barely |

The quantization error is ≤7 per channel — generally imperceptible on LED hardware. Effects don't
need special handling for ROLI devices.

---

## 8. Touch & Pressure Input

### 8.1 Input Architecture

ROLI Blocks with touch surfaces (Lightpad, Seaboard, Touch) produce high-resolution multitouch
events. These are published to Hypercolor's event bus as **reactive inputs** that effects can bind
to.

```rust
/// Touch event from a ROLI Block surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlocksTouchEvent {
    pub device_id: DeviceId,
    pub action: TouchAction,
    pub index: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,      // pressure
    pub vx: f32,     // horizontal velocity
    pub vy: f32,     // vertical velocity
    pub vz: f32,     // pressure velocity
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchAction {
    Start,
    Move,
    End,
}
```

### 8.2 Event Bus Integration

Touch events are published to the event bus under the `blocks/touch` topic:

```rust
// In BlocksBackend event reader task
async fn process_touch_event(bus: &EventBus, event: BlocksTouchEvent) {
    bus.publish(Event::BlocksTouch(event)).await;
}
```

Effects can subscribe to touch events and use them as control parameters:

- **Pressure (z)** → effect intensity, brightness, or color saturation
- **Position (x, y)** → color hue, spatial offset, focal point
- **Velocity (vx, vy)** → particle emission direction, wave propagation

### 8.3 Touch-to-Pixel Mapping

The touch surface and LED grid share the same 15×15 coordinate space. A touch at `(x=0.5, y=0.5)`
corresponds to the center pixel `(7, 7)`. This enables reactive effects where touching a key
directly illuminates the corresponding LED:

```rust
fn touch_to_pixel(x: f32, y: f32) -> (u8, u8) {
    let px = (x * 14.0).round().clamp(0.0, 14.0) as u8;
    let py = (y * 14.0).round().clamp(0.0, 14.0) as u8;
    (px, py)
}
```

### 8.4 Touch Rate

ROLI devices report touch events at approximately **100–200 Hz** per finger. With multitouch, this
can produce significant event volume. The backend should:

1. Forward all events to the event bus (no pre-filtering)
2. Let effects sample at their own rate via `Signal::derive` or similar
3. Optionally batch events per render tick if bus congestion is detected

---

## 9. BlocksBackend Implementation

### 9.1 Struct Definition

```rust
pub struct BlocksBackend {
    /// Socket path for blocksd connection.
    socket_path: PathBuf,
    /// Active connection (None if disconnected).
    connection: Option<BlocksConnection>,
    /// Known devices reported by blocksd.
    devices: HashMap<DeviceId, BlocksDevice>,
    /// UID → DeviceId mapping for event routing.
    uid_map: HashMap<u64, DeviceId>,
    /// Per-device brightness (applied by blocksd).
    brightness: HashMap<DeviceId, u8>,
    /// Event sender for touch/device events.
    event_tx: Option<mpsc::UnboundedSender<BlocksEvent>>,
    /// Reconnection state.
    reconnect_state: ReconnectState,
}

struct BlocksDevice {
    uid: u64,
    info: DeviceInfo,
    block_type: RoliBlockType,
    connected: bool,
    frames_sent: u64,
    last_frame_at: Option<Instant>,
}

struct ReconnectState {
    last_attempt: Option<Instant>,
    delay: Duration,
    consecutive_failures: u32,
}
```

### 9.2 DeviceBackend Trait Implementation

```rust
#[async_trait::async_trait]
impl DeviceBackend for BlocksBackend {
    fn info(&self) -> BackendInfo {
        BackendInfo {
            id: "blocks".to_owned(),
            name: "ROLI Blocks (blocksd)".to_owned(),
            description: "ROLI Lightpad, LUMI Keys, and Seaboard via blocksd daemon".to_owned(),
        }
    }

    async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
        let conn = match self.ensure_connected().await {
            Ok(c) => c,
            Err(_) => return Ok(vec![]),  // blocksd not running — no devices
        };

        let response = conn.request_discover().await?;

        self.devices.clear();
        self.uid_map.clear();

        let mut infos = Vec::with_capacity(response.devices.len());
        for dev in &response.devices {
            let info = device_info_from_blocks(dev);
            let device_id = info.id.clone();

            self.uid_map.insert(dev.uid, device_id.clone());
            self.devices.insert(device_id.clone(), BlocksDevice {
                uid: dev.uid,
                info: info.clone(),
                block_type: RoliBlockType::from_api(&dev.block_type)
                    .unwrap_or(RoliBlockType::Lightpad),
                connected: false,
                frames_sent: 0,
                last_frame_at: None,
            });

            infos.push(info);
        }

        Ok(infos)
    }

    async fn connect(&mut self, id: &DeviceId) -> Result<()> {
        let device = self.devices.get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocks device: {id}"))?;
        device.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self, id: &DeviceId) -> Result<()> {
        if let Some(device) = self.devices.get_mut(id) {
            device.connected = false;
        }
        Ok(())
    }

    async fn write_colors(&mut self, id: &DeviceId, colors: &[[u8; 3]]) -> Result<()> {
        let device = self.devices.get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocks device: {id}"))?;

        if !device.connected {
            bail!("device not connected: {id}");
        }

        let conn = self.connection.as_mut()
            .ok_or_else(|| anyhow::anyhow!("blocksd not connected"))?;

        // Send binary frame (681 bytes total)
        conn.write_frame_binary(device.uid, colors).await?;

        device.frames_sent += 1;
        device.last_frame_at = Some(Instant::now());

        Ok(())
    }

    async fn set_brightness(&mut self, id: &DeviceId, brightness: u8) -> Result<()> {
        let device = self.devices.get(id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocks device: {id}"))?;

        let conn = self.connection.as_mut()
            .ok_or_else(|| anyhow::anyhow!("blocksd not connected"))?;

        conn.request_brightness(device.uid, brightness).await?;
        self.brightness.insert(id.clone(), brightness);

        Ok(())
    }

    fn target_fps(&self, _id: &DeviceId) -> Option<u32> {
        Some(25)
    }
}
```

### 9.3 Connection Helpers

```rust
impl BlocksBackend {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            connection: None,
            devices: HashMap::new(),
            uid_map: HashMap::new(),
            brightness: HashMap::new(),
            event_tx: None,
            reconnect_state: ReconnectState {
                last_attempt: None,
                delay: Duration::from_millis(500),
                consecutive_failures: 0,
            },
        }
    }

    /// Default socket path from environment.
    pub fn default_socket_path() -> PathBuf {
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            PathBuf::from(runtime_dir).join("blocksd/blocksd.sock")
        } else {
            PathBuf::from("/tmp/blocksd/blocksd.sock")
        }
    }

    /// Connect to blocksd if not already connected. Returns error if blocksd
    /// is unreachable (not running, socket doesn't exist, etc.).
    async fn ensure_connected(&mut self) -> Result<&mut BlocksConnection> {
        if self.connection.is_some() {
            return Ok(self.connection.as_mut().expect("checked above"));
        }

        // Respect backoff timing
        if let Some(last) = self.reconnect_state.last_attempt {
            if last.elapsed() < self.reconnect_state.delay {
                bail!("reconnect backoff active");
            }
        }

        self.reconnect_state.last_attempt = Some(Instant::now());

        match BlocksConnection::connect(&self.socket_path).await {
            Ok(conn) => {
                self.reconnect_state.delay = Duration::from_millis(500);
                self.reconnect_state.consecutive_failures = 0;
                self.connection = Some(conn);
                Ok(self.connection.as_mut().expect("just set"))
            }
            Err(e) => {
                self.reconnect_state.consecutive_failures += 1;
                self.reconnect_state.delay = (self.reconnect_state.delay * 2)
                    .min(Duration::from_secs(5));
                Err(e)
            }
        }
    }
}
```

---

## 10. Discovery & Lifecycle

### 10.1 Daemon Detection

Hypercolor does not start or manage blocksd — it discovers it by probing the socket path. If the
socket doesn't exist, the backend returns an empty device list. No error, no warning — blocksd is
simply optional.

```rust
async fn discover(&mut self) -> Result<Vec<DeviceInfo>> {
    if !self.socket_path.exists() {
        return Ok(vec![]);
    }
    // ... connect and query
}
```

### 10.2 Hot-Plug via Events

After initial discovery, the backend subscribes to device events. When blocksd reports a new device
(USB hot-plug, magnetic DNA snap), the backend:

1. Constructs `DeviceInfo` from the event payload
2. Registers the device in its local map
3. Publishes `DeviceDiscovered` to Hypercolor's device registry
4. The layout engine may auto-assign the new device

When a device is removed:

1. Marks the device as disconnected
2. Publishes `DeviceDisconnected` to the registry
3. Retains the device in the local map (for reconnection matching)

### 10.3 Lifecycle State Machine

```
                       blocksd not running
                       ┌──────────────┐
                       │   Dormant    │◀─── socket doesn't exist
                       └──────┬───────┘
                              │ socket appears (next discover() call)
                              ▼
                       ┌──────────────┐
                       │  Connected   │──── ping/pong verified
                       └──────┬───────┘
                              │ subscribe + discover
                              ▼
                       ┌──────────────┐
              ┌───────▶│    Active    │◀─── devices discovered, events flowing
              │        └──────┬───────┘
              │               │ socket EOF / error
              │               ▼
              │        ┌──────────────┐
              └────────│ Reconnecting │──── exponential backoff
                       └──────────────┘
```

### 10.4 Multi-Block Topologies

When multiple ROLI blocks are snapped together via DNA connectors, blocksd reports each as an
independent device with its own UID. Hypercolor treats them as separate devices in the spatial
layout — each with its own 15×15 zone.

For future consideration: blocksd could report topology connection data (which blocks are adjacent
on which edges), enabling Hypercolor to auto-stitch them into a unified 30×15 or 15×30 surface.
This is not in scope for the initial implementation.

---

## 11. Configuration

### 11.1 TOML Schema

```toml
[backends.blocks]
# Enable the ROLI Blocks backend (default: true when feature enabled)
enabled = true

# Path to blocksd Unix socket
# Default: $XDG_RUNTIME_DIR/blocksd/blocksd.sock
socket_path = "/run/user/1000/blocksd/blocksd.sock"

# Subscribe to touch events from pressure-sensitive blocks
# Publishes to event bus as blocks/touch
touch_events = true

# Subscribe to button events
button_events = false
```

### 11.2 Feature Flag

The `blocks` feature is default-enabled in `hypercolor-core`. It adds:

- `tokio::net::UnixStream` dependency (already available via tokio)
- `BlocksBackend` struct and registration in daemon startup
- No additional crate dependencies — the backend uses only `tokio`, `serde_json`, `anyhow`, and
  `base64` (all already in the dependency tree)

### 11.3 Daemon Registration

```rust
// In hypercolor-daemon/src/startup.rs
if config.backends.blocks.enabled {
    let socket_path = config.backends.blocks.socket_path.clone()
        .unwrap_or_else(BlocksBackend::default_socket_path);
    backend_manager_inner.register_backend(
        Box::new(BlocksBackend::new(socket_path))
    );
}
```

---

## 12. Error Handling & Resilience

### 12.1 Failure Modes

| Failure | Detection | Recovery |
|---------|-----------|----------|
| blocksd not installed | Socket path doesn't exist | Return empty device list; log nothing |
| blocksd not running | `connect()` ECONNREFUSED | Exponential backoff reconnect |
| blocksd crashes mid-session | Socket EOF / broken pipe | Mark devices disconnected, reconnect |
| Device disconnected | `device_removed` event | Remove from device map, notify registry |
| Frame rejected | Binary response `0x00` | Retry on next render tick; treat as unavailable device or invalid payload |
| Socket write timeout | 1s write deadline | Treat as disconnect, reconnect |
| Malformed JSON from blocksd | `serde_json` parse error | Log warning, skip message, continue |
| Version mismatch | `pong` version check | Treat as failed handshake and retry with backoff |

### 12.2 Graceful Degradation

The backend never causes Hypercolor to fail or stall:

- **Discovery** returns `Ok(vec![])` on any blocksd communication failure
- **`write_colors`** returns error but the render loop drops it (same as WLED UDP failures)
- **Disconnection** is non-blocking — devices simply stop updating
- **Reconnection** runs in the background with backoff — no blocking the render thread

### 12.3 Logging

```rust
// Connection events (info level)
tracing::info!("blocksd connected at {}", socket_path.display());
tracing::info!("blocksd disconnected, {} devices lost", device_count);

// Frame rejections (debug level — daemon rejected the write)
tracing::debug!("blocks frame rejected for device {uid}");

// Protocol errors (warn level)
tracing::warn!("blocksd sent malformed message: {err}");
```

---

## 13. Performance Budget

### 13.1 Latency Budget

```
Effect render tick          0 ms
  └─ Sampler (15×15 = 225 pixels)  ~0.01 ms
  └─ write_colors() entry          ~0.001 ms
  └─ Binary frame serialize        ~0.002 ms  (681 bytes, no alloc)
  └─ Unix socket write             ~0.01 ms   (local, no network)
  └─ blocksd receive + parse       ~0.05 ms
  └─ RGB888 → RGB565 conversion    ~0.005 ms  (225 pixels)
  └─ SharedDataChange diff         ~0.02 ms   (450 bytes vs previous)
  └─ 7-bit SysEx packing           ~0.01 ms
  └─ MIDI USB write                ~2-5 ms    (USB HID latency)
  └─ Device VM repaint             ~0-40 ms   (25 Hz timer)
────────────────────────────────────────────────
Total end-to-end:                   2-45 ms (dominated by USB + device timer)
```

### 13.2 Bandwidth

| Path | Data Size | Rate | Throughput |
|------|-----------|------|-----------|
| Hypercolor → blocksd | 681 B/frame | 25 fps | 17 KB/s |
| blocksd → device | ~100-450 B/frame (diff) | 25 fps | 2.5-11 KB/s |
| MIDI USB bandwidth | 31.25 KB/s nominal | — | Shared with touch events |

The diff compression in blocksd is critical: a full 450-byte RGB565 frame at 25 fps would consume
~11 KB/s of the ~31 KB/s MIDI bandwidth, leaving little room for touch events and control messages.
With diff encoding, typical updates (smooth color transitions) compress to 50–200 bytes.

### 13.3 Memory

- `BlocksBackend` struct: ~1 KB base
- Per device: ~500 B (`BlocksDevice` + `DeviceInfo` + zone data)
- Socket buffers: ~4 KB (read + write `BytesMut`)
- Frame coalescer: 675 B per active device
- Total for 4 devices: ~8 KB

Negligible compared to the effect render pipeline.

---

## 14. Testing Strategy

### 14.1 Unit Tests

**Mock blocksd server** — a test helper that binds a Unix socket and speaks the NDJSON + binary
protocol. Tests verify:

- Discovery request/response parsing
- Binary frame serialization (681-byte messages)
- JSON request/response round-trips
- Device ID determinism (same UID → same DeviceId)
- Brightness command encoding
- Event deserialization (touch, device_added, device_removed)

```rust
#[tokio::test]
async fn test_discover_parses_devices() {
    let (server, socket_path) = MockBlocksd::start().await;
    server.add_device(mock_lightpad(uid: 12345));

    let mut backend = BlocksBackend::new(socket_path);
    let devices = backend.discover().await.unwrap();

    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].family, DeviceFamily::Roli);
    assert_eq!(devices[0].zones[0].led_count, 225);
    assert_eq!(devices[0].zones[0].topology, DeviceTopologyHint::Matrix { rows: 15, cols: 15 });
}

#[tokio::test]
async fn test_binary_frame_format() {
    let (server, socket_path) = MockBlocksd::start().await;
    server.add_device(mock_lightpad(uid: 42));

    let mut backend = BlocksBackend::new(socket_path);
    backend.discover().await.unwrap();
    let device_id = backend.devices.keys().next().unwrap().clone();
    backend.connect(&device_id).await.unwrap();

    let colors: Vec<[u8; 3]> = vec![[255, 0, 0]; 225];
    backend.write_colors(&device_id, &colors).await.unwrap();

    let frame = server.last_frame(42).await;
    assert_eq!(frame.len(), 685);
    assert_eq!(frame[0], 0xBD);  // magic
    assert_eq!(frame[1], 0x01);  // type
    assert_eq!(u64::from_le_bytes(frame[2..10].try_into().unwrap()), 42);  // uid
    assert_eq!(&frame[10..13], &[255, 0, 0]);  // first pixel
}

#[tokio::test]
async fn test_device_id_determinism() {
    let id_a = device_id_from_uid(12345);
    let id_b = device_id_from_uid(12345);
    let id_c = device_id_from_uid(99999);

    assert_eq!(id_a, id_b);
    assert_ne!(id_a, id_c);
}
```

### 14.2 Integration Tests

Require blocksd running with at least one device connected. Gated behind `#[cfg(feature =
"blocks-integration")]` and an environment variable:

```rust
#[tokio::test]
#[cfg(feature = "blocks-integration")]
async fn test_real_device_solid_red() {
    let mut backend = BlocksBackend::new(BlocksBackend::default_socket_path());
    let devices = backend.discover().await.unwrap();
    assert!(!devices.is_empty(), "no ROLI blocks connected");

    let id = devices[0].id.clone();
    backend.connect(&id).await.unwrap();

    let red: Vec<[u8; 3]> = vec![[255, 0, 0]; 225];
    backend.write_colors(&id, &red).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;
    backend.disconnect(&id).await.unwrap();
}
```

### 14.3 Resilience Tests

- Backend returns `Ok(vec![])` when socket doesn't exist
- Backend reconnects after mock server restarts
- Backend handles malformed JSON without panicking
- Frame writes return error (not panic) when disconnected
- Backoff timing is respected between reconnection attempts

### 14.4 blocksd-Side Tests

The blocksd API server needs its own test suite (Python, pytest):

- Socket bind/accept lifecycle
- JSON request/response encoding
- Binary frame parsing (magic byte, UID extraction, pixel data)
- Concurrent client handling
- Graceful shutdown (in-flight requests complete)
- Event broadcast to multiple subscribers

---

## 15. Implementation Sequence

### Phase 1: blocksd API Server

**Scope:** Add an async Unix socket server to blocksd.
**Crate:** N/A (Python)
**Estimated files:** 3–4 new modules in blocksd

1. **`api/server.py`** — asyncio Unix socket server, accepts connections, manages client lifecycle
2. **`api/protocol.py`** — NDJSON + binary frame parser/serializer
3. **`api/handlers.py`** — request handlers (discover, frame, brightness, ping)
4. **`api/events.py`** — event subscription manager, broadcasts device/touch/button events
5. **Integration into `daemon.py`** — start API server alongside topology manager, wire up callbacks

**Key decisions:**
- Server starts on daemon boot, binds socket, accepts multiple concurrent clients
- Frame writes are non-blocking — accepted into a per-device buffer, applied on next tick
- Touch events are broadcast to all subscribed clients (fan-out)
- Binary frame path: peek first byte, dispatch to binary or JSON parser

### Phase 2: Hypercolor Backend

**Scope:** `BlocksBackend` implementing `DeviceBackend`.
**Crate:** `hypercolor-core`
**Module:** `hypercolor_core::device::blocks`

1. **`blocks/mod.rs`** — public re-exports
2. **`blocks/backend.rs`** — `BlocksBackend` struct + `DeviceBackend` impl
3. **`blocks/connection.rs`** — `BlocksConnection` socket management, NDJSON + binary I/O
4. **`blocks/types.rs`** — `RoliBlockType`, API message structs, `BlocksTouchEvent`
5. **`blocks/tests/`** — mock server + unit tests
6. **Registration in `hypercolor-daemon`** — add to `startup.rs` backend list

**Key decisions:**
- `ConnectionType::Bridge` — new semantics: out-of-process, may be unavailable
- `DeviceFamily::Roli` — new variant in `hypercolor-types`
- Binary frame path for `write_colors` — 681 bytes, no JSON overhead in hot path
- JSON for everything else — discovery, brightness, health checks
- Touch events as `Event::BlocksTouch` on the event bus

### Phase 3: Touch Integration (Stretch)

**Scope:** Reactive effect inputs from touch events.
**Crate:** `hypercolor-core` (event bus) + `hypercolor-sdk` (effect bindings)

1. **Event bus topic** — `blocks/touch` for touch events, `blocks/button` for button events
2. **Effect control binding** — map touch X/Y/Z to effect parameters via the control system
3. **SDK exposure** — TypeScript effects can subscribe to `blocks.touch` events
4. **Spatial feedback** — touch position maps to pixel position for direct visual feedback

This phase is independent of Phases 1–2 and can be deferred.

---

## Appendix A: blocksd API Message Reference

### Request Messages

| Type | Fields | Response Type |
|------|--------|---------------|
| `ping` | `id` | `pong` |
| `discover` | `id` | `discover_response` |
| `frame` | `uid`, `pixels` (base64) | `frame_ack` |
| `brightness` | `uid`, `value` (0–255) | `brightness_ack` |
| `subscribe` | `events` (string array) | `subscribed` |

### Binary Messages

| Magic | Type | Payload | Response |
|-------|------|---------|----------|
| `0xBD` | `0x01` | UID (8B LE) + RGB888 (675B) | `0x01` or `0x00` |

### Server-Sent Events

| Type | Fields | Trigger |
|------|--------|---------|
| `device_added` | `device` object | USB hot-plug or DNA snap |
| `device_removed` | `uid`, `reason` | Timeout or USB disconnect |
| `touch` | `uid`, `action`, `index`, `x`, `y`, `z`, `vx`, `vy`, `vz` | Touch surface interaction |
| `button` | `uid`, `action` | Mode button press/release |

## Appendix B: ROLI Protocol Quick Reference

For implementors working on blocksd's API server — key protocol constants:

| Constant | Value | Usage |
|----------|-------|-------|
| ROLI VID | `0x2AF4` | USB vendor ID |
| SysEx Header | `F0 00 21 10 77` | All ROLI messages |
| Heap LED Offset | 94 bytes | After BitmapLEDProgram |
| Frame Size (RGB565) | 450 bytes | 225 pixels × 2 bytes |
| Frame Size (RGB888) | 675 bytes | 225 pixels × 3 bytes |
| Max In-Flight | 200 bytes | ACK backpressure limit |
| Retransmit Timeout | 250 ms | ACK wait before resend |
| Device Repaint Rate | ~25 Hz | LittleFoot VM timer |
| Master Ping Interval | 400 ms | Keepalive for master block |
| DNA Ping Interval | 1666 ms | Keepalive for connected blocks |
| Ping Timeout | 5000 ms | Disconnect threshold |

## Appendix C: LUMI Keys Key-to-Pixel Mapping

The LUMI Keys Block uses the same 15×15 grid as the Lightpad, but the physical layout maps keys
to specific pixel regions. The 24 keys occupy rows 8–14 of the grid (the lower portion), with the
upper rows available for status indicators or custom visuals.

A future enhancement could provide a `LumiKeysLayout` that maps MIDI note numbers to pixel
coordinates, enabling per-key illumination that matches the musical keyboard layout. This would
require blocksd to expose MIDI note events alongside touch events.
