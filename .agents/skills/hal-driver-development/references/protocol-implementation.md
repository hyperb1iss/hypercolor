# Protocol Implementation Walkthrough

Annotated patterns from existing Hypercolor drivers. Read the SKILL.md first — this is the deep-dive.

## Anatomy of encode_frame_into

From Lian Li ENE (`drivers/lianli/ene.rs`). The top-level `encode_frame_into` is a pure dispatch on variant; the phase structure lives in the per-topology encoders:

```rust
fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
    match self.variant {
        LianLiHubVariant::Sl | LianLiHubVariant::SlV2 | LianLiHubVariant::SlRedragon => {
            self.encode_single_ring(colors, commands);
        }
        LianLiHubVariant::Al | LianLiHubVariant::AlV2 => {
            self.encode_dual_port_groups(colors, commands);
        }
        LianLiHubVariant::SlInfinity => self.encode_sl_infinity(colors, commands),
        LianLiHubVariant::TlFan => commands.clear(),
    }
}
```

The single-ring encoder is the readable one. It is **four** phases, not three: three per group, plus one frame-wide commit at the end.

```rust
fn encode_single_ring(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
    let leds_per_fan = usize::from(self.variant.leds_per_fan());
    let group_capacity = leds_per_fan * usize::from(self.variant.max_fans_per_group());
    let mut encoder = CommandBuffer::new(commands);
    let mut wrote_any = false;

    for group in 0..usize::from(self.variant.group_count()) {
        let start = group * group_capacity;
        if start >= colors.len() {
            break;
        }
        let end = colors.len().min(start + group_capacity);
        let group_colors = &colors[start..end];
        let fan_count = group_colors.len().div_ceil(leds_per_fan);
        if fan_count == 0 {
            continue;
        }
        wrote_any = true;
        let group_u8 = u8::try_from(group).expect("group index should fit in u8");
        let fan_count_u8 = u8::try_from(fan_count).expect("fan count should fit in u8");

        // Phase 1: activate. Command 0x10, feature report, 20ms post delay.
        self.push_activate(&mut encoder, group_u8, fan_count_u8);

        // Phase 2: color data. Command 0x30 | port, output report, 20ms post delay.
        self.push_sl_color_data(&mut encoder, group_u8, group_colors, fan_count * leds_per_fan);

        // Phase 3: per-group commit. Command 0x10 | port with the static-effect args.
        self.push_commit(&mut encoder, group_u8);
    }

    // Phase 4: one frame commit for the whole hub. Command 0x60.
    if wrote_any {
        self.push_frame_commit(&mut encoder);
    }
    encoder.finish();
}
```

Key observations:

- `CommandBuffer::new(commands)` borrows the pre-existing vec — no allocation. It does not clear; `push_fill` overwrites slot by slot and `finish()` truncates to the used count
- Different phases use different `TransferType` values: `HidReport` for the 0x10 / 0x60 command packets, `Primary` for the 0x30 color payload
- Color byte order is R-B-G. `encode_ene_color` writes `[r, b, g]` after applying the variant's white limit. This is hardware-specific, not a bug. `TlFan` is the exception and stays plain RGB
- Command opcodes are computed inline as literals (`0x10`, `0x30 | port`, `0x60`), not hoisted into `CMD_*` constants
- The packet type is chosen by variant, not by phase. Commands: SL and SL Redragon use `EnePacket11`, everything else uses `EnePacket65`. Color payloads: AL uses `EneOutputPacket146`, AL V2 / SL V2 / SL Infinity use `EneOutputPacket353`, and SL / SL Redragon use `push_fill` instead of a struct because their payload length is `2 + leds * 3` and tracks fan count
- `encoder.finish()` truncates to actual command count

The dual-ring variants derive the port from group and ring:

```rust
for ring in [DualRing::Inner, DualRing::Outer] {
    self.push_activate(&mut encoder, group_u8, fan_count_u8);
    let port = group_u8 * 2 + ring.port_offset();
    // ... color data for this ring, then commit for this port
}
```

## Anatomy of parse_response

From ASUS Aura (`drivers/asus/protocol.rs`, runtime topology discovery). It dispatches on a leading marker byte after stripping any report ID prefix:

```rust
fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
    let payload = strip_report_id(data);
    let Some(&marker) = payload.first() else {
        return Err(ProtocolError::MalformedResponse {
            detail: "ASUS response is empty".to_owned(),
        });
    };

    match marker {
        FIRMWARE_RESPONSE_MARKER => {
            let firmware = parse_firmware_response(payload)?;
            let mut topology = self
                .topology
                .write()
                .expect("ASUS topology lock should not be poisoned");
            topology.firmware = Some(firmware.clone());
            topology.init_phase = AuraInitPhase::FirmwareReceived;
            // board-name and firmware-string overrides applied here
            Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: firmware.into_bytes(),
            })
        }
        CONFIG_RESPONSE_MARKER => {
            let table = parse_config_table(payload)?;
            let mut topology = self
                .topology
                .write()
                .expect("ASUS topology lock should not be poisoned");
            if !topology.overrides_applied {
                topology.mainboard_leds = u32::from(table[0x1B]);
                topology.rgb_header_count = u32::from(table[0x1D]);
            }
            topology.init_phase = AuraInitPhase::Configured;
            Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: table.to_vec(),
            })
        }
        _ => Ok(ProtocolResponse {
            status: ResponseStatus::Unsupported,
            data: payload.to_vec(),
        }),
    }
}
```

What to copy from this:

- `ProtocolResponse` has no constructors. Build the struct literal. There is no `::status()` or `::firmware()`
- Every `ProtocolError` variant is a struct variant. `MalformedResponse` needs `detail: String`; a bare `ProtocolError::MalformedResponse` does not compile
- Unknown markers return `ResponseStatus::Unsupported`. There is no `Unknown`
- There is no `ProtocolError::InternalError`. A lock that cannot legitimately be poisoned gets `.expect("reason")`
- ASUS uses `RwLock<AuraTopology>` for interior mutability because `parse_response` takes `&self`, but topology is discovered at runtime. `Protocol: Send + Sync`, so this has to be a lock and not a `RefCell`
- Firmware overrides only apply when the discovered values have not already been overridden, which is why the discovered fields are gated on `overrides_applied`

### Parsing with a zerocopy struct

When the response is a fixed-layout packet rather than a marker byte, parse it with `read_from_prefix`. In zerocopy 0.8 that returns `Result<(Self, &[u8]), _>`, so it **must** be destructured as a tuple. From `drivers/razer/packet.rs`:

```rust
pub(super) fn parse_response(data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
    if data.len() < RAZER_REPORT_LEN {
        return parse_short_response(data);
    }

    // HID transports can leave a report ID prefix attached on some platforms.
    let (report, _remainder) =
        RazerReport::read_from_prefix(data).map_err(|_| ProtocolError::MalformedResponse {
            detail: format!(
                "expected at least {} bytes, got {}",
                RAZER_REPORT_LEN,
                data.len()
            ),
        })?;

    let status = map_status(report.status);
    if status == ResponseStatus::Failed {
        return Err(ProtocolError::DeviceError { status });
    }

    let data_size = usize::from(report.data_size);
    if data_size > REPORT_ARGS_LEN {
        return Err(ProtocolError::MalformedResponse {
            detail: format!("data size exceeds arguments field: {data_size}"),
        });
    }

    Ok(ProtocolResponse {
        status,
        data: report.args[..data_size].to_vec(),
    })
}
```

## Chunking Patterns

### Razer: Row-Based Chunks

Chunk capacity is 22 columns for Standard, Extended, and ExtendedArgb matrices, but 16 for Linear (`frame_chunk_capacity`). The header differs by command set: Standard writes a 4-byte header starting with `0xFF`, Extended writes a 5-byte header starting with two zero bytes. From `drivers/razer/protocol.rs`:

```rust
let max_chunk = Self::frame_chunk_capacity(self.matrix_type);
for row in 0..rows {
    let row_colors = &colors[row * cols..(row + 1) * cols];

    for chunk_start in (0..cols).step_by(max_chunk) {
        let chunk_end = min(chunk_start + max_chunk, cols);
        let mut args = [0_u8; REPORT_ARGS_LEN];
        let row_u8 = u8::try_from(row).unwrap_or(0);
        let start_col = u8::try_from(chunk_start).unwrap_or(0);
        let stop_col = u8::try_from(chunk_end - 1).unwrap_or(0);  // inclusive!

        let (command_class, command_id, mut args_len, declared_size) = match self.command_set {
            RazerLightingCommandSet::Standard => {
                args[..4].copy_from_slice(&[0xFF, row_u8, start_col, stop_col]);
                (0x03, 0x0B, 4, Some(STANDARD_MATRIX_FRAME_DATA_SIZE))
            }
            RazerLightingCommandSet::Extended => {
                args[..5].copy_from_slice(&[0x00, 0x00, row_u8, start_col, stop_col]);
                (0x0F, 0x03, 5, None)
            }
        };

        for color in &row_colors[chunk_start..chunk_end] {
            args[args_len..args_len + color.len()].copy_from_slice(color);  // plain RGB
            args_len += color.len();
        }

        self.push_packet_with_options(
            &mut encoder,
            frame_transaction_id,
            command_class,
            command_id,
            &args[..args_len],
            declared_size,
            self.frame_commands_expect_response,
            Duration::from_millis(1),
        );
    }
}

if self.should_append_frame_activation() {
    self.push_activation_command(&mut encoder);
}
```

Key points:

- Stop column is **inclusive** (`chunk_end - 1`), not exclusive
- **The CRC is never computed at the call site.** `packet::build_report` fills `report.crc` as the last step of assembling every `RazerReport`, so an encoder that touches `report.crc` itself is doing something wrong. The XOR covers bytes `[2..88)` regardless of data size
- The transaction ID is `self.frame_transaction_id.unwrap_or(self.version.transaction_id())`. Frame packets can use a different transaction ID from control packets on the same device
- Standard passes an explicit `declared_data_size`; Extended lets `build_report` derive it from the args length

### Corsair Lighting Node: Component-Separated Channels

Three packets per 50-LED chunk, one per color component, then a Commit per channel. From `drivers/corsair/lighting_node/protocol.rs`:

```rust
for (chunk_index, chunk) in channel_colors.chunks(DIRECT_CHUNK_SIZE).enumerate() {
    let start = u8::try_from(chunk_index * DIRECT_CHUNK_SIZE).unwrap_or(u8::MAX);
    let count = u8::try_from(chunk.len()).unwrap_or(u8::MAX);

    for (component, color_channel) in [
        (0_usize, LightingNodeColorChannel::Red),
        (1_usize, LightingNodeColorChannel::Green),
        (2_usize, LightingNodeColorChannel::Blue),
    ] {
        let mut packet = LnDirectPacket::new_zeroed();
        packet.packet_id = LightingNodePacketId::Direct.byte();
        packet.channel = channel;
        packet.start_led = start;
        packet.led_count = count;
        packet.color_channel = color_channel.byte();
        for (index, color) in chunk.iter().enumerate() {
            packet.values[index] = color[component];
        }

        encoder.push_struct(&packet, true, Duration::ZERO, Duration::ZERO, TransferType::Primary);
    }
}

encoder.push_fill(true, Duration::ZERO, Duration::ZERO, TransferType::Primary, |buffer| {
    Self::write_packet(buffer, LightingNodePacketId::Commit, &[0xFF])
});
```

Corsair sends R values, then G values, then B values, per chunk. Not interleaved RGB. `DIRECT_CHUNK_SIZE` is 50 and `LnDirectPacket` is 65 bytes: a leading `padding: u8` (the HID report slot), five header bytes, 50 component values, then 9 bytes of tail padding. Note `expects_response: true` on every packet here, unlike the Lian Li and Razer encoders which are write-only.

Each channel is also preceded by a PortState packet that puts the channel in software mode:

```rust
encoder.push_fill(true, Duration::ZERO, Duration::ZERO, TransferType::Primary, |buffer| {
    Self::write_packet(
        buffer,
        LightingNodePacketId::PortState,
        &[channel, LightingNodePortState::Software.byte()],
    );
});
```

## Device Descriptor Factory Pattern

Each driver module exports a `descriptors() -> &'static [DeviceDescriptor]` function. It usually lives in the module's `devices.rs`, but Razer nests it at `drivers/razer/devices/mod.rs` and Corsair concatenates four sub-modules in `drivers/corsair/devices.rs`. Two patterns exist:

**Pattern 1: Static slice** (preferred when all fields are const-compatible):

```rust
// In src/drivers/mydevice/devices.rs
fn is_v2_firmware(candidate: &str) -> bool {
    candidate.trim().starts_with("2.")
}

const fn my_hid_intent(interface: u8) -> TransportIntent {
    TransportIntent::Hid(HidTransportIntent {
        access: HidAccessMode::HostManaged,
        interface,
        report_id: 0x00,
        report_mode: HidRawReportMode::FeatureReport,
        max_report_len: MY_PACKET_LEN,
        usage_page: None,
        usage: None,
    })
}

static MY_DESCRIPTORS: &[DeviceDescriptor] = &[
    DeviceDescriptor {
        vendor_id: 0x1234,
        product_id: 0x5678,
        name: "My Device Pro",
        family: DeviceFamily::new_static("myvendor", "My Vendor"),
        transport: TransportType::UsbHid { interface: 0 },
        protocol: ProtocolBinding {
            id: "myvendor/pro",
            build: || Box::new(MyProtocol::new(MyVariant::Pro)),
        },
        firmware_predicate: None,
    },
    DeviceDescriptor {
        vendor_id: 0x1234,
        product_id: 0x5678,
        name: "My Device Pro (v2 firmware)",
        family: DeviceFamily::new_static("myvendor", "My Vendor"),
        transport: TransportType::UsbHid { interface: 0 },
        protocol: ProtocolBinding {
            id: "myvendor/pro-v2",
            build: || Box::new(MyProtocolV2::new()),
        },
        firmware_predicate: Some(is_v2_firmware),
    },
];

pub fn descriptors() -> &'static [DeviceDescriptor] {
    MY_DESCRIPTORS
}
```

**A descriptor that resolves a `TransportIntent` cannot use this pattern.**
`resolve_current_transport` is a `const fn`, but `Result::expect` is not, so
`resolve_current_transport(my_hid_intent(0)).expect(...)` inside a
`static ...: &[DeviceDescriptor]` fails to compile with E0015, "cannot call
non-const method `Result::expect` in statics". Every resolve site in the tree
lives inside a `LazyLock` closure for exactly this reason, and all four plain
`static ...: &[DeviceDescriptor]` declarations (Lian Li, QMK, Dygma, Corsair
peripherals) name literal `TransportType` variants and resolve nothing. Use
Pattern 2 below when the transport has to be resolved:

```rust
static MY_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    vec![DeviceDescriptor {
        vendor_id: 0x1234,
        product_id: 0x5678,
        name: "My Device Pro (v2 firmware)",
        family: DeviceFamily::new_static("myvendor", "My Vendor"),
        transport: resolve_current_transport(my_hid_intent(0))
            .expect("HID transport should support the current platform"),
        protocol: ProtocolBinding {
            id: "myvendor/pro-v2",
            build: || Box::new(MyProtocolV2::new()),
        },
        firmware_predicate: Some(is_v2_firmware),
    }]
});

pub fn descriptors() -> &'static [DeviceDescriptor] {
    MY_DESCRIPTORS.as_slice()
}
```

Three things in there are load-bearing:

- `DeviceFamily` is a **struct**, not an enum. `DeviceFamily::MyFamily` does not exist. Use `new_static(id, name)` in const contexts; `new(id, name)` and `named(name)` are the runtime constructors and sanitize the id for you
- Never hardcode `TransportType::UsbHidRaw`. It is Linux-only, and no descriptor in the tree names it. Declare a `TransportIntent` and let `resolve_current_transport` map `HostManaged` to `UsbHidRaw` on Linux and `UsbHidApi` elsewhere
- `firmware_predicate` is `Option<fn(&str) -> bool>`. Write a named local predicate. `firmware_matches` is a private free function inside `drivers/lianli/devices.rs` and is not importable from anywhere else; the Lian Li descriptors reference `is_al_hid_firmware` and `is_al10_firmware`, which wrap it locally

When descriptors are near-identical across many PIDs, drivers wrap the whole literal in a `macro_rules!` (ASUS, Lian Li, Nollie, PrismRGB, Corsair all do this) so only the varying fields appear per device.

**Pattern 2: LazyLock** (required when any field needs a non-const call, including transport resolution, and used whenever descriptors are built programmatically, as with Razer's device catalog):

```rust
static RAZER_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::with_capacity(128);
    keyboards::push_all(&mut descriptors);
    mice::push_all(&mut descriptors);
    peripherals::push_all(&mut descriptors);
    laptops::push_all(&mut descriptors);
    mousepads::push_all(&mut descriptors);
    descriptors
});

pub fn descriptors() -> &'static [DeviceDescriptor] {
    RAZER_DESCRIPTORS.as_slice()
}
```

**Selection order for same-PID descriptors** (`ProtocolDatabase::lookup_with_firmware_for_driver_ids`): if a firmware string is known, the first candidate whose predicate matches wins. Otherwise the first candidate with **no** predicate wins, which is why a predicate-free fallback descriptor matters for devices whose firmware was never read. Only if every candidate carries a predicate does the first candidate win as a last resort. When `enabled_driver_ids` is supplied, descriptors whose driver ID is not in the set are skipped at every stage. The predicate receives the firmware string parsed out of `parse_response()` during init.

## Testing Without Hardware

Tests live in `crates/hypercolor-hal/tests/{feature}_tests.rs` and drive the public API, so import through `hypercolor_hal::` rather than reaching into private modules. The pattern is to assert on the exact prefix bytes of each command, which pins opcode, port, and color order in one go. From `tests/lianli_protocol_tests.rs`:

```rust
#[test]
fn sl_frame_encodes_activate_color_commit_and_frame_commit() {
    let protocol =
        Ene6k77Protocol::new(LianLiHubVariant::Sl).with_fan_counts([2, 0, 0, 0, 0, 0, 0, 0]);
    let colors = vec![[10, 20, 30]; 32];

    let commands = protocol.encode_frame(&colors);
    assert_eq!(commands.len(), 4);

    assert_eq!(commands[0].transfer_type, TransferType::HidReport);
    assert_eq!(&commands[0].data[..4], &[0xE0, 0x10, 0x32, 0x02]);

    assert_eq!(commands[1].transfer_type, TransferType::Primary);
    assert_eq!(commands[1].post_delay, ENE_COMMAND_DELAY);
    assert_eq!(commands[1].data.len(), 98);
    assert_eq!(&commands[1].data[..5], &[0xE0, 0x30, 10, 30, 20]);  // R-B-G

    assert_eq!(commands[2].transfer_type, TransferType::HidReport);
    assert_eq!(&commands[2].data[..6], &[0xE0, 0x10, 0x01, 0x02, 0x00, 0x00]);

    assert_eq!(commands[3].transfer_type, TransferType::HidReport);
    assert_eq!(&commands[3].data[..4], &[0xE0, 0x60, 0x00, 0x01]);
}
```

Always test:

- Packet count for various LED counts
- Packet sizes match wire expectations
- `transfer_type` and `post_delay` per phase, not just the payload bytes
- Color byte ordering (feed asymmetric colors like `[10, 20, 30]` so a swap is visible)
- Checksum correctness
- Chunking boundary behavior (exact multiples vs remainders)

`benches/protocol_encoding.rs` is the criterion bench for the encode hot path. Add a case there for any driver that will run in the render loop.
