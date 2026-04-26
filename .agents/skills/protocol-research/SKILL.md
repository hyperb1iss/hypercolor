---
name: protocol-research
version: 1.0.0
description: >-
  This skill should be used when researching device protocols before implementing
  drivers. Triggers on "reverse engineer protocol", "research device", "find
  protocol docs", "USB capture", "Wireshark USB", "how does this device work",
  "capture USB traffic", "document wire format", "write a protocol spec",
  "what protocol does this use", "add support for new device", "new device
  driver", or any pre-implementation research for crates/hypercolor-hal/ drivers.
---

# Protocol Research for Hypercolor Drivers

Research methodology for understanding device protocols before implementation. Every driver starts here — implementation without research produces broken, incomplete drivers.

## Research Phase Output

A completed research phase produces a **spec document** in `docs/specs/` containing:

1. Device identification (VID/PID, firmware versions, variants)
2. Transport type (HID, bulk, control, SMBus)
3. Packet layout diagrams (byte-by-byte with field names)
4. Command vocabulary (init, color, commit, firmware query)
5. Timing requirements (inter-packet delays, frame intervals)
6. Color byte ordering (RGB? RBG? BGR?)
7. Checksum/CRC algorithms
8. Topology (LED counts, zones, addressing)
9. Variant matrix (which models use which protocol version)
10. Known quirks and platform-specific behavior

## Research Sources

**USB traffic captures are the primary source of truth.** Capture the vendor's own software communicating with the device — this is the definitive reference for packet layouts, timing, and byte ordering.

Community protocol documentation (wikis, blog posts, forum threads) and open-source RGB projects (liquidctl, openrazer, etc.) can provide additional context and save time, but always verify against captures. Write clean Hypercolor implementations using our own architecture — never copy code from other projects.

| Source                                            | Value                   | Notes                                                                |
| ------------------------------------------------- | ----------------------- | -------------------------------------------------------------------- |
| **Vendor's Windows/macOS software** (USB capture) | Ground truth            | Use Wireshark + USBPcap or usbmon                                    |
| **Community protocol docs**                       | Context                 | Wikis, blogs, forum RE threads                                       |
| **Open-source RGB ecosystem**                     | Reference               | liquidctl, openrazer, and others document protocol details           |
| **Reddit/Discord**                                | Firmware tables         | Community-maintained compatibility lists                             |
| **FCC filings**                                   | Hardware identification | VID/PID, chipset info                                                |
| **Vendor firmware changelogs**                    | Protocol changes        | "Fixed LED control" = protocol change                                |
| **Existing Hypercolor drivers**                   | Best starting point     | If a similar device family already has a driver, start from our code |

## USB Traffic Capture Workflow

1. **Set solid red, capture, then set solid green** — diffing these two captures isolates exactly which bytes carry color data and reveals byte ordering (RGB vs RBG vs BGR)
2. **Identify checksum bytes** — bytes that change between the two captures but aren't in color positions. XOR the full packets to spotlight them
3. **Verify color byte ordering** — red capture should show `0xFF` in R positions and `0x00` in G/B; green capture inverts this. If R and B swap, the device uses RBG or BGR
4. **Note inter-packet timing** — capture timestamps reveal required `post_delay` values between commands

## Transport vs Transfer Types

When studying any protocol implementation, note that a single transport call maps to TWO things in Hypercolor:

- **`TransportType`** (registry.rs) — device-level transport binding, set once in `DeviceDescriptor`. Determines how the backend opens and talks to the device (e.g., `UsbControl`, `UsbHidApi`, `UsbHidRaw`, `UsbBulk`, `I2cSmBus`).
- **`TransferType`** (protocol.rs) — per-command path hint on `ProtocolCommand`. Allows a single protocol to mix transfer paths within one device session (e.g., HID feature reports for init, bulk for frame data). Variants: `Primary`, `Bulk`, `HidReport`.

| Protocol Pattern                           | Hypercolor Equivalent                                                 |
| ------------------------------------------ | --------------------------------------------------------------------- |
| Fixed-size byte buffer with manual offsets | Zerocopy struct with `report_id: u8` field                            |
| HID feature report send                    | `TransportType::UsbHidApi` or `UsbHidRaw` + `TransferType::HidReport` |
| USB control transfer                       | `TransportType::UsbControl` + `TransferType::Primary`                 |
| HID interrupt write                        | `TransportType::UsbHid` + `TransferType::Primary`                     |
| Per-LED color loop with count mismatch     | `normalize_colors() -> Cow<'a, [[u8; 3]]>`                            |
| Sleep/delay between commands               | `post_delay: Duration::from_millis(N)`                                |
| Read response after command                | `expects_response: true` + `parse_response()`                         |

## Spec Document Format

Follow the conventions and required sections defined in **`references/spec-conventions.md`**. Use existing specs as templates:

| Spec                              | Best Template For                                                    |
| --------------------------------- | -------------------------------------------------------------------- |
| `17-razer-protocol-driver.md`     | Multi-version protocols, CRC algorithms                              |
| `19-lian-li-uni-hub-driver.md`    | Multi-variant devices, dual transport types, firmware disambiguation |
| `24-asus-aura-protocol-driver.md` | Runtime topology discovery, large device databases                   |

## Firmware Disambiguation

Some devices share a PID but use different protocols based on firmware version. This requires a **firmware predicate** — a function on `DeviceDescriptor` that inspects the device's firmware string before committing to a protocol.

**Methodology** (see spec 19 section 11 for a worked example):

1. **Query firmware** — send a firmware-read command during init. Parse the version from the response.
2. **Define predicate** — set `firmware_predicate: Some(|fw| ...)` on the `DeviceDescriptor`. The registry tries each descriptor for a given VID/PID; the first whose predicate matches (or has no predicate) wins.
3. **Register multiple descriptors per PID** — one per firmware range, each binding a different `ProtocolFactory`.
4. **Document the matrix** — the spec must include a variant table showing PID + firmware range + protocol mapping.

## Topology Documentation

For each device variant, document:

- Total LED count per zone
- Zone addressing scheme (linear, matrix, ring)
- Physical layout (fan ring inner/outer, strip segments, matrix rows/cols)
- Whether zone count is firmware-reported or hardcoded

## Detailed References

- **`references/research-methodology.md`** — Full protocol research workflow: sources, USB capture techniques, C++ to Rust translation patterns, common pitfalls
- **`references/spec-conventions.md`** — Our spec numbering, section format, and documentation standards
