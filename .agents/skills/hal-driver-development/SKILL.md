---
name: hal-driver-development
version: 1.0.0
description: >-
  This skill should be used when writing, porting, or debugging device drivers
  in hypercolor-hal. Triggers on "add a driver", "port a driver", "implement
  protocol", "device not working", "wire format", "encode frame", "USB HID
  packet", "zerocopy struct", "CommandBuffer", "device database entry",
  "transport type", "frame encoding", "protocol implementation", "add device
  support", or any work in crates/hypercolor-hal/.
---

# Hypercolor HAL Driver Development

## Architecture Boundary

`hypercolor-hal` must never depend on `hypercolor-core` — that would create a circular dependency (`core` depends on `hal`). Key dependencies: `hypercolor-types`, `nusb`, `zerocopy`, `hidapi`, `tokio`, `tokio-serial`, `midir`, `image`, `thiserror`, `tracing`, and Linux-only `async-hid`, `i2cdev`.

## Protocol Trait Contract

Every driver implements `Protocol` (in `src/protocol.rs`). Key methods:

- `name()` → human-readable protocol name (`&'static str`)
- `init_sequence()` → commands sent on device connect (mode switch, firmware probe)
- `shutdown_sequence()` → graceful release (restore hardware control)
- `encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand>` — convenience wrapper
- `encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>)` — **prefer this** — reuses the command vector across frames (zero-alloc hot path)
- `encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>>` — hardware brightness control
- `encode_display_frame_into(&self, jpeg_data: &[u8], commands: &mut Vec<ProtocolCommand>) -> Option<()>` — pixel display frame encoding (Corsair LCD, Push 2)
- `parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError>` — device replies
- `connection_diagnostics()` → optional one-shot verification commands for write-only devices
- `keepalive()` → returns `Option<ProtocolKeepalive>` (commands + interval) for devices that need periodic traffic to stay in direct mode
- `keepalive_commands()` → resolves the command sequence for a keepalive tick (override for stateful keepalives)
- `response_timeout()` → budget for commands expecting a reply (default 1s)
- `zones()` → physical LED zones for spatial mapping
- `capabilities()` → what the device supports
- `total_leds()` → LED count (determines color slice length)
- `frame_interval()` → target frame timing

**Always implement `encode_frame_into`**. The default `encode_frame` allocates a new Vec per frame — fine for tests, terrible at 60 FPS.

## ProtocolCommand Structure

Each command carries metadata for the transport layer:

```rust
ProtocolCommand {
    data: Vec<u8>,
    expects_response: bool,      // read after sending?
    response_delay: Duration,    // pause before reading
    post_delay: Duration,        // pause after operation
    transfer_type: TransferType, // Primary | Bulk | HidReport
}
```

`transfer_type` tells the transport *how* to send — some devices mix HID feature reports for commands with bulk transfers for color data (Corsair LINK), or feature reports for commands with output reports for colors (Lian Li).

## CommandBuffer API

`CommandBuffer::new(commands)` wraps a `&mut Vec<ProtocolCommand>` for zero-alloc frame encoding:

```rust
let mut buffer = CommandBuffer::new(commands);
buffer.push_struct(&my_packet, false, Duration::ZERO, COMMAND_DELAY, TransferType::HidReport);
// push_fill takes a FnOnce(&mut Vec<u8>) closure — write directly into the reusable buffer
buffer.push_fill(false, Duration::ZERO, Duration::ZERO, TransferType::Primary, |buf| {
    buf.resize(65, 0x00);
});
// push_slice is a convenience wrapper over push_fill
buffer.push_slice(&raw_bytes, false, Duration::ZERO, Duration::ZERO, TransferType::Primary);
buffer.finish(); // truncates to actual used count
```

`push_struct` writes any `IntoBytes + Immutable` struct directly — no intermediate `Vec<u8>`.
`push_fill` signature: `push_fill(expects_response, response_delay, post_delay, transfer_type, FnOnce(&mut Vec<u8>))`.

## Zerocopy Wire-Format Structs

**Mandatory pattern** for all fixed-size protocol packets:

```rust
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct MyPacket {
    report_id: u8,
    command: u8,
    data: [u8; 62],
}

const _: () = assert!(
    std::mem::size_of::<MyPacket>() == 64,
    "MyPacket must match wire size"
);
```

Rules:
- `#[repr(C)]` is **required** — without it, Rust reorders fields
- Compile-time size assertion is **mandatory** for every packet struct
- `FromZeros + IntoBytes` for write-only packets (most frame encoding)
- `FromBytes + IntoBytes` for packets also parsed from responses
- **Never derive both `FromBytes` and `FromZeros`** — `FromBytes` implies `FromZeros`, dual derive causes `E0119`
- Use `read_from_prefix()` not `read_from_bytes()` for parsing — HID transports may return larger buffers (extra report ID byte)
- Multi-byte fields: `zerocopy::byteorder::{LittleEndian, U16}` for wire endianness

## Color Slice Normalization

Effects produce variable-length color slices. Protocols expect exact LED counts. Use `Cow` to avoid allocation when lengths match:

```rust
fn normalize_colors<'a>(colors: &'a [[u8; 3]], expected: usize) -> Cow<'a, [[u8; 3]]> {
    if colors.len() == expected {
        Cow::Borrowed(colors)
    } else {
        let mut padded = colors.to_vec();
        padded.resize(expected, [0, 0, 0]);
        Cow::Owned(padded)
    }
}
```

## Device Database Registration

`src/database.rs` holds a static `LazyLock<Vec<DeviceDescriptor>>`. Each driver module exposes a `descriptors()` function:

```rust
pub fn descriptors() -> &'static [DeviceDescriptor] {
    static DESCRIPTORS: &[DeviceDescriptor] = &[DeviceDescriptor {
        vendor_id: 0x1234,
        product_id: 0x5678,
        name: "My Device",
        family: DeviceFamily::MyFamily,
        transport: TransportType::UsbHid { interface: 0 },
        protocol: ProtocolBinding {
            id: "myvendor/mydevice",
            build: || Box::new(MyProtocol::new()),
        },
        firmware_predicate: None, // or Some(|fw| firmware_matches(fw, "2.0"))
    }];
    DESCRIPTORS
}
```

**Firmware predicates** disambiguate same-PID devices with different protocols (e.g., Lian Li AL firmware 1.7 uses HID, older uses vendor control).

## Known Wire Format Gotchas

| Device Family | Gotcha |
|--------------|--------|
| Lian Li | Color byte order is **R-B-G**, not RGB |
| Lian Li AL | Dual-ring addressing: `port = group * 2 + ring` (inner fan vs outer edge) |
| Lian Li V2 | Output report sizes differ: SL V2/AL V2 = 353 bytes, others = 146 or 98 |
| Razer | XOR checksum covers bytes `[2..88)` (bytes 2 through 87 inclusive, 86 bytes) |
| Razer | 6 protocol versions — transaction_id byte selects version (0xFF/0x3F/0x1F/0x9F) |
| Razer | 4 custom effect activation styles across device generations |
| ASUS | Runtime topology discovery via `RwLock` interior mutability in `parse_response()` |
| ASUS | Board-specific firmware overrides for 18+ known boards |
| Corsair LN | Components sent separately (R, then G, then B) — 50 LEDs per packet |
| Corsair LINK | 16-bit LE length-prefixed framing (513-byte packets) |
| Report IDs | Some platforms include report ID in buffer, others strip it — always check |

## Multi-Phase Update Patterns

Some devices require sequenced commands per frame:

- **Lian Li ENE**: Activate → Color data (per port) → Commit
- **Razer Extended**: Color chunks (22 LEDs/row max) → Custom effect activation
- **Corsair LN**: Per-channel R/G/B components → Commit packet
- **ASUS**: Direct color chunks (20 RGB triples/packet) → Apply flag on final chunk

## Transport Selection Guide

| Transport | Associated Data | When to Use | File |
|-----------|----------------|-------------|------|
| `UsbControl` | `{ interface, report_id }` | HID feature reports via control transfers (Razer) | `transport/control.rs` |
| `UsbHid` | `{ interface }` | HID interrupt endpoints (QMK, PrismRGB) | `transport/hid.rs` |
| `UsbHidApi` | `{ interface?, report_id, report_mode, usage_page?, usage? }` | Cross-platform `hidapi` access for live input devices (mice, keyboards) | `transport/hidapi.rs` |
| `UsbHidRaw` | `{ interface, report_id, report_mode, usage_page?, usage? }` | Linux `/dev/hidraw*` direct access (Lian Li) | `transport/hidraw.rs` |
| `UsbBulk` | `{ interface, report_id }` | Bulk endpoints + HID feature sideband (Corsair LINK) | `transport/bulk.rs` |
| `UsbMidi` | `{ midi_interface, display_interface, display_endpoint }` | MIDI control + bulk display (Ableton Push 2) | `transport/midi.rs` |
| `I2cSmBus` | `{ address }` | I2C/SMBus on motherboard (ASUS ENE) | `transport/smbus.rs` |
| `UsbVendor` | (none) | Vendor-specific control transfers (Lian Li legacy) | `transport/vendor.rs` |
| `UsbSerial` | `{ baud_rate }` | USB CDC-ACM serial (Dygma) | `transport/serial.rs` |

## New Protocol Checklist

1. [ ] Zerocopy packet structs with compile-time size assertions
2. [ ] `encode_frame_into` implemented (not just `encode_frame`)
3. [ ] `CommandBuffer` with `push_struct` — never build `Vec<ProtocolCommand>` with fresh allocs per frame
4. [ ] `Cow` normalization for color input slice
5. [ ] `connection_diagnostics()` implemented for write-only devices (verifies device accepts commands)
6. [ ] `keepalive()` implemented if device exits direct mode on idle (returns commands + interval)
7. [ ] Device descriptors registered in `database.rs`
8. [ ] Tests for encoding without hardware (`tests/` directory)
9. [ ] Spec document in `docs/specs/`
10. [ ] Frame interval matches device refresh rate

## Detailed References

- **`references/protocol-implementation.md`** — Full Protocol impl walkthrough with annotated examples from Razer and Lian Li
- **`references/zerocopy-wire-formats.md`** — Detailed zerocopy patterns, response parsing, multi-byte fields, and platform-specific report ID handling
