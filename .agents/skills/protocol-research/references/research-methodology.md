# Protocol Research Methodology

How to research and document device protocols for Hypercolor driver development. The vendor's own software + USB captures are the ground truth. Community protocol documentation, wikis, and open-source projects in the RGB ecosystem can provide additional context.

## Research Sources (Priority Order)

1. **USB traffic captures** — Capture the vendor's Windows/macOS software communicating with the device. This is ground truth. See the capture workflow below.
2. **Vendor documentation** — Some vendors publish SDK docs or protocol specs (rare but invaluable when available).
3. **Community protocol documentation** — Wikis, blog posts, forum threads where people have documented wire formats. Search for the device name + "protocol" or "reverse engineer."
4. **Open-source RGB projects** — Projects like liquidctl, openrazer, and others in the RGB ecosystem may have documented protocol details for specific device families. Study their docs and protocol descriptions to understand how the device communicates. Always write clean Hypercolor implementations using our own architecture.
5. **Existing Hypercolor drivers** — If a similar device family already has a driver (e.g., adding a new Razer variant when `razer/protocol.rs` exists), start from our own code.

## USB Traffic Capture Workflow

1. **Set solid red, capture, then set solid green** — diffing these two captures isolates exactly which bytes carry color data and reveals byte ordering (RGB vs RBG vs BGR)
2. **Identify checksum bytes** — bytes that change between the two captures but aren't in color positions. XOR the full packets to spotlight them
3. **Verify color byte ordering** — red capture should show `0xFF` in R positions and `0x00` in G/B; green capture inverts this. If R and B swap, the device uses RBG or BGR
4. **Note inter-packet timing** — capture timestamps reveal required `post_delay` values between commands
5. **Capture the init sequence** — record what happens when the vendor software first opens the device (before setting any colors). This reveals firmware queries, mode switches, and topology discovery
6. **Capture shutdown** — what the software sends when it exits (restores hardware control mode)

## What to Document

For each protocol, produce a spec in `docs/specs/` covering:

- **Packet layouts** — byte-by-byte diagrams for each command type
- **Command vocabulary** — init, firmware query, color data, commit, brightness, shutdown
- **Color encoding** — byte order (RGB/RBG/BGR), max LEDs per packet, packing format
- **Checksums** — algorithm, which bytes are covered, verification examples
- **Timing** — inter-packet delays, frame intervals, response timeouts
- **Topology** — LED counts per zone, addressing scheme (linear/matrix/ring), variant differences
- **Firmware variants** — which firmware versions use which protocol, how to detect

## C++ → Rust Translation Patterns

When studying open-source protocol implementations (in any language), these patterns map to Hypercolor's architecture:

**Transport vs Transfer:** A protocol reference's transport call determines TWO things in Hypercolor:

- **`TransportType`** (registry.rs) — device-level transport binding, set once in `DeviceDescriptor`. Determines how the backend opens and talks to the device (e.g., `UsbControl`, `UsbHidApi`, `UsbHidRaw`, `UsbBulk`, `I2cSmBus`).
- **`TransferType`** (protocol.rs) — per-command path hint on `ProtocolCommand`. Allows a single protocol to mix transfer paths within one device session (e.g., HID feature reports for init, bulk for frame data). Variants: `Primary`, `Bulk`, `HidReport`.

| Source Pattern                             | Hypercolor Equivalent                                                                                  |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| Fixed-size byte buffer with manual offsets | Zerocopy struct with `report_id: u8` field                                                             |
| HID feature report send                    | `TransportType::UsbHidApi` or `UsbHidRaw` + `TransferType::HidReport`                                  |
| USB control transfer                       | `TransportType::UsbControl` + `TransferType::Primary`                                                  |
| HID interrupt write                        | `TransportType::UsbHid` + `TransferType::Primary`                                                      |
| Per-LED color loop with count mismatch     | `normalize_colors() -> Cow<'a, [[u8; 3]]>` — borrow when LED count matches, allocate only when padding |
| Sleep/delay between commands               | `post_delay: Duration::from_millis(N)`                                                                 |
| Read response after command                | `expects_response: true` + `parse_response()`                                                          |

## Common Pitfalls

| Pattern                       | Pitfall                      | Correct Hypercolor Translation                    |
| ----------------------------- | ---------------------------- | ------------------------------------------------- |
| `RGBGetRValue(color)`         | Assumes RGB ordering         | Check actual byte positions — may be RBG or BGR   |
| HID write with `len+1`        | +1 includes report ID        | Include report ID in zerocopy struct              |
| `usleep(1000)`                | Units are microseconds       | `Duration::from_micros(1000)` (= 1ms)             |
| HID get feature report        | Blocks until response        | `expects_response: true` on preceding command     |
| `sizeof(buf)`                 | Includes report ID byte      | Compile-time assertion must match total wire size |
| Magic numbers at byte offsets | Undocumented, easy to mismap | Define named constants for every offset           |

## Verifying Your Understanding

Always verify your protocol understanding against actual USB traffic from the vendor's software:

1. Run the vendor's Windows/macOS software
2. Capture with Wireshark + USBPcap (Windows) or usbmon (Linux)
3. Compare captured packets byte-by-byte with your spec
4. Pay special attention to: byte ordering, checksum algorithms, timing between packets

The vendor's software is always ground truth. Community documentation and open-source implementations may have bugs, be incomplete, or cover different firmware versions.
