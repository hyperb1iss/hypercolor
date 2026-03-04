# 19 -- Lian Li Uni Hub Protocol Driver

> Native USB HID driver for the entire Lian Li Uni Hub family. Two transport modes, R-B-G color ordering, eight channels of SL Infinity goodness, and white LED protection baked in.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::lianli`
**Author:** Nova
**Date:** 2026-03-04

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Protocol Architecture — Two Transport Modes](#3-protocol-architecture--two-transport-modes)
4. [Modern HID Protocol Detail](#4-modern-hid-protocol-detail)
5. [SL Infinity Specifics](#5-sl-infinity-specifics)
6. [Fan Speed Control](#6-fan-speed-control)
7. [White Color Protection](#7-white-color-protection)
8. [HAL Integration](#8-hal-integration)
9. [Relationship to PrismRGB (Spec 04)](#9-relationship-to-prismrgb-spec-04)
10. [Testing Strategy](#10-testing-strategy)

---

## 1. Overview

Native USB HID driver for Lian Li's Uni Fan hub controller family. These hubs centralize RGB and fan control for Lian Li's Uni Fan ecosystem — SL, AL, SL V2, AL V2, and SL Infinity fans.

Clean-room implementation derived from:
- OpenRGB's ENE-based controller implementations (`LianLiController/` — 10 subdirectories)
- uni-sync Rust crate by EightB1ts
- L-Connect 3 × OpenRGB Beta documentation

**Primary VID:** `0x0CF2` (ENE Technology) — all Uni Hub variants.

**Primary target:** SL Infinity (`0xA102`) — Bliss's active hardware. Full coverage of all variants for completeness.

The protocol has two distinct transport families:
- **Original Hub** (`0x7750`): libusb vendor-specific control transfers with register-addressed wIndex
- **Modern Hubs** (`0xA100`+): HID reports with a 3-phase packet protocol

---

## 2. Device Registry

### 2.1 Modern HID Hubs (VID `0x0CF2`)

| Controller | PID | Interface | Usage Page | Usage | LEDs/Fan | Max Fans/Ch | Channels | Packet Size |
|---|---|---|---|---|---|---|---|---|
| Uni Hub SL | `0xA100` | 1 | `0xFF72` | `0xA1` | 16 | 4 | 4 | 11B |
| Uni Hub AL | `0xA101` | 1 | `0xFF72` | `0xA1` | 8+12 (fan+edge) | 4 | 4 | 65B |
| Uni Hub SL Infinity | `0xA102` | 1 | `0xFF72` | `0xA1` | 16 | 6 | **8** | 65B |
| Uni Hub SL V2 | `0xA103` | 1 | `0xFF72` | `0xA1` | 16 | 6 | 4 | 65B |
| Uni Hub AL V2 | `0xA104` | 1 | `0xFF72` | `0xA1` | 8+12 | 4 | 4 | 65B |
| Uni Hub SL V2 v0.5 | `0xA105` | 1 | `0xFF72` | `0xA1` | 16 | 6 | 4 | 65B |
| Strimer L Connect | `0xA200` | 1 | `0xFF72` | `0xA1` | variable | — | — | 65B |

### 2.2 Legacy Hubs

| Controller | PID | Transport | LEDs/Fan | Channels |
|---|---|---|---|---|
| Uni Hub (original) | `0x7750` | libusb control transfer | 16 | 4 |

### 2.3 Other Lian Li (VID `0x0416`, Nuvoton)

| Controller | PID | Interface | Notes |
|---|---|---|---|
| GA II Trinity | `0x7373` | 2 | GPU cooler, separate protocol |
| GA II Trinity Perf | `0x7371` | 2 | GPU cooler, separate protocol |

### 2.4 Firmware Version Gating

Some hubs dispatch to different drivers based on firmware version read from the USB product string:

| PID | Firmware | Driver |
|-----|----------|--------|
| `0xA100` | `v1.8` only | SL HID driver |
| `0xA101` | `v1.7` | AL HID driver |
| `0xA101` | `v1.0` | AL10 libusb driver (falls back to control transfers) |

SLV2, ALV2, and SL Infinity accept all firmware versions.

---

## 3. Protocol Architecture — Two Transport Modes

### 3.1 Original Hub (PID `0x7750`) — Libusb Control Transfers

The original Uni Hub uses vendor-specific USB control transfers with register-addressed wIndex values. No HID report protocol — instead, individual config writes to memory-mapped addresses followed by a commit write.

**Control transfer parameters:**

| Operation | bmRequestType | bRequest | wValue | wIndex | Data |
|-----------|---------------|----------|--------|--------|------|
| Write | `0x40` | `0x80` | `0x0000` | register address | config bytes |
| Read | `0xC0` | `0x81` | `0x0000` | register address | response buffer |

**Register address map (per channel):**

| Register | Ch 1 | Ch 2 | Ch 3 | Ch 4 | Purpose |
|----------|------|------|------|------|---------|
| LED data | `0xE300` | `0xE3C0` | `0xE480` | `0xE540` | 192-byte color buffer |
| Mode | `0xE021` | `0xE031` | `0xE041` | `0xE051` | Effect mode byte |
| Speed | `0xE022` | `0xE032` | `0xE042` | `0xE052` | Speed byte |
| Direction | `0xE023` | `0xE033` | `0xE043` | `0xE053` | Direction byte |
| Brightness | `0xE029` | `0xE039` | `0xE049` | `0xE059` | Brightness byte |
| Commit | `0xE02F` | `0xE03F` | `0xE04F` | `0xE05F` | Write `0x01` to apply |
| Firmware | `0xB500` | — | — | — | 5-byte version (read) |

**Color buffer:** 192 bytes = 64 LEDs × 3 bytes (R-B-G order).

**Timing:** 5ms delay after every control transfer write.

### 3.2 Modern Hubs (PIDs `0xA100`+) — HID Reports

All modern hubs use HID reports with a 3-phase packet protocol per channel:

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│ Phase 1:     │     │ Phase 2:     │     │ Phase 3:     │
│ Activate     │ ──▷ │ Color Data   │ ──▷ │ Commit       │
│ (StartAction)│     │ (SendColor)  │     │ (CommitAction)│
└──────────────┘     └──────────────┘     └──────────────┘
      5ms                  5ms                  5ms
```

Every packet starts with transaction ID `0xE0` at byte 0.

**HID detection:** Interface `0x01`, usage page `0xFF72`, usage `0xA1`.

**Timing:** 5ms minimum delay after every `hid_write()` call.

---

## 4. Modern HID Protocol Detail

### 4.1 Phase 1 — Activate (SendStartAction)

Announces how many fans are connected on a channel. Must be sent before color data.

**Packet format:**

```
Byte  SL (0xA100)    AL (0xA101)     SLV2/INF (0xA102-05)
────  ─────────────  ──────────────  ─────────────────────
 [0]  0xE0           0xE0            0xE0
 [1]  0x10           0x10            0x10
 [2]  0x32           0x40            0x60
 [3]  channel×16     channel+1       (channel<<4)+fans
      + num_fans
 [4]  —              num_fans        —
```

**Sub-command byte summary:**

| Hub Variant | Activate Sub-Command |
|------------|---------------------|
| SL (`0xA100`) | `0x32` |
| AL (`0xA101`) | `0x40` |
| SL V2 / AL V2 / SL V2 v0.5 | `0x60` |
| SL Infinity | `0x60` |

**Channel encoding examples (SL V2 / SL Infinity):**

| Channel | Fans | Byte[3] | Calculation |
|---------|------|---------|-------------|
| 0 | 3 | `0x03` | `(0 << 4) + 3` |
| 1 | 6 | `0x16` | `(1 << 4) + 6` |
| 5 | 4 | `0x54` | `(5 << 4) + 4` |

**SL Infinity exception:** The start action maps pairs of logical channels to one physical fan array:

```
buf[3] = 1 + (channel / 2)    // channels 0,1 → 1; channels 2,3 → 2; etc.
buf[4] = 0x04                  // hardcoded (known limitation in OpenRGB)
```

**Packet sizes:** SL = 11 bytes. AL, SLV2, SL Infinity = 65 bytes.

### 4.2 Phase 2 — Color Data (SendColorData)

Sends LED color values for a channel. **Critical: color byte order is R-B-G, NOT R-G-B.**

**Packet format:**

```
Byte  All variants
────  ─────────────────────────────────
 [0]  0xE0
 [1]  0x30 + <channel_offset>
 [2+] R, B, G, R, B, G, ...  (per LED)
```

**Channel offset calculation:**

| Hub | Offset Formula | Example (ch 2, fan zone) |
|-----|---------------|-------------------------|
| SL | `channel` | `0x32` |
| AL | `fan_or_edge + (channel × 2)` | `0x34` (fan), `0x35` (edge) |
| SLV2 / SL Infinity | `channel` | `0x32` |

The AL series has **two sub-zones per channel**: fan LEDs (8/fan) and edge LEDs (12/fan). Each sub-zone gets its own color data packet with separate offsets.

**Packet sizes:**

| Hub | Color Packet Size | Max Payload |
|-----|------------------|-------------|
| SL (`0xA100`) | 2 + (num_LEDs × 3) dynamic | 4 fans × 16 LEDs × 3 = 192B |
| AL (`0xA101`) | 146 bytes | 4 fans × (8+12) LEDs × 3 = 240B |
| SLV2 / SL Infinity | 353 bytes | 6 fans × 16 LEDs × 3 = 288B |

### 4.3 Phase 3 — Commit (SendCommitAction)

Activates the lighting configuration for a channel.

**Packet format:**

```
Byte  All variants
────  ───────────────────────────────
 [0]  0xE0
 [1]  0x10 + <channel_offset>       // same offset scheme as §4.2 but base 0x10
 [2]  effect                         // mode byte (see §4.4)
 [3]  speed                          // speed byte
 [4]  direction                      // direction byte
 [5]  brightness                     // brightness byte
```

**Channel offset calculation:** Same as color data, but with base `0x10` instead of `0x30`.

### 4.4 Effect Byte Tables

#### SLV2 / SL Infinity Effects (Primary)

| Effect | Byte | Colors | Direction | Notes |
|--------|------|--------|-----------|-------|
| Static Color | `0x01` | full array | — | All LEDs set to specified colors |
| Breathing | `0x02` | full array | — | Fade in/out cycle |
| Rainbow Morph | `0x04` | — | — | Spectrum shift |
| Rainbow Wave | `0x05` | — | yes | Traveling rainbow |
| Staggered | `0x18` | 2 | — | Alternating segments |
| Tide | `0x1A` | 2 | — | Wave pattern |
| Runway | `0x1C` | 2 | — | Chase pattern |
| Mixing | `0x1E` | 2 | — | Color blend |
| Stack | `0x20` | 1 | yes | Stacking animation |
| Stack Multi Color | `0x21` | — | yes | Rainbow stack |
| Neon | `0x22` | — | — | Neon pulse |
| Color Cycle | `0x23` | 3 | yes | Rotate through colors |
| Meteor | `0x24` | 2 | — | Shooting star |
| Voice | `0x26` | — | — | Audio reactive |
| Groove | `0x27` | 2 | yes | Rhythmic pattern |
| Render | `0x28` | 4 | yes | Multi-color sweep |
| Tunnel | `0x29` | 4 | yes | Tunnel vision effect |

#### SL Infinity Merged Effects

These mode bytes produce synchronized effects spanning all channels:

| Effect | Byte | Notes |
|--------|------|-------|
| Meteor Merged | `0x2A` | Cross-channel meteor |
| Runway Merged | `0x2B` | Cross-channel chase |
| Tide Merged | `0x2C` | Cross-channel tide |
| Mixing Merged | `0x2D` | Cross-channel blend |
| Stack Multi Color Merged | `0x2E` | Cross-channel rainbow stack |

#### SL Effects (Original)

The SL (`0xA100`) uses the same byte values as SLV2 where they overlap. Merged modes on the SL use a separate `SendMerge` packet (sub-command `0x33`) rather than distinct mode bytes.

#### AL Effects

The AL (`0xA101`) uses a different byte mapping:

| Effect | Byte |
|--------|------|
| Static | `0x01` |
| Breathing | `0x02` |
| Rainbow Wave | `0x28` |
| Rainbow Morph | `0x35` |
| Meteor | `0x19` |
| Runway | `0x1A` |
| Mixing | `0x2F` |
| Stack | `0x30` |
| Tide | `0x31` |
| Color Cycle | `0x2B` |
| Staggered | `0x37` |
| Neon (Taichi) | `0x2C` |
| Voice | `0x2E` |
| Tornado | `0x36` |

### 4.5 Speed Byte Encoding

| Level | Byte | Percentage |
|-------|------|-----------|
| Slowest | `0x02` | 0% |
| Slow | `0x01` | 25% |
| Medium | `0x00` | 50% |
| Fast | `0xFF` | 75% |
| Fastest | `0xFE` | 100% |

Note: The original hub (`0x7750`) uses different values: `0x04` (slowest) through `0x00` (fastest).

### 4.6 Brightness Byte Encoding

| Level | Byte | Percentage |
|-------|------|-----------|
| 100% (brightest) | `0x00` | Full |
| 75% | `0x01` | — |
| 50% | `0x02` | — |
| 25% | `0x03` | — |
| Off | `0x08` | Black |

### 4.7 Direction Byte

| Direction | Byte |
|-----------|------|
| Left to Right | `0x00` |
| Right to Left | `0x01` |

---

## 5. SL Infinity Specifics

The SL Infinity is the most capable hub variant with double the channel count and support for merged cross-channel effects.

### 5.1 Key Specifications

| Feature | Value |
|---------|-------|
| PID | `0xA102` |
| Channels | **8** (vs. 4 on all other hubs) |
| Max fans per channel | 6 |
| LEDs per fan | 16 |
| Max total LEDs | 8 × 6 × 16 = **768** |
| Sub-command | `0x60` (same as SLV2) |
| Color packet size | 353 bytes |

### 5.2 Dual-Channel Fan Addressing

The SL Infinity's 8 logical channels map to 4 physical fan groups. Each fan has two LED zones (spinner ring and side band), addressed as separate channels:

```
Logical channels 0, 1 → Physical fan group 1
Logical channels 2, 3 → Physical fan group 2
Logical channels 4, 5 → Physical fan group 3
Logical channels 6, 7 → Physical fan group 4
```

This enables independent control of the spinner and side LED zones on SL Infinity fans.

### 5.3 Merged Effects

Merged effects (mode bytes `0x2A`–`0x2E`) synchronize animations across all channels. When a merged mode is active, the hub coordinates timing across all connected fans for seamless cross-channel effects.

Unlike the original SL hub which uses a separate `SendMerge` packet, the SL Infinity encodes the merge state directly in the effect byte — no additional protocol step required.

---

## 6. Fan Speed Control

Fan speed control is available on all hub variants alongside RGB. The protocol uses dedicated packets separate from the lighting 3-phase sequence.

### 6.1 RGB Sync Toggle

Enable/disable fan speed syncing with RGB effects:

```
[0] 0xE0
[1] 0x10
[2] <model_byte>     // hub variant identifier
[3] <sync: 0 or 1>   // 0 = independent, 1 = synced
```

### 6.2 Channel Mode (PWM vs Manual)

```
[0] 0xE0
[1] 0x10
[2] <model_byte>
[3] <channel_bits>   // bitmask of channels in manual mode
```

### 6.3 Manual Speed Set

```
[0] 0xE0
[1] <channel + 0x20>
[2] 0x00
[3] <speed_byte>
```

Speed formulas vary by model:
- **SL/AL:** Direct byte value (0x00–0xFF range)
- **SLV2/ALV2:** Percentage-mapped byte
- **SL Infinity:** Same as SLV2 formula

### 6.4 RPM Reading

Fan RPM data is available via read operations. The original hub uses `wIndex` addresses `0xE800`–`0xE806` for per-channel RPM reads. Modern hubs report RPM through the HID channel.

---

## 7. White Color Protection

Lian Li Uni Hubs include a hardware protection mechanism to prevent LED damage from sustained maximum white output.

### 7.1 Sum-Based Limiter (SLV2 / SL Infinity)

Applied per-LED before color data is packed:

```rust
/// Apply white color protection for SLV2/SL Infinity hubs.
///
/// If the sum of R + G + B exceeds 460, all channels are
/// proportionally scaled down to bring the sum to 460.
fn brightness_limit(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let sum = r as u16 + g as u16 + b as u16;
    if sum > 460 {
        let scale = 460.0 / sum as f32;
        (
            (r as f32 * scale) as u8,
            (g as f32 * scale) as u8,
            (b as f32 * scale) as u8,
        )
    } else {
        (r, g, b)
    }
}
```

### 7.2 Equality-Based Limiter (AL / Original Hub)

Simpler check — only triggers on pure white/grey where all channels are equal and exceed 153:

```rust
/// Apply white color protection for AL hubs.
///
/// Clamps pure white/grey values (R == G == B) to a maximum
/// of (153, 153, 153) to prevent LED damage.
fn al_brightness_limit(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    if r > 153 && r == g && g == b {
        (153, 153, 153)
    } else {
        (r, g, b)
    }
}
```

### 7.3 SL (Original)

The SL hub (`0xA100`) has no software-side white limiter — it relies on the 5-level brightness scale for protection.

### 7.4 Limiter Application

The limiter is applied **before** R-B-G reordering and packing into the color data buffer. It operates on standard RGB values.

---

## 8. HAL Integration

### 8.1 Hub Variant Enum

```rust
/// Lian Li Uni Hub hardware variant.
///
/// Parameterizes packet format differences across the hub family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LianLiHubVariant {
    /// Original hub (0x7750) — libusb control transfers.
    Original,
    /// SL hub (0xA100) — 11-byte HID packets.
    Sl,
    /// AL hub (0xA101) — 65-byte HID, fan+edge sub-zones.
    Al,
    /// SL V2 / AL V2 / SL V2 v0.5 (0xA103-0xA105) — 65-byte HID.
    SlV2,
    /// SL Infinity (0xA102) — 65-byte HID, 8 channels.
    SlInfinity,
}

impl LianLiHubVariant {
    /// Activate sub-command byte for Phase 1.
    pub fn activate_sub_cmd(self) -> u8 {
        match self {
            Self::Original => 0x00, // not used (libusb path)
            Self::Sl => 0x32,
            Self::Al => 0x40,
            Self::SlV2 | Self::SlInfinity => 0x60,
        }
    }

    /// Number of channels supported.
    pub fn channel_count(self) -> u8 {
        match self {
            Self::SlInfinity => 8,
            _ => 4,
        }
    }

    /// Whether this variant has separate fan/edge sub-zones.
    pub fn has_edge_zones(self) -> bool {
        matches!(self, Self::Al)
    }

    /// Color packet size in bytes.
    pub fn color_packet_size(self) -> usize {
        match self {
            Self::Original => 192,
            Self::Sl => 0, // dynamic
            Self::Al => 146,
            Self::SlV2 | Self::SlInfinity => 353,
        }
    }
}
```

### 8.2 Protocol Implementation

```rust
/// Lian Li protocol encoder/decoder.
///
/// Pure byte encoding for the 3-phase HID protocol.
/// Parameterized by hub variant to handle packet format
/// differences. Contains no I/O.
pub struct LianLiProtocol {
    /// Hub hardware variant.
    variant: LianLiHubVariant,
    /// Per-channel fan counts.
    fan_counts: [u8; 8],
}
```

### 8.3 Transport

Two transport implementations based on hub type:

```rust
/// HID transport for modern Lian Li hubs (0xA100+).
pub struct LianLiHidTransport {
    device: nusb::Device,
    interface: u8,  // always 1
}

/// Libusb control transfer transport for original hub (0x7750).
pub struct LianLiCtrlTransport {
    device: nusb::Device,
}
```

### 8.4 Device Family

Requires a new `DeviceFamily::LianLi` variant in `hypercolor-types`:

```rust
pub enum DeviceFamily {
    OpenRgb,
    Wled,
    Hue,
    Razer,
    Corsair,
    LianLi,  // new
    Custom(String),
}
```

### 8.5 Protocol Database Registration

```rust
// Modern HID hubs
lianli_device!(UNI_HUB_SL,          0xA100, "Lian Li Uni Hub - SL",          Sl);
lianli_device!(UNI_HUB_AL,          0xA101, "Lian Li Uni Hub - AL",          Al);
lianli_device!(UNI_HUB_SL_INFINITY, 0xA102, "Lian Li Uni Hub - SL Infinity", SlInfinity);
lianli_device!(UNI_HUB_SL_V2,       0xA103, "Lian Li Uni Hub - SL V2",       SlV2);
lianli_device!(UNI_HUB_AL_V2,       0xA104, "Lian Li Uni Hub - AL V2",       SlV2); // uses SLV2 driver
lianli_device!(UNI_HUB_SL_V2_05,    0xA105, "Lian Li Uni Hub - SL V2 v0.5",  SlV2);

// Legacy
lianli_device!(UNI_HUB_ORIGINAL,    0x7750, "Lian Li Uni Hub",               Original);
```

---

## 9. Relationship to PrismRGB (Spec 20)

PrismRGB controllers (Spec 20) and Lian Li Uni Hubs are **completely separate hardware** despite both being associated with Lian Li's ecosystem.

### 9.1 Key Differences

| Aspect | PrismRGB (Spec 20) | Uni Hub (This Spec) |
|--------|-------------------|---------------------|
| VIDs | `0x16D5`, `0x16D2`, `0x16D0` | `0x0CF2` (ENE) |
| Protocol | 65-byte HID reports, `0xFC`/`0xFE` commands | 3-phase HID or libusb control transfers |
| Products | Prism 8, Nollie 8, Prism S, Prism Mini | Uni Hub SL/AL/V2/Infinity |
| Color format | GRB or RGB (varies) | **R-B-G** (all variants) |

### 9.2 Strimer Cable Disambiguation

Both ecosystems can drive Strimer LED cables, but via completely different controllers:

- **PrismRGB Prism S** (`0x16D0:0x1294`) — standalone Strimer controller, 65-byte HID, Spec 20 protocol
- **Lian Li Strimer L Connect** (`0x0CF2:0xA200`) — hub-controlled Strimer, Uni Hub HID protocol

Same cable, different protocols, different drivers. They must not be confused during device detection.

---

## 10. Testing Strategy

### 10.1 Mock HID Transport

```rust
/// Mock HID transport for Lian Li protocol testing.
///
/// Records all writes and verifies packet structure.
pub struct MockLianLiTransport {
    /// All packets written via hid_write().
    writes: Vec<Vec<u8>>,
    /// Expected packet sizes per variant.
    expected_size: usize,
}
```

### 10.2 Test Categories

**3-phase protocol validation:**
- Phase 1: Verify activate packet format per variant (sub-command byte, channel encoding)
- Phase 2: Verify R-B-G color ordering (not RGB!)
- Phase 3: Verify commit packet with correct effect, speed, direction, brightness bytes
- Full sequence: activate → color → commit for each variant

**Packet format per model:**
- SL: 11-byte packets, `0x32` sub-command
- AL: 65-byte packets, `0x40` sub-command, fan+edge dual packets
- SLV2: 65-byte packets, `0x60` sub-command, `(channel << 4) + fans` encoding
- SL Infinity: 65-byte packets, `0x60` sub-command, 8-channel addressing

**R-B-G color encoding round-trip:**
- Input RGB `(255, 128, 64)` → wire bytes `(255, 64, 128)` — verify B and G are swapped
- Full 16-LED fan buffer: 48 bytes of interleaved R-B-G triplets

**White color protection:**
- Sum-based: `(200, 200, 200)` → sum 600 > 460 → scaled to `(153, 153, 153)`
- Sum-based: `(255, 0, 0)` → sum 255 ≤ 460 → unchanged
- Equality-based: `(200, 200, 200)` → clamped to `(153, 153, 153)`
- Equality-based: `(200, 100, 50)` → unchanged (not equal channels)

**Fan speed formulas:**
- Verify speed byte encoding against uni-sync reference values
- Channel mode bitmask encoding
- RPM read parsing

**Effect byte mapping:**
- Verify each effect resolves to the correct mode byte per variant
- Merged mode bytes (`0x2A`–`0x2E`) only on SLV2/SL Infinity
- AL uses different byte values — verify no cross-contamination

---

## References

- `~/dev/OpenRGB/Controllers/LianLiController/` — 10 controller subdirectories
- `~/dev/OpenRGB/Controllers/LianLiController/LianLiControllerDetect.cpp` — detection and registration
- `~/dev/OpenRGB/Controllers/LianLiController/LianLiUniHubSLInfinityController/` — primary target implementation
- `~/dev/OpenRGB/Controllers/LianLiController/LianLiUniHubSLV2Controller/` — SLV2 implementation
- `~/dev/OpenRGB/Controllers/LianLiController/LianLiUniHubALController/` — AL implementation with fan/edge zones
- `~/dev/OpenRGB/Controllers/LianLiController/LianLiUniHubController/` — original hub libusb protocol
- uni-sync: `github.com/EightB1ts/uni-sync` — Rust fan speed control reference
- L-Connect 3 × OpenRGB Beta: `lian-li.com/l-connect-3-x-openrgb/`
