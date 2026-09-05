# 80 -- Lian Li TL LCD & Wireless Driver

> Full support for the Lian Li Uni Fan TL LCD family: the wired per-fan LCD
> panels (JPEG over HID), the 2.4GHz wireless ecosystem (RF dongle for fan/RGB,
> DES-wrapped bulk receivers for the screens), and the shared display encoding
> layer that makes LCD devices a declarative pattern in the HAL instead of a
> per-driver one-off. Corsair LCD and Push 2 migrate onto the shared layer as
> proof of the abstraction.

**Status:** Draft
**Crate:** `hypercolor-hal` (display layer + drivers), additive touchpoints in `hypercolor-core` and `hypercolor-daemon`
**Module path:** `hypercolor_hal::display`, `hypercolor_hal::drivers::lianli::{lcd, wireless}`
**Author:** Nova
**Date:** 2026-08-30

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry & Variant Matrix](#2-device-registry--variant-matrix)
3. [Architecture: Transports and Device Model](#3-architecture-transports-and-device-model)
4. [Shared Display Encoding Layer](#4-shared-display-encoding-layer)
5. [Wired TL LCD Protocol (0x04FC:0x7393)](#5-wired-tl-lcd-protocol-0x04fc0x7393)
6. [Wireless RF Dongle Protocol](#6-wireless-rf-dongle-protocol)
7. [Wireless LCD Receiver Protocol (0x1CBE)](#7-wireless-lcd-receiver-protocol-0x1cbe)
8. [Topology & Zones](#8-topology--zones)
9. [HAL Integration](#9-hal-integration)
10. [Daemon & Data Integration](#10-daemon--data-integration)
11. [Open Questions & Risks](#11-open-questions--risks)
12. [Testing Strategy](#12-testing-strategy)

[References](#references) · [Review History](#review-history)

---

## 1. Overview

The Uni Fan TL family is Lian Li's flagship fan line. Spec 19 §6 covers the
wired Uni Hub TL fan/RGB protocol (`0x0416:0x7372`), which
`hypercolor_hal::drivers::lianli::tl` already implements. This spec adds
everything the TL LCD SKUs bolt on top:

- **Wired TL LCD panels.** Each LCD fan carries a 1.6" round 400x400 IPS panel
  that enumerates as its own USB HID device (`0x04FC:0x7393`, one per fan).
  The panel accepts chunked JPEG frames at up to ~30 fps.
- **The wireless generation (TL V2, Dec 2024; TL V3; TL Flex, 2026).** Fan
  PWM, per-LED RGB, and sensor data move to a proprietary 2.4GHz link driven
  through a USB dongle. The LCD stream stays wired: each wireless LCD fan
  exposes a USB bulk device (`0x1CBE:0x0006` for TL V2) that accepts
  DES-wrapped JPEG frames. A wireless TL LCD build therefore needs two live
  transports.
- **A shared display encoding layer in the HAL.** Corsair LCD and Push 2 each
  hand-roll the same dance today: convert pixels to device bytes, then chunk
  them into framed packets with sequence counters and final flags. Lian Li
  would be the third copy. This spec extracts the shared machinery into
  `hypercolor_hal::display` and migrates the two existing display drivers onto
  it, so every future LCD device (NZXT, Thermaltake, Galahad II LCD
  `0x0416:0x7395` already sitting at `status = "researched"`) is configuration
  plus wire quirks, not a new subsystem.

The protocol facts below are derived from two independent open-source
reverse-engineering efforts that agree byte-for-byte on every overlapping
command: `sgtaziz/lian-li-linux` (Rust, hardware-tested, complete) and
`lewisgibson/FanControl.LianLi` (C#, derived from an L-Connect 3 decompile).
The load-bearing wire facts in §5–§7 (packet builders, crypto, chunking,
response handling) were verified against the upstream source files directly on
2026-08-30, not just against research summaries. Neither liquidctl nor OpenRGB
supports any TL device. Hypercolor's implementation is clean-room per house
policy: we implement from the wire facts documented here, never from another
project's code.

### Acceptance criteria (staged)

**v1 ships and is validated on hardware:**

- Wired TL LCD panels (`0x04FC:0x7393`): live JPEG streaming, hardware
  brightness/rotation at init, multi-panel identity.
- V1 wireless dongle (`0x0416:0x8040`): fan discovery, per-LED RGB streaming
  and firmware-looped upload, PWM hold-steady + clock keepalive (§6.8).
- Wireless LCD receivers (`0x1CBE:0x0005/0x0006`): live JPEG streaming with
  DES-wrapped headers.
- The shared display layer, with Corsair LCD and Push 2 migrated and their
  existing test suites passing unmodified.

**Documented but not registered in v1** (registering a descriptor makes
discovery bind it, so these stay out of `LIANLI_DESCRIPTORS` and live only in
the vendor db as `status = "researched"` until hardware-validated): V2 dongle
(`0x1A86:0xE304`, §11.5), TL V3 fan-type bytes (§11.6 — the classification
byte ranges ship in the parser but gated fan types are reported as unknown
models, not bound to features).

**Research-only in this spec:** TL Flex (`0x1CBE:0xA018` WinUSB/H.264 path,
`0x43A8` controller), pairing/binding UI (§6.4; devices ship pre-bound and
L-Connect can rebind — a future `hypercolor-driver-support` service), fan
curves as a product feature (v1 only holds observed PWM steady, §6.8),
firmware-looped RF animations as an effect-delivery optimization.

## 2. Device Registry & Variant Matrix

### 2.1 USB identity map

| Device | VID | PID | Class | Transport | Notes |
|---|---|---|---|---|---|
| Uni Hub TL (fan/RGB, wired) | `0x0416` | `0x7372` | HID | `UsbHidApi`, usage page `0xFF1B` | Already supported (spec 19 §6). Product string carries firmware: `TL_Series_ControllerV0.62` |
| **Uni Fan TL LCD panel (wired)** | `0x04FC` | `0x7393` | HID | `UsbHidApi`, report ID `0x02` | One USB device **per LCD fan**. Ships non-unique serial `TL_LCDV0.1` |
| L-Wireless TX dongle V1 | `0x0416` | `0x8040` | vendor (`ff-00-00`) | `DriverUsb` (bulk + RX companion) | Product string carries firmware (`SLV3TX_V1.6`); iSerial absent, so identity keys on the USB path. Both halves sit behind the controller's internal `1A86:8091` hub |
| L-Wireless RX dongle V1 | `0x0416` | `0x8041` | vendor (`ff-00-00`) | `UsbBulk` (companion) | The half that answers `GetDev`; opened by the TX's driver-owned transport factory as its companion, never a device of its own (§6.1) |
| L-Wireless TX dongle V2 | `0x1A86` | `0xE304` | vendor | `UsbBulk` | WCH/QinHeng silicon; 2025 hardware revision; **validation-gated** |
| L-Wireless RX dongle V2 | `0x1A86` | `0xE305` | vendor | — | |
| V2 dongle HID companion | `0x1A86` | `0x2107` | HID | HID | Cmd `0x1C` returns the paired group's 6-byte MAC |
| **TL V2 wireless LCD receiver** | `0x1CBE` | `0x0006` | vendor | `UsbBulk` | One per wireless LCD fan, over its 9-pin USB cable |
| SL V3 wireless LCD receiver | `0x1CBE` | `0x0005` | vendor | `UsbBulk` | Same protocol as TL V2 (shared implementation); SL fans carry 40 LEDs vs TL's 26 |
| TL Flex LCD | `0x1CBE` | `0xA018` | vendor (WinUSB) | — | Different framing (H.264/`StartPlay`); research-only |
| TL Flex controller | `0x43A8` | `0x0101`/`0x0102` | — | — | Research-only |

`0x04FC` is Sunplus and `0x1CBE` is the Luminary Micro/TI VID — Lian Li ships
LCD MCUs under borrowed vendor IDs, so these descriptors live in the `lianli`
family regardless of VID.

### 2.2 Variant matrix

| Variant | LCD | LEDs | Control path | Controller |
|---|---|---|---|---|
| TL 120/140 (wired, non-LCD) | none | infinity ARGB | Uni Hub TL, ≤10/port | `0x0416:0x7372` |
| TL LCD 120/140 (wired, Dec 2023) | 1.6" 400x400 IPS, ~30 fps | 26 (marketing; see §11.1), 2 zones | Uni Hub TL for fan/RGB + one `0x04FC:0x7393` per panel; ≤3 LCD/port, ports 2–4 only, ≤9 LCD/hub | hub + per-fan HID |
| TL Wireless 120/140 (non-LCD, Dec 2024) | none | 26, 2 zones | 2.4GHz only (no USB on fan) | dongle V1/V2 |
| TL LCD Wireless "TL V2" (Dec 2024) | 1.6" 400x400 IPS, 60 Hz panel | 26, 2 zones | RGB/PWM over RF; LCD over per-fan USB (`0x1CBE:0x0006`); ≤3 LCD direct on a 9-pin header, ≤9 via powered splitter, ≤20/PC | dongle + per-fan bulk |
| TL V3 Wireless (LCD variants) | same class | 26 | same RF ecosystem, new fan-type bytes (§6.5); **validation-gated** | dongle |
| TL FLEX / LCD FLEX (2026) | 1.8" 400x400, 60 Hz, 512MB onboard flash | 26 | per-fan FLEX receivers; WinUSB LCD path; **research-only** | Flex controller |

RF capacity: 10 channels per dongle, 4 fans per cluster, up to 40 non-LCD
fans. Wired-hub limits come from Lian Li's own guides (LCD fans lag/flicker on
hub port 1; L-Connect caps 12 TL LCD system-wide).

### 2.3 Firmware disambiguation

No firmware predicates are needed for the new descriptors today: every
protocol fork in this family arrives as a distinct VID/PID, not a firmware
range behind one PID. Firmware versions ride USB product strings
(`TL_Series_ControllerV0.62`, `SLV3TX_V1.6`) — match on VID/PID only, never on
the full string. Lian Li precedent (SL hub PID change `0x7750` → `0xA100`
after a firmware update; AL v1.0/v1.7 transport fork) says leave room for
predicates later; every descriptor added here is predicate-free per the
lookup fallback rule.

## 3. Architecture: Transports and Device Model

### 3.1 One panel, one `DeviceId`

The hardware resolves the multi-panel question for us: every LCD panel —
wired `0x04FC:0x7393` or wireless `0x1CBE:0x0006` — is its own USB device.
Each becomes its own `DeviceId` with exactly one `Display` segment, which is
precisely the shape the existing daemon display pipeline assumes
(`display_target_geometry_for_device`, `DisplayOutputLane`, and
`DeviceDisplaySink` are all keyed per device with a single display geometry).

The daemon needs **no structural changes** — no new pipeline, worker model, or
API surface. It does need two small additive changes, specified in §10:

1. **An encoded-frame size budget.** The daemon owns JPEG encoding
   (`display_output/encode.rs`, fixed `JPEG_QUALITY`); the HAL only receives
   encoded bytes. Devices with a hard wire cap (the wireless receiver's
   101,888-byte payload limit, §7.3) advertise a budget the daemon's encoder
   must respect via bounded quality step-down.
2. **A descriptor serial quirk for fingerprinting.** `DeviceIdentifier::
   fingerprint` keys on `serial.or(usb_path)` — serial wins when present — so
   nine panels all reporting `TL_LCDV0.1` would collapse into one
   `DeviceFingerprint`. The scanner must consult a descriptor-declared quirk
   before building the identifier (§5.6, §10).

The fan's RGB LEDs belong to a different device (the wired hub, or the
wireless dongle), exactly as Corsair splits LINK lighting from LCD devices.
Presenting fan-LEDs + fan-LCD as one logical device is a future UX
unification; the wireless correlation handshake that would enable it (dongle
HID cmd `0x1C` → group MAC, matched by shared USB parent hub) is documented in
§6.6 and deliberately unused in v1.

### 3.2 Three new protocols

| Protocol id | Device | Role | Transport |
|---|---|---|---|
| `lianli/tl-lcd` | `0x04FC:0x7393` | wired panel: JPEG display sink | HID output reports, report ID `0x02`, 512B |
| `lianli/wireless` | dongles (`0x0416:0x8040`; V2 gated) | fan discovery + per-LED RGB over RF | USB bulk, 64B packets, TX primary + RX companion (`TransferType::Companion`) |
| `lianli/wireless-lcd` | `0x1CBE:0x0005/0x0006` | wireless panel: DES-wrapped JPEG display sink | USB bulk, 512B header + payload |

All three ids share the `lianli` prefix so they stay inside the existing
`lianli` driver module (driver id derives from the protocol-id prefix).

### 3.3 Display data flow (for orientation)

```
daemon display_output worker
  canvas RGBA → brightness LUT → TurboJPEG (400x400, Sub2x2)
  → [size-budget check, bounded quality step-down — §10]
  → Arc<OwnedDisplayFramePayload{ format: Jpeg, 400, 400 }>
  → DisplayOutputLane.write() → UsbDisplaySink → usb actor
  → Protocol::encode_display_payload_into()        ← this spec's seam
  → ProtocolCommands → transport
```

The daemon already owns JPEG encoding, per-device FPS caps, static-hold
refresh, and preview streaming. The HAL's job is turning JPEG bytes (or raw
RGB for framebuffer devices) into wire packets — which is what §4 makes
shared.

## 4. Shared Display Encoding Layer

New module: `crates/hypercolor-hal/src/display/`.

**Design stance: a helper library, not a framework.** Each protocol's
`encode_display_payload_into` remains the orchestrator, keeps its own state,
and calls shared engines for the parts that are today duplicated and
historically bug-prone. This is deliberate: Push 2 keeps a JPEG-decode cache
and a lazily created TurboJPEG decompressor inside its protocol state, and
Corsair appends a wire keepalive conditionally after frame chunks — a trait
object that owned the whole frame would either forbid that statefulness or
grow hooks until it stopped being an abstraction. The protocols stay in
charge; the library removes the copies.

### 4.1 The two stages

**Stage 1 — payload:** how pixels become device bytes.

- `JpegPassthrough` — the daemon already delivers JPEG; the HAL forwards
  bytes. (Corsair LCD, both Lian Li LCD paths.)
- `PixelRepack` — RGB888 → packed pixel formats for raw-framebuffer devices.
  Shared helpers in `display/repack.rs`: RGB565/BGR565 little-endian packing,
  optional XOR mask applied per byte-run, line stride + filler padding.
  (Push 2: BGR565 LE, the signal-shroud XOR, 1920B lines padded to 2048.)

**Stage 2 — framing:** how device bytes get on the wire: chunk iteration,
sequence counters, final flags, size fields, per-chunk command policy.

### 4.2 The framing engines

```rust
// crates/hypercolor-hal/src/display/mod.rs

pub struct ChunkContext<'a> {
    /// Total encoded frame length across all chunks.
    pub total_len: usize,
    /// Zero-based chunk index within this frame.
    pub packet_index: u32,
    /// This chunk's payload bytes.
    pub payload: &'a [u8],
    pub is_final: bool,
}

/// Per-chunk command policy. Everything ProtocolCommand needs beyond bytes.
pub struct ChunkCommandPolicy {
    pub transfer_type: TransferType,
    pub expects_response: bool,
    pub response_delay: Duration,
    pub post_delay: Option<Duration>,
}

pub trait DisplayChunkLayout: Send + Sync {
    /// Fixed on-wire packet length (header + payload + padding).
    fn packet_len(&self) -> usize;
    /// Maximum payload bytes carried per packet.
    fn max_payload(&self) -> usize;
    /// Where payload bytes start inside the packet.
    fn payload_offset(&self) -> usize;
    /// Write the header (and any trailer) into the zeroed packet buffer.
    /// The engine has already copied the payload to `payload_offset()`.
    fn write_header(&self, packet: &mut [u8], ctx: &ChunkContext<'_>);
    /// Command policy for this chunk (ack-per-chunk vs fire-and-forget,
    /// pacing, transfer path).
    fn command_policy(&self, ctx: &ChunkContext<'_>) -> ChunkCommandPolicy;
    /// Maximum chunk count this layout's sequence counter can express
    /// (e.g. the wired TL LCD's u24 counter; a 101,888-byte frame needs
    /// 204 chunks, far inside every limit here — the bound exists so
    /// overflow is an error, never a wrapped counter).
    fn max_chunks(&self) -> u32;
}

/// Chunk `data` across fixed-size packets via `CommandBuffer::push_fill`,
/// one `ProtocolCommand` per chunk, applying the layout's command policy.
/// Zero-length `data` emits NOTHING (the existing Corsair suite pins zero
/// packets for an empty JPEG, and the daemon never delivers empty frames).
/// Errors if the chunk count would exceed `max_chunks()`.
pub fn encode_chunked_display_frame(
    layout: &dyn DisplayChunkLayout,
    data: &[u8],
    commands: &mut Vec<ProtocolCommand>,
) -> Result<(), DisplayEncodeError>;

/// Prefixed single-buffer variant: one header block followed by the payload
/// in ONE wire write. `fixed_frame_len: Some(n)` zero-pads the buffer to
/// exactly `n` bytes (the wireless Lian Li receiver requires a fixed
/// 102,400-byte write); `None` sends header_len + data.len().
pub fn encode_prefixed_display_frame(
    header_len: usize,
    write_header: impl FnOnce(&mut [u8], &PrefixContext<'_>),
    data: &[u8],
    fixed_frame_len: Option<usize>,
    policy: ChunkCommandPolicy,
    commands: &mut Vec<ProtocolCommand>,
) -> Result<(), DisplayEncodeError>;
```

Design notes:

- `write_header` receives the whole packet buffer, so a layout that must
  transform its header (the DES-CBC encryption in §7) or write a trailer does
  so inside its impl. Config-only framing would be too rigid; a small trait
  is the right altitude.
- The engines own everything that is currently duplicated: chunk-boundary
  arithmetic (the exact-boundary edge cases the Corsair tests pin),
  sequence-counter width handling, final-flag placement, total-size
  repetition, zero-padding, and scratch discipline.
- Frame preambles (Push 2 emits a header command before pixel data) stay in
  the protocol: it pushes its preamble command, then calls the chunk engine.
  No hook needed.
- Both engines are fallible (`DisplayEncodeError::{PayloadTooLarge,
  TooManyChunks}`); truncation is never silent. The existing display seam
  (`encode_display_payload_into` returning `Option<()>`) has no error
  channel, and `None` already means "format unsupported", so protocols map an
  engine error to **skip-and-warn**: emit no commands, log a rate-limited
  warning, return `Some(())`. The daemon's encoded-size budget (§10) makes
  this a should-never-fire backstop, not a control path.

### 4.3 Wire keepalive helper

`display/keepalive.rs`: interval-tracked wire keepalive
(`WireKeepalive::new(interval)`, `.due()`, `.mark_sent()`), extracted from
`CorsairLcdProtocol`'s 30s keepalive. The protocol decides where keepalive
commands go (Corsair appends one after frame chunks when due). Distinct from
the daemon's static-hold refresh, which re-sends frames so panels don't blank
— that already covers the Lian Li panels with zero driver code.

### 4.4 Typed display settings hook

```rust
pub enum DisplayRotation { Deg0, Deg90, Deg180, Deg270 }

pub enum DisplaySetting {
    /// 0..=100. Devices with nonlinear curves own their LUT (§7.2).
    Brightness(u8),
    Rotation(DisplayRotation),
    FrameRate(u8),
}

// New optional method on `Protocol`, default `None`:
fn encode_display_setting(&self, _setting: DisplaySetting)
    -> Option<Vec<ProtocolCommand>> { None }
```

v1 uses this only from init sequences (each LCD protocol sets its own
defaults). Software brightness via the daemon's byte-space LUT remains
authoritative. Surfacing hardware brightness/rotation through the displays
API is future work; the hook exists so it lands without another trait change.

### 4.5 Protocol plumbing: responses, timeouts, read sizing

Ride-along extensions needed by this family (§5.3, §6.5) and latently by the
existing TL hub. `ProtocolCommand` gains three optional fields, all
backward-compatible defaults:

```rust
/// Number of response reports to read when `expects_response` is true.
/// Each is passed to `parse_response` in order. Default 1.
pub response_count: u8,
/// Per-command response timeout. None = the protocol-wide
/// `response_timeout()` (the only knob that exists today, which cannot
/// express the wired LCD's 3000 ms init reads vs 200 ms steady reads).
pub response_timeout: Option<Duration>,
/// Receive CAPACITY in bytes — an upper bound, not an expected length.
/// None = one transport-default read. The bulk transport sizes its
/// receive buffer to the endpoint max packet size (64 B on the dongle),
/// so a multi-packet reply like GetDev (§6.5, 4..=508 bytes) MUST set
/// this. The transport accumulates packets and completes the logical
/// read on the FIRST of: a short packet (< endpoint max packet size),
/// capacity reached, or an inter-packet gap timeout (default 20 ms,
/// distinct from `response_timeout`, which bounds the wait for the
/// first packet). A gap-terminated read returns the accumulated bytes
/// as a normal reply, not an error — replies whose length is an exact
/// multiple of the packet size (a six-record GetDev reply is exactly
/// 256 = 4 × 64 bytes) have no short-packet terminator and cost one
/// gap timeout, nothing more.
pub response_len: Option<usize>,
```

The USB actor currently performs exactly one receive per responding command;
it loops `response_count` times, passing each report to `parse_response` in
arrival order. **Multi-report parsing is ordinal-sensitive:** a parser that
treats every report of a command identically will let a later report
overwrite state derived from an earlier one. Both wired Lian Li
`GetProductInfo` commands (hub `0xA6` and LCD `0x3D`) answer with two reports
— version string, then build-date string. The existing TL hub driver sets
`response_count: 2` on `0xA6` as part of this work (today's single read
leaves the date report queued — a latent desync that has not yet bitten only
because init is the last read on that device), **and** its `parse_response`
keeps the first `0xA6` payload as the firmware version, logging and
discarding the second; a final-state test pins version-not-date (§12).

### 4.6 Migration: Corsair LCD and Push 2

Both existing display drivers move onto the shared layer **in this work**, so
the abstraction is proven by three real drivers on day one:

- `CorsairLcdProtocol::encode_display_frame_into` becomes a
  `DisplayChunkLayout` impl (packet_len 1024, max_payload 1016, zerocopy
  `LcdDisplayPacket` header, `TransferType::Bulk`, no per-chunk acks) plus
  the shared `WireKeepalive`; the protocol still appends the keepalive report
  itself when due.
- Push 2's display path keeps its JPEG-decode cache and preamble in protocol
  state, and delegates pixel packing to `repack` helpers and chunk emission
  to the engine.

Acceptance bar: the existing `corsair_lcd_display_tests` (23 tests) and
`push2_display_tests` suites pass **unmodified** — byte-identical wire
output. The migration is a refactor, not a behavior change.

## 5. Wired TL LCD Protocol (0x04FC:0x7393)

Confirmed on hardware by the reference implementation; packet builder and
session logic verified against upstream source (`tl_lcd.rs`). Screen: 400x400
round IPS, JPEG frames, practical ceiling ~30 fps, reference uses JPEG
quality 90.

### 5.1 Transport

HID output reports, report ID `0x02`, fixed 512-byte packets, written via
interrupt-OUT (hidapi write). Responses are short reports read with a 64-byte
buffer. Descriptor:

```rust
TransportIntent::Hid(HidTransportIntent {
    access: HidAccessMode::Direct,
    interface: 0,
    report_id: 0x02,
    report_mode: HidRawReportMode::OutputReportWithReportId,
    max_report_len: 512,
    usage_page: None,
    usage: None,
})
```

Timeouts: **200 ms** per response read in steady state; **3000 ms** for init
reads (identity, handshake, firmware) — carried per command via
`response_timeout` (§4.5), since the protocol-wide timeout cannot express the
split. The reference flushes pending input reports before the firmware query;
with `response_count` keeping reads paired, a session-start drain is
recommended but not required.

### 5.2 Packet format

Every host→device packet is 512 bytes:

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x02` | HID report identifier |
| 1 | 1 | Command | §5.3 | |
| 2–5 | 4 | Total size | u32 BE | Full transfer size in bytes, repeated in every chunk of the transfer. For non-chunked commands: the payload length |
| 6–8 | 3 | Packet number | u24 BE | Zero-based chunk counter, **resets to 0 for every transfer** |
| 9–10 | 2 | Payload length | u16 BE | This packet's payload byte count (≤501) |
| 11–511 | ≤501 | Payload | | JPEG chunk or command payload; zero-padded |

Device→host responses reuse the same header shape (command echo at offset 1,
payload length at offsets 9–10, payload from offset 11) but arrive as short
reports; read them into a 64-byte buffer.

### 5.3 Command vocabulary

| Command | Byte | Responses | Notes |
|---|---|---|---|
| GetHandshake | `0x3C` (60) | 1 | reply payload: §5.4 |
| GetProductInfo | `0x3D` (61) | **2** | reply 1 = firmware version string, reply 2 = build date/time string; both NUL-padded ASCII in the payload field. `response_count: 2` |
| ReadSerial | `0x3E` (62) | 1 | reply payload: §5.4 |
| WriteSerial | `0x3F` (63) | 0 | payload = 32-byte serial, NUL-padded. See §5.7 |
| LcdControl | `0x40` (64) | 1 | payload: §5.5. Response drained, contents unused |
| WriteJpg | `0x41` | 1 per chunk | static image; ack after **every** chunk, `ack[1]` must equal `0x41` |
| WriteAvi | `0x45` | — | defined, unused |
| **WriteSyncJpg** | `0x46` | 0 | streamed frame, no acks — the live path |
| WriteBootAvi | `0x47` | — | boot animation (unused v1) |
| WriteBootJpg | `0x48` | — | boot image (unused v1) |

### 5.4 Response payloads

GetHandshake (`0x3C`) reply payload (offsets relative to payload start at
byte 11):

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | mode | 1/3/4/5/6 | current `LcdControl` mode (§5.5) |
| 1–2 | 2 | frame_index | u16 BE | current display frame counter |

ReadSerial (`0x3E`) reply payload:

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0–31 | 32 | serial | ASCII, NUL-padded | factory value is the firmware string `TL_LCDV0.1` (non-unique) |
| 32 | 1 | port | 0-based | hub port this panel hangs off |
| 33 | 1 | index | 0-based | position within the port's chain |

### 5.5 LcdControl (0x40) payload (11 bytes)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | mode | 1=ShowJpg, 3=ShowAvi, 4=ShowAppSync, **5=LcdSetting**, 6=LcdTest | LcdSetting applies brightness/fps/rotation without changing content mode |
| 1–3 | 3 | reserved | `0x00` | |
| 4 | 1 | brightness | 0–100 | |
| 5 | 1 | fps | 30 | reference always sends 30 |
| 6 | 1 | rotation | 0/1/2/3 | 0/90/180/270° |
| 7–10 | 4 | reserved | `0x00` | |

### 5.6 Session model

- **Init sequence** (order per the reference; all reads at the 3000 ms init
  timeout): ReadSerial → GetHandshake → GetProductInfo (2 responses) →
  LcdControl(mode=**LcdSetting**, brightness, fps=30, rotation=0). Hardware
  brightness is set to 100 at init; the daemon's software brightness LUT is
  the sole runtime brightness authority.
- **Live streaming:** `encode_display_payload_into(Jpeg)` → chunk engine
  (§4.2) with the §5.2 layout, all chunks `WriteSyncJpg`, no acks, no
  explicit mode switch — the panel displays synced frames as they stream.
  This is the only frame path v1 uses.
- **Static image** (documented, unused v1): `WriteJpg` with per-chunk acks,
  then `LcdControl(mode=ShowJpg)` to latch it.
- **Blanking:** daemon static-hold refresh re-sends the last frame; no
  driver-side keepalive needed.
- **Capabilities:** `has_display: true`, `display_resolution: (400, 400)`,
  circular, `max_fps: 30`, `frame_interval: 33ms`. Zone: one `SegmentInfo`
  ("Display", 0 LEDs, `DeviceTopologyHint::Display { 400, 400, circular:
  true }`, `DeviceColorFormat::Jpeg`).

### 5.7 Identity: the non-unique serial problem

Every wired panel ships iSerial `TL_LCDV0.1`, and
`DeviceIdentifier::fingerprint` keys on `serial.or(usb_path)` — serial wins —
so a chain of panels would collapse into one `DeviceId`. The fix is
descriptor-driven:

1. **Descriptor serial quirk.** `DeviceDescriptor` gains
   `serial_quirk: Option<SerialQuirk>`;
   `SerialQuirk::PlaceholderValues(&["TL_LCDV0.1"])` on this descriptor tells
   the USB scanner to treat a matching serial as absent when building the
   `DeviceIdentifier`, so the fingerprint keys on the **USB port path**
   (panels hang off stable physical positions). The scanner likewise skips
   the `PortableIdentityClaim` for placeholder serials — that layer already
   has a refusal path for exactly this shape.
2. **Post-connect refinement.** The ReadSerial `(port, index)` record is
   surfaced as device metadata (stable per chain position) for diagnostics
   and layout naming. It does not and cannot rewrite the registered
   `DeviceId` — identity is fixed at scan time.
3. **Serial adoption is out of v1.** The reference driver auto-writes a UUID
   serial (`WriteSerial`) to any panel whose serial isn't UUID-shaped, then
   re-reads it. Hypercolor v1 does **not**: it mutates user hardware, it
   re-keys fingerprints (the path-keyed device is orphaned and a serial-keyed
   one appears), and — decisively — no bridge exists today from daemon config
   into HAL protocol construction (`ProtocolFactory` takes no configuration
   and HAL modules advertise `config: false`), so a "config-gated" version
   is unimplementable as specified. The command stays documented (§5.3) as a
   future escalation that would ride whatever config-to-protocol bridge
   lands first (§11.9).

### 5.8 Concurrency caution

The reference driver serializes all access across a chain of panels with a
process-wide lock. Our per-device USB actors may be fine (each panel is its
own HID handle), but treat concurrent multi-panel streaming as a validation
item on real hardware, and remember the Windows lesson already in the graph:
every HID call must carry an outer deadline so a wedged panel cannot pin the
blocking-thread pool. Nine panels at 30 fps is the stress case; the daemon's
per-device display FPS caps and adaptive tiers stay the load-shedding
mechanism (measured, not preemptively nerfed).

## 6. Wireless RF Dongle Protocol

The controller tunnels RF frames over USB bulk. It is two USB functions
behind one internal hub: the **TX** (`0x8040`) takes RF envelope slices and
answers the MAC query; the **RX** (`0x8041`) answers the `GetDev` device
table poll. The TX answers *every* command with its own status packet
(command echo, its MAC, a running counter, its firmware), so discovery must
go to the RX. Verified on hardware 2026-09-04 (V1, `SLV3TX_V1.6`), which is
also where the earlier draft's "RX is not a protocol device" was found
wrong. In the HAL the TX descriptor's driver-owned transport factory opens
the RX sibling (same parent port chain) and pairs them as a
`CompanionTransport`; the protocol tags RX commands
`TransferType::Companion`. RGB framing verified against upstream source
(`wireless/rgb.rs`).

### 6.1 USB layer

64-byte bulk packets; the transport claims interface 0 and auto-discovers its
bulk endpoint pair (observed `0x01` OUT / `0x81` IN).

Host→dongle packet:

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | USB command | `0x10` / `0x11` | `0x10` = send RF chunk (also GetDev poll), `0x11` = get master MAC |
| 1 | 1 | chunk index | 0–3 | which 60-byte slice of the 240-byte RF buffer (RF sends); page count for GetDev |
| 2 | 1 | RF channel | 1–39 | |
| 3 | 1 | rx_type | 1–13, `0xFF` | radio endpoint slot; `0xFF` broadcast |
| 4–63 | 60 | RF data | | 60-byte slice of the RF buffer |

Precomposed control frames (zero-padded to 64): `11 08 00 00` TX reset,
`11 01 00 00` enter video mode, `10 01 04 34` / `10 01 04 37` RX queries,
`10 01 04 30` RX LCD mode.

**Master discovery:** write `{0x11, channel}`, read `{0x11, mac[6], ...}`
(bytes 11–12 = master firmware, u16 BE, when present). Channel scan order:
8, then even 2–38, then odd 1–39. Default channel 8.

### 6.2 RF envelope (240 bytes, shipped as 4 × 60-byte chunks)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | envelope opcode | `0x12` | RF_SELECT |
| 1 | 1 | sub-command | §6.3 | |
| 2–7 | 6 | target MAC | | slave device MAC |
| 8–13 | 6 | master MAC | | |
| 14–17 | 4 | command-specific | | rx_type/channel/slot for PWM & bind; effect index for RGB |
| 18+ | | payload | | command-specific (§6.4, §6.7) |

The four USB chunks of one RF buffer are paced **1 ms apart** (`post_delay`
on each chunk command; the encoder owns this pacing, not the caller). The
reference driver ships 1 ms between slices; 2 ms is what its RGB path's
inter-envelope header repeat uses.

### 6.3 RF sub-commands

| Name | Byte | Purpose |
|---|---|---|
| RF_PWM_CMD | `0x10` | fan PWM set / bind carrier |
| RF_SELECTED_GROUP | `0x12` | group select |
| RF_CLOCK_SYNC | `0x14` | 1 Hz master clock + sensor broadcast (220B blob) |
| SaveConfig | `0x15` | persist bindings to receiver flash (broadcast 3×, 200 ms gaps) |
| RF_REBOOT_LCD | `0x16` | reboot LCD MCU |
| RF_SET_RGB | `0x20` | per-LED RGB, compressed (§6.7) |
| RF_SEND_PIC | `0x22` | small image over RF, 220B chunks — AIO caps only, **not** the fan LCD path |
| RF_MB_LIGHT_SYNC | `0x27` | motherboard lighting sync |

(Others exist for AIO/case products: `0x19`, `0x21`, `0x23`.)

### 6.4 Binding (documented, not implemented in v1)

Bind = RF_PWM_CMD frame carrying master MAC at [8..14], rx slot (1–13) at
[16], current PWM at [17..21]; sent 6× with 30 ms gaps; poll GetDev until the
device reports the new master/rx (3 sightings, 5 s timeout); then SaveConfig.
Unbind writes an all-zero master MAC and rx=0. Bindings persist in receiver
flash, which is why v1 can require pre-bound fans. Recovery without
Hypercolor: L-Connect on any machine can rebind.

### 6.5 Discovery: GetDev and the 42-byte device record

Poll the **RX** by writing `{0x10, pages}` (pages = ceil(known devices /
10), clamped 1–2; v1 always polls 2) and reading one logical bulk response.
Observed on hardware: the reply is **page-sized, not record-sized**: 448
bytes for one page and 896 for two, exact multiples of the 64-byte packet
with no short packet, so the read ends on the inter-packet gap (~25–30 ms
total). The GetDev command therefore sets a response capacity of 1024 and
relies on the §4.5 completion rule (short packet, capacity, or gap).

The parser must not clamp the count byte: a TX status packet echoing the
poll (`10 a0 71 ae …`) would read as 160 devices. A count above 12 is a
malformed reply, and the controller's own MAC at bytes 1–6 is the status
packet, which the protocol ignores and keeps its last table.

Response layout:

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | echo | `0x10` | |
| 1 | 1 | device count | ≤12 | |
| 2–3 | 2 | motherboard PWM | | bit 7 of [2] = unavailable; else duty = on/(on+off) from ([2]&0x7F, [3]) |
| 4+ | 42×N | device records | | |

Each 42-byte record:

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0–5 | 6 | device MAC | | |
| 6–11 | 6 | master MAC | | all-zero = unbound |
| 12 | 1 | RF channel | 1–39 | |
| 13 | 1 | rx_type | 1–13 | radio endpoint slot |
| 14–17 | 4 | system time | | ms × 0.625 |
| 18 | 1 | device type | 0, 255, … | 0 = fan cluster, 255 = master; others = AIO/case gear |
| 19 | 1 | fan count | 0–4 (+10) | ≥10 flags SL-INF right-attach chain; subtract 10 |
| 20–23 | 4 | effect index | | per fan slot |
| 24–27 | 4 | fan-type bytes | §below | per slot; model classification |
| 28–35 | 8 | fan RPMs | 4 × u16 BE | high nibble of each high byte = status bits: `0x40` MB-light-sync, `0x20` PWM line |
| 36–39 | 4 | current PWM | 0–255/slot | |
| 40 | 1 | cmd_seq | | echoed to ack commands |
| 41 | 1 | validation | **`0x1C`** | reject the record otherwise |

Fan-type byte → model: `27, 32–35` = TL V2 Wireless **LCD**; `28–31` = TL V2
Wireless (no LCD); `51–58` = TL V3 (LCD = `51, 52, 55, 56`;
validation-gated). All TL wireless: 26 LEDs/fan, minimum duty 11%.

**Topology lifetime (v1):** fan membership is captured by GetDev polling
during connect (the init handshake path, like the wired TL hub) and refreshed
by the 1 Hz upkeep polling (§6.8), which updates RPM/PWM state and cmd_seq
**by MAC against the connect-time cluster set**. The protocol freezes that
set the moment upkeep or streaming begins: a later poll never reorders or
grows the routing, because the daemon's segments were published from the
connect-time order and a reordered table would put one cluster's colors on
another. Only fan clusters (record type 0) bound to this controller's MAC
enter the set; receivers bound elsewhere, unbound receivers, and AIO or case
gear heard on the channel are ignored. A truncated reply (fewer whole records
than the count byte) keeps the last table. Segment topology is published to
the daemon only at connect/reconnect, so v1 surfaces membership changes (a
newly bound fan) after a rescan/reconnect; live topology-change publication
is a named follow-up, not silently promised.

### 6.6 Group correlation (future)

The V2 dongle's HID companion (`0x1A86:0x2107`) answers HID cmd `0x1C` with
the paired group MAC; matching that against `0x1CBE` receivers via shared USB
parent hub links RF fans to their LCD panels. Documented for the future
logical-device unification; unused in v1.

### 6.7 Per-LED RGB over RF (RF_SET_RGB, 0x20)

Payload pipeline: raw RGB bytes (3 per LED, R-G-B order, fans in the cluster
concatenated in slot order) → **tinyuz** compression (§6.9) → 220-byte chunks
across RF frames. SL-INF right-attach chains reverse per-fan chunk order
before compression; TL chains do not.

Header frame (packet index 0), fields within the §6.2 envelope:

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 14–17 | 4 | effect index | | opaque 4-byte tag; echoed in GetDev records |
| 18 | 1 | packet index | 0 | |
| 19 | 1 | total packets | ceil(clen/220) **+ 1** | header counts itself |
| 20–23 | 4 | compressed length | u32 BE | |
| 24 | 1 | reserved | 0 | |
| 25–26 | 2 | total frames | u16 BE | 1 = live still; >1 = firmware-looped animation |
| 27 | 1 | LEDs per fan | 26 (TL) | |
| 28–31 | 4 | reserved | 0 | |
| 32–33 | 2 | frame interval | u16 BE, ms | loop interval for animations; reference sends 5000 for single stills |
| 34–239 | 206 | reserved | 0 | zero-filled remainder of the 240-byte envelope |

The header frame is sent `header_repeats` times (≥1; inter-repeat gap 2 ms
when ≤2 repeats, else 20 ms). Data frames (index 1..N) carry the packet index
at [18] and up to 220 compressed bytes at [20..240]; bytes past the final
chunk's length are zero.

Live streaming = `total_frames: 1` per render tick. The achievable tick rate
is bandwidth-bound and must be measured on hardware (§11.4) — the spec sets
no artificial cap. Firmware-looped animation upload is the documented
optimization path for static effects.

### 6.8 Steady-state upkeep: the v1 keepalive policy

Two facts force a policy: fans revert to firmware-default speed when PWM
traffic goes silent (V2 receivers default 1000 RPM), and missing the 1 Hz
clock broadcast puts fan firmware into an autonomous fallback that
occasionally spikes RPM.

**v1 policy — hold-steady, decided:** while Hypercolor owns the dongle, it

1. captures each cluster's current per-slot PWM from GetDev records at
   connect,
2. re-broadcasts those observed values via RF_PWM_CMD at 1 Hz (payload:
   envelope + rx_type/channel/slot at [14..17], 4 PWM bytes at [17..21]);
   values are never invented — only observed duty is held. Undetected slots
   send 0,
3. emits RF_CLOCK_SYNC at 1 Hz: first an init frame (bytes [14..64] filled
   with the `0x14` sentinel), then frames carrying real date/time at
   [32..39], zeroed CPU/GPU sensor fields, and the per-receiver fan blocks
   (14 × 12 bytes at [50..218]) left zero. The hardware-tested reference
   sends those blocks zeroed as well (`clock_sync.rs` "defaults for now"),
   so zero is the validated value; filling them from observed state is a
   follow-up gated on hardware, not a v1 requirement,
4. sends nothing after disconnect/shutdown — fans revert to firmware
   defaults, which is the same behavior as L-Connect exiting; documented, not
   hidden.

This holds user-set speeds steady without making Hypercolor a fan-curve
product. Exact clock-blob field values are validated on hardware before the
descriptor ships enabled (§11). Both upkeep streams and the GetDev poll run
as periodic protocol upkeep through the same actor seam Corsair's LCD
keepalive uses.

### 6.9 tinyuz compression

The firmware decompresses RGB payloads with **tinyuz** (sisong/tinyuz, the
HDiffPatch-family embedded LZ codec), configured with a **4 KB dictionary**.
The reference driver vendors the upstream C library over FFI; Hypercolor does
not (workspace `unsafe_code = forbid`, and the HAL takes no C toolchain
dependency). Instead:

- `lianli/wireless/tinyuz.rs` implements a **pure-Rust tinyuz encoder**
  producing streams valid for a 4 KB-dictionary decoder. The normative
  bitstream definition is the upstream tinyuz repository (pin the commit in
  the module header at implementation time). Encoder scope: correct streams
  first; compression ratio is secondary (a worst-case TL cluster is
  4 × 26 × 3 = 312 raw bytes — even a literal-heavy stream fits comfortably
  in a few 220-byte chunks).
- Conformance is proven without C in CI (§12): a small pure-Rust tinyuz
  **decoder in test code**, itself validated against committed
  (input, reference-compressed) fixture pairs generated offline with the
  upstream tools; our encoder round-trips through that validated decoder.

## 7. Wireless LCD Receiver Protocol (0x1CBE)

The wireless panel path. One bulk device per LCD fan (`0x0006` TL V2,
`0x0005` SL V3 — same protocol, shared implementation). Confirmed on
hardware; header builder and frame path verified against upstream source
(`crypto.rs`, `slv3_lcd.rs`).

### 7.1 DES-wrapped 512-byte command header

Every command starts with a 512-byte encrypted header: a **504-byte
plaintext** encrypted with DES-CBC, PKCS#7 padding, key = IV = ASCII
**`slv3tuzx`**. 504 is block-aligned, so PKCS#7 appends one full 8-byte
padding block: the ciphertext is exactly **512 bytes** and fills the header.

Plaintext layout (504 bytes):

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | command | §7.2 | |
| 1 | 1 | reserved | `0x00` | |
| 2 | 1 | magic | `0x1A` | |
| 3 | 1 | magic | `0x6D` | |
| 4–7 | 4 | timestamp | u32 **LE**, ms | monotonic: if a new raw value ≤ the last sent, send last + 1 |
| 8–503 | ≤496 | params | | command-specific, zero-padded |

(The WinUSB variant — Flex and friends, research-only — encrypts a 500-byte
plaintext instead: PKCS#7 pads 500 → 504 ciphertext bytes, placed at
[0..504] of a 512-byte frame with [504..510] zero and trailer `[510]=0xA1`,
`[511]=0x1A`.)

Implementation: RustCrypto `des` + `cbc` crates (pure Rust; add to
`deny.toml` review). DES here is obfuscation, not security — the key ships in
every L-Connect install and multiple public repos.

### 7.2 Command vocabulary

| Cmd | Byte | Params (plaintext offset 8+) | Purpose |
|---|---|---|---|
| GetVer | `0x0A` | none | firmware version; reply §7.3 |
| Reboot | `0x0B` | none | reboot MCU; **no response read** |
| Rotate | `0x0D` | [8] = rotation & 0x03 | 0–3 = 0/90/180/270° |
| Brightness | `0x0E` | [8] = mapped value | firmware LUT anchors: 0→0, 25→10, 50→30, 75→40, 100→100, linear interpolation between (e.g. 37→20, 62→35), input clamped to 100 |
| FrameRate | `0x0F` | [8] = fps | init sends **120** |
| SetClock / StopClock | `0x33`/`0x34` | date/time + mode | on-device clock overlay (unused v1) |
| **PushJpg** | `0x65` | [8..12] = JPEG size, u32 **BE** | JPEG frame push — the main path |
| PushPng / ClearPng | `0x66`/`0x67` | size u32 BE / none | PNG overlay layer (unused v1) |
| CheckNewLcd | `0x80` | none | probe hardware revision (init step 1) |
| SwitchToDesktop | `0x96` | none | desktop-monitor mode (unused) |

### 7.3 Session model

- **Init:** CheckNewLcd (`0x80`) → FrameRate(120) → GetVer (`0x0A`). After
  every command (init and steady-state alike, Reboot excepted) the host
  performs one **tolerant drain read** (≤511 bytes, standard timeout, result
  logged and otherwise ignored — the reference discards these status packets
  and their layout is undocumented). The GetVer reply is the exception worth
  parsing: plaintext, firmware version as a NUL-terminated ASCII string
  starting at byte 8.
- **Frame:** one `PushJpg` header + JPEG payload in a single **fixed
  102,400-byte** zero-padded bulk write — header at [0..512], JPEG at
  [512..512+len], zeros to the end — via `encode_prefixed_display_frame`
  with `fixed_frame_len: Some(102_400)` (§4.2), then the tolerant drain
  read. Max JPEG payload: **101,888 bytes**; larger frames are a
  `PayloadTooLarge` error, prevented upstream by the daemon's encoded-size
  budget (§10). 400x400 at quality ~85 lands ~30–60 KB, comfortably inside.
- **Settings:** Brightness/Rotate map to the §4.4 `DisplaySetting` hook, with
  the firmware brightness LUT owned by this protocol. Init sets brightness
  100 (hardware) and rotation 0; the daemon's software LUT stays the runtime
  brightness authority.
- **Capabilities:** as §5.6 but the panel refreshes at 60 Hz; frame delivery
  stays daemon-capped (`max_fps: 30`, revisit upward after hardware
  measurement — never downward).
- **Identity:** receiver serial uniqueness is unverified (§11.7). The
  descriptor carries the same `SerialQuirk` machinery as §5.7, with the
  placeholder list filled in from hardware observation.

## 8. Topology & Zones

- **Wired LCD panel / wireless LCD receiver:** one `SegmentInfo` per device:
  `"Display"`, `led_count: 0`, `DeviceTopologyHint::Display { width: 400,
  height: 400, circular: true }`, `DeviceColorFormat::Jpeg`. No LED zones.
- **Wireless dongle:** one `DeviceId` for the dongle; one `SegmentInfo` per
  discovered fan slot (from GetDev records), `DeviceTopologyHint::Ring
  { count: 26 }`, `DeviceColorFormat::Rgb`, mirroring how the wired TL hub
  emits per-fan segments today. Fan-type bytes select per-slot naming
  (TL V2 / TL V2 LCD / TL V3).
- **Wired TL hub:** unchanged (spec 19 §6): per-fan ring segments; the LCD
  adds nothing to the hub device. The 20-vs-26 LED question is §11.1.
- **Attachment templates:** new `data/attachments/builtin/lian-li/` entries
  for the TL LCD fan (26-LED ring + centered display) alongside the existing
  `lian-li-tl-fan.toml` ring mapping.

## 9. HAL Integration

### 9.1 Module layout

```
crates/hypercolor-hal/src/display/           # NEW shared layer (§4)
  mod.rs          # ChunkContext, ChunkCommandPolicy, DisplayChunkLayout,
                  # engines, DisplaySetting, DisplayEncodeError
  repack.rs       # RGB565/BGR565 packing, XOR masks, line padding
  keepalive.rs    # WireKeepalive
crates/hypercolor-hal/src/drivers/lianli/
  lcd.rs          # wired TL LCD protocol (§5)
  wireless/
    mod.rs        # dongle protocol: discovery, envelope framing, upkeep
    rgb.rs        # RF_SET_RGB encoding
    tinyuz.rs     # pure-Rust tinyuz encoder (§6.9)
    lcd.rs        # 0x1CBE receiver protocol (§7)
    crypto.rs     # DES-CBC header wrap (des + cbc crates)
  devices.rs      # descriptors appended
```

### 9.2 Descriptors

All registered in `LIANLI_DESCRIPTORS` with `DeviceFamily::new_static
("lianli", "Lian Li")`, predicate-free:

| Descriptor | Transport | Protocol binding | Notes |
|---|---|---|---|
| `0x04FC:0x7393` "Uni Fan TL LCD" | `TransportIntent::Hid` per §5.1, resolved via `resolve_current_transport` | `lianli/tl-lcd` | `serial_quirk: PlaceholderValues(["TL_LCDV0.1"])` |
| `0x0416:0x8040` "L-Wireless Controller" | `TransportType::DriverUsb` (factory opens TX bulk + RX `0x8041` companion under the same hub) | `lianli/wireless` | bulk endpoints auto-discovered on both halves (observed `0x01`/`0x81`) |
| `0x1CBE:0x0006` "Uni Fan TL Wireless LCD" | `UsbBulk { interface: 0, report_id: 0 }` | `lianli/wireless-lcd` | serial quirk pending hardware observation (§11.7) |
| `0x1CBE:0x0005` "Uni Fan SL Wireless LCD" | `UsbBulk { interface: 0, report_id: 0 }` | `lianli/wireless-lcd` | |

**Not registered in v1:** the V2 dongle (`0x1A86:0xE304`) — a registered
descriptor is a live discovery binding, and V2 is unvalidated (§11.5); its
descriptor is added by the validation follow-up, not shipped dormant. Also
deliberately never registered: RX-dongle PIDs (`0x8041`, `0xE305`), the V2
HID companion (`0x2107`), and the Flex IDs.

The existing wired TL descriptor stays as-is; migrating it from its raw
`TransportType` literal to `TransportIntent` is a nice-to-have riding along
only if zero-risk.

### 9.3 HAL descriptor vs DriverModule

HAL descriptors, all four registered in v1 (wired LCD, V1 dongle, both
`0x1CBE` receivers). Every device enumerates by VID/PID; discovery of
fans behind the dongle is a handshake exactly like the wired TL hub's `0xA1`;
no driver-scoped config, stored credentials, or active network discovery is
required for bound operation. Pairing is the one flow that would want a
driver surface, and it is explicitly deferred (§6.4). The wireless dongle's
periodic upkeep (§6.8) rides the existing protocol keepalive seam, not a
driver module.

### 9.4 Cross-cutting protocol changes

- `ProtocolCommand.response: ResponsePlan { count, timeout, capacity,
  tolerance }` (§4.5), with the actor read loop, per-command timeout
  override, and bulk multi-packet logical reads to match.
  `ResponseTolerance::Optional` completes a command whose reply never
  arrives (the wireless LCD receiver's status packets, the TL hub's second
  `0xA6` report). `ChunkCommandPolicy` carries a plan the display engines
  apply per chunk. The wired TL hub's `0xA6` adopts `count: 2` plus the
  first-report-wins parse fix.
- `TransferType::Companion` and `transport::companion::CompanionTransport`
  for devices made of two USB functions (§6.1).
- `DeviceDescriptor.serial_quirk: Option<SerialQuirk>` consulted by the USB
  scanner (§5.7, §10).
- The display seam is one hook, `Protocol::encode_display_payload_into ->
  Result<(), DisplayEncodeError>`; the JPEG-only `encode_display_frame`
  pair and the §4.4 settings hook were removed in review (no caller; the
  daemon owns brightness, rotation, and frame rate).
- New crate deps: `des`, `cbc` (RustCrypto; `cargo deny` review).

### 9.5 Wire structs

Every fixed-layout packet gets a zerocopy struct with a compile-time size
assert, per spec 62: the wired LCD 512-byte packet (11-byte header), the
64-byte dongle USB packet, the 240-byte RF envelope, the 42-byte GetDev
record (parse side), and the 504-byte DES header plaintext. Multi-byte
fields use `zerocopy::byteorder` types with explicit endianness — note the
mix: wired LCD counters and RF sizes are **BE**, the DES header timestamp is
**LE**.

## 10. Daemon & Data Integration

Two additive daemon changes (the pipeline itself — workers, lanes, faces,
previews, simulator — is untouched):

- **Encoded-frame size budget.** `DeviceFeatures` gains
  `max_display_frame_len: Option<usize>` (the wireless receiver sets
  101,888; other displays leave it `None`). `display_output`'s encode step
  checks the budget after compression; on overflow it re-encodes with
  stepped-down quality (bounded ladder, e.g. 85 → 70 → 55 → 40), and if the
  floor still overflows it drops the frame with a rate-limited warning
  rather than sending a truncated JPEG. Covered by a high-entropy-frame test
  (§12).
- **Serial-quirk fingerprinting.** The USB scanner consults
  `descriptor.serial_quirk` before building the `DeviceIdentifier`: a
  placeholder serial is treated as absent, so the fingerprint keys on the
  USB port path, and the portable-identity claim is skipped (its existing
  refusal path). Covered by a two-panels-same-serial test (§12).

Registration and data surfaces:

- **Displays API:** panels appear in `GET /api/v1/displays` automatically
  once their segments carry a `Display` topology hint; display faces,
  previews, and the simulator work unchanged. Verify
  `DisplayDescriptor::derive()` classifies a circular 400x400 fan panel
  sensibly (`Round` shape; class label is cosmetic follow-up).
- **Delivered frame rates, stated honestly:** capabilities advertise the
  panel hardware limit (`max_fps: 30`), but the daemon's own caps govern
  delivery — JPEG scene output is capped at 15 fps
  (`DISPLAY_OUTPUT_MAX_FPS`), display faces at 30
  (`DISPLAY_FACE_DEFAULT_FPS`). v1 ships inside those existing caps: faces
  run at 30, scene mirroring at 15. Raising the JPEG scene cap (or adding a
  per-device ceiling above it) is a named performance follow-up taken only
  with encode-cost measurements across a multi-panel rig. **The existing
  caps are floors:** per the repository performance contract, measurement
  may justify raising them, never lowering — matching §7.3's
  "never downward" rule for panel delivery rates.
- **`data/drivers/vendors/lianli.toml`:** new `[[devices]]` entries for
  `0x7393` (type `lcd`), the V1 dongle (type `fan_controller`), and the
  `0x1CBE` receivers (type `lcd`), each with per-device VID override since
  three of them are not `0x0416`. V2 dongle, TL V3, and Flex IDs land as
  `status = "researched"` until validated. Then `just compat`.
- **udev:** `0x0416` is already covered vendor-wide (hidraw + usb). Add:
  `0x04FC:0x7393` hidraw + usb (HID transport), `0x1CBE:0x0005/0x0006` usb
  (bulk), `0x1A86:0xE304` usb. Keep them PID-scoped — `0x04FC`, `0x1CBE`,
  and `0x1A86` are all shared/borrowed VIDs (Sunplus cameras, TI dev boards,
  every CH340 serial adapter on earth), so vendor-wide rules would grab
  unrelated hardware. The udev-rules test enforces the transport/rule
  pairing.

## 11. Open Questions & Risks

1. **Wired TL LED count: 20 or 26?** The reference driver hardcodes 20
   LEDs/fan for the wired hub; Lian Li marketing and the wireless firmware
   say 26 (2 zones, dual infinity mirror). Our wired TL driver uses 26 today.
   Resolve on hardware (drive LEDs 20–25 explicitly and look); until then the
   existing 26 stands.
2. **Wired hub per-LED streaming does not publicly exist.** Both reference
   implementations drive only firmware effects + per-fan color on
   `0x0416:0x7372`; SignalRGB says the same. Our wired TL driver's
   average-to-one-color behavior is at the known ceiling. Getting true
   per-LED on wired TL needs an original USB capture of L-Connect (if
   L-Connect even does it — it may not). Sidequest logged; not a blocker.
3. **Clock-sync blob fidelity.** The §6.8 hold-steady policy is decided, but
   the exact per-field tolerances of the 220-byte clock blob (which zeroed
   sensor fields the firmware accepts without side effects) need hardware
   validation before the dongle descriptor ships enabled.
4. **RF RGB throughput ceiling is unmeasured.** Compressed frame size ×
   220-byte chunks × 2 ms pacing bounds the live per-LED rate; nobody has
   published numbers. Measure on hardware, then set the dongle protocol's
   `max_fps`/`frame_interval` from data (provisional at implementation:
   the wired TL hub's 100 ms interval as a floor, adjusted by measurement in
   either direction). No preemptive caps.
5. **V2 dongle deltas.** V2 (`0x1A86`) appears to add an HID-flavored path
   (the reference has a dedicated module for it) alongside behavior changes
   (RPM sync moved into receivers, no signal-loss spin-up). V1 is the
   reference target; the V2 descriptor is **not registered in v1** (§9.2) —
   it is added by a follow-up only after its quirks are captured on
   hardware.
6. **TL V3 protocol identity** is assumed equal to V2 (same reference code
   paths, new fan-type bytes) — capture-unverified; gated with V2.
7. **Wireless LCD receiver identity + hotplug.** Receiver serial uniqueness
   is unverified; the descriptor carries `SerialQuirk` machinery with the
   placeholder list to be filled from hardware. Panels arrive via USB
   splitters; 9-at-once enumeration and multi-panel bulk throughput need
   hardware validation.
8. **Wired multi-panel concurrency** (§5.8): per-device actors vs the
   reference's global chain lock — validate on hardware.
9. **Config-to-protocol bridge.** `WriteSerial` adoption (§5.7) and any
   future per-driver tunable need a route from daemon config into HAL
   protocol construction; none exists (`ProtocolFactory` is
   configuration-free). Design that bridge as its own small spec when the
   first real consumer lands; nothing in v1 depends on it.
10. **Hardware findings, 2026-09-04 (V1 controller, no fans bound).**
    Master query answers on channel 8 first try (MAC `a0:71:ae:72:ab:3c`,
    firmware `0x0010`). GetDev lives on the RX and answers page-sized
    (§6.5). Bytes 7–10 of the TX status packet are a running counter that
    froze once video mode was entered. Everything below needs powered
    fans: real GetDev records, the live RGB tick rate (§11.4), the
    clock-blob tolerance (§11.3), receiver serial uniqueness (§11.7), and
    whether the TX's per-command status packets, which the RF send path
    never reads (the reference does not read them either), ever back up
    the device under sustained streaming.
11. **DES/`slv3tuzx` in open source.** The key is already public in multiple
    repos and in every L-Connect install; shipping it is documentation of an
    interoperability fact, not a secret. Noting for license/audit review.

## 12. Testing Strategy

All hardware-free, in `crates/hypercolor-hal/tests/` unless noted:

- **`display_layer_tests.rs`** — engine edge cases: payload exactly at
  max_payload, max_payload+1, zero-length data (no packets emitted — the
  Corsair empty-JPEG regression guard), single chunk, final-flag on the last
  packet only, sequence width/endianness, per-chunk `ChunkCommandPolicy`
  application, `fixed_frame_len` zero-padding, `PayloadTooLarge` and
  `TooManyChunks` errors plus the skip-and-warn seam mapping, scratch reuse
  across frames.
- **Corsair/Push 2 migration** — the existing `corsair_lcd_display_tests`
  (23 tests) and `push2_display_tests` suites pass unmodified: byte-identical
  output is the acceptance gate for §4.6.
- **`lianli_lcd_tests.rs`** — wired panel: 512-byte packet header fields (BE
  total size repeated per chunk, u24 counter resetting per transfer, u16
  payload length), 501/502-byte chunk boundaries, WriteSyncJpg emits no-ack
  commands vs WriteJpg per-chunk acks with `response_count: 1` each,
  LcdControl payload bytes (mode/brightness/fps/rotation offsets), init
  sequence order and timeouts (3000 ms init vs 200 ms steady),
  GetProductInfo `response_count: 2`, ReadSerial reply parse
  (serial/port/index), handshake reply parse.
- **`lianli_wireless_tests.rs`** — RF envelope: 240-byte buffer split into
  4×60 chunks with correct USB header bytes and 2 ms pacing; MAC placement;
  GetDev record parse (validation byte `0x1C` rejection, RPM/status nibble
  unpacking, fan-type classification incl. gated V3 bytes); PWM frame layout
  and hold-steady value passthrough; RGB header frame fields (compressed
  length BE at [20..24], total packets = chunks+1, total_frames, LED count,
  interval), header repeat pacing (2 ms ≤2 repeats, else 20 ms), 220-byte
  chunk boundaries, SL-INF per-fan reversal (and TL non-reversal).
- **`lianli_wireless_lcd_tests.rs`** — DES known-answer vectors: fixed
  plaintext (pinned timestamp) → exact 512-byte ciphertext with `slv3tuzx`
  (vectors generated at implementation with an independent DES-CBC tool, not
  our own code); plaintext magic `0x1A 0x6D`; LE timestamp monotonic bump
  rule; PushJpg params (size u32 BE at [8..12]); fixed 102,400-byte frame
  with header at [0..512], JPEG at [512..], zeros after; 101,888 cap
  enforced; brightness LUT anchors + interpolation (37→20, 62→35, clamp
  200→100); init order; Reboot sends no read.
- **`tinyuz_tests.rs`** — the §6.9 conformance architecture: a pure-Rust
  test decoder validated against committed (input, reference-compressed)
  fixture pairs; our encoder round-trips through that decoder for solid,
  gradient, alternating, palette, and multi-fan frames; determinism check.
- **Actor/protocol plumbing** (core tests) — `response_count` read loop: a
  two-report command followed by another command stays in sync;
  `response_timeout` override reaches the transport; `response_len`
  capacity + completion rule drives multi-packet bulk reads (GetDev replies
  with 2, 6, and 12 records over a 64-byte-packet mock transport — the
  6-record case pins gap-termination on an exact packet boundary). TL hub
  final-state test: after a full `0xA6` exchange the stored firmware is the
  version string, not the build date.
- **Scanner identity** (core tests) — two devices, same VID/PID, both serial
  `TL_LCDV0.1`, different paths → two distinct fingerprints under the
  descriptor quirk; without the quirk they collapse (regression guard).
- **Daemon budget** (`hypercolor-daemon` tests) — high-entropy 400x400 frame
  exceeding 101,888 at base quality: bounded step-down, then frame drop with
  warning at the floor; budget-less devices unaffected.
- **Database/udev** — descriptor lookup tests for each new VID/PID;
  udev-rules pairing test covers the new IDs.
- **Integration without hardware** — the display simulator (spec 41)
  exercises the daemon-side path; a mock-transport actor test drives
  `encode_display_payload_into` end-to-end for both LCD protocols.

## References

- `sgtaziz/lian-li-linux` — hardware-tested Rust implementation; primary wire
  source. The following files were read directly (main branch, 2026-08-30)
  and are the byte-level authority behind §5–§7: `lianli-devices/src/
  tl_lcd.rs`, `crypto.rs`, `slv3_lcd.rs`, `wireless/rgb.rs`, `tinyuz.rs`
  (FFI bindings; upstream codec is sisong/tinyuz, 4 KB dictionary). Protocol
  facts verified; no code reused.
- `sgtaziz/tl-lcd-linux` (archived) — independent confirmation of the
  DES-CBC `slv3tuzx` header wrap.
- `lewisgibson/FanControl.LianLi` — L-Connect 3 decompile-derived
  `docs/protocol.md` + `docs/lighting.md`; byte-level cross-corroboration of
  the wired TL hub commands.
- OpenRGB issue #4882 (Uni Hub TL request; unimplemented) and SignalRGB forum
  threads — ecosystem status; confirm no per-LED wired path is known.
- linux-hardware.org probes for `0416:7372`, `04fc:7393`, `0416:8040` —
  real-system enumeration evidence. uni-sync issue #38 — V1 dongle lsusb.
- Lian Li: TL LCD / TL Wireless / TL Flex product pages, Uni Hub TL page,
  TL fan-number guide, wireless FAQ, L-Connect 3 changelog, L-Wireless
  controller revision notice. FCC grantee `2ANYX` filings `RF-T-B`,
  `RF-R-LCD-B` (radio chip identity unread; exhibits blocked to bots).
- In-repo: spec 19 (Lian Li Uni Hub driver), spec 18 (Corsair, LCD prior
  art), spec 62 (zerocopy structs), spec 41 (display simulator), spec 42
  (display faces), spec 69 (display descriptor taxonomy),
  `docs/research/faces/sensor-dashboards.md` (TL LCD hardware facts).

## Review History

- **R5 (2026-08-30, Codex, single-line arithmetic confirmation):** **PASS.**
- **R4 (2026-08-30, Codex xhigh, certification):** NEEDS_CHANGES — 5/5
  round-3 fixes verified consistent; one arithmetic typo (§6.5 two-record
  example "88 + 4" → "88 = 4 + 2 × 42"), fixed.
- **R3 (2026-08-30, Codex xhigh, convergence check):** NEEDS_CHANGES —
  6/9 round-2 fixes verified; residuals: a stale WriteSerial cross-reference,
  two leftover "validation-gated descriptor" phrasings contradicting the
  not-registered decision, and a stale descriptor count. New: `response_len`
  redefined as receive capacity with an explicit completion rule (short
  packet / capacity / 20 ms inter-packet gap — a six-record GetDev reply is
  exactly 256 bytes with no short-packet terminator) plus a boundary test;
  the §10 fps clause rewritten as floors-only to match the repository
  performance contract. All five adopted.
- **R2 (2026-08-30, Codex xhigh, fix-verification pass):** NEEDS_CHANGES —
  10/16 round-1 fixes verified, 3 incomplete, 3 regressed, 6 new findings;
  all adopted. Headline fixes: GetDev multi-packet bulk reads via
  `response_len` (the 64-byte endpoint packet would have truncated
  12-record replies); first-report-wins `0xA6` parse so the build-date
  report cannot overwrite the firmware version; empty-frame behavior now
  emits nothing (preserving Corsair's byte-identical migration); fallible
  framing engines with an explicit skip-and-warn seam mapping; per-command
  `response_timeout`; WriteSerial dropped from v1 (no config-to-protocol
  bridge exists); V2 dongle descriptor unregistered rather than
  "gated-but-live"; honest delivered-fps statement (15 fps JPEG scene /
  30 fps faces under current daemon caps); remaining `Value` columns and
  reserved-byte rows added.
- **R1 (2026-08-30, Codex xhigh, hostile six-lens pass):** NEEDS_CHANGES —
  5 blockers, 8 majors, 3 minors; all 16 verified against the repo and
  upstream source and adopted. Headline fixes: DES 504→512 PKCS#7
  arithmetic; fixed 102,400-byte zero-padded wireless frame; daemon
  encoded-size budget replacing the impossible HAL-side quality step-down;
  descriptor serial quirk for placeholder-serial fingerprinting;
  `response_count` for two-report commands; constructible `UsbBulk`
  descriptor fields; §4 reshaped from framework trait to helper library with
  per-chunk command policy; tinyuz conformance architecture; decided §6.8
  hold-steady keepalive policy; staged acceptance criteria; wire tables
  normalized to house format; corrected wired-LCD init/mode facts and
  response layouts from upstream source.

### Round 6 (2026-09-04, implementation review, PRs #238/#239 and wave 3)

Four parallel review lanes (abstraction fitness, many-panel scale, dead and
duplicate code, plumbing correctness) plus a hardware probe of the V1
controller. Landed: `ResponsePlan` with a tolerance; one display hook with
an error channel; the settings hook, the JPEG-only hook pair, and the
backend's JPEG-only write pair deleted; the display segment made the single
source of display truth (`DisplaySurface`, `DeviceColorFormat::Jpeg` gone,
capability fields derived at adoption); the companion transport seam; the
RX-side GetDev correction above. Deferred with sizing: per-transport IO
threads and one blocking call per display frame (wired multi-panel
throughput), encode/transport pipelining, content-shared face sessions,
macOS/Windows HID identity for identical-serial panels, command-aware
`parse_response` for acked uploads, wider pixel repack.
