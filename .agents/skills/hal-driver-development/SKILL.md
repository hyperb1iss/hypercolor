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

`hypercolor-hal` must never depend on `hypercolor-core` — that would create a circular dependency (`core` depends on `hal`). Dependencies: `hypercolor-color`, `hypercolor-types`, `async-trait`, `thiserror`, `tracing`, `nusb`, `tokio`, `tokio-serial`, `zerocopy`, `hidapi`, `midir`, `image`, `turbojpeg`. Linux adds `alsa`, `async-hid`, `futures-util`, `i2cdev`, `nix`; Windows adds `hypercolor-windows-pawnio`.

## HAL Descriptor or Driver Module?

A HAL device descriptor is not the only way to add hardware support, and it is
the wrong way for most non-USB devices. Every driver module assembled from HAL
descriptors is granted exactly one capability (`database.rs`):

```rust
DriverCapabilitySet {
    protocol_catalog: true,
    config: false,
    discovery: false,
    pairing: false,
    output_backend: false,
    runtime_cache: false,
    credentials: false,
    presentation: false,
    controls: false,
}
```

So a HAL descriptor buys wire-format encoding on the shared USB output backend
and nothing else. If the device needs driver-scoped config, a pairing or
authorization flow, active discovery, stored credentials, presentation
metadata, or dynamic control surfaces, it needs a real `DriverModule` in
`hypercolor-driver-api` (see `docs/specs/51-unified-driver-module-api.md` and
`docs/specs/52-dynamic-driver-control-surfaces.md`), not a descriptor here.
The network drivers (`hypercolor-driver-hue`, `-nanoleaf`, `-wled`, `-govee`)
are the worked examples.

The HAL's own module descriptors are derived, not hand-written:
`ProtocolDatabase::module_descriptors()` groups `DEVICE_DESCRIPTORS` by
`DeviceDescriptor::driver_id()`, and `hypercolor-driver-builtin`'s
`hal_catalog_driver_modules()` wraps each group. Every module stamps
`DRIVER_MODULE_API_SCHEMA_VERSION`.

## Before USB Driver Surgery

For “device jank”, “USB jank”, or “all USB devices stutter” reports, query daemon telemetry before editing protocol code:

```bash
just diagnose -- --json
hypercolor diagnose --system -j
curl -s -X POST http://127.0.0.1:9420/api/v1/diagnose \
  -H 'content-type: application/json' -d '{"system":true}'
```

Use the data to place the bug:

- `snapshot.render.latest_frame.gpu_sample_stale` or `output_frame_source=published_frame` means LEDs may be receiving old sampled data before any USB write happens.
- `snapshot.device_output.items[]` shows per-device queue health: `backend_id`, `fps_sent`, `fps_queued`, `frames_dropped`, `avg_queue_wait_ms`, `avg_write_ms`, `worker_finished`, `last_error`, `last_sequence`.
- `snapshot.usb.display_frames_delayed_for_led_total` and wait times reveal shared USB actor display-lane contention, not necessarily LED protocol failure.
- If all USB devices jank in unison and queues are healthy, investigate shared render sampling/output reuse first.
- If `output_frame_source=current_frame`, `gpu_sample_retry_hit=true`, writes are fast, and slow-frame warnings are `wake_late`, look for host scheduler pressure such as active Rust/Servo builds before editing USB protocol code.
- Queue `frames_dropped` is not automatically bad: capped devices intentionally replace stale pending payloads when render FPS exceeds device FPS. Compare `fps_sent` to `fps_target` and check write/queue latency plus errors.
- If one family has high `avg_write_ms`, drops, or errors while others are clean, then inspect transport/protocol encoding.

Do not lower FPS, resolution, LED counts, or performance caps to hide driver symptoms.

## Protocol Trait Contract

Every driver implements `Protocol` (in `src/protocol.rs`). The trait is declared
`pub trait Protocol: Send + Sync`, so any interior mutability has to be a `Mutex`
or `RwLock`. A `RefCell` field compiles fine inside the driver and only blows up
later at the `Box<dyn Protocol>` boundary in `ProtocolBinding::build`.

Required methods (no default body):

- `name()` → human-readable protocol name (`&'static str`)
- `init_sequence()` → commands sent on device connect (mode switch, firmware probe)
- `shutdown_sequence()` → graceful release (restore hardware control)
- `encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand>` — convenience wrapper
- `parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError>` — device replies
- `zones()` → physical LED zones for spatial mapping
- `capabilities()` → what the device supports
- `total_leds()` → LED count (determines color slice length)
- `frame_interval()` → target frame timing

Provided methods worth overriding:

- `encode_frame_into(&self, colors, commands: &mut Vec<ProtocolCommand>)` — **prefer this** — reuses the command vector across frames (zero-alloc hot path); the default just forwards to `encode_frame`
- `encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>>` — hardware brightness control
- `encode_scroll_mode(&self, mode: ScrollMode) -> Option<Vec<ProtocolCommand>>` — hardware scroll wheel mode (Razer mice)
- `encode_scroll_smart_reel(&self, enabled: bool) -> Option<Vec<ProtocolCommand>>`
- `encode_scroll_acceleration(&self, enabled: bool) -> Option<Vec<ProtocolCommand>>`
- `connection_diagnostics()` → optional one-shot verification commands for write-only devices
- `keepalive()` → returns `Option<ProtocolKeepalive>` (commands + interval) for devices that need periodic traffic to stay in direct mode
- `keepalive_commands()` → resolves the command sequence for a keepalive tick (override for stateful keepalives)
- `response_timeout()` → budget for commands expecting a reply (default 1s)
- `encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>>` — pixel display frame encoding (Corsair LCD, Push 2)
- `encode_display_frame_into(&self, jpeg_data, commands) -> Option<()>` — buffer-reusing variant
- `encode_display_payload_into(&self, payload: DisplayFramePayload<'_>, commands) -> Option<()>` — format dispatch; the default routes `DisplayFrameFormat::Jpeg` to `encode_display_frame_into` and rejects `Rgb`

**Always implement `encode_frame_into`**. The default `encode_frame` allocates a new Vec per frame — fine for tests, terrible at 60 FPS.

## ProtocolError and ProtocolResponse

`ProtocolError` has exactly four variants and every one of them is a struct
variant. There is no unit variant and no `InternalError`:

```rust
ProtocolError::CrcMismatch { expected: u8, actual: u8 }
ProtocolError::MalformedResponse { detail: String }
ProtocolError::DeviceError { status: ResponseStatus }
ProtocolError::EncodingError { detail: String }
```

`ResponseStatus` is `Ok | Busy | Failed | Timeout | Unsupported`. Return
`Unsupported` for a response marker the protocol does not recognize; there is no
`Unknown` variant.

`ProtocolResponse` has no impl block, so there are no constructors. Build it as a
struct literal:

```rust
Ok(ProtocolResponse { status: ResponseStatus::Ok, data: payload.to_vec() })
```

For a lock that cannot legitimately be poisoned, use `.expect("reason")` rather
than reaching for an error variant that does not exist. `drivers/asus/protocol.rs`
does exactly that for its topology `RwLock`.

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

`transfer_type` tells the transport _how_ to send — some devices mix HID feature reports for commands with bulk transfers for color data (Corsair LINK), or feature reports for commands with output reports for colors (Lian Li).

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

Effects produce variable-length color slices. Protocols expect exact LED counts. There is no shared helper for this: each driver defines its own private `normalize_colors` method on its protocol struct, because the expected length and the truncate/pad policy are protocol-specific. Nine drivers currently carry one. Use `Cow` to avoid allocation when lengths already match:

```rust
fn normalize_colors<'a>(&self, colors: &'a [[u8; 3]]) -> Cow<'a, [[u8; 3]]> {
    let expected = usize::try_from(self.total_leds()).unwrap_or(0);
    if expected == 0 {
        return Cow::Borrowed(&[]);
    }
    if colors.len() == expected {
        return Cow::Borrowed(colors);
    }

    let mut normalized = vec![[0_u8; 3]; expected];
    let copy_len = min(colors.len(), expected);
    normalized[..copy_len].copy_from_slice(&colors[..copy_len]);
    Cow::Owned(normalized)
}
```

Reference implementation: `drivers/razer/protocol.rs`, which also logs a `warn!` on every length mismatch.

## Device Database Registration

`src/database.rs` holds a static `LazyLock<Vec<DeviceDescriptor>>`. Each driver module exposes a `descriptors()` function, usually in its own `devices.rs` but not always: Razer nests it at `drivers/razer/devices/mod.rs`, and Corsair splits across four sub-modules (`lighting_node`, `lcd`, `link`, `peripheral`) that `drivers/corsair/devices.rs` concatenates.

```rust
fn is_v2_firmware(candidate: &str) -> bool {
    candidate.trim().starts_with("2.")
}

static DESCRIPTORS: &[DeviceDescriptor] = &[DeviceDescriptor {
    vendor_id: 0x1234,
    product_id: 0x5678,
    name: "My Device",
    family: DeviceFamily::new_static("myvendor", "My Vendor"),
    transport: TransportType::UsbHid { interface: 0 },
    protocol: ProtocolBinding {
        id: "myvendor/mydevice",
        build: || Box::new(MyProtocol::new()),
    },
    firmware_predicate: None, // or Some(is_v2_firmware)
}];

pub fn descriptors() -> &'static [DeviceDescriptor] {
    DESCRIPTORS
}
```

That plain-slice form works only because every field is const-constructible. A
descriptor whose `transport` comes from `resolve_current_transport(...).expect(...)`
cannot live in a `static ...: &[DeviceDescriptor]` at all; see Transport
Selection Guide below.

`DeviceFamily` is a **struct, not an enum**. There is no `DeviceFamily::MyFamily` variant to reach for. Three constructors exist: `new_static(id, name)` for const contexts (what every HAL driver uses), `new(id, name)`, and `named(name)` which derives the id from the display name. The id must be lowercase ASCII; `new` and `named` sanitize it for you, `new_static` does not, so pass a clean id.

**`protocol.id` shape is load-bearing.** `DeviceDescriptor::driver_id()` splits it on the first `/` and takes the left half, falling back to the family id when there is no slash. That driver id becomes the driver module ID, which is in turn the key under `config.drivers` in the daemon config and the filter key for `enabled_driver_ids`. Pick `"<driver_id>/<model>"` and keep the left half stable across every descriptor in the module.

**Firmware predicates** disambiguate same-PID devices with different protocols (Lian Li AL firmware 1.7 uses HID, 1.0 uses vendor control). Write a named local predicate; `firmware_matches` is a private free function inside `drivers/lianli/devices.rs` and cannot be imported. Resolution order in `ProtocolDatabase::lookup_with_firmware_for_driver_ids`: when a firmware string is known, the first descriptor whose predicate matches wins; otherwise the first descriptor with **no** predicate wins; only if every candidate has a predicate does the first candidate win as a last resort. So a predicate-free descriptor is the fallback for a device whose firmware was never read.

## Known Wire Format Gotchas

| Device Family | Gotcha                                                                                       |
| ------------- | --------------------------------------------------------------------------------------------- |
| Lian Li ENE   | Color byte order is **R-B-G**, not RGB (`encode_ene_color` in `lianli/ene.rs`)               |
| Lian Li TL    | Carve-out: `TlFan` is plain RGB (`DeviceColorFormat::Rgb`), every other variant is `Rbg`     |
| Lian Li AL    | Dual-ring addressing: `port = group * 2 + ring.port_offset()` (inner fan vs outer edge)      |
| Lian Li       | Four asserted struct sizes: 11, 65, 146, 353. AL uses 146, SL V2 / AL V2 / SL Infinity use 353 |
| Lian Li SL    | SL and SL Redragon color payloads are built with `push_fill`, so length is `2 + leds * 3` and varies with fan count (98 bytes for two 16-LED fans) |
| Razer         | XOR checksum covers bytes `[2..88)` (bytes 2 through 87 inclusive, 86 bytes)                 |
| Razer         | 6 protocol versions — transaction_id selects: 0xFF/0x3F/0x1F/0x9F/0x08/0x60                  |
| Razer         | 4 custom effect activation styles across device generations                                  |
| ASUS          | Runtime topology discovery via `RwLock` interior mutability in `parse_response()`            |
| ASUS          | Topology overrides for 14 known board names plus 3 firmware strings                          |
| Corsair LN    | Components sent separately (R, then G, then B) — 50 LEDs per packet                          |
| Corsair LINK  | 513-byte packets: bytes 0-1 zero, `0x01` at byte 2, command at byte 3. The 16-bit LE length prefix belongs to the inner `build_link_write_buffer` payload, not the outer packet |
| Report IDs    | Only `UsbHidApi` and `UsbHidRaw` ever prepend it; every other transport sends the struct verbatim |

**Who owns the report ID byte is a property of the transport, not a global
rule.** `report_mode` exists on exactly two of the nine `TransportType`
variants, `UsbHidApi` and `UsbHidRaw`, and the prepend lives in one place per
backend: `encode_hidapi_packet` (`transport/hidapi.rs`) and
`encode_hidraw_packet` (`transport/hidraw.rs`). Those two push `report_id` in
front of the payload unless the mode is `FeatureReportWithReportId` or
`OutputReportWithReportId`. Every other transport has no `report_mode` and no
prepend step. `UsbControl` and `UsbBulk` carry the report ID out of band in the
control-transfer `wValue`, so their structs stay clean (`RazerReport` has no
report-ID field). `UsbHid`, `UsbMidi`, `UsbSerial`, `I2cSmBus`, and
`UsbVendor` send the buffer as-is, so the struct has to carry the byte itself.
Lian Li ENE is the worked example: its descriptors declare
`TransportType::UsbHid` (`lianli/devices.rs`), every ENE packet struct starts
with a `report_id` field set to `ENE_REPORT_ID` (`lianli/common.rs`,
`lianli/ene.rs`), and `transport/hid.rs` reads that first byte back out of the
payload. Drop it there and every field on the wire shifts by one.

## Multi-Phase Update Patterns

Some devices require sequenced commands per frame:

- **Lian Li ENE**: four phases, not three. Per group: Activate (`0x10`) → Color data (`0x30 | port`) → per-group Commit (`0x10 | port`, static effect). Then one frame Commit (`0x60`) after all groups
- **Razer matrix**: Color chunks per row → custom effect activation, appended only when `should_append_frame_activation()`. Chunk capacity is 22 columns for Standard/Extended/ExtendedArgb but **16** for Linear
- **Corsair LN**: Per channel, a PortState packet → per 50-LED chunk, three Direct packets (R, then G, then B) → Commit packet
- **ASUS**: Direct color chunks (20 RGB triples/packet) → Apply flag on final chunk

## Transport Selection Guide

**HID descriptors declare a platform-free `TransportIntent`, they do not hardcode a `TransportType`.** hidraw is Linux-only, so a descriptor that names `UsbHidRaw` directly cannot compile a working binary anywhere else. Zero descriptors in the tree do it. Declare the intent and let `resolve_current_transport` pick:

```rust
const fn my_hid_intent(interface: u8) -> TransportIntent {
    TransportIntent::Hid(HidTransportIntent {
        access: HidAccessMode::HostManaged,
        interface,
        report_id: MY_REPORT_ID,
        report_mode: HidRawReportMode::OutputReport,
        max_report_len: MY_PAYLOAD_LEN + 1,
        usage_page: None,
        usage: None,
    })
}

// in the descriptor:
transport: resolve_current_transport(my_hid_intent(2))
    .expect("HID transport should support the current platform"),
```

Resolution is `const`: `HostManaged` becomes `UsbHidRaw` on Linux and `UsbHidApi` elsewhere; `Direct` becomes `UsbHid` on Linux and `UsbHidApi` elsewhere. `TransportIntent::I2cSmBus` resolves on Linux and Windows and errors on macOS. Drivers using the intent path today: ASUS, Corsair (LCD, Lighting Node, LINK), Nollie, PrismRGB.

**A resolving descriptor must live behind `LazyLock`.** `resolve_current_transport`
is a `const fn`, but `Result::expect` is not, so the `.expect(...)` above is a
non-const call. Put it in a `static ...: &[DeviceDescriptor]` and the crate
fails to build with E0015, "cannot call non-const method `Result::expect` in
statics". Every driver on the intent path declares
`static ..._DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>>` and expands the
descriptor macro inside the closure. The four plain-slice statics in the tree
(Lian Li, QMK, Dygma, Corsair peripherals) name literal `TransportType`
variants and resolve nothing.

| Transport    | Associated Data                                                                 | When to Use                                                               | File                   |
| ------------ | --------------------------------------------------------------------------------- | --------------------------------------------------------------------------- | ---------------------- |
| `UsbControl` | `{ interface, report_id }`                                                      | HID feature reports via control transfers (Razer)                         | `transport/control.rs` |
| `UsbHid`     | `{ interface }`                                                                 | HID interrupt endpoints (Lian Li ENE; Linux resolution of `Direct`)       | `transport/hid.rs`     |
| `UsbHidApi`  | `{ interface?, report_id, report_mode, max_report_len, usage_page?, usage? }`    | Cross-platform `hidapi` (Lian Li TL, QMK, Razer; macOS/Windows HID)       | `transport/hidapi.rs`  |
| `UsbHidRaw`  | `{ interface, report_id, report_mode, usage_page?, usage? }`                     | Linux `/dev/hidraw*`; only ever produced by resolving `HostManaged`       | `transport/hidraw.rs`  |
| `UsbBulk`    | `{ interface, report_id }`                                                      | Bulk endpoints + HID feature sideband; no current descriptor selects it   | `transport/bulk.rs`    |
| `UsbMidi`    | `{ midi_interface, display_interface, display_endpoint }`                       | MIDI control + bulk display (Ableton Push 2)                              | `transport/midi.rs`    |
| `I2cSmBus`   | `{ address }`                                                                   | I2C/SMBus on motherboard (ASUS Aura)                                      | `transport/smbus.rs`   |
| `UsbVendor`  | (none)                                                                          | Vendor-specific control transfers (Lian Li UNI Hub original, AL10)        | `transport/vendor.rs`  |
| `UsbSerial`  | `{ baud_rate }`                                                                 | USB CDC-ACM serial (Dygma, Nollie serial models)                          | `transport/serial.rs`  |

`max_report_len` on `UsbHidApi` sizes the read and feature-report buffer, including the report ID byte when the OS API expects it. Omitting it is a compile error, and getting it wrong truncates responses.

**SMBus is a separate registration path.** ASUS Aura SMBus does not go through `DeviceDescriptor` at all: `smbus_registry.rs` builds the protocol by ID (`asus/aura-smbus`), and `database.rs` hand-pushes a `DriverProtocolDescriptor` for it and hardcodes the `"asus"` module lookup to attach the SMBus transport. Adding a second SMBus vendor means touching both.

## Hooks Beyond the Descriptor

Two per-driver hooks live outside `drivers/` and are easy to miss when a device has swappable physical parts:

- `src/attachment_profile.rs` — protocol-specific slot topology. Given a `DeviceInfo` and its `ComponentBinding`s, it returns the `ComponentSlot`s a device actually exposes. PrismRGB and Nollie use it for GPU cable and channel layouts.
- `src/protocol_config.rs` — runtime protocol configuration derived from those bindings. It rebuilds the `Box<dyn Protocol>` with a different LED count when, say, a triple Strimer replaces a dual.

Both are keyed by protocol ID string constants, so a new driver that needs either must add its IDs there.

## Testing

Integration tests live in `crates/hypercolor-hal/tests/`, one file per driver family or transport (34 files, 31 following the `{feature}_tests.rs` convention). Encoding tests need no hardware: construct the protocol, feed a synthetic color slice, assert on `ProtocolCommand` count, `data.len()`, and byte positions. `benches/protocol_encoding.rs` is a criterion bench for the hot encode path; add a case there when a new driver lands in the render loop.

## New Protocol Checklist

1. [ ] Zerocopy packet structs with compile-time size assertions
2. [ ] `encode_frame_into` implemented (not just `encode_frame`)
3. [ ] `CommandBuffer` with `push_struct` — never build `Vec<ProtocolCommand>` with fresh allocs per frame
4. [ ] `Cow` normalization for color input slice
5. [ ] `connection_diagnostics()` implemented for write-only devices (verifies device accepts commands)
6. [ ] `keepalive()` implemented if device exits direct mode on idle (returns commands + interval)
7. [ ] Device descriptors registered in `database.rs`
8. [ ] **udev rule in `udev/99-hypercolor.rules`** — `tests/udev_rules_tests.rs` walks `ProtocolDatabase::all()` and asserts a matching rule line per transport family, so a descriptor with no rule turns `just test` red. Existing rules are vendor-wide, so a new PID under a vendor Hypercolor already supports is covered; a descriptor introducing a **new VID** needs new lines. HID transports need both a `hidraw` and a `usb` rule; `UsbSerial` needs `tty`; `I2cSmBus` is covered generically by the `/dev/i2c-*` rule; everything else needs `usb`
9. [ ] Vendor entry in `data/drivers/vendors/<vendor>.toml`, then `just compat` to regenerate `data/compat/`. This is the canonical device database the public compatibility matrix is built from, and it is not derived from `database.rs`
10. [ ] Tests for encoding without hardware (`tests/{feature}_tests.rs`)
11. [ ] Spec document in `docs/specs/`
12. [ ] Frame interval matches device refresh rate

## Detailed References

- **`references/protocol-implementation.md`** — Full Protocol impl walkthrough with annotated examples from Razer and Lian Li
- **`references/zerocopy-wire-formats.md`** — Detailed zerocopy patterns, response parsing, multi-byte fields, and platform-specific report ID handling
- **`docs/specs/62-zerocopy-protocol-structs.md`** — the source spec for the zerocopy packet pattern
- **`docs/specs/16-hardware-abstraction-layer.md`** — HAL architecture: Protocol trait, Transport trait, protocol database, USB backend and scanner
- **`docs/specs/51-unified-driver-module-api.md`** — the driver-module model that HAL descriptors feed into
