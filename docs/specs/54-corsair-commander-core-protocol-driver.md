# 54 -- Corsair Commander Core Protocol Driver

> Clean-room implementation specification for Corsair Commander Core, Commander Core XT, and Commander ST telemetry and fan control. This document is sufficient to implement the scoped driver from the spec alone.

**Status:** Draft (clean-room closure)
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::corsair::commander_core`
**Author:** Nova
**Date:** 2026-05-04

---

## Table of Contents

1. [Overview](#1-overview)
2. [Clean-Room Boundary](#2-clean-room-boundary)
3. [Device Registry](#3-device-registry)
4. [HID Transport and Framing](#4-hid-transport-and-framing)
5. [Command Vocabulary](#5-command-vocabulary)
6. [Endpoint Transactions](#6-endpoint-transactions)
7. [Parsed Data Layouts](#7-parsed-data-layouts)
8. [Channel Mapping and Semantics](#8-channel-mapping-and-semantics)
9. [HAL Integration and Tests](#9-hal-integration-and-tests)
10. [Recommendation](#10-recommendation)

---

## 1. Overview

This spec covers the Commander Core family control endpoint used for:

- firmware version reads
- LED-count/topology reads
- fan and pump presence reads
- temperature sensor presence and values
- current pump/fan RPM reads
- fixed duty fan/pump control
- temperature curve fan/pump control

This spec does **not** define RGB frame streaming. The known packet set can discover RGB
port LED counts, but no direct lighting packet is specified here. Implement lighting only
after a separate packet-level RGB spec or USB capture exists.

The device protocol is a small command envelope over USB HID. Most data is accessed by
opening logical endpoint `0x00` in a mode, performing one or more read/write commands, then
closing endpoint `0x00`.

---

## 2. Clean-Room Boundary

This document is the allowed implementation source. It intentionally records packet bytes,
offsets, endianness, state transitions, and validation rules in Hypercolor-owned wording.

Implementation rules:

- Do not consult, port, translate, or compare against third-party implementations while
  writing the Hypercolor driver.
- Do not import test fixtures from previous projects. Build new golden packets from this spec.
- If a field is missing here, capture traffic from hardware or write a new facts-only research
  addendum before implementing that behavior.
- Treat historical research inputs as provenance only; they are not implementation references.

---

## 3. Device Registry

All devices use Corsair VID `0x1B1C`, HID interface `0`, and the 96-byte HID command protocol
described below.

| Product                | VID:PID       | `has_pump` | Speed ports | RGB/LED-count ports | Notes                                      |
| ---------------------- | ------------- | ---------- | ----------- | ------------------- | ------------------------------------------ |
| Commander Core         | `1B1C:0C1C`   | yes        | 7           | 7                   | Port 0 is AIO/pump; ports 1-6 are fans/RGB |
| Commander Core XT      | `1B1C:0C2A`   | no         | 6           | 7                   | No pump channel; fan 1 maps to port 0      |
| Commander ST           | `1B1C:0C32`   | yes        | 7           | 7                   | Treat like Core until hardware proves else |

Risk note: these devices have a history of firmware-specific quirks. Ship the first
Hypercolor implementation as experimental behind explicit device descriptors, with robust
sleep-on-error behavior and packet tests.

---

## 4. HID Transport and Framing

### 4.1 Report size

Host writes are `97` bytes:

- byte `0`: HID report ID, always `0x00`
- bytes `1..=96`: 96-byte command payload

Device reads are `96` bytes. Reads may return unrelated queued HID reports; keep reading
until response byte `0` equals `0x00`.

### 4.2 Outbound command envelope

Every outbound packet starts with:

```text
00 08 <command...> <data...> <zero padding to 97 bytes>
```

Offsets in the full host write buffer:

| Offset | Size | Meaning                                      |
| ------ | ---- | -------------------------------------------- |
| `0x00` | 1    | HID report ID, always `0x00`                 |
| `0x01` | 1    | Controller namespace, always `0x08`          |
| `0x02` | 1    | Command opcode                               |
| `0x03` | N    | Command arguments, if any                    |
| after  | N    | Command data, if any                         |
| rest   | N    | Zero padding to a 97-byte host write buffer  |

### 4.3 Inbound response envelope

Response bytes:

| Offset | Size | Meaning                                         |
| ------ | ---- | ----------------------------------------------- |
| `0x00` | 1    | Valid response marker, expected `0x00`          |
| `0x01` | 1    | Echo of outbound command opcode                 |
| `0x02` | 1    | Reserved/status byte unless noted               |
| `0x03` | N    | Command-specific response payload               |

Validation:

- Ignore queued reads until byte `0` is `0x00`.
- Require response byte `1` to equal the outbound command opcode.
- Treat a mismatched echo as a transport error.

---

## 5. Command Vocabulary

All packets below are shown as full host-write prefixes before zero padding.

### 5.1 Global commands

| Name              | Bytes                        | Response payload                                 |
| ----------------- | ---------------------------- | ------------------------------------------------ |
| Wake/software     | `00 08 01 03 00 02`          | Command echo only                                |
| Sleep/hardware    | `00 08 01 03 00 01`          | Command echo only                                |
| Get firmware      | `00 08 02 13`                | Version bytes at response offsets `3, 4, 5`      |
| Close endpoint 0  | `00 08 05 01 00`             | Command echo only                                |

Firmware version formatting is:

```text
<response[3]>.<response[4]>.<response[5]>
```

Example: response bytes `... 01 02 21 ...` produce firmware `1.2.33`.

### 5.2 Endpoint commands

| Name              | Bytes                        | Notes                                            |
| ----------------- | ---------------------------- | ------------------------------------------------ |
| Open endpoint 0   | `00 08 0D 00 <mode...>`      | `mode` is one or two bytes                       |
| Read initial      | `00 08 08 00 01`             | First response contains data type at `3..=4`     |
| Read more         | `00 08 08 00 02`             | Continuation data starts at response offset `3`  |
| Read final        | `00 08 08 00 03`             | Continuation data starts at response offset `3`  |
| Write first       | `00 08 06 00 <write header>` | First chunk includes write header and data type  |
| Write more        | `00 08 07 00 <data chunk>`   | Continuation data only                           |

### 5.3 Modes and data types

| Purpose                   | Open mode bytes | Data type bytes |
| ------------------------- | --------------- | --------------- |
| LED count/topology        | `20`            | `0F 00`         |
| Current speeds            | `17`            | `06 00`         |
| Current temperatures      | `21`            | `10 00`         |
| Connected speed devices   | `1A`            | `09 00`         |
| Hardware speed mode       | `60 6D`         | `03 00`         |
| Hardware fixed percent    | `61 6D`         | `04 00`         |
| Hardware curve percent    | `62 6D`         | `05 00`         |

Hardware speed mode values:

| Value  | Meaning              |
| ------ | -------------------- |
| `0x00` | Fixed duty percent   |
| `0x02` | Temperature curve    |

Other mode values are unknown and must be preserved when not updating that port.

---

## 6. Endpoint Transactions

### 6.1 Wake guard

All public operations must run inside a wake guard:

1. Send wake/software.
2. Perform all reads/writes.
3. Send sleep/hardware in `finally`/`Drop`, even when parsing or transport fails.

Failing to sleep the device can leave lighting or cooling behavior in a transient software
mode. Tests must cover sleep-on-error.

### 6.2 Read transaction

Inputs:

- `mode`: bytes from section 5.3
- `data_type`: bytes from section 5.3

Sequence:

1. Open endpoint 0 with `00 08 0D 00 <mode...>`.
2. Send read initial.
3. Send read more.
4. Send read final.
5. Close endpoint 0.
6. Validate that the initial response bytes `3..=4` equal `data_type`.
7. Return the concatenated data payload:

```text
initial_response[5..96] + read_more_response[3..96] + read_final_response[3..96]
```

The returned payload is padded. Use the count byte in each data layout to determine the
meaningful length.

### 6.3 Write transaction

Inputs:

- `mode`: bytes from section 5.3
- `data_type`: bytes from section 5.3
- `data`: complete replacement payload for the selected data type

Sequence:

1. First perform the read transaction for the same `mode`/`data_type`. This validates the
   endpoint before writing and reduces the chance of writing the wrong configuration block.
2. Open endpoint 0 with `00 08 0D 00 <mode...>`.
3. Send `Write first` with a write header plus the first data chunk.
4. Send `Write more` chunks until all remaining data bytes are sent.
5. Close endpoint 0.

Full write-first packet layout:

| Full offset | Size | Value                                                  |
| ----------- | ---- | ------------------------------------------------------ |
| `0x00`      | 1    | `00` report ID                                         |
| `0x01`      | 1    | `08` namespace                                         |
| `0x02`      | 1    | `06` write-first opcode                                |
| `0x03`      | 1    | `00` endpoint                                          |
| `0x04`      | 2    | little-endian total length: `len(data_type)+len(data)` |
| `0x06`      | 2    | reserved, `00 00`                                      |
| `0x08`      | 2    | data type bytes                                        |
| `0x0A`      | N    | first data chunk                                       |

Chunk sizing:

| Packet       | Data bytes available | Full write data starts |
| ------------ | -------------------- | ---------------------- |
| Write first  | `87`                 | offset `0x0A`          |
| Write more   | `93`                 | offset `0x04`          |

If `data.len() <= 87`, no `Write more` packet is needed.

---

## 7. Parsed Data Layouts

All multi-byte fields in endpoint data payloads are little-endian.

### 7.1 LED count/topology, data type `0F 00`

Payload:

| Offset | Size | Meaning                                 |
| ------ | ---- | --------------------------------------- |
| `0x00` | 1    | channel record count                    |
| `0x01` | 4*N  | channel records                         |

Channel record:

| Relative offset | Size | Meaning                                           |
| --------------- | ---- | ------------------------------------------------- |
| `0x00`          | 2    | status: `0x0002` connected, `0x0003` disconnected |
| `0x02`          | 2    | LED count, valid only when connected              |

Labeling:

- `has_pump = true`: record `0` is AIO LED count; records `1..=6` are RGB ports 1-6.
- `has_pump = false`: records are RGB ports 1-7.

This topology data is read-only in this spec.

### 7.2 Connected speed devices, data type `09 00`

Payload:

| Offset | Size | Meaning                         |
| ------ | ---- | ------------------------------- |
| `0x00` | 1    | port count                      |
| `0x01` | N    | one status byte per speed port  |

Status values:

| Value  | Meaning       |
| ------ | ------------- |
| `0x07` | connected     |
| `0x01` | not connected |

Other values are unknown; report them as unknown rather than connected.

### 7.3 Current speeds, data type `06 00`

Payload:

| Offset | Size | Meaning                                  |
| ------ | ---- | ---------------------------------------- |
| `0x00` | 1    | speed count                              |
| `0x01` | 2*N  | little-endian RPM value per speed port   |

An absent or stalled port reports RPM `0`.

### 7.4 Current temperatures, data type `10 00`

Payload:

| Offset | Size | Meaning                                      |
| ------ | ---- | -------------------------------------------- |
| `0x00` | 1    | temperature sensor count                     |
| `0x01` | 3*N  | temperature records                          |

Temperature record:

| Relative offset | Size | Meaning                                      |
| --------------- | ---- | -------------------------------------------- |
| `0x00`          | 1    | status: `0x00` connected, `0x01` disconnected |
| `0x01`          | 2    | little-endian tenths of deg C                |

Temperature value:

```text
deg_c = raw_u16_le / 10.0
```

### 7.5 Hardware speed mode, data type `03 00`

Payload:

| Offset | Size | Meaning                       |
| ------ | ---- | ----------------------------- |
| `0x00` | 1    | speed port count              |
| `0x01` | N    | one mode byte per speed port  |

To set a port to fixed duty, change that port's mode byte to `0x00`. To set a port to
curve mode, change it to `0x02`. Preserve all other ports and unknown mode values.

### 7.6 Hardware fixed percent, data type `04 00`

Payload:

| Offset | Size | Meaning                                      |
| ------ | ---- | -------------------------------------------- |
| `0x00` | 1    | speed port count                             |
| `0x01` | 2*N  | little-endian duty percent per speed port    |

Duty values are integer percents in the range `0..=100`. Clamp user input before writing.

### 7.7 Hardware curve percent, data type `05 00`

Payload:

| Offset | Size | Meaning                   |
| ------ | ---- | ------------------------- |
| `0x00` | 1    | speed port count          |
| `0x01` | var  | one curve record per port |

Curve record:

| Relative offset | Size | Meaning                                           |
| --------------- | ---- | ------------------------------------------------- |
| `0x00`          | 1    | temperature sensor selector, normally `0x00`      |
| `0x01`          | 1    | point count                                       |
| `0x02`          | 4*N  | curve points                                      |

Curve point:

| Relative offset | Size | Meaning                                |
| --------------- | ---- | -------------------------------------- |
| `0x00`          | 2    | little-endian tenths of deg C          |
| `0x02`          | 2    | little-endian duty percent             |

Validation:

- Accept `2..=7` curve points.
- Clamp duty to `0..=100`.
- Preserve curve records for ports not being updated.
- Use sensor selector `0x00` unless a future capture documents other selectors.

---

## 8. Channel Mapping and Semantics

### 8.1 Speed channel indexes

Internal speed port indexes are zero-based.

Commander Core and Commander ST (`has_pump = true`):

| User channel | Internal port |
| ------------ | ------------- |
| `pump`       | `0`           |
| `fan1`       | `1`           |
| `fan2`       | `2`           |
| `fan3`       | `3`           |
| `fan4`       | `4`           |
| `fan5`       | `5`           |
| `fan6`       | `6`           |
| `fans`       | `1..=6`       |

Commander Core XT (`has_pump = false`):

| User channel | Internal port |
| ------------ | ------------- |
| `fan1`       | `0`           |
| `fan2`       | `1`           |
| `fan3`       | `2`           |
| `fan4`       | `3`           |
| `fan5`       | `4`           |
| `fan6`       | `5`           |
| `fans`       | `0..=5`       |

### 8.2 Fixed speed update

For `set_fixed_speed(channel, duty)`:

1. Resolve user channel to one or more internal ports.
2. In wake guard, read hardware speed mode (`03 00`), set target ports to `0x00`, and write
   the full mode payload back.
3. Read hardware fixed percent (`04 00`), set target ports to clamped duty percent, and write
   the full fixed-percent payload back.
4. Sleep device on success or error.

### 8.3 Curve update

For `set_speed_profile(channel, points)`:

1. Validate `2..=7` points.
2. Resolve user channel to one or more internal ports.
3. In wake guard, read hardware speed mode (`03 00`), set target ports to `0x02`, and write
   the full mode payload back.
4. Read hardware curve percent (`05 00`), replace target port curve records, preserve all
   other records, and write the full curve payload back.
5. Sleep device on success or error.

### 8.4 Hardware limitations

Some pumps and fans enforce minimum speed limits and will not stop even when duty is `0`.
The driver should report the requested duty separately from measured RPM.

Some devices visibly blink or flash lighting when fan configuration is written. That is a
device behavior of the known command path. Hypercolor should avoid repeated writes for
unchanged values.

---

## 9. HAL Integration and Tests

### 9.1 Driver shape

Implement a family-specific protocol module with:

- a transport helper that owns the wake guard and response validation
- `read_endpoint(mode, data_type) -> Vec<u8>`
- `write_endpoint(mode, data_type, data)`
- parsers for each data type in section 7
- channel mapping driven by the descriptor's `has_pump` flag

Suggested descriptor capabilities:

| Capability          | Commander Core | Commander Core XT | Commander ST |
| ------------------- | -------------- | ----------------- | ------------ |
| telemetry speeds    | yes            | yes               | yes          |
| telemetry temps     | yes            | yes               | yes          |
| fixed fan control   | yes            | yes               | yes          |
| curve fan control   | yes            | yes               | yes          |
| RGB streaming       | no             | no                | no           |

Register lighting as unsupported until a separate RGB packet spec lands.

### 9.2 Packet tests

Golden packet tests must cover:

- wake packet
- sleep packet
- get firmware packet and response parse
- open endpoint for every mode in section 5.3
- close endpoint
- read transaction payload concatenation and data-type validation
- write-first chunk layout for short and long payloads
- write-more chunking at 87/93 byte boundaries

### 9.3 Parser tests

Parser tests must cover:

- connected and disconnected LED records
- connected speed status values `0x07` and `0x01`
- RPM little-endian parsing
- temperature connected/disconnected records
- fixed-percent read/modify/write preserving other ports
- curve read/modify/write preserving other ports

### 9.4 State tests

State tests must cover:

- sleep is sent after successful initialization
- sleep is sent after parser error
- sleep is sent after transport error
- invalid channels are rejected before device writes
- unchanged control values do not issue writes

---

## 10. Recommendation

Implement this as an experimental Commander Core family telemetry and fan-control driver,
not as an RGB driver. The protocol is well-scoped enough for safe firmware reads, topology
reads, RPM/temperature telemetry, fixed duty control, and curve control. RGB streaming
should remain blocked until a separate clean-room lighting spec exists.
