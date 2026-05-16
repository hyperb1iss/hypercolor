# hypercolor-hal

*Hardware abstraction layer for USB/HID/SMBus native device drivers.*

This crate separates pure protocol encoding from transport I/O. It defines
the `Protocol` trait for wire-format encode/decode, provides a static
`ProtocolDatabase` keyed by USB VID/PID, and ships nine protocol driver
families. Transport adapters wrap nusb/hidapi/hidraw/serial/MIDI/SMBus so
protocol code never touches I/O directly.

By workspace rule, `hypercolor-hal` must never depend on `hypercolor-core`.
It sits below `core` in the dependency graph and is consumed by both `core`
and `hypercolor-driver-builtin`. On Windows it conditionally pulls in
`hypercolor-windows-pawnio` as its SMBus transport backend (the Linux
equivalent uses the kernel's `/dev/i2c-*` device tree via `i2cdev`).

## Workspace position

**Depends on:** `hypercolor-types`; `hypercolor-windows-pawnio` (Windows
only, via `[target.cfg]` — not a feature flag).

**Depended on by:** `hypercolor-core`, `hypercolor-driver-builtin`.

## Key types and traits

**Protocol abstraction**

- `Protocol`, `ProtocolCommand`, `ProtocolResponse`, `ProtocolZone`,
  `ResponseStatus`, `ProtocolError` — implement `Protocol` to add a new
  driver family.
- `ProtocolDatabase` — static device registry; maps `(vid, pid)` pairs to
  `DeviceDescriptor` records.
- `DeviceDescriptor`, `ProtocolBinding`, `ProtocolFactory`, `TransportType`
  — metadata used to register and instantiate protocol drivers.

**Transport layer**

- `Transport`, `TransportError` — unified transport trait.
- Implementations in `transport/`: `hid`, `hidraw`, `hidapi`, `bulk`,
  `control`, `serial`, `midi`, `smbus`, `vendor`.

**Configuration and attachment**

- `ProtocolRuntimeConfig`, `runtime_config_for_attachment_profile` —
  per-device runtime configuration derived from attachment profiles.
- `effective_attachment_slots`, `normalize_attachment_profile_slots` —
  attachment profile slot utilities.

**SMBus enumeration**

- `SmBusProbe`, `SmBusProbeError`, `build_smbus_protocol`,
  `probe_smbus_devices_in_root`, `ASUS_AURA_SMBUS_PROTOCOL_ID` —
  SMBus enumeration helpers. On Windows, these route through
  `hypercolor-windows-pawnio`; on Linux, through `i2cdev`.

## Driver families

| Family | Variants covered |
|---|---|
| `razer` | Keyboard, mouse, mousepad, laptop, peripheral; CRC-validated USB protocol |
| `asus` | USB Aura, SMBus Aura; motherboard RGB header enumeration |
| `corsair` | Lighting Node, iCUE LINK, LCD display, Bragi (NXP + legacy) |
| `lianli` | ENE and TL controller variants; legacy and common helpers |
| `dygma` | Defy/Raise firmware protocol |
| `nollie` | Gen1/Gen2 USB, NOS2, Stream65, serial variant |
| `prismrgb` | PrismRGB LED controller |
| `qmk` | QMK firmware RGB protocol |
| `push2` | Ableton Push 2: separate LED palette and JPEG display lanes |

## Feature flags

None. The Windows/Linux transport split is handled by
`[target.'cfg(...)'.dependencies]`, not by feature flags.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux. Licensed under Apache-2.0.
