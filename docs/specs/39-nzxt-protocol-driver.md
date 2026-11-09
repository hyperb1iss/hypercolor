# 39 -- NZXT Protocol Driver

> Native USB HID and USB serial driver for the NZXT ecosystem. This document is the authoritative packet specification; any gap called out here requires fresh USB capture or a new facts-only spec before implementation.

**Status:** Draft (Rev 3 -- Phase 1 lighting implemented, unverified on hardware)
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::nzxt`
**Author:** Nova
**Date:** 2026-04-10
**Revised:** 2026-11-05 (Rev 3 adds implementation readings and the Phase 1 gap register; no new protocol facts)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Implementation Boundary](#2-implementation-boundary)
3. [Device Registry](#3-device-registry)
4. [Protocol Families](#4-protocol-families)
5. [Topology Catalog](#5-topology-catalog)
6. [HAL Integration](#6-hal-integration)
7. [Render and Control Strategy](#7-render-and-control-strategy)
8. [Testing Strategy](#8-testing-strategy)
9. [Research Gaps and Phase Plan](#9-research-gaps-and-phase-plan)
10. [Recommendation](#10-recommendation)
11. [References](#references)

---

## 1. Overview

Native driver for NZXT hardware via the `hypercolor-hal` abstraction layer. The NZXT ecosystem is not one protocol. It is at least seven distinct protocol families:

1. Hue Plus legacy USB serial.
2. Smart Device V1 / Hue 1 HID.
3. Gen2 NZXT HID controllers shared by Hue 2, Smart Device V2, RGB and Fan Controller, Control Hub, several motherboards, and some AIO RGB paths.
4. Legacy Kraken X2/M2 AIO HID.
5. Modern Kraken AIO telemetry and fan-control HID.
6. NZXT peripheral HID (`0x43` command family) for keyboards and mice.
7. LCD/screen transport for Kraken screen AIOs, which requires dedicated capture before implementation.

The clean-room implementation should therefore be organized around protocol families, not product marketing names.

### Core conclusions

- This spec is sufficient for the protocol families it describes. Unresolved legacy details and LCD transport must be filled by fresh captures or new facts-only research notes.
- Most NZXT RGB channels use **GRB** byte order.
- The main Gen2 HID RGB stream is a 64-byte packet family built around `0x22`.
- Motherboards reuse Gen2 digital RGB packets but add a separate analog 12V RGB path via `0x2A`.
- Kraken Elite v2 (`0x3012`) is a transport variant: same high-level semantics, but **512-byte reports** and `0x26` lighting commands.
- LCD support for Kraken Z/Elite families is **not** specified here. Do not claim screen support until an LCD packet map is captured and added.

---

## 2. Implementation Boundary

This document is the authoritative specification for the NZXT packet formats it describes.
Implementation work should use this spec, hardware captures, and Hypercolor's own HAL
patterns. If a field, command, topology ID, timing rule, or LCD transfer path is missing,
stop at the documented behavior, capture the missing traffic, and extend this spec before
writing code for that behavior.

Do not infer support from product marketing names. Only implement a capability when this
document defines the transport, packet framing, byte layout, update timing, and testable
expected behavior.

---

## 3. Device Registry

### 3.1 Legacy controllers

| VID:PID     | Family          | Product                 |
| ----------- | --------------- | ----------------------- |
| `04D8:00DF` | Hue Plus serial | Hue Plus                |
| `1E71:1714` | Smart Device V1 | Hue 1 / Smart Device V1 |
| `1E71:170E` | Legacy Kraken   | Kraken X2               |
| `1E71:1715` | Legacy Kraken   | Kraken M2               |

### 3.2 Gen2 controller family

| PID      | Product                                         |
| -------- | ----------------------------------------------- |
| `0x2001` | Hue 2                                           |
| `0x2002` | Hue 2 Ambient                                   |
| `0x2005` | N7 Z390 / motherboard-family controller         |
| `0x2006` | Smart Device V2                                 |
| `0x2007` | Kraken X3 Series                                |
| `0x2009` | RGB and Fan Controller                          |
| `0x200A` | N7 Z490                                         |
| `0x200B` | N7 B550                                         |
| `0x200C` | N7 Z590                                         |
| `0x200D` | Smart Device V2 Case Controller                 |
| `0x200E` | RGB and Fan Controller                          |
| `0x200F` | Smart Device V2 Case Controller                 |
| `0x2010` | RGB and Fan Controller                          |
| `0x2011` | RGB and Fan Controller                          |
| `0x2012` | RGB Controller / RGB and Fan Controller variant |
| `0x2014` | Kraken X3 Series RGB                            |
| `0x2016` | N7 Z690                                         |
| `0x2017` | N5 Z690                                         |
| `0x2019` | RGB and Fan Controller                          |
| `0x201B` | N7 B650E                                        |
| `0x201D` | N7 Z790                                         |
| `0x201F` | RGB and Fan Controller                          |
| `0x2020` | RGB and Fan Controller                          |
| `0x2021` | RGB Controller / RGB and Fan Controller variant |
| `0x2022` | Control Hub                                     |

### 3.3 Screen and modern AIO family

| PID      | Product                          | Capabilities specified here                                                 |
| -------- | -------------------------------- | --------------------------------------------------------------------------- |
| `0x3008` | Kraken Z3                        | RGB, pump/fan telemetry, pump/fan control                                   |
| `0x300C` | Kraken Elite                     | Pump/fan telemetry, pump/fan control; RGB/LCD unresolved                    |
| `0x300E` | Kraken                           | Pump/fan telemetry, pump/fan control; RGB/LCD unresolved                    |
| `0x3012` | Kraken Elite v2 / 2024 Elite RGB | 24-LED ring RGB, external RGB channel, pump/fan telemetry, pump/fan control |

### 3.4 Peripheral family

| PID      | Product              |
| -------- | -------------------- |
| `0x2100` | Lift Mouse           |
| `0x2103` | Function             |
| `0x2104` | Function TKL         |
| `0x2105` | Function MiniTKL     |
| `0x2106` | Function ISO         |
| `0x2107` | Function TKL ISO     |
| `0x2108` | Function MiniTKL ISO |
| `0x2130` | Function 2           |
| `0x2131` | Function 2 ISO       |
| `0x2136` | Function 2 variant   |

### 3.5 Partner kit on NZXT protocol

| PID      | Product                        |
| -------- | ------------------------------ |
| `0x2004` | Vertagear RGB LED Upgrade Kits |

### 3.6 Gen2 accessory IDs

These IDs appear in the Gen2 topology response (`0x21 0x03`) and determine segment names and LED counts.

| ID     | Accessory                  | LEDs |
| ------ | -------------------------- | ---- |
| `0x01` | Hue 1 strip                | 10   |
| `0x02` | Aer 1 fan                  | 8    |
| `0x04` | Hue 2 strip 10 LED         | 10   |
| `0x05` | Hue 2 strip 8 LED          | 8    |
| `0x06` | Hue 2 strip 6 LED          | 6    |
| `0x08` | Hue 2 cable comb           | 14   |
| `0x09` | Hue 2 underglow 300 mm     | 15   |
| `0x0A` | Hue 2 underglow 200 mm     | 10   |
| `0x0B` | Aer 2 120                  | 8    |
| `0x0C` | Aer 2 140                  | 8    |
| `0x10` | Kraken X3 ring             | 8    |
| `0x11` | Kraken X3 logo             | 1    |
| `0x13` | F120 RGB                   | 18   |
| `0x14` | F140 RGB                   | 18   |
| `0x15` | F120 RGB Duo               | 20   |
| `0x16` | F140 RGB Duo               | 20   |
| `0x17` | F120 RGB Core              | 8    |
| `0x18` | F140 RGB Core              | 8    |
| `0x19` | F120 RGB Core case version | 8    |
| `0x1D` | F360 RGB Core case version | 24   |
| `0x1E` | Kraken Elite ring          | 24   |
| `0x1F` | F420 RGB                   | 24   |

---

## 4. Protocol Families

### 4.1 Hue Plus serial

**Transport:** USB serial  
**VID:PID:** `04D8:00DF`  
**Baud:** `256000`  
**Packet size:** `125` bytes  
**Color order:** `GRB`

Hue Plus is a fully separate protocol from every HID-based NZXT family.

#### Topology probe

Write 2 bytes:

```text
8D <channel>
```

Read 5 bytes:

- If byte `3 == 0x01`, LED count = byte `4 * 8`
- Otherwise LED count = byte `4 * 10`

#### Lighting packet

```text
byte 0  = 0x4B
byte 1  = channel + 1
byte 2  = mode
byte 3  = flags (bit 4 = direction)
byte 4  = (color_idx << 5) | speed
byte 5+ = color payload
```

Observed behavior:

- Direct mode can address up to 40 LEDs.
- Indexed/effect mode can send up to 8 colors.
- Indexed-color modes expand color slots across the controller's internal 40-LED buffer before transmit.

#### Implementation note

This should be implemented as its own `UsbSerialProtocol`, not folded into the HID family.

### 4.2 Smart Device V1 / Hue 1

**Transport:** USB HID  
**VID:PID:** `1E71:1714`  
**Interface:** `0`  
**Usage page:** `0xFF00`  
**Usage:** `0x0001`  
**Collection:** `0x0000`  
**Report size:** `65` bytes  
**Color order:** `GRB`

#### Initialization

Initialization sequence:

1. Write `[0x01, 0x5C]`
2. Write `[0x01, 0x5D]`
3. Read until a 21-byte response is received

The init response yields:

- Firmware major at byte `0x0B`
- Firmware minor at byte `0x0E`
- Device count at byte `0x11`
- Accessory type from `usb_buf[0x10] >> 3`

Accessory type mapping:

- `0` = Hue+ strip, 10 LEDs each
- `1` = Aer RGB fan, 8 LEDs each

All connected accessories are expected to be homogeneous.

#### Direct RGB streaming

Packet 1:

```text
byte 0  = 0x02
byte 1  = 0x4B
byte 2  = mode
byte 3  = flags (bit 4 = direction)
byte 4  = (color_idx << 5) | speed
byte 5+ = first 20 LEDs * 3 bytes
```

Packet 2:

```text
byte 0  = 0x03
byte 1+ = remaining color bytes
```

#### Effect IDs

| Mode          | ID     |
| ------------- | ------ |
| Fixed         | `0x00` |
| Fading        | `0x01` |
| Spectrum      | `0x02` |
| Marquee       | `0x03` |
| Cover Marquee | `0x04` |
| Alternating   | `0x05` |
| Pulsing       | `0x06` |
| Breathing     | `0x07` |
| Alert         | `0x08` |
| Candlelight   | `0x09` |
| Wings         | `0x0C` |
| Wave          | `0x0D` |

#### Scope decision

Phase 1 should support direct RGB only. Hardware effects can wait.

### 4.3 Gen2 NZXT controller family

This is the main NZXT protocol family and the most important implementation target.

**Transport:** USB HID  
**Report size:** `64` bytes  
**Color order:** `GRB`  
**Used by:** Hue 2, Smart Device V2, RGB and Fan Controller, Control Hub, several motherboard digital headers, Vertagear kit, Kraken X3 external channel, Kraken Z3 external channel, and some Elite RGB paths.

#### Core command set

| Purpose                | Request          | Response                 |
| ---------------------- | ---------------- | ------------------------ |
| Firmware query         | `10 01`          | `11 01`                  |
| LED topology query     | `20 03`          | `21 03`                  |
| Direct RGB stream      | `22 1n ...`      | none required            |
| Apply stream           | `22 A0 ...`      | none required            |
| Fan poll enable        | `60 03`          | none documented          |
| Fan poll interval      | `60 02 ...`      | none documented          |
| Fan set                | `62 01 ...`      | async fan packet follows |
| Fan state async packet | none             | `67 02`                  |

In the stream request, `n` is the packet number in the low nibble of byte 1 (see the direct RGB stream layout below).

#### Firmware query

Request:

```text
10 01
```

Response:

```text
11 01 .... fw_major fw_minor fw_patch
```

Known tested firmware: `1.13.0`.

#### Topology query

Request:

```text
20 03
```

Response begins:

```text
21 03 ...
```

Observed fields:

- Channel count at byte `14`
- Per-channel accessory slots begin at byte `0x0F`
- Accessory records are 6 bytes per channel and map accessory IDs through the table in §3.6

#### Direct RGB stream

Per packet:

```text
byte 0 = 0x22
byte 1 = 0x10 | packet_number
byte 2 = 1 << channel
byte 3 = 0x00
byte 4+ = raw color payload
```

Constraints:

- 64-byte HID report.
- 4-byte header leaves 60 bytes of payload.
- 60 bytes = 20 LEDs at 3 bytes each.
- The local JS plugins loop until all requested LEDs are sent.

#### Apply packet

The shared apply packet observed across Gen2 devices is:

```text
22 A0 <channel_mask> 00 01 00 00 28 00 00 80 00 32 00 00 01
```

This should be treated as a required commit barrier after direct RGB streaming.

#### Fan polling

Poll setup:

```text
60 03
60 02 01 E8 <ticks> 01 E8 <ticks>
```

Plugin comments indicate the device ticks 3 times per second. Poll interval is therefore encoded in 1/3 second units.

Fan set:

```text
62 01 <mask> [duty bytes at 3+fan_index]
```

Async fan status packet:

```text
67 02 ...
```

Offsets:

- Mode base = `16`
- RPM base = `24`
- Duty base = `40`

Fan modes:

- `0` = Not Connected
- `1` = DC
- `2` = PWM

#### Implementation note

This family needs runtime topology discovery and mutable protocol state. Follow the same pattern used by Corsair iCUE LINK-style dynamic topology drivers:

- Discover VID/PID and interface.
- Query firmware.
- Query topology.
- Build channel/segment model from accessory IDs.
- Stream direct frames using 20-LED chunks plus apply packet.

### 4.4 Motherboard analog 12V RGB path

NZXT motherboards reuse the Gen2 digital path for NZXT headers and ARGB headers, but analog 12V RGB headers use a different packet family.

**Digital headers:** same `0x22` stream/apply packets as §4.3  
**Analog headers:** `0x2A 0x04 0x08 0x08 ...`

#### Channel model

| PID      | NZXT headers | ARGB headers | 12V RGB headers |
| -------- | ------------ | ------------ | --------------- |
| `0x2005` | 3            | 0            | 0               |
| `0x200A` | 2            | 1            | 1               |
| `0x200B` | 2            | 1            | 1               |
| `0x200C` | 2            | 1            | 1               |
| `0x2016` | 2            | 1            | 1               |
| `0x2017` | 2            | 1            | 1               |
| `0x201B` | 4            | 2            | 0               |
| `0x201D` | 4            | 2            | 0               |

#### Analog RGB packet

Template:

```text
2A 04 08 08 00 32 00 RR GG BB ... [56]=01 [57]=00 [58]=01 [59]=03
```

Write the color into bytes `7`, `8`, and `9`, using the configured RGB byte order:

- `RGB`
- `RBG`
- `BGR`
- `BRG`
- `GBR`
- `GRB`

#### Implementation note

Model analog 12V headers as separate fixed-color zones or pseudo-devices. They do not support addressable LED streaming.

### 4.5 Legacy Kraken X2/M2

**Transport:** USB HID  
**VID:PID:** `1E71:170E`, `1E71:1715`  
**Report size:** `64` bytes

This family predates Gen2.

#### Status read

A 64-byte status packet is available without an explicit request and parses as:

- Liquid temperature = `byte1 + byte2 * 0.1`
- Fan RPM = bytes `3..4` big-endian
- Pump RPM = bytes `5..6` big-endian
- Firmware = bytes `0x0B`, `0x0C`, `0x0D`, `0x0E`

#### RGB packet

```text
byte 0 = 0x02
byte 1 = 0x4C
byte 2 = channel bits + motion/direction bits
byte 3 = mode
byte 4 = speed | (size << 3) | (cis << 5)
byte 5+ = 9 RGB triples
```

Channel values:

- `0x00` = sync
- `0x01` = logo
- `0x02` = ring

Color ordering:

- Ring path is RGB
- Logo path is transformed to GRB-like ordering in software

Zones:

- Logo = 1 LED
- Ring = 8 LEDs

#### Effect IDs

| Mode          | ID     |
| ------------- | ------ |
| Fixed         | `0x00` |
| Fading        | `0x01` |
| Spectrum      | `0x02` |
| Marquee       | `0x03` |
| Cover Marquee | `0x04` |
| Alternating   | `0x05` |
| Breathing     | `0x06` |
| Pulse         | `0x07` |
| Tai Chi       | `0x08` |
| Water Cooler  | `0x09` |
| Loading       | `0x0A` |
| Wings         | `0x0C` |

#### Scope decision

Direct mode and basic telemetry belong in Phase 2. Legacy hardware effect parity is optional.

### 4.6 Modern Kraken AIO family

Modern Kraken AIOs split into several subfamilies.

#### 4.6.1 Kraken X3 (`0x2007`, `0x2014`)

**RGB ring and external channel:** Gen2 `0x22` family  
**Logo:** separate analog-style write  
**Pump control:** `0x72 0x01 ...`  
**Observed status response:** `0x75 0x02`

Pump ring topology:

- Ring = 8 LEDs
- Logo = 1 LED
- External channel = 40 LEDs max

Logo write packet:

```text
2A 04 04 04 00 32 00 G R B
```

with bytes `56..59 = 01 00 01 03`.

Older alternative packet for `0x2007`:

```text
21 04 04 04 00 32 00 G R B
```

**Important:** `0x2007` and `0x2014` may need PID-specific logo write variants.

Pump set:

```text
72 01 00 00 [40 duty bytes]
```

Minimum pump duty: `25`.

#### 4.6.2 Kraken Z3 (`0x3008`)

**Interface:** `1`  
**RGB:** Gen2-style `0x22` stream/apply for one 40-LED external channel  
**Telemetry request:** `74 01`  
**Telemetry response:** `75 01`  
**Pump set:** `72 01 00 00 [40 bytes]`  
**Fan set:** `72 02 00 00 [40 bytes]`

Telemetry offsets:

- Liquid temp = bytes `15`, `16`
- Pump RPM = bytes `17`, `18` little-endian
- Pump duty = byte `19`
- Fan RPM = bytes `23`, `24` little-endian
- Fan duty = byte `25`

#### 4.6.3 Kraken / Kraken Elite (`0x300E`, `0x300C`)

This spec currently covers:

- Pump/fan telemetry
- Pump/fan control

It does **not** expose:

- RGB ring transport
- Logo transport
- LCD transport

Observed telemetry/control:

- Status request: `74 01`
- Status response: `75 01`
- Pump set: `72 01 00 00 [40 bytes]`
- Fan set: `72 02 00 00 [40 bytes]`

This is enough for thermal control support, not enough for full RGB or LCD support.

#### 4.6.4 Kraken Elite v2 (`0x3012`)

This is the most complete modern NZXT screen AIO variant specified here.

**Interface:** `1`  
**Report size:** `512` bytes  
**Ring LEDs:** `24`  
**External channel:** `40` max  
**Color order:** `GRB`

Ring write:

```text
26 14 01 01 [RGB data]
```

External channel write:

```text
26 14 02 02 [RGB data]
```

Apply:

```text
26 06 01 00 01 00 00 18 00 00 80 00 32 00 00 01
```

Plugin comment: hard-code LED count to `24` or the device rejects the submit.

Telemetry:

- Status request: `74 01`
- Status response: `75 01`
- Same offsets as Z3/Elite family

Control:

- Pump set: `72 01 00 00 [40 bytes]`
- Fan set: `72 02 01 01 [40 bytes]`

#### Implementation note

Treat `0x3012` as a dedicated protocol variant, not just another Gen2 controller. The semantics rhyme with Gen2, but the report size and lighting command family do not.

### 4.7 NZXT Function keyboard family

**Transport:** USB HID  
**Interface:** `1`  
**Usage page:** `0xFFCA`  
**Usage:** `0x0001`  
**Collection:** `0x0000`

The local keyboard plugin models these keyboards as **10 lighting zones**, not per-key devices.

#### Supported PIDs

- Function: `0x2103`, `0x2106`
- Function TKL: `0x2104`, `0x2107`
- Function MiniTKL: `0x2105`, `0x2108`
- Function 2 variants: `0x2130`, `0x2131`, `0x2136`

#### Initialization

Firmware fetch:

```text
43 81 00 01
```

Then read twice. Firmware version comes from bytes `3..5` of the second response.

The plugin uses firmware `>= 1.3.71` to switch to the new lighting protocol.

Software mode:

```text
43 81 00 84
43 81 00 86
43 82 00 41 64
43 97 00 10 01
```

#### Old protocol

Four 65-byte packets encode 10 zones using a dense bit-mapped format. The exact bit map is
not specified here. Defer this path until a packet-level facts-only map or USB capture is
added to this document.

#### New protocol

Header:

```text
43 BD 01 10 02 FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF FF 00 0A
```

Interpretation from plugin comments:

- `0xFF` bytes enable all keys across all banks
- `0x0A` is the number of zones

Then append zone color quads:

```text
R G B 00
```

Commit packet:

```text
43 01
```

#### Scope decision

Phase 3 only. Hypercolor should support the new protocol first, then add the legacy zone packet path if needed.

### 4.8 Lift mouse

**Transport:** USB HID  
**VID:PID:** `1E71:2100`  
**Interface:** `0`

#### DPI and sensor config

Set DPI packet:

```text
43 8B 00 96 <polling> <lod> 04 02 <dpi1/100> <dpi2/100> <dpi3/100> <dpi4/100> 00 32
```

Apply:

```text
43 81 00 9A
```

Header packets:

```text
43 81 00 84
43 81 00 86
```

There is also an unused flash-save variant:

- `43 8C 00 93 ...`
- `43 81 00 9A`

#### RGB packet

Base:

```text
43 97 00 10 01 3F ...
```

Color is written at offsets:

- `0x17` = R
- `0x18` = G
- `0x19` = B

#### Scope decision

Phase 4. This is straightforward but not core to the first NZXT milestone.

### 4.9 Vertagear RGB LED Upgrade Kits

**PID:** `0x2004`

This partner kit uses the Gen2-style controller protocol:

- Stream: `0x22 0x10|n`
- Apply: `0x22 0xA0`

One observed quirk: the apply packet uses `0x0A` in the count-like field where most Gen2 devices use `0x28`.

This suggests a reusable Gen2 core with device-specific apply descriptors.

---

## 5. Topology Catalog

### 5.1 Common LED counts

Known LED-count catalog:

| Accessory           | LEDs |
| ------------------- | ---- |
| NZXT strip 6        | 6    |
| NZXT strip 8        | 8    |
| NZXT strip 10       | 10   |
| Aer fan / Aer 2 fan | 8    |
| F120/F140 RGB       | 18   |
| F120/F140 RGB Duo   | 20   |
| F120/F140 RGB Core  | 8    |
| F360 RGB Core       | 24   |
| F420 RGB            | 24   |
| Kraken X3 ring      | 8    |
| Kraken X3 logo      | 1    |
| Kraken Elite ring   | 24   |

### 5.2 Layout guidance

Use Hypercolor topology descriptors for LED geometry:

- strips: linear positions
- Aer fans: simple ring/square layouts
- F-series fans: circular ring coordinates
- Duo fans: denser 20-LED ring
- Kraken Elite v2 ring: 24-point circular path

### 5.3 Color-order rule

Default rule:

- Assume **GRB**

Exceptions:

- Legacy Kraken X2/M2 ring uses RGB
- Legacy Kraken X2/M2 logo uses software-transformed order
- Motherboard analog 12V headers use configurable order

---

## 6. HAL Integration

### 6.1 Module layout

Phase 1 lighting landed with this layout:

```text
hypercolor_hal::drivers::nzxt
  mod.rs              family docs, vendor ID, re-exports
  packet.rs           zerocopy wire structs for every Phase 1 packet
  topology.rs         accessory catalog (§3.6) and 0x21 0x03 response parsing
  gen2.rs             NzxtGen2Protocol: profiles, runtime layout, stream/apply encoding
  motherboard.rs      header table type, analog 12V packet, analog byte orders
  kraken_modern.rs    Kraken X3 and Z3 profiles, X3 logo packet
  kraken_elite_v2.rs  NzxtKrakenEliteV2Protocol (512-byte reports)
  devices.rs          descriptors for every Phase 1 PID
```

The families held back in §9.4 (Hue Plus, Smart Device V1) and the later phases (legacy
Kraken, Function keyboard, Lift mouse, screens) get their own modules when their gaps close.
Nothing in the shipped modules assumes they exist.

### 6.2 Driver split

Two protocol types cover Phase 1:

| Type                         | Devices                                                       |
| ---------------------------- | ------------------------------------------------------------- |
| `NzxtGen2Protocol`           | Gen2 controllers, NZXT motherboards, Kraken X3, Kraken Z3     |
| `NzxtKrakenEliteV2Protocol`  | Kraken Elite v2 (`0x3012`)                                    |

The Kraken X3 and Z3 lighting paths are byte-for-byte the Gen2 `0x22` stream and apply
packets (§4.6.1, §4.6.2), so they run on `NzxtGen2Protocol` with a Kraken profile instead of a
second protocol type. The profile (`Gen2Profile`) carries everything that differs:

| Profile                  | Discovery                 | Digital channel cap | Per-channel cap        | Fixed-color zones   |
| ------------------------ | ------------------------- | ------------------- | ---------------------- | ------------------- |
| `controller()`           | topology query            | 8                   | none (wire limit only) | none                |
| `motherboard(headers)`   | topology query            | NZXT + ARGB headers | none (wire limit only) | 12V RGB headers     |
| `kraken_x3(logo)`        | topology query            | 8                   | 40 on external channels | logo on `0x2014`   |
| `kraken_z3()`            | fixed: channel 0, 40 LEDs | 1                   | 40                     | none                |

`kraken_modern.rs` owns the Kraken profiles and the X3 logo packet; `motherboard.rs` owns the
header table type and the analog packet. `NzxtKrakenEliteV2Protocol` stays separate because
its report size and command family differ (§4.6.4).

The names `NzxtHuePlusProtocol`, `NzxtSmartDeviceV1Protocol`, `NzxtKrakenLegacyProtocol`,
`NzxtFunctionKeyboardProtocol`, and `NzxtLiftMouseProtocol` stay reserved for their phases.

### 6.3 Transport

Every Phase 1 device uses a host-managed HID transport: `/dev/hidraw*` on Linux, hidapi on
Windows and macOS. Host-managed access keeps the kernel HID driver bound. Linux ships hwmon
drivers for several of these PIDs (`nzxt-smart2` for the RGB and Fan Controller and Smart
Device V2, `nzxt-kraken3` for the Kraken X3, Z3, and 2023/2024 families), and their
documentation states that the smart2 driver coexists with userspace hidraw tools. Claiming
the interface directly would detach those drivers and take fan and temperature telemetry
away from the host.

Byte 0 of every NZXT packet in this document is the first byte on the wire, and it changes
per command (`0x20`, `0x22`, `0x26`, `0x2A`). The protocol therefore owns that byte and the
transport runs in `OutputReportWithReportId` mode, which sends the payload verbatim. The
descriptor's `report_id` is unused in that mode and is set to `0x00`.

| Family          | Interface | Report length | Reads                       |
| --------------- | --------- | ------------- | --------------------------- |
| Gen2 controller | 0         | 64            | topology response only      |
| Motherboard     | 0         | 64            | topology response only      |
| Kraken X3       | 0         | 64            | topology response only      |
| Kraken Z3       | 1         | 64            | none                        |
| Kraken Elite v2 | 1         | 512           | none                        |

### 6.4 Discovery keys

Dispatch by:

- VID/PID
- interface number
- usage page where relevant
- report size where relevant (`64` vs `512`)

Phase 1 needs only VID/PID plus interface. The full routing plan, including later phases:

- `04D8:00DF` -> serial Hue Plus (held back, §9.4)
- `1E71:1714@if0` -> Smart Device V1 (held back, §9.4)
- `1E71:2001/2002/...@if0` -> Gen2 controller family
- `1E71:170E/1715@if0` -> legacy Kraken
- `1E71:3008/300C/300E/3012@if1` -> modern Kraken family
- `1E71:210x/213x@if1` -> Function keyboard family
- `1E71:2100@if0` -> Lift mouse

### 6.5 Descriptor table

| PID      | Protocol ID                          | Profile                  |
| -------- | ------------------------------------ | ------------------------ |
| `0x2001` | `nzxt/hue-2`                         | controller               |
| `0x2002` | `nzxt/hue-2-ambient`                 | controller               |
| `0x2005` | `nzxt/n7-z390`                       | motherboard 3 / 0 / 0    |
| `0x2006` | `nzxt/smart-device-v2`               | controller               |
| `0x2007` | `nzxt/kraken-x3`                     | Kraken X3, no logo       |
| `0x2009` | `nzxt/rgb-fan-controller-2009`       | controller               |
| `0x200A` | `nzxt/n7-z490`                       | motherboard 2 / 1 / 1    |
| `0x200B` | `nzxt/n7-b550`                       | motherboard 2 / 1 / 1    |
| `0x200C` | `nzxt/n7-z590`                       | motherboard 2 / 1 / 1    |
| `0x200D` | `nzxt/smart-device-v2-case-200d`     | controller               |
| `0x200E` | `nzxt/rgb-fan-controller-200e`       | controller               |
| `0x200F` | `nzxt/smart-device-v2-case-200f`     | controller               |
| `0x2010` | `nzxt/rgb-fan-controller-2010`       | controller               |
| `0x2011` | `nzxt/rgb-fan-controller-2011`       | controller               |
| `0x2012` | `nzxt/rgb-controller-2012`           | controller               |
| `0x2014` | `nzxt/kraken-x3-rgb`                 | Kraken X3, logo packet   |
| `0x2016` | `nzxt/n7-z690`                       | motherboard 2 / 1 / 1    |
| `0x2017` | `nzxt/n5-z690`                       | motherboard 2 / 1 / 1    |
| `0x2019` | `nzxt/rgb-fan-controller-2019`       | controller               |
| `0x201B` | `nzxt/n7-b650e`                      | motherboard 4 / 2 / 0    |
| `0x201D` | `nzxt/n7-z790`                       | motherboard 4 / 2 / 0    |
| `0x201F` | `nzxt/rgb-fan-controller-201f`       | controller               |
| `0x2020` | `nzxt/rgb-fan-controller-2020`       | controller               |
| `0x2021` | `nzxt/rgb-controller-2021`           | controller               |
| `0x2022` | `nzxt/control-hub`                   | controller               |
| `0x3008` | `nzxt/kraken-z3`                     | Kraken Z3                |
| `0x3012` | `nzxt/kraken-elite-v2`               | Elite v2 (own protocol)  |

Motherboard columns are NZXT / ARGB / 12V RGB header counts from §4.4. Every descriptor is
predicate-free; no PID shares a protocol with another family.

### 6.6 Runtime state

Profiles that discover their topology hold the layout behind an `RwLock`. The layout starts
with only the profile's static zones (12V headers, the X3 logo) and is rebuilt when a
`0x21 0x03` response arrives. Phase 1 keeps no firmware version (§9.4) and no fan or pump
state (out of scope).

The device may interleave unrelated input reports with the topology reply: the Gen2 fan
status packet `0x67 0x02` once fan polling is enabled (the Linux hwmon driver enables it),
or the Kraken status packet `0x75 0x02`. Any report other than `0x21 0x03` parses as
`Unsolicited`: the USB backend reads the next report in its place without resending the
query, so the reply already queued behind the status packets is the one consumed. The
backend tolerates 16 unsolicited reports per command, then fails it. The topology reply is
required: a read timeout (an empty read on hidapi, a timeout error on hidraw) fails init
instead of leaving the device with no channels, as does a `0x21 0x03` reply too short to
hold the channel count at byte 14, which means the transport and the protocol disagree on
the report size. A failed init surfaces as a connect error, and the device lifecycle
retries the connect.

### 6.7 Registration

The `nzxt` driver module is enabled by default and claims its devices like any other HAL
family. It carries the experimental label (Spec 89): nobody on the team owns NZXT
hardware, so every descriptor is unverified. The label informs users; it gates nothing.
The device database entries carry the same unverified marker.

### 6.8 Capability model

Model modern NZXT devices as capability sets rather than monoliths:

- `lighting_external_channel`
- `lighting_pump_ring`
- `lighting_logo`
- `fan_control`
- `pump_control`
- `temperature_probe`
- `lcd_screen`

This avoids pretending `0x300C`, `0x300E`, and `0x3012` all expose the same surface.

---

## 7. Render and Control Strategy

### 7.1 Phase 1 direct-lighting path

Implement direct RGB only for:

- Hue Plus
- Smart Device V1
- Gen2 controllers
- Kraken X3 external channel and pump ring
- Kraken Z3 external channel
- Kraken Elite v2 ring and external channel
- motherboard digital headers

Status as of Rev 3:

| Target                                  | State                                                  |
| --------------------------------------- | ------------------------------------------------------ |
| Hue Plus                                | held back, §9.4                                        |
| Smart Device V1                         | held back, §9.4                                        |
| Gen2 controllers                        | implemented                                            |
| Kraken X3 external channel and ring     | implemented on both PIDs                               |
| Kraken X3 logo                          | implemented on `0x2014`; `0x2007` held back, §9.4      |
| Kraken Z3 external channel              | implemented                                            |
| Kraken Elite v2 ring                    | implemented                                            |
| Kraken Elite v2 external channel        | held back, §9.4                                        |
| Motherboard NZXT digital headers        | implemented                                            |
| Motherboard ARGB headers                | held back, §9.4                                        |
| Motherboard 12V analog headers          | implemented, default byte order RGB                    |

Implemented here means encoded and tested against this document. None of it has run on
NZXT hardware yet.

### 7.2 Phase 1 thermal and fan path

Implement telemetry and control for:

- Gen2 fan controllers
- Kraken X3 pump control
- Kraken Z3 pump and fan control
- Kraken/Elite `0x300C` / `0x300E`
- Kraken Elite v2

### 7.3 Deferred hardware effects

Defer these until direct mode is stable:

- Hue 1 effects
- Hue 2 hardware effects
- legacy Kraken animation modes
- keyboard legacy zone protocol

### 7.4 LCD architecture

Do not bury LCD inside the lighting protocol. Add a separate future-facing screen capability:

```text
DeviceTransport + LightingProtocol + optional ScreenProtocol
```

This is important because NZXT's modern AIOs combine:

- addressable RGB
- fan/pump control
- thermal telemetry
- LCD rendering

The screen path should remain optional until the protocol is known.

---

## 8. Testing Strategy

### 8.1 Packet tests

Add golden packet tests for:

- Hue Plus frame encoding
- Smart Device V1 dual-packet split
- Gen2 20-LED chunking and apply packet
- motherboard analog RGB packet encoding
- Kraken X3 logo variant encoding
- Kraken Elite v2 `0x26` packets and 512-byte report sizing
- Function keyboard new protocol packet assembly
- Lift mouse DPI packet encoding

Phase 1 coverage lives in `crates/hypercolor-hal/tests/nzxt_protocol_tests.rs`. The Gen2
chunking tests pin the packet count at 1, 20, 21, 40, 41, and 144 LEDs, the color that lands
at byte 4 of the second packet, and the zero tail of a short final packet. Every expected
byte array in that file is written out from the tables in §4 and §9.5, not computed by the
code under test.

### 8.2 Topology tests

Add tests for:

- Gen2 accessory ID to segment mapping
- LED limit clamping
- external channel + pump ring composite devices
- motherboard header count per PID

Descriptor coverage (every Phase 1 PID, its protocol ID, interface, report length, and
profile) lives in `crates/hypercolor-hal/tests/nzxt_devices_tests.rs`.

### 8.3 Integration strategy

Priority integration targets:

1. A Gen2 RGB and Fan Controller.
2. A Kraken X3 or Z3.
3. A Kraken Elite v2 (`0x3012`).
4. An NZXT motherboard with digital headers.

### 8.4 LCD capture work

Before claiming full Kraken screen support, capture real USB traffic while:

1. Switching LCD face.
2. Uploading a static image.
3. Uploading animated content.
4. Rotating the display.
5. Returning control to hardware defaults.

Without this, full screen support is not verified.

---

## 9. Research Gaps and Phase Plan

### 9.1 The biggest gap: LCD transport

What we know:

- NZXT screen AIOs exist in the device registry.
- No LCD packet map is specified here.

What we do not know yet:

- LCD endpoint selection
- report size and framing
- image format
- chunking strategy
- init/enable sequence
- whether the device uses JPEG, raw RGB565, PNG, or compressed blocks
- whether the screen path shares the same HID interface as telemetry/fan control

### 9.2 Secondary gap: Kraken `0x300C` / `0x300E` lighting

Thermal control is specified for these PIDs. Full RGB support remains unresolved until a packet map is captured and added here.

### 9.3 Phase plan

| Phase       | Scope                                                                                                                                                  |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Phase 1** | Hue Plus, Smart Device V1 direct RGB, Gen2 controllers, motherboard digital/analog headers, Kraken X3/Z3/Elite v2 lighting, pump/fan telemetry/control |
| **Phase 2** | Legacy Kraken X2/M2 direct RGB and telemetry, Gen2 effect parity, Vertagear variant support                                                            |
| **Phase 3** | Function keyboard new protocol, legacy keyboard protocol, Lift mouse                                                                                   |
| **Phase 4** | LCD capture, LCD clean-room spec, screen protocol implementation for Kraken Z/Elite families                                                           |

Rev 3 status: Phase 1 lighting is implemented for the pieces this document fully specifies
(§7.1 status table). Phase 1 pump, fan, and telemetry work waits on an engine cooling
surface; the HAL has no consumer for it yet.

### 9.4 Phase 1 lighting: held-back pieces

Each row below is an in-scope Phase 1 lighting piece this document does not specify well
enough to encode. They stay unimplemented until a USB capture or a new facts-only note
supplies the missing facts. Do not fill these from memory of other implementations.

| Piece                              | Missing facts                                                                                                                                                                                                                                                                          |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Hue Plus (§4.1)                    | Channel count. Whether the probe's `<channel>` byte is 0- or 1-based when the lighting packet uses `channel + 1`. The byte 2 mode value for direct streaming, and the byte 3 and byte 4 values that go with it (§4.1 has no mode table). Whether the controller acknowledges lighting packets, and the minimum spacing between them. |
| Smart Device V1 (§4.2)             | The byte 2 mode value for direct streaming and the byte 3 and byte 4 values that go with it (the effect table lists `Fixed = 0x00` but never ties it to the direct stream). Whether packet 2 is still sent when 20 or fewer LEDs are present. The report ID of the 21-byte init response: identifying it by length alone fails where hidapi pads input reports to the longest report (Windows). The LED cap. |
| Motherboard ARGB headers (§4.4)    | Which `1 << channel` index each ARGB header occupies, and where its LED count comes from (the topology response identifies NZXT accessories, not generic strips).                                                                                                                       |
| Kraken X3 `0x2007` logo (§4.6.1)   | Which of the two logo packets (`0x2A` or `0x21`) this PID takes, and whether the `0x21` variant carries the bytes 56 to 59 trailer.                                                                                                                                                    |
| Kraken Elite v2 external (§4.6.4)  | A commit packet for `26 14 02 02`. The documented apply carries `0x01` at byte 2 and `0x18` (24) at byte 7, both ring values, so it cannot be assumed to commit the external channel.                                                                                                  |
| Gen2 firmware version (§4.3)       | Byte offsets of `fw_major`, `fw_minor`, `fw_patch` in the `11 01` response. Lighting does not depend on them, so Phase 1 never sends `10 01`.                                                                                                                                          |
| Frame pacing (all families)        | No family documents a maximum frame rate or a minimum gap between reports. Phase 1 targets 30 FPS with no inter-report delay; revisit with hardware.                                                                                                                                   |
| HID report numbering (all families) | Whether these devices declare numbered HID reports. Phase 1 sends byte 0 (`0x20`, `0x22`, `0x26`, `0x2A`) as the first byte of the report in report-ID-in-payload mode. On Linux hidraw that byte reaches the wire either way (usbhid strips only a leading `0x00`). The Windows and macOS HID APIs treat byte 0 as the report ID: if the device uses unnumbered reports they need a `0x00` prefix (and on Windows a buffer of the declared output report length, 65 bytes), and the packets as written would be rejected or misframed. Settled by the HID report descriptor: on Linux `/sys/class/hidraw/hidrawN/device/report_descriptor` or `usbhid-dump`, on Windows `HidP_GetCaps` (`OutputReportByteLength`) plus the descriptor's Report ID items (tag `0x85`). Until then, treat Windows and macOS as the less certain platforms. |

### 9.5 Phase 1 encoding readings

Where this document is silent but only one reading fits the documented facts, the
implementation takes that reading. Each one is listed so a hardware tester knows what to
check first.

1. Every report is zero padded to its documented length: 64 bytes for Gen2-family packets,
   512 bytes for Kraken Elite v2.
2. Byte 0 of every packet is the first byte on the wire and is sent verbatim (§6.3).
3. Topology slots: `0x00` marks an empty slot and ends that channel's chain. An ID missing
   from §3.6 also ends the chain, because the LED offsets of anything after an unknown
   accessory cannot be known. Accessories before it are kept.
4. The channel count at byte `14` is clamped to 8: the channel mask is one byte, and a
   64-byte response has room for exactly eight 6-byte records after offset `0x0F`. A channel
   whose record would run past the received report is dropped.
5. The apply packet's `<channel_mask>` is the same `1 << channel` as the stream packets. Each
   channel gets its stream packets followed immediately by its own apply. Channels with no
   known accessories get neither.
6. Six accessories of at most 24 LEDs (144 LEDs, 8 packets) always fit the 4-bit packet
   number, so the topology path never needs a runtime chunk cap.
7. Kraken Z3 has one external channel, so its mask is `1 << 0 = 0x01`. With no topology query
   documented for the Z3, the channel is always streamed as 40 LEDs, the count §4.6.2 states.
8. Kraken X3: the channel carrying accessory `0x10` is the pump ring. The channel carrying
   `0x11` is the logo and is never streamed, because §4.6.1 routes the logo through the
   separate `0x2A` write. Every streamed channel is capped at 40 LEDs (the external channel
   maximum) at an accessory boundary: accessories that would cross 40 are dropped whole. The
   pump ring's 8 LEDs never reach the cap. On `0x2014` the logo is a static 1-LED zone.
9. Motherboards: like every Gen2 profile, a channel is streamed only when the topology
   reply names known NZXT accessories on it, so the code assumes nothing about which index
   a given header occupies. The NZXT plus ARGB header count from §4.4 is only an upper
   bound on channel indexes. ARGB headers stay dark in practice: a generic strip reports
   no accessory ID, and §9.4 records that their channel index and LED count source are
   unknown. The 12V header is a static fixed-color zone. Its byte order defaults to RGB,
   the order the §4.4 template is written in; the profile accepts any of the six orders,
   but no user-facing setting selects one yet.
10. Zone order is digital channels in channel order (accessories in slot order inside each
    channel), then fixed-color zones (12V header or X3 logo).
11. Layout hints follow §5.2 (strips linear, fans and pump rings circular), with one
    exception: the F360 RGB Core case version (`0x1D`) and F420 RGB (`0x1F`) are triple-fan
    units whose LEDs span three separate fans, so they are laid out as strips rather than
    as one 24-LED ring.

Wire tables for every packet Phase 1 sends:

**Gen2 topology query (64 bytes)**

| Offset | Size | Field       | Value  |
| ------ | ---- | ----------- | ------ |
| 0      | 1    | Command     | `0x20` |
| 1      | 1    | Subcommand  | `0x03` |
| 2-63   | 62   | Padding     | `0x00` |

**Gen2 topology response (read)**

| Offset          | Size | Field         | Notes                                   |
| --------------- | ---- | ------------- | --------------------------------------- |
| 0               | 1    | Command       | `0x21`                                  |
| 1               | 1    | Subcommand    | `0x03`                                  |
| 14              | 1    | Channel count | clamped to 8                            |
| 15 + 6c + s     | 1    | Accessory ID  | channel `c`, slot `s` (0-5), §3.6 table |

**Gen2 direct stream (64 bytes)**

| Offset | Size | Field          | Value                                  |
| ------ | ---- | -------------- | -------------------------------------- |
| 0      | 1    | Command        | `0x22`                                 |
| 1      | 1    | Packet number  | `0x10 \| n`, `n` = 0 for LEDs 0-19, 1 for LEDs 20-39, and so on |
| 2      | 1    | Channel mask   | `1 << channel`                         |
| 3      | 1    | Reserved       | `0x00`                                 |
| 4-63   | 60   | Colors         | up to 20 LEDs as G, R, B; zero padded  |

**Gen2 apply (64 bytes)**

| Offset | Size | Value                                                 |
| ------ | ---- | ----------------------------------------------------- |
| 0-1    | 2    | `22 A0`                                               |
| 2      | 1    | channel mask (`1 << channel`)                         |
| 3-15   | 13   | `00 01 00 00 28 00 00 80 00 32 00 00 01`              |
| 16-63  | 48   | `0x00`                                                |

**Fixed-color write: motherboard 12V header and Kraken X3 logo (64 bytes)**

| Offset | Size | 12V header                        | X3 logo (`0x2014`) |
| ------ | ---- | --------------------------------- | ------------------ |
| 0-6    | 7    | `2A 04 08 08 00 32 00`            | `2A 04 04 04 00 32 00` |
| 7-9    | 3    | color in the configured order     | G, R, B            |
| 10-55  | 46   | `0x00`                            | `0x00`             |
| 56-59  | 4    | `01 00 01 03`                     | `01 00 01 03`      |
| 60-63  | 4    | `0x00`                            | `0x00`             |

**Kraken Elite v2 ring write (512 bytes)**

| Offset | Size | Value                             |
| ------ | ---- | --------------------------------- |
| 0-3    | 4    | `26 14 01 01`                     |
| 4-75   | 72   | 24 LEDs as G, R, B                |
| 76-511 | 436  | `0x00`                            |

**Kraken Elite v2 apply (512 bytes)**

| Offset | Size | Value                                                 |
| ------ | ---- | ----------------------------------------------------- |
| 0-15   | 16   | `26 06 01 00 01 00 00 18 00 00 80 00 32 00 00 01`     |
| 16-511 | 496  | `0x00`                                                |

Per-frame command sequence: for each streamed channel, its stream packets then its apply;
then one fixed-color write per fixed zone. The Elite v2 frame is the ring write then the
apply. No command in a frame expects a response.

---

## 10. Recommendation

Build the NZXT driver around a **plugin-first protocol split**:

1. `gen2` as the primary shared HID core.
2. `kraken_modern` layered on top for pump/fan telemetry and PID-specific RGB variants.
3. `kraken_elite_v2` as its own 512-byte transport specialization.
4. `screen` as a separate future protocol gated on real USB captures.

This is the cleanest path because it matches the actual wire-level families, delivers broad NZXT RGB and fan coverage quickly, and avoids faking LCD support from incomplete evidence.

---

## References

- Spec 16, Hardware Abstraction Layer: the `Protocol` trait, transports, and descriptor database this driver plugs into.
- Spec 62, Zerocopy Protocol Structs: the packet-struct pattern every Phase 1 packet follows.
- Linux kernel hwmon documentation, `nzxt-smart2` (RGB and Fan Controller, Smart Device V2), including its note on coexisting with hidraw userspace tools: https://docs.kernel.org/hwmon/nzxt-smart2.html
- Linux kernel hwmon documentation, `nzxt-kraken3` (Kraken X3, Z3, 2023, 2023 Elite, 2024 Elite), which leaves RGB and LCD control to userspace: https://docs.kernel.org/hwmon/nzxt-kraken3.html
- Rev 2 of this document is the clean-room closure of the earlier NZXT research notes; Rev 3 adds implementation readings (§9.5) and the gap register (§9.4) without new protocol facts.
