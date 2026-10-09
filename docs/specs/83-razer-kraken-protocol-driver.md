# 83 -- Razer Kraken Protocol Driver

> Native driver for Razer Kraken headsets that keep their lighting in a
> memory map instead of speaking the 90-byte Razer feature report. Wire
> format, address map, frame encoding, and HAL integration for the Kraken
> Ultimate.

**Status:** Implemented (pending hardware validation on issue #362)
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::razer::kraken`
**Author:** Nova
**Date:** 2026-10-08

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Address Maps](#3-address-maps)
4. [Memory-Access Wire Format](#4-memory-access-wire-format)
5. [Frame Encoding and Timing](#5-frame-encoding-and-timing)
6. [HAL Integration](#6-hal-integration)
7. [Testing Strategy](#7-testing-strategy)
8. [Hardware Validation](#8-hardware-validation)
9. [References](#references)

---

## 1. Overview

The Kraken 7.1 Chroma, Kraken V2, Kraken Tournament Edition, Kraken Ultimate,
and Kraken Kitty V2 predate the shared Razer report described in spec 17.
Their firmware exposes lighting as a handful of RAM registers (a custom
color, an effect bitfield, and per-effect color slots), plus an EEPROM region
holding the firmware version and serial number. The host drives them with
one request shape: "read or write N bytes at address A".

`RazerProtocolVersion::KrakenV4` in `drivers/razer/types.rs` is unrelated.
It selects transaction ID `0x60` for the newer Kraken V4 family, which does
speak the 90-byte report. Nothing in this spec applies to it.

This driver currently registers the Kraken Ultimate only, prompted by
GitHub issue #362. The protocol encoder is keyed by a `KrakenModel` enum so
that the sibling headsets on the same address map become new variants plus
descriptors, not new modules.

The implementation is a clean-room port built from the protocol knowledge
documented in OpenRazer's `razerkraken_driver` kernel module and OpenRGB's
`RazerKrakenController`. No vendor USB capture exists yet; section 8 lists
what a capture should confirm.

**Vendor ID:** `0x1532` (Razer Inc.)

---

## 2. Device Registry

| Device | PID | Address map | Hypercolor status |
|---|---|---|---|
| Kraken 7.1 (Classic) | `0x0501` | Rainie (on/off only) | Not registered |
| Kraken 7.1 (Classic alt) | `0x0506` | Rainie (on/off only) | Not registered |
| Kraken 7.1 Chroma | `0x0504` | Rainie | Not registered |
| Kraken 7.1 V2 | `0x0510` | Kylie | Not registered |
| Kraken Tournament Edition | `0x0520` | Kylie | Not registered |
| **Kraken Ultimate** | **`0x0527`** | **Kylie** | **Registered** |
| Kraken Kitty V2 (Black Edition V2) | `0x0560` | Kylie | Not registered |

"Rainie" and "Kylie" are OpenRazer's codenames for the Kraken 7.1 Chroma and
Kraken V2 register layouts. Every Kylie device shares the Ultimate's
addresses, so registering one is a `KrakenModel` variant and a descriptor
once someone can test it.

There is no firmware split on the Ultimate PID, so the descriptor carries no
firmware predicate.

---

## 3. Address Maps

### Shared RAM registers

| Address | Size | Register |
|---|---|---|
| `0x1189` | 1 | Custom color red |
| `0x118A` | 1 | Custom color green |
| `0x118B` | 1 | Custom color blue |
| `0x118C` | 1 | Custom color intensity |

### Kylie RAM registers (Kraken Ultimate)

| Address | Size | Register |
|---|---|---|
| `0x172D` | 1 | LED effect bitfield (see below) |
| `0x1741` | 4 | Static / single-color breathing: R, G, B, intensity |
| `0x1745` | 8 | Two-color breathing: two R, G, B, intensity slots |
| `0x174D` | 12 | Three-color breathing: three R, G, B, intensity slots |

### Rainie RAM registers (reference only)

| Address | Size | Register |
|---|---|---|
| `0x1008` | 1 | LED effect bitfield (logo on/off on the Classic) |
| `0x15DE` | 4 | Static / breathing color: R, G, B, intensity |

### EEPROM

| Address | Size | Contents |
|---|---|---|
| `0x0030` | 2 | Firmware version, BCD (major, minor) |
| `0x7F00` | 22 | Serial number, ASCII |

### Effect bitfield

Bit 0 is the least significant bit.

| Bit | Mask | Meaning |
|---|---|---|
| 0 | `0x01` | LED on (static) |
| 1 | `0x02` | Single-color breathing |
| 2 | `0x04` | Spectrum cycling |
| 3 | `0x08` | Sync |
| 4 | `0x10` | Two-color breathing |
| 5 | `0x20` | Three-color breathing |

OpenRazer composes these as follows: static is `0x01`, spectrum is `0x05`,
single breathing is `0x0B`, dual breathing is `0x19`, triple breathing is
`0x29`, and off is `0x00`. Custom (direct) color is `0x01` written after the
color lands in `0x1189`.

---

## 4. Memory-Access Wire Format

### Request: HID output report `0x04` (37 bytes on the wire)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x04` | Prepended by the HID transport, not part of `KrakenRequest` |
| 1 | 1 | Destination | `0x40`, `0x20`, `0x00` | RAM write, EEPROM read, RAM read |
| 2 | 1 | Length | 1..=32 | Bytes to read or write |
| 3 | 2 | Address | big-endian | High byte first |
| 5 | 32 | Arguments | | Write payload, zero-padded past `Length` |

`KrakenRequest` is the 36-byte body (offsets 1 through 36), with a
compile-time size assertion. The address field is a zerocopy
`U16<BigEndian>`.

### Response: HID input report `0x05` (33 bytes on the wire)

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Report ID | `0x05` | Memory-read result |
| 1 | 32 | Data | | Bytes read from the requested address, in order |

Reads are only issued by the post-connect diagnostic probe. The parser uses
`read_from_prefix`, so a platform that hands back a longer padded buffer
still parses, and it rejects any other report ID. That rejection matters
because the same HID collection also carries the headset's consumer-control
input reports (volume and mute).

### Color encoding

Plain RGB, one byte per channel, written as a 3-byte RAM write at `0x1189`.
The intensity register at `0x118C` is left untouched, matching both
reference implementations' direct paths. There is no checksum.

---

## 5. Frame Encoding and Timing

### Topology

One zone, `Earcups`, one LED, `DeviceTopologyHint::Point`, RGB. Both earcup
logos show the same color; there is no per-side addressing. OpenRGB models
the device the same way (a single-LED "Headset" zone).

### Init

One RAM write: effect bitfield `0x01` at `0x172D`. This switches the LED to
static-on so the custom-color registers are displayed, and it surfaces a
broken write path as a connect failure instead of a silent dark headset.

### Per frame (2 reports)

```
1. 40 03 11 89 RR GG BB 00 ... 00    custom color -> 0x1189..0x118B
2. 40 01 17 2D 01 00 ... 00          effect bitfield = static on
```

The effect write follows every color write because that is the sequence
OpenRGB's Direct mode sends on every LED update, and OpenRazer's
`matrix_effect_custom` attribute sends the same pair. Whether the firmware
needs the second write to latch the color is unconfirmed (section 8).

Both reports are built with `CommandBuffer::push_struct` into the reused
command vector, so steady-state encoding allocates nothing.

### Shutdown

Empty. Every write targets RAM, so the headset falls back to its stored
effect on the next power cycle or when Synapse reasserts control.

### Timing

OpenRazer sleeps `length x 15 ms` after the writes it treats as persistent
(static, breathing, spectrum, off), but sends custom-color writes and reads
with no delay. Hypercolor follows the custom path: no `post_delay` on any
command. The frame interval cap is 2 ms (500 fps advertised); the render
loop's 60 fps ceiling is the real cadence, which works out to at most 120
short reports per second.

### Diagnostics

`connection_diagnostics()` reads the 2-byte firmware version from EEPROM
`0x0030` with a 250 ms response budget. The backend logs a successful probe
at debug level and never fails a connect on it, so it doubles as a cheap
"the device answers on this collection" signal in user logs. A headset that
stays silent past the budget produces one warning per connect, carrying the
probe bytes and the empty reply, which is how a silent collection shows up
in a report.

---

## 6. HAL Integration

### Descriptor

```rust
TransportType::UsbHidApi {
    interface: Some(3),
    report_id: 0x04,
    report_mode: HidRawReportMode::OutputReport,
    max_report_len: 37,
    usage_page: Some(0x000C), // Consumer
    usage: Some(0x0001),      // Consumer Control
}
```

Protocol ID `razer/kraken-ultimate`, driver ID `razer`, family
`DeviceFamily::new_static("razer", "Razer")`. The descriptor helper is
`kraken_descriptor` in `drivers/razer/devices/mod.rs`, and the registration
sits next to the Seiren V3 Chroma in `devices/peripherals.rs`. Like the rest
of the Razer family, it names `UsbHidApi` directly on every platform rather
than resolving a `TransportIntent`.

### Report path

At the USB level, OpenRazer sends each request as a class `SET_REPORT`
control transfer: `bmRequestType = 0x21`, `bRequest = 0x09`,
`wValue = 0x0204` (report type Output, report ID 4), `wIndex = 3`, 37 bytes.
That is an output report, not a feature report, so the descriptor uses
`HidRawReportMode::OutputReport` and every command uses
`TransferType::Primary`. `TransferType::HidReport` would force the HIDAPI
transport onto the feature-report path and must not be used here.

Report ID ownership: `encode_hidapi_packet` prepends `0x04` because the mode
is not a `...WithReportId` variant, so `KrakenRequest` carries no report ID
field.

### Platform notes

- **Linux.** HIDAPI opens the interface 3 hidraw node and `write()` sends
  the 37-byte buffer; the kernel issues the output report. The existing
  vendor-wide `1532` hidraw udev rule grants access. If OpenRazer's
  `razerkraken` module is bound, hidraw still exists because that driver
  starts the device with `HID_CONNECT_DEFAULT`.
- **Windows.** HIDAPI opens the Consumer Control top-level collection on
  `MI_03` and `hid_write` issues `WriteFile`. The HID class driver delivers
  it over the interrupt OUT endpoint if the interface has one, and as a
  `SET_REPORT(Output)` control transfer otherwise; both reach the same
  firmware handler. Windows requires the buffer to be at least the
  collection's `OutputReportByteLength`. HIDAPI pads shorter writes and then
  reports the padded length (or 0 when `WriteFile` completes synchronously),
  and the hidapi transport's `check_output_write` accepts both. A 37-byte
  write matches the report size OpenRazer and
  OpenRGB use, so no padding is expected. Consumer Control collections are
  not opened exclusively by the OS, so sharing with Synapse's own handle is
  allowed, though the two will fight over the LED.
- **macOS.** Same HIDAPI path; untested.

### Code layout

| File | Contents |
|---|---|
| `drivers/razer/kraken.rs` | `KrakenRequest`, `KrakenResponse`, `KrakenModel`, `KrakenProtocol` |
| `drivers/razer/devices/mod.rs` | `PID_KRAKEN_ULTIMATE`, `build_kraken_ultimate_protocol`, `kraken_descriptor` |
| `drivers/razer/devices/peripherals.rs` | Descriptor registration |
| `data/drivers/vendors/razer.toml` | Compatibility matrix entry |

---

## 7. Testing Strategy

`crates/hypercolor-hal/tests/razer_kraken_tests.rs` asserts, without
hardware:

- A frame is exactly two 36-byte reports: the color write, then the effect
  write
- Color bytes land at `0x1189`, `0x118A`, `0x118B` in R, G, B order, with
  the intensity byte and padding left zero
- Through `encode_hidapi_packet`, each report becomes 37 bytes led by `0x04`
- Empty frames pad to black and oversized frames keep only the first color
- `encode_frame_into` truncates stale slots and reuses the same buffers
  across frames
- Init sends the static-on effect write; shutdown sends nothing
- The diagnostic probe reads 2 bytes at EEPROM `0x0030` with a 250 ms budget
- `parse_response` accepts input report `0x05` (including padded buffers)
  and rejects other report IDs and truncated reads
- One `Earcups` point zone, direct color, no hardware brightness, 2 ms
  frame interval
- `ProtocolDatabase::lookup(0x1532, 0x0527)` resolves to the descriptor and
  transport above

`benches/protocol_encoding.rs` carries a `razer_kraken_ultimate_1` case.

---

## 8. Hardware Validation

The protocol is well documented by two independent projects, but nobody on
the Hypercolor side has watched it on the wire. The issue #362 reporter
(Windows) should confirm:

1. **Connect and color.** With Synapse closed, the headset connects and
   follows effects. Logs at `hypercolor_core=debug,hypercolor_hal=trace`
   show the post-connect diagnostic result, the selected HIDAPI path, and
   each output report.
2. **Firmware probe.** The diagnostic either succeeds (the trace log shows
   `05 VV VV ...`) or fails. A failure is harmless, but it tells us whether
   input report `0x05` arrives on the Consumer Control collection on
   Windows.
3. **Write length.** No "short hidapi output write" or padded-write errors.
   If they appear, the collection's `OutputReportByteLength` differs from
   37 and we need the HID descriptor (a USBPcap capture of the
   `GET_DESCRIPTOR` for interface 3 or a USB tree viewer dump).
4. **Brightness.** If colors look dim after Synapse ran with a low
   brightness, the intensity register at `0x118C` is in play and the color
   write should grow to 4 bytes with intensity `0xFF`.
5. **Effect write per frame.** A USBPcap capture of Synapse's own custom or
   "Chroma Connect" traffic would show whether it repeats the `0x172D`
   write. If not, the per-frame effect write can move to init only.

---

## References

- OpenRazer, `driver/razerkraken_driver.h`: report struct
  (`razer_kraken_request_report`, 37 bytes), address map, effect bitfield
  notes
- OpenRazer, `driver/razerkraken_driver.c`: `razer_kraken_send_control_msg`
  (`SET_REPORT`, `wValue = 0x0204`, `wIndex = 3`), the custom-effect write
  pair, the firmware and serial reads, and the Kylie address assignment for
  `0x0527`
- OpenRazer, `daemon/openrazer_daemon/hardware/headsets.py`:
  `RazerKrakenUltimate` capability list
- OpenRGB, `Controllers/RazerController/RazerKrakenController/`:
  `hid_write` of the 37-byte report, Direct mode calling the custom path on
  every update
- OpenRGB, `Controllers/RazerController/RazerControllerDetect.cpp`: Kraken
  Ultimate detector on interface 3, usage page `0x0C`, usage `0x01`
- OpenRGB, `Controllers/RazerController/RazerDevices.cpp`: single-LED
  "Headset" zone for `0x0527`
- Spec 17, Razer Protocol Driver (the 90-byte report family)
- GitHub issue #362, Razer Kraken Ultimate support request
