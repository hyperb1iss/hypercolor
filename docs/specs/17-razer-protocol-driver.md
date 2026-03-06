# 17 -- Razer Protocol Driver

> Native USB HID driver for Razer Chroma peripherals. Byte-level packet formats, protocol generation dispatch, matrix addressing, and clean-room integration with the HAL.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::razer`
**Author:** Nova
**Date:** 2026-03-04

---

## Table of Contents

1. [Overview](#1-overview)
2. [Packet Format](#2-packet-format)
3. [Protocol Generations](#3-protocol-generations)
4. [Command Reference](#4-command-reference)
5. [Matrix Addressing](#5-matrix-addressing)
6. [Target Devices](#6-target-devices)
7. [HAL Integration](#7-hal-integration)
8. [Render Pipeline](#8-render-pipeline)
9. [Testing Strategy](#9-testing-strategy)

---

## 1. Overview

Native USB HID driver for Razer peripherals via the `hypercolor-hal` abstraction layer. All Razer Chroma devices share a common 90-byte HID feature report protocol over USB control transfers, differentiated by a transaction ID byte that selects between protocol generations.

Clean-room implementation derived from publicly available protocol knowledge:
- OpenRGB's `RazerController` (C++)
- openrazer wiki reverse-engineering documentation
- uchroma's Rust HID report builder and Python protocol abstractions

**Vendor ID:** `0x1532` (Razer Inc.)

All devices use USB control transfers with HID feature reports — not interrupt endpoints — making `nusb` control transfer APIs the correct transport binding.

---

## 2. Packet Format

Every Razer command is a 90-byte (`0x5A`) HID feature report with the following layout:

```
Offset  Size  Field               Description
──────  ────  ──────────────────  ──────────────────────────────────────────
  0       1   status              0x00 on send; response code on read
  1       1   transaction_id      Protocol version selector (see §3)
  2       2   remaining_packets   LE u16 — multi-packet sequences (usually 0x0000)
  4       1   protocol_type       Always 0x00
  5       1   data_size           Number of valid bytes in arguments field
  6       1   command_class       Command class (0x00, 0x03, 0x07, 0x0F)
  7       1   command_id          Command ID within class
  8      80   arguments           Payload data (zero-padded beyond data_size)
 88       1   crc                 XOR checksum
 89       1   reserved            Always 0x00
```

### USB Control Transfer Parameters

Sent as HID feature reports via USB control transfers:

| Operation | bmRequestType | bRequest | wValue | wIndex |
|-----------|---------------|----------|--------|--------|
| SET_REPORT (send) | `0x21` | `0x09` | `0x0300` | interface number |
| GET_REPORT (recv) | `0xA1` | `0x01` | `0x0300` | interface number |

The `wValue` encodes `(HID_REPORT_TYPE_FEATURE << 8) | report_id` where `report_id = 0x00` for all standard devices. Some devices (Leviathan V2) use `report_id = 0x07`, yielding `wValue = 0x0307`.

### CRC Calculation

XOR fold of bytes `[1..86]` inclusive (transaction_id through arguments[78]):

```rust
/// Compute Razer report CRC.
///
/// XOR of bytes 1 through 86 (86 bytes total), covering
/// transaction_id, remaining_packets, protocol_type, data_size,
/// command_class, command_id, and the first 79 argument bytes.
pub fn razer_crc(buf: &[u8; 90]) -> u8 {
    buf[1..87].iter().fold(0u8, |acc, &b| acc ^ b)
}
```

This matches the OpenRazer/uchroma canonical formula. The CRC byte is stored at offset 88 before transmission.

For performance, the implementation may use 8-byte-at-a-time XOR folding via `u64` reinterpretation (reference: uchroma's `fast_crc_impl`).

### Response Status Codes

| Value | Status | Action |
|-------|--------|--------|
| `0x00` | Unknown | Treat as error |
| `0x01` | Busy | Retry up to 3× with 100ms backoff |
| `0x02` | Ok | Success — read response data from args |
| `0x03` | Fail | Error — command rejected |
| `0x04` | Timeout | Retry up to 3× with 100ms backoff |
| `0x05` | Unsupported | Command not supported (non-fatal) |

---

## 3. Protocol Generations

Razer devices span multiple protocol generations, distinguished by the `transaction_id` byte. This byte is fixed per device model, but it does not uniquely determine the lighting command family. Some devices mix a newer transaction ID with the older standard LED command set.

```rust
/// Razer protocol generation, determined by transaction_id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RazerProtocolVersion {
    /// Legacy protocol — transaction_id 0xFF.
    /// Older Chroma devices (BlackWidow 2014, DeathAdder Chroma).
    Legacy,

    /// Extended transaction family — transaction_id 0x3F.
    /// Huntsman V2, BlackWidow V3, Cynosa V2, Seiren Emote.
    Extended,

    /// Modern transaction family — transaction_id 0x1F.
    /// Basilisk V3, Cobra Pro, Blade 2021+, newest peripherals.
    Modern,

    /// Wireless keyboard transaction family — transaction_id 0x9F.
    /// DeathStalker V2 Pro wireless, Huntsman V2 wireless.
    WirelessKb,
}

impl RazerProtocolVersion {
    /// Transaction ID byte written to packet offset 1.
    pub fn transaction_id(self) -> u8 {
        match self {
            Self::Legacy     => 0xFF,
            Self::Extended   => 0x3F,
            Self::Modern     => 0x1F,
            Self::WirelessKb => 0x9F,
        }
    }
}

/// Lighting command family used for color/effect/brightness packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RazerLightingCommandSet {
    /// Standard LED commands (`0x03` class).
    Standard,
    /// Extended matrix commands (`0x0F` class).
    Extended,
}
```

### Transaction ID Mapping Table

| Transaction ID | Transaction Family | Lighting Command Family | Example Devices |
|---------------|----------|---------------|-----------------|
| `0xFF` | Legacy | Standard (`0x03`) | BlackWidow 2014, DeathAdder Chroma, Mamba 2015 |
| `0x3F` | Extended | Extended (`0x0F`) | Huntsman V2, BlackWidow V3, Cynosa V2 |
| `0x3F` | Extended | Extended (`0x0F`) | Seiren Emote (transported as 4 × 16, reported as 8 × 8) |
| `0x1F` | Modern | Extended (`0x0F`) | Basilisk V3, Cobra Pro, Viper V3 |
| `0x1F` | Modern | Standard (`0x03`) | Blade 15 (Late 2021 Advanced), some laptop keyboards |
| `0x9F` | Wireless KB | Extended (`0x0F`) | DeathStalker V2 Pro (wireless), Huntsman V2 (wireless) |

---

## 4. Command Reference

### 4.1 Command Classes

| Class | Purpose | Protocols |
|-------|---------|-----------|
| `0x00` | Device info, mode, polling rate | All |
| `0x03` | Standard LED control & effects | Legacy plus standard-command modern devices (for example Blade laptops) |
| `0x04` | DPI / mouse sensor | All (mice only) |
| `0x05` | Profile management | All |
| `0x07` | Power / battery | All (wireless only) |
| `0x0F` | Extended matrix effects & frames | Devices using the extended lighting command family across `0x3F`, `0x1F`, and `0x9F` transaction IDs |

### 4.2 Device Info Commands (Class 0x00)

| Command | ID | Data Size | Arguments |
|---------|----|-----------|-----------|
| Get firmware version | `0x81` | 2 | — (response: major, minor) |
| Get serial number | `0x82` | 22 | — (response: ASCII string) |
| Set device mode | `0x04` | 2 | `[mode, 0x00]` |
| Get device mode | `0x84` | 2 | — |

**Device mode values:**
- `0x00` — Hardware mode (device runs its own effects)
- `0x03` — Software/driver mode (host controls lighting)

### 4.3 Standard LED Commands (Class 0x03 — Legacy)

| Command | ID | Data Size | Arguments |
|---------|----|-----------|-----------|
| Set LED brightness | `0x03` | 3 | `[varstore, led_id, brightness]` |
| Get LED brightness | `0x83` | 3 | `[varstore, led_id, —]` |
| Set LED color | `0x01` | 5 | `[varstore, led_id, R, G, B]` |
| Set effect (activate) | `0x0A` | varies | `[effect_id, ...]` |
| Custom frame (matrix) | `0x0B` | varies | `[0xFF, row, start_col, stop_col, RGB...]` |
| Custom frame (linear) | `0x0C` | 50 | `[start_col, stop_col, RGB...]` |

### 4.4 Extended Matrix Commands (Class 0x0F — Extended/Modern)

| Command | ID | Data Size | Arguments |
|---------|----|-----------|-----------|
| Set brightness | `0x04` | 3 | `[varstore, led_id, brightness]` |
| Get brightness | `0x84` | 3 | `[varstore, led_id, —]` |
| Set effect | `0x02` | varies | `[varstore, led_id, effect_id, ...]` |
| Custom frame data | `0x03` | varies | `[0x00, 0x00, row, start_col, stop_col, RGB...]` |

**Extended effect IDs (class 0x0F, command 0x02):**

| Effect ID | Name | Additional Args |
|-----------|------|-----------------|
| `0x00` | Off / None | — |
| `0x01` | Static | R, G, B |
| `0x02` | Breathing | mode, [R, G, B, ...] |
| `0x03` | Spectrum Cycle | — |
| `0x04` | Wave | direction byte |
| `0x05` | Reactive | speed, R, G, B |
| `0x08` | Custom Frame | — (activates per-key mode) |

### 4.5 Storage Flags

| Flag | Value | Behavior |
|------|-------|----------|
| `NOSTORE` | `0x00` | Ephemeral — lost on power cycle or USB disconnect |
| `VARSTORE` | `0x01` | Persisted to device flash memory |

Hypercolor uses `NOSTORE` exclusively for real-time rendering. `VARSTORE` is reserved for user-initiated "save to device" operations.

### 4.6 LED ID Constants

| Constant | Value | Used By |
|----------|-------|---------|
| `ZERO_LED` | `0x00` | Mice (Basilisk V3, DeathAdder, Viper) |
| `SCROLL_WHEEL` | `0x01` | Mice (standalone scroll LED) |
| `BATTERY` | `0x03` | Wireless devices |
| `LOGO` | `0x04` | Mice, headsets (standalone logo LED) |
| `BACKLIGHT` | `0x05` | All keyboards (key matrix backlight) |
| `MACRO` | `0x07` | Keyboards with macro keys |
| `RIGHT_SIDE` | `0x10` | Laptops (right LED strip) |
| `LEFT_SIDE` | `0x11` | Laptops (left LED strip) |
| `ARGB_CH_1..6` | `0x1A..0x1F` | ARGB controller channels |

---

## 5. Matrix Addressing

Razer devices use row/column addressing for their LED matrices. The frame data commands carry per-row LED data segments.

### 5.1 Matrix Types

```rust
/// Matrix addressing mode for a Razer device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RazerMatrixType {
    /// No per-key matrix — use single-color or zone effects only.
    None,

    /// Standard matrix — command class 0x03, id 0x0B.
    /// Older keyboards (BlackWidow 2016, Ornata).
    Standard,

    /// Extended matrix — command class 0x0F, id 0x03.
    /// Modern keyboards and mice (Huntsman V2, Basilisk V3).
    Extended,

    /// Linear (single-row) — command class 0x03, id 0x0C.
    /// Strips, mousepads, single-zone accessories.
    Linear,

    /// Extended ARGB — command class 0x0F, id 0x03 with ARGB LED IDs.
    /// Addressable RGB controller channels.
    ExtendedArgb,
}
```

### 5.2 Frame Data Layouts

**Standard matrix (class 0x03, id 0x0B):**

```
args[0]    = 0xFF          (flag byte)
args[1]    = row_index
args[2]    = start_col
args[3]    = stop_col
args[4..]  = RGB data      (3 bytes per LED: R, G, B)
data_size  = (stop_col - start_col + 1) * 3 + 4
```

**Extended matrix (class 0x0F, id 0x03):**

```
args[0]    = 0x00          (reserved)
args[1]    = 0x00          (reserved)
args[2]    = row_index
args[3]    = start_col
args[4]    = stop_col
args[5..]  = RGB data      (3 bytes per LED: R, G, B)
data_size  = (stop_col - start_col + 1) * 3 + 5
```

**Linear (class 0x03, id 0x0C):**

```
args[0]    = start_col
args[1]    = stop_col
args[2..]  = RGB data
data_size  = 0x32 (50 bytes fixed)
```

### 5.3 Packet Capacity

The arguments field is 80 bytes. After header bytes, the maximum LED data per packet:

| Matrix Type | Header Bytes | RGB Capacity | Max LEDs/Packet |
|-------------|-------------|-------------|-----------------|
| Standard | 4 | 76 bytes | **25 LEDs** |
| Extended | 5 | 75 bytes | **25 LEDs** |
| Linear | 2 | 50 bytes (fixed) | **16 LEDs** |

For keyboards with more than 25 columns, each row must be split across multiple packets.

### 5.4 Multi-Row Frame Commit Sequence

For a full-frame update on a keyboard (e.g., 6×22 Huntsman V2):

```
1. For each row 0..5:
   a. Build frame data packet with (row, 0, 21, RGB[0..21])
   b. Send SET_REPORT
   c. Sleep 1ms
2. Send custom frame activation:
   - Extended: class 0x0F, id 0x02, args [NOSTORE, led_id, 0x08]
   - Standard: class 0x03, id 0x0A, args [0x05, NOSTORE]
```

---

## 6. Target Devices

### 6.1 Priority Devices (Bliss's Hardware)

#### Razer Huntsman V2 (Full-Size)

| Field | Value |
|-------|-------|
| PID | `0x026C` |
| Protocol | Extended (`0x3F`) |
| Matrix | Extended, 6 rows × 22 columns |
| LED ID | `BACKLIGHT` (`0x05`) |
| Interface | 3 |
| Total LEDs | 132 |

#### Razer Basilisk V3

| Field | Value |
|-------|-------|
| PID | `0x0099` |
| Protocol | Modern (`0x1F`) |
| Matrix | Extended, 1 row × 11 columns |
| LED ID | `ZERO_LED` (`0x00`) |
| Interface | 3 |
| Zones | Logo (1 LED) + Scroll Wheel (1) + Underglow Strip (9) |
| Total LEDs | 11 |

Note: The Basilisk V3 does not support hardware breathing — Hypercolor must render breathing effects in software.

#### Razer Seiren V3 Chroma

| Field | Value |
|-------|-------|
| PID | `0x056F` (estimated) |
| Protocol | TBD — likely Modern (`0x1F`) |
| Matrix | TBD — likely small LED ring, estimated 8-16 LEDs |
| LED ID | TBD |
| Interface | TBD |
| Status | **Needs USB capture verification** |

The Seiren V3 Chroma is not present in OpenRGB or openrazer databases as of this writing. The only Seiren variant with known protocol data is the Seiren Emote (`0x0F1B`, Extended transaction family, extended lighting commands, 4 × 16 transport geometry exposed as an 8 × 8 matrix). The V3 Chroma likely uses the Modern protocol (`0x1F`) with a small LED matrix for its RGB ring. Implementation will require USB traffic capture from L-Connect or Synapse for protocol verification.

### 6.2 PID Registry (Broader Coverage)

**Keyboards:**

| Device | PID | Transaction ID | Matrix | Rows × Cols |
|--------|-----|---------------|--------|-------------|
| BlackWidow V3 | `0x0268` | `0x3F` | Extended | 6 × 22 |
| BlackWidow V3 TKL | `0x0269` | `0x3F` | Extended | 6 × 19 |
| BlackWidow V4 | `0x028D` | `0x3F` | Extended | 6 × 22 |
| Huntsman V2 TKL | `0x026B` | `0x3F` | Extended | 6 × 19 |
| Huntsman V3 Pro | `0x02A6` | `0x3F` | Extended | 6 × 22 |
| Huntsman V3 Pro TKL | `0x02A7` | `0x3F` | Extended | 6 × 19 |
| Cynosa V2 | `0x025E` | `0x3F` | Extended | 6 × 22 |
| DeathStalker V2 | `0x0295` | `0x3F` | Extended | 1 × 8 |
| DeathStalker V2 Pro (wireless) | `0x0296` | `0x9F` | Extended | 6 × 22 |
| Ornata V3 | `0x02A1` | `0x3F` | Extended | 6 × 22 |

**Mice:**

| Device | PID | Transaction ID | Matrix | LEDs |
|--------|-----|---------------|--------|------|
| Basilisk V3 35K | `0x00CB` | `0x1F` | Extended | 1 × 11 |
| Basilisk V3 Pro (wired) | `0x00AA` | `0x1F` | Extended | 1 × 13 |
| Cobra Pro (wired) | `0x00AF` | `0x1F` | Extended | 1 × 11 |
| DeathAdder V3 Pro (wired) | `0x00B6` | `0x1F` | Extended | 1 × 1 |
| Viper V3 Pro (wired) | `0x00C3` | `0x1F` | Extended | 1 × 6 |
| Naga V2 Pro (wired) | `0x00C7` | `0x1F` | Extended | 1 × 3 |

**Other:**

| Device | PID | Transaction ID | Notes |
|--------|-----|---------------|-------|
| Seiren Emote | `0x0F1B` | `0x3F` | Microphone, 4 × 16 transport geometry, reported as 8 × 8 (64 LEDs) |
| Firefly V2 | `0x008A` | `0x3F` | Mousepad, 1 × 20 |
| Chroma Addressable RGB Controller | `0x0F1F` | `0x3F` | 6 ARGB channels |
| Charging Pad Chroma | `0x0F26` | `0x3F` | 1 × 10 |

---

## 7. HAL Integration

### 7.1 Protocol Implementation

`RazerProtocol` implements the HAL `Protocol` trait as a **pure byte encoder/decoder** with zero I/O:

```rust
/// Razer protocol encoder/decoder.
///
/// Handles packet construction, CRC, and response parsing.
/// Contains no I/O — all USB communication flows through
/// the transport layer.
pub struct RazerProtocol {
    /// Protocol generation for this device.
    version: RazerProtocolVersion,
    /// Lighting command family used for non-mode packets.
    command_set: RazerLightingCommandSet,
    /// Matrix addressing mode.
    matrix_type: RazerMatrixType,
    /// Matrix dimensions (rows, columns).
    matrix_size: (u8, u8),
    /// Optional user-facing dimensions when transport geometry differs.
    reported_matrix_size: Option<(u8, u8)>,
    /// Primary LED ID for this device.
    led_id: u8,
}
```

### 7.2 Transport

On Linux, Hypercolor uses `TransportType::UsbHidRaw` for Razer devices so the kernel HID driver can stay attached while feature reports flow through `/dev/hidraw*`:

```rust
/// Linux HIDRAW transport for Razer devices.
///
/// Sends/receives 90-byte feature reports through hidapi.
pub struct UsbHidRawTransport {
    /// Open `/dev/hidraw*` node path.
    device_path: String,
    /// Report ID (0x00 for most devices, 0x07 for Leviathan V2).
    report_id: u8,
    /// Maximum feature report length.
    max_packet_len: usize,
}
```

### 7.3 Device Descriptor

Each supported device is described by a static descriptor mapping VID/PID to protocol parameters:

```rust
/// Static descriptor for a known Razer device.
pub struct DeviceDescriptor {
    /// USB vendor ID.
    pub vendor_id: u16,
    /// USB product ID.
    pub product_id: u16,
    /// Human-readable device name.
    pub name: &'static str,
    /// Device family classification.
    pub family: DeviceFamily,
    /// Transport binding used to reach the device.
    pub transport: TransportType,
    /// Protocol constructor and stable identifier.
    pub protocol: ProtocolBinding,
}
```

### 7.4 Device Family

Requires a new `DeviceFamily::Razer` variant in `hypercolor-types`:

```rust
pub enum DeviceFamily {
    OpenRgb,
    Wled,
    Hue,
    Razer,  // new
    Custom(String),
}
```

### 7.5 Protocol Database Registration

```rust
// Example registration entries
razer_device!(HUNTSMAN_V2, 0x026C, "Razer Huntsman V2",
    Extended, Extended, (6, 22), 3, BACKLIGHT);
razer_device!(BASILISK_V3, 0x0099, "Razer Basilisk V3",
    Modern, Extended, (1, 11), 3, ZERO_LED);
```

---

## 8. Render Pipeline

### 8.1 Full-Frame Update (Keyboard)

For a device with an N-row matrix:

```
┌─────────────────────────────────────────────┐
│ 1. Build brightness-scaled RGB buffer       │
│ 2. For each row 0..N-1:                     │
│    a. Encode frame data packet (row, cols)   │
│    b. SET_REPORT (send packet)              │
│    c. Sleep 1ms                             │
│ 3. Send custom frame activation command     │
└─────────────────────────────────────────────┘
```

### 8.2 Single-Row Update (Mouse / Accessory)

For linear devices like the Basilisk V3:

```
┌─────────────────────────────────────────────┐
│ 1. Build brightness-scaled RGB buffer       │
│ 2. Encode single frame data packet          │
│    (row=0, start_col=0, stop_col=N-1)       │
│ 3. SET_REPORT (send packet)                 │
│ 4. Send custom frame activation command     │
└─────────────────────────────────────────────┘
```

### 8.3 Timing Budget

| Operation | Duration | Source |
|-----------|----------|--------|
| Inter-command delay | 7ms | uchroma `CMD_DELAY_MS` |
| Inter-row delay | 1ms | OpenRGB `SetLEDs()` |
| Response read timeout | 1000ms | USB control transfer timeout |
| Busy/Timeout retry backoff | 100ms | uchroma retry logic |

For a 6-row keyboard at 60fps (16.6ms budget):
- 6 row packets × 1ms inter-row = 6ms
- 1 activation packet = ~1ms
- Total: ~7ms per frame — fits within the 16.6ms budget

### 8.4 Brightness Scaling

Brightness is applied host-side before frame commit. The RGB values in the frame data are pre-multiplied by the brightness factor (0.0–1.0), avoiding a separate brightness command round-trip.

For devices that only support zone-level brightness (no per-key), use the extended brightness command (class 0x0F, id 0x04) as a fallback.

---

## 9. Testing Strategy

### 9.1 Mock Transport

A mock transport records all control transfers and can replay canned responses:

```rust
/// Mock USB transport for testing.
///
/// Records SET_REPORT calls and returns pre-configured
/// GET_REPORT responses for protocol validation.
pub struct MockRazerTransport {
    /// Recorded outgoing packets.
    sent: Vec<[u8; 90]>,
    /// Pre-configured responses keyed by (command_class, command_id).
    responses: HashMap<(u8, u8), [u8; 90]>,
}
```

### 9.2 Test Categories

**CRC validation:**
- Compute CRC for known-good packets from uchroma test vectors
- Verify round-trip: build packet → compute CRC → validate CRC → pass

**Command encoding:**
- Round-trip encode/decode for all command types in §4
- Verify packet layout byte-by-byte against reference implementations
- Protocol version selection: correct transaction_id and command_class per device

**Frame data serialization:**
- Standard matrix: verify row/col addressing and RGB packing
- Extended matrix: verify 2-byte reserved prefix
- Linear: verify fixed 50-byte data_size
- Multi-row splitting when column count > 25

**Device descriptor validation:**
- All PIDs in registry resolve to valid descriptors
- Matrix size × 3 bytes fits within packet capacity constraints
- Protocol version → command class mapping is consistent

---

## References

- `~/dev/OpenRGB/Controllers/RazerController/` — C++ protocol implementation
- `~/dev/OpenRGB/Controllers/RazerController/RazerDevices.cpp` — device registry with matrix sizes
- `~/dev/uchroma/rust/hid/report.rs` — Rust report builder
- `~/dev/uchroma/rust/crc.rs` — CRC implementation (SIMD-style u64 folding)
- `~/dev/uchroma/uchroma/server/protocol.py` — protocol version abstraction
- `~/dev/uchroma/uchroma/server/commands.py` — command class/ID definitions
- openrazer wiki: Reverse Engineering USB Protocol
