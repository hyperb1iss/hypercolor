# 90 -- Wooting Protocol Driver

> Native HID driver for per-key RGB on Wooting analog keyboards and the UwU keypad. Device identification, the three wire generations and what selects between them, byte-level report layouts, and the input-safety rules that keep the keyboard typing while Hypercolor drives its lighting.

**Status:** Draft (implemented, unverified on hardware; nobody on the team owns a Wooting)
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::wooting`
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Protocol Generations](#3-protocol-generations)
4. [v1 Protocol](#4-v1-protocol)
5. [Command Channel and Colour Encoding](#5-command-channel-and-colour-encoding)
6. [v2 Protocol](#6-v2-protocol)
7. [Multi-Report Protocol](#7-multi-report-protocol)
8. [HAL Integration](#8-hal-integration)
9. [Testing Strategy](#9-testing-strategy)
10. [Known Gaps and Follow-ups](#10-known-gaps-and-follow-ups)
11. [References](#references)

---

## 1. Overview

Wooting builds Hall-effect and optical analog keyboards (One, Two, Two HE, 60HE, 80HE) and a three-key keypad (UwU). Every model with RGB exposes a vendor-defined HID collection, the "configuration" collection, that carries profile, firmware, and lighting traffic. Per-key lighting is a colour matrix written to that collection as one or more HID output reports, bracketed by two feature-report commands: one to take host control of the LEDs and one to hand them back to the active profile.

Wooting publishes an official RGB SDK for this interface. It is licensed MPL-2.0 (confirmed from the `LICENSE` file at the commit cited in [References](#references)). This spec restates the protocol facts from that SDK, from OpenRGB's Wooting controller (GPL-2.0-or-later, read as a reference only), and from public HID descriptor dumps, in Hypercolor's own words and tables. No SDK or OpenRGB code was copied or transliterated; the implementation was written from this document.

### Core conclusions

- Lighting lives on a vendor collection selected by **usage page**: `0x1337` before firmware 2.12, `0xFF55` ("multi-report") on firmware 2.12 and newer. The vendor SDK never selects by interface number.
- There are three wire generations. v1 (Atmel VID, 2017 to 2019 firmware) writes raw PWM values to the LED driver chips in checksummed parts. v2 and the multi-report generation both write one 6 x 21 matrix of RGB565 colours.
- Colours are **RGB565, little-endian**, one 16-bit cell per matrix position, with a fixed 21-column stride regardless of model width.
- Commands travel as 8-byte HID feature reports; colour frames travel as HID output reports. No step needs a reply.
- The configuration collection carries no keystrokes. Driving it through the host HID stack never touches the keyboard, mouse, consumer, analog, or gamepad functions.

---

## 2. Device Registry

### 2.1 Identification

| Field | Value | Notes |
|---|---|---|
| Vendor ID (current) | `0x31E3` | Wooting's own VID, every keyboard on v2 firmware or newer |
| Vendor ID (legacy) | `0x03EB` | Atmel's VID, borrowed by v1 firmware on the original One and Two |
| Config usage page (legacy) | `0x1337` | v1 and v2 firmware |
| Config usage page (multi-report) | `0xFF55` | Firmware 2.12 and newer, usage `0x0001` |
| Analog usage page | `0xFF54` (`0xFF53` on firmware 2.13+), `0xFFFF` on older AVR boards | Not used by this driver; listed so it is never mistaken for lighting |

Every keyboard on the `0x31E3` VID folds its **gamepad emulation mode** into the low nibble of the product ID:

| Low nibble | Mode |
|---|---|
| `0x0` | Xbox controller emulation |
| `0x1` | Classic DirectInput controller emulation |
| `0x2` | Controller emulation disabled |

A model's base PID ends in `0`; the same keyboard enumerates as `base`, `base | 1`, or `base | 2` depending on the mode set in Wootility. The v1 PIDs on the Atmel VID have no gamepad variants.

### 2.2 Model table

Matrix size is the part of the 6 x 21 wire matrix the model populates. "First row" is the wire row the model's top key row lands on.

| Model | VID | Base PID | MCU family | Matrix (rows x cols) | First row | Registered generation |
|---|---|---|---|---|---|---|
| Wooting One (v1 firmware) | `0x03EB` | `0xFF01` | AVR | 6 x 17 | 0 | v1 |
| Wooting Two (v1 firmware) | `0x03EB` | `0xFF02` | AVR | 6 x 21 | 0 | v1 |
| Wooting One | `0x31E3` | `0x1100` | AVR | 6 x 17 | 0 | v2 |
| Wooting Two | `0x31E3` | `0x1200` | AVR | 6 x 21 | 0 | v2 |
| Wooting Two Lekker Edition | `0x31E3` | `0x1210` | AVR | 6 x 21 | 0 | v2 |
| Wooting Two HE | `0x31E3` | `0x1220` | AVR | 6 x 21 | 0 | v2 |
| Wooting Two HE (ARM) | `0x31E3` | `0x1230` | ARM | 6 x 21 | 0 | multi-report |
| Wooting 60HE | `0x31E3` | `0x1300` | AVR | 5 x 14 | 1 | v2 |
| Wooting 60HE (ARM) | `0x31E3` | `0x1310` | ARM | 5 x 14 | 1 | multi-report |
| Wooting 60HE+ | `0x31E3` | `0x1320` | ARM | 5 x 14 | 1 | multi-report |
| Wooting 60HE v2 | `0x31E3` | `0x1340` | ARM | 5 x 14 | 1 | multi-report |
| Wooting 80HE | `0x31E3` | `0x1400` | ARM | 6 x 17 | 0 | multi-report |
| Wooting UwU RGB | `0x31E3` | `0x1510` | ARM | 5 x 7 | 0 | multi-report |

Notes on the geometry:

- The vendor SDK describes every keyboard as six rows. The 60% boards have no function row, so their number row sits on wire row 1 (OpenRGB's 60HE key table places every key on rows 1 to 5, columns 0 to 13). Hypercolor exposes those boards as 5 x 14 starting at wire row 1, so the spatial layout carries no empty top row.
- The UwU RGB matrix mixes lightbar LEDs, analog keys, and digital buttons in a 5 x 7 grid that does not mirror the physical layout. Hypercolor exposes the grid as-is.
- The 80HE lightbar is not reachable through this interface. Wootility 5.0.0-beta.7 notes that the lightbar keeps its own animation while an SDK application drives the keys.

### 2.3 Not registered

| Device | Reason |
|---|---|
| Wooting UwU (non-RGB), `0x31E3:0x1500` to `0x1502` | No addressable LEDs |
| Update-mode PIDs (`0x03EB:0x2402`, `0x03EB:0x2403`, `0x31E3:0x122F` and siblings) | Bootloader; lighting is never driven there |
| Wooting 80HE+ (announced 2026-09-08) | No public PID yet |
| 60HE v2 Split Spacebar and 60HE v2 Module | No separate PID found; if they reuse `0x1340` they are already covered |

---

## 3. Protocol Generations

### 3.1 What selects a generation

| Generation | VID | Config usage page | Colour reports per frame | Colour encoding |
|---|---|---|---|---|
| v1 | `0x03EB` | `0x1337` | 4 (One) or 5 (Two), 129 bytes each | 8-bit PWM per channel, CRC-protected |
| v2 | `0x31E3` | `0x1337` | 1 x 257 bytes, or 4 x 65 bytes on 64-byte collections | RGB565 matrix |
| Multi-report | `0x31E3` | `0xFF55` | 1 x 2047 bytes (report `0x05`) | RGB565 matrix |

The vendor SDK decides at runtime:

1. The VID and base PID pick the model (geometry) and whether the v1 path applies (Atmel VID only).
2. The usage page of the opened collection picks between v2 (`0x1337`) and multi-report (`0xFF55`).
3. On v2, the collection's HID report descriptor picks the packet size. A `Report Count` item with a one-byte argument (prefix `0x95`, count 64) means 64-byte reports; a two-byte argument (prefix `0x96`, count 256) means 256-byte reports. When the descriptor cannot be read, the SDK falls back to a per-model default: small packets on the Two HE (ARM), 60HE (ARM), 60HE+, and UwU boards, large everywhere else.

OpenRGB ties the usage-page switch to firmware version (v2 through 2.11 on `0x1337`, 2.12 and newer on `0xFF55`). A third-party tool built against a 60HE+ ties it to the ARM boards. The two readings agree once firmware 2.12 is out: the ARM boards moved to `0xFF55`, and no source shows an AVR board on `0xFF55`.

### 3.2 What Hypercolor registers

The HAL descriptor database maps one VID/PID to one descriptor, and a descriptor names one usage page. Hypercolor therefore registers each model on the generation its current firmware exposes:

- **v1** for the two Atmel-VID PIDs.
- **v2, large packets** for the AVR boards on `0x31E3` (One, Two, Two Lekker Edition, Two HE, 60HE).
- **Multi-report** for the ARM boards (Two HE ARM, 60HE ARM, 60HE+, 60HE v2, 80HE, UwU RGB).

An ARM board still on firmware older than 2.12 exposes `0x1337` instead. Hypercolor finds no `0xFF55` collection, the transport open fails with the candidate list in the error, and the fix is a firmware update through Wootility. The v2 small-packet encoder exists and is tested, ready for the follow-up in [§10](#10-known-gaps-and-follow-ups).

---

## 4. v1 Protocol

v1 firmware addresses the LED driver chips directly. Each frame is four parts (One) or five parts (Two). A part is one 96-byte register window of one driver chip, holding PWM duty for 24 LEDs.

### 4.1 Colour report (129 bytes, output report, unnumbered)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x00` | Unnumbered collection |
| 1 | 1 | Magic | `0xD0` | |
| 2 | 1 | Magic | `0xDA` | |
| 3 | 1 | Command | `0x0B` | Raw colours |
| 4 | 1 | Chip | 0 to 2 | LED driver chip |
| 5 | 1 | Register | `0x00` or `0x60` | First register of the 96-byte window |
| 6 | 96 | PWM | | Window contents, layout in §4.3 |
| 102 | 25 | Padding | `0x00` | |
| 127 | 2 | CRC | | CRC-16/XMODEM over bytes `0..127`, little-endian |

The checksum is CRC-16/XMODEM: polynomial `0x1021`, initial value `0x0000`, no input or output reflection, no final XOR (catalogue check value `0x31C3` for `"123456789"`). It covers the report ID byte; with a zero initial value a leading zero byte does not change the result.

### 4.2 Parts

LED index `n` belongs to part `n / 24` and slot `n % 24` inside it.

| Part | Chip | Register | LED indices | Sent for |
|---|---|---|---|---|
| 0 | 0 | `0x00` | 0 to 23 | One, Two |
| 1 | 0 | `0x60` | 24 to 47 | One, Two |
| 2 | 1 | `0x00` | 48 to 71 | One, Two |
| 3 | 1 | `0x60` | 72 to 95 | One, Two |
| 4 | 2 | `0x00` | 96 to 116 | Two only (numpad) |

The highest populated LED index is 95 on the One and 116 on the Two; higher indices are skipped.

### 4.3 PWM window layout

A window is two 48-byte banks of twelve LEDs. Inside a bank, red registers for LEDs 0 to 5 sit at `0x00` to `0x05` and LEDs 6 to 11 at `0x08` to `0x0D`; green is red `+ 0x10` and blue is red `+ 0x20`. The second bank repeats the pattern at `+ 0x30`.

| Slot | Red register | Slot | Red register |
|---|---|---|---|
| 0 to 5 | `0x00` to `0x05` | 12 to 17 | `0x30` to `0x35` |
| 6 to 11 | `0x08` to `0x0D` | 18 to 23 | `0x38` to `0x3D` |

Closed form: `red = 0x30 * (slot / 12) + 8 * ((slot % 12) / 6) + slot % 6`.

### 4.4 Key matrix to LED index

Row and column are the same 6 x 21 key matrix every generation uses (row 0 is the function row). `--` marks a cell with no LED.

| Row | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 0 | -- | 11 | 12 | 23 | 24 | 36 | 47 | 85 | 84 | 49 | 48 | 59 | 61 | 73 | 81 | 80 | 113 | 114 | 115 | 116 |
| 1 | 2 | 1 | 14 | 13 | 26 | 25 | 35 | 38 | 37 | 87 | 86 | 95 | 51 | 63 | 75 | 72 | 74 | 96 | 97 | 98 | 99 |
| 2 | 3 | 4 | 15 | 16 | 27 | 28 | 39 | 42 | 40 | 88 | 89 | 52 | 53 | 71 | 76 | 83 | 77 | 102 | 103 | 104 | 100 |
| 3 | 5 | 6 | 17 | 18 | 29 | 30 | 41 | 46 | 44 | 90 | 93 | 54 | 57 | 65 | -- | -- | -- | 105 | 106 | 107 | -- |
| 4 | 9 | 8 | 19 | 20 | 31 | 34 | 32 | 45 | 43 | 91 | 92 | 55 | -- | 66 | -- | 78 | -- | 108 | 109 | 110 | 101 |
| 5 | 10 | 22 | 21 | -- | -- | -- | 33 | -- | -- | -- | 94 | 58 | 67 | 68 | 70 | 79 | 82 | -- | 111 | 112 | -- |

ISO boards have an Enter key and a left Shift the matrix has no cell for. Their LEDs mirror the ANSI keys: whatever colour LED 65 (Enter, row 3 column 13) gets, LED 62 gets too, and LED 9 (left Shift, row 4 column 0) is mirrored onto LED 7. Neither LED 62 nor LED 7 appears anywhere else in the matrix.

### 4.5 Channel values

v1 firmware writes the PWM duty it is given. The vendor SDK runs each channel through its own gamma lookup first (and quantises to RGB565 along the way, an artefact of its shared matrix buffer). Hypercolor instead decodes the sRGB transfer curve (IEC 61966-2-1) so PWM duty is proportional to linear light: `pwm = round(255 * srgb_to_linear(value / 255))`.

| sRGB | 0 | 1 | 10 | 64 | 128 | 192 | 254 | 255 |
|---|---|---|---|---|---|---|---|---|
| PWM | 0 | 0 | 1 | 13 | 55 | 134 | 253 | 255 |

### 4.6 Golden checksums

All-black frames differ only in the chip and register bytes, so each part has a fixed checksum:

| Part | Chip | Register | CRC bytes 127, 128 |
|---|---|---|---|
| 0 | `0x00` | `0x00` | `A5 C4` |
| 1 | `0x00` | `0x60` | `7C E8` |
| 2 | `0x01` | `0x00` | `02 E9` |
| 3 | `0x01` | `0x60` | `DB C5` |
| 4 | `0x02` | `0x00` | `EB 9F` |

Part 0 with only Escape (LED 0) at full white (`0xFF` at window offsets `0x00`, `0x10`, `0x20`) carries `47 68`.

---

## 5. Command Channel and Colour Encoding

### 5.1 Feature commands (8 bytes, feature report)

| Offset | Size | Field | v1 / v2 | Multi-report |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x00` | `0x01` |
| 1 | 1 | Magic | `0xD0` | `0xD1` |
| 2 | 1 | Magic | `0xDA` | `0xDA` |
| 3 | 1 | Command | | |
| 4 | 1 | Parameter 3 | | |
| 5 | 1 | Parameter 2 | | |
| 6 | 1 | Parameter 1 | | |
| 7 | 1 | Parameter 0 | | |

The parameters go out in reverse of their logical order: the vendor SDK names them 0 to 3 and writes parameter 3 first. On the multi-report collection the feature report is report ID 1 with seven data bytes.

| Command | ID | Parameters | Used by Hypercolor |
|---|---|---|---|
| Raw colours | `0x0B` (11) | Only ever the third byte of a colour report | Every frame |
| Device configuration | `0x13` (19) | None; the layout byte (0 ANSI, 1 ISO, 2 JIS, 3 ANSI split space, 4 ISO split space) is at response offset 10 on v2 and newer, 9 on v1 | No |
| Single key colour | `0x1E` (30) | v2+: P0 = packed matrix position (column in bits 0 to 4, row in bits 5 to 7), P1 red, P2 green, P3 blue. v1: P0 = LED index | No |
| Single key reset | `0x1F` (31) | P3 = position (v2+) or LED index (v1) | No |
| Reset all | `0x20` (32) | None | Shutdown |
| Colour init | `0x21` (33) | None | Init |

### 5.2 RGB565 colour cells

v2 and multi-report colour reports carry the matrix as 126 little-endian 16-bit cells, row-major, with a 21-column stride: cell `(row, col)` sits at report offset `4 + 2 * (21 * row + col)`. Models narrower than 21 columns or shorter than 6 rows leave the rest black.

`cell = (red & 0xF8) << 8 | (green & 0xFC) << 3 | blue >> 3`

| RGB | Cell | Wire bytes |
|---|---|---|
| `FF 00 00` | `0xF800` | `00 F8` |
| `00 FF 00` | `0x07E0` | `E0 07` |
| `00 00 FF` | `0x001F` | `1F 00` |
| `12 34 56` | `0x11AA` | `AA 11` |
| `FF FF FF` | `0xFFFF` | `FF FF` |
| `01 02 03` | `0x0000` | `00 00` |

The firmware corrects colour on the board, so the cells take the sRGB values directly.

### 5.3 Responses

Every feature command produces a reply as an HID **input report** on the same collection (never a feature-report read). On v2 the reply is 256 bytes, read in as many input reports as it takes; on v1 it is 128 bytes. On the multi-report collection it is one input report (33 bytes with the report ID for ordinary commands), with byte 4 a status: `0x88` success, `0x66` unsupported. The vendor SDK reads and discards the reply to every command it sends, with a 1 s timeout.

Hypercolor reads no replies. Its HAL reads a command's reply over the same transfer type the command used, which for a feature command is a feature-report read, not the input report the firmware sends. An unread input report is harmless: the host HID stack keeps polling the endpoint while the device is open, and the reply is dropped from the per-handle queue when it fills. A rejected feature report fails at the write itself.

### 5.4 Sequences

| Phase | Commands | Timing |
|---|---|---|
| Connect | Colour init (`0x21`) | 50 ms settle before the first frame (OpenRGB's figure; the SDK instead blocks up to 1 s on the reply) |
| Each frame | Colour report(s), §4, §6, or §7 | No inter-report delay |
| Disconnect | Reset all (`0x20`) | None; the keyboard returns to the active profile's lighting |

No keepalive exists. Nothing in the SDK, OpenRGB, or user reports suggests the keyboard reverts on its own while a host holds colour control; it stays on the last frame until reset or unplugged.

---

## 6. v2 Protocol

### 6.1 Colour report (257 bytes, output report, unnumbered)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x00` | Unnumbered collection |
| 1 | 1 | Magic | `0xD0` | |
| 2 | 1 | Magic | `0xDA` | |
| 3 | 1 | Command | `0x0B` | Raw colours |
| 4 | 252 | Matrix | | 126 RGB565 cells, §5.2 |
| 256 | 1 | Padding | `0x00` | Fills the 256-byte report |

### 6.2 Small packets (4 x 65 bytes)

On collections that declare 64-byte reports, the 256 bytes after the report ID (offsets 1 to 256 above) go out as four consecutive output reports of 64 bytes, each with its own `0x00` report ID in front:

| Report | Report ID | Payload |
|---|---|---|
| 1 | `0x00` | Bytes 1 to 64 (magic, command, cells 0 to 29, low byte of cell 30) |
| 2 | `0x00` | Bytes 65 to 128 |
| 3 | `0x00` | Bytes 129 to 192 |
| 4 | `0x00` | Bytes 193 to 256 (ends with the padding byte) |

---

## 7. Multi-Report Protocol

### 7.1 Report IDs

The `0xFF55` collection declares numbered reports of several sizes, symmetric for input and output. From a published HID descriptor dump of a Two HE (ARM) in no-gamepad mode:

| Report ID | Data bytes | Role |
|---|---|---|
| 1 | 32 in / out, 7 feature | Feature commands (§5.1) and ordinary replies |
| 2 | 62 | |
| 3 | 254 | |
| 4 | 510 | |
| 5 | 1022 | Colour frames (vendor SDK) |
| 6 | 2046 | Longest report |

A colour payload needs 255 data bytes (two magic bytes, the command, 252 matrix bytes), which every report from ID 4 up holds. The vendor SDK sends ID 5; its source comment calls ID 5 the 510-byte report and ID 4 the 254-byte one, one ID off from the descriptor dump. OpenRGB sends ID 4 as a 2046-byte write. Both work in the field, since the payload fits either report. Hypercolor follows the vendor SDK byte for byte.

### 7.2 Colour report (2047-byte write, output report `0x05`)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x05` | |
| 1 | 1 | Magic | `0xD1` | |
| 2 | 1 | Magic | `0xDA` | |
| 3 | 1 | Command | `0x0B` | Raw colours |
| 4 | 252 | Matrix | | 126 RGB565 cells, §5.2 |
| 256 | 1791 | Padding | `0x00` | Out to the collection's longest output report |

The write is always 2047 bytes, the longest report plus its ID, which is the length Windows requires for any output write to the collection and the length the vendor SDK uses on every platform.

---

## 8. HAL Integration

### 8.1 Module layout

| File | Contents |
|---|---|
| `drivers/wooting/types.rs` | VIDs, usage pages, command IDs, report sizes, `GamepadMode`, `WootingInterface` (v1, v2 with packet size, multi-report), `WootingModel` geometry |
| `drivers/wooting/packet.rs` | Zerocopy report structs with compile-time size assertions, RGB565 packing, matrix fill, CRC-16/XMODEM |
| `drivers/wooting/v1.rs` | §4 LED index matrix, PWM register layout, part addressing, sRGB decode |
| `drivers/wooting/protocol.rs` | `WootingProtocol`: one encoder per model and generation, `encode_frame_into` over `CommandBuffer`, `Cow` normalization |
| `drivers/wooting/devices.rs` | `MODELS` table, gamepad-variant expansion, descriptors, `transport_for` |

Driver ID `wooting`, family `wooting`. Protocol IDs are `wooting/<model>` (v1 adds `-v1`); the three gamepad PIDs of a model share one protocol ID.

### 8.2 Transport and input safety

These are keyboards, so the one hard rule is that Hypercolor never takes an interface away from the kernel. Every descriptor uses `TransportType::UsbHidApi` on every platform:

- `interface: None`, `usage_page: Some(0x1337 or 0xFF55)`, `usage: None`. The collection is chosen by usage page exactly as the vendor SDK chooses it, so a gamepad interface that shifts interface numbers cannot redirect writes.
- `report_mode: OutputReportWithReportId`. Packets carry their own report ID; colour reports go out as output reports (`TransferType::Primary`) and commands as feature reports (`TransferType::HidReport`) on the same handle.
- `max_report_len`: 129 (v1), 257 (v2), 2047 (multi-report).

hidapi on Linux is the hidraw backend, on macOS opens devices in shared mode, and on Windows goes through the HID class driver. None of them opens the USB device node or claims an interface, so `usbhid` (and its macOS and Windows counterparts) stays bound to the keyboard, consumer, mouse, analog, and gamepad functions. The descriptors deliberately avoid the Linux `HostManaged` intent, whose hidraw transport cannot mix feature and output reports on one handle.

### 8.3 udev rules

A product-wide hidraw rule would hand the fallback group every hidraw node of the keyboard, including the boot keyboard and analog interfaces, which is a keystroke feed. A `usb_device` rule would let a group member detach the keyboard's input driver through usbfs. Hypercolor ships neither for Wooting.

The ARM boards carry the configuration collection alone on interface 2 (a Windows descriptor dump of the Two HE ARM, and an lsusb dump of the 60HE ARM in which interface 2 is the only one with both IN and OUT endpoints). The rules match the interface's `modalias`, which pins VID, PID, interface class, and interface number on one sysfs device:

```
SUBSYSTEM=="hidraw", ATTRS{modalias}=="usb:v31E3p1230d*ic03isc00ip00in02", MODE="0660", GROUP="users", TAG+="uaccess"
```

`ic03isc00ip00` demands a non-boot HID interface, so the rule can never match the boot keyboard (`ic03isc01ip01`) or the vendor-class Xbox gamepad function. If Xbox emulation inserted its interface ahead of the configuration collection, interface 2 would become the keyboard or the gamepad and the rule would match nothing.

| Products | Rule |
|---|---|
| ARM boards, Xbox (`base`) and no-gamepad (`base | 2`) PIDs | Interface 2, as above |
| ARM boards, DirectInput PIDs (`base | 1`) | None: a DirectInput gamepad is itself a non-boot HID interface, so a shifted interface number could land the grant on it |
| AVR boards and v1 firmware | None: no published dump confirms their configuration interface |

Products without a rule work as root or with Wooting's own udev rules, which Wootility on Linux requires anyway (help.wooting.io article 147).

### 8.4 Topology

One zone per device, `Keyboard` (or `Keypad` on the UwU), `DeviceTopologyHint::Matrix { rows, cols }` with the §2.2 geometry and `rows * cols` LEDs, colour format RGB. Colours are row-major. Cells with no LED behind them are part of the matrix and simply stay dark, the same convention the Razer matrix keyboards use.

### 8.5 Timing

| Generation | Frame interval | Reports per frame |
|---|---|---|
| v1 | 33 ms | 4 or 5 x 129 bytes, from an 8-bit controller |
| v2 | 16 ms | 1 x 257 or 4 x 65 bytes |
| Multi-report | 16 ms | 1 x 2047 bytes |

Neither the SDK nor OpenRGB throttles frames; these intervals are Hypercolor's choice.

### 8.6 Coexistence

- **OpenRGB.** `crates/hypercolor-openrgb-host/data/detectors.toml` gains a `wooting` family (prefix `Wooting `) seeded with the thirteen detector names from the OpenRGB 1.0 id map. While the native driver owns an enabled Wooting device, the managed OpenRGB config withholds the detectors that claim its PIDs.
- **Wootility.** Not a conflict. Wootility 5 handles several applications on the keyboard at once and leaves the lighting to an SDK application until that application resets it. The conflict catalog records why it is absent.
- **Other SDK applications.** SignalRGB is already a whole-system entry in the conflict catalog. Two SDK applications writing frames at once both succeed, and the keyboard shows whichever frame landed last.

### 8.7 Stability label

The driver is experimental: enabled by default, labelled as untested on real hardware, and recorded as unverified in the device database.

---

## 9. Testing Strategy

No test opens a device. `crates/hypercolor-hal/tests/`:

| File | Asserts |
|---|---|
| `wooting_protocol_tests.rs` | RGB565 table (§5.2), CRC check value, feature commands per generation (§5.1), multi-report frame length, header and 21-column stride (§7), 60% row offset and UwU grid (§2.2), v2 large report and small-packet split (§6), v1 register layout, sRGB decode table, golden checksums, ANSI to ISO mirroring, numpad chip (§4), zone geometry, truncate and pad, command-buffer reuse without reallocation |
| `wooting_devices_tests.rs` | Every VID/PID in §2 resolves to exactly one descriptor and protocol ID; §2.3 products stay unregistered; gamepad variants agree; each generation binds its usage page and report length; every transport rides the host HID stack and selects the vendor collection by usage page; the hidapi and hidraw transport sources contain no interface claim or detach call |
| `udev_rules_tests.rs` | Each §8.3 product has exactly its interface-scoped rule, every other Wooting product has none, no Wooting rule opens the USB device node, and every Wooting rule excludes the boot keyboard class |

`benches/protocol_encoding.rs` carries an 80HE multi-report case and a Two v1 case.

---

## 10. Known Gaps and Follow-ups

1. **ARM boards on firmware before 2.12.** Registered on `0xFF55`, so they fail to connect until updated. Supporting both pages needs the HAL to pick between descriptors that share a VID/PID by the usage pages actually present, or a HID transport that accepts a fallback usage page. The v2 small-packet encoder is ready for that.
2. **v2 packet size from the report descriptor.** The AVR boards use the SDK's large-packet default. A board whose descriptor declares 64-byte reports would need the descriptor read the SDK performs.
3. **Interface layout of the AVR boards and v1 firmware.** No public dump; their udev rules wait on one.
4. **80HE+ PID.** Unknown at the time of writing.
5. **Keyboard layout.** The device configuration reply names ANSI, ISO, or JIS. Reading it would let the spatial layout label keys, but it needs an input-report read the HAL cannot yet pair with a feature-report command.
6. **Hardware verification.** Every byte here is from the sources below, cross-checked by golden tests, and has never reached a Wooting.

---

## References

- Wooting RGB SDK, `github.com/WootingKb/wooting-rgb-sdk` at commit `7e091a0dbdbc3fb01b5e7f7e77b18a7548c595aa` (2026-10-26), files `src/wooting-usb.c`, `src/wooting-usb.h`, `src/wooting-rgb-sdk.c`, `src/wooting-rgb-sdk.h`, `README.md`, `resources/uwu-matrix.png`. Licensed MPL-2.0 per its `LICENSE` file. Read as a reference; nothing copied.
- OpenRGB, `gitlab.com/CalcProgrammer1/OpenRGB`, `Controllers/WootingKeyboardController/` at commit `3a74c85be849deac713a3df71e90af54e9e0e19f` (GPL-2.0-or-later): detector registration, the v2/v3 firmware split, the V3 report choice, and the 60% key table. Read as a reference; nothing copied.
- Two HE (ARM) `0x31E3:0x1232` HID descriptor dump, firmware 2.12+: https://github.com/hrvach/deskhop/issues/335
- 60HE (ARM) `0x31E3:0x1312` USB descriptor dump: https://forum.level1techs.com/t/wooting-60he-and-hid-ports/210911
- wootswitch HID protocol notes (60HE+, reply status bytes, usage page per board family): https://github.com/clemenscodes/wootswitch/blob/main/docs/hid-protocol.md
- Wooting analog SDK usage pages: https://github.com/WootingKb/wooting-analog-sdk
- Wooting udev rules for Wootility on Linux: https://help.wooting.io/article/147-configuring-device-access-for-wootility-under-linux-udev-rules
- Wootility 5.0.0-beta.7 changelog (multiple applications, lightbar under SDK control): https://wooting.io/wootility/changelogs/5.0.0-beta.7
- 80HE+ announcement: https://wooting.io/post/where-did-the-wooting-80he-go
- Spec 16 (Hardware Abstraction Layer) and Spec 62 (Zerocopy Protocol Structs) for the HAL contracts this driver plugs into.
