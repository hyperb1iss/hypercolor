# Zerocopy Wire Format Patterns

Detailed patterns for defining protocol packet structs in Hypercolor.

## Derive Combinations

| Scenario | Derives | Notes |
|----------|---------|-------|
| Write-only packet (frame encoding) | `FromZeros, IntoBytes, KnownLayout, Immutable` | Most common |
| Read+write packet (command + response) | `FromBytes, IntoBytes, KnownLayout, Immutable` | `FromBytes` implies `FromZeros` — never derive both |
| Nested struct inside packet | Same as parent | All fields must be zerocopy-compatible |

## Multi-Byte Wire Fields

Use `zerocopy::byteorder` types, not native integers:

```rust
use zerocopy::byteorder::{LittleEndian, BigEndian, U16, U32};

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct MyPacket {
    length: U16<LittleEndian>,   // 2 bytes, LE on wire
    sequence: U32<BigEndian>,    // 4 bytes, BE on wire
    payload: [u8; 58],
}
```

Set values with `.set()`:
```rust
packet.length.set(payload_len as u16);
```

## Response Parsing: read_from_prefix vs read_from_bytes

**Always use `read_from_prefix()`** for parsing device responses:

```rust
let (response, _remainder) = MyResponse::read_from_prefix(data)
    .map_err(|_| ProtocolError::MalformedResponse)?;
```

Why: HID transports often return buffers larger than the struct (extra report ID byte at index 0, padding at end). `read_from_bytes()` requires exact size match and will fail.

## Report ID Handling

Platform behavior varies:

| Platform | Report ID Behavior |
|----------|--------------------|
| Linux hidraw | Includes report ID as first byte on read |
| Linux hidapi | Strips report ID on read |
| macOS hidapi | Strips report ID on read |
| Windows hidapi | Includes report ID as first byte |

For writing: always include report ID in the struct as the first field. The transport layer handles stripping if needed.

For reading: use `read_from_prefix()` which tolerates the extra byte.

## Packet Size Validation

Every packet struct **must** have a compile-time assertion:

```rust
const _: () = assert!(
    std::mem::size_of::<RazerReport>() == 90,
    "RazerReport must be exactly 90 bytes to match HID feature report size"
);
```

Common packet sizes across Hypercolor drivers:

| Device Family | Packet Size | Why |
|--------------|-------------|-----|
| Razer | 90 bytes | HID feature report |
| Lian Li ENE | 65 bytes (cmd) / 146-353 bytes (color) | HID feature + output report |
| Lian Li TL | 64 bytes | HID interrupt |
| Corsair LN | 65 bytes | HID feature report |
| Corsair LINK | 513 bytes | USB bulk (16-bit length prefix + 511 payload) |
| ASUS USB | 65 bytes | HID feature report |
| QMK | 32 or 64 bytes | HID interrupt (device-dependent) |

## Color Array Fields

For packets carrying LED color data, size the array to fit the maximum chunk:

```rust
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct ColorPacket {
    header: [u8; 5],
    colors: [u8; 60],    // 20 LEDs × 3 bytes (RGB)
}
```

Fill partially if fewer LEDs — zerocopy's `FromZeros` initializes everything to 0x00, so unused slots are black (LEDs off). No explicit zeroing needed.

## Enum Fields on Wire

Wire protocol enums are just u8 values. Define Rust enums with `#[repr(u8)]` if converting, or use raw `u8` fields in the packet struct and named constants:

```rust
// Prefer named constants over enums in packet structs
const CMD_ACTIVATE: u8 = 0x35;
const CMD_COLOR: u8 = 0x36;
const CMD_COMMIT: u8 = 0x35;  // yes, same as activate with different args

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Packet {
    command: u8,  // use CMD_* constants
    // ...
}
```

Zerocopy requires all fields to be valid for any bit pattern. Rust enums with `#[repr(u8)]` are NOT valid for arbitrary bytes — they'll fail `FromBytes` if the wire sends an unknown variant. Use plain `u8` fields in wire structs.
