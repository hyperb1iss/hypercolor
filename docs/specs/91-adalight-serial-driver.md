# 91 -- Adalight Serial Driver

> Manually configured driver for DIY serial LED controllers: Arduino, ESP8266,
> ESP32, and RP2040 boards running Adalight-compatible firmware. Byte-level
> framing for classic Adalight and HyperSerial AWA, serial bandwidth math,
> latest-frame pacing, and why the driver lives beside the network drivers
> instead of in the HAL.

**Status:** Implemented (experimental, untested on real hardware)
**Crate:** `hypercolor-driver-adalight`
**Module path:** `hypercolor_driver_adalight`
**Driver id:** `adalight`
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Packet Format](#2-packet-format)
3. [Protocol Variants](#3-protocol-variants)
4. [Classic Adalight](#4-classic-adalight)
5. [AWA](#5-awa)
6. [Color Encoding and White Channel](#6-color-encoding-and-white-channel)
7. [Timing and Bandwidth](#7-timing-and-bandwidth)
8. [Configuration](#8-configuration)
9. [Driver Integration](#9-driver-integration)
10. [Safety](#10-safety)
11. [Testing Strategy](#11-testing-strategy)
12. [Implementation Notes](#12-implementation-notes)
13. [References](#references)

---

## 1. Overview

Adalight started as an Adafruit project: an Arduino streams RGB values from a
host over a USB-serial link into a WS2801 or WS2812 strip behind a monitor.
Its six-byte `Ada` header became the de facto framing for DIY ambient
lighting, and every major host speaks it: Prismatik, Hyperion.ng, HyperHDR,
and WLED's serial input. HyperSerial, the ESP8266, ESP32, and RP2040 firmware
from the HyperHDR project, extends the framing as **AWA**: same header shape,
new magic bytes, a Fletcher-style trailer, and optional white-channel data
for RGBW strips. AWA targets line rates of 2 Mbaud and up.

This driver lets Hypercolor drive those boards directly. The firmware
families it targets:

| Firmware | Boards | Framing | Default baud |
|---|---|---|---|
| Adafruit Adalight sketch (`LEDstream`) | AVR Arduinos | Ada | 115200 |
| Adalight-FastLED | AVR and most FastLED targets | Ada | 115200 |
| HyperSerialEsp8266 | ESP8266 | AWA | 2000000 |
| HyperSerialESP32 | ESP32, ESP32-S2, ESP32-S3, ESP32-C3 | AWA | 2000000 |
| HyperSerialPico | RP2040, RP2350 (USB CDC) | AWA | nominal |
| WLED serial input | ESP8266, ESP32 running WLED | Ada | 115200 |

Every fact in sections 2 through 7 comes from the sources listed in
[References](#references), written here in our own words. The MIT-licensed
sources (Hyperion.ng, HyperHDR, HyperSerial) were read for byte layouts and
checksum arithmetic. The copyleft sources (the Adalight and Adalight-FastLED
sketches under LGPL-3.0, WLED under EUPL-1.2, Prismatik under GPL) were read
for observable behavior only. No code from any of them was copied or
transliterated; the implementation was written from this spec.

### Goals

- Drive Ada and AWA firmware from a manually configured port.
- Pace output to what the serial line can carry, coalescing to the newest
  frame when the render loop is faster, and tell the user the computed limit.
- Never open, probe, or write to a port the user did not configure.

### Non-goals

- Auto-detection by USB VID/PID (see [Safety](#10-safety)).
- LightBerry APA102 mode (`LBAPA`), a host-side variant for one discontinued
  product line.
- The HyperSerial statistics request and the HyperHDR ESP reset handshake.
- TPM2, which WLED also accepts over serial.

---

## 2. Packet Format

Every frame starts with the same six-byte header. What follows depends on the
variant.

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 3 | Magic | see §3 | Selects the variant |
| 3 | 1 | Count high | `(N - 1) >> 8` | LED count minus one, big-endian |
| 4 | 1 | Count low | `(N - 1) & 0xFF` | |
| 5 | 1 | Check | `hi ^ lo ^ 0x55` | Header check byte |
| 6 | `B * N` | Payload | LED bytes | `B` is 3, or 4 for AWA RGBW (§5.3) |
| 6 + B * N | 0 or 4 | Calibration | gain, red, green, blue | AWA calibrated frames only (§5.2) |
| end - 3 | 0 or 3 | Trailer | `s1, s2, ext` | AWA only (§5.4) |

The count field always carries `N - 1`, so one frame describes 1 to 65,536
LEDs. LightBerry mode is the one exception in the ecosystem (it sends `N`),
which is part of why the driver does not offer it.

Worked header examples:

| LEDs | Count field | Header bytes |
|---|---|---|
| 1 | `0x0000` | `41 64 61 00 00 55` |
| 2 | `0x0001` | `41 64 61 00 01 54` |
| 300 | `0x012B` | `41 64 61 01 2B 7F` |
| 65,536 | `0xFFFF` | `41 64 61 FF FF 55` |

---

## 3. Protocol Variants

| Config value | Magic | ASCII | Payload per LED | Suffix | Accepted by |
|---|---|---|---|---|---|
| `ada` | `41 64 61` | `Ada` | 3 | none | Adafruit Adalight, Adalight-FastLED, WLED |
| `awa` | `41 77 61` | `Awa` | 3 | trailer | Every HyperSerial build |
| `awa` + `white = "firmware"` | `41 77 41` | `AwA` | 3 | calibration + trailer | Every HyperSerial build; RGBW builds apply it |
| `awa` + `white = "host"` | `41 57 61` | `AWa` | 4 | trailer | HyperSerial builds compiled for RGBW or APA102 only |

HyperSerial does not accept the `Ada` header, and the Adalight sketches and
WLED do not accept `Awa`, so the variant must match the firmware. HyperSerial
documents the three AWA headers as protocol versions 1 (`Awa`), 2 (`AwA`), and
3 (`AWa`).

---

## 4. Classic Adalight

### 4.1 Frame

Header (§2) followed by `3 * N` payload bytes. No trailer, no acknowledgement.

```
41 64 61 00 01 54  FF 00 00  00 80 FF
\_ "Ada" _/ \cnt/ chk \LED 0/ \LED 1/
```

### 4.2 Firmware behavior

- **Hello.** The Adafruit sketch and Adalight-FastLED print `Ada\n` at boot
  and again once a second while no data arrives. WLED prints `Ada` once at
  boot.
- **Payload order.** The Adalight sketches store wire bytes as R, G, B and
  apply the strip's own order on output, so the host sends RGB. Prismatik
  and Hyperion reorder on the host for firmware that does not.
- **Length mismatch.** Adalight-FastLED clears the strip when it accepts a
  header, reads every LED the header announces, and drops LEDs beyond its
  strip length. Missing LEDs stay black.
- **Resync.** On a magic mismatch the parser resumes scanning for `A`; on a
  check-byte mismatch it resumes after the magic. A lost byte therefore costs
  at most the frame in flight.
- **Input flush.** Adalight-FastLED, by default, discards buffered input after
  every strip update. A frame that arrives while the sketch is latching LEDs
  is lost, and the sketch resyncs on the next header. `latch_ms` (§7.4)
  exists for this.
- **Idle timeout.** The Adafruit sketch blanks the strip after 15 s without
  data. Adalight-FastLED blanks after 60 s by default (configurable, 0
  disables). WLED returns to its own effects after 2.5 s.
- **Line rate.** Both sketches default to 115200 baud. Native-USB boards
  ignore the value.

### 4.3 WLED serial caveat

While WLED's serial parser is looking for a header, several bytes are
commands rather than data: `v` prints the version, and `0xB0` to `0xB7`
switch the serial baud rate (115200 up to 1.5 Mbaud). If a byte is lost
mid-frame, payload bytes in that range can change WLED's baud rate until it
restarts. This is WLED's design and affects every Adalight host; the setup
guide recommends the network WLED driver instead.

---

## 5. AWA

### 5.1 Base frame (`Awa`)

Header with magic `41 77 61`, `3 * N` payload bytes, then the three-byte
trailer (§5.4).

```
41 77 61 00 00 55  10 20 30  60 A0 63
\_ "Awa" _/        \LED 0 /  s1 s2 ext
```

### 5.2 Calibrated frame (`AwA`)

Magic `41 77 41`. After the RGB payload come four calibration bytes, then the
trailer. The trailer covers the calibration bytes too.

| Offset (after payload) | Size | Field | Description |
|---|---|---|---|
| 0 | 1 | Gain | Scale applied to the derived white channel |
| 1 | 1 | Red | Red correction used while deriving white |
| 2 | 1 | Green | Green correction used while deriving white |
| 3 | 1 | Blue | Blue correction used while deriving white |

HyperSerial RGBW builds derive white on the device: they correct each channel
through lookup tables built from these values, take the minimum as white,
subtract it from R, G, and B, and scale white by the gain. New calibration
takes effect after the frame that carried it renders. The firmware's own
defaults are gain `0xFF` with R, G, B `0xA0` for cold-white strips, and
`0xFF, 0xB0, 0xB0, 0x70` for neutral white. Hyperion.ng and HyperHDR default
the host-side values to 255 across the board. This driver defaults to the
firmware's cold-white values, so turning calibration on without values
reproduces what an RGBW build does by default.

### 5.3 RGBW frame (`AWa`)

Magic `41 57 61`. Each LED takes four bytes: the three color bytes in the
configured color order, then white. The count field is still `N - 1` (LEDs,
not bytes), and the trailer covers all `4 * N` bytes. Only HyperSerial builds
compiled for RGBW (`NEOPIXEL_RGBW`) or APA102 accept this header; other builds
fall back to header seeking.

### 5.4 Trailer

The trailer protects everything between the header and the trailer: the
payload and, for `AwA`, the calibration block.

1. Start three accumulators `s1`, `s2`, `ext` at zero, and an eight-bit
   position counter at zero.
2. For each protected byte `b`, in order:
   - `s1 = (s1 + b) mod 255`
   - `s2 = (s2 + s1) mod 255`
   - `ext = (ext + (b XOR position)) mod 255`, then increment `position`,
     wrapping from 255 to 0.
3. Send `s1`, then `s2`, then `ext`, except that an `ext` of `0x41` is sent
   as `0xAA`.

The substitution keeps the trailer from ending in `A`. The ESP8266 build
verifies only `s1` and `s2` and returns to header seeking for the third
byte, where an `A` would falsely start a frame. The ESP32 and RP2040 builds
verify all three bytes and render only on a full match.

Worked vectors:

| Protected bytes | Trailer | Notes |
|---|---|---|
| `10 20 30` | `60 A0 63` | Base example |
| `FF` | `00 00 00` | Sums reduce modulo 255 |
| `41` | `41 41 AA` | `ext` substitution |
| 258 zero bytes | `00 00 01` | Position wraps at 256 |
| `00 00 00 FF A0 A0 A0` | `E1 C3 F0` | Black LED plus cold-white calibration |

### 5.5 Firmware behavior

- **Limits.** HyperSerial drops frames above 4,096 LEDs. Its receive ring is
  9,040 bytes on ESP32 and RP2040 and 5,120 bytes on ESP8266.
- **Hello.** HyperSerial prints a `Welcome!` line and `Awa driver <version>`
  at boot (except on ESP32-S2) and on request.
- **Idle.** The parser resets after 5 s without data. HyperSerial never
  blanks the strip on idle; an optional relay pin can cut power instead.
- **Line rate.** 2,000,000 baud by default. The firmware README reports that
  CP2102 bridges top out near 1 Mbaud, CH9102 bridges reach 4 Mbaud, and
  ESP32-S2 native CDC reaches 5 Mbaud. At 2 Mbaud, 100 Hz, and 250 LEDs it
  reports a 1 to 5 percent corrupt-frame rate, which the trailer catches.
- **Statistics request.** Count bytes `2A A2` with check byte `15` or `35`
  (never a valid check for those counts) ask the firmware to print
  statistics. This driver never sends it.

---

## 6. Color Encoding and White Channel

Hypercolor renders RGB. Each LED is reordered into one of six wire orders:

| `color_order` | Wire bytes from RGB `(r, g, b)` |
|---|---|
| `rgb` (default) | `r, g, b` |
| `rbg` | `r, b, g` |
| `grb` | `g, r, b` |
| `gbr` | `g, b, r` |
| `brg` | `b, r, g` |
| `bgr` | `b, g, r` |

The Adalight sketches and HyperSerial expect RGB and reorder for the strip
themselves; the option exists for firmware built with a different order.

White handling (`white`):

| Mode | Framing | Who derives white | Bytes per LED |
|---|---|---|---|
| `off` (default) | `Ada` or `Awa` | nobody (RGB strip) or firmware with built-in defaults | 3 |
| `firmware` | `AwA` | firmware, with the calibration block | 3 |
| `host` | `AWa` | Hypercolor | 4 |

In `host` mode white is the shared minimum of the three channels, which is
then subtracted from each: `(200, 100, 50)` becomes color `(150, 50, 0)` and
white `50`. The reordered color bytes come first, white last. No
Adalight-framed firmware the sources cover accepts four bytes per LED, so
`firmware` and `host` require `protocol = "awa"`.

---

## 7. Timing and Bandwidth

### 7.1 Line budget

Every source runs the port at 8N1: one start bit, eight data bits, one stop
bit, no parity, no flow control. A byte occupies 10 bit times, so a frame of
`F` bytes at `R` baud occupies the line for

```
frame_time = ceil(10 * F * 1_000_000 / R) microseconds
line_max_fps = floor(1_000_000 / (frame_time + latch))
```

| Strip | Variant | Baud | Frame bytes | Frame time | Line max FPS |
|---|---|---|---|---|---|
| 60 LEDs | Ada | 115200 | 186 | 16.146 ms | 61 |
| 100 LEDs | Ada | 115200 | 306 | 26.563 ms | 37 |
| 300 LEDs | Ada | 115200 | 906 | 78.646 ms | 12 |
| 60 LEDs | AWA | 2000000 | 189 | 0.945 ms | 1058 |
| 300 LEDs | AWA | 2000000 | 909 | 4.545 ms | 220 |
| 300 LEDs | AWA RGBW | 2000000 | 1209 | 6.045 ms | 165 |

Native-USB boards (RP2040, ESP32-S2 and S3 CDC, ATmega32U4) ignore the baud
value; their real limit is the USB link. The driver computes the budget from
the configured value, so those boards should be configured with a rate their
link can actually carry (HyperSerialPico's documentation uses 2 Mbaud and
up).

### 7.2 Pacing and coalescing

The backend advertises an output cadence whose minimum interval is the line
interval (`frame_time + latch`, or the user cap's interval when that is
longer). The core's latest-frame output queue honors it: while a frame is
waiting out the interval, newer frames replace it, so the device always
receives the newest frame and nothing queues behind the UART. The render
loop's own rate and tiers are untouched; a slow strip slows only itself.

As a backstop for adapters slower than their nominal rate, each write first
asks the driver how many bytes are still waiting in its output queue
(`TIOCOUTQ` on Unix, `ClearCommError` on Windows). When at least one whole
frame is still waiting, the write is reported as cadence-suppressed and the
output queue resends the newest frame one interval later. Queue depth, and so
added latency, stays under two frames.

Writes never call `tcdrain` (which would block an async worker thread for
the whole frame time). A write that cannot hand its bytes to the driver
within `max(1 s, 4 * frame_time)` fails with a write error so the lifecycle
can reconnect.

### 7.3 Keepalive

Idle behavior differs by firmware (§4.2, §5.5): the Adalight sketches blank
after 15 or 60 s and WLED drops out of realtime mode after 2.5 s. The output
cadence asks the core to resend the last frame after 1 s of silence, which
keeps every known firmware showing the current scene. Hyperion.ng's default
refresh interval is also 1 s.

### 7.4 Latch gap

`latch_ms` (default 0) adds idle time after every frame for firmware that
cannot receive while it updates the strip, such as Adalight-FastLED on AVR
with its input flush. WS2812 strips latch at about 30 µs per LED, so 300
LEDs need roughly 9 ms. This is an explicit per-device control; the default
follows the line rate.

### 7.5 Opening the port and board reset

Opening a tty on Linux asserts DTR and RTS, and classic Arduinos (ATmega328P
behind a USB-serial bridge) reset on that edge. The bootloader runs for one
to two seconds before the sketch starts. The driver does not touch DTR or
RTS, matching Hyperion.ng. With the default `startup_delay_ms = 0` the first
frames go to the bootloader and are lost; the sketch syncs on the first
header after boot. Setting `startup_delay_ms` (Hyperion.ng's fallback is
1500 ms) holds the first frame until the board is up. When a delay is set,
the driver reads whatever the firmware printed during it (`Ada`, or
`Awa driver ...`) and logs it, which confirms the baud rate and firmware
without sending anything. The connect timeout grows by the delay.

### 7.6 Shutdown

On disconnect the driver sends three black frames, but only if it ever sent
a frame, then closes the port on a blocking thread (closing a tty waits for
its output to drain). The repeats matter because HyperSerial never blanks on
idle and drops corrupt frames, and Adalight-FastLED drops frames that arrive
while it latches; any one of the three surviving turns the strip off.
Hyperion.ng repeats its power-off frame five times for the same reason.

---

## 8. Configuration

Devices are listed under `drivers.adalight.devices`:

```toml
[[drivers.adalight.devices]]
id = "desk"                       # stable identity: 1-64 of [A-Za-z0-9_-]
name = "Desk Strip"               # optional, defaults to id
port = "/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0"
baud_rate = 115200                # optional: 115200 for ada, 2000000 for awa
protocol = "ada"                  # ada | awa
led_count = 120
color_order = "rgb"               # rgb | rbg | grb | gbr | brg | bgr
white = "off"                     # off | firmware | host (awa only)
white_calibration = { gain = 255, red = 160, green = 160, blue = 160 }
startup_delay_ms = 0              # 0-10000
latch_ms = 0                      # 0-1000
max_fps = 0                       # 0 follows the line and the render loop
```

Validation rejects, with a message naming the device:

| Rule | Limit |
|---|---|
| Device count | at most 64 |
| `id` | 1-64 ASCII letters, digits, `-`, `_`; unique |
| `name` | at most 128 characters |
| `port` | non-empty after trimming, at most 255 bytes, no control characters; unique |
| `baud_rate` | 1,200 to 12,000,000 |
| `led_count` | 1 to 65,536 for `ada`, 1 to 4,096 for `awa` |
| `white` | `firmware` and `host` need `protocol = "awa"` |
| `startup_delay_ms`, `latch_ms`, `max_fps` | 10,000, 1,000, and 1,000 |
| Frame time | at most 1 s; a slower layout cannot animate |

`white_calibration` is read only when `white = "firmware"`; omitted fields
take the cold-white defaults. The driver's control surface exposes the
`devices` list as structured objects with the same fields, and a device's
read-only surface shows its port, baud rate, protocol, frame size, line
maximum FPS, and effective output cap.

---

## 9. Driver Integration

### 9.1 Placement: a driver module, not a HAL protocol

The HAL already has a serial transport (`UsbSerialTransport`, used by the
Dygma and Nollie CDC protocols). It is not the right home here:

- **Claiming.** HAL protocols are reached through USB discovery: a
  descriptor's VID and PID claim a device, and `UsbSerialTransport::open`
  finds its port by VID, PID, and serial number. Adalight boards use CH340,
  CP210x, FTDI, and stock Arduino or Espressif USB ids that also sit in
  unrelated hardware on the same machine, so any VID/PID path would grab the
  wrong device. This driver must only open a path the user typed.
- **Transport semantics.** `UsbSerialTransport` frames replies as
  newline-terminated lines ending in a `.` line (Dygma Focus) and calls
  `flush` (a blocking `tcdrain`) after every send. Adalight is write-only
  streaming where a blocking drain would stall an async worker for a whole
  frame at 115200 baud.
- **Shape.** Manual configuration, discovery from config, per-device control
  surfaces, and an owned output backend are exactly the network driver
  pattern (Specs 35, 50, 51, 52). The pixel protocols driver (Spec 87) is the
  closest sibling.

So the driver is its own crate, `hypercolor-driver-adalight`, depending on
`hypercolor-driver-api`, `hypercolor-driver-support`, `hypercolor-types`, and
`tokio-serial` (already in the workspace through the HAL, same major
version, so no new third-party crates). `hypercolor-driver-builtin` registers
it behind a `serial` feature group (`adalight`), on by default.

### 9.2 Module

| Item | Value |
|---|---|
| Descriptor | `DriverDescriptor::new("adalight", "Adalight Serial", DriverTransportKind::Serial, true, false)` |
| Output binding | `OutputBinding::Owned`, backend id `adalight` |
| Capabilities | config, discovery, controls, presentation |
| Default config | enabled, `devices = []` |

The module kind is derived from the transport (`DriverTransportKind::Serial`
maps to `hal`), because `DriverModuleKind` has no variant for a module that
owns a serial output backend. The value is only a label in the CLI.

### 9.3 Discovery

Discovery parses the config and materializes one `DiscoveredDevice` per
entry. It opens no port.

| Field | Value |
|---|---|
| Fingerprint | `usb:adalight:<id>` (`FingerprintNamespace::Usb`), from the config id only |
| `connection_type` | `Usb` (nearly every board is a USB-serial device) |
| `origin` | driver `adalight`, backend `adalight`, transport `Serial` |
| Segment | one `Strip` segment, `Rgbw` color format when `white` is not `off` |
| `max_fps` | effective output cap (§7.1, user cap applied) |
| `claim` | `None`: nothing came from hardware |
| Metadata | `port`, `baud_rate`, `protocol`, `frame_bytes`, `line_max_fps`, and `adalight_device` (the entry as JSON) |

The backend adopts a device by re-validating `adalight_device`, so a config
edit reaches the next connection without rebuilding the backend.

### 9.4 Backend

- **Connect** opens the configured path at the configured baud, 8N1, no flow
  control, on a blocking thread. Unix opens take `TIOCEXCL` and an exclusive `flock`, so a second host
  (Hyperion, Prismatik) holding the port makes the open fail instead of
  sharing it. A missing port is a retryable connection failure; a
  permission error is reported as such.
- **Write** encodes into a buffer whose header is built once, applies the
  backlog check (§7.2), and hands the frame to the driver.
- **Disconnect** sends three black frames (§7.6) and closes the port off
  the async workers.
- **Cadence** `OutputCadence::from_min_interval(frame_interval,
  effective_max_fps).with_max_frame_silence(1 s)`.
- **Lifecycle** the connect timeout is the default plus `startup_delay_ms`.

---

## 10. Safety

- The driver never enumerates serial ports and never claims a USB device by
  VID or PID. The only path it opens is one named in its config.
- It never sends a byte outside the configured framing: no probes, no
  statistics requests, no reset sequences, no DTR or RTS changes.
- Exclusive opens keep two hosts from interleaving frames on one port.
- No real serial device is touched by any test (§11).
- The WLED baud-switch behavior (§4.3) is documented for users; it is a
  recoverable firmware quirk, not a hazard to the host or hardware.

---

## 11. Testing Strategy

Everything runs without hardware.

| Test file | Coverage |
|---|---|
| `tests/protocol_tests.rs` | Golden bytes for each header variant, count encoding at 1, 2, 300, and 65,536 LEDs, the check byte, every trailer vector in §5.4, calibrated and RGBW frames, color order before the checksum, padding and truncation, bandwidth math |
| `tests/config_tests.rs` | TOML parsing and defaults, every validation rule in §8, line budget, latch, and user cap math |
| `tests/driver_tests.rs` | Descriptor, default config, discovery identity and metadata, stable ids across port changes, control value round trip, driver and device surfaces |
| `tests/backend_tests.rs` | Unix only. Pseudo-terminal pairs created by the test: Ada and AWA frames read back byte for byte (including `0A 0D 11 13` to prove raw mode), padding, three-frame blackout on disconnect, no blackout before the first frame, startup delay with a greeting, idempotent connect, a stalled port failing within its bound, a missing path inside a fresh temp directory, cadence and lifecycle policy |

The pty tests open only the slave path of a pair they just created. The
missing-port test uses a path inside an empty temporary directory.

---

## 12. Implementation Notes

- **Experimental.** Nobody has run this driver against real firmware yet.
  Once the `DriverStability` label lands (Spec 89), the descriptor gains
  `.experimental()` and the device database entries record
  `verification = "unverified"`.
- **Native USB boards.** The backlog check reads the driver's queue depth,
  so a CDC board that drains faster than its nominal baud is still paced by
  the configured rate (§7.1). Configure such boards with a realistic rate.
- **Firmware greeting.** Logged only; it is not surfaced in device metadata.
- **Windows.** `COM` paths work through the same `tokio-serial` backend. The
  pty integration tests are Unix only; the protocol, config, and driver tests
  run everywhere.

---

## References

| Source | License | Used for |
|---|---|---|
| [adafruit/Adalight](https://github.com/adafruit/Adalight) `Arduino/LEDstream/LEDstream.pde`, `Processing/Adalight/Adalight.pde` | LGPL-3.0-or-later | Behavior only: `Ada` header, count minus one, check byte, hello, 15 s idle blanking, resync, 115200 default |
| [dmadison/Adalight-FastLED](https://github.com/dmadison/Adalight-FastLED) `LEDstream_FastLED.ino` | LGPL-3.0-or-later | Behavior only: 60 s timeout, input flush after each update, length mismatch handling |
| [awawa-dev/HyperSerialESP32](https://github.com/awawa-dev/HyperSerialESP32) `include/main.h`, `framestate.h`, `calibration.h`, `statistics.h` | MIT | AWA v1, v2, v3 headers, calibration order, trailer verification, 4,096 LED limit, statistics request, RGBW derivation, defaults |
| [awawa-dev/HyperSerialEsp8266](https://github.com/awawa-dev/HyperSerialEsp8266) `include/main.h` | MIT | Two-byte trailer verification, 5,120-byte ring |
| [awawa-dev/HyperSerialPico](https://github.com/awawa-dev/HyperSerialPico) | MIT | Same protocol over USB CDC |
| [hyperion-project/hyperion.ng](https://github.com/hyperion-project/hyperion.ng) `libsrc/leddevice/dev_serial/LedDeviceAdalight.cpp`, `ProviderRs232.cpp`, `schemas/schema-adalight.json` | MIT | Host framing for Ada, AWA, and LBAPA, trailer arithmetic, `delayAfterConnect`, refresh interval, power-off frames, baud list |
| [awawa-dev/HyperHDR](https://github.com/awawa-dev/HyperHDR) `DriverSerialAdalight.cpp`, `EspTools.h`, `schema-adalight.json` | MIT | `AWa` RGBW framing, calibration header rules, ESP reset handshake (not implemented), defaults |
| [wled/WLED](https://github.com/wled/WLED) `wled00/wled_serial.cpp` | EUPL-1.2-or-later | Behavior only: `Ada`-only serial input, baud-switch bytes, 2.5 s realtime timeout |
| [psieg/Lightpack](https://github.com/psieg/Lightpack) `LedDeviceAdalight.cpp` | GPL | Behavior only: same `Ada` header, host-side color order, write-only port use |
