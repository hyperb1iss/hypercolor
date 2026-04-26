# Protocol Implementation Walkthrough

Annotated patterns from existing Hypercolor drivers. Read the SKILL.md first — this is the deep-dive.

## Anatomy of encode_frame_into

From Lian Li ENE (three-phase: activate → color → commit):

```rust
fn encode_frame_into(&self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>) {
    commands.clear();
    let mut buffer = CommandBuffer::new(commands);
    let colors = normalize_colors(colors, self.total_leds() as usize);

    for group in 0..self.group_count() {
        let port = self.port_for_group(group);

        // Phase 1: Activate — tells controller which port receives data next
        let mut activate = EnePacket65::new_zeroed();
        activate.report_id = ENE_REPORT_ID;  // 0xE0
        activate.command = CMD_ACTIVATE;
        activate.arg0 = port;
        buffer.push_struct(&activate, false, Duration::ZERO, ENE_COMMAND_DELAY, TransferType::HidReport);

        // Phase 2: Color data — sent as output report (different transfer type!)
        let chunk = &colors[group_offset..group_offset + group_led_count];
        let mut color_pkt = EneColorPacket::new_zeroed();
        color_pkt.report_id = ENE_REPORT_ID;
        for (i, &[r, g, b]) in chunk.iter().enumerate() {
            color_pkt.colors[i * 3]     = r;
            color_pkt.colors[i * 3 + 1] = b;  // R-B-G byte order!
            color_pkt.colors[i * 3 + 2] = g;
        }
        buffer.push_struct(&color_pkt, false, Duration::ZERO, Duration::ZERO, TransferType::Primary);
    }

    // Phase 3: Commit — finalizes the frame
    let mut commit = EnePacket65::new_zeroed();
    commit.report_id = ENE_REPORT_ID;
    commit.command = CMD_COMMIT;
    buffer.push_struct(&commit, false, Duration::ZERO, ENE_COMMAND_DELAY, TransferType::HidReport);

    buffer.finish();
}
```

Key observations:

- `CommandBuffer::new(commands)` borrows the pre-existing vec — no allocation
- Different phases use different `TransferType` values (HidReport for commands, Primary for data)
- Color byte order is R-B-G — this is hardware-specific, not a bug
- `normalize_colors` uses `Cow` to avoid allocation when lengths match
- `buffer.finish()` truncates to actual command count

## Anatomy of parse_response

From ASUS Aura (runtime topology discovery):

```rust
fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
    // Response might have extra report ID byte — use read_from_prefix
    let resp = AuraConfigResponse::read_from_prefix(data)
        .map_err(|_| ProtocolError::MalformedResponse)?;

    match resp.command {
        CMD_CONFIG => {
            // Firmware reports zone counts — update topology
            let mut topo = self.topology.write()
                .map_err(|_| ProtocolError::InternalError)?;
            topo.mainboard_leds = resp.mainboard_count as usize;
            topo.header_count = resp.header_count as usize;
            Ok(ProtocolResponse::status(ResponseStatus::Ok))
        }
        CMD_FIRMWARE => {
            let fw = String::from_utf8_lossy(&resp.firmware_string);
            Ok(ProtocolResponse::firmware(fw.trim_end_matches('\0').to_string()))
        }
        _ => Ok(ProtocolResponse::status(ResponseStatus::Unknown))
    }
}
```

Note: ASUS uses `RwLock<AuraTopology>` for interior mutability because `parse_response` takes `&self` (Protocol is `&self` throughout), but topology is discovered at runtime.

## Chunking Patterns

### Razer: Row-Based Chunks (22 LEDs max)

```rust
for (row_idx, chunk) in colors.chunks(22).enumerate() {
    let mut report = RazerReport::new_zeroed();
    report.transaction_id = self.transaction_id();  // version-dependent!
    report.data_size = (chunk.len() * 3 + 4) as u8;
    report.command_class = CMD_EXTENDED_MATRIX;
    report.command_id = CMD_SET_CUSTOM;
    report.args[0] = 0x00;                  // storage
    report.args[1] = row_idx as u8;         // row
    report.args[2] = 0x00;                  // start column
    report.args[3] = (chunk.len() - 1) as u8;  // end column (inclusive!)
    for (i, &[r, g, b]) in chunk.iter().enumerate() {
        report.args[4 + i * 3] = r;
        report.args[5 + i * 3] = g;
        report.args[6 + i * 3] = b;        // Standard RGB order
    }
    report.crc = razer_crc(&report);        // XOR bytes [2..88) (bytes 2 through 87 inclusive)
    buffer.push_struct(&report, false, Duration::ZERO, Duration::from_millis(1), TransferType::Primary);
}
// Activation command after all chunks
```

Key: End column is **inclusive** (`len() - 1`), not exclusive. CRC covers a fixed range regardless of data size.

### Corsair: Component-Separated Channels

```rust
for channel in 0..self.channel_count {
    for component in [Component::R, Component::G, Component::B] {
        let mut pkt = CorsairDirectPacket::new_zeroed();
        pkt.report_id = 0x00;
        pkt.channel = channel;
        pkt.component = component as u8;
        for (i, &[r, g, b]) in channel_leds.iter().take(50).enumerate() {
            pkt.values[i] = match component {
                Component::R => r,
                Component::G => g,
                Component::B => b,
            };
        }
        buffer.push_struct(&pkt, false, Duration::ZERO, Duration::ZERO, TransferType::Primary);
    }
}
// Commit after all channels
```

Corsair sends R values, then G values, then B values — per channel. Not interleaved RGB.

## Device Descriptor Factory Pattern

Each driver module exports a `descriptors() -> &'static [DeviceDescriptor]` function. Two patterns exist:

**Pattern 1: Static slice** (preferred when all fields are const-compatible):

```rust
// In src/drivers/mydevice/devices.rs
static MY_DESCRIPTORS: &[DeviceDescriptor] = &[
    DeviceDescriptor {
        vendor_id: 0x1234,
        product_id: 0x5678,
        name: "My Device Pro",
        family: DeviceFamily::MyFamily,
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
        family: DeviceFamily::MyFamily,
        transport: TransportType::UsbHidRaw {
            interface: 0,
            report_id: 0x00,
            report_mode: HidRawReportMode::FeatureReport,
            usage_page: None,
            usage: None,
        },
        protocol: ProtocolBinding {
            id: "myvendor/pro-v2",
            build: || Box::new(MyProtocolV2::new()),
        },
        firmware_predicate: Some(|fw| firmware_matches(fw, "2.")),
    },
];

pub fn descriptors() -> &'static [DeviceDescriptor] {
    MY_DESCRIPTORS
}
```

**Pattern 2: LazyLock** (when descriptors are built programmatically, e.g. Razer's 100+ devices):

```rust
static MY_DESCRIPTORS: LazyLock<Vec<DeviceDescriptor>> = LazyLock::new(|| {
    let mut descriptors = Vec::with_capacity(64);
    // ... programmatic construction ...
    descriptors
});

pub fn descriptors() -> &'static [DeviceDescriptor] {
    MY_DESCRIPTORS.as_slice()
}
```

When two descriptors share the same VID/PID, the one with `firmware_predicate` is tried first. Predicate receives the firmware string from `parse_response()` during init.

## Testing Without Hardware

```rust
#[test]
fn encode_frame_produces_correct_packet_count() {
    let protocol = MyProtocol::new(MyVariant::Standard);
    let colors: Vec<[u8; 3]> = (0..protocol.total_leds())
        .map(|i| [i as u8, 0, 0])
        .collect();
    let commands = protocol.encode_frame(&colors);

    // Expected: 1 activate + N color chunks + 1 commit
    assert_eq!(commands.len(), expected_count);

    // Verify packet sizes
    for cmd in &commands {
        assert_eq!(cmd.data.len(), EXPECTED_PACKET_SIZE);
    }

    // Verify color byte order in first color packet
    let color_pkt = &commands[1].data;
    assert_eq!(color_pkt[OFFSET_R], 0x00);  // first LED red
    assert_eq!(color_pkt[OFFSET_B], 0x00);  // check byte order!
    assert_eq!(color_pkt[OFFSET_G], 0x00);
}
```

Always test:

- Packet count for various LED counts
- Packet sizes match wire expectations
- Color byte ordering
- Checksum correctness
- Chunking boundary behavior (exact multiples vs remainders)
