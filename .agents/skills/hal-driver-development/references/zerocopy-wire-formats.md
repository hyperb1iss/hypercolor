# Zerocopy Wire Format Patterns

Detailed patterns for defining protocol packet structs in Hypercolor. The source spec is `docs/specs/62-zerocopy-protocol-structs.md`; the workspace pins zerocopy 0.8.

## Derive Combinations

| Scenario                               | Derives                                        | Notes                                               |
| -------------------------------------- | ---------------------------------------------- | --------------------------------------------------- |
| Write-only packet (frame encoding)     | `FromZeros, IntoBytes, KnownLayout, Immutable` | Most common                                         |
| Read+write packet (command + response) | `FromBytes, IntoBytes, KnownLayout, Immutable` | `FromBytes` implies `FromZeros` — never derive both |
| Nested struct inside packet            | Same as parent                                 | All fields must be zerocopy-compatible              |

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
let (response, _remainder) = MyResponse::read_from_prefix(data).map_err(|_| {
    ProtocolError::MalformedResponse {
        detail: format!("expected at least {} bytes, got {}", MY_RESPONSE_LEN, data.len()),
    }
})?;
```

Two things the compiler will hold you to. The workspace pins zerocopy 0.8, where `read_from_prefix` returns `Result<(Self, &[u8]), _>`, so it must be destructured as a tuple; binding a single value does not compile. And `ProtocolError::MalformedResponse` is a struct variant carrying `detail: String`, not a unit variant.

Why prefix and not bytes: HID transports often return buffers larger than the struct (extra report ID byte at index 0, padding at end). `read_from_bytes()` requires exact size match and will fail.

## Report ID Handling

Platform behavior varies:

| Platform       | Report ID Behavior                       |
| -------------- | ---------------------------------------- |
| Linux hidraw   | Includes report ID as first byte on read |
| Linux hidapi   | Strips report ID on read                 |
| macOS hidapi   | Strips report ID on read                 |
| Windows hidapi | Includes report ID as first byte         |

**For writing, start by asking which transport the descriptor resolves to.** Only two of the nine `TransportType` variants carry a `report_mode` at all, `UsbHidApi` and `UsbHidRaw`, and only those two have a prepend step: `encode_hidapi_packet` (`transport/hidapi.rs`) and `encode_hidraw_packet` (`transport/hidraw.rs`) push `report_id` in front of the payload unless the mode is one of the `*WithReportId` variants, in which case they send the payload untouched.

Under `UsbHidApi` or `UsbHidRaw`:

| `HidRawReportMode`          | Transport behavior          | Packet struct                        |
| --------------------------- | --------------------------- | ------------------------------------ |
| `FeatureReport`             | Prepends the report ID byte | Must **not** carry a report-ID field |
| `OutputReport`              | Prepends the report ID byte | Must **not** carry a report-ID field |
| `FeatureReportWithReportId` | Sends the payload as-is     | Must carry the report ID as field 0  |
| `OutputReportWithReportId`  | Sends the payload as-is     | Must carry the report ID as field 0  |

Get this backwards and the report ID ships twice, shifting every subsequent byte by one. Worked examples: ASUS declares `OutputReport` and `AuraDirectPacket` has no report-ID field (64-byte payload, 65 bytes on the wire). Lian Li TL declares `OutputReportWithReportId` and `TlPacket` starts with one (64 bytes total, report ID included).

**Every other transport sends your payload verbatim, and where the report ID lives then depends on which one.** `UsbControl` and `UsbBulk` carry it out of band, in the control-transfer `wValue` as `(HID_REPORT_TYPE_FEATURE << 8) | report_id`, so the struct stays clean: `RazerReport` is 90 bytes with no report-ID field. `UsbHid`, `UsbMidi`, `UsbSerial`, `I2cSmBus`, and `UsbVendor` write the buffer as-is with nothing added, so any leading byte the device expects has to be field 0 of your struct.

Lian Li ENE is the case to reason from. Its descriptors declare `TransportType::UsbHid` (`lianli/devices.rs`), which has no `report_mode` and no prepend step, so `EnePacket11`, `EnePacket65`, and the output packets all start with a `report_id` field that the encoder sets to `ENE_REPORT_ID` (`lianli/common.rs`, `lianli/ene.rs`). `transport/hid.rs` reads that first byte back out of the payload to address the feature report. Omit it and you have not just lost a byte, you have sent a wrong report ID and shifted every field after it.

`HidAccessMode::Direct` resolves to exactly this case on Linux (`transport.rs`), so an intent-based descriptor can land on `UsbHid` even though the intent named a `report_mode`. Check what your target resolves to before deciding the struct layout.

For reading: use `read_from_prefix()` which tolerates the extra byte.

## Packet Size Validation

Every packet struct **must** have its size pinned. The usual form is a compile-time assertion next to the struct, as in `lianli/common.rs`:

```rust
const _: () = assert!(
    std::mem::size_of::<EnePacket65>() == 65,
    "EnePacket65 must match the 65-byte ENE feature report size"
);
```

Corsair, QMK, ASUS, Nollie, PrismRGB, and Push 2 all pin their structs the same way. Razer is the exception: `razer/crc.rs` declares `RAZER_REPORT_LEN = 90` and the size check lives in `tests/razer_protocol_tests.rs` as a runtime `assert_eq!` against that constant. Either form satisfies the rule; pick the const assertion for a new struct, because it fails at build time rather than at `cargo test`.

Common packet sizes across Hypercolor drivers:

| Device Family | Packet Size                                   | Why                                                          |
| ------------- | --------------------------------------------- | ------------------------------------------------------------ |
| Razer         | `RazerReport` = 90 bytes                      | HID feature report, `RAZER_REPORT_LEN`                       |
| Lian Li ENE   | `EnePacket11` = 11, `EnePacket65` = 65 (cmd)  | HID feature report; SL/SL Redragon use the 11-byte form      |
| Lian Li ENE   | `EneOutputPacket146` / `EneOutputPacket353`   | Output report; AL uses 146, SL V2 / AL V2 / SL Infinity use 353 |
| Lian Li TL    | `TlPacket` = 64 bytes                         | `TL_PACKET_LEN`, report ID included in the struct            |
| Corsair LN    | `LnDirectPacket` = 65 bytes                   | `LN_WRITE_BUF_SIZE`                                          |
| Corsair LINK  | 513 bytes                                     | `LINK_WRITE_BUF_SIZE`; two zero bytes, `0x01`, then command  |
| Corsair LCD   | `LcdDisplayPacket` = 1024 bytes               | `LCD_PACKET_SIZE`, bulk display chunk                        |
| ASUS USB      | `AuraDirectPacket` = 64 bytes                 | `AURA_REPORT_PAYLOAD_LEN`; the transport prepends the report ID, so 65 on the wire |
| QMK           | `QmkPacket` = 65 bytes                        | `PACKET_SIZE`, fixed; report ID is field 0                   |

Corsair LINK has two layers and they are easy to conflate. `build_link_packet` (`corsair/framing.rs`) allocates the 513-byte buffer, leaves bytes 0 and 1 zero, writes `0x01` at byte 2, and starts the command at byte 3. There is no length prefix at that layer. The 16-bit little-endian length belongs to the inner payload built by `build_link_write_buffer`, whose layout is `len_le16 | 00 00 | data_type[2] | payload`, and that buffer is then handed to `build_link_packet` as data.

Not every payload is a fixed struct. Lian Li SL and SL Redragon color data is built with `CommandBuffer::push_fill` at `2 + leds * 3` bytes, so the on-wire length tracks fan count (98 bytes for two 16-LED fans). Use `push_fill` when the length is data-dependent and a zerocopy struct when it is not.

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

Wire protocol enums are just u8 values. Keep the packet struct field a plain `u8` and put the naming in an ordinary Rust enum with a `const fn byte()` accessor. Corsair does this in `drivers/corsair/types.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightingNodePacketId {
    Firmware,
    Direct,
    Commit,
    Reset,
    PortState,
    Brightness,
}

impl LightingNodePacketId {
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Firmware => 0x02,
            Self::Direct => 0x32,
            Self::Commit => 0x33,
            Self::Reset => 0x37,
            Self::PortState => 0x38,
            Self::Brightness => 0x39,
        }
    }
}

#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct LnDirectPacket {
    padding: u8,
    packet_id: u8,  // set from LightingNodePacketId::Direct.byte()
    // ...
}
```

Bare `const CMD_*: u8` constants are fine too where there is no natural grouping. What matters is that the struct field stays `u8`.

Zerocopy requires all fields to be valid for any bit pattern. Rust enums with `#[repr(u8)]` are NOT valid for arbitrary bytes — they'll fail `FromBytes` if the wire sends an unknown variant. Use plain `u8` fields in wire structs.
