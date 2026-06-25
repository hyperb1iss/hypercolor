+++
title = "Adding a HAL driver"
description = "The Protocol contract, zerocopy structs, CommandBuffer, encode_frame_into, device-DB entry, and tests for a new USB HAL driver."
weight = 30
+++

# Adding a HAL driver

This page walks through adding a USB HID device driver to `hypercolor-hal`, from protocol
research through a finished, tested implementation. The guide uses the Razer driver as its
reference implementation; the exact files live in
`crates/hypercolor-hal/src/drivers/razer/`.

---

## Architecture recap

Before touching code, know where each piece lives.

`hypercolor-hal` contains pure protocol encoding and the static device database. It has no
engine dependency, no async I/O, and critically no `hypercolor-core` import. The dependency
arrow is strictly `hypercolor-types` → `hypercolor-hal` → `hypercolor-core`. Breaking this
direction creates a circular dependency that will not compile.

`hypercolor-core` owns the `UsbBackend` adapter that wraps a `Protocol` and `Transport` pair
and implements the engine's `DeviceBackend` trait. You do not touch it for a new driver unless
you are adding a new transport type.

Driver modules are named after silicon or OEM family, not brand. The `lianli` module covers
ENE and TL hubs; rebranded SKUs are entries in `devices.rs`, not new module directories.
Follow the same rule: create `crates/hypercolor-hal/src/drivers/<silicon_family>/` and put
every device that shares the wire format there.

---

## Step 1: Protocol research

Before writing a single packet struct, you need a reliable source of truth for the device's
wire format.

1. Capture USB traffic from vendor software using Wireshark with `usbmon` on Linux or USBPcap
   on Windows. Set solid red, then solid green, and diff the two captures to isolate the color
   bytes and reveal byte ordering.
2. Cross-reference with community projects (OpenRGB source, liquidctl, device-specific GitHub
   repos) for additional context.
3. Write a spec in `docs/specs/` before implementing. The spec is the review artifact; the
   Rust code follows it.

{% callout(type="warning") %}
Do not start implementing from a partial spec. An 80% correct spec produces subtle
encoding bugs that are hard to bisect later because everything compiles clean.
{% end %}

The `.agents/skills/protocol-research/` skill documents the full research methodology,
including USB capture workflow, checksum identification, and timing measurement.

---

## Step 2: Create the driver module

Add your driver under `crates/hypercolor-hal/src/drivers/<family>/`:

```
crates/hypercolor-hal/src/drivers/myfamily/
├── mod.rs          # pub use re-exports
├── protocol.rs     # Protocol impl
├── types.rs        # enums, constants, command IDs
└── devices.rs      # descriptors() + ProtocolBinding factories
```

Declare the module in `crates/hypercolor-hal/src/drivers/mod.rs`:

```rust
pub mod myfamily;
```

---

## Step 3: Define wire-format structs with zerocopy

Every fixed-size USB packet must be a `#[repr(C)]` struct with `zerocopy` derives. This is
mandatory, not optional. Manual offset indexing produces silent misalignment bugs and is
rejected in review.

```rust
use zerocopy::{FromZeros, IntoBytes, KnownLayout, Immutable};

/// 64-byte command packet for My Family devices.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub(super) struct MyFamilyPacket {
    pub report_id: u8,
    pub command:   u8,
    pub channel:   u8,
    pub led_count: u8,
    pub rgb_data:  [u8; 60],
}

const _: () = assert!(
    std::mem::size_of::<MyFamilyPacket>() == 64,
    "MyFamilyPacket must match wire size exactly"
);
```

Rules to follow exactly:

- `#[repr(C)]` is required. Without it, Rust may reorder fields to optimize alignment.
- The compile-time size assertion is mandatory for every packet struct.
- Use `FromZeros + IntoBytes` for write-only packets, which covers most frame encoding paths.
- Use `FromBytes + IntoBytes` for structs that are also parsed from device responses.
  `FromBytes` implies `FromZeros`, so do not derive both; the dual derive causes `E0119`.
- Multi-byte little-endian fields: use `zerocopy::byteorder::U16<LittleEndian>` rather
  than `u16`, so endianness is enforced at the type level and not patched in at runtime with
  `.to_le_bytes()`.
- For response parsing, use `read_from_prefix()` rather than `read_from_bytes()`. HID
  transports may return oversized buffers with a leading report ID byte;
  `read_from_prefix` handles this safely.

The Razer `RazerReport` struct in `crates/hypercolor-hal/src/drivers/razer/crc.rs` is the
canonical 90-byte example. It derives `FromBytes + IntoBytes + KnownLayout + Immutable`
because it is both written and parsed. The `razer_crc` helper XORs bytes `[2..88]` and
stores the result at offset `88`. The checksum derivation is verified against known-good
hardware captures.

---

## Step 4: Implement the Protocol trait

The `Protocol` trait (in `crates/hypercolor-hal/src/protocol.rs`) is the core contract.
All methods are synchronous and infallible for encoding; errors only surface during response
parsing. The trait is `Send + Sync`, so implementations must not hold non-Send state.

```rust
use std::borrow::Cow;
use std::time::Duration;

use hypercolor_types::device::{
    DeviceCapabilities, DeviceColorFormat, DeviceColorSpace,
    DeviceFeatures, DeviceTopologyHint,
};
use crate::protocol::{
    CommandBuffer, Protocol, ProtocolCommand, ProtocolError,
    ProtocolResponse, ProtocolZone, ResponseStatus, TransferType,
};

pub struct MyFamilyProtocol {
    led_count: u32,
    channel:   u8,
}

impl MyFamilyProtocol {
    pub fn new(led_count: u32, channel: u8) -> Self {
        Self { led_count, channel }
    }

    fn normalize<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
        let expected = self.led_count as usize;
        if colors.len() == expected {
            Cow::Borrowed(colors)
        } else {
            let mut padded = colors.to_vec();
            padded.resize(expected, [0, 0, 0]);
            Cow::Owned(padded)
        }
    }
}

impl Protocol for MyFamilyProtocol {
    fn name(&self) -> &'static str {
        "My Family v1"
    }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        // Commands to switch device into software-control mode.
        // Return Vec::new() if the device needs no init.
        Vec::new()
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        // Commands to restore hardware control. Vec::new() if not needed.
        Vec::new()
    }

    // encode_frame is required by the trait.
    // Implement it by delegating to encode_frame_into so that
    // all encoding logic lives in one place.
    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    // encode_frame_into is the hot path; implement this.
    // The default trait implementation delegates to encode_frame (allocating).
    // Override it here to reuse the buffer instead.
    fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
        let normalized = self.normalize(colors);
        let mut buffer = CommandBuffer::new(commands);

        let mut packet = MyFamilyPacket::new_zeroed();
        packet.report_id = 0x00;
        packet.command   = 0x04;
        packet.channel   = self.channel;
        packet.led_count = u8::try_from(normalized.len()).unwrap_or(0);
        for (i, &[r, g, b]) in normalized.iter().enumerate().take(20) {
            let base = i * 3;
            packet.rgb_data[base]     = r;
            packet.rgb_data[base + 1] = g;
            packet.rgb_data[base + 2] = b;
        }

        buffer.push_struct(
            &packet,
            false,          // expects_response
            Duration::ZERO, // response_delay
            Duration::ZERO, // post_delay
            TransferType::Primary,
        );
        buffer.finish();
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        let (packet, _) = MyFamilyPacket::read_from_prefix(data)
            .map_err(|_| ProtocolError::MalformedResponse {
                detail: format!(
                    "expected {} bytes, got {}",
                    std::mem::size_of::<MyFamilyPacket>(),
                    data.len()
                ),
            })?;
        // Map device status byte to ResponseStatus
        let _ = packet; // replace with real status mapping
        Ok(ProtocolResponse { status: ResponseStatus::Ok, data: Vec::new() })
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        vec![ProtocolZone {
            name: "Main".to_owned(),
            led_count: self.led_count,
            topology: DeviceTopologyHint::Strip,
            color_format: DeviceColorFormat::Rgb,
            layout_hint: None,
        }]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: self.led_count,
            supports_direct: true,
            supports_brightness: false,
            has_display: false,
            display_resolution: None,
            max_fps: 60,
            color_space: DeviceColorSpace::default(),
            features: DeviceFeatures::default(),
        }
    }

    fn total_leds(&self) -> u32 {
        self.led_count
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16) // ~60 FPS
    }
}
```

---

## Step 5: `encode_frame_into` and CommandBuffer

`encode_frame_into` is the frame-rendering hot path called at up to 60 FPS. It receives
a `&mut Vec<ProtocolCommand>` and fills it in place, reusing the vector's capacity across
frames. The trait's default `encode_frame_into` implementation delegates to `encode_frame`,
which allocates a fresh `Vec` per call, so always override it.

`CommandBuffer` wraps the mutable vector and provides three writing methods:

```rust
let mut buffer = CommandBuffer::new(commands);

// Write a zerocopy struct directly, no intermediate copy.
buffer.push_struct(
    &packet,
    expects_response,
    response_delay,
    post_delay,
    transfer_type,
);

// Write raw bytes via a closure that fills directly into a reusable Vec<u8>.
buffer.push_fill(
    expects_response,
    response_delay,
    post_delay,
    TransferType::Primary,
    |buf| {
        buf.resize(65, 0x00);
        buf[0] = REPORT_ID;
        // ...
    },
);

// Write a raw byte slice (convenience wrapper over push_fill).
buffer.push_slice(&raw_bytes, expects_response, response_delay, post_delay, transfer_type);

buffer.finish(); // truncate the commands Vec to the actual used count
```

`TransferType` is the per-command transport-path hint with three variants: `Primary` (the
transport's default data path, and the `#[default]`), `Bulk` (a bulk endpoint), and
`HidReport` (HID feature reports over control transfers). Use `Primary` unless the device
mixes transfer types for different commands.

### Color slice normalization

Effects produce variable-length color slices. Protocols expect an exact LED count. Use `Cow`
to avoid allocation when the lengths already match:

```rust
use std::borrow::Cow;

fn normalize<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
    let expected = self.led_count as usize;
    if colors.len() == expected {
        Cow::Borrowed(colors)
    } else {
        let mut padded = colors.to_vec();
        padded.resize(expected, [0, 0, 0]);
        Cow::Owned(padded)
    }
}
```

The Razer driver's `normalize_colors` method in `protocol.rs` uses this pattern and emits a
`tracing::warn!` when a length mismatch occurs. Copy that behavior so mismatches surface in
daemon logs rather than silently producing incorrect colors.

### Multi-phase update patterns

Some device families require a sequenced command burst per frame. Emit all phases in a single
`encode_frame_into` call using multiple `push_struct` calls:

```rust
// Example: Activate → Color data → Commit
buffer.push_struct(
    &activate_packet,
    false, Duration::ZERO, Duration::ZERO,
    TransferType::Primary,
);
buffer.push_struct(
    &color_packet,
    false, Duration::ZERO, Duration::from_millis(1),
    TransferType::Primary,
);
buffer.push_struct(
    &commit_packet,
    true, Duration::from_millis(5), Duration::ZERO,
    TransferType::Primary,
);
buffer.finish();
```

The inter-command `post_delay` values come from USB capture timestamps, not guesswork. Measure
them in your capture and document them in the spec.

---

## Step 6: Wire-format gotchas by family

If your device is closely related to an existing family, check these known quirks before
spending hours bisecting a wrong-color bug:

| Family | Gotcha |
|---|---|
| Lian Li (any) | Color byte order is R-B-G, not RGB |
| Lian Li AL | Dual-ring addressing: `port = group * 2 + ring` (inner fan vs outer edge) |
| Razer | XOR checksum covers bytes `[2..88)`, which is 86 bytes. Six protocol versions keyed by `transaction_id` byte. |
| Razer | Four distinct custom-effect activation styles across device generations |
| ASUS | Runtime topology discovery via `RwLock` interior mutability inside `parse_response()` |
| Corsair LN | Color components sent separately (R, then G, then B); 50 LEDs per packet |
| Corsair LINK | 16-bit LE length-prefixed framing, 513-byte packets |
| Report IDs | Some platforms include the HID report ID in the buffer; others strip it. Always parse with `read_from_prefix`. |

---

## Step 7: Keepalives and connection diagnostics

Some devices exit software-control mode after a period of silence. Implement `keepalive()` to
schedule background traffic:

```rust
fn keepalive(&self) -> Option<ProtocolKeepalive> {
    let mut commands = Vec::new();
    // Push the command that keeps the device in direct mode
    // ...
    Some(ProtocolKeepalive {
        commands,
        interval: Duration::from_secs(5),
    })
}
```

For write-only devices (no response expected during normal frame sends), implement
`connection_diagnostics()` to provide a one-shot verification command the transport can use
to confirm the USB path is live after connect:

```rust
fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
    // A read command that returns a response, typically a firmware query.
    vec![self.build_firmware_query()]
}
```

Both default to empty: only implement them if the device needs them.

---

## Step 8: Register the device in the database

`crates/hypercolor-hal/src/database.rs` holds a `LazyLock<Vec<DeviceDescriptor>>` that maps
USB VID/PID pairs to protocol constructors. Each driver module exposes a `descriptors()`
function returning a static slice:

```rust
// crates/hypercolor-hal/src/drivers/myfamily/devices.rs

use hypercolor_types::device::DeviceFamily;
use crate::registry::{DeviceDescriptor, ProtocolBinding, TransportType};
use super::protocol::MyFamilyProtocol;

pub fn descriptors() -> &'static [DeviceDescriptor] {
    static DESCRIPTORS: &[DeviceDescriptor] = &[
        DeviceDescriptor {
            vendor_id: 0x1234,
            product_id: 0x5678,
            name: "MyBrand Widget Pro",
            family: DeviceFamily::MyFamily,
            transport: TransportType::UsbHid { interface: 0 },
            protocol: ProtocolBinding {
                id: "myfamily/widget-pro",
                build: || Box::new(MyFamilyProtocol::new(30, 0x01)),
            },
            firmware_predicate: None,
        },
        // Additional SKUs are additional entries here, not new modules.
    ];
    DESCRIPTORS
}
```

Then wire it into `database.rs`:

```rust
// In crates/hypercolor-hal/src/database.rs
use crate::drivers::{/* existing families */, myfamily};

static DEVICE_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::new();
    // ... existing extend_from_slice calls ...
    descriptors.extend_from_slice(myfamily::devices::descriptors());
    descriptors
});
```

### Transport type selection

Choose the transport that matches your device's USB transfer type:

| Transport | When to use |
|---|---|
| `UsbControl { interface, report_id }` | HID feature reports over USB control transfers (Razer) |
| `UsbHid { interface }` | HID interrupt endpoints (QMK, PrismRGB) |
| `UsbHidApi { interface?, report_id, report_mode, max_report_len, usage_page?, usage? }` | Cross-platform `hidapi`; use for live input devices (mice, keyboards) to keep the OS HID stack attached |
| `UsbHidRaw { interface, report_id, report_mode, usage_page?, usage? }` | Linux `/dev/hidraw*` direct access (Lian Li) |
| `UsbBulk { interface, report_id }` | Bulk transfers with HID feature-report sideband (Corsair LINK) |
| `UsbMidi { midi_interface, display_interface, display_endpoint }` | MIDI control + bulk display (Ableton Push 2) |
| `UsbSerial { baud_rate }` | USB CDC-ACM serial (Dygma) |
| `I2cSmBus { address }` | Linux I2C/SMBus on motherboard (ASUS ENE) |
| `UsbVendor` | Vendor-specific control transfers (no associated fields) |

### Firmware-gated dispatch

When two devices share a PID but need different protocol parameters, use `firmware_predicate`.
This is common with hub revisions where the manufacturer reused a PID across firmware
generations (Lian Li AL v1.7 vs older):

```rust
DeviceDescriptor {
    vendor_id: 0x0CF2,
    product_id: 0xA101,
    name: "Lian Li Uni Hub AL (v1.7+)",
    family: DeviceFamily::LianLi,
    transport: TransportType::UsbHidRaw { interface: 0, report_id: 0x00,
        report_mode: HidRawReportMode::FeatureReport,
        usage_page: None, usage: None },
    protocol: ProtocolBinding {
        id: "lianli/uni-hub-al",
        build: || Box::new(LianLiProtocol::new(LianLiHubVariant::Al)),
    },
    firmware_predicate: Some(|fw| fw.contains("v1.7")),
},
```

`ProtocolDatabase::lookup_with_firmware` evaluates predicates in order and falls back to a
`None`-predicate entry if no firmware string matches.

### DeviceFamily and protocol IDs

`DeviceFamily` is defined in `hypercolor-types`. If your driver covers genuinely new silicon,
add a variant there. The `ProtocolBinding::id` field uses the format `"<family>/<model-slug>"`,
for example `"razer/huntsman-v2"`. This ID appears in the driver registry, the compatibility
matrix, and daemon telemetry.

---

## Step 9: Write tests

Tests live in `tests/` at the crate level, not in inline `#[cfg(test)]` blocks. The project
convention is strict: external test files only.

Create `crates/hypercolor-hal/tests/myfamily_protocol_tests.rs`:

```rust
use hypercolor_hal::drivers::myfamily::protocol::MyFamilyProtocol;
use hypercolor_hal::protocol::Protocol;

/// Encoding a full-brightness white frame produces the expected byte layout.
#[test]
fn encode_frame_white_produces_correct_packet() {
    let protocol = MyFamilyProtocol::new(4, 0x01);
    let colors = vec![[255, 255, 255]; 4];
    let commands = protocol.encode_frame(&colors);

    assert_eq!(commands.len(), 1, "expected one command for a 4-LED frame");
    let data = &commands[0].data;
    assert_eq!(data[0], 0x00, "report_id should be 0");
    assert_eq!(data[1], 0x04, "command byte should be 0x04");
    assert_eq!(data[3], 4,    "led_count should match");
    assert_eq!(&data[4..7], &[255, 255, 255], "first LED should be white");
}

/// Short color slices are zero-padded to the expected LED count.
#[test]
fn encode_frame_pads_short_input() {
    let protocol = MyFamilyProtocol::new(8, 0x01);
    let colors = vec![[255, 0, 0]; 4]; // only half the LEDs provided
    let commands = protocol.encode_frame(&colors);
    assert!(!commands.is_empty());
    let data = &commands[0].data;
    // LED at index 7 should be black
    let last_led_offset = 4 + 7 * 3;
    assert_eq!(&data[last_led_offset..last_led_offset + 3], &[0, 0, 0]);
}

/// encode_frame_into reuses an existing Vec without reallocating.
#[test]
fn encode_frame_into_reuses_buffer() {
    let protocol = MyFamilyProtocol::new(4, 0x01);
    let mut commands = Vec::with_capacity(2);
    let colors = vec![[0, 128, 255]; 4];

    protocol.encode_frame_into(&colors, &mut commands);
    let first_ptr = commands.as_ptr();

    commands.clear();
    protocol.encode_frame_into(&colors, &mut commands);

    // Pointer should be stable when capacity was sufficient.
    assert_eq!(commands.as_ptr(), first_ptr, "buffer should not have reallocated");
}

/// A registered VID/PID resolves to the correct protocol via the database.
#[test]
fn protocol_database_resolves_myfamily_device() {
    use hypercolor_hal::database::ProtocolDatabase;
    let descriptor = ProtocolDatabase::lookup(0x1234, 0x5678);
    assert!(descriptor.is_some(), "device should be in the database");
    let protocol = (descriptor.unwrap().protocol.build)();
    assert_eq!(protocol.total_leds(), 30);
}
```

The full test checklist from the HAL development skill:

- Round-trip encoding: build commands and verify byte layout matches reference captures
- CRC or checksum validation against known-good test vectors
- Color format reordering if the wire order differs from RGB (Lian Li uses R-B-G)
- Short-input padding and long-input truncation
- `encode_frame_into` buffer reuse (capacity stability)
- `ProtocolDatabase::lookup` resolves every registered VID/PID
- `total_leds()` matches the expected count from the spec
- `zones()` returns the correct topology hint for the device's physical shape
- `parse_response()` maps all known status bytes to the correct `ResponseStatus`
- Malformed responses return `ProtocolError::MalformedResponse` rather than panicking

For packet-level tests, build the expected byte payload by hand from your spec or USB capture
and compare it with `assert_eq!(&commands[0].data, &expected_bytes[..])`. USB capture replay
tests, where you record real packets and verify the encoder produces identical output, give
the highest confidence that the device will actually light up.

---

## Step 10: Run the gates

```bash
just verify                         # fmt + lint + test (full workspace)
just test-crate hypercolor-hal      # HAL tests only
just lint                           # Clippy with -D warnings
```

{% callout(type="info") %}
`unsafe_code` is `forbid` across the entire workspace. If a situation seems to require
`unsafe`, it almost certainly does not: `zerocopy` provides everything needed for safe
type-punning of wire-format buffers.
{% end %}

`just verify` must pass clean before submitting. A failing `just lint` is a blocker.

---

## Full checklist

Before opening a pull request, confirm every item:

- [ ] Protocol spec written in `docs/specs/` before implementation
- [ ] Driver module named after the silicon or OEM family, not the consumer brand
- [ ] Zerocopy packet structs with `#[repr(C)]` and compile-time size assertions for each
- [ ] `encode_frame` implemented and `encode_frame_into` overridden (not just the default)
- [ ] `CommandBuffer` used with `push_struct`, with no per-frame `Vec` allocations
- [ ] Color input normalized with `Cow` to avoid allocation when lengths match
- [ ] `connection_diagnostics()` implemented for write-only devices
- [ ] `keepalive()` implemented if the device exits direct mode while idle
- [ ] Device descriptor(s) registered in `database.rs` via `descriptors()`
- [ ] `DeviceFamily` variant added to `hypercolor-types` if this is new silicon
- [ ] Tests in `tests/` covering encoding, normalization, padding, and DB lookup
- [ ] `just verify` passes clean

---

## Related pages

- [@/contributing/_index.md](@/contributing/_index.md): dev setup, commit conventions, and workspace gates
- [@/contributing/debugging.md](@/contributing/debugging.md): `RUST_LOG` targets and protocol-level tracing
- [@/contributing/adding-a-network-driver.md](@/contributing/adding-a-network-driver.md): the network driver track (WLED, Govee, Hue)
- [@/hardware/compatibility.md](@/hardware/compatibility.md): the generated device compatibility matrix
- [@/hardware/usb-devices.md](@/hardware/usb-devices.md): udev rules and USB access on Linux
