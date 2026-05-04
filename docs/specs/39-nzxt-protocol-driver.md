# 39 -- NZXT Protocol Driver

> Native USB HID and USB serial driver for the NZXT ecosystem. This document is the authoritative packet specification; any gap called out here requires fresh USB capture or a new facts-only spec before implementation.

**Status:** Draft (Rev 2 -- clean-room closure)
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::nzxt`
**Author:** Nova
**Date:** 2026-04-10

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

| Purpose                | Request     | Response                 |
| ---------------------- | ----------- | ------------------------ | ------------- |
| Firmware query         | `10 01`     | `11 01`                  |
| LED topology query     | `20 03`     | `21 03`                  |
| Direct RGB stream      | `22 10      | n ...`                   | none required |
| Apply stream           | `22 A0 ...` | none required            |
| Fan poll enable        | `60 03`     | none documented          |
| Fan poll interval      | `60 02 ...` | none documented          |
| Fan set                | `62 01 ...` | async fan packet follows |
| Fan state async packet | none        | `67 02`                  |

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

Recommended structure:

```text
hypercolor_hal::drivers::nzxt
  mod.rs
  detect.rs
  hue_plus.rs
  smart_device_v1.rs
  gen2.rs
  motherboard.rs
  kraken_legacy.rs
  kraken_modern.rs
  keyboard.rs
  mouse.rs
  topology.rs
  packet.rs
```

### 6.2 Driver split

Use one backend family with protocol dispatch:

- `NzxtHuePlusProtocol`
- `NzxtSmartDeviceV1Protocol`
- `NzxtGen2Protocol`
- `NzxtKrakenLegacyProtocol`
- `NzxtKrakenModernProtocol`
- `NzxtKrakenEliteV2Protocol`
- `NzxtFunctionKeyboardProtocol`
- `NzxtLiftMouseProtocol`

### 6.3 Discovery keys

Dispatch by:

- VID/PID
- interface number
- usage page where relevant
- report size where relevant (`64` vs `512`)

Recommended examples:

- `04D8:00DF` -> serial Hue Plus
- `1E71:1714@if0` -> Smart Device V1
- `1E71:2001/2002/...@if0` -> Gen2 controller family
- `1E71:170E/1715@if0` -> legacy Kraken
- `1E71:3008/300C/300E/3012@if1` -> modern Kraken family
- `1E71:210x/213x@if1` -> Function keyboard family
- `1E71:2100@if0` -> Lift mouse

### 6.4 Runtime state

Gen2 and modern Kraken families need interior mutable state for:

- firmware version
- discovered channel count
- per-channel segment/accessory mapping
- connected fan mode and rpm cache
- PID-specific packet descriptors

### 6.5 Capability model

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

### 8.2 Topology tests

Add tests for:

- Gen2 accessory ID to segment mapping
- LED limit clamping
- external channel + pump ring composite devices
- motherboard header count per PID

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

---

## 10. Recommendation

Build the NZXT driver around a **plugin-first protocol split**:

1. `gen2` as the primary shared HID core.
2. `kraken_modern` layered on top for pump/fan telemetry and PID-specific RGB variants.
3. `kraken_elite_v2` as its own 512-byte transport specialization.
4. `screen` as a separate future protocol gated on real USB captures.

This is the cleanest path because it matches the actual wire-level families, delivers broad NZXT RGB and fan coverage quickly, and avoids faking LCD support from incomplete evidence.
