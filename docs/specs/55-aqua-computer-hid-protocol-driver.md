# 55 -- Aqua Computer HID Protocol Driver

> Clean-room implementation specification for Aqua Computer / Aquacomputer HID status reports and fixed fan control. This document is sufficient to implement the scoped driver from the spec alone.

**Status:** Draft (clean-room closure)
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::aqua_computer`
**Author:** Nova
**Date:** 2026-05-04

---

## Table of Contents

1. [Overview](#1-overview)
2. [Clean-Room Boundary](#2-clean-room-boundary)
3. [Device Registry](#3-device-registry)
4. [HID Transport](#4-hid-transport)
5. [Shared Encodings](#5-shared-encodings)
6. [Status Report Maps](#6-status-report-maps)
7. [Feature Report Writes](#7-feature-report-writes)
8. [Channel Semantics and hwmon Coexistence](#8-channel-semantics-and-hwmon-coexistence)
9. [HAL Integration and Tests](#9-hal-integration-and-tests)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

This spec covers Aqua Computer HID devices that expose periodic sensor status reports and,
for fan/pump controllers, mutable configuration feature reports.

Scoped capabilities:

- firmware version and serial number reads
- physical and virtual temperature sensor reads
- fan/pump RPM, voltage, current, and power reads
- D5 Next voltage rail reads
- Quadro flow sensor reads
- fixed duty fan/pump control for D5 Next, Octo, and Quadro
- optional hwmon-backed reads/writes when the kernel driver exposes matching attributes

Out of scope:

- RGB lighting control
- fan curve authoring
- PID-control authoring
- screen/display control
- Farbwerk Nano and High Flow Next packet maps

The RGB controllers in this family are useful telemetry targets from this spec, but their
lighting command protocol is not documented here.

---

## 2. Clean-Room Boundary

This document is the allowed implementation source. It records packet IDs, report lengths,
offsets, scales, checksum behavior, and write semantics in Hypercolor-owned wording.

Implementation rules:

- Do not consult, port, translate, or compare against third-party implementations while
  writing the Hypercolor driver.
- Do not import previous fixtures or example reports. Build new synthetic fixtures from the
  offsets and scales below.
- If a field or device is missing here, use fresh hardware captures or a new facts-only
  research addendum before implementing it.
- Preserve unknown control-report bytes exactly. These feature reports are device settings,
  not command packets.

---

## 3. Device Registry

All decoded devices use vendor ID `0x0C70`.

| Product       | PID      | Status report | Feature report | Fixed speed control | Notes                         |
| ------------- | -------- | ------------- | -------------- | ------------------- | ----------------------------- |
| D5 Next       | `0xF00E` | `0x9E`        | `0x329`        | pump + fan          | Pump, optional fan, rails     |
| Farbwerk      | `0xF00A` | `0xB6`        | none           | no                  | Four physical temp sensors    |
| Farbwerk 360  | `0xF010` | `0xB6`        | none           | no                  | Four physical + 16 virtual temps |
| Octo          | `0xF011` | `0x147`       | `0x65F`        | fan1..fan8          | Eight fan channels            |
| Quadro        | `0xF00D` | `0xDC`        | `0x3C1`        | fan1..fan4          | Four fan channels + flow      |

Known inventory without packet maps in this spec:

| Product        | PID      | Required next step              |
| -------------- | -------- | ------------------------------- |
| Farbwerk Nano  | `0xF00F` | capture/status-map addendum     |
| High Flow Next | `0xF012` | capture/status-map addendum     |

Do not register unsupported PIDs as functional devices unless the descriptor exposes only
an explicit "capture required" diagnostic mode.

---

## 4. HID Transport

### 4.1 Status report

Status report properties:

- report ID: `0x01`
- transport: HID interrupt/input report
- cadence: approximately one report per second
- initialization: no wake or init command is required
- read length: device-specific length from section 3

The returned buffer includes report ID `0x01` at byte `0`. All offsets in this spec are
absolute offsets in that returned status-report buffer.

Polling guidance:

1. For direct reads, clear queued reports before waiting for a fresh status report.
2. Read the exact status-report length for the detected device.
3. If the report ID is not `0x01`, discard and read again until timeout.
4. Parse only offsets listed in this document.

### 4.2 Feature report

Feature report properties:

- report ID: `0x03`
- transport: HID `GET_FEATURE_REPORT` / `SET_FEATURE_REPORT`
- length: device-specific feature-report length from section 3
- checksum: CRC-16/USB stored in the last two bytes, big-endian

Feature reports are mutable device configuration snapshots. To change one field:

1. Fetch the full feature report with report ID `0x03`.
2. Modify only the documented bytes for the selected channel.
3. Recompute the checksum over the documented checksum window.
4. Write the complete feature report back.

Never construct a small partial feature report. Never zero unknown settings.

### 4.3 Checksum

The feature report checksum is CRC-16/USB.

CRC parameters:

| Parameter | Value                  |
| --------- | ---------------------- |
| width     | 16                     |
| poly      | `0x8005`               |
| init      | `0xFFFF`               |
| refin     | true                   |
| refout    | true                   |
| xorout    | `0xFFFF`               |
| check     | `0xB4C8` for `123456789` |

Checksum window for a feature report of length `L`:

```text
crc_input = report[1 .. L-2]
```

That is, exclude byte `0` (`0x03` report ID) and exclude the final two checksum bytes.
Store the resulting 16-bit checksum big-endian at offsets `L-2` and `L-1`.

---

## 5. Shared Encodings

Unless a table says otherwise, multi-byte values are unsigned big-endian.

### 5.1 Temperature sensors

Temperature sensor value:

```text
deg_c = raw_u16_be * 0.01
```

Disconnected physical or virtual sensors report raw value `0x7FFF`. Skip disconnected
sensors instead of surfacing `327.67 C`.

### 5.2 Fan/pump info substructure

Fan info substructures are used for both pumps and ordinary fan headers.

| Relative offset | Size | Scale / meaning                         |
| --------------- | ---- | --------------------------------------- |
| `0x00`          | 2    | controller percent/target, raw u16      |
| `0x02`          | 2    | voltage: raw `* 0.01` volts             |
| `0x04`          | 2    | current: raw `* 0.001` amps             |
| `0x06`          | 2    | power: raw `* 0.01` watts               |
| `0x08`          | 2    | speed: raw RPM                          |

Use the device-specific base offsets in section 6. RPM lives at `base + 0x08`.

### 5.3 Device statics

For all decoded status reports:

| Offset | Size | Encoding             |
| ------ | ---- | -------------------- |
| `0x03` | 2    | serial number part 1 |
| `0x05` | 2    | serial number part 2 |
| `0x0D` | 2    | firmware version     |

Serial number formatting:

```text
format!("{part1:05}-{part2:05}")
```

Firmware version is exposed as the raw big-endian integer unless a future device-specific
mapping is documented.

---

## 6. Status Report Maps

### 6.1 D5 Next, PID `0xF00E`

Status report length: `0x9E`
Feature report length: `0x329`

Temperature sensors:

| Sensor              | Offset |
| ------------------- | ------ |
| Liquid temperature  | `0x57` |
| Virtual temp 1      | `0x3F` |
| Virtual temp 2      | `0x41` |
| Virtual temp 3      | `0x43` |
| Virtual temp 4      | `0x45` |
| Virtual temp 5      | `0x47` |
| Virtual temp 6      | `0x49` |
| Virtual temp 7      | `0x4B` |
| Virtual temp 8      | `0x4D` |

Fan/pump info substructures:

| Channel | Base offset | RPM offset |
| ------- | ----------- | ---------- |
| pump    | `0x6C`      | `0x74`     |
| fan     | `0x5F`      | `0x67`     |

Voltage rails:

| Rail | Offset | Scale                 |
| ---- | ------ | --------------------- |
| +12V | `0x37` | raw `* 0.01` volts    |
| +5V  | `0x39` | raw `* 0.01` volts    |

Feature-report fixed-speed control offsets:

| Channel | Offset |
| ------- | ------ |
| fan     | `0x41` |
| pump    | `0x96` |

### 6.2 Farbwerk, PID `0xF00A`

Status report length: `0xB6`
Feature report: not specified

Temperature sensors:

| Sensor   | Offset |
| -------- | ------ |
| Sensor 1 | `0x2F` |
| Sensor 2 | `0x31` |
| Sensor 3 | `0x33` |
| Sensor 4 | `0x35` |

No fan, pump, fixed-speed, or RGB control is specified here.

### 6.3 Farbwerk 360, PID `0xF010`

Status report length: `0xB6`
Feature report: not specified

Temperature sensors:

| Sensor   | Offset |
| -------- | ------ |
| Sensor 1 | `0x32` |
| Sensor 2 | `0x34` |
| Sensor 3 | `0x36` |
| Sensor 4 | `0x38` |

Virtual temperature sensors:

| Sensor range       | Offset formula            |
| ------------------ | ------------------------- |
| Virtual temp 1-16  | `0x3A + (index * 2)`      |

Use zero-based `index` in the formula. Virtual temp 1 has index `0`.

No fan, pump, fixed-speed, or RGB control is specified here.

### 6.4 Octo, PID `0xF011`

Status report length: `0x147`
Feature report length: `0x65F`

Temperature sensors:

| Sensor   | Offset |
| -------- | ------ |
| Sensor 1 | `0x3D` |
| Sensor 2 | `0x3F` |
| Sensor 3 | `0x41` |
| Sensor 4 | `0x43` |

Virtual temperature sensors:

| Sensor range       | Offset formula            |
| ------------------ | ------------------------- |
| Virtual temp 1-16  | `0x45 + (index * 2)`      |

Fan info substructure bases:

| Channel | Base offset |
| ------- | ----------- |
| fan1    | `0x7D`      |
| fan2    | `0x8A`      |
| fan3    | `0x97`      |
| fan4    | `0xA4`      |
| fan5    | `0xB1`      |
| fan6    | `0xBE`      |
| fan7    | `0xCB`      |
| fan8    | `0xD8`      |

Feature-report fixed-speed control offsets:

| Channel | Offset  |
| ------- | ------- |
| fan1    | `0x05A` |
| fan2    | `0x0AF` |
| fan3    | `0x104` |
| fan4    | `0x159` |
| fan5    | `0x1AE` |
| fan6    | `0x203` |
| fan7    | `0x258` |
| fan8    | `0x2AD` |

### 6.5 Quadro, PID `0xF00D`

Status report length: `0xDC`
Feature report length: `0x3C1`

Temperature sensors:

| Sensor   | Offset |
| -------- | ------ |
| Sensor 1 | `0x34` |
| Sensor 2 | `0x36` |
| Sensor 3 | `0x38` |
| Sensor 4 | `0x3A` |

Virtual temperature sensors:

| Sensor range       | Offset formula            |
| ------------------ | ------------------------- |
| Virtual temp 1-16  | `0x3C + (index * 2)`      |

Fan info substructure bases:

| Channel | Base offset |
| ------- | ----------- |
| fan1    | `0x70`      |
| fan2    | `0x7D`      |
| fan3    | `0x8A`      |
| fan4    | `0x97`      |

Flow sensor:

| Field       | Offset | Scale         |
| ----------- | ------ | ------------- |
| flow sensor | `0x6E` | raw `dL/h`    |

Feature-report fixed-speed control offsets:

| Channel | Offset  |
| ------- | ------- |
| fan1    | `0x036` |
| fan2    | `0x08B` |
| fan3    | `0x0E0` |
| fan4    | `0x135` |

---

## 7. Feature Report Writes

### 7.1 Fan-control substructure

Each writable fan/pump channel has a feature-report substructure at the offsets listed in
section 6.

| Relative offset | Size | Meaning                                      |
| --------------- | ---- | -------------------------------------------- |
| `0x00`          | 1    | control mode                                 |
| `0x01`          | 2    | direct duty in centi-percent, big-endian     |

Known control mode values:

| Value  | Meaning                                      |
| ------ | -------------------------------------------- |
| `0x00` | manual/direct percent mode                   |
| `0x01` | PID control mode                             |
| `0x02` | curve mode                                   |

This spec only writes mode `0x00`.

### 7.2 Direct fixed-speed write

Inputs:

- channel name
- duty percent `0..=100`
- device descriptor with feature-report length and channel offset

Sequence:

1. Clamp duty to `0..=100`.
2. Fetch the full feature report with report ID `0x03` and the descriptor's feature length.
3. Validate `report[0] == 0x03` and `report.len() == feature_length`.
4. Resolve the channel to its feature-report offset.
5. Set `report[offset + 0] = 0x00`.
6. Encode `duty * 100` as big-endian u16 and write it to `report[offset + 1 .. offset + 3]`.
7. Recompute CRC-16/USB over `report[1 .. feature_length-2]`.
8. Store the CRC big-endian at `report[feature_length-2 .. feature_length]`.
9. Wait at least 200 ms before `SET_FEATURE_REPORT`.
10. Send the full feature report.

The 200 ms delay protects devices that reject quick successive configuration writes. Keep it
even if local tests pass without it.

### 7.3 Unsupported writes

Do not write the following from this spec:

- RGB colors or effects
- fan curves
- PID settings
- virtual temperature values
- persistent save reports

Save reports are known to exist in vendor software behavior, but direct control works from
the feature report alone in the scoped behavior. Do not invent a save report without capture.

---

## 8. Channel Semantics and hwmon Coexistence

### 8.1 Direct channel names

| Device  | Valid direct channels              |
| ------- | ---------------------------------- |
| D5 Next | `pump`, `fan`                      |
| Octo    | `fan1` through `fan8`              |
| Quadro  | `fan1` through `fan4`              |

Farbwerk and Farbwerk 360 have no fixed-speed channels in this spec.

### 8.2 hwmon preference

When a Linux hwmon device is bound and exposes the required attributes, prefer hwmon for
reads and writes unless the caller explicitly asks for direct HID access.

Generic fan mapping:

| User channel | hwmon PWM       | hwmon enable       |
| ------------ | --------------- | ------------------ |
| `fanN`       | `pwmN`          | `pwmN_enable`      |

D5 Next custom mapping:

| User channel | hwmon PWM | hwmon enable  |
| ------------ | --------- | ------------- |
| `pump`       | `pwm1`    | `pwm1_enable` |
| `fan`        | `pwm2`    | `pwm2_enable` |

### 8.3 hwmon fixed-speed write

For hwmon fixed speed:

1. Resolve `pwmX` and `pwmX_enable`.
2. Require both attributes to exist.
3. Write `1` to `pwmX_enable` to select direct PWM mode.
4. Wait at least 200 ms.
5. Convert duty percent to PWM with integer truncation:

```text
pwm = duty * 255 / 100
```

6. Write `pwm` to `pwmX`.

If hwmon is present but lacks the required PWM attributes, direct HID writes may be used
only when the caller allows direct access.

---

## 9. HAL Integration and Tests

### 9.1 Driver shape

Recommended module split:

- `descriptor`: device registry, report lengths, offsets, capabilities
- `status`: status-report parsing and sensor structs
- `control`: feature-report mutation, CRC, fixed-speed writes
- `hwmon`: optional hwmon attribute mapping
- `protocol`: Hypercolor HAL `Protocol` integration

Descriptor capabilities:

| Capability             | D5 Next | Farbwerk | Farbwerk 360 | Octo | Quadro |
| ---------------------- | ------- | -------- | ------------ | ---- | ------ |
| firmware/serial        | yes     | yes      | yes          | yes  | yes    |
| physical temps         | yes     | yes      | yes          | yes  | yes    |
| virtual temps          | yes     | no       | yes          | yes  | yes    |
| fan/pump telemetry     | yes     | no       | no           | yes  | yes    |
| fixed-speed control    | yes     | no       | no           | yes  | yes    |
| RGB control            | no      | no       | no           | no   | no     |

### 9.2 Parser tests

Build synthetic status buffers for each device and assert:

- report ID validation
- serial formatting from offsets `0x03` and `0x05`
- firmware raw integer parsing from offset `0x0D`
- temperature scaling and `0x7FFF` skip behavior
- fan RPM, voltage, current, and power scaling
- D5 rail voltage scaling
- Quadro flow sensor raw `dL/h`
- no reads beyond the descriptor's status length

### 9.3 CRC and feature tests

Tests must cover:

- CRC-16/USB check vector `123456789 -> 0xB4C8`
- checksum window excludes report ID and final two checksum bytes
- checksum storage is big-endian
- direct fixed-speed write mutates only mode, duty, and checksum bytes
- duty percent is encoded as centi-percent
- unsupported channels fail before HID writes
- 200 ms delay is represented behind an injectable clock/timer in tests

### 9.4 hwmon tests

Tests must cover:

- D5 `pump` -> `pwm1`, `fan` -> `pwm2`
- generic `fanN` -> `pwmN`
- `pwmX_enable = 1` is written before `pwmX`
- duty `100` maps to PWM `255`
- missing hwmon attributes fall back only when direct access is allowed

---

## 10. Recommendation

Implement Aqua Computer support as telemetry-first with fixed fan control for D5 Next, Octo,
and Quadro. Prefer hwmon when available, because the kernel driver arbitrates device access
cleanly. Keep RGB, curves, PID editing, Farbwerk Nano, and High Flow Next blocked until
separate clean-room packet specs exist.
