# PrismRGB / Nollie USB HID Protocol Reference

> Reverse-engineered from SignalRGB driver plugins. Ready for OpenRGB implementation.

---

## Device Family Overview

All three controllers share the same basic architecture: USB HID devices with 65-byte packets (byte 0 = report ID `0x00`, bytes 1-64 = payload). They're made by the same manufacturer (Nollie/PrismRGB) with minor protocol variations.

| Device | VID | PID | HID Interface | Channels | LEDs/Channel | Color Format | Max LEDs |
|---|---|---|---|---|---|---|---|
| **Prism S** | `0x16D0` | `0x1294` | 2 | 2 (Strimer cables) | 120/108/162 | RGB | 282 |
| **Prism 8** | `0x16D5` | `0x1F01` | 0 | 8 | 126 | **GRB** | 1008 |
| **Prism Mini** | `0x16D0` | `0x1407` | 2 | 1 | 128 | RGB | 128 |
| **Nollie 8 v2** | `0x16D2` | `0x1F01` | 0 | 8 | 126 | **GRB** | 1008 |

---

## Prism 8 / Nollie 8 Protocol (Identical)

These two devices share the exact same protocol. Only the VID differs.

### Initialization Sequence

```
1. Get firmware version:
   WRITE: [0x00, 0xFC, 0x01, 0x00...] (65 bytes, zero-padded)
   READ:  [??, ??, VERSION, ...]  → version = response[2]

2. Get channel LED counts:
   WRITE: [0x00, 0xFC, 0x03, 0x00...] (65 bytes, zero-padded)
   READ:  [ch0_hi, ch0_lo, ch1_hi, ch1_lo, ...] → 8 x uint16 big-endian

3. Set hardware effect (static color for when host disconnects):
   WRITE: [0x00, 0xFE, 0x02, 0x00, R, G, B, 0x64, 0x0A, 0x00, 0x01]
```

### Render Loop (called every frame at 33fps or 60fps)

```
For each channel (0-7):
  Split channel color data into packets of 21 LEDs (63 bytes RGB)

  For each packet:
    packet_id = packet_index + (channel * 6)
    WRITE: [0x00, packet_id, <up to 63 bytes of GRB color data>] (65 bytes)

After all channels sent:
  WRITE: [0x00, 0xFF] (65 bytes) ← FRAME COMMIT / LATCH

Periodically (every 150 frames, firmware v2+):
  Read voltage:
    WRITE: [0x00, 0xFC, 0x1A, 0x00...] (65 bytes)
    READ:  [??, usb_lo, usb_hi, sata1_lo, sata1_hi, sata2_lo, sata2_hi, ...]
    → voltage = uint16 / 1000 (in volts)

  Update channel LED counts if changed:
    WRITE: [0x00, 0xFE, 0x03, ch0_lo, ch0_hi, ch1_lo, ch1_hi, ...]
```

### Packet Addressing Scheme

```
Channel 0: packets 0, 1, 2, 3, 4, 5   (up to 126 LEDs = 6 packets × 21 LEDs)
Channel 1: packets 6, 7, 8, 9, 10, 11
Channel 2: packets 12, 13, 14, 15, 16, 17
...
Channel 7: packets 42, 43, 44, 45, 46, 47
```

### Shutdown

```
1. Send all channels with shutdown color (GRB format)
2. Set hardware effect:
   WRITE: [0x00, 0xFE, 0x02, 0x00, R, G, B, 0x64, 0x0A, 0x00, 0x01]
3. Activate hardware mode:
   WRITE: [0x00, 0xFE, 0x01, 0x00]
```

### Key Constants

```
Brightness multiplier: 0.75 (Prism 8 only, Nollie 8 doesn't apply)
Max LEDs per packet: 21 (63 bytes / 3 bytes per LED)
Packet size: 65 bytes (report ID + 64 data)
Frame commit byte: 0xFF
Command prefix (query): 0xFC
Command prefix (write): 0xFE
```

---

## Prism S Protocol (Strimer Controller)

The Prism S is specifically designed for Lian Li Strimer cables. It manages two cable types:
- **24-pin ATX Strimer**: 120 LEDs in a 20x6 grid
- **Dual 8-pin GPU Strimer**: 108 LEDs in a 27x4 grid
- **Triple 8-pin GPU Strimer**: 162 LEDs in a 27x6 grid

### Initialization

```
Settings save (also used for shutdown color):
  WRITE: [0x00, 0xFE, 0x01, R, G, B, cable_mode]
  cable_mode: 0 = Triple 8-pin, 1 = Dual 8-pin
  Pause 50ms after write
```

### Render Loop

The Prism S uses a different packet scheme -- it builds a single large buffer and sends it in 64-byte chunks.

```
Buffer Layout:
  ATX Cable (if connected):
    Packets 0-4: RGB data (63 bytes each = 315 bytes for 105 LEDs)
    Packet 15: Final ATX packet (remaining LEDs)
    Total: 120 LEDs × 3 = 360 bytes across ~6 logical packets

  GPU Cable marker:
    If ATX connected: byte 320 = 0x05
    If ATX not connected: first packet starts with 0x05 then zero-fill

  GPU Cable (Dual 8-pin):
    18 bytes inline after 0x05 marker
    Packets 6-9: RGB data (63 bytes each)
    Packet 20: Final GPU packet
    Total: 108 LEDs × 3 = 324 bytes

  GPU Cable (Triple 8-pin):
    18 bytes inline after 0x05 marker
    Packets 6-13: RGB data (63 bytes each)
    Total: 162 LEDs × 3 = 486 bytes

Final transmission:
  Split entire buffer into 64-byte chunks
  For each chunk:
    WRITE: [0x00, <64 bytes of data>] (65 bytes)
```

### Key Differences from Prism 8

- Color format: **RGB** (not GRB)
- Brightness multiplier: **0.50** (dimmer than Prism 8's 0.75)
- No frame commit byte (0xFF) -- data latches on completion
- No firmware version query
- No voltage monitoring
- Packet numbering encodes cable type, not just channel
- Interface 2 (not 0)

---

## Prism Mini Protocol

Single-channel controller for up to 128 LEDs. More advanced firmware with hardware lighting effects.

### Initialization

```
Request firmware version:
  WRITE: [0x00, 0x00, 0x00, 0x00, 0xCC] (65 bytes)
  READ:  [??, major, minor, patch] → "major.minor.patch"
  Expected: "1.0.0"
```

### Render Loop

```
For each packet (1 to ceil(LED_count / 20)):
  WRITE: [0x00, packet_num, total_packets, 0x00, 0xAA, <RGB data...>] (65 bytes)

  Where:
    packet_num: 1-indexed
    total_packets: total packet count
    0xAA: data marker byte
    RGB data: up to 60 bytes (20 LEDs × 3)
```

### Low Power Saver Mode

Per-LED brightness limiting to reduce power draw on WS2812 strips:
```
For each LED (R, G, B triple):
  total = R + G + B
  if total > 175:
    scale = 175 / total
    R *= scale; G *= scale; B *= scale
```

### Color Compression Mode

Packs two LEDs into 3 bytes by reducing to 4-bit color:
```
compressed[0] = (R1 >> 4) | ((G1 >> 4) << 4)
compressed[1] = (B1 >> 4) | ((R2 >> 4) << 4)
compressed[2] = (G2 >> 4) | ((B2 >> 4) << 4)
```

### Hardware Lighting Configuration

```
WRITE: [0x00, 0x00, 0x00, 0x00, 0xBB,
        hwl_enable,      // 0x00 or 0x01
        hwl_return,       // return to HW lighting when host disconnects
        return_after_sec, // 1-60 seconds
        effect_mode,      // 1=Rainbow Wave, 2=Rainbow Cycle, 3=Solid, 4=Breathing
        effect_speed,     // 1-20
        brightness,       // 10-255
        R, G, B,          // solid/breathing color
        status_led,       // onboard LED enable
        compression]      // color compression enable
```

---

## OpenRGB Implementation Notes

### What's Needed for Each Device

**Prism 8 / Nollie 8 (easiest -- nearly identical to each other)**:
1. USB HID open with correct VID/PID and interface
2. Firmware version query (0xFC 0x01)
3. Channel LED count query (0xFC 0x03)
4. Per-channel GRB data transmission with packet_id = packet + channel*6
5. Frame commit (0xFF byte)
6. Hardware effect on disconnect (0xFE commands)

**Prism S**:
1. USB HID open on interface 2
2. Build combined ATX + GPU cable buffer
3. Chunk and send as 65-byte packets
4. Shutdown color save (0xFE 0x01)

**Prism Mini**:
1. USB HID open on interface 2
2. Firmware version check (0xCC command)
3. Numbered packet transmission with 0xAA marker
4. Optional: low power saver, color compression, hardware lighting config

### OpenRGB Controller Class Structure (C++)

```cpp
// Suggested file structure for OpenRGB fork:
// Controllers/PrismRGBController/
//   PrismRGBController.h          -- base class with shared HID ops
//   PrismRGBController.cpp
//   PrismRGB8Controller.h         -- Prism 8 / Nollie 8 (8-channel)
//   PrismRGB8Controller.cpp
//   PrismRGBSController.h         -- Prism S (Strimer)
//   PrismRGBSController.cpp
//   PrismRGBMiniController.h      -- Prism Mini
//   PrismRGBMiniController.cpp
//   RGBController_PrismRGB.h      -- OpenRGB RGBController wrapper
//   RGBController_PrismRGB.cpp
//   PrismRGBControllerDetect.cpp  -- Device detection
```

### Detection Entries

```cpp
// PrismRGBControllerDetect.cpp
REGISTER_HID_DETECTOR("PrismRGB Prism S",  DetectPrismRGBSControllers,  0x16D0, 0x1294);
REGISTER_HID_DETECTOR("PrismRGB Prism 8",  DetectPrismRGB8Controllers,  0x16D5, 0x1F01);
REGISTER_HID_DETECTOR("PrismRGB Prism Mini", DetectPrismRGBMiniControllers, 0x16D0, 0x1407);
REGISTER_HID_DETECTOR("Nollie 8",          DetectNollie8Controllers,    0x16D2, 0x1F01);
```

---

## SignalRGB Plugin API Reference (for future driver extraction)

The SignalRGB device API used in these plugins:

```javascript
// Device I/O
device.write(data: number[], length: number)     // Send HID report
device.read(data: number[], length: number, timeout?: number)  // Read HID report
device.flush()                                     // Clear read buffer
device.pause(ms: number)                           // Delay

// Channel Management
device.SetLedLimit(count: number)                  // Set total LED capacity
device.addChannel(name: string, ledCount: number)  // Create lighting channel
device.channel(name: string).getColors(format, order)  // Get effect colors
device.channel(name: string).ledCount              // Current LED count
device.createColorArray(color, count, format, order)   // Generate color buffer

// Subdevice Management (for Prism S Strimers)
device.createSubdevice(id: string)
device.setSubdeviceName(id, name)
device.setSubdeviceSize(id, width, height)
device.setSubdeviceLeds(id, names[], positions[][])
device.subdeviceColor(deviceId, x, y)              // Sample canvas at position

// Frame Rate
device.setFrameRateTarget(fps: number)             // 33 or 60

// Logging
device.log(message: string)
device.notify(title, message, duration)

// Validation
endpoint.interface                                  // HID interface number
```

This API maps cleanly to OpenRGB's controller abstraction:
- `device.write/read` → `hid_write/hid_read`
- `device.addChannel` → `RGBController::zones`
- `device.channel().getColors` → `RGBController::colors`
- `device.SetLedLimit` → `RGBController::leds`
