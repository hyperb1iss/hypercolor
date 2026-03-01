# Hypercolor USB HID Device Backend Specification

> Definitive protocol reference for all PrismRGB and Nollie controller families. Byte-level packet formats, initialization sequences, render loops, and Rust implementation types.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Common Types](#3-common-types)
4. [Prism 8 Protocol](#4-prism-8-protocol)
5. [Nollie 8 v2 Protocol](#5-nollie-8-v2-protocol)
6. [Prism S Protocol](#6-prism-s-protocol)
7. [Prism Mini Protocol](#7-prism-mini-protocol)
8. [HidController Trait](#8-hidcontroller-trait)
9. [Error Handling](#9-error-handling)
10. [Platform Setup](#10-platform-setup)
11. [Implementation Notes](#11-implementation-notes)

---

## 1. Overview

All PrismRGB and Nollie controllers communicate over USB HID with 65-byte feature/output reports. Byte 0 is always the HID report ID (`0x00`), leaving 64 bytes of payload per packet. The controllers share a common command vocabulary (`0xFC` for queries, `0xFE` for configuration writes) but differ in channel layout, color format, and packetization strategy.

### Transport Layer

```
┌──────────────────────────────────────────────────────────┐
│                    USB HID Report (65 bytes)              │
├──────┬───────────────────────────────────────────────────┤
│ [0]  │ Report ID: always 0x00                            │
│ [1]  │ Payload byte 0 (packet_id, command, or data)      │
│ [2]  │ Payload byte 1                                    │
│ ...  │ ...                                               │
│ [64] │ Payload byte 63                                   │
└──────┴───────────────────────────────────────────────────┘

Total: 1 byte report ID + 64 bytes payload = 65 bytes per write
All unused bytes MUST be zero-padded.
```

### Command Prefixes

| Prefix | Direction | Purpose |
|--------|-----------|---------|
| `0xFC` | Write then Read | Query commands (firmware version, channel counts, voltage) |
| `0xFE` | Write only | Configuration commands (hardware effect, settings save, channel update) |
| `0xFF` | Write only | Frame commit / latch (Prism 8, Nollie 8 only) |
| `0xAA` | Write only | Data marker (Prism Mini only) |
| `0xBB` | Write only | Hardware lighting config (Prism Mini only) |
| `0xCC` | Write only | Firmware version query (Prism Mini only) |

---

## 2. Device Registry

| Device | VID | PID | HID Interface | Channels | LEDs/Channel | Max LEDs | Color Format | Brightness Scale |
|--------|-----|-----|---------------|----------|--------------|----------|-------------|-----------------|
| **Prism 8** | `0x16D5` | `0x1F01` | 0 | 8 | 126 | 1008 | GRB | 0.75 |
| **Nollie 8 v2** | `0x16D2` | `0x1F01` | 0 | 8 | 126 | 1008 | GRB | 1.00 |
| **Prism S** | `0x16D0` | `0x1294` | 2 | 2 | variable | 282 | RGB | 0.50 |
| **Prism Mini** | `0x16D0` | `0x1407` | 2 | 1 | 128 | 128 | RGB | 1.00 |

```rust
/// USB Vendor/Product ID pairs for device detection
pub const PRISM_8_VID: u16     = 0x16D5;
pub const PRISM_8_PID: u16     = 0x1F01;
pub const NOLLIE_8_VID: u16    = 0x16D2;
pub const NOLLIE_8_PID: u16    = 0x1F01;
pub const PRISM_S_VID: u16     = 0x16D0;
pub const PRISM_S_PID: u16     = 0x1294;
pub const PRISM_MINI_VID: u16  = 0x16D0;
pub const PRISM_MINI_PID: u16  = 0x1407;

/// HID interface numbers
pub const PRISM_8_INTERFACE: i32     = 0;
pub const NOLLIE_8_INTERFACE: i32    = 0;
pub const PRISM_S_INTERFACE: i32     = 2;
pub const PRISM_MINI_INTERFACE: i32  = 2;
```

---

## 3. Common Types

### Color Formats

```rust
/// Color byte ordering for LED data transmission
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFormat {
    /// Red, Green, Blue — used by Prism S, Prism Mini
    Rgb,
    /// Green, Red, Blue — used by Prism 8, Nollie 8
    Grb,
}

impl ColorFormat {
    /// Encode an (R, G, B) triple into the device's native byte order
    pub fn encode(&self, r: u8, g: u8, b: u8) -> [u8; 3] {
        match self {
            ColorFormat::Rgb => [r, g, b],
            ColorFormat::Grb => [g, r, b],
        }
    }
}
```

### Packet Builder

```rust
/// Fixed-size HID report buffer
pub const HID_REPORT_SIZE: usize = 65;

/// A 65-byte HID output report with report ID 0x00
#[derive(Clone)]
pub struct PrismPacket {
    buf: [u8; HID_REPORT_SIZE],
    cursor: usize,
}

impl PrismPacket {
    /// Create a new packet with report ID 0x00 and zero-filled payload
    pub fn new() -> Self {
        Self {
            buf: [0u8; HID_REPORT_SIZE],
            cursor: 1, // skip report ID byte
        }
    }

    /// Create a packet with the first payload byte set (e.g., packet_id or command prefix)
    pub fn with_header(byte: u8) -> Self {
        let mut pkt = Self::new();
        pkt.buf[1] = byte;
        pkt.cursor = 2;
        pkt
    }

    /// Create a command packet: [0x00, prefix, subcommand, ...]
    pub fn command(prefix: u8, subcommand: u8) -> Self {
        let mut pkt = Self::new();
        pkt.buf[1] = prefix;
        pkt.buf[2] = subcommand;
        pkt.cursor = 3;
        pkt
    }

    /// Append a single byte at the current cursor position
    pub fn push(&mut self, byte: u8) -> &mut Self {
        assert!(self.cursor < HID_REPORT_SIZE, "packet overflow");
        self.buf[self.cursor] = byte;
        self.cursor += 1;
        self
    }

    /// Append a slice of bytes starting at the current cursor
    pub fn extend(&mut self, data: &[u8]) -> &mut Self {
        let end = self.cursor + data.len();
        assert!(end <= HID_REPORT_SIZE, "packet overflow: {} + {} > {}", self.cursor, data.len(), HID_REPORT_SIZE);
        self.buf[self.cursor..end].copy_from_slice(data);
        self.cursor = end;
        self
    }

    /// Set a byte at an absolute offset (0-indexed from report ID)
    pub fn set(&mut self, offset: usize, byte: u8) -> &mut Self {
        assert!(offset < HID_REPORT_SIZE, "offset out of bounds");
        self.buf[offset] = byte;
        self
    }

    /// Return the raw 65-byte buffer for hid_write
    pub fn as_bytes(&self) -> &[u8; HID_REPORT_SIZE] {
        &self.buf
    }
}
```

### Device Info

```rust
/// Identifies a discovered PrismRGB/Nollie device
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub vid: u16,
    pub pid: u16,
    pub interface: i32,
    pub serial: Option<String>,
    pub product: String,
    pub firmware_version: Option<u8>,
    pub device_type: HidDeviceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidDeviceType {
    Prism8,
    Nollie8,
    PrismS,
    PrismMini,
}

impl HidDeviceType {
    pub fn color_format(&self) -> ColorFormat {
        match self {
            Self::Prism8 | Self::Nollie8 => ColorFormat::Grb,
            Self::PrismS | Self::PrismMini => ColorFormat::Rgb,
        }
    }

    pub fn brightness_scale(&self) -> f32 {
        match self {
            Self::Prism8 => 0.75,
            Self::PrismS => 0.50,
            Self::Nollie8 | Self::PrismMini => 1.00,
        }
    }

    pub fn max_leds(&self) -> u16 {
        match self {
            Self::Prism8 | Self::Nollie8 => 1008,
            Self::PrismS => 282,
            Self::PrismMini => 128,
        }
    }
}
```

---

## 4. Prism 8 Protocol

**Device:** PrismRGB Prism 8 Controller
**VID/PID:** `0x16D5` / `0x1F01` | **Interface:** 0
**Color format:** GRB | **Brightness scale:** 0.75
**Channels:** 8, each up to 126 LEDs (6 packets x 21 LEDs)

### 4.1 Initialization Sequence

Initialization consists of three steps executed in order. Each write is a 65-byte HID report. All responses are read as 65-byte HID reports with a configurable timeout.

#### Step 1: Query Firmware Version

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFC │ Command prefix (query)           │
│ 2      │ 1    │ 0x01 │ Subcommand: firmware version     │
│ 3-64   │ 62   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

READ ← 65 bytes
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0      │ 1    │ Report ID (device-dependent)            │
│ 1      │ 1    │ Reserved                                │
│ 2      │ 1    │ Firmware version (uint8)                │
│ 3-64   │ 62   │ Reserved                                │
└────────┴──────┴────────────────────────────────────────┘
```

```rust
impl Prism8Controller {
    fn query_firmware_version(&mut self) -> Result<u8, HidError> {
        let pkt = PrismPacket::command(0xFC, 0x01);
        self.device.write(pkt.as_bytes())?;

        let mut response = [0u8; HID_REPORT_SIZE];
        self.device.read_timeout(&mut response, 1000)?;

        Ok(response[2])
    }
}
```

#### Step 2: Query Channel LED Counts

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFC │ Command prefix (query)           │
│ 2      │ 1    │ 0x03 │ Subcommand: channel LED counts   │
│ 3-64   │ 62   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

READ ← 65 bytes  (8 channels × 2 bytes = 16 bytes of data)
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0-1    │ 2    │ Channel 0 LED count (uint16, big-endian)│
│ 2-3    │ 2    │ Channel 1 LED count (uint16, big-endian)│
│ 4-5    │ 2    │ Channel 2 LED count (uint16, big-endian)│
│ 6-7    │ 2    │ Channel 3 LED count (uint16, big-endian)│
│ 8-9    │ 2    │ Channel 4 LED count (uint16, big-endian)│
│ 10-11  │ 2    │ Channel 5 LED count (uint16, big-endian)│
│ 12-13  │ 2    │ Channel 6 LED count (uint16, big-endian)│
│ 14-15  │ 2    │ Channel 7 LED count (uint16, big-endian)│
│ 16-64  │ 49   │ Reserved                                │
└────────┴──────┴────────────────────────────────────────┘
```

```rust
/// Channel configuration parsed from the device
pub struct ChannelConfig {
    pub led_counts: [u16; 8],
}

impl Prism8Controller {
    fn query_channel_config(&mut self) -> Result<ChannelConfig, HidError> {
        let pkt = PrismPacket::command(0xFC, 0x03);
        self.device.write(pkt.as_bytes())?;

        let mut response = [0u8; HID_REPORT_SIZE];
        self.device.read_timeout(&mut response, 1000)?;

        let mut led_counts = [0u16; 8];
        for ch in 0..8 {
            let offset = ch * 2;
            led_counts[ch] = u16::from_be_bytes([
                response[offset],
                response[offset + 1],
            ]);
        }

        Ok(ChannelConfig { led_counts })
    }
}
```

#### Step 3: Set Hardware Effect (Idle Fallback)

Sets the static color the controller displays when the host software is not actively streaming frames. This ensures LEDs don't go dark if the daemon crashes or stops.

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFE │ Command prefix (config write)    │
│ 2      │ 1    │ 0x02 │ Subcommand: set hardware effect  │
│ 3      │ 1    │ 0x00 │ Reserved                         │
│ 4      │ 1    │ R    │ Red component (0-255)            │
│ 5      │ 1    │ G    │ Green component (0-255)          │
│ 6      │ 1    │ B    │ Blue component (0-255)           │
│ 7      │ 1    │ 0x64 │ Brightness (100%)                │
│ 8      │ 1    │ 0x0A │ Effect speed (10)                │
│ 9      │ 1    │ 0x00 │ Reserved                         │
│ 10     │ 1    │ 0x01 │ Effect enable flag               │
│ 11-64  │ 54   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘
```

```rust
/// Hardware fallback effect configuration
pub struct HardwareEffect {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub brightness: u8, // 0-100
    pub speed: u8,      // 1-20
    pub enabled: bool,
}

impl Default for HardwareEffect {
    fn default() -> Self {
        Self { r: 0, g: 0, b: 0, brightness: 100, speed: 10, enabled: true }
    }
}

impl Prism8Controller {
    fn set_hardware_effect(&mut self, effect: &HardwareEffect) -> Result<(), HidError> {
        let mut pkt = PrismPacket::command(0xFE, 0x02);
        pkt.push(0x00)                        // reserved
           .push(effect.r)
           .push(effect.g)
           .push(effect.b)
           .push(effect.brightness)
           .push(effect.speed)
           .push(0x00)                        // reserved
           .push(if effect.enabled { 0x01 } else { 0x00 });
        self.device.write(pkt.as_bytes())?;
        Ok(())
    }
}
```

### 4.2 Render Loop

The render loop transmits LED color data for all active channels, then sends a frame commit to latch the data. Runs at 33fps or 60fps.

#### Packet Addressing Scheme

Each channel occupies a fixed 6-packet block. The `packet_id` encodes both the channel index and the position within that channel:

```
packet_id = packet_index + (channel * 6)

Channel 0:  packets  0,  1,  2,  3,  4,  5
Channel 1:  packets  6,  7,  8,  9, 10, 11
Channel 2:  packets 12, 13, 14, 15, 16, 17
Channel 3:  packets 18, 19, 20, 21, 22, 23
Channel 4:  packets 24, 25, 26, 27, 28, 29
Channel 5:  packets 30, 31, 32, 33, 34, 35
Channel 6:  packets 36, 37, 38, 39, 40, 41
Channel 7:  packets 42, 43, 44, 45, 46, 47

Each packet carries 21 LEDs × 3 bytes = 63 bytes of color data
6 packets × 21 LEDs = 126 LEDs max per channel
8 channels × 126 LEDs = 1008 LEDs max total
```

#### Color Data Packet

```
WRITE → 65 bytes
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0      │ 1    │ Report ID: 0x00                        │
│ 1      │ 1    │ Packet ID (0-47, see addressing above) │
│ 2-4    │ 3    │ LED 0: G, R, B                         │
│ 5-7    │ 3    │ LED 1: G, R, B                         │
│ 8-10   │ 3    │ LED 2: G, R, B                         │
│ ...    │ ...  │ ...                                     │
│ 62-64  │ 3    │ LED 20: G, R, B                        │
└────────┴──────┴────────────────────────────────────────┘

Color order: GRB (Green first, then Red, then Blue)
Max LEDs per packet: 21 (63 bytes / 3 bytes per LED)
Unused LED slots in the final packet: zero-padded
```

```rust
/// Maximum LEDs per data packet
const LEDS_PER_PACKET: usize = 21;
/// Maximum packets per channel
const PACKETS_PER_CHANNEL: usize = 6;
/// Maximum channels
const MAX_CHANNELS: usize = 8;

impl Prism8Controller {
    fn send_channel_data(
        &mut self,
        channel: u8,
        colors: &[(u8, u8, u8)],  // (R, G, B) tuples
    ) -> Result<(), HidError> {
        let scale = self.brightness_scale(); // 0.75 for Prism 8

        for (pkt_idx, chunk) in colors.chunks(LEDS_PER_PACKET).enumerate() {
            let packet_id = pkt_idx as u8 + (channel * PACKETS_PER_CHANNEL as u8);
            let mut pkt = PrismPacket::with_header(packet_id);

            for &(r, g, b) in chunk {
                // Apply brightness scaling and encode as GRB
                let r_scaled = (r as f32 * scale) as u8;
                let g_scaled = (g as f32 * scale) as u8;
                let b_scaled = (b as f32 * scale) as u8;
                let encoded = ColorFormat::Grb.encode(r_scaled, g_scaled, b_scaled);
                pkt.extend(&encoded);
            }

            self.device.write(pkt.as_bytes())?;
        }

        Ok(())
    }
}
```

#### Frame Commit

After all channel data has been transmitted, a commit packet latches the new colors.

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFF │ Frame commit / latch command      │
│ 2-64   │ 63   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘
```

```rust
/// Frame commit byte — signals the controller to latch all pending color data
const FRAME_COMMIT: u8 = 0xFF;

impl Prism8Controller {
    fn commit_frame(&mut self) -> Result<(), HidError> {
        let pkt = PrismPacket::with_header(FRAME_COMMIT);
        self.device.write(pkt.as_bytes())?;
        Ok(())
    }
}
```

### 4.3 Voltage Monitoring

Available on firmware v2+. Sent every 150 frames (~2.5 seconds at 60fps) to monitor USB and SATA power rail voltages. Useful for detecting power issues with high-LED-count setups.

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFC │ Command prefix (query)           │
│ 2      │ 1    │ 0x1A │ Subcommand: read voltages        │
│ 3-64   │ 62   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

READ ← 65 bytes
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0      │ 1    │ Reserved                                │
│ 1-2    │ 2    │ USB voltage (uint16 LE, millivolts)     │
│ 3-4    │ 2    │ SATA 1 voltage (uint16 LE, millivolts)  │
│ 5-6    │ 2    │ SATA 2 voltage (uint16 LE, millivolts)  │
│ 7-64   │ 58   │ Reserved                                │
└────────┴──────┴────────────────────────────────────────┘

Voltage calculation: value_f32 = uint16_value as f32 / 1000.0 (volts)
Expected ranges: USB ~5.0V, SATA ~5.0V or ~12.0V
```

```rust
/// Power rail voltage readings from the controller
pub struct VoltageReading {
    pub usb_volts: f32,
    pub sata1_volts: f32,
    pub sata2_volts: f32,
}

/// Voltage monitoring interval (frames between reads)
const VOLTAGE_POLL_INTERVAL: u32 = 150;

impl Prism8Controller {
    fn read_voltage(&mut self) -> Result<VoltageReading, HidError> {
        let pkt = PrismPacket::command(0xFC, 0x1A);
        self.device.write(pkt.as_bytes())?;

        let mut response = [0u8; HID_REPORT_SIZE];
        self.device.read_timeout(&mut response, 1000)?;

        Ok(VoltageReading {
            usb_volts: u16::from_le_bytes([response[1], response[2]]) as f32 / 1000.0,
            sata1_volts: u16::from_le_bytes([response[3], response[4]]) as f32 / 1000.0,
            sata2_volts: u16::from_le_bytes([response[5], response[6]]) as f32 / 1000.0,
        })
    }
}
```

### 4.4 Dynamic Channel Update

When the number of LEDs connected to a channel changes (hot-plug), the host can write the updated counts back to the controller.

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFE │ Command prefix (config write)    │
│ 2      │ 1    │ 0x03 │ Subcommand: update channel counts│
│ 3-4    │ 2    │      │ Channel 0 LED count (uint16 LE)  │
│ 5-6    │ 2    │      │ Channel 1 LED count (uint16 LE)  │
│ 7-8    │ 2    │      │ Channel 2 LED count (uint16 LE)  │
│ 9-10   │ 2    │      │ Channel 3 LED count (uint16 LE)  │
│ 11-12  │ 2    │      │ Channel 4 LED count (uint16 LE)  │
│ 13-14  │ 2    │      │ Channel 5 LED count (uint16 LE)  │
│ 15-16  │ 2    │      │ Channel 6 LED count (uint16 LE)  │
│ 17-18  │ 2    │      │ Channel 7 LED count (uint16 LE)  │
│ 19-64  │ 46   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

Note: Query (0xFC 0x03) returns big-endian, but update (0xFE 0x03)
uses little-endian. This asymmetry is confirmed from the original
SignalRGB driver source.
```

```rust
impl Prism8Controller {
    fn update_channel_counts(&mut self, counts: &[u16; 8]) -> Result<(), HidError> {
        let mut pkt = PrismPacket::command(0xFE, 0x03);
        for &count in counts {
            let bytes = count.to_le_bytes();
            pkt.extend(&bytes);
        }
        self.device.write(pkt.as_bytes())?;
        Ok(())
    }
}
```

### 4.5 Shutdown Sequence

Graceful shutdown restores the hardware fallback effect so LEDs don't go dark.

```
Shutdown sequence:
1. Send all channels filled with the shutdown color (GRB encoded)
2. Commit the frame (0xFF)
3. Set hardware effect with desired idle color (0xFE 0x02)
4. Activate hardware mode (0xFE 0x01)
```

```
Step 4 — Activate Hardware Mode:
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFE │ Command prefix (config write)    │
│ 2      │ 1    │ 0x01 │ Subcommand: activate HW mode     │
│ 3      │ 1    │ 0x00 │ Reserved                         │
│ 4-64   │ 61   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘
```

```rust
impl Prism8Controller {
    fn shutdown(&mut self, idle_color: (u8, u8, u8)) -> Result<(), HidError> {
        let (r, g, b) = idle_color;

        // 1. Fill all channels with shutdown color
        for ch in 0..MAX_CHANNELS as u8 {
            let led_count = self.channel_config.led_counts[ch as usize] as usize;
            let colors = vec![(r, g, b); led_count];
            self.send_channel_data(ch, &colors)?;
        }

        // 2. Commit the frame
        self.commit_frame()?;

        // 3. Set hardware fallback effect
        self.set_hardware_effect(&HardwareEffect {
            r, g, b,
            brightness: 100,
            speed: 10,
            enabled: true,
        })?;

        // 4. Activate hardware mode
        let mut pkt = PrismPacket::command(0xFE, 0x01);
        pkt.push(0x00);
        self.device.write(pkt.as_bytes())?;

        Ok(())
    }
}
```

### 4.6 Complete Render Frame

Putting it all together, a single render frame for the Prism 8:

```rust
impl Prism8Controller {
    /// Push a complete frame to all channels.
    /// `frame_colors` is indexed by channel, each containing (R, G, B) tuples.
    fn render_frame(
        &mut self,
        frame_colors: &[Vec<(u8, u8, u8)>; 8],
        frame_counter: &mut u32,
    ) -> Result<(), HidError> {
        // Send color data for each active channel
        for (ch, colors) in frame_colors.iter().enumerate() {
            if !colors.is_empty() {
                self.send_channel_data(ch as u8, colors)?;
            }
        }

        // Latch the frame
        self.commit_frame()?;

        // Periodic voltage monitoring (firmware v2+)
        *frame_counter += 1;
        if self.firmware_version >= 2 && *frame_counter % VOLTAGE_POLL_INTERVAL == 0 {
            match self.read_voltage() {
                Ok(v) => {
                    tracing::debug!(
                        usb_v = v.usb_volts,
                        sata1_v = v.sata1_volts,
                        sata2_v = v.sata2_volts,
                        "voltage reading"
                    );
                }
                Err(e) => {
                    tracing::warn!(?e, "failed to read voltage");
                }
            }

            // Re-query channel counts in case of hot-plug
            if let Ok(config) = self.query_channel_config() {
                if config.led_counts != self.channel_config.led_counts {
                    self.update_channel_counts(&config.led_counts)?;
                    self.channel_config = config;
                }
            }
        }

        Ok(())
    }
}
```

---

## 5. Nollie 8 v2 Protocol

**Device:** Nollie 8 v2 Controller
**VID/PID:** `0x16D2` / `0x1F01` | **Interface:** 0
**Color format:** GRB | **Brightness scale:** 1.00

### 5.1 Protocol Equivalence

The Nollie 8 v2 uses the **exact same protocol** as the Prism 8. The only differences:

| Property | Prism 8 | Nollie 8 v2 |
|----------|---------|-------------|
| Vendor ID | `0x16D5` | `0x16D2` |
| Product ID | `0x1F01` | `0x1F01` (identical) |
| Brightness scale | 0.75 | 1.00 (no scaling) |

Every packet format, initialization sequence, render loop, voltage monitoring, and shutdown procedure described in [Section 4](#4-prism-8-protocol) applies identically. The implementation should share all protocol logic and parameterize only the VID and brightness scale.

```rust
/// Unified controller for both Prism 8 and Nollie 8 v2
pub struct EightChannelController {
    device: HidDevice,
    device_type: HidDeviceType,  // Prism8 or Nollie8
    firmware_version: u8,
    channel_config: ChannelConfig,
    frame_counter: u32,
}

impl EightChannelController {
    pub fn new(device: HidDevice, device_type: HidDeviceType) -> Result<Self, HidError> {
        assert!(matches!(device_type, HidDeviceType::Prism8 | HidDeviceType::Nollie8));
        let mut ctrl = Self {
            device,
            device_type,
            firmware_version: 0,
            channel_config: ChannelConfig { led_counts: [0; 8] },
            frame_counter: 0,
        };
        ctrl.init()?;
        Ok(ctrl)
    }

    fn brightness_scale(&self) -> f32 {
        self.device_type.brightness_scale()
    }
}
```

---

## 6. Prism S Protocol

**Device:** PrismRGB Prism S (Strimer Controller)
**VID/PID:** `0x16D0` / `0x1294` | **Interface:** 2
**Color format:** RGB | **Brightness scale:** 0.50
**Channels:** 2 (ATX cable + GPU cable)

### 6.1 Strimer Cable Types

The Prism S supports one ATX cable and one GPU cable connected simultaneously:

| Cable | LED Count | Grid Layout | Data Size |
|-------|----------|-------------|-----------|
| **24-pin ATX Strimer** | 120 | 20 columns x 6 rows | 360 bytes |
| **Dual 8-pin GPU Strimer** | 108 | 27 columns x 4 rows | 324 bytes |
| **Triple 8-pin GPU Strimer** | 162 | 27 columns x 6 rows | 486 bytes |

```rust
/// Strimer cable configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrimerCable {
    /// 24-pin ATX Strimer: 120 LEDs in a 20x6 grid
    Atx24Pin,
    /// Dual 8-pin GPU Strimer: 108 LEDs in a 27x4 grid
    GpuDual8Pin,
    /// Triple 8-pin GPU Strimer: 162 LEDs in a 27x6 grid
    GpuTriple8Pin,
}

impl StrimerCable {
    pub fn led_count(&self) -> u16 {
        match self {
            Self::Atx24Pin => 120,
            Self::GpuDual8Pin => 108,
            Self::GpuTriple8Pin => 162,
        }
    }

    pub fn grid_dimensions(&self) -> (u16, u16) {
        match self {
            Self::Atx24Pin => (20, 6),
            Self::GpuDual8Pin => (27, 4),
            Self::GpuTriple8Pin => (27, 6),
        }
    }

    pub fn cable_mode_byte(&self) -> u8 {
        match self {
            Self::Atx24Pin => 0xFF,        // not used in cable_mode field
            Self::GpuTriple8Pin => 0x00,
            Self::GpuDual8Pin => 0x01,
        }
    }
}
```

### 6.2 Initialization — Settings Save

The Prism S does not have a firmware version query. Initialization consists of saving the shutdown/idle color and cable configuration.

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0xFE │ Command prefix (config write)    │
│ 2      │ 1    │ 0x01 │ Subcommand: save settings        │
│ 3      │ 1    │ R    │ Idle color red (0-255)           │
│ 4      │ 1    │ G    │ Idle color green (0-255)         │
│ 5      │ 1    │ B    │ Idle color blue (0-255)          │
│ 6      │ 1    │ mode │ Cable mode: 0=Triple, 1=Dual     │
│ 7-64   │ 58   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

IMPORTANT: Wait 50ms after this write before sending any further packets.
```

```rust
pub struct PrismSConfig {
    pub idle_color: (u8, u8, u8),
    pub gpu_cable: StrimerCable,  // GpuDual8Pin or GpuTriple8Pin
}

impl PrismSController {
    fn save_settings(&mut self, config: &PrismSConfig) -> Result<(), HidError> {
        let (r, g, b) = config.idle_color;
        let mut pkt = PrismPacket::command(0xFE, 0x01);
        pkt.push(r).push(g).push(b).push(config.gpu_cable.cable_mode_byte());
        self.device.write(pkt.as_bytes())?;

        // Required 50ms pause after settings save
        std::thread::sleep(std::time::Duration::from_millis(50));
        Ok(())
    }
}
```

### 6.3 Render Loop — Combined Buffer Strategy

Unlike the Prism 8's per-channel packet addressing, the Prism S builds a single contiguous buffer containing both ATX and GPU cable data, then transmits it as sequential 64-byte chunks. There is no frame commit byte — data latches automatically when transmission completes.

#### Buffer Layout

The buffer is constructed logically and then split into 64-byte chunks for transmission. The layout varies based on which cables are connected.

```
ATX + GPU Combined Buffer:

  ┌───────────────────────────────────────────────────────────┐
  │ ATX Cable Data (if connected)                             │
  ├───────┬────────────────────────────────────────────────────┤
  │ Pkt 0 │ Bytes 0-62:   LEDs 0-20 RGB (63 bytes)           │
  │ Pkt 1 │ Bytes 63-125: LEDs 21-41 RGB (63 bytes)          │
  │ Pkt 2 │ Bytes 126-188: LEDs 42-62 RGB (63 bytes)         │
  │ Pkt 3 │ Bytes 189-251: LEDs 63-83 RGB (63 bytes)         │
  │ Pkt 4 │ Bytes 252-314: LEDs 84-104 RGB (63 bytes)        │
  │Pkt 15 │ Bytes 315-359: LEDs 105-119 RGB (45 bytes)       │
  ├───────┼────────────────────────────────────────────────────┤
  │       │ Byte 320: GPU marker = 0x05                       │
  │       │ Bytes 321-359: First 13 GPU LEDs (39 bytes inline)│
  ├───────┼────────────────────────────────────────────────────┤
  │ GPU Cable Data (continuing after marker)                   │
  │ Pkt 6 │ LEDs continuing... 63 bytes each                  │
  │ Pkt 7 │                                                    │
  │ ...   │                                                    │
  │Pkt 20 │ Final GPU packet (Dual 8-pin)                     │
  │  or   │                                                    │
  │Pkt 13 │ Final GPU packet (Triple 8-pin)                   │
  └───────┴────────────────────────────────────────────────────┘

GPU only (no ATX):

  ┌───────────────────────────────────────────────────────────┐
  │ Byte 0: GPU marker = 0x05                                │
  │ Bytes 1-63: Zero fill                                     │
  │ Then: GPU cable data in subsequent 64-byte chunks         │
  └───────────────────────────────────────────────────────────┘
```

#### Transmission Protocol

```
For each 64-byte chunk of the combined buffer:

WRITE → 65 bytes
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0      │ 1    │ Report ID: 0x00                        │
│ 1-64   │ 64   │ Buffer chunk data (raw RGB bytes)      │
└────────┴──────┴────────────────────────────────────────┘

No explicit frame commit — data latches on transmission completion.
No packet_id field — chunks are sent sequentially and the controller
reconstructs the buffer by byte offset.
```

```rust
/// GPU marker byte — separates ATX data from GPU data in the combined buffer
const GPU_MARKER: u8 = 0x05;
/// Maximum bytes per HID payload (64 bytes, packet is 65 with report ID)
const HID_PAYLOAD_SIZE: usize = 64;

impl PrismSController {
    fn render_frame(
        &mut self,
        atx_colors: Option<&[(u8, u8, u8)]>,   // up to 120 LEDs
        gpu_colors: Option<&[(u8, u8, u8)]>,    // up to 108 or 162 LEDs
    ) -> Result<(), HidError> {
        let scale = 0.50_f32; // Prism S brightness multiplier
        let mut buffer = Vec::new();

        // --- ATX cable data ---
        if let Some(colors) = atx_colors {
            for &(r, g, b) in colors {
                buffer.push((r as f32 * scale) as u8);
                buffer.push((g as f32 * scale) as u8);
                buffer.push((b as f32 * scale) as u8);
            }
            // Pad ATX section to byte 320 if needed
            while buffer.len() < 320 {
                buffer.push(0x00);
            }
        }

        // --- GPU marker + GPU cable data ---
        if gpu_colors.is_some() {
            if atx_colors.is_none() {
                // No ATX: marker at start, then zero-fill first chunk
                buffer.push(GPU_MARKER);
                buffer.resize(HID_PAYLOAD_SIZE, 0x00);
            } else {
                // ATX present: marker is at byte 320
                buffer[320] = GPU_MARKER;
            }
        }

        if let Some(colors) = gpu_colors {
            // GPU LED data follows the marker (inline or in subsequent packets)
            let gpu_start = if atx_colors.is_some() { 321 } else { HID_PAYLOAD_SIZE };

            // Ensure buffer is long enough for the GPU data insertion point
            while buffer.len() < gpu_start {
                buffer.push(0x00);
            }

            for &(r, g, b) in colors {
                buffer.push((r as f32 * scale) as u8);
                buffer.push((g as f32 * scale) as u8);
                buffer.push((b as f32 * scale) as u8);
            }
        }

        // --- Transmit: split buffer into 64-byte chunks ---
        for chunk in buffer.chunks(HID_PAYLOAD_SIZE) {
            let mut pkt = PrismPacket::new();
            pkt.extend(chunk);
            self.device.write(pkt.as_bytes())?;
        }

        Ok(())
    }
}
```

### 6.4 Key Differences from Prism 8

| Feature | Prism 8 / Nollie 8 | Prism S |
|---------|-------------------|---------|
| Color format | GRB | RGB |
| Brightness scale | 0.75 / 1.00 | 0.50 |
| Frame commit | Explicit `0xFF` byte | Implicit (on completion) |
| Firmware version query | Yes (`0xFC 0x01`) | No |
| Voltage monitoring | Yes (`0xFC 0x1A`) | No |
| Packet addressing | `packet_id = idx + ch*6` | Sequential byte stream |
| HID interface | 0 | 2 |
| Channel model | 8 independent channels | 2 cables in combined buffer |

### 6.5 Shutdown

```rust
impl PrismSController {
    fn shutdown(&mut self, idle_color: (u8, u8, u8)) -> Result<(), HidError> {
        // Send one final frame with the idle color
        let atx_colors = vec![idle_color; 120];
        let gpu_count = self.gpu_cable.led_count() as usize;
        let gpu_colors = vec![idle_color; gpu_count];
        self.render_frame(Some(&atx_colors), Some(&gpu_colors))?;

        // Save settings with idle color
        self.save_settings(&PrismSConfig {
            idle_color,
            gpu_cable: self.gpu_cable,
        })?;

        Ok(())
    }
}
```

---

## 7. Prism Mini Protocol

**Device:** PrismRGB Prism Mini Controller
**VID/PID:** `0x16D0` / `0x1407` | **Interface:** 2
**Color format:** RGB | **Brightness scale:** 1.00
**Channels:** 1, up to 128 LEDs

### 7.1 Initialization — Firmware Version Query

The Prism Mini uses a different command byte (`0xCC`) than the Prism 8 family (`0xFC 0x01`).

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0x00 │ Reserved                         │
│ 2      │ 1    │ 0x00 │ Reserved                         │
│ 3      │ 1    │ 0x00 │ Reserved                         │
│ 4      │ 1    │ 0xCC │ Firmware version query marker     │
│ 5-64   │ 60   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

READ ← 65 bytes
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0      │ 1    │ Reserved                                │
│ 1      │ 1    │ Major version                           │
│ 2      │ 1    │ Minor version                           │
│ 3      │ 1    │ Patch version                           │
│ 4-64   │ 61   │ Reserved                                │
└────────┴──────┴────────────────────────────────────────┘

Version string: "{major}.{minor}.{patch}"  (expected: "1.0.0")
```

```rust
/// Firmware version for Prism Mini
#[derive(Debug, Clone)]
pub struct MiniVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u8,
}

impl std::fmt::Display for MiniVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl PrismMiniController {
    fn query_firmware_version(&mut self) -> Result<MiniVersion, HidError> {
        let mut pkt = PrismPacket::new();
        pkt.set(4, 0xCC); // firmware version marker at offset 4
        self.device.write(pkt.as_bytes())?;

        let mut response = [0u8; HID_REPORT_SIZE];
        self.device.read_timeout(&mut response, 1000)?;

        Ok(MiniVersion {
            major: response[1],
            minor: response[2],
            patch: response[3],
        })
    }
}
```

### 7.2 Render Loop — Numbered Packets with Data Marker

The Prism Mini uses a header with explicit packet numbering and a `0xAA` data marker byte. Each packet carries up to 20 LEDs (60 bytes of RGB data).

```
WRITE → 65 bytes
┌────────┬──────┬────────────────────────────────────────┐
│ Offset │ Size │ Description                             │
├────────┼──────┼────────────────────────────────────────┤
│ 0      │ 1    │ Report ID: 0x00                        │
│ 1      │ 1    │ Packet number (1-indexed)              │
│ 2      │ 1    │ Total packet count                     │
│ 3      │ 1    │ Reserved: 0x00                         │
│ 4      │ 1    │ Data marker: 0xAA                      │
│ 5-7    │ 3    │ LED 0: R, G, B                         │
│ 8-10   │ 3    │ LED 1: R, G, B                         │
│ 11-13  │ 3    │ LED 2: R, G, B                         │
│ ...    │ ...  │ ...                                     │
│ 62-64  │ 3    │ LED 19: R, G, B                        │
└────────┴──────┴────────────────────────────────────────┘

Color order: RGB (standard)
Max LEDs per packet: 20 (60 bytes / 3 bytes per LED)
Max total LEDs: 128 → ceil(128/20) = 7 packets
Packet numbering: 1-indexed (packet 1, 2, 3, ...)
No explicit frame commit — data latches after final packet
```

```rust
/// Maximum LEDs per Prism Mini data packet
const MINI_LEDS_PER_PACKET: usize = 20;
/// Data marker byte
const MINI_DATA_MARKER: u8 = 0xAA;

impl PrismMiniController {
    fn render_frame(&mut self, colors: &[(u8, u8, u8)]) -> Result<(), HidError> {
        let total_packets = colors.len().div_ceil(MINI_LEDS_PER_PACKET) as u8;

        for (pkt_idx, chunk) in colors.chunks(MINI_LEDS_PER_PACKET).enumerate() {
            let packet_num = (pkt_idx + 1) as u8; // 1-indexed

            let mut pkt = PrismPacket::new();
            pkt.set(1, packet_num);
            pkt.set(2, total_packets);
            pkt.set(3, 0x00);              // reserved
            pkt.set(4, MINI_DATA_MARKER);  // 0xAA

            // Write LED RGB data starting at offset 5
            let mut cursor = 5;
            for &(r, g, b) in chunk {
                pkt.set(cursor, r);
                pkt.set(cursor + 1, g);
                pkt.set(cursor + 2, b);
                cursor += 3;
            }

            self.device.write(pkt.as_bytes())?;
        }

        Ok(())
    }
}
```

### 7.3 Low Power Saver Mode

Per-LED brightness limiting that caps the total RGB sum to prevent overcurrent on WS2812 strips powered through USB. This is a host-side computation applied before encoding.

```
Algorithm:
  For each LED (R, G, B):
    total = R + G + B
    if total > 175:
      scale = 175.0 / total as f32
      R = (R as f32 * scale) as u8
      G = (G as f32 * scale) as u8
      B = (B as f32 * scale) as u8

Threshold: 175 (sum of all three color components)
Effect: Preserves hue and saturation, reduces brightness proportionally
```

```rust
/// Maximum sum of R+G+B components per LED in low-power mode
const LOW_POWER_THRESHOLD: u16 = 175;

/// Apply low-power brightness limiting to a color triple.
/// Preserves hue and saturation, scales brightness so R+G+B <= 175.
pub fn apply_low_power_saver(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let total = r as u16 + g as u16 + b as u16;
    if total > LOW_POWER_THRESHOLD {
        let scale = LOW_POWER_THRESHOLD as f32 / total as f32;
        (
            (r as f32 * scale) as u8,
            (g as f32 * scale) as u8,
            (b as f32 * scale) as u8,
        )
    } else {
        (r, g, b)
    }
}
```

### 7.4 Color Compression Mode

Optional 4-bit color packing that doubles the LED density per packet. Two LEDs are compressed into 3 bytes by reducing each channel from 8-bit to 4-bit precision.

```
Compression algorithm (2 LEDs → 3 bytes):

  Input:  LED1 = (R1, G1, B1), LED2 = (R2, G2, B2)  [8-bit each]
  Output: 3 compressed bytes

  compressed[0] = (R1 >> 4) | ((G1 >> 4) << 4)    // R1 low nibble, G1 high nibble
  compressed[1] = (B1 >> 4) | ((R2 >> 4) << 4)    // B1 low nibble, R2 high nibble
  compressed[2] = (G2 >> 4) | ((B2 >> 4) << 4)    // G2 low nibble, B2 high nibble

Bit-level layout:
  Byte 0: [G1₃ G1₂ G1₁ G1₀ R1₃ R1₂ R1₁ R1₀]
  Byte 1: [R2₃ R2₂ R2₁ R2₀ B1₃ B1₂ B1₁ B1₀]
  Byte 2: [B2₃ B2₂ B2₁ B2₀ G2₃ G2₂ G2₁ G2₀]
```

```rust
/// Compress two RGB LEDs into 3 bytes using 4-bit color depth.
/// Halves the bandwidth at the cost of color precision (16 levels per channel).
pub fn compress_color_pair(
    led1: (u8, u8, u8),
    led2: (u8, u8, u8),
) -> [u8; 3] {
    let (r1, g1, b1) = led1;
    let (r2, g2, b2) = led2;
    [
        (r1 >> 4) | ((g1 >> 4) << 4),
        (b1 >> 4) | ((r2 >> 4) << 4),
        (g2 >> 4) | ((b2 >> 4) << 4),
    ]
}

/// Compress a full LED color array into 4-bit packed format.
/// Input must have an even number of LEDs (pad with black if needed).
pub fn compress_colors(colors: &[(u8, u8, u8)]) -> Vec<u8> {
    let mut compressed = Vec::with_capacity((colors.len() / 2) * 3);
    for pair in colors.chunks(2) {
        let led1 = pair[0];
        let led2 = if pair.len() > 1 { pair[1] } else { (0, 0, 0) };
        compressed.extend_from_slice(&compress_color_pair(led1, led2));
    }
    compressed
}
```

### 7.5 Hardware Lighting Configuration

Configures the Prism Mini's onboard effect engine. Used for setting a hardware fallback effect and enabling/disabling features like color compression and the onboard status LED.

```
WRITE → 65 bytes
┌────────┬──────┬──────┬──────────────────────────────────┐
│ Offset │ Size │ Value│ Description                       │
├────────┼──────┼──────┼──────────────────────────────────┤
│ 0      │ 1    │ 0x00 │ Report ID                        │
│ 1      │ 1    │ 0x00 │ Reserved                         │
│ 2      │ 1    │ 0x00 │ Reserved                         │
│ 3      │ 1    │ 0x00 │ Reserved                         │
│ 4      │ 1    │ 0xBB │ Hardware lighting config marker   │
│ 5      │ 1    │      │ HW lighting enable (0x00/0x01)   │
│ 6      │ 1    │      │ Return to HW on disconnect (0/1) │
│ 7      │ 1    │      │ Return timeout (1-60 seconds)    │
│ 8      │ 1    │      │ Effect mode (see table below)    │
│ 9      │ 1    │      │ Effect speed (1-20)              │
│ 10     │ 1    │      │ Brightness (10-255)              │
│ 11     │ 1    │      │ Solid/breathing color: Red       │
│ 12     │ 1    │      │ Solid/breathing color: Green     │
│ 13     │ 1    │      │ Solid/breathing color: Blue      │
│ 14     │ 1    │      │ Status LED enable (0x00/0x01)    │
│ 15     │ 1    │      │ Compression enable (0x00/0x01)   │
│ 16-64  │ 49   │ 0x00 │ Zero padding                     │
└────────┴──────┴──────┴──────────────────────────────────┘

Effect modes:
┌──────┬───────────────────┐
│ Code │ Effect             │
├──────┼───────────────────┤
│ 0x01 │ Rainbow Wave       │
│ 0x02 │ Rainbow Cycle      │
│ 0x03 │ Solid Color        │
│ 0x04 │ Breathing          │
└──────┴───────────────────┘
```

```rust
/// Hardware lighting effect modes for Prism Mini
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MiniEffectMode {
    RainbowWave  = 0x01,
    RainbowCycle = 0x02,
    Solid        = 0x03,
    Breathing    = 0x04,
}

/// Full hardware lighting configuration for the Prism Mini controller
#[derive(Debug, Clone)]
pub struct MiniHardwareLighting {
    /// Enable onboard hardware lighting engine
    pub enabled: bool,
    /// Return to hardware lighting when host software disconnects
    pub return_on_disconnect: bool,
    /// Seconds to wait before returning to hardware mode (1-60)
    pub return_timeout_sec: u8,
    /// Hardware effect to display
    pub effect_mode: MiniEffectMode,
    /// Effect speed (1-20)
    pub speed: u8,
    /// Brightness level (10-255)
    pub brightness: u8,
    /// Color for Solid and Breathing effects
    pub color: (u8, u8, u8),
    /// Enable the onboard status LED indicator
    pub status_led: bool,
    /// Enable 4-bit color compression mode
    pub compression: bool,
}

impl Default for MiniHardwareLighting {
    fn default() -> Self {
        Self {
            enabled: true,
            return_on_disconnect: true,
            return_timeout_sec: 5,
            effect_mode: MiniEffectMode::RainbowCycle,
            speed: 10,
            brightness: 128,
            color: (255, 0, 128),
            status_led: true,
            compression: false,
        }
    }
}

/// Hardware lighting config marker byte
const MINI_HW_MARKER: u8 = 0xBB;

impl PrismMiniController {
    fn set_hardware_lighting(
        &mut self,
        config: &MiniHardwareLighting,
    ) -> Result<(), HidError> {
        let mut pkt = PrismPacket::new();
        pkt.set(4, MINI_HW_MARKER);
        pkt.set(5, config.enabled as u8);
        pkt.set(6, config.return_on_disconnect as u8);
        pkt.set(7, config.return_timeout_sec.clamp(1, 60));
        pkt.set(8, config.effect_mode as u8);
        pkt.set(9, config.speed.clamp(1, 20));
        pkt.set(10, config.brightness.clamp(10, 255));
        pkt.set(11, config.color.0);  // R
        pkt.set(12, config.color.1);  // G
        pkt.set(13, config.color.2);  // B
        pkt.set(14, config.status_led as u8);
        pkt.set(15, config.compression as u8);

        self.device.write(pkt.as_bytes())?;
        Ok(())
    }
}
```

### 7.6 Shutdown

```rust
impl PrismMiniController {
    fn shutdown(
        &mut self,
        idle_color: (u8, u8, u8),
        hw_config: &MiniHardwareLighting,
    ) -> Result<(), HidError> {
        // Send final frame with idle color
        let colors = vec![idle_color; self.led_count as usize];
        self.render_frame(&colors)?;

        // Configure hardware lighting for when host disconnects
        self.set_hardware_lighting(hw_config)?;

        Ok(())
    }
}
```

---

## 8. HidController Trait

The unified trait that all PrismRGB/Nollie controllers implement. This plugs into the Hypercolor `DeviceBackend` abstraction defined in `crates/hypercolor-core/src/device/traits.rs`.

```rust
use std::time::Duration;

/// Unified controller trait for all PrismRGB/Nollie USB HID devices.
/// Each implementation handles protocol-specific initialization, packetization,
/// and shutdown while exposing a uniform interface to the render loop.
pub trait HidController: Send {
    /// Human-readable device name (e.g., "PrismRGB Prism 8")
    fn name(&self) -> &str;

    /// Device type identifier
    fn device_type(&self) -> HidDeviceType;

    /// Initialize the device: query firmware, configure channels, set idle effect.
    /// Called once after USB connection is established.
    fn init(&mut self) -> Result<(), HidError>;

    /// Push a single frame of LED colors to the device.
    /// The outer slice is indexed by zone/channel. Each inner slice contains
    /// (R, G, B) tuples in standard RGB order — the implementation handles
    /// format conversion (GRB encoding, brightness scaling, etc.).
    fn render(&mut self, zones: &[&[(u8, u8, u8)]]) -> Result<(), HidError>;

    /// Gracefully shut down the device, restoring hardware fallback lighting.
    fn shutdown(&mut self, idle_color: (u8, u8, u8)) -> Result<(), HidError>;

    /// Total number of LEDs across all channels/zones
    fn total_leds(&self) -> u16;

    /// Number of channels/zones
    fn zone_count(&self) -> u8;

    /// LED count per zone
    fn zone_led_counts(&self) -> Vec<u16>;

    /// Native color format of the device
    fn color_format(&self) -> ColorFormat;

    /// Recommended frame interval for this device
    fn frame_interval(&self) -> Duration {
        Duration::from_millis(16) // ~60fps default
    }
}
```

### Implementation Summary

```rust
/// Prism 8 / Nollie 8 v2 — shared implementation, parameterized by device type
pub struct EightChannelController {
    device: HidDevice,
    device_type: HidDeviceType,
    firmware_version: u8,
    channel_config: ChannelConfig,
    frame_counter: u32,
}

impl HidController for EightChannelController { /* ... */ }

/// Prism S — Strimer cable controller
pub struct PrismSController {
    device: HidDevice,
    atx_connected: bool,
    gpu_cable: StrimerCable,
}

impl HidController for PrismSController { /* ... */ }

/// Prism Mini — single-channel addressable LED controller
pub struct PrismMiniController {
    device: HidDevice,
    firmware_version: MiniVersion,
    led_count: u16,
    low_power_mode: bool,
    compression_enabled: bool,
    hw_lighting: MiniHardwareLighting,
}

impl HidController for PrismMiniController { /* ... */ }
```

### Device Discovery and Construction

```rust
use hidapi::HidApi;

/// Known device identifiers for detection
const KNOWN_DEVICES: &[(u16, u16, i32, HidDeviceType)] = &[
    (PRISM_8_VID,    PRISM_8_PID,    PRISM_8_INTERFACE,    HidDeviceType::Prism8),
    (NOLLIE_8_VID,   NOLLIE_8_PID,   NOLLIE_8_INTERFACE,   HidDeviceType::Nollie8),
    (PRISM_S_VID,    PRISM_S_PID,    PRISM_S_INTERFACE,    HidDeviceType::PrismS),
    (PRISM_MINI_VID, PRISM_MINI_PID, PRISM_MINI_INTERFACE, HidDeviceType::PrismMini),
];

/// Scan USB bus for all connected PrismRGB/Nollie devices
pub fn discover_devices(api: &HidApi) -> Vec<HidDeviceInfo> {
    let mut found = Vec::new();

    for device_info in api.device_list() {
        for &(vid, pid, iface, dtype) in KNOWN_DEVICES {
            if device_info.vendor_id() == vid
                && device_info.product_id() == pid
                && device_info.interface_number() == iface
            {
                found.push(HidDeviceInfo {
                    vid,
                    pid,
                    interface: iface,
                    serial: device_info.serial_number().map(|s| s.to_string()),
                    product: device_info
                        .product_string()
                        .unwrap_or("Unknown")
                        .to_string(),
                    firmware_version: None, // populated during init
                    device_type: dtype,
                });
            }
        }
    }

    found
}

/// Open a discovered device and return a boxed HidController
pub fn open_device(
    api: &HidApi,
    info: &HidDeviceInfo,
) -> Result<Box<dyn HidController>, HidError> {
    let device = api.open(info.vid, info.pid)?;

    let controller: Box<dyn HidController> = match info.device_type {
        HidDeviceType::Prism8 | HidDeviceType::Nollie8 => {
            Box::new(EightChannelController::new(device, info.device_type)?)
        }
        HidDeviceType::PrismS => {
            Box::new(PrismSController::new(device)?)
        }
        HidDeviceType::PrismMini => {
            Box::new(PrismMiniController::new(device)?)
        }
    };

    Ok(controller)
}
```

---

## 9. Error Handling

### Error Types

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HidError {
    /// USB HID communication failure
    #[error("HID I/O error: {0}")]
    Io(#[from] hidapi::HidError),

    /// Device did not respond within the expected timeout
    #[error("device read timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    /// Device returned an unexpected or malformed response
    #[error("unexpected response: expected {expected}, got {actual}")]
    UnexpectedResponse {
        expected: String,
        actual: String,
    },

    /// Attempted to write more LEDs than the device/channel supports
    #[error("LED count overflow: {requested} requested, {max} maximum")]
    LedCountOverflow { requested: u16, max: u16 },

    /// Packet buffer overflow during construction
    #[error("packet overflow: {written} bytes written, {capacity} capacity")]
    PacketOverflow { written: usize, capacity: usize },

    /// Device disconnected during operation
    #[error("device disconnected: {product} (VID={vid:#06x}, PID={pid:#06x})")]
    Disconnected {
        product: String,
        vid: u16,
        pid: u16,
    },

    /// Voltage reading outside safe operating range
    #[error("voltage out of range on {rail}: {volts:.2}V (expected {expected_min:.1}V-{expected_max:.1}V)")]
    VoltageWarning {
        rail: String,
        volts: f32,
        expected_min: f32,
        expected_max: f32,
    },
}
```

### Retry and Recovery Strategy

```rust
/// Configuration for USB communication retry behavior
pub struct HidRetryPolicy {
    /// Maximum write retries before declaring device lost
    pub max_write_retries: u32,
    /// Delay between retry attempts
    pub retry_delay: Duration,
    /// Read timeout for query responses
    pub read_timeout: Duration,
}

impl Default for HidRetryPolicy {
    fn default() -> Self {
        Self {
            max_write_retries: 3,
            retry_delay: Duration::from_millis(10),
            read_timeout: Duration::from_millis(1000),
        }
    }
}
```

Retry logic should be applied at the `HidController` level, not within individual packet sends. If a write fails:

1. Retry up to `max_write_retries` times with `retry_delay` between attempts.
2. If all retries fail, emit `HidError::Disconnected` and signal the event bus (`HypercolorEvent::DeviceDisconnected`).
3. The render loop skips disconnected devices and periodically attempts reconnection via `discover_devices()`.

---

## 10. Platform Setup

### Linux: udev Rules

Without udev rules, USB HID devices require root access. Install these rules to grant user-space access to all PrismRGB/Nollie controllers.

**File:** `/etc/udev/rules.d/60-hypercolor-hid.rules`

```udev
# PrismRGB Prism 8
SUBSYSTEM=="usb", ATTR{idVendor}=="16d5", ATTR{idProduct}=="1f01", MODE="0666"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d5", ATTRS{idProduct}=="1f01", MODE="0666"

# Nollie 8 v2
SUBSYSTEM=="usb", ATTR{idVendor}=="16d2", ATTR{idProduct}=="1f01", MODE="0666"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d2", ATTRS{idProduct}=="1f01", MODE="0666"

# PrismRGB Prism S
SUBSYSTEM=="usb", ATTR{idVendor}=="16d0", ATTR{idProduct}=="1294", MODE="0666"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1294", MODE="0666"

# PrismRGB Prism Mini
SUBSYSTEM=="usb", ATTR{idVendor}=="16d0", ATTR{idProduct}=="1407", MODE="0666"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="16d0", ATTRS{idProduct}=="1407", MODE="0666"
```

Reload after installation:

```bash
sudo udevadm control --reload-rules
sudo udevadm trigger
```

### Windows: hidapi Setup

On Windows, `hidapi` uses the WinAPI HID driver. No special installation is required — standard HID class devices are accessible without admin privileges once the correct interface number is selected.

**Cargo dependency:**

```toml
[target.'cfg(windows)'.dependencies]
hidapi = { version = "2", features = ["windows-native"] }

[target.'cfg(unix)'.dependencies]
hidapi = { version = "2", features = ["linux-static-hidraw"] }
```

**Important Windows considerations:**
- `hidapi` on Windows requires matching the correct HID interface number. The `interface_number()` filter in `discover_devices()` is critical.
- Some devices expose multiple HID interfaces (keyboard, consumer control, vendor-specific). Only the vendor-specific interface at the documented interface number (0 or 2) should be opened.
- Windows Defender SmartScreen may flag unsigned executables that access USB devices. Signing the binary resolves this.

### macOS: hidapi Setup

On macOS, `hidapi` uses IOKit for HID communication. No kernel extensions or special permissions are required for vendor-specific HID devices.

```toml
[target.'cfg(target_os = "macos")'.dependencies]
hidapi = { version = "2", features = ["macos-native"] }
```

---

## 11. Implementation Notes

### Byte Order Summary

| Protocol | Query Response | Config Write |
|----------|---------------|-------------|
| Prism 8 channel counts query (0xFC 0x03) | **Big-endian** | N/A |
| Prism 8 channel counts update (0xFE 0x03) | N/A | **Little-endian** |
| Prism 8 voltage (0xFC 0x1A) | **Little-endian** | N/A |

This endianness mismatch is intentional behavior observed in the original firmware. The implementation must handle both.

### Timing Constraints

| Operation | Minimum Delay | Notes |
|-----------|--------------|-------|
| Prism S settings save | 50ms | Required pause after `0xFE 0x01` write |
| Prism 8 voltage poll | Every 150 frames | ~2.5s at 60fps, ~4.5s at 33fps |
| Inter-packet gap | None required | Controllers handle back-to-back writes |
| Frame commit to next frame | One frame interval | Don't send color data until next tick |

### Bandwidth Analysis

| Device | Packets/Frame | Bytes/Frame | At 60fps |
|--------|--------------|-------------|----------|
| Prism 8 (all 8ch, 126 LEDs each) | 48 data + 1 commit = 49 | 49 x 65 = 3,185 | 191 KB/s |
| Nollie 8 (all 8ch, 126 LEDs each) | 49 | 3,185 | 191 KB/s |
| Prism S (ATX + Triple GPU) | ceil(846/64) = 14 | 14 x 65 = 910 | 55 KB/s |
| Prism Mini (128 LEDs) | 7 | 7 x 65 = 455 | 27 KB/s |

All devices operate well within USB 1.1 Full Speed bandwidth (1.5 MB/s for interrupt transfers).

### Thread Safety

The `hidapi::HidDevice` handle is `Send` but not `Sync`. Each controller instance owns its handle and must be driven from a single thread. The recommended architecture is a dedicated HID I/O thread per device, communicating with the render loop via a `tokio::sync::mpsc` channel:

```rust
/// Message sent from the render loop to the HID I/O thread
pub enum HidCommand {
    /// Push a frame of LED colors
    Frame(Vec<Vec<(u8, u8, u8)>>),
    /// Gracefully shut down and close the device
    Shutdown { idle_color: (u8, u8, u8) },
}
```

### Testing Strategy

Hardware-independent testing is achieved by abstracting the HID I/O behind a trait:

```rust
/// Abstraction over HID read/write for testability
pub trait HidTransport: Send {
    fn write(&self, data: &[u8]) -> Result<usize, HidError>;
    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, HidError>;
}

/// Real hidapi implementation
impl HidTransport for hidapi::HidDevice {
    fn write(&self, data: &[u8]) -> Result<usize, HidError> {
        Ok(self.write(data)?)
    }

    fn read_timeout(&self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, HidError> {
        Ok(self.read_timeout(buf, timeout_ms)?)
    }
}

/// Mock transport for unit tests — records all writes and replays canned reads
#[cfg(test)]
pub struct MockHidTransport {
    pub writes: std::sync::Mutex<Vec<Vec<u8>>>,
    pub read_responses: std::sync::Mutex<Vec<Vec<u8>>>,
}
```

---

## Appendix A: Quick Reference — All Packet Formats

| Device | Packet | Bytes [0..] | Purpose |
|--------|--------|-------------|---------|
| **Prism 8 / Nollie 8** | `[0x00, 0xFC, 0x01, 0x00...]` | 65 | Query firmware version |
| | `[0x00, 0xFC, 0x03, 0x00...]` | 65 | Query channel LED counts |
| | `[0x00, 0xFC, 0x1A, 0x00...]` | 65 | Read voltage rails |
| | `[0x00, 0xFE, 0x01, 0x00, ...]` | 65 | Activate hardware mode |
| | `[0x00, 0xFE, 0x02, 0x00, R, G, B, 0x64, 0x0A, 0x00, 0x01]` | 65 | Set hardware effect |
| | `[0x00, 0xFE, 0x03, ch0_lo, ch0_hi, ...]` | 65 | Update channel counts |
| | `[0x00, packet_id, GRB...]` | 65 | Color data (21 LEDs) |
| | `[0x00, 0xFF]` | 65 | Frame commit / latch |
| **Prism S** | `[0x00, 0xFE, 0x01, R, G, B, mode]` | 65 | Save settings |
| | `[0x00, <64 bytes RGB data>]` | 65 | Buffer chunk (sequential) |
| **Prism Mini** | `[0x00, 0x00, 0x00, 0x00, 0xCC]` | 65 | Query firmware version |
| | `[0x00, pkt#, total, 0x00, 0xAA, RGB...]` | 65 | Color data (20 LEDs) |
| | `[0x00, 0x00, 0x00, 0x00, 0xBB, ...]` | 65 | Hardware lighting config |

## Appendix B: Command Byte Map

```
0x00-0xFB  — Packet IDs / data payload (device-specific)
0xFC       — Query prefix (Prism 8 / Nollie 8)
  0xFC 0x01  Firmware version
  0xFC 0x03  Channel LED counts
  0xFC 0x1A  Voltage monitoring
0xFE       — Config write prefix (all devices)
  0xFE 0x01  Activate hardware mode / save settings
  0xFE 0x02  Set hardware effect
  0xFE 0x03  Update channel counts
0xFF       — Frame commit (Prism 8 / Nollie 8 only)
0xAA       — Data marker (Prism Mini, at offset 4)
0xBB       — Hardware lighting config marker (Prism Mini, at offset 4)
0xCC       — Firmware version query (Prism Mini, at offset 4)
```
