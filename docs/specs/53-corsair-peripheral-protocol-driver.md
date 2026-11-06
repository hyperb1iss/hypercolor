# 53 -- Corsair Peripheral Protocol Driver

> Native driver specification for Corsair keyboards, mice, mousepads, headset stands, and wireless dongles. Covers Bragi, NXP/CUE, and legacy peripheral protocols with clean HAL integration, topology tables, transport safety rules, and a phased implementation plan.

**Status:** Implemented. Bragi wired RGB and the NXP/CUE keyboards, mice, mousepad,
and headset stand are registered on input-safe transports; none has been run on
hardware yet. Legacy peripherals and the K66 are held, and Bragi receivers have
routing primitives but no descriptor (section 2.6).
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::corsair::peripheral`
**Author:** Nova
**Date:** 2026-05-03
**Revised:** 2026-11-05 (NXP/CUE registration, input-safety contract, udev scoping,
receiver routing gaps)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Identification](#2-device-identification)
3. [Protocol Family Dispatch](#3-protocol-family-dispatch)
4. [Bragi Protocol](#4-bragi-protocol)
5. [NXP/CUE Protocol](#5-nxpcue-protocol)
6. [Legacy Protocol](#6-legacy-protocol)
7. [Color Encoding and Topology](#7-color-encoding-and-topology)
8. [Timing and Transport Safety](#8-timing-and-transport-safety)
9. [HAL Integration and Testing](#9-hal-integration-and-testing)
10. [Recommendation](#10-recommendation)
- [References](#references)

---

## 1. Overview

Hypercolor already has native Corsair support for iCUE LINK hubs, LCD devices, and Lighting Node style controllers. This spec extends the Corsair driver family into **USB peripherals**:

- Keyboards: K55/K60/K63/K65/K68/K70/K95/K100/Strafe families
- Mice: M55/M65/M95/Sabre/Scimitar/Harpoon/Glaive/Katar/Ironclaw/Nightsword/Dark Core families
- Mousepads and stands: Polaris, MM700, ST100
- Wireless receivers and paired subdevices

These peripherals are not downstream LINK bus devices. They enumerate as independent USB devices under Corsair VID `0x1B1C` and use a mixture of vendor HID, interrupt endpoint, and legacy control-transfer protocols.

### 1.1 Relationship to Spec 18

Spec 18 remains the parent strategy for the broad Corsair ecosystem:

| Existing Spec 18 Family | Current Hypercolor Status | This Spec |
| ----------------------- | ------------------------- | --------- |
| iCUE LINK               | Native protocol exists    | Out of scope |
| LCD devices             | Native protocol exists    | Out of scope |
| Lighting Node           | Native protocol exists    | Out of scope |
| Commander Core / XT     | Researched                | Out of scope |
| Peripheral V2           | Deferred                  | **In scope** |
| Legacy Peripheral       | Deferred                  | **In scope** |

This spec should not replace any existing `corsair::link`, `corsair::lcd`, or `corsair::lighting_node` code. It adds a sibling module:

```text
crates/hypercolor-hal/src/drivers/corsair/
  peripheral/
    mod.rs
    devices.rs
    bragi.rs
    nxp.rs
    legacy.rs
    topology.rs
    types.rs
```

### 1.2 Clean-Room Rules

Use protocol observations, USB IDs, packet traces, public issue reports, and hardware testing as research input. Implement all Rust code in Hypercolor style:

- Do not copy C/C++ packet-building code.
- Encode wire formats as typed Rust structures or small explicit builders.
- Keep descriptor data declarative.
- Put tests in `crates/hypercolor-hal/tests/`, not inline `#[cfg(test)]` blocks.
- Treat third-party implementations as research input only; no source translation.

### 1.3 Driver Goals

| Goal | Requirement |
| ---- | ----------- |
| Direct RGB | Stream per-zone or per-key colors through `Protocol::encode_frame()` |
| Safe input devices | Avoid breaking keyboard/mouse HID input while controlling lighting |
| Robust topology | Report per-model LED counts and zone hints accurately |
| Explicit variants | Use protocol family, packet size, endpoint quirks, and topology as descriptor data |
| Graceful fallback | Unknown or unsafe variants should register as `researched`, not `supported` |
| Future control surfaces | Leave room for DPI, battery, poll-rate, brightness, pairing, and onboard profile controls |

---

## 2. Device Identification

### 2.1 Vendor

| Field | Value |
| ----- | ----- |
| Vendor | Corsair |
| VID | `0x1B1C` |
| Driver family | `corsair` |
| New module | `corsair::peripheral` |

### 2.2 Protocol Families

| Protocol | Packet Size | Transport Pattern | Products | Initial Priority |
| -------- | ----------- | ----------------- | -------- | ---------------- |
| Bragi | 64, 128, or 1024 bytes | Vendor HID output/input reports with handle resources | Modern keyboards, mice, wireless dongles, MM700 | **Phase 1** |
| NXP/CUE | 64 bytes | HID interrupt reports or HID control reports depending on firmware | Older RGB keyboards, mice, mousepads, ST100 | Phase 2 |
| Legacy | control transfers | USB vendor/HID control commands | K65/K70/K90/K95 legacy, M95 | Phase 4 |

### 2.3 Bragi Device Matrix

Bragi devices should use the same protocol core with per-device descriptor data:

| PID | Model | Type | Packet | LEDs | Notes |
| --- | ----- | ---- | ------ | ---- | ----- |
| `0x1BA4` | K55 Pro | keyboard | 64B | 6 | Zoned keyboard |
| `0x1BA1` | K55 Pro XT | keyboard | 64B | 137 | Per-key keyboard |
| `0x1B62` | K57 Wireless Dongle | dongle | 64B | 0 | Parent receiver |
| `0x1B6E` | K57 Wireless USB | keyboard | 64B | 137 | Wireless-capable child/device |
| `0x1BA0` | K60 Pro RGB | keyboard | 64B | 123 | Per-key |
| `0x1BAD` | K60 Pro RGB Low Profile | keyboard | 64B | 123 | Per-key |
| `0x1B8D` | K60 Pro RGB SE | keyboard | 64B | 123 | Per-key |
| `0x1B83` | K60 Pro Mono | keyboard | 64B | 123 | Monochrome lighting resource |
| `0x1BC7` | K60 Pro TKL Black | keyboard | 64B | 123 | Per-key |
| `0x1BED` | K60 Pro TKL White | keyboard | 64B | 123 | Per-key |
| `0x1BAF` | K65 Mini | keyboard | 1024B | 123 | Bragi jumbo |
| `0x1B73` | K70 TKL | keyboard | 1024B | 193 | Bragi jumbo |
| `0x1BB9` | K70 TKL Champion Optical | keyboard | 1024B | 193 | Bragi jumbo |
| `0x1BC4` | K70 RGB Pro | keyboard | 1024B | 193 | Bragi jumbo |
| `0x1BB3` | K70 RGB Pro V2 | keyboard | 1024B | 193 | Bragi jumbo |
| `0x1BD4` | K70 Pro Optical | keyboard | 1024B | 193 | Bragi jumbo |
| `0x2B0A` | K70 Core RGB | keyboard | 64B | 123 | Per-key |
| `0x1BFF` | K70 Core RGB variant 2 | keyboard | 64B | 123 | Per-key |
| `0x1BFD` | K70 Core RGB variant 3 | keyboard | 64B | 123 | Per-key |
| `0x2B01` | K70 Core RGB TKL | keyboard | 64B | 102 | Per-key |
| `0x1B89` | K95 Platinum XT | keyboard | 64B | 156 | Top-bar variant |
| `0x1B7C` | K100 RGB Optical V1 | keyboard | 1024B | 193 | Bragi jumbo, alternate RGB payload |
| `0x1B7D` | K100 Mechanical | keyboard | 1024B | 193 | Bragi jumbo, alternate RGB payload |
| `0x1BC5` | K100 RGB Optical V2 | keyboard | 1024B | 193 | Bragi jumbo, alternate RGB payload |
| `0x1B70` | M55 RGB Pro | mouse | 64B | 2 | Software packet lacks wheel input |
| `0x1B4B` | Dark Core RGB SE wired | mouse | 64B | 12 | Extended mouse topology |
| `0x1B4C` | Ironclaw RGB Wireless wired | mouse | 64B | 6 | Wireless-capable |
| `0x1B66` | Ironclaw RGB Wireless Dongle | dongle | 64B | 0 | Parent receiver |
| `0x1B5E` | Harpoon Wireless wired | mouse | 64B | 2 | Endpoint offset quirk |
| `0x1B65` | Harpoon Wireless Dongle | dongle | 64B | 0 | Parent receiver |
| `0x1B93` | Katar Pro | mouse | 64B | 2 | Two-zone mouse |
| `0x1BBA` | Katar Pro V2 | mouse | 64B | 2 | Two-zone mouse |
| `0x1BAC` | Katar Pro XT | mouse | 64B | 2 | Two-zone mouse |
| `0x1B9E` | M65 RGB Ultra Wired | mouse | 64B | 3 | Three-zone mouse |
| `0x1BB5` | M65 RGB Ultra Wireless wired | mouse | 64B | 2 | Wireless-capable |
| `0x1BF0` | M75 Gaming Mouse | mouse | 64B | 2 | Two-zone mouse |
| `0x1B80` | Dark Core RGB Pro | mouse | 64B | 12 | Extended mouse topology |
| `0x1B81` | Dark Core RGB Pro Wireless | dongle | 64B | 0 | Parent receiver |
| `0x1B7E` | Dark Core RGB Pro SE | mouse | 64B | 12 | Extended mouse topology |
| `0x1B7F` | Dark Core RGB Pro SE Wireless | dongle | 64B | 0 | Parent receiver |
| `0x1BE3` | Scimitar Elite Bragi | mouse | 128B | 5 | Large packet, short input report |
| `0x1B9B` | MM700 | mousepad | 64B | 3 | Endpoint offset quirk |
| `0x1BC9` | MM700 3XL | mousepad | 64B | 3 | Endpoint offset quirk |
| `0x1BA6` | Generic Bragi Dongle | dongle | 64B | 0 | Multi-device receiver |

Descriptors for dongles must not report RGB zones directly. The paired child devices own LED topology and color state.

### 2.4 NXP/CUE Device Matrix

NXP/CUE devices share a 64-byte command grammar but differ sharply in topology and color depth.

| PID | Model | Type | Color Path | Notes |
| --- | ----- | ---- | ---------- | ----- |
| `0x1B3D` | K55 | keyboard | zoned | 3-zone packet plus winlock quirk |
| `0x1B40` | K63 non-RGB | keyboard | monochrome | Single-color lighting |
| `0x1B45` | K63 Wireless | keyboard | limited wireless | Hardware animation preferred |
| `0x1B50` | K63 Wireless variant 2 | keyboard/dongle | limited wireless | Experimental |
| `0x1B8C` | K63 Wireless variant 3 | keyboard | limited wireless | Experimental |
| `0x1B8F` | K63 Wireless variant 4 | keyboard/dongle | limited wireless | Experimental |
| `0x1B17` | K65 RGB | keyboard | 512-color | 9-bit color packing |
| `0x1B37` | K65 Lux | keyboard | full-range | 24-bit planar |
| `0x1B39` | K65 Rapidfire | keyboard | full-range | Unclean-exit quirk |
| `0x1B41` | K66 | keyboard | no lights | CUE protocol without backlight |
| `0x1B4F` | K68 RGB | keyboard | full-range | V3 endpoint override |
| `0x1B3F` | K68 non-RGB | keyboard | monochrome | Winlock quirk |
| `0x1B13` | K70 RGB | keyboard | 512-color | 9-bit color packing |
| `0x1B33` | K70 Lux RGB | keyboard | full-range | 24-bit planar |
| `0x1B36` | K70 Lux non-RGB | keyboard | monochrome | 24-bit protocol, red channel only |
| `0x1B38` | K70 Rapidfire | keyboard | full-range | Unclean-exit quirk |
| `0x1B3A` | K70 Rapidfire non-RGB | keyboard | monochrome | Unclean-exit quirk |
| `0x1B49` | K70 MK.2 | keyboard | full-range | File-based hardware profile support |
| `0x1B6B` | K70 MK.2 SE | keyboard | full-range | File-based hardware profile support |
| `0x1B55` | K70 MK.2 Low Profile | keyboard | full-range | File-based hardware profile support |
| `0x1B11` | K95 RGB | keyboard | 512-color | Unclean-exit quirk |
| `0x1B2D` | K95 Platinum | keyboard | full-range | File-based hardware profile support |
| `0x1B20` | Strafe RGB | keyboard | full-range | Sidelight quirk |
| `0x1B15` | Strafe non-RGB | keyboard | 512-color/monochrome | 3-bit lighting |
| `0x1B44` | Strafe non-RGB variant 2 | keyboard | full-range/monochrome | 8-bit lighting |
| `0x1B48` | Strafe MK.2 | keyboard | full-range | V3 endpoint override |
| `0x1B12` | M65 | mouse | mouse zones | 2-zone hardware save |
| `0x1B2E` | M65 Pro | mouse | mouse zones | 2-zone hardware save |
| `0x1B5A` | M65 RGB Elite | mouse | mouse zones | File-based hardware profile support |
| `0x1B34` | Glaive | mouse | mouse zones | Only front/back/side software zones |
| `0x1B74` | Glaive Pro | mouse | mouse + DPI packet | DPI RGB in DPI packet |
| `0x1B14` | Sabre Optical | mouse | mouse zones | 3-zone hardware save |
| `0x1B19` | Sabre Laser | mouse | mouse zones | 3-zone hardware save |
| `0x1B2F` | Sabre variant | mouse | mouse zones | 3-zone hardware save |
| `0x1B32` | Sabre O2 | mouse | mouse zones | 3-zone hardware save |
| `0x1B1E` | Scimitar | mouse | mouse zones | 4-zone hardware save |
| `0x1B3E` | Scimitar Pro | mouse | mouse zones | File-based hardware profile support |
| `0x1B8B` | Scimitar Elite | mouse | mouse zones | V3 endpoint override |
| `0x1B3C` | Harpoon | mouse | mouse zones | V2 endpoint override |
| `0x1B75` | Harpoon Pro | mouse | mouse zones | Adjustable poll rate |
| `0x1B22` | Katar | mouse | mouse zones | Older NXP mouse |
| `0x1B5D` | Ironclaw | mouse | mouse zones | V3 endpoint override |
| `0x1B5C` | Nightsword | mouse | mouse zones | V3 endpoint override |
| `0x1B35` | Dark Core | mouse | hardware animation | NXP wireless family |
| `0x1B64` | Dark Core Wireless | dongle | hardware animation | NXP wireless family |
| `0x1B51` | Dark Core SE Wireless | dongle | hardware animation | NXP wireless family |
| `0x1B3B` | Polaris | mousepad | mousepad zones | Single endpoint |
| `0x0A34` | ST100 | headset stand | mousepad-style zones | Single endpoint |

### 2.5 Legacy Device Matrix

Legacy devices should be a late-phase compatibility target.

| PID | Model | Type | Lighting Support | Notes |
| --- | ----- | ---- | ---------------- | ----- |
| `0x1B07` | K65 Legacy | keyboard | hardware mode only | Vendor control commands |
| `0x1B09` | K70 Legacy | keyboard | hardware mode only | Vendor control commands |
| `0x1B02` | K90 Legacy | keyboard | hardware mode only | Behaves like legacy K95 |
| `0x1B08` | K95 Legacy | keyboard | hardware mode only | Mode and brightness commands |
| `0x1B06` | M95 | mouse | backlight on/off | Legacy mouse RGB path |

### 2.6 Registration Decisions

A Corsair peripheral registers only when its lighting is reachable through a
transport that cannot detach the kernel input driver (section 8.7). That is the
only gate: a registered device that nobody has run on hardware is still
registered, and carries the experimental label once the HAL stability field
exists.

| PID | Model | Decision | Transport | Reason |
| --- | ----- | -------- | --------- | ------ |
| Bragi wired set (section 9.5) | 35 keyboards, mice, mousepads | Registered | HIDAPI, interface 1, page `0xFF42`, output report with the magic byte first | Vendor-only lighting interface |
| `0x1B3D` | K55 | Registered | HIDAPI, interface 1, page `0xFFC2`, usage `0x0004`, output report ID 0 | Command collection is vendor-only on interface 1 |
| `0x1B17` | K65 RGB | Registered | same as K55 | same |
| `0x1B37` | K65 Lux RGB | Registered | same as K55 | same |
| `0x1B13` | K70 RGB | Registered | same as K55 | same; layout confirmed on fw 3.08 |
| `0x1B33` | K70 Lux RGB | Registered | same as K55 | same |
| `0x1B49` | K70 MK.2 | Registered | same as K55 | same; layout confirmed on fw 3.24 |
| `0x1B11` | K95 RGB | Registered | same as K55 | same; fw 2.05 has no hidraw path (section 5.1) |
| `0x1B2D` | K95 Platinum | Registered | same as K55 | same; layout confirmed on fw 3.21 |
| `0x1B20` | Strafe RGB | Registered | same as K55 | same; fw 2.05 has no hidraw path (section 5.1) |
| `0x1B12` | M65 | Registered | same as K55 | same; fw 2.04 has no hidraw path (section 5.1) |
| `0x1B1E` | Scimitar | Registered | same as K55 | same |
| `0x1B3B` | Polaris | Registered | HIDAPI, interface 0, page `0xFFC2`, usage `0x0001`, output report ID 0 | Command collection on interface 0; interface 1 is a keyboard collection and is never opened |
| `0x0A34` | ST100 | Registered | HIDAPI, interface 0, page `0xFFC2`, output report ID 0 | Single vendor-only interface |
| `0x1B41` | K66 | Held | none | No backlight; nothing to drive |
| `0x1B07` | K65 Legacy | Held | would need USB vendor control | Section 6.3 |
| `0x1B09` | K70 Legacy | Held | would need USB vendor control | Section 6.3 |
| `0x1B02` | K90 Legacy | Held | would need USB vendor control | Section 6.3 |
| `0x1B08` | K95 Legacy | Held | would need USB vendor control | Section 6.3 |
| `0x1B06` | M95 | Held | would need USB vendor control | Section 6.3 |
| `0x1B62`, `0x1B66`, `0x1B65`, `0x1B81`, `0x1B7F`, `0x1BA6` | Bragi receivers | Held | none | Child routing is underspecified (section 4.20.1) |

---

## 3. Protocol Family Dispatch

### 3.1 Dispatch Rule

Descriptor construction should make protocol selection explicit:

```rust
enum CorsairPeripheralProtocolKind {
    Bragi(BragiDescriptor),
    Nxp(NxpDescriptor),
    Legacy(LegacyDescriptor),
}
```

Do not infer protocol behavior from loose product-name matching at runtime. Register each PID with:

- Protocol kind
- Packet size
- Product class
- LED topology
- Endpoint/report quirk profile
- Monochrome/full-color flag
- Wireless/dongle role
- Optional firmware predicate

### 3.2 Product Class

```rust
enum CorsairPeripheralClass {
    Keyboard,
    Mouse,
    Mousepad,
    HeadsetStand,
    Dongle,
}
```

Class determines default topology, frame slicing, and optional control surfaces.

### 3.3 Descriptor Shape

```rust
struct CorsairPeripheralDescriptor {
    vid: u16,
    pid: u16,
    name: &'static str,
    class: CorsairPeripheralClass,
    protocol: CorsairPeripheralProtocolKind,
    topology: CorsairPeripheralTopology,
    quirks: CorsairPeripheralQuirks,
}
```

Recommended quirk flags:

| Quirk | Meaning |
| ----- | ------- |
| `monochrome` | RGB protocol exists, but only one channel matters |
| `no_lights` | Device uses CUE commands but has no direct lighting |
| `bragi_jumbo` | 1024-byte Bragi output reports |
| `bragi_large` | 128-byte Bragi output reports |
| `bragi_short_input_report` | Short input reports for non-lighting HID data |
| `bragi_alt_lighting_fallback` | Try alternate lighting resource if standard lighting resource fails |
| `bragi_endpoint_k57` | Out endpoint `0x02`, in endpoint `0x82` |
| `bragi_endpoint_modern_keyboard` | Out endpoint `0x01`, in endpoint `0x82` |
| `bragi_endpoint_mm700` | Input endpoint offset starts at `0x84` |
| `nxp_v2_override` | Uses modern endpoint configuration even with low firmware value |
| `nxp_v3_override` | Uses newest endpoint configuration even with low firmware value |
| `nxp_single_endpoint` | Device has one non-HID input endpoint |
| `nxp_512_color` | Uses 9-bit packed color instead of 24-bit color |
| `nxp_dpi_rgb_in_dpi_packet` | DPI LEDs travel with DPI configuration |
| `requires_unclean_exit` | Do not attempt normal HID handover on disconnect |
| `wireless` | Device can run wirelessly |
| `dongle` | Device is a receiver and may enumerate children |

### 3.4 Registration Status

Support labels in `data/drivers/vendors/corsair.toml`:

| Status | Meaning |
| ------ | ------- |
| `supported` | Registered: packet encoder implemented and tested, transport input-safe (section 8.7) |
| `researched` | PID and protocol family known, but not registered; the entry's notes say why |

Experimental is a label, not a status. A registered device nobody has run on
hardware is enabled by default like any other; its protocol carries the
experimental stability label and its database entry records
`verification = "unverified"` once those fields exist. No config flag gates it.

---

## 4. Bragi Protocol

Bragi is the modern Corsair peripheral protocol used by current keyboards, mice, wireless receivers, and the MM700 mousepad. It is built around:

- A magic byte (`0x08`)
- Short command opcodes
- Property get/set commands
- Resource handles
- Chunked writes to opened handles
- Optional wireless child routing

### 4.1 Packet Sizes

| Packet Class | Size | Devices |
| ------------ | ---- | ------- |
| Standard | 64 bytes | Most Bragi mice, K55/K57/K60/K70 Core, K95 XT, MM700 |
| Large | 128 bytes | Scimitar Elite Bragi |
| Jumbo | 1024 bytes | K100, K65 Mini, K70 TKL, K70 Pro |

The protocol payload builders should accept `packet_size` from the descriptor. Do not hardcode 64 or 1024 in the protocol core.

### 4.2 Transport

Bragi uses output reports and input responses. Hypercolor should prefer a transport that does **not** detach the kernel input driver:

```rust
TransportType::UsbHidApi {
    interface: Some(interface),
    report_id: 0x00,
    report_mode: HidRawReportMode::OutputReportWithReportId,
    max_report_len: packet_size,
    usage_page: Some(0xFF42), // when available
    usage: None,
}
```

If a device exposes separate lighting and input collections, bind only the vendor lighting collection. If the OS cannot identify the collection safely, leave the descriptor researched until a safe transport path exists.

### 4.3 Endpoint Quirks

Some platforms expose raw interrupt endpoints rather than logical HID collections. If a future transport opens endpoints directly, use this routing table:

| Device Set | OUT EP | IN EP | Notes |
| ---------- | ------ | ----- | ----- |
| Default Bragi | `0x04` | `0x84` | Common case |
| K57 dongle | `0x02` | `0x82` | Receiver |
| K57 USB, K55 Pro, K55 Pro XT, K100, K65 Mini, K70 TKL, K70 Pro | `0x01` | `0x82` | Modern keyboard path |
| Harpoon Wireless | default | default | Input endpoint scan offset starts at `0x82` |
| MM700 | default | default | Input endpoint scan offset starts at `0x84` |

Endpoint quirks should live in descriptor data, not `match` statements spread through protocol logic.

### 4.4 Command Vocabulary

| Name | Byte | Direction | Purpose |
| ---- | ---- | --------- | ------- |
| `SET` | `0x01` | host -> device | Set scalar property |
| `GET` | `0x02` | host -> device | Read scalar property |
| `CLOSE_HANDLE` | `0x05` | host -> device | Close a resource handle |
| `WRITE_DATA` | `0x06` | host -> device | First chunk of a resource write |
| `CONTINUE_WRITE` | `0x07` | host -> device | Additional chunks of a resource write |
| `READ_DATA` | `0x08` | host -> device | Read bytes from an opened handle |
| `PROBE_HANDLE` | `0x09` | host -> device | Query opened handle length |
| `OPEN_HANDLE` | `0x0D` | host -> device | Open a resource handle |
| `POLL` | `0x12` | host -> device | Keep software-control session alive |

Every Bragi request starts with `0x08`.

### 4.5 Properties

| Property | Byte | Access | Meaning |
| -------- | ---- | ------ | ------- |
| `POLLRATE` | `0x01` | get/set | Poll rate selector |
| `BRIGHTNESS` | `0x02` | get/set | Fine hardware brightness |
| `MODE` | `0x03` | get/set | Hardware/software mode |
| `ANGLE_SNAP` | `0x07` | get/set | Mouse angle snap |
| `BATTERY_LEVEL` | `0x0F` | get | Battery level, tenths of percent |
| `BATTERY_STATUS` | `0x10` | get | Charging/discharging/charged |
| `VID` | `0x11` | get | Child device vendor ID |
| `PID` | `0x12` | get | Child device product ID |
| `APP_VER` | `0x13` | get | Application firmware version |
| `BLD_VER` | `0x14` | get | Bootloader/build firmware version |
| `RADIO_APP_VER` | `0x15` | get | Wireless radio app version |
| `RADIO_BLD_VER` | `0x16` | get | Wireless radio build version |
| `DPI_INDEX` | `0x1E` | get/set | Current DPI stage |
| `DPI_MASK` | `0x1F` | get/set | Enabled DPI stages |
| `DPI_X` | `0x21` | get/set | DPI X value |
| `DPI_Y` | `0x22` | get/set | DPI Y value |
| `DPI0_COLOR` | `0x2F` | get/set | DPI stage color |
| `DPI1_COLOR` | `0x30` | get/set | DPI stage color |
| `DPI2_COLOR` | `0x31` | get/set | DPI stage color |
| `HWLAYOUT` | `0x41` | get | Keyboard physical layout |
| `BRIGHTNESS_COARSE` | `0x44` | get/set | Coarse hardware brightness |
| `SUBDEVICE_BITFIELD` | `0x36` | get | Wireless child presence bitmap |

### 4.6 Modes and Poll Rates

| Name | Byte | Meaning |
| ---- | ---- | ------- |
| `MODE_HARDWARE` | `0x01` | Device uses onboard hardware behavior |
| `MODE_SOFTWARE` | `0x02` | Host owns lighting/input vendor events |
| `POLLRATE_8MS` | `0x01` | 125 Hz |
| `POLLRATE_4MS` | `0x02` | 250 Hz |
| `POLLRATE_2MS` | `0x03` | 500 Hz |
| `POLLRATE_1MS` | `0x04` | 1000 Hz |

Hypercolor should only expose poll rate control after a dynamic control-surface pass. RGB support does not require changing poll rate.

### 4.7 Property Get Packet

| Offset | Size | Field | Value | Description |
| ------ | ---- | ----- | ----- | ----------- |
| 0 | 1 | Magic | `0x08` | Bragi magic |
| 1 | 1 | Command | `0x02` | `GET` |
| 2 | 1 | Property | variable | Property ID |
| 3 | 1 | Reserved | `0x00` | Always zero |
| 4..N | rest | Padding | `0x00` | Zero-fill to packet size |

Response:

| Offset | Size | Field | Description |
| ------ | ---- | ----- | ----------- |
| 0 | 1 | Magic/echo | Device-specific echo |
| 1 | 1 | Command/echo | Device-specific echo |
| 2 | 1 | Error | `0x00` on success, `0x05` not supported |
| 3..5 | 3 | Value | Little-endian 24-bit scalar |

Values are read as:

```text
value = response[3] | (response[4] << 8) | (response[5] << 16)
```

### 4.8 Property Set Packet

| Offset | Size | Field | Value | Description |
| ------ | ---- | ----- | ----- | ----------- |
| 0 | 1 | Magic | `0x08` | Bragi magic |
| 1 | 1 | Command | `0x01` | `SET` |
| 2 | 1 | Property | variable | Property ID |
| 3 | 1 | Reserved | `0x00` | Always zero |
| 4 | 1 | Value LSB | variable | 16-bit value low byte |
| 5 | 1 | Value MSB | variable | 16-bit value high byte |
| 6..N | rest | Padding | `0x00` | Zero-fill to packet size |

Response error byte is at offset `2`.

### 4.9 Resources and Handles

| Resource | Value | Purpose |
| -------- | ----- | ------- |
| `LIGHTING` | `0x0001` | Standard RGB lighting data |
| `LIGHTING_MONOCHROME` | `0x0010` | Single-channel lighting |
| `ALT_LIGHTING` | `0x0022` | Alternate RGB data format |
| `LIGHTING_EXTRA` | `0x002E` | Secondary lighting resource for some keyboards |
| `PAIRING_ID` | `0x0005` | Wireless pairing ID |
| `ENCRYPTION_KEY` | `0x0006` | Wireless pairing key |

| Handle | Value | Purpose |
| ------ | ----- | ------- |
| `LIGHTING_HANDLE` | `0x00` | Main lighting writes |
| `GENERIC_HANDLE` | `0x01` | Short-lived reads/writes such as pairing ID |
| `SECOND_LIGHTING_HANDLE` | `0x02` | Optional secondary lighting resource |

`GENERIC_HANDLE` must be closed immediately after use. Do not leave it open.

### 4.10 Open Handle Packet

| Offset | Size | Field | Value | Description |
| ------ | ---- | ----- | ----- | ----------- |
| 0 | 1 | Magic | `0x08` | Bragi magic |
| 1 | 1 | Command | `0x0D` | `OPEN_HANDLE` |
| 2 | 1 | Handle | variable | Target handle |
| 3 | 1 | Resource LSB | variable | Resource low byte |
| 4 | 1 | Resource MSB | variable | Resource high byte |
| 5 | 1 | Reserved | `0x00` | Always zero |
| 6..N | rest | Padding | `0x00` | Zero-fill |

If the response error is `0x03`, close the handle and retry once. Other nonzero errors are non-fatal during alternate-lighting probing but fatal during ordinary activation.

### 4.11 Close Handle Packet

| Offset | Size | Field | Value | Description |
| ------ | ---- | ----- | ----- | ----------- |
| 0 | 1 | Magic | `0x08` | Bragi magic |
| 1 | 1 | Command | `0x05` | `CLOSE_HANDLE` |
| 2 | 1 | Count | `0x01` | One handle |
| 3 | 1 | Handle | variable | Target handle |
| 4 | 1 | Reserved | `0x00` | Always zero |
| 5..N | rest | Padding | `0x00` | Zero-fill |

Close failures should be logged at warning level and should not panic.

### 4.12 Write Data Packet

First packet:

| Offset | Size | Field | Value | Description |
| ------ | ---- | ----- | ----- | ----------- |
| 0 | 1 | Magic | `0x08` | Bragi magic |
| 1 | 1 | Command | `0x06` | `WRITE_DATA` |
| 2 | 1 | Handle | variable | Target handle |
| 3..6 | 4 | Length | little-endian u32 | Payload byte length |
| 7..N | variable | Payload | bytes | First chunk |

Continue packet:

| Offset | Size | Field | Value | Description |
| ------ | ---- | ----- | ----- | ----------- |
| 0 | 1 | Magic | `0x08` | Bragi magic |
| 1 | 1 | Command | `0x07` | `CONTINUE_WRITE` |
| 2 | 1 | Handle | variable | Target handle |
| 3..N | variable | Payload | bytes | Next chunk |

Chunking formula:

```text
first_payload_capacity = packet_size - 7
continue_payload_capacity = packet_size - 3
```

The encoder must split a logical payload over one first packet plus zero or more continue packets. Each physical packet must be padded to `packet_size`.

### 4.13 Read Data Packets

Probe handle:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Magic | `0x08` |
| 1 | 1 | Command | `0x09` |
| 2 | 1 | Handle | variable |
| 3 | 1 | Reserved | `0x00` |

The response stores length as little-endian u32 at `response[5..9]`.

Read handle:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Magic | `0x08` |
| 1 | 1 | Command | `0x08` |
| 2 | 1 | Handle | variable |
| 3 | 1 | Reserved | `0x00` |

Each response carries payload beginning at offset `3`. Payload per response is `packet_size - 3`.

### 4.14 Init Sequence

Recommended Bragi init sequence:

1. Query `MODE`.
2. If mode is hardware, temporarily set `MODE_SOFTWARE` so properties and handles can be read.
3. Query firmware properties: `APP_VER`, `BLD_VER`, `RADIO_APP_VER`, `RADIO_BLD_VER`.
4. Query `POLLRATE` and `MAX_POLLRATE`.
5. Query brightness support: try `BRIGHTNESS`, then `BRIGHTNESS_COARSE`.
6. For wireless-capable devices, read pairing ID through `GENERIC_HANDLE`.
7. For keyboards, query `HWLAYOUT`.
8. Return to `MODE_HARDWARE`.

The protocol should not enter software lighting mode until the backend connects and starts streaming frames.

### 4.15 Activation Sequence

To activate direct RGB:

1. Set `MODE_SOFTWARE`.
2. Start Bragi poll/keepalive schedule.
3. Open `LIGHTING_HANDLE` with resource:
   - `LIGHTING_MONOCHROME` if descriptor is monochrome
   - `LIGHTING` otherwise
4. If opening `LIGHTING` returns unsupported on a device with `bragi_alt_lighting_fallback`, retry with `ALT_LIGHTING`.
5. If alternate lighting succeeds, use alternate RGB encoding for that session.
6. If alternate lighting is used, open `SECOND_LIGHTING_HANDLE` with `LIGHTING_EXTRA`.

To deactivate:

1. Close `LIGHTING_HANDLE`.
2. Close `SECOND_LIGHTING_HANDLE` if opened.
3. Set `MODE_HARDWARE`.

### 4.16 Keepalive

Bragi software mode should send a poll packet roughly every 50 seconds while active:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Magic | `0x08` |
| 1 | 1 | Command | `0x12` |
| 2..N | rest | Padding | `0x00` |

Expected response command byte should echo `0x12`. Log mismatches but do not disconnect unless repeated failures exceed the backend retry policy.

### 4.17 Standard RGB Encoding

Standard Bragi RGB payload is component-planar:

```text
payload[0..zones]             = red values
payload[zones..zones*2]       = green values
payload[zones*2..zones*3]     = blue values
```

For monochrome devices:

```text
payload[0..zones] = intensity values
```

Recommended intensity source for monochrome:

```text
intensity = max(r, g, b)
```

Standard write:

| Step | Operation |
| ---- | --------- |
| 1 | Normalize colors to exact zone count |
| 2 | Build planar payload |
| 3 | Write payload to `LIGHTING_HANDLE` |
| 4 | If all LEDs transition between all-black and non-black, update hardware brightness if supported |

### 4.18 Alternate RGB Encoding

Alternate Bragi RGB payload is interleaved after a two-byte header:

| Payload Offset | Size | Field | Value |
| -------------- | ---- | ----- | ----- |
| 0 | 1 | Header | `0x12` |
| 1 | 1 | Reserved | `0x00` |
| 2.. | `zones * 3` | RGB triples | `R, G, B` per LED |

The total payload length is:

```text
2 + zones * 3
```

Use this only after standard `LIGHTING` resource probing fails with an unsupported-like response and `ALT_LIGHTING` opens successfully.

### 4.19 Brightness-Off Optimization

When a frame changes from any nonzero LED to all black:

- If fine brightness exists, set `BRIGHTNESS` to `0`.
- Else if coarse brightness exists, set `BRIGHTNESS_COARSE` to `0`.

When a frame changes from all black to any nonzero LED:

- If fine brightness exists, set `BRIGHTNESS` to `1000`.
- Else if coarse brightness exists, set `BRIGHTNESS_COARSE` to `3`.

Keep this optimization below the RGB write in the command list so lights turn off quickly. Turning lights back on may remain slower on hardware-controlled brightness devices.

### 4.20 Wireless Dongles and Children

Bragi dongles report child presence through `SUBDEVICE_BITFIELD`.

Presence bits:

```text
bit 1..7 = paired child slot present
```

Child discovery sequence:

1. Query parent `SUBDEVICE_BITFIELD`.
2. For each set bit, create a child device handle in HAL state.
3. Route child Bragi property requests through parent transport.
4. OR the child slot ID into the request magic byte before writing through parent.
5. Query child `VID` and `PID`.
6. Match child PID against normal Bragi descriptors.
7. Register child as a logical device with parent relation metadata.

Child disconnect sequence:

1. Re-query bitfield.
2. For each previously registered child whose bit is now clear, mark disconnected.
3. Close child handles and remove child from parent state.

Initial implementation may skip child registration and treat dongles as `researched`. Do not expose a dongle as an RGB device.

#### 4.20.1 Gaps That Block Receiver Registration

The sequence above names the steps but leaves out facts an implementation needs
before a receiver can be registered without guessing. Until a capture or a
hardware session answers them, every receiver stays `researched`.

| Gap | Why it blocks |
| --- | ------------- |
| Reply to a query routed to an empty slot | Hypercolor computes a device's whole init sequence before any reply arrives, so child queries cannot wait on the bitfield. Querying all seven slots needs to know whether an empty slot answers with an error byte, answers nothing (a timeout fails the connect), or answers garbage. |
| Reply framing for routed requests | Section 4.7 calls offsets 0 and 1 a device-specific echo and does not echo the property ID, so a reply cannot be matched to the routed query that caused it. |
| Presence changes | Wireless children sleep and wake. The spec says to re-query the bitfield but not when, whether the receiver announces changes unprompted, or what a write to a sleeping child returns. A write error there would cycle the receiver through disconnect and reconnect. |
| Receiver-side session | Whether the receiver itself must enter `MODE_SOFTWARE` and receive the keepalive, or only the child. |
| Child packet size | Whether a child whose wired descriptor uses 128- or 1024-byte reports is limited to the receiver's 64-byte report when routed. |
| Child identity | Section 2.3 lists receiver children (K57 `0x1B6E`, Dark Core RGB Pro `0x1B80`) with no wired descriptor to match, and "register child as a logical device" has no HAL contract: one USB device maps to one Hypercolor device today, with no parent relation or stable child fingerprint. |

#### 4.20.2 Implemented Primitives

What the section does specify is implemented and tested, with no descriptor
using it yet:

- `BragiProperty::{VendorId, ProductId, SubdeviceBitfield}` (`0x11`, `0x12`, `0x36`).
- `BragiChildSlot`: slots 1 through 7, `magic()` returns `0x08 | slot`, and
  `present_in(bitfield)` yields the slots named by bits 1 through 7, ignoring
  bit 0 and every bit above 7.
- `BragiDeviceConfig::routed_to(slot)`: every packet the protocol emits (init,
  frame, brightness, keepalive, shutdown) carries the routed magic byte, and
  nothing else in the packet changes.

### 4.21 Pairing and Battery

Battery:

| Property | Decode |
| -------- | ------ |
| `BATTERY_LEVEL` | `level = value / 10` |
| `BATTERY_STATUS` | enum: unknown, charging, discharging, charged |

Pairing:

- Pairing ID length: 8 bytes
- Encryption key length: 16 bytes
- Pairing writes use `GENERIC_HANDLE`
- Pairing ID must be written before encryption key

Pairing controls are out of scope for initial RGB support. Model them later as dynamic driver control surfaces.

---

## 5. NXP/CUE Protocol

NXP/CUE is the older 64-byte Corsair peripheral protocol. It covers many RGB keyboards, mice, mousepads, and the ST100 headset stand.

### 5.1 Transport

Hypercolor drives NXP lighting through the vendor command collection with
HIDAPI, which is hidraw on Linux. Section 8.7 makes that the only allowed path.

Observed interface layouts:

| Layout | Seen on | Interface 0 | Interface 1 | Interface 2 |
| ------ | ------- | ----------- | ----------- | ----------- |
| Two-interface | K70 RGB fw 3.08, K70 MK.2 fw 3.24, K95 Platinum fw 3.21 | Boot keyboard plus NKRO, consumer, pointer, and an input-only vendor collection (page `0xFFC2`, usage `0x0003`, report ID 14) | Command collection only (page `0xFFC2`, usage `0x0004`): 64-byte input, output, and feature reports with no report ID; interrupt IN `0x82` and OUT `0x02` | none |
| Three-interface | K95 RGB fw 2.05, Strafe RGB fw 2.05, M65 fw 2.04 | Boot keyboard or mouse | Interrupt IN `0x82` only, carrying keyboard input | Command interface with interrupt OUT `0x03` only |

Desk accessories:

| Device | Interface 0 | Interface 1 |
| ------ | ----------- | ----------- |
| Polaris (`0x1B3B`) | Command collection (page `0xFFC2`, usage `0x0001`) | Keyboard collection (page `0x0001`, usage `0x0006`) |
| ST100 (`0x0A34`) | Single vendor-only interface, interrupt IN `0x81` and OUT `0x01`, 64 bytes | none |

The usage page alone does not identify the command collection: on the
two-interface layout it also appears, input-only, on the composite input
interface. Descriptors therefore pin the interface and, where the page is
shared, the usage.

On the three-interface layout, `usbhid` refuses interface 2 because it has no
interrupt IN endpoint, so the kernel creates no hidraw node for it and no
host-managed HID path reaches the command interface. Hypercolor does not drive
that firmware: the transport finds no `0xFFC2`/`0x0004` collection on
interface 1 and the connect fails closed, before any packet is written.

Write path. Each command is one output report: report number 0 followed by the
64-byte packet. The kernel sends an output report on the interface's interrupt
OUT endpoint when it has one, and otherwise as a HID `SET_REPORT` control
request:

| Field | Value |
| ----- | ----- |
| `bRequestType` | `0x21` |
| `bRequest` | `0x09` |
| `wValue` | `0x0200` (output report, ID 0) |
| `wIndex` | interface number |
| `wLength` | `64` |

Those are the two write paths older tooling chose between by firmware version,
so Hypercolor needs no firmware predicate and no negotiated transport mode.

Read path. Hypercolor reads nothing. Current firmware answers on the interrupt
IN endpoint; older firmware answers through a feature-report read:

| Field | Value |
| ----- | ----- |
| `bRequestType` | `0xA1` |
| `bRequest` | `0x01` |
| `wValue` | `0x0300` (feature report, ID 0) |
| `wIndex` | interface number |
| `wLength` | `64` |

Waiting for an input report on that firmware would stall every command, and
lighting needs no reply, so every NXP command is write-only. While the hidraw
node is open the kernel keeps polling the IN endpoint, so unread replies cannot
back up the device.

### 5.2 Packet Geometry

Every NXP command is 64 bytes:

| Offset | Size | Field | Description |
| ------ | ---- | ----- | ----------- |
| 0 | 1 | Command | `SET`, `GET`, `WRITE_BULK`, or `READ_BULK` |
| 1 | 1 | Field | Protocol field ID |
| 2..63 | 62 | Arguments/payload | Command-specific, zero-padded |

### 5.3 Command Vocabulary

| Name | Byte | Purpose |
| ---- | ---- | ------- |
| `CMD_SET` | `0x07` | Write a single field |
| `CMD_GET` | `0x0E` | Read a single field |
| `CMD_WRITE_BULK` | `0x7F` | Write a stream of up to five packets |
| `CMD_READ_BULK` | `0xFF` | Read a stream of up to five packets |

### 5.4 Field Vocabulary

| Name | Byte | Access | Purpose |
| ---- | ---- | ------ | ------- |
| `FIELD_IDENT` | `0x01` | read | Firmware identification |
| `FIELD_RESET` | `0x02` | write | Reset / bootloader transition |
| `FIELD_SPECIAL` | `0x04` | write | Special function mode |
| `FIELD_LIGHTING` | `0x05` | write | Hardware/software lighting mode |
| `FIELD_POLLRATE` | `0x0A` | write | Poll rate |
| `FIELD_FW_START` | `0x0C` | write | Firmware update start |
| `FIELD_FW_DATA` | `0x0D` | write | Firmware update data |
| `FIELD_MOUSE` | `0x13` | mixed | Mouse subcommands |
| `FIELD_KB_HWCLR` | `0x14` | mixed | Keyboard hardware colors |
| `FIELD_M_PROFID` | `0x15` | mixed | Mouse profile ID |
| `FIELD_M_PROFNM` | `0x16` | mixed | Mouse profile name, UTF-16LE |
| `FIELD_KB_HWANM` | `0x17` | mixed | Hardware animation commands |
| `FIELD_M_COLOR` | `0x22` | write | Mouse software colors |
| `FIELD_MP_COLOR` | `0x22` | write | Mousepad software colors |
| `FIELD_KB_ZNCLR` | `0x25` | write | Zoned keyboard color |
| `FIELD_KB_9BCLR` | `0x27` | write | 9-bit keyboard color |
| `FIELD_KB_COLOR` | `0x28` | write | 24-bit keyboard color |
| `FIELD_KEYINPUT` | `0x40` | write | HID vs vendor input event mode |
| `FIELD_BATTERY` | `0x50` | read | Battery status |

### 5.5 Mode Values

| Name | Byte | Purpose |
| ---- | ---- | ------- |
| `MODE_HARDWARE` | `0x01` | Hardware-controlled lighting/input |
| `MODE_SOFTWARE` | `0x02` | Host-controlled lighting/input |
| `MODE_SIDELIGHT` | `0x08` | Strafe sidelight toggle |
| `MODE_WINLOCK` | `0x09` | K55/K68 winlock LED |

### 5.6 Color Selectors

| Name | Byte |
| ---- | ---- |
| `COLOR_RED` | `0x01` |
| `COLOR_GREEN` | `0x02` |
| `COLOR_BLUE` | `0x03` |

### 5.7 NXP Init Sequence

As built, init sends one write-only packet: `SET FIELD_LIGHTING MODE_SOFTWARE`
(`07 05 02 00`, zero-padded to 64 bytes).

- It does not read `FIELD_IDENT`. The kernel picks the write path (section
  5.1), and older firmware has no input-report reply to wait for.
- It does not touch `FIELD_SPECIAL` (`0x04`) or `FIELD_KEYINPUT` (`0x40`). Those
  route the top-row keys and the key matrix to the vendor collection, which is
  input handling, not lighting. The daemon must not disrupt normal keyboard or
  mouse input.
- Devices with no lighting (`NoLights`) send nothing.

### 5.8 NXP Activation Sequence

To stream direct RGB:

1. Set `FIELD_LIGHTING` / `MODE_SOFTWARE`.
2. Force one RGB frame.
3. Continue sending only when colors change.

To deactivate:

1. Set `FIELD_LIGHTING` / `MODE_HARDWARE`.
2. Do not reset the USB device unless required by descriptor quirk.

### 5.9 Full-Range Keyboard RGB

Full-range keyboards use planar 24-bit component writes. A frame consists of 12 packets:

| Packet | Command | Field | Arguments | Payload |
| ------ | ------- | ----- | --------- | ------- |
| 1 | `WRITE_BULK` | `0x01` | length `0x3C` | Red bytes `0..59` |
| 2 | `WRITE_BULK` | `0x02` | length `0x3C` | Red bytes `60..119` |
| 3 | `WRITE_BULK` | `0x03` | length `0x30` | Red bytes `120..end`, padded |
| 4 | `SET` | `FIELD_KB_COLOR` | `COLOR_RED, 0x03, 0x01` | Commit red |
| 5 | `WRITE_BULK` | `0x01` | length `0x3C` | Green bytes `0..59` |
| 6 | `WRITE_BULK` | `0x02` | length `0x3C` | Green bytes `60..119` |
| 7 | `WRITE_BULK` | `0x03` | length `0x30` | Green bytes `120..end`, padded |
| 8 | `SET` | `FIELD_KB_COLOR` | `COLOR_GREEN, 0x03, 0x01` | Commit green |
| 9 | `WRITE_BULK` | `0x01` | length `0x3C` | Blue bytes `0..59` |
| 10 | `WRITE_BULK` | `0x02` | length `0x3C` | Blue bytes `60..119` |
| 11 | `WRITE_BULK` | `0x03` | length `0x30` | Blue bytes `120..end`, padded |
| 12 | `SET` | `FIELD_KB_COLOR` | `COLOR_BLUE, 0x03, 0x02` | Commit blue |

Packet header shape for `WRITE_BULK`:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x7F` |
| 1 | 1 | Bulk index | `0x01..0x03` |
| 2 | 1 | Length | `0x3C` or `0x30` |
| 3 | 1 | Reserved | `0x00` |
| 4..63 | 60 | Payload | Component bytes |

Packet header shape for component commit:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `0x28` |
| 2 | 1 | Color | `0x01`, `0x02`, or `0x03` |
| 3 | 1 | Chunk count | `0x03` |
| 4 | 1 | Apply selector | `0x01` or `0x02` |

For monochrome devices that use the same protocol, send only the red/intensity channel commit sequence.

### 5.10 Packed 512-Color Keyboard RGB

Older keyboards use 9-bit color: each channel is quantized to 3 bits and two LEDs are packed per byte.

Quantization:

```text
component3 = component8 >> 5
```

Optional ordered dithering may be added later, but initial implementation should use deterministic truncation unless visual testing proves dithering is needed.

Packing:

```text
packed = ((7 - second_component3) << 4) | (7 - first_component3)
```

Frame packet sequence:

| Packet | Command | Purpose |
| ------ | ------- | ------- |
| 1 | `WRITE_BULK 0x01 60` | Red plane bytes `0..59` |
| 2 | `WRITE_BULK 0x02 60` | Red remainder + green start |
| 3 | `WRITE_BULK 0x03 60` | Green remainder + blue start |
| 4 | `WRITE_BULK 0x04 36` | Blue remainder |
| 5 | `SET FIELD_KB_9BCLR` | Apply packed 9-bit frame |

Apply packet:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `0x27` |
| 2 | 1 | Reserved | `0x00` |
| 3 | 1 | Reserved | `0x00` |
| 4 | 1 | Length/check | `0xD8` |

### 5.11 K55-Style Zoned Keyboard RGB

K55-style zoned lighting uses interleaved RGB triples:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `0x25` |
| 2 | 1 | Reserved | `0x00` |
| 3 | 1 | Reserved | `0x00` |
| 4..12 | 9 | RGB data | `R,G,B` for three zones |

Winlock LED quirk:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `FIELD_LIGHTING` (`0x05`) |
| 2 | 1 | Mode | `MODE_WINLOCK` (`0x09`) |
| 3 | 1 | Reserved | `0x00` |
| 4 | 1 | Enabled | `0x00` or `0x01` |

### 5.12 Strafe Sidelight

Strafe devices have side lighting that is controlled separately:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `FIELD_LIGHTING` (`0x05`) |
| 2 | 1 | Mode | `MODE_SIDELIGHT` (`0x08`) |
| 3 | 1 | Reserved | `0x00` |
| 4 | 1 | Enabled | `0x00` or `0x01` |

The sidelight packet should be sent before RGB frame packets when sidelight state changes.

### 5.13 Mouse RGB

NXP mouse RGB packet:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `FIELD_M_COLOR` (`0x22`) |
| 2 | 1 | Zone count | variable |
| 3 | 1 | Mode | `0x01` |
| 4.. | variable | Zone entries | `zone_id, R, G, B` |

Default zone IDs are one-based. Use descriptor topology to decide which software zones exist.

Special case:

- Glaive should write only zones `front`, `back`, and `side`.
- Devices with `nxp_dpi_rgb_in_dpi_packet` must send DPI LEDs through DPI configuration instead of the main RGB packet.

### 5.14 Mousepad and ST100 RGB

Mousepad-style RGB packet:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `FIELD_MP_COLOR` (`0x22`) |
| 2 | 1 | Zone count | typically `15` |
| 3 | 1 | Reserved | `0x00` |
| 4.. | variable | RGB triples | `R,G,B` per zone |

This applies to Polaris-style mousepads and the ST100 stand if enabled.

### 5.15 NXP Wireless and Hardware Animation

Some NXP wireless devices do not support real-time direct RGB through the ordinary mouse/keyboard frame packet. They use hardware animation commands instead.

Hardware animation packet starts:

| Offset | Size | Field | Value |
| ------ | ---- | ----- | ----- |
| 0 | 1 | Command | `0x07` |
| 1 | 1 | Field | `0xAA` |
| 2..3 | 2 | Reserved | `0x00` |
| 4.. | variable | Animation payload | Device-specific |

Initial Hypercolor support should mark these as `researched` unless a static-color hardware animation path is explicitly implemented and tested.

---

## 6. Legacy Protocol

Legacy devices are mostly hardware controlled. They are useful for completeness but should not block Bragi/NXP work.

### 6.1 Keyboard Control Transfer

Legacy keyboard command macro:

| USB Field | Value |
| --------- | ----- |
| `bRequestType` | `0x40` |
| `bRequest` | `(command >> 16) & 0xFF` |
| `wValue` | `command & 0xFFFF` |
| `wIndex` | `0x0000` |
| `wLength` | `0` |
| timeout | 5000ms |

Command values:

| Name | Value | Purpose |
| ---- | ----- | ------- |
| `NK95_HWOFF` | `0x020030` | Disable hardware mode |
| `NK95_HWON` | `0x020001` | Enable hardware mode |
| `NK95_M1` | `0x140001` | Select M1 |
| `NK95_M2` | `0x140002` | Select M2 |
| `NK95_M3` | `0x140003` | Select M3 |
| `NK95_BRIGHTNESS_0` | `0x310000` | Brightness off |
| `NK95_BRIGHTNESS_33` | `0x310001` | Brightness 33% |
| `NK95_BRIGHTNESS_66` | `0x310002` | Brightness 66% |
| `NK95_BRIGHTNESS_100` | `0x310003` | Brightness 100% |

Direct per-key RGB is not required for legacy keyboards in Hypercolor. Expose at most brightness and hardware-mode controls in a later dynamic control-surface pass.

### 6.2 Legacy M95 Backlight

M95 legacy mouse backlight command:

| USB Field | Value |
| --------- | ----- |
| `bRequestType` | `0x40` |
| `bRequest` | `49` |
| `wValue` | `0` off, `1` on |
| `wIndex` | `0` |
| `wLength` | `0` |

This is binary lighting, not full RGB. Register as `researched` until Hypercolor supports point-source on/off-only capabilities cleanly.

### 6.3 Why Legacy Devices Stay Researched

Every legacy command is a USB vendor control request (`bRequestType 0x40`) to the
device. hidraw carries only HID class requests (output, input, and feature
reports), so no host-managed HID path reaches these commands. Sending them
takes the whole USB device node through `UsbVendor`, and write access to that
node is enough for any holder to detach `usbhid` from the keyboard or mouse.
That fails the input-safety rule in section 8.7, and the protocol has no direct
RGB to gain in exchange (hardware modes and brightness only). The K65, K70,
K90, and K95 Legacy and the M95 stay `researched`.

---

## 7. Color Encoding and Topology

### 7.1 Topology Strategy

Hypercolor should not reproduce a keyboard input map inside the lighting driver. Instead, store only lighting topology:

```rust
enum CorsairPeripheralTopology {
    Keyboard {
        led_count: u32,
        layout: KeyboardLightingLayout,
        extra_regions: &'static [KeyboardExtraRegion],
    },
    Mouse {
        zones: &'static [MouseZone],
    },
    Mousepad {
        zones: u32,
    },
    HeadsetStand {
        zones: u32,
    },
    None,
}
```

`Protocol::zones()` should return a small number of physical zones:

- Per-key keyboard: one matrix/custom zone containing all LEDs
- Zoned keyboard: one strip/custom zone per logical zone group when useful
- Mouse: named point/custom zones
- Mousepad: strip/custom zone
- Dongle: no RGB zones

Use `DeviceTopologyHint::Custom` for keyboards with non-rectangular top bars, macro columns, side bars, or logo LEDs. A richer `ZoneLayoutHint` can be added later.

### 7.2 Bragi LED Counts

| Model Family | LED Count | Suggested Topology |
| ------------ | --------- | ------------------ |
| K55 Pro | 6 | `Custom` zones |
| K55 Pro XT | 137 | `Custom` keyboard |
| K57 USB | 137 | `Custom` keyboard |
| K60 Pro RGB/LP/SE/Mono/TKL | 123 | `Matrix { rows: 6, cols: 21 }` or `Custom` |
| K65 Mini | 123 | `Matrix { rows: 6, cols: 21 }` or `Custom` |
| K70 Core RGB | 123 | `Matrix { rows: 6, cols: 21 }` or `Custom` |
| K70 Core RGB TKL | 102 | `Matrix { rows: 6, cols: 17 }` or `Custom` |
| K95 Platinum XT | 156 | `Custom` keyboard plus top bar |
| K100 | 193 | `Custom` keyboard plus wheel/top/side/logo regions |
| K70 TKL / K70 Pro | 193 | `Custom` keyboard plus logo/top regions |
| M55 RGB Pro | 2 | `Point`/custom mouse zones |
| Ironclaw Wireless USB | 6 | custom mouse zones |
| Harpoon Wireless USB | 2 | custom mouse zones |
| Katar Pro / V2 / XT | 2 | custom mouse zones |
| M65 RGB Ultra | 2 or 3 | custom mouse zones |
| M75 Gaming Mouse | 2 | custom mouse zones |
| Dark Core RGB Pro / SE | 12 | custom mouse zones |
| Scimitar Elite Bragi | 5 | custom mouse zones |
| MM700 / 3XL | 3 | `Strip` or custom mousepad zones |

### 7.3 Bragi Mouse Zone Names

Use these names where the model supports them:

| Logical Name | Meaning |
| ------------ | ------- |
| `front` | Front mouse light |
| `back` | Rear/logo light |
| `dpi` | DPI indicator |
| `wheel` | Scroll wheel light |
| `thumb` | Thumb/side panel |
| `side` | Side light |
| `bar0..bar4` | Extended Dark Core bar LEDs |
| `dpi0..dpi5` | DPI stage LEDs |

Descriptor topology should define actual ordering per model. Do not assume all mice expose all zones.

### 7.4 NXP Keyboard Lighting Classes

| Class | Products | Encoding |
| ----- | -------- | -------- |
| 512-color keyboards | K65 RGB, K70 RGB, K95 RGB, Strafe non-RGB first variant | Packed 9-bit |
| Full-range keyboards | Lux, Rapidfire, MK.2, Platinum, Strafe RGB/MK.2, K68 RGB | 24-bit planar |
| Monochrome keyboards | K63 non-RGB, K68 non-RGB, K70 non-RGB variants, K60 Mono | Red/intensity channel only |
| Zoned keyboards | K55 | 3 RGB zones |
| No-light keyboards | K66 | No RGB frame output |

NXP per-key keyboards address 144 hardware LED slots: the packed frame's 216
bytes (`0xD8`) are three planes of 72 bytes at two LEDs per byte. Hypercolor
exposes the slots in hardware index order as one `Custom` zone, and per-key
positions come from the spatial layout. A full-range plane declares 168 bytes
(60 + 60 + 48). Slots 144 through 167 are not mapped by this spec (the K95
Platinum light bar is the likely occupant), so Hypercolor writes zero there.
The K55 exposes its three zones as a 1x3 matrix.

### 7.5 NXP Mouse Zone Counts

| Family | Software Zones | Hardware Save Zones | Notes |
| ------ | -------------- | ------------------- | ----- |
| M65 | up to 6 | 2 | Skip DPI LED in hardware save |
| Sabre | up to 6 | 3 | Skip DPI LED in hardware save |
| Scimitar | up to 6 | 4 | Skip DPI LED in hardware save |
| Glaive | 3 | device-specific | Only front/back/side in software path |
| Glaive Pro | 3 + DPI | device-specific | DPI RGB in DPI packet |
| Dark Core NXP | hardware animation | hardware animation | Defer direct RGB |

The M65 and Scimitar descriptors drive all six software zone IDs (1 through 6);
which IDs each model actually lights is unverified. The Polaris and the ST100
use the 15-slot mousepad packet. The ST100 has nine physical zones (one logo
zone and eight base zones, per Corsair's manual); their order within the 15
slots is not in this spec, so Hypercolor drives all 15.

### 7.6 Frame Normalization

All protocols must normalize input frames to expected topology length.

Policy:

| Condition | Behavior |
| --------- | -------- |
| Input length equals expected | Borrow input slice |
| Input length shorter | Pad missing LEDs with black |
| Input length longer | Truncate extra LEDs |
| Expected length zero | Return no commands |

For protocols where a mismatch indicates a topology bug rather than normal user input, log a warning with `expected`, `actual`, and device name.

### 7.7 Color Space

All peripheral protocols in this spec use RGB byte ordering at the Hypercolor boundary. Internal packet layout may be planar, interleaved, or packed, but `Protocol::encode_frame()` receives `[[u8; 3]]` in RGB order.

Color-space metadata:

```rust
DeviceColorFormat::Rgb
DeviceColorSpace::default()
```

Do not apply gamma correction in the driver. Gamma belongs in the render pipeline or device color profile layer.

---

## 8. Timing and Transport Safety

### 8.1 Response Timeouts

| Protocol | Timeout | Notes |
| -------- | ------- | ----- |
| Bragi | 2s | Response-driven; no fixed inter-command delay needed after successful responses |
| NXP modern | 2s | Wait on response/input report when command expects a response |
| NXP control | 5s control timeout | USB control transfer timeout |
| Legacy | 5s control timeout | USB control transfer timeout |

### 8.2 Frame Rate

Recommended initial frame rates:

| Protocol | Default Max FPS | Reason |
| -------- | --------------- | ------ |
| Bragi 64B | 30 | Conservative for multi-packet keyboards |
| Bragi 1024B | 30 | Large reports, response-gated |
| Bragi mouse | 45 | Small payloads |
| NXP keyboard full-range | 30 | 12 packets per frame |
| NXP keyboard 512-color | 30 | 5 packets per frame |
| NXP mouse | 45 | Single packet |
| NXP mousepad | 30 | Single packet but more LEDs |
| Legacy | 0 | No direct animation |

Expose exact `frame_interval()` per descriptor. Start conservative; hardware smoke tests can raise caps later.

### 8.3 Software vs Hardware Mode

Drivers must return devices to hardware mode on disconnect when safe.

| Protocol | Active Mode | Idle Mode |
| -------- | ----------- | --------- |
| Bragi | `MODE_SOFTWARE` | `MODE_HARDWARE` |
| NXP | `FIELD_LIGHTING/MODE_SOFTWARE` | `FIELD_LIGHTING/MODE_HARDWARE` |
| Legacy | normally hardware | hardware |

If a device has `requires_unclean_exit`, skip risky handover work and let the OS/device settle naturally.

### 8.4 Input Device Safety

Hypercolor is an RGB lighting daemon, not a keyboard remapper. It must not break normal HID input.

Rules:

1. Prefer HIDAPI/hidraw collection access over claiming whole USB interfaces.
2. Filter by vendor usage page/usage when the OS exposes that metadata.
3. Do not detach `usbhid` for keyboard or mouse input interfaces.
4. Do not synthesize replacement input devices.
5. Do not enable vendor input-event mode unless the lighting protocol requires it.
6. If safe lighting collection selection is impossible, keep the device `researched`.

This may require extending HAL transports before enabling some peripherals.

### 8.5 Kernel Driver Interaction

Some NXP paths historically used direct interrupt endpoints. On Linux, direct endpoint claiming can detach `usbhid`, causing input loss. Hypercolor should first attempt:

1. `UsbHidApi` with output/input reports.
2. `UsbHidRaw` with output/input report mode.
3. Direct `UsbHid` only for non-input devices or explicitly tested safe interfaces.

Mousepads, stands, and some controller-like devices are safer candidates for direct `UsbHid` than keyboards/mice.

As built, every Corsair peripheral uses option 1. None uses direct `UsbHid`,
including the mousepads and the stand: the Polaris shares its USB device with a
keyboard collection, so it is an input device too.

### 8.6 Error Policy

| Error | Policy |
| ----- | ------ |
| Unsupported Bragi property | Log debug/warn, continue if optional |
| Failed Bragi lighting handle | Try alternate resource if descriptor allows; otherwise disconnect protocol |
| NXP response timeout | Retry frame once, then mark device degraded |
| Frame length mismatch | Normalize and warn |
| Unknown child PID | Keep parent connected, ignore child |
| Unknown keyboard layout byte | Use `Custom` topology, log once |

---

### 8.7 Input-Safe Registration Rule

A Corsair peripheral descriptor is registered only when all of these hold:

1. Its transport is host-managed HID: `UsbHidApi` or `UsbHidRaw`. Both keep
   `usbhid` attached, and neither claims a USB interface.
2. The transport pins the interface that carries the lighting collection and
   filters on a vendor-defined usage page (`0xFF00` and up), plus the usage
   wherever that page also appears on an input interface.
3. The PID has exactly one descriptor, with no firmware predicate, so there is
   no alternate descriptor to fall back to. The USB backend opens a
   descriptor's transport as declared and never substitutes another.

`crates/hypercolor-hal/tests/corsair_peripheral_input_safety_tests.rs` enforces
the rule. It classifies every `TransportType` variant with an exhaustive match,
so a new variant fails to compile until someone decides whether it can detach
a kernel driver. It checks every registered Corsair descriptor that is either
in the peripheral module or typed keyboard, mouse, mousepad, or headset in the
vendor database. That catches a peripheral registered outside the module too.

### 8.8 Linux Device Permissions

`udev/99-hypercolor.rules` grants Corsair access per product, never
vendor-wide. Keyboards and mice expose their boot-keyboard, NKRO, and pointer
collections as hidraw nodes, and granting those to the fallback group would hand
every member a keystroke feed.

Peripheral rules are two-step. The first rule tags a product's hidraw nodes
with `ENV{HYPERCOLOR_CORSAIR_LIGHTING_IF}`. The second grants only the node
whose USB interface matches the tag. Two steps are needed because `idProduct`
lives on the USB device and `bInterfaceNumber` on the USB interface, and udev
requires every `ATTRS` key in one rule to match the same parent.

| Tag | Grant | Products |
| --- | ----- | -------- |
| `01` | interface 1 | Bragi keyboards, mice, and mousepads |
| `01-io` | interface 1 with `bNumEndpoints == 02` | NXP keyboards and mice. The endpoint check admits the two-interface command interface and keeps the three-interface layout's IN-only keyboard interface root-only. |
| `00` | interface 0 | Polaris, ST100 |

Peripherals get no `usb_device` grant: Hypercolor never opens their USB device
node, and write access to it is enough to detach `usbhid`. Fan, LCD, and
Lighting Node controllers have no input collections and claim their HID
interface directly, so they get per-product hidraw and `usb_device` rules.
`crates/hypercolor-hal/tests/udev_rules_tests.rs` pins all of this.

One exposure remains by design. The granted lighting node is writable by the
fallback group, and a writer could send `FIELD_KEYINPUT` to move key reports
onto that node. Closing that needs the grant to be uaccess-only, which only
works from a rules file that sorts before `73-seat-late.rules`.

---

## 9. HAL Integration and Testing

### 9.1 Module Layout

```text
crates/hypercolor-hal/src/drivers/corsair/peripheral/
  mod.rs          # public exports
  devices.rs      # descriptors and factories
  types.rs        # protocol enums, descriptors, quirks
  topology.rs     # topology tables and zone helpers
  bragi.rs        # Bragi protocol implementation
  nxp.rs          # NXP/CUE protocol implementation
  legacy.rs       # legacy control command implementation
```

`crates/hypercolor-hal/src/drivers/corsair/devices.rs` should aggregate the new descriptors:

```rust
all.extend_from_slice(peripheral::devices::descriptors());
```

Only do this after at least one protocol family has passing unit tests and a conservative descriptor subset.

### 9.2 Public Factories

Use explicit builders:

```rust
pub fn build_corsair_k70_pro_protocol() -> Box<dyn Protocol>;
pub fn build_corsair_k100_protocol() -> Box<dyn Protocol>;
pub fn build_corsair_m55_protocol() -> Box<dyn Protocol>;
```

For many models, builders can delegate to generic constructors:

```rust
fn build_bragi_keyboard(
    name: &'static str,
    led_count: u32,
    packet_size: usize,
    quirks: CorsairPeripheralQuirks,
) -> Box<dyn Protocol>;
```

### 9.3 Protocol State

Bragi protocol should keep lightweight mutable state:

```rust
struct BragiState {
    active: bool,
    lighting_handle_open: bool,
    second_lighting_handle_open: bool,
    alt_lighting: bool,
    brightness_mode: BragiBrightnessMode,
    last_frame_was_black: bool,
}
```

Use `RwLock` or `Mutex` inside the protocol only if state changes during `parse_response()` or between encode calls. For pure frame encoding, keep state in the backend connection layer instead.

NXP protocol can be mostly stateless after descriptor configuration.

### 9.4 Protocol Trait Mapping

| `Protocol` Method | Bragi | NXP | Legacy |
| ----------------- | ----- | --- | ------ |
| `name()` | descriptor name | descriptor name | descriptor name |
| `init_sequence()` | property discovery subset | firmware/mode discovery | none or brightness probe |
| `shutdown_sequence()` | close handles + hardware mode | hardware mode | hardware mode |
| `encode_frame()` | planar or alt Bragi write | NXP frame packets | empty |
| `encode_brightness()` | property set if supported | optional future | legacy brightness levels |
| `keepalive()` | 50s poll packet | usually none | none |
| `parse_response()` | error byte/property values | echo/status validation | control status |
| `zones()` | descriptor topology | descriptor topology | empty/point |
| `capabilities()` | direct RGB if LEDs > 0 | direct RGB if LEDs > 0 | limited/no direct |

### 9.5 Descriptor Registration

Initial supported subset should be Bragi wired devices with known packet sizes and LED counts:

| PID | Model | Why First |
| --- | ----- | --------- |
| `0x1BB3` | K70 RGB Pro V2 | Jumbo Bragi keyboard, common modern family |
| `0x1B73` | K70 TKL | Jumbo Bragi keyboard |
| `0x1BAF` | K65 Mini | Jumbo Bragi compact keyboard |
| `0x1BA0` | K60 Pro RGB | Standard Bragi keyboard |
| `0x1B70` | M55 RGB Pro | Simple two-zone Bragi mouse |
| `0x1B93` | Katar Pro | Simple two-zone Bragi mouse |
| `0x1B9B` | MM700 | Simple Bragi mousepad |

As built, all 35 Bragi wired descriptors and the 13 NXP/CUE descriptors in
section 2.6 are registered. Receivers, the K66, and legacy devices stay
`researched` for the reasons in section 2.6.

### 9.6 Tests

Create:

```text
crates/hypercolor-hal/tests/corsair_peripheral_bragi_tests.rs
crates/hypercolor-hal/tests/corsair_peripheral_nxp_tests.rs
crates/hypercolor-hal/tests/corsair_peripheral_database_tests.rs
```

Required Bragi tests:

- Property get packet is padded to descriptor packet size.
- Property set packet encodes 16-bit value little-endian.
- Open handle encodes resource little-endian.
- Write payload chunks at `packet_size - 7` then `packet_size - 3`.
- Standard RGB encoding is red plane, green plane, blue plane.
- Monochrome encoding emits one intensity plane.
- Alternate RGB encoding emits `0x12, 0x00` then RGB triples.
- Jumbo packet descriptors emit 1024-byte packets.
- Large packet descriptor emits 128-byte packets.
- All-black transition emits brightness-off command when supported.
- Non-black transition emits brightness-on command when supported.

Required NXP tests:

- Full-range keyboard emits 12 packets in correct component order.
- 512-color keyboard packs two 3-bit components per byte.
- K55 zoned packet emits three RGB triples.
- Mouse packet emits `zone_id, R, G, B` entries.
- Mousepad packet emits contiguous RGB triples.
- Monochrome keyboard emits red/intensity path only.

Required database tests:

- All supported descriptors use VID `0x1B1C`.
- Bragi jumbo devices use max report length 1024.
- Scimitar Elite Bragi uses max report length 128.
- Dongles have zero LED count and no direct RGB capability.
- Every registered peripheral is `supported` in `data/drivers/vendors/corsair.toml`, nothing `researched` is registered, and the K66 and legacy devices stay unregistered.
- NXP descriptors pin interface, usage page, usage, report ID 0, and a 65-byte HIDAPI buffer, and report 144 slots (keyboards), 6 zones (mice), or 15 slots (accessories).

Required input-safety tests (`corsair_peripheral_input_safety_tests.rs`):

- Every Corsair peripheral transport is host-managed HID on a vendor usage page.
- Every peripheral pins its lighting interface; NXP keyboards and mice pin usage `0x0004`.
- Each peripheral PID has exactly one descriptor and no firmware predicate.
- The classifier rejects every interface-claiming transport.

Required udev tests (`udev_rules_tests.rs`):

- No Corsair rule is vendor-wide.
- Each peripheral's tag grants the interface its transport opens, NXP keyboard and mouse grants require two endpoints, and no peripheral gets a whole-product hidraw grant or a `usb_device` grant.

Additional NXP encoder tests:

- Init sends only the lighting-mode packet and never touches `FIELD_SPECIAL` or `FIELD_KEYINPUT`.
- Every init, frame, and shutdown command is write-only and 64 bytes.
- Reused command buffers encode the same bytes as fresh ones.

### 9.7 Hardware Smoke Tests

Manual smoke-test checklist per device:

1. Device input still works before, during, and after daemon connection.
2. Solid red, green, blue frames map to correct color channels.
3. All-black frame turns lights off.
4. Shutdown returns to hardware lighting.
5. Reconnect works without unplugging.
6. Frame stream at default FPS does not produce visible flicker.
7. No kernel disconnect/reconnect spam appears in logs.

Capture useful facts from smoke tests into descriptor quirks.

### 9.8 Documentation Updates

When implementation lands:

- Add supported devices to `data/drivers/vendors/corsair.toml`.
- Update `docs/specs/18-corsair-integration.md` to point peripheral readers here.
- Add compatibility matrix generation coverage if `just compat` consumes driver database entries.
- Mention input-safety limitations in user-facing docs.

---

## 10. Recommendation

Build this in three deliberate waves.

### Wave 1: Bragi Wired RGB

Implement `corsair::peripheral::bragi` first.

Why:

- It covers the most modern Corsair peripherals.
- It has the cleanest RGB data model.
- It aligns with existing Hypercolor patterns from iCUE LINK: endpoint-like handles, chunked writes, keepalive, and runtime state.
- It avoids NXP's firmware transport split and legacy control-transfer quirks.

Scope:

- Bragi packet builder
- Standard and alternate RGB encoders
- Static descriptor topology for wired/non-dongle devices
- Conservative transport binding
- Unit/database tests

Do not enable wireless child enumeration in Wave 1.

### Wave 2: NXP/CUE RGB

Shipped 2026-11-05 on the hidraw command collection (section 5.1), without
waiting for a hardware run, because the input-safety rule in section 8.7 holds
by construction.

Scope:

- 24-bit planar keyboard path
- 512-color packed keyboard path
- K55 zoned path
- Mouse and mousepad packets
- Firmware/override transport selection

Start with non-wireless devices and non-input-sensitive devices where possible.

### Wave 3: Wireless, Legacy, and Controls

Receiver routing primitives exist (section 4.20.2); registration waits on the
gaps in section 4.20.1. Legacy devices stay held (section 6.3).

Add advanced features last:

- Bragi dongle child enumeration
- Battery telemetry
- Pairing control surface
- DPI color/control
- Poll rate control
- Legacy brightness/on-off support

The clear choice is **Bragi first, wired only, descriptor-driven**. It gives Hypercolor meaningful modern peripheral coverage while keeping the implementation small enough to verify thoroughly. NXP then becomes a second protocol module, not a pile of special cases mixed into the first one.

---

## References

Facts only; no code was copied from any GPL source.

- Linux kernel, `Documentation/hid/hidraw.rst`: hidraw `write()` takes the report number as the first byte, 0 for devices without numbered reports. https://www.kernel.org/doc/html/latest/hid/hidraw.html
- Linux kernel, `drivers/hid/hidraw.c` (`hidraw_send_report`): output reports fall back to a `SET_REPORT` request when the transport has no interrupt OUT path. https://github.com/torvalds/linux/blob/master/drivers/hid/hidraw.c
- Linux kernel, `drivers/hid/usbhid/hid-core.c`: `usbhid` refuses an interface with no interrupt IN endpoint. https://github.com/torvalds/linux/blob/master/drivers/hid/usbhid/hid-core.c
- systemd, `udev(7)`: parent-matching keys in one rule must all match the same parent device. https://www.freedesktop.org/software/systemd/man/latest/udev.html
- OpenRGB detector table (GPL-2.0; read for interface and usage-page facts only): NXP keyboards and mice on interface 1, page `0xFFC2`; Polaris and ST100 on page `0xFFC2`. https://gitlab.com/CalcProgrammer1/OpenRGB/-/blob/master/Controllers/CorsairPeripheralController/CorsairPeripheralControllerDetect.cpp
- OpenRGB work item 4944, HIDAPI enumeration of a K70 RGB (`1B13`): interface 0 carries keyboard, consumer, `0xFFC0`, and `0xFFC2`/`0x0003` collections; interface 1 carries only `0xFFC2`/`0x0004`. https://gitlab.com/CalcProgrammer1/OpenRGB/-/work_items/4944
- OpenRGB issue 4016, HIDAPI enumeration: Polaris interface 0 `0xFFC2`/`0x0001`, interface 1 keyboard `0x0001`/`0x0006`; K100 (`1BC5`) Bragi `0xFF42` split over interfaces 1 (`0x0001`) and 2 (`0x0002`). https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4016
- linux-hardware.org `lsusb -v` probes: K70 RGB fw 3.08 (https://linux-hardware.org/?probe=ead58254b2&log=lsusb), K95 Platinum fw 3.21 (https://linux-hardware.org/?probe=d6735c5f18&log=lsusb), K95 RGB fw 2.05 (https://linux-hardware.org/?probe=45c2efaa3e&log=lsusb), M65 fw 2.04 (https://linux-hardware.org/?probe=17bcc983ca&log=lsusb), ST100 (https://linux-hardware.org/?probe=59ffe32cfa&log=lsusb), M55 RGB Pro (https://linux-hardware.org/index.php?probe=8ea3fec27b&log=lsusb), K70 RGB Pro (https://linux-hardware.org/?probe=28f1074e0a&log=lsusb).
- ckb-next issue 659, Strafe RGB fw 2.05 `dmesg`: interface 1 binds as a keyboard input device. https://github.com/ckb-next/ckb-next/issues/659
- K70 MK.2 fw 3.24 `dmesg` report: the vendor interface binds as hidraw with no input device. https://gist.github.com/KenEucker/f13eb2edbbd5c5ab02b66e0e6ddeb8c8
- Corsair ST100 RGB user manual: nine RGB zones, one logo and eight base. https://manuals.online/corsair/rack-and-stand/st100-rgb/user-manual/1rmy1
