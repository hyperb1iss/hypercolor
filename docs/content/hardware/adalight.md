+++
title = "Adalight serial controllers"
description = "Drive DIY LED strips behind Arduino, ESP8266, ESP32, and RP2040 boards running Adalight or HyperSerial (AWA) firmware. Devices are configured by hand: serial port, baud rate, protocol, and LED count."
weight = 35
+++

The Adalight driver streams to DIY LED strips wired to a microcontroller on a serial port. It speaks the two framings that ambient-lighting firmware uses:

- **Adalight** (`ada`): the original Adafruit sketch, Adalight-FastLED, and WLED's serial input. Usually 115200 baud.
- **AWA** (`awa`): HyperSerial firmware for ESP8266, ESP32, and RP2040, built for HyperHDR. Usually 2,000,000 baud, with a checksum on every frame and optional white-channel support for RGBW strips.

If your board already works with Hyperion.ng, HyperHDR, or Prismatik, the same port, baud rate, protocol, and LED count should work here.

{% <callout type="warning"> %}
This driver is **experimental**: it is built and tested against the published framing, but nobody has run it on a real board yet. Reports from real rigs are very welcome.
{% </callout> %}

These boards sit behind generic USB-serial chips (CH340, CP210x, FTDI, and stock Arduino or Espressif USB ids) that also show up in plenty of other hardware. Hypercolor therefore never guesses: it opens only the ports you list, never scans serial ports, and never claims a board by its USB id. Your board will still show up in `hypercolor devices unclaimed`, because the USB driver sees a chip nothing claims. That is expected.

## Prerequisites

- The board runs Adalight-compatible firmware, and you know its **LED count** and **baud rate**. Both are set when the firmware is built or flashed.
- No other program holds the port. Hypercolor opens it exclusively, so quit Prismatik, Hyperion, HyperHDR, or an Arduino serial monitor first.
- **Linux:** your user can open the port. Most distributions grant that through a group: `dialout` on Debian, Ubuntu, and Fedora, `uucp` on Arch.

  ```bash
  sudo usermod -aG dialout $USER   # or uucp on Arch; log out and back in
  ```

## Find the port

| OS | Where to look | Example |
|---|---|---|
| Linux | `ls -l /dev/serial/by-id/` | `/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0` |
| macOS | `ls /dev/cu.*` | `/dev/cu.usbserial-1420` |
| Windows | Device Manager, **Ports (COM & LPT)** | `COM4` |

On Linux, prefer the `/dev/serial/by-id/` path over `/dev/ttyUSB0`: it stays the same when you plug boards in a different order. On macOS, use the `cu.` device rather than `tty.`.

## Add a board

The driver is enabled by default and does nothing until you list a device. Add an entry to `hypercolor.toml` (`hypercolor config path` prints its location):

```toml
[[drivers.adalight.devices]]
id = "desk"                 # stable identity, letters/digits/-/_
name = "Desk Strip"
port = "/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0"
protocol = "ada"            # ada | awa
led_count = 120
```

Then run a discovery pass, or restart the daemon:

```bash
hypercolor devices discover --target adalight
hypercolor devices list
```

The strip appears like any other device, ready for the layout editor. Its identity comes from `id`, so renaming it or moving it to another port keeps its place in your layouts.

## Examples

### Arduino Uno or Nano with the Adalight sketch

Classic Arduinos reset when a program opens their port and need a second or two to boot. Give them a startup delay so the first frames are not lost:

```toml
[[drivers.adalight.devices]]
id = "monitor"
port = "/dev/serial/by-id/usb-1a86_USB2.0-Serial-if00-port0"
protocol = "ada"
baud_rate = 115200
led_count = 60
startup_delay_ms = 2000
```

With a startup delay set, the daemon log shows what the firmware printed while it booted. The Adalight sketches print `Ada`, which confirms the baud rate is right.

### ESP32 with HyperSerial

```toml
[[drivers.adalight.devices]]
id = "tv"
name = "TV Backlight"
port = "/dev/serial/by-id/usb-Silicon_Labs_CP2102_USB_to_UART_Bridge_Controller_0001-if00-port0"
protocol = "awa"            # baud_rate defaults to 2000000 for awa
led_count = 250
```

### RGBW strip on HyperSerial

HyperSerial builds compiled for RGBW strips can derive the white channel themselves from a calibration Hypercolor sends with every frame, or take a white value Hypercolor computes:

```toml
[[drivers.adalight.devices]]
id = "shelf"
port = "COM5"
protocol = "awa"
led_count = 144
white = "firmware"          # firmware derives white from the calibration
white_calibration = { gain = 255, red = 160, green = 160, blue = 160 }
```

Use `white = "host"` instead to have Hypercolor compute white (the shared minimum of red, green, and blue) and send four bytes per LED. That mode needs a HyperSerial build compiled for RGBW. The Adalight sketches read three bytes per LED, so both white modes require `protocol = "awa"`.

## Settings

| Key | Default | Description |
|---|---|---|
| `id` | required | Stable identity: 1-64 letters, digits, `-`, `_` |
| `name` | the id | Display name |
| `port` | required | Serial port path or `COMn` name |
| `baud_rate` | 115200 (`ada`), 2000000 (`awa`) | Line rate; must match the firmware |
| `protocol` | `ada` | `ada` or `awa` |
| `led_count` | required | LEDs the firmware drives: up to 65,536 for `ada`, 4,096 for `awa` |
| `color_order` | `rgb` | `rgb`, `rbg`, `grb`, `gbr`, `brg`, or `bgr`. The firmware usually reorders for the strip itself, so change this only if red shows the wrong color |
| `white` | `off` | `off`, `firmware`, or `host` (AWA only) |
| `white_calibration` | 255, 160, 160, 160 | `gain`, `red`, `green`, `blue`, each 0-255; used with `white = "firmware"` |
| `startup_delay_ms` | 0 | Wait after opening the port before the first frame, up to 10,000 |
| `latch_ms` | 0 | Idle gap after each frame, up to 1,000 (see below) |
| `max_fps` | 0 | Frame rate cap; 0 follows the serial line and the render loop |

## Frame rate and serial bandwidth

A serial line moves one byte per 10 bit times, so the baud rate and the strip length set a hard ceiling on the frame rate. Each frame is a 6-byte header plus 3 bytes per LED (AWA adds 3 checksum bytes):

| Strip | Protocol | Baud | Frames per second |
|---|---|---|---|
| 60 LEDs | ada | 115200 | 61 |
| 100 LEDs | ada | 115200 | 37 |
| 300 LEDs | ada | 115200 | 12 |
| 300 LEDs | awa | 2000000 | 220 |

Hypercolor computes this limit for every board and paces output to it. When the render loop runs faster than the line, each frame replaces the one still waiting, so the strip always shows the newest frame and nothing piles up behind the serial port. The rest of your rig keeps its full frame rate.

To see the computed limit for a board:

```bash
hypercolor devices controls <device>
```

The **Serial Line Max FPS** row is what the line can carry, and **Output FPS Cap** is what the board is driven at after `max_fps`. The daemon also logs a warning at connect when a board's line is slower than the render loop. Raising `baud_rate` (and reflashing the firmware to match) is the only way to lift the limit.

Boards with native USB, like the RP2040, ESP32-S2, ESP32-S3, and Arduino Leonardo, ignore the baud rate and run as fast as their USB link allows. Hypercolor still computes the limit from the baud you configure, so set a rate that reflects the link, such as 2000000 or higher.

### Flicker or dropped frames on AVR boards

Adalight-FastLED throws away serial data that arrives while it is updating the strip. If the next frame arrives during that update, it is lost. Give the board an idle gap after each frame: WS2812 strips need about 30 µs per LED, so `latch_ms = 4` suits around 100 LEDs and `latch_ms = 10` suits 300.

## Troubleshooting

**Permission denied.** Your user is not in the group that owns the port. See [Prerequisites](#prerequisites), then log out and back in.

**Port busy, or the device keeps failing to connect.** Another program has the port open. Close it and run `hypercolor devices discover --target adalight`. A board that is unplugged gets a few reconnect attempts with backoff; if it was gone longer than that, plug it in and run the same discovery command.

**Nothing lights up.** Check that `protocol` and `baud_rate` match the firmware: HyperSerial ignores `ada` frames and the Adalight sketches ignore `awa` frames. Setting `startup_delay_ms` makes the daemon log the firmware's greeting, which shows quickly whether the baud rate is right.

**Only part of the strip lights, or the pattern is offset.** `led_count` must match what the firmware was built for.

**Colors are swapped.** Change `color_order`. Try `grb` first; it is the native order of WS2812 strips.

**The strip goes dark when nothing changes.** It should not: Hypercolor resends the last frame every second so the firmware's idle timeout never fires. If it still blanks, check the daemon log for write errors.

**WLED over serial.** WLED accepts Adalight frames on its serial port, but some byte values are commands to it while it waits for a frame, and one of them changes its baud rate. If a byte gets lost, WLED can end up on the wrong baud until it restarts. Use the [WLED driver](@/hardware/wled.md) over the network instead when you can.
