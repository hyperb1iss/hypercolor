# 28 -- Ableton Push 2 Protocol Driver

> USB MIDI + bulk display driver for the Ableton Push 2. Palette-indexed RGB pads, animated buttons, a 960×160 pixel display, and the first MIDI transport in the HAL.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::push2`
**Author:** Nova
**Date:** 2026-03-09

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Transport: USB MIDI](#3-transport-usb-midi)
4. [LED Color System](#4-led-color-system)
5. [LED Addressing](#5-led-addressing)
6. [LED Animations](#6-led-animations)
7. [Touch Strip LEDs](#7-touch-strip-leds)
8. [Display Interface](#8-display-interface)
9. [SysEx Command Reference](#9-sysex-command-reference)
10. [Initialization & Mode Switching](#10-initialization--mode-switching)
11. [HAL Integration](#11-hal-integration)
12. [Render Pipeline](#12-render-pipeline)
13. [Wire-Format Structs](#13-wire-format-structs)
14. [Testing Strategy](#14-testing-strategy)

---

## 1. Overview

Native USB driver for the Ableton Push 2 controller via the `hypercolor-hal` abstraction layer. Push 2 is unique in the Hypercolor device ecosystem: it communicates over **USB MIDI** (not HID) for LED control, and uses a **USB bulk endpoint** for its 960×160 pixel display. This makes it the first device requiring a MIDI transport layer.

Push 2 has **four distinct LED subsystems** orchestrated through a single palette-indexed color model:

| Subsystem | Count | Type | Addressing |
|-----------|-------|------|-----------|
| Pads | 64 | RGB | MIDI Note On (notes 36–99) |
| RGB Buttons | ~26 | RGB | MIDI CC |
| White Buttons | ~12 | White-only | MIDI CC |
| Touch Strip | 31 | White (8-level) | SysEx command |

Plus a **960×160 RGB565 display** over USB bulk — a secondary "zone" rendered through the existing `write_display_frame` path (accepting JPEG input, decoded to RGB565 inside the protocol).

### Prior Art

- [Ableton Push 2 MIDI and Display Interface Manual](https://github.com/Ableton/push-interface) — official open-source protocol spec (Rev 1.1, Jan 2017)
- OpenRGB does not support Push 2
- Various Max/MSP and Python community implementations exist

### Relationship to Other Specs

- **Spec 16 (HAL):** Defines the `Protocol` and `Transport` traits this driver implements
- **Spec 01 (Core Engine):** Canvas/sampler integration for the 8×8 pad grid
- **Spec 06 (Spatial Engine):** Matrix topology mapping for the pad grid
- **Spec 14 (Screen Capture):** Display rendering pipeline (JPEG path)

### What Makes Push 2 Different

1. **MIDI transport** — all other HAL drivers use HID, bulk, serial, or SMBus. Push 2 needs a new `UsbMidi` transport variant
2. **Palette-indexed colors** — no direct RGB. The 128-entry palette must be reprogrammed per-frame to map Hypercolor's RGB output to the device
3. **Multi-protocol** — MIDI on one USB interface, bulk transfers on another, running simultaneously
4. **Display** — 960×160 RGB565 with XOR signal shaping, rendered via bulk endpoint

---

## 2. Device Registry

### 2.1 USB Identifiers

| Field | Value |
|-------|-------|
| Vendor ID | `0x2982` (Ableton AG) |
| Product ID | `0x1967` |
| USB Class | Composite (MIDI + Vendor-specific) |
| Power | 500mA max, external PSU optional |

### 2.2 USB Interfaces

| Interface | Class | Purpose | Transport |
|-----------|-------|---------|-----------|
| 0 | Vendor-specific | Display (bulk) | libusb bulk OUT, endpoint `0x01` |
| 1 | USB Audio / MIDI Streaming | MIDI port 1 (Live) | ALSA sequencer / CoreMIDI |
| 2 | USB Audio / MIDI Streaming | MIDI port 2 (User) | ALSA sequencer / CoreMIDI |

### 2.3 MIDI Port Names by OS

| OS | Live Port (Port 1) | User Port (Port 2) |
|----|--------------------|--------------------|
| Linux | `Ableton Push 2 nn:0` | `Ableton Push 2 nn:1` |
| macOS | `Ableton Push 2 Live Port` | `Ableton Push 2 User Port` |

### 2.4 Protocol Database Registration

```rust
pub const PUSH2: DeviceDescriptor = DeviceDescriptor {
    vendor_id: 0x2982,
    product_id: 0x1967,
    name: "Ableton Push 2",
    family: DeviceFamily::Custom("Ableton".to_owned()),
    transport: TransportType::UsbMidi {
        midi_interface: 2,      // User port
        display_interface: 0,   // Bulk display
        display_endpoint: 0x01,
    },
    // NOTE: DeviceFamily has no "Controller" variant. Using Custom("Ableton").
    // Consider adding DeviceFamily::Ableton if more Ableton devices are added.
    protocol: ProtocolBinding {
        id: "push2",
        build: || Box::new(Push2Protocol::new()),
    },
    firmware_predicate: None,
};
```

> **New transport variant required.** `TransportType::UsbMidi` is a new variant — see [§3](#3-transport-usb-midi).
>
> **Files requiring changes:**
> - `crates/hypercolor-hal/src/registry.rs` — add `UsbMidi` variant to `TransportType` enum
> - `crates/hypercolor-core/src/device/usb_backend.rs` — add `UsbMidi` match arm in transport construction
> - `crates/hypercolor-hal/src/transport/` — add `midi.rs` transport implementation
> - `udev/99-hypercolor.rules` — add Ableton VID `0x2982` permission rules

---

## 3. Transport: USB MIDI

### 3.1 New Transport Variant

Push 2 requires a new `TransportType` variant and corresponding `Transport` implementation:

```rust
/// New variant in TransportType enum
UsbMidi {
    midi_interface: u8,     // MIDI interface for LED control (User port)
    display_interface: u8,  // Bulk interface for display frames
    display_endpoint: u8,   // Bulk OUT endpoint address
}
```

### 3.2 MIDI Transport Implementation

The MIDI transport wraps platform MIDI APIs:

| Platform | MIDI Backend | Display Backend |
|----------|-------------|----------------|
| Linux | ALSA sequencer (`alsa-rawmidi` or `midir`) | `nusb` bulk transfer |
| macOS | CoreMIDI (`midir`) | `nusb` bulk transfer |

The transport must open **two independent I/O paths** simultaneously:

1. **MIDI path** — for Note On, CC, and SysEx messages (LED control)
2. **Bulk path** — for display frame data (claimed via `nusb`, same as Corsair LCD)

```rust
pub struct Push2Transport {
    /// midir's connected output — call .send(&[u8]) to write MIDI.
    midi_out: Mutex<MidiOutputConnection>,
    /// SysEx reply channel — midir input is callback-based, so we bridge
    /// to a bounded tokio::mpsc queue for async receive with timeout.
    sysex_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
    /// nusb handle for display bulk transfers (interface 0).
    bulk_handle: DeviceHandle,
}

impl Push2Transport {
    pub fn new(
        midi_out: MidiOutputConnection,
        midi_in_port: MidiInputPort,
        bulk_handle: DeviceHandle,
    ) -> Result<Self, TransportError> {
        let (tx, rx) = mpsc::channel(16);

        // midir input is callback-based: connect with a closure that
        // forwards SysEx replies into the mpsc channel.
        let _midi_in_conn = MidiInput::new("hypercolor-push2")?
            .connect(&midi_in_port, "push2-sysex", move |_ts, msg, _| {
                // Only forward SysEx messages (F0..F7)
                if msg.first() == Some(&0xF0) {
                    let _ = tx.blocking_send(msg.to_vec());
                }
            }, ())?;

        Ok(Self {
            midi_out: Mutex::new(midi_out),
            sysex_rx: Mutex::new(rx),
            bulk_handle,
        })
    }
}

#[async_trait]
impl Transport for Push2Transport {
    async fn send(&self, data: &[u8]) -> Result<(), TransportError> {
        self.send_with_type(data, TransferType::Primary).await
    }

    async fn send_with_type(
        &self, data: &[u8], transfer_type: TransferType,
    ) -> Result<(), TransportError> {
        match transfer_type {
            TransferType::Primary => {
                self.midi_out.lock().await.send(data)
                    .map_err(|e| TransportError::SendFailed(e.to_string()))
            }
            TransferType::Bulk => {
                self.bulk_handle.write_bulk(0x01, data).await
                    .map_err(|e| TransportError::SendFailed(e.to_string()))
            }
            _ => Err(TransportError::UnsupportedTransfer),
        }
    }

    async fn receive(&self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        // Receive SysEx replies via the mpsc bridge with timeout
        let mut rx = self.sysex_rx.lock().await;
        tokio::time::timeout(timeout, rx.recv())
            .await
            .map_err(|_| TransportError::Timeout)?
            .ok_or(TransportError::Disconnected)
    }
}
```

**Key design note:** `midir::MidiInput::connect()` is callback-based — it spawns an OS thread
that invokes a closure on every incoming MIDI message. There is no blocking `receive()` API.
The transport bridges this to async Hypercolor by forwarding SysEx replies through a bounded
`tokio::mpsc` channel, which `receive()` reads with a timeout. The `MidiInputConnection` must
be kept alive (stored in the transport or an outer struct) for the callback to remain active.

### 3.3 MIDI Library: `midir`

**Decision:** Use `midir` — the standard cross-platform Rust MIDI I/O crate.

- Linux: ALSA sequencer backend
- macOS: CoreMIDI backend
- Windows: WinMM backend (not a priority, but free)
- Handles SysEx send/receive natively
- Well-maintained, minimal dependencies
- One crate covers both target platforms (Linux + macOS) without conditional compilation

### 3.4 Device Matching

The MIDI transport must match the MIDI port to the USB device. Strategy:

1. Enumerate USB devices via `nusb` — find `0x2982:0x1967`
2. Enumerate MIDI ports via `midir` — find port names containing "Ableton Push 2"
3. On Linux, correlate via sysfs USB path (both `nusb` and ALSA expose the bus/device path)
4. Open the **User port** (port 2) for LED control
5. Claim interface 0 via `nusb` for display bulk transfers

---

## 4. LED Color System

### 4.1 Palette Architecture

Push 2 does **not** support direct RGB addressing. All LED colors are set through a **128-entry color palette**:

```
LED ← velocity/value (0–127) → Palette[index] → {R, G, B, W}
```

Each palette entry has four channels:
- **R, G, B** — used by RGB LEDs (pads, colored buttons)
- **W** — used by white-only LEDs

### 4.2 Palette Reprogramming Strategy

To display arbitrary Hypercolor RGB colors on Push 2, the driver must **reprogram palette entries every frame**:

1. Receive `colors: &[[u8; 3]]` from the engine (up to 90 RGB values for pads + buttons)
2. Deduplicate colors — the 90 LEDs likely use fewer than 128 unique colors
3. Write each unique color as a palette entry via SysEx command `0x03`
4. Send Note On / CC messages with the corresponding palette index as velocity
5. Send SysEx `0x05` (Reapply Color Palette) to flush changes

**Palette slot allocation:**

| Slots | Purpose |
|-------|---------|
| 0 | Reserved: OFF (black) |
| 1–90 | Dynamic: mapped per-frame to current Hypercolor colors |
| 91–127 | Available for animation endpoints or future use |

### 4.3 Set LED Color Palette Entry (SysEx 0x03)

```
F0 00 21 1D 01 01 03 <index> <r_LSB> <r_MSB> <g_LSB> <g_MSB> <b_LSB> <b_MSB> <w_LSB> <w_MSB> F7
```

8-bit values are encoded as two 7-bit SysEx bytes:
- `LSB` = bits [6:0]
- `MSB` = bit [7]
- Reconstruction: `value = (MSB << 7) | LSB`

```rust
/// Encode an 8-bit value into SysEx 7-bit pair.
fn encode_sysex_byte(value: u8) -> (u8, u8) {
    (value & 0x7F, (value >> 7) & 0x01)
}

/// Build a Set LED Color Palette Entry SysEx message.
///
/// The W channel controls white-only LEDs that share the same palette.
/// We derive W from perceptual luminance so white buttons respond to
/// the same palette index with appropriate brightness.
fn set_palette_entry(index: u8, r: u8, g: u8, b: u8) -> [u8; 17] {
    let (r_lsb, r_msb) = encode_sysex_byte(r);
    let (g_lsb, g_msb) = encode_sysex_byte(g);
    let (b_lsb, b_msb) = encode_sysex_byte(b);
    // Derive W from perceptual luminance (BT.709) so white-only buttons
    // get meaningful brightness from the same palette slot.
    let w = ((r as f32 * 0.2126 + g as f32 * 0.7152 + b as f32 * 0.0722) + 0.5) as u8;
    let (w_lsb, w_msb) = encode_sysex_byte(w);
    [
        0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01, 0x03,
        index,
        r_lsb, r_msb, g_lsb, g_msb, b_lsb, b_msb,
        w_lsb, w_msb,
        0xF7,
    ]
}
```

### 4.4 Color Deduplication

At 30–60 FPS, sending 90 SysEx palette updates per frame is expensive. Optimize with:

1. **Frame-level dedup** — hash current colors, only reprogram entries that changed from previous frame
2. **Color quantization** — if >127 unique colors needed (unlikely for 90 LEDs), quantize to nearest
3. **Batch palette writes** — group SysEx messages, minimize per-message overhead
4. **Dirty tracking** — maintain a `[Option<[u8; 3]>; 128]` mirror of the device palette, only send diffs

### 4.5 Reapply Color Palette (SysEx 0x05)

```
F0 00 21 1D 01 01 05 F7
```

Forces all LEDs to refresh from current palette. Send after updating palette entries to ensure visual consistency (avoids partial updates where some LEDs show old colors).

---

## 5. LED Addressing

### 5.1 Pad Grid (8×8 Matrix)

64 RGB pads addressed via MIDI Note On. Bottom-left = note 36, top-right = note 99:

```
        Track1  Track2  Track3  Track4  Track5  Track6  Track7  Track8
       ┌──────┬──────┬──────┬──────┬──────┬──────┬──────┬──────┐
Row 8  │  92  │  93  │  94  │  95  │  96  │  97  │  98  │  99  │  ← top
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 7  │  84  │  85  │  86  │  87  │  88  │  89  │  90  │  91  │
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 6  │  76  │  77  │  78  │  79  │  80  │  81  │  82  │  83  │
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 5  │  68  │  69  │  70  │  71  │  72  │  73  │  74  │  75  │
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 4  │  60  │  61  │  62  │  63  │  64  │  65  │  66  │  67  │
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 3  │  52  │  53  │  54  │  55  │  56  │  57  │  58  │  59  │
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 2  │  44  │  45  │  46  │  47  │  48  │  49  │  50  │  51  │
       ├──────┼──────┼──────┼──────┼──────┼──────┼──────┼──────┤
Row 1  │  36  │  37  │  38  │  39  │  40  │  41  │  42  │  43  │  ← bottom
       └──────┴──────┴──────┴──────┴──────┴──────┴──────┴──────┘
```

**MIDI message:** `0x90 <note> <palette_index>` (Note On, channel 0, velocity = color)

To turn off a pad: send velocity 0 (palette index 0 = black).

### 5.2 RGB Buttons

Addressed via MIDI Control Change. Velocity/value = palette index (0–127).

**Display Buttons (above display):**

| CC | Button |
|----|--------|
| 102 | Track 1 |
| 103 | Track 2 |
| 104 | Track 3 |
| 105 | Track 4 |
| 106 | Track 5 |
| 107 | Track 6 |
| 108 | Track 7 |
| 109 | Track 8 |

**Display Buttons (below display):**

| CC | Button |
|----|--------|
| 20 | Select 1 |
| 21 | Select 2 |
| 22 | Select 3 |
| 23 | Select 4 |
| 24 | Select 5 |
| 25 | Select 6 |
| 26 | Select 7 |
| 27 | Select 8 |

**Scene Launch Buttons (right side, vertical):**

| CC | Scene |
|----|-------|
| 43 | Scene 1 (top) |
| 42 | Scene 2 |
| 41 | Scene 3 |
| 40 | Scene 4 |
| 39 | Scene 5 |
| 38 | Scene 6 |
| 37 | Scene 7 |
| 36 | Scene 8 (bottom) |

**Transport & Navigation (RGB subset):**

| CC | Button |
|----|--------|
| 85 | Play |
| 86 | Record |
| 3 | Tap Tempo |
| 9 | Metronome |

**MIDI message:** `0xB0 <cc> <palette_index>` (CC, channel 0)

### 5.3 White-Only Buttons

Same CC addressing, but only the **W** channel of the palette entry is used. These buttons have a single white LED — they cannot display color. Because `set_palette_entry` now derives W from luminance (§4.3), white buttons respond automatically to any palette index with appropriate brightness.

| CC | Button | CC | Button |
|----|--------|----|--------|
| 28 | Master | 62 | Page Left |
| 29 | Stop Clip | 63 | Page Right |
| 30 | Setup | 87 | New |
| 31 | Layout | 88 | Duplicate |
| 35 | Convert | 89 | Automate |
| 44 | Arrow Left | 90 | Fixed Length |
| 45 | Arrow Right | 110 | Device |
| 46 | Arrow Up | 111 | Browse |
| 47 | Arrow Down | 112 | Mix |
| 48 | Select | 113 | Clip |
| 49 | Shift | 116 | Quantize |
| 50 | Note | 117 | Double Loop |
| 51 | Session | 118 | Delete |
| 52 | Add Device | 119 | Undo |
| 53 | Add Track | 56 | Repeat |
| 54 | Octave Down | 57 | Accent |
| 55 | Octave Up | 58 | Scale |
| 59 | User | 60 | Mute |
| 61 | Solo | | |

**Driver scope note:** The initial implementation controls only the RGB zones (pads + RGB buttons + touch strip). White buttons are **left at their default palette values** — their LEDs will remain in whatever state the device firmware initializes. A future enhancement can add white buttons as an optional zone if reactive white-button effects are desired.

### 5.4 Zone Layout for Hypercolor

The engine sees Push 2 as a multi-zone device:

| Zone | LEDs | Topology | Color Format |
|------|------|----------|-------------|
| `Pads` | 64 | `Matrix { rows: 8, cols: 8 }` | RGB |
| `Buttons Above` | 8 | `Strip` | RGB |
| `Buttons Below` | 8 | `Strip` | RGB |
| `Scene Launch` | 8 | `Strip` | RGB |
| `Transport` | 4 | `Custom` | RGB |
| `Touch Strip` | 31 | `Strip` | White (8-level, quantized from RGB) |
| `Display` | 0 | `Display { width: 960, height: 160, circular: false }` | JPEG (decoded to RGB565 internally) |

**Total addressable LEDs:** 123 (64 pads + 28 RGB buttons + 31 touch strip)
**Display:** 960×160 pixels (separate zone, not counted as LEDs)

**Scoped out of initial implementation:** ~35 white-only buttons (see §5.3). These are left at device defaults — controllable via the same palette mechanism if a white-button zone is added later.

---

## 6. LED Animations

### 6.1 Animation Model

Push 2 supports hardware-accelerated LED animations controlled by **MIDI channel selection**. The device requires **MIDI System Real-Time clock messages** (`0xF8`) from the host to advance animation timing.

### 6.2 Two-Step Animation Protocol

1. Set the **starting color** — Note On / CC on **channel 0** (velocity = palette index)
2. Set the **ending color + animation** — Note On / CC on **channels 1–15** (velocity = palette index for end color)

The LED will transition between the two palette colors using the animation type and speed determined by the channel.

### 6.3 Animation Channel Table

| Channel | Type | Duration |
|---------|------|----------|
| 0 | Static (no animation) | — |
| 1 | One-shot fade | 1/24th note |
| 2 | One-shot fade | 1/16th note |
| 3 | One-shot fade | 1/8th note |
| 4 | One-shot fade | 1/4 note |
| 5 | One-shot fade | 1/2 note |
| 6 | Pulsing (continuous) | 1/24th note |
| 7 | Pulsing | 1/16th note |
| 8 | Pulsing | 1/8th note |
| 9 | Pulsing | 1/4 note |
| 10 | Pulsing | 1/2 note |
| 11 | Blinking (on/off) | 1/24th note |
| 12 | Blinking | 1/16th note |
| 13 | Blinking | 1/8th note |
| 14 | Blinking | 1/4 note |
| 15 | Blinking | 1/2 note |

### 6.4 MIDI Clock & Transport for Animations

Animations require three MIDI System Real-Time messages from the host:

| Message | Byte | Purpose |
|---------|------|---------|
| Start | `0xFA` | Begin animation playback (resets phase) |
| Clock | `0xF8` | Advance animation timing (24 ppqn) |
| Stop | `0xFC` | Halt animations (LEDs hold current state) |

The animation clock runs at 24 PPQN (pulses per quarter note). At 120 BPM, that's 48 clock messages/second. The driver must send Start before the first Clock, and Stop to halt.

**Port routing:** In User mode, MIDI clock must be sent on the **User port** (port 2). The device ignores clock on the Live port when in User mode.

**Initial implementation: static colors only** (channel 0). The driver should:

1. **Not send MIDI clock by default** — animations are optional and clock conflicts with DAW use
2. Not send Start/Stop messages
3. Expose animation capability as a future enhancement

If animations are added later, the driver becomes a MIDI clock source. This requires careful design to avoid conflicts when the device is connected alongside a DAW — only one clock source should be active at a time.

---

## 7. Touch Strip LEDs

### 7.1 LED Layout

31 white LEDs along the touch strip, addressed via a dedicated SysEx command. LED 0 = bottom, LED 30 = top.

### 7.2 Host Control Setup

Before the host can control touch strip LEDs, send configuration SysEx (`0x17`) with bits 0 and 1 set:

```
F0 00 21 1D 01 01 17 6B F7
```

Configuration byte `0x6B` = `0b1101011`:
- Bit 0 = 1: LEDs controlled by host
- Bit 1 = 1: Host sends SysEx (required for `0x19` LED control)
- Bit 2 = 0: Values as pitch bend (for touch position output)
- Bit 3 = 1: Point display mode
- Bit 4 = 0: Bar starts at bottom
- Bit 5 = 1: Autoreturn enabled
- Bit 6 = 1: Autoreturn to center

**Important:** Bit 1 must be set when using SysEx command `0x19` to drive LEDs from the host.
With bit 1 = 0, the device expects raw values and controls its own LED visualization.

### 7.3 Set Touch Strip LEDs (SysEx 0x19)

```
F0 00 21 1D 01 01 19 <b0> <b1> ... <b15> F7
```

16 data bytes encode 31 LEDs using 3-bit brightness indices packed in pairs:

```
Byte layout: 0|LED(n+1)[2:0]|0|LED(n)[2:0]

b0:  0|LED1[2:0]|0|LED0[2:0]     (LEDs 0–1)
b1:  0|LED3[2:0]|0|LED2[2:0]     (LEDs 2–3)
...
b14: 0|LED29[2:0]|0|LED28[2:0]   (LEDs 28–29)
b15: 0|000|0|LED30[2:0]           (LED 30 only)
```

### 7.4 Brightness Palette (3-bit)

| Index | Brightness |
|-------|-----------|
| 0 | Off |
| 1 | 2 |
| 2 | 4 |
| 3 | 8 |
| 4 | 16 |
| 5 | 32 |
| 6 | 64 |
| 7 | 127 (full) |

### 7.5 Encoding Helper

```rust
/// Pack 31 touch strip brightness values (0–7 each) into 16 SysEx data bytes.
fn encode_touch_strip(levels: &[u8; 31]) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for i in 0..15 {
        let lo = levels[i * 2] & 0x07;
        let hi = levels[i * 2 + 1] & 0x07;
        bytes[i] = (hi << 4) | lo;
    }
    bytes[15] = levels[30] & 0x07;
    bytes
}
```

---

## 8. Display Interface

### 8.1 Specifications

| Parameter | Value |
|-----------|-------|
| Resolution | 960 × 160 pixels |
| Color depth | 16-bit RGB565, little-endian |
| Frame rate | 60 FPS max |
| Buffering | Double-buffered (displays on frame completion) |
| Blanking | Screen goes black after 2s with no frame data |
| USB endpoint | Bulk OUT `0x01`, interface 0 |

### 8.2 Pixel Format (RGB565)

```
Bit:  15  14  13  12  11  10  09  08  07  06  05  04  03  02  01  00
      B4  B3  B2  B1  B0  G5  G4  G3  G2  G1  G0  R4  R3  R2  R1  R0
```

Little-endian byte order on the wire.

### 8.3 Frame Structure

1. Send 16-byte **frame header**: `FF CC AA 88 00 00 00 00 00 00 00 00 00 00 00 00`
2. Send 160 **line buffers**, each 2048 bytes:
   - 1920 bytes = 960 pixels × 2 bytes/pixel
   - 128 bytes padding (zeroes)
3. Pixel payload: **327,680 bytes** (160 × 2048)
4. Total frame: **327,696 bytes** (16-byte header + 327,680 pixel payload)
5. Sent as 16-byte header + **640 USB bulk transfers** of 512 bytes each

### 8.4 XOR Signal Shaping

Every line buffer must be XOR'd with a repeating 4-byte mask before transmission:

```
Offset 0: XOR 0xE7
Offset 1: XOR 0xF3
Offset 2: XOR 0xE7
Offset 3: XOR 0xFF
```

Pattern repeats across all 2048 bytes of each line buffer.

```rust
const DISPLAY_XOR_MASK: [u8; 4] = [0xE7, 0xF3, 0xE7, 0xFF];

/// Apply signal shaping XOR mask to a line buffer in-place.
fn xor_shape_line(line: &mut [u8; 2048]) {
    for (i, byte) in line.iter_mut().enumerate() {
        *byte ^= DISPLAY_XOR_MASK[i & 3];
    }
}
```

### 8.5 Display Brightness (SysEx 0x08 / 0x09)

**Set:** `F0 00 21 1D 01 01 08 <b_LSB> <b_MSB> F7` (0–255)
**Get:** `F0 00 21 1D 01 01 09 F7`

USB-powered brightness is capped at ~100 (~7% of max).

### 8.6 Integration with Hypercolor

The display maps to the existing `write_display_frame` / `encode_display_frame` path:

1. Engine renders effect to a 960×160 canvas
2. Encodes as JPEG (or raw RGB565 — see below)
3. Protocol's `encode_display_frame` converts to XOR-shaped bulk packets
4. Transport sends via bulk endpoint

Push 2 expects raw RGB565 pixels — it has no onboard JPEG decoder. The existing
`encode_display_frame(&self, jpeg_data: &[u8])` API passes JPEG data end-to-end through core,
daemon, and backend. Rather than redesigning the cross-crate display pipeline, the Push 2
protocol accepts JPEG input through the standard API and **decodes to RGB565 inside the
protocol implementation** using the `image` crate (already a workspace dependency via Corsair
LCD).

```rust
fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
    // Decode JPEG → RGB8, convert to RGB565, apply XOR shaping, chunk to bulk packets
    let img = image::load_from_memory_with_format(jpeg_data, ImageFormat::Jpeg).ok()?;
    let rgb = img.resize_exact(960, 160, FilterType::Nearest).to_rgb8();
    Some(self.build_display_commands(&rgb))
}
```

This keeps the core/daemon display pipeline unchanged while giving Push 2 correct pixel output.
If raw-frame APIs are added later (e.g., `DeviceColorFormat::Rgb565`), the JPEG decode step can
be bypassed — but that's a cross-crate change beyond the scope of this driver.

---

## 9. SysEx Command Reference

All Push 2 SysEx commands follow this structure:

```
F0 00 21 1D 01 01 <CMD> [ARGS...] F7
```

| Byte(s) | Value | Meaning |
|---------|-------|---------|
| `F0` | — | Start of Exclusive |
| `00 21 1D` | — | Ableton manufacturer ID |
| `01` | — | Device ID |
| `01` | — | Model ID (Push 2) |
| `CMD` | 7-bit | Command ID |
| `ARGS` | 7-bit × N | Up to 17 seven-bit values |
| `F7` | — | End of Exclusive |

### 9.1 Full Command Table

| ID | Command | Has Reply | Used By Driver |
|----|---------|-----------|---------------|
| `0x03` | Set LED Color Palette Entry | No | ✓ Hot path |
| `0x04` | Get LED Color Palette Entry | Yes | Init (read defaults) |
| `0x05` | Reapply Color Palette | No | ✓ Hot path |
| `0x06` | Set LED Brightness | No | ✓ Brightness control |
| `0x07` | Get LED Brightness | Yes | Init |
| `0x08` | Set Display Brightness | No | ✓ Brightness control |
| `0x09` | Get Display Brightness | Yes | Init |
| `0x0A` | Set MIDI Mode | Yes (both ports) | ✓ Init (switch to User) |
| `0x0B` | Set LED PWM Frequency | No | Optional tuning |
| `0x14` | Set LED White Balance | No | Optional calibration |
| `0x15` | Get LED White Balance | Yes | Init (read calibration) |
| `0x17` | Set Touch Strip Config | No | ✓ Init (host LED control) |
| `0x18` | Get Touch Strip Config | Yes | Init |
| `0x19` | Set Touch Strip LEDs | No | ✓ Frame encoding |
| `0x1A` | Request Statistics | Yes | Diagnostics |
| `0x1E` | Set Aftertouch Mode | No | — (input, not LED) |
| `0x1F` | Get Aftertouch Mode | Yes | — |

### 9.2 Device Identity Request

Standard MIDI Identity Request:

**Request:** `F0 7E 01 06 01 F7`
**Reply:**
```
F0 7E 01 06 02 00 21 1D 67 32 02 00
<major> <minor> <build_LSB> <build_MSB>
<serial_0..4> <board_rev> F7
```

Use during discovery to confirm device identity and read firmware version.

---

## 10. Initialization & Mode Switching

### 10.1 Init Sequence

```
Step  Command                              Purpose
────  ─────────────────────────────────────────────────────────────────
  1   Identity Request (F0 7E 01 06 01 F7) Confirm device, read firmware
  2   Set MIDI Mode → User (0x0A 0x01)     Route LED commands to User port
  3   Set Touch Strip Config (0x17 0x6B)    Enable host LED control
  4   Set LED Brightness (0x06 <level>)     Match user preference
  5   Set Display Brightness (0x08 <level>) Match user preference
  6   Read default palette (0x04 × 128)     Cache factory palette for restore
  7   All LEDs off (velocity 0 sweep)       Clean slate
```

### 10.2 Shutdown Sequence

```
Step  Command                               Purpose
────  ──────────────────────────────────────────────────────────────
  1   All LEDs off (velocity 0 sweep)       Dark all pads and buttons
  2   Restore factory palette (0x03 × 128)  Leave device in stock state
  3   Reapply palette (0x05)                Flush restored palette
  4   Set MIDI Mode → Live (0x0A 0x00)      Return control to Ableton Live
  5   Reset touch strip config (0x17 0x68)  Return to default touch strip
```

### 10.3 Mode Considerations

- **User mode** (`0x01`) is required for external LED control. In Live mode, Ableton Live owns the LEDs.
- **Dual mode** (`0x02`) allows both Live and the driver to send — risk of visual conflicts. Avoid unless explicitly requested.
- The mode switch reply is echoed to **both** MIDI ports. The driver should expect and discard the echo on the Live port.

---

## 11. HAL Integration

### 11.1 Protocol Implementation

```rust
use std::sync::RwLock;

/// Mutable state for per-frame diff tracking.
/// Wrapped in RwLock because Protocol trait methods take &self.
/// Pattern matches existing drivers (Corsair Link, ASUS Aura).
struct Push2State {
    /// Mirror of the device's current palette (for dirty tracking).
    palette: [[u8; 4]; 128],       // [R, G, B, W] per slot
    /// Previous frame's LED→palette index mapping.
    prev_led_indices: Vec<u8>,
    /// Touch strip state mirror.
    prev_touch_strip: [u8; 31],
    /// Whether any palette entry was modified this frame.
    palette_dirty: bool,
}

pub struct Push2Protocol {
    state: RwLock<Push2State>,
}
```

### 11.2 Protocol Trait Implementation

```rust
impl Protocol for Push2Protocol {
    fn name(&self) -> &'static str { "Ableton Push 2" }

    fn init_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            // Identity request
            midi_sysex(IDENTITY_REQUEST),
            // Set User mode
            push2_sysex(0x0A, &[0x01]),
            // Enable host touch strip control (bit 0 + bit 1)
            push2_sysex(0x17, &[0x6B]),
            // All pads off
            all_leds_off(),
        ]
    }

    fn shutdown_sequence(&self) -> Vec<ProtocolCommand> {
        vec![
            all_leds_off(),
            // Return to Live mode
            push2_sysex(0x0A, &[0x00]),
            // Reset touch strip to device-controlled defaults
            push2_sysex(0x17, &[0x68]),
        ]
    }

    fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand> {
        let mut commands = Vec::new();
        self.encode_frame_into(colors, &mut commands);
        commands
    }

    fn encode_frame_into(
        &self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>,
    ) {
        // See §12 Render Pipeline — acquires state.write() for diff tracking
    }

    fn encode_brightness(&self, brightness: u8) -> Option<Vec<ProtocolCommand>> {
        // Map 0–255 engine brightness to 0–127 Push 2 range
        let push_brightness = brightness / 2;
        Some(vec![ProtocolCommand::new(
            push2_sysex(0x06, &[push_brightness]),
            TransferType::Primary,
        )])
    }

    fn parse_response(&self, data: &[u8]) -> Result<ProtocolResponse, ProtocolError> {
        // Validate SysEx framing (F0...F7)
        if data.len() < 2 || data[0] != 0xF0 || data[data.len() - 1] != 0xF7 {
            return Err(ProtocolError::InvalidResponse);
        }
        // Check Ableton manufacturer ID
        if data.len() >= 7 && data[1..4] == [0x00, 0x21, 0x1D] {
            Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: data[6..data.len() - 1].to_vec(),
            })
        } else {
            // Standard identity reply (F0 7E ...)
            Ok(ProtocolResponse {
                status: ResponseStatus::Ok,
                data: data.to_vec(),
            })
        }
    }

    fn connection_diagnostics(&self) -> Vec<ProtocolCommand> {
        // Request device statistics (SysEx 0x1A) to verify responsiveness
        vec![ProtocolCommand::new(
            push2_sysex(0x1A, &[]),
            TransferType::Primary,
        ).with_response(Duration::from_millis(500))]
    }

    fn encode_display_frame(&self, jpeg_data: &[u8]) -> Option<Vec<ProtocolCommand>> {
        // Decode JPEG → RGB8, convert to RGB565, apply XOR shaping,
        // chunk into bulk transfer packets. See §8.6.
        let img = image::load_from_memory_with_format(
            jpeg_data, ImageFormat::Jpeg,
        ).ok()?;
        let rgb = img.resize_exact(960, 160, FilterType::Nearest).to_rgb8();
        Some(self.build_display_commands(&rgb))
    }

    fn zones(&self) -> Vec<ProtocolZone> {
        vec![
            ProtocolZone {
                name: "Pads".to_owned(),
                led_count: 64,
                topology: DeviceTopologyHint::Matrix { rows: 8, cols: 8 },
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Buttons Above".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Buttons Below".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Scene Launch".to_owned(),
                led_count: 8,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Transport".to_owned(),
                led_count: 4,
                topology: DeviceTopologyHint::Custom,
                color_format: DeviceColorFormat::Rgb,
            },
            ProtocolZone {
                name: "Touch Strip".to_owned(),
                led_count: 31,
                topology: DeviceTopologyHint::Strip,
                color_format: DeviceColorFormat::Rgb, // quantized to 8-level white
            },
            ProtocolZone {
                name: "Display".to_owned(),
                led_count: 0, // display, not discrete LEDs
                topology: DeviceTopologyHint::Display {
                    width: 960,
                    height: 160,
                    circular: false,
                },
                color_format: DeviceColorFormat::Jpeg, // JPEG in, RGB565 conversion internal
            },
        ]
    }

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            led_count: 123,
            supports_direct: true,
            supports_brightness: true,
            has_display: true,
            display_resolution: Some((960, 160)),
            max_fps: 60,
        }
    }

    fn total_leds(&self) -> u32 { 123 }
    fn frame_interval(&self) -> Duration { Duration::from_millis(16) } // 60 FPS
}
```

### 11.3 Capabilities

| Capability | Supported |
|-----------|-----------|
| Direct RGB | ✓ (via palette reprogramming) |
| Brightness | ✓ (global LED brightness 0–127) |
| Display | ✓ (960×160 RGB565) |
| Display brightness | ✓ (0–255) |
| Animations | Future (requires MIDI clock) |
| Input events | Future (pad pressure, encoders) |
| Keepalive | Not needed (no timeout) |

---

## 12. Render Pipeline

### 12.1 Frame Encoding Flow

```
Engine colors (&[[u8; 3]])
       │
       ▼
┌─────────────────────┐
│  Color Dedup         │  Unique colors → palette slots
│  (hash + cache)      │  Typically 10–40 unique per frame
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  Palette Diff        │  Compare against prev frame palette
│  (dirty tracking)    │  Only emit SysEx for changed entries
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  SysEx Palette Cmds  │  Set LED Color Palette Entry (0x03)
│  (per changed slot)  │  + Reapply Palette (0x05)
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  MIDI LED Cmds       │  Note On (pads) + CC (buttons)
│  (per changed LED)   │  Only LEDs whose palette index changed
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│  Touch Strip Cmd     │  SysEx 0x19 (if strip colors changed)
│  (single message)    │
└──────────┘
```

### 12.2 encode_frame_into Implementation

```rust
fn encode_frame_into(
    &self, colors: &[[u8; 3]], commands: &mut Vec<ProtocolCommand>,
) {
    let mut encoder = CommandBuffer::new(commands);
    let colors = self.normalize_colors(colors); // Cow borrow or pad/truncate

    // Phase 1: Build color→palette_index map, emit palette SysEx for changes
    let mut color_map: HashMap<[u8; 3], u8> = HashMap::new();
    let mut next_slot: u8 = 1; // slot 0 = OFF

    // Insert black as slot 0
    color_map.insert([0, 0, 0], 0);

    for color in colors.iter() {
        if !color_map.contains_key(color) {
            let slot = next_slot;
            next_slot += 1;

            // Only emit SysEx if this slot's color changed from prev frame
            if self.palette[slot as usize] != *color {
                let sysex = set_palette_entry(slot, color[0], color[1], color[2]);
                encoder.push_slice(
                    &sysex,
                    false,              // no response expected
                    Duration::ZERO,
                    Duration::ZERO,
                    TransferType::Primary,
                );
            }

            color_map.insert(*color, slot);
        }
    }

    // Phase 2: Reapply palette if any entries changed
    if self.palette_dirty() {
        encoder.push_slice(
            &REAPPLY_PALETTE_SYSEX,
            false, Duration::ZERO, Duration::ZERO,
            TransferType::Primary,
        );
    }

    // Phase 3: Emit Note On for pads (zone 0, indices 0–63)
    for (i, color) in colors[..64].iter().enumerate() {
        let note = PAD_NOTE_MAP[i];
        let index = color_map[color];
        // Only send if palette index changed for this LED
        if self.prev_note_index(note) != index {
            let msg = [0x90, note, index]; // Note On, ch 0
            encoder.push_slice(
                &msg,
                false, Duration::ZERO, Duration::ZERO,
                TransferType::Primary,
            );
        }
    }

    // Phase 4: Emit CC for buttons (zones 1–4)
    let button_colors = &colors[64..92];
    for (i, color) in button_colors.iter().enumerate() {
        let cc = BUTTON_CC_MAP[i];
        let index = color_map[color];
        if self.prev_cc_index(cc) != index {
            let msg = [0xB0, cc, index]; // CC, ch 0
            encoder.push_slice(
                &msg,
                false, Duration::ZERO, Duration::ZERO,
                TransferType::Primary,
            );
        }
    }

    // Phase 5: Touch strip (zone 5, indices 92–122)
    let strip_colors = &colors[92..123];
    let strip_levels = quantize_touch_strip(strip_colors);
    if strip_levels != self.prev_touch_strip {
        let packed = encode_touch_strip(&strip_levels);
        let mut sysex = Vec::with_capacity(24);
        sysex.extend_from_slice(&TOUCH_STRIP_HEADER);
        sysex.extend_from_slice(&packed);
        sysex.push(0xF7);
        encoder.push_slice(
            &sysex,
            false, Duration::ZERO, Duration::ZERO,
            TransferType::Primary,
        );
    }

    encoder.finish();
}
```

### 12.3 Pad Note Map

```rust
/// Maps linear pad index (0–63) to MIDI note number.
/// Index 0 = bottom-left pad, index 63 = top-right pad.
const PAD_NOTE_MAP: [u8; 64] = [
    36, 37, 38, 39, 40, 41, 42, 43, // Row 1 (bottom)
    44, 45, 46, 47, 48, 49, 50, 51, // Row 2
    52, 53, 54, 55, 56, 57, 58, 59, // Row 3
    60, 61, 62, 63, 64, 65, 66, 67, // Row 4
    68, 69, 70, 71, 72, 73, 74, 75, // Row 5
    76, 77, 78, 79, 80, 81, 82, 83, // Row 6
    84, 85, 86, 87, 88, 89, 90, 91, // Row 7
    92, 93, 94, 95, 96, 97, 98, 99, // Row 8 (top)
];
```

### 12.4 Button CC Map

```rust
/// Maps linear button index (0–27) to MIDI CC number.
/// Ordered: 8 above display, 8 below display, 8 scene launch, 4 transport.
const BUTTON_CC_MAP: [u8; 28] = [
    // Above display (Track 1–8)
    102, 103, 104, 105, 106, 107, 108, 109,
    // Below display (Select 1–8)
    20, 21, 22, 23, 24, 25, 26, 27,
    // Scene launch (top → bottom)
    43, 42, 41, 40, 39, 38, 37, 36,
    // Transport
    85, 86, 3, 9,
];
```

### 12.5 Touch Strip Quantization

```rust
/// Quantize RGB color to 3-bit touch strip brightness (0–7).
/// Uses perceptual luminance (ITU-R BT.709).
fn quantize_touch_strip(colors: &[[u8; 3]]) -> [u8; 31] {
    let mut levels = [0u8; 31];
    for (i, rgb) in colors.iter().take(31).enumerate() {
        let luma = (rgb[0] as f32 * 0.2126
                  + rgb[1] as f32 * 0.7152
                  + rgb[2] as f32 * 0.0722) / 255.0;
        levels[i] = (luma * 7.0).round() as u8;
    }
    levels
}
```

### 12.6 Performance Characteristics

| Scenario | SysEx msgs | MIDI msgs | Bytes/frame |
|----------|-----------|-----------|-------------|
| All same color | 1 palette + 1 reapply | 64 notes + 28 CCs | ~300 |
| Steady state (no change) | 0 | 0 | 0 |
| Full rainbow (64 unique) | 64 palette + 1 reapply | 64 notes + 28 CCs | ~1,400 |
| Worst case (all unique, all changed) | 92 palette + 1 reapply | 64 notes + 28 CCs | ~1,900 |

At MIDI baud rate (~3,125 bytes/sec for DIN MIDI), this would be a bottleneck — but USB MIDI has no such limit. USB Full Speed allows ~1 MB/s, so even worst-case frames complete in <2ms.

---

## 13. Wire-Format Structs

### 13.1 SysEx Builder

Push 2 SysEx messages are variable-length, so zerocopy fixed-size structs aren't the right fit for the SysEx path. Instead, use a typed builder:

```rust
/// Ableton Push 2 SysEx header (common prefix for all commands).
const PUSH2_SYSEX_HEADER: [u8; 7] = [
    0xF0,       // Start of Exclusive
    0x00, 0x21, 0x1D, // Ableton manufacturer ID
    0x01,       // Device ID
    0x01,       // Model ID (Push 2)
    // CMD byte follows
];

/// Build a complete Push 2 SysEx message.
fn push2_sysex(cmd: u8, args: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(8 + args.len());
    msg.extend_from_slice(&PUSH2_SYSEX_HEADER);
    msg.push(cmd);
    msg.extend_from_slice(args);
    msg.push(0xF7); // End of Exclusive
    msg
}
```

### 13.2 Display Frame Header

```rust
/// Fixed 16-byte header sent before each display frame.
#[derive(IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Push2DisplayHeader {
    magic: [u8; 4],    // FF CC AA 88
    padding: [u8; 12], // All zeroes
}

const _: () = assert!(
    std::mem::size_of::<Push2DisplayHeader>() == 16,
    "Push2DisplayHeader must be exactly 16 bytes"
);

const DISPLAY_HEADER: Push2DisplayHeader = Push2DisplayHeader {
    magic: [0xFF, 0xCC, 0xAA, 0x88],
    padding: [0; 12],
};
```

### 13.3 Display Line Buffer

```rust
/// Single display line: 960 pixels × 2 bytes (RGB565) + 128 bytes padding.
#[derive(FromZeros, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct Push2DisplayLine {
    pixels: [u8; 1920], // 960 × 2 bytes RGB565
    padding: [u8; 128],
}

const _: () = assert!(
    std::mem::size_of::<Push2DisplayLine>() == 2048,
    "Push2DisplayLine must be exactly 2048 bytes"
);
```

---

## 14. Testing Strategy

### 14.1 Unit Tests

Located in `crates/hypercolor-hal/tests/push2/`:

| Test | Validates |
|------|----------|
| `sysex_encoding` | SysEx byte encoding (7-bit split, message framing) |
| `palette_entry_encoding` | Set Palette Entry message format for all color extremes |
| `pad_note_map` | Linear index ↔ MIDI note mapping (boundary: 0→36, 63→99) |
| `button_cc_map` | Linear index ↔ CC mapping for all button groups |
| `touch_strip_packing` | 31 brightness values → 16 packed bytes |
| `touch_strip_quantization` | RGB → 3-bit luminance quantization accuracy |
| `display_xor_shaping` | XOR mask application, round-trip verification |
| `display_line_size` | Compile-time size assertion for `Push2DisplayLine` |
| `display_header_format` | Header magic bytes match spec |
| `frame_encoding_dedup` | Color deduplication produces correct palette slot count |
| `frame_encoding_diff` | Unchanged frames produce zero commands |
| `init_sequence` | Init commands are well-formed and in correct order |
| `shutdown_sequence` | Shutdown restores device to Live mode |

### 14.2 Integration Tests

| Test | Validates |
|------|----------|
| `palette_round_trip` | Write palette entries, read back via `0x04`, verify match |
| `display_frame_timing` | 60 FPS display sustains without USB transfer errors |
| `mode_switch_echo` | Mode switch reply received on both ports |
| `identity_parse` | Device identity reply parsed correctly |

### 14.3 Udev Rules (Linux)

Add Ableton VID to `udev/99-hypercolor.rules` for non-root access to the bulk display interface:

```udev
# Ableton Push 2 — bulk display interface requires libusb access
SUBSYSTEM=="usb", ATTR{idVendor}=="2982", ATTR{idProduct}=="1967", MODE="0660", GROUP="users", TAG+="uaccess"
```

The MIDI interfaces are managed by the ALSA sequencer and don't need udev rules — `midir` accesses them through the standard ALSA API. Only the bulk display endpoint (interface 0, claimed via `nusb`) requires direct USB device permissions.

### 14.4 Manual Verification

- Connect Push 2, run Hypercolor with `--log-level debug`
- Apply a solid color effect → all 64 pads light up uniformly
- Apply a rainbow gradient → pads show smooth color gradient across 8×8 grid
- Verify touch strip reflects effect intensity
- Verify display renders effect visualization
- Disconnect/reconnect → device recovers cleanly
- Launch Ableton Live after shutdown → Live regains full control

---

## Appendix A: Decisions

| # | Topic | Decision | Rationale |
|---|-------|----------|-----------|
| 1 | MIDI library | **`midir`** | Cross-platform (Linux ALSA + macOS CoreMIDI) in a single crate. No conditional compilation, native SysEx support, well-maintained. |
| 2 | Display rendering | **JPEG in, RGB565 internal** | Use existing JPEG display pipeline unchanged. Decode JPEG → RGB565 inside the protocol impl using the `image` crate. Avoids cross-crate API changes; can be optimized to raw RGB565 later if needed. |
| 3 | Palette management | **Per-frame dedup** | Simple, stateless, worst case ~1.5 KB which clears USB in <2ms. LRU adds complexity with subtle failure modes. Upgrade only if profiling proves palette writes are a bottleneck. |

## Appendix B: Open Questions

1. **Input events** — Push 2 sends pad velocity, aftertouch, encoder rotation, and button presses. These could feed into Hypercolor's (future) reactive effect system. Not in scope for the initial driver but worth designing the data path.

2. **Conflict with Ableton Live** — if Live is running, it claims the User port. The driver should detect this (failed mode switch) and surface a clear error rather than silently failing.

3. **MIDI clock for animations** — if we add animation support, the driver becomes a MIDI clock source. This interacts with DAW sync. Needs careful design to avoid conflicts.

## Appendix C: Device Statistics (SysEx 0x1A)

Useful for diagnostics and connection health:

```
F0 00 21 1D 01 01 1A F7
```

Reply fields:
- **Power supply status:** 1 = external PSU, 0 = USB-only
- **Run ID:** 0–127, persists across sleep, resets on reboot (detect device reboots)
- **Uptime:** seconds since last reboot (32-bit, encoded as 5 SysEx bytes)

The driver should query statistics during `connection_diagnostics()` to verify the device is responsive and report power status (which affects brightness limits).
