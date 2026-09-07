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
11. Which integration surface the device needs (see below)

## HAL Descriptor or Driver Module?

Decide this during research, not after the encoder is written. A device registered through a `hypercolor-hal` `DeviceDescriptor` lands in a driver module whose capability set is `protocol_catalog: true` and everything else `false`. That buys wire-format encoding on the shared USB output backend and nothing more.

If the research turns up any of the following, the device needs a real `DriverModule` in `hypercolor-driver-api` rather than a HAL descriptor, and the spec should say so:

- Driver-scoped configuration the user has to set (a bridge address, an API host)
- A pairing, authorization, or button-press handshake
- Active discovery (mDNS, SSDP, a vendor broadcast) rather than VID/PID enumeration
- Stored credentials or tokens
- Presentation metadata or dynamic control surfaces

See `docs/specs/51-unified-driver-module-api.md` and `docs/specs/52-dynamic-driver-control-surfaces.md`. The network drivers (`hypercolor-driver-hue`, `-nanoleaf`, `-wled`, `-govee`) are the worked examples; every USB/HID/SMBus family in `hypercolor-hal` is the other case.

Separately, note that a supported device also needs an entry in `data/drivers/vendors/<vendor>.toml` (VID, PID, type, status, driver, transport) so `just compat` can regenerate the public compatibility matrix. That file is hand-maintained, not derived from the HAL database.

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

### Windows USBPcap + CDC ACM Notes

When sniffing USB serial devices on Windows, prefer Wireshark's `usbcom`
dissector fields over raw `usb.capdata` once the capture is decoded:

```powershell
tshark -r capture.pcapng `
  -Y "usb.device_address == <addr> && (usbcom.data.out_payload || usbcom.data.in_payload)" `
  -T fields `
  -e frame.number -e frame.time_relative -e usb.src -e usb.dst `
  -e usb.endpoint_address -e usbcom.data.out_payload -e usbcom.data.in_payload
```

Decode the hex payloads to ASCII for a clean bidirectional serial transcript.
This avoids parsing USBPcap headers by hand and filters out noisy unrelated USB
traffic. If `dumpcap -D` omits USB interfaces, `tshark -D` may still show
`\\.\USBPcap1`; USBPcap extcap config requires the literal interface path:

```powershell
& "C:\Program Files\Wireshark\extcap\USBPcapCMD.exe" `
  --extcap-config --extcap-interface "\\.\USBPcap1"
```

All-device USBPcap captures can become hundreds of MB in under a minute on RGB
systems. Capture raw pcaps locally, but commit decoded transcripts and SHA256
receipts instead of large `.pcapng` files. For Cinder/OpenGL-style vendor apps
with poor UI Automation support, start the capture first and let the human drive
the UI slowly with 2-3 second gaps; UAC/focus changes can close transient tool
windows and ruin automated click sequences.

## Transport vs Transfer Types

When studying any protocol implementation, note that a single transport call maps to TWO things in Hypercolor:

- **`TransportType`** (registry.rs) — device-level transport binding, resolved once per `DeviceDescriptor`. Determines how the backend opens and talks to the device (e.g., `UsbControl`, `UsbHidApi`, `UsbHidRaw`, `UsbBulk`, `I2cSmBus`). HID descriptors do not name one directly: they declare a platform-free `TransportIntent` and let `resolve_current_transport` pick per target OS.
- **`TransferType`** (protocol.rs) — per-command path hint on `ProtocolCommand`. Allows a single protocol to mix transfer paths within one device session (e.g., HID feature reports for init, bulk for frame data). Variants: `Primary`, `Bulk`, `HidReport`.

| Protocol Pattern                           | Hypercolor Equivalent                                                            |
| ------------------------------------------ | -------------------------------------------------------------------------------- |
| Fixed-size byte buffer with manual offsets | Zerocopy struct; whether it carries a `report_id` field depends on the transport (see below) |
| HID feature report send                    | `TransportIntent::Hid(HidTransportIntent { report_mode: FeatureReport, .. })` + `TransferType::HidReport` |
| USB control transfer                       | `TransportType::UsbControl` + `TransferType::Primary`                            |
| HID interrupt write                        | `HidAccessMode::Direct` (resolves to `UsbHid` on Linux) + `TransferType::Primary` |
| Per-LED color loop with count mismatch     | a private per-driver `normalize_colors(&self, ..) -> Cow<'a, [[u8; 3]]>` method  |
| Sleep/delay between commands               | `post_delay: Duration::from_millis(N)`                                           |
| Read response after command                | `expects_response: true` + `parse_response()`                                    |

Note the report ID row. "The capture shows a leading report ID byte" never by itself decides whether the packet struct carries one; the transport the descriptor resolves to decides it. Only `UsbHidApi` and `UsbHidRaw` have a `report_mode` field, and only those two prepend `report_id` (in `encode_hidapi_packet` and `encode_hidraw_packet` respectively), and only when the mode is not `FeatureReportWithReportId` or `OutputReportWithReportId`. Every other transport sends the payload verbatim. `UsbControl` and `UsbBulk` pass the report ID in the control-transfer `wValue` instead, so their structs stay clean; `UsbHid`, `UsbMidi`, `UsbSerial`, `I2cSmBus`, and `UsbVendor` add nothing, so there the struct must carry the byte itself. Lian Li ENE is the case to check yourself against: `UsbHid` descriptors, and every ENE packet struct starts with `report_id`.

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
2. **Define predicate** — set `firmware_predicate: Some(named_predicate)` on the `DeviceDescriptor`, where `named_predicate` is a local `fn(&str) -> bool`.
3. **Register multiple descriptors per PID** — one per firmware range, each binding a different `ProtocolFactory`.
4. **Leave one descriptor predicate-free** — this is the part that is easy to miss. `ProtocolDatabase::lookup_with_firmware_for_driver_ids` only consults predicates when a firmware string is already known. With no firmware in hand it picks the first candidate that has **no** predicate, and only falls back to the first candidate overall if every one carries a predicate. A PID whose descriptors all have predicates will bind arbitrarily on first discovery.
5. **Expect the enabled-driver filter** — when the caller passes `enabled_driver_ids`, descriptors whose `driver_id()` is absent from that set are skipped at every stage. A device can therefore go unmatched purely because its driver module is disabled in config.
6. **Document the matrix** — the spec must include a variant table showing PID + firmware range + protocol mapping.

## Topology Documentation

For each device variant, document:

- Total LED count per zone
- Zone addressing scheme (linear, matrix, ring)
- Physical layout (fan ring inner/outer, strip segments, matrix rows/cols)
- Whether zone count is firmware-reported or hardcoded

## Detailed References

- **`references/research-methodology.md`** — Full protocol research workflow: sources, USB capture techniques, C++ to Rust translation patterns, common pitfalls
- **`references/spec-conventions.md`** — Our spec numbering, section format, and documentation standards
