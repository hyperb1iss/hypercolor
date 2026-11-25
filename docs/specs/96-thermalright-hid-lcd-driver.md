# 96 -- Thermalright HID LCD Driver

> Native support for the Thermalright cooler LCDs that enumerate as
> `0x0416:0x5302` (product string `USBDISPLAY`): the Frozen Warframe 240,
> 360, and SE and their 320x240 siblings. The panel identifies its model in
> a handshake reply, then takes RGB565 frames as a stream of 512-byte HID
> output reports, turned a quarter clockwise. Units on firmware 4.07 are
> left unclaimed (section 7). Requested in GitHub issue #599.

**Status:** Implemented, awaiting hardware confirmation
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::thermalright::{hid_lcd, devices}`
**Author:** Nova
**Date:** 2026-11-24

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Wire Format](#3-wire-format)
4. [Command Vocabulary](#4-command-vocabulary)
5. [Session Model](#5-session-model)
6. [Topology and Orientation](#6-topology-and-orientation)
7. [The Firmware 4.07 Variant](#7-the-firmware-407-variant)
8. [HAL Integration](#8-hal-integration)
9. [Open Questions and Risks](#9-open-questions-and-risks)
10. [Testing Strategy](#10-testing-strategy)
11. [Field Test Plan](#11-field-test-plan)

[References](#references)

---

## 1. Overview

Thermalright ships one USB identity across several coolers whose screens
differ in size and pixel format. Issue #599 reported a Frozen Warframe 360
on Windows 11 with `0416:5302` unclaimed: interface 0 bound to `HidUsb`,
interface 1 to WinUSB under the name `USBDISPLAY`.

The controller is a Winbond (Nuvoton) part, so the vendor ID is Winbond's,
the same one several Lian Li controllers report. The protocol is
Thermalright's: their Windows app, TRCC, drives this ID from its
`ThreadSendDeviceDataH` worker.

The driver was derived from these sources. No code was copied; each was read
for byte layouts and timing.

| Source | Contribution | Hardware evidence |
|--------|--------------|-------------------|
| [Lexonight1/thermalright-trcc-linux](https://github.com/Lexonight1/thermalright-trcc-linux) (Python, GPL-3.0-or-later) at `41a79a7` | Handshake, reply validation, frame header, chunking, the PM table, the rotation, the keepalive cadence, and the firmware 4.07 quirks, all transcribed from a decompile of TRCC | Users report Frozen Warframe 240, 360, and SE panels working; its changelog records each byte-order and rotation fix against a reporter's glass |
| [rejeb/linux-thermalright-lcd-control](https://github.com/rejeb/linux-thermalright-lcd-control) (Python, Apache-2.0) at `e249ed9` | An independent handshake parser and an RGB565 encoder that turns the frame the same way | Ships `0416:5302` support; its frame header differs (section 3.4) |
| Issue #599 diagnostics bundle | The Windows driver bindings for both interfaces | The reporter's unit |

No USB capture was available. Section 11 lists what the first field test
needs to report.

## 2. Device Registry

### 2.1 USB identity map

| Interface | Class | Windows driver | Endpoints | Role |
|-----------|-------|----------------|-----------|------|
| 0 | HID (`0x03`) | `HidUsb` | interrupt OUT `0x02`, IN `0x81`, 512-byte reports | Everything this driver sends |
| 1 | Vendor (`0xFF`) | WinUSB, named `USBDISPLAY` | not documented | Nothing drives it |

The composite parent is `usbccgp`. Endpoints come from trcc's
`PROTOCOL_USBLCDNEW.md` (lines 41 and 153 to 160) and `hid_lcd.py` (line
138, plus `_EP_READ` in `_base.py` line 334).

### 2.2 Panel model matrix

The handshake reply carries a panel model byte (PM) and a sub-model byte
(SUB). trcc maps PM to a frame-buffer code (FBL) through
`_PM_TO_FBL_OVERRIDES` (`core/protocol.py` lines 420 to 446) and then to a
profile (`FBL_PROFILES`, lines 145 to 203). For every PM below, FBL equals
PM.

| PM | Geometry | Pixel bytes | Driven here | Source line |
|----|----------|-------------|-------------|-------------|
| 49 | 320x240 | RGB565, high byte first | Yes | `protocol.py` 156; trcc changelog lines 1124 to 1128 (from the Windows app, not yet seen on glass) |
| 50 | 320x240 | RGB565, low byte first | Yes | 157 |
| 51 | 320x240 | RGB565, low byte first | Yes | 158 (Frozen Warframe 240) |
| 52 | 320x240 | RGB565, low byte first | Yes | 159 (BA120 Vision) |
| 53 | 320x240 | RGB565, low byte first | Yes | 160 |
| 58 | 320x240 | RGB565, low byte first | Yes | 162 (Frozen Warframe SE) |
| 36, 37 | 240x240 | RGB565, unturned | No | 147 and 148 (Assassin Spirit 120 Vision) |
| 128 | 1280x480 | JPEG | No | 198 (Trofeo Vision) |

The low-byte-first order for 51 and 53 is not a guess: trcc first shipped
them big-endian and inverted every color (changelog lines 4942 to 4946).

Any other model fails the session (section 5.1). Supporting the square and
JPEG panels needs their own geometry and, for 128, a JPEG encode path; both
are listed in `data/drivers/vendors/thermalright.toml` as `researched`.

### 2.3 The shared vendor ID

`0x0416` is Winbond's. Hypercolor's other claims on it are Lian Li's TL Fan
Hub (`0x7372`), wireless TX and RX (`0x8040`, `0x8041`), and the Universal
Screen's LED ring (`0x8050`, not registered). `0x5302` collides with none of
them. The udev rule names the product and interface, like the Lian Li ones.

There is one descriptor and no firmware predicate.

## 3. Wire Format

All multi-byte fields are little-endian.

### 3.1 Handshake (512 bytes)

| Offset | Size | Value | Meaning |
|--------|------|-------|---------|
| 0 | 4 | `DA DB DC DD` | Magic |
| 4 | 8 | zero | |
| 12 | 4 | `01 00 00 00` | Command: device info |
| 16 | 496 | zero | |

Sources: `hid_lcd.py` lines 385 to 394; `PROTOCOL_USBLCDNEW.md` lines 163
to 172.

### 3.2 Handshake reply

| Offset | Size | Meaning |
|--------|------|---------|
| 0 | 4 | Magic echo, `DA DB DC DD` |
| 4 | 1 | SUB |
| 5 | 1 | PM |
| 12 | 1 | `0x01`, the command echo |
| 16 | 1 | `0x10` when a serial follows |
| 20 | 16 | Serial |

A reply is the **standard shape** when it is at least 20 bytes, carries the
magic, and has `0x01` at byte 12 (`hid_lcd.py` lines 396 to 409 and 411 to
444; `PROTOCOL_USBLCDNEW.md` line 456).

Every other reply fails the handshake as malformed. One of them gets its
own log line: the **short shape**, the eight bytes `DA DB DC DD 00 3A 00 00`
(SUB 0, PM 58) that a firmware 4.07 streaming unit volunteers (`hid_lcd.py`
lines 398 to 402). Linux hidraw hands the eight bytes over as they arrive;
Windows `HidUsb` returns every input report whole, so there it arrives as a
512-byte report that is zero past byte 6. The driver recognizes either form
and says it met the 4.07 streaming variant, which it does not drive. Such a
unit should never get this far (section 7); the line exists for a 4.07
panel whose descriptor reports some other release.

### 3.3 Frame header (20 bytes)

| Offset | Size | Value | Meaning |
|--------|------|-------|---------|
| 0 | 4 | `DA DB DC DD` | Magic |
| 4 | 2 | `02 00` | Frame type: picture |
| 6 | 2 | `01 00` | Encoding: RGB565 (`00 00` is JPEG) |
| 8 | 2 | `F0 00` | Width, 240 |
| 10 | 2 | `40 01` | Height, 320 |
| 12 | 4 | `02 00 00 00` | Command: picture |
| 16 | 4 | `00 58 02 00` | Payload length, 153,600 |

Source: `hid_lcd.py` lines 446 to 477, and the length-source column of
`PROTOCOL_USBLCDNEW.md` line 466. The width and height are TRCC's hardcoded
240x320 for every RGB565 frame, whichever way the panel is mounted. Byte 12
carries the command the way the handshake does (and the LED packets on the
same magic, `REFERENCE_TECHNICAL.md` lines 293 to 303), which is why the
driver names it so; trcc writes the four bytes as a constant.

### 3.4 Report stream

A frame is the header followed by the 153,600-byte framebuffer: 153,620
bytes, zero-padded to the next multiple of 512 (154,112), and sent as 301
output reports of 512 bytes. The first report holds the header and the
first 492 pixel bytes; the last holds the final 20 pixel bytes and 492 zero
bytes.

Each report is one HID output report with report ID `0x00`; the host HID
stack takes the ID byte in front and strips it from the wire. trcc found
that the firmware latches a frame only when it arrives as 512-byte reports:
one bulk write of the whole frame transfers cleanly and leaves the panel on
its logo (`hid_lcd.py` lines 348 to 356).

rejeb's driver builds a 22-byte header with 320 and 240 in the size fields
(`hid_devices.py` lines 59 to 72). This driver follows trcc, whose header
matches TRCC's decompile and whose users report these panels working.

### 3.5 Pixels

RGB565: red in bits 15 to 11, green 10 to 5, blue 4 to 0. Models 50 to 53
and 58 send each pixel low byte first, model 49 high byte first.

The raster is 240 pixels wide and 320 tall, row-major. On every
standard-shape panel the composed 320x240 image is turned 90 degrees
clockwise into it: raster pixel (x, y) is source pixel (y, 239 - x). Raster
line `n` is source column `n` read from the bottom row up.

trcc reaches this through its encode table: 320x240 RGB565 panels take the
default arm, base 90 (`protocol.py` lines 384 to 387), applied at user
orientation 0 by `wire_angle` (lines 755 to 797) through Qt's
`QTransform().rotate(90)` (`adapters/render/qt.py` lines 209 to 214), which
turns clockwise in Qt's y-down image space. rejeb's encoder flips rows and
transposes (`image_encoder.py` lines 26 to 34), the same turn written
another way. It also zeroes the last pixel of every output line, which
trcc does not; this driver does not either.

## 4. Command Vocabulary

TRCC's command constants (`PROTOCOL_USBLCDNEW.md` lines 46 to 58):

| Code | Name | Sent here |
|------|------|-----------|
| 1 | Device info | Yes, the handshake |
| 2 | Picture | Yes, every frame |
| 3 | Logo | No |
| 4 | Firmware update (OTA) | **Never** |
| 5 | Upgrade state | No |
| 6 | Rotate | No |
| 7 | Screen settings | No |
| 8 | Backlight | No |
| 9 | Logo state | No |

`HidLcdCommand` has two variants, device info and picture. Nothing else has
a representation in the driver, so nothing else can be encoded, and a test
walks every command a session emits to hold that line for code 4 in
particular. Backlight and rotate exist on the firmware, but trcc applies
both in software and never sends them; this driver does the same, through
the daemon's software brightness and mounting rotation.

## 5. Session Model

### 5.1 Init

One command: the handshake report, then a 200 ms pause, then one read with a
5-second timeout (`_base.py` lines 64 and 98 to 99, `_init_exchange` lines
357 to 366).

The reply's model picks the profile (section 2.2). A model outside the table
fails the command with `DeviceError(Unsupported)` after a warning naming PM
and SUB, so the session ends and nothing is drawn. A malformed reply fails
it with `MalformedResponse`. The backend logs the reply bytes in both cases,
and the protocol logs the raw reply at debug before it parses anything.

Two pieces of trcc's exchange are not reproduced:

- **The 50 ms pause before the handshake write.** TRCC waits after claiming
  the interface. Hypercolor's HID transports claim nothing (the host HID
  driver stays bound), and the protocol trait has no pre-command delay.
- **Three attempts, 0.5 s apart** (`_handshake_retry`, lines 368 to 405). A
  failed handshake fails the connect instead, and the device lifecycle's
  reconnect is the retry.

### 5.2 Frames

The daemon delivers raw RGB888 at the surface geometry (section 6). The
driver packs it into the stream of section 3.4 and emits 301 unacknowledged
output reports, with a 1 ms settle after the last one. trcc sleeps that 1 ms
once per frame, after the loop, not between reports (`hid_lcd.py` lines 59
and 357 to 371).

A payload in any other format is refused (`DisplayEncodeError::Unsupported`),
as is one at the wrong geometry (`WrongGeometry`) or with too few bytes
(`Repack`). Nothing is ever padded or truncated onto the wire.

### 5.3 Keepalive

The panel returns to its logo a few seconds after frames stop. trcc resends
the last frame after one second without one for this family; TRCC itself
resends every family about every 150 ms (`core/ports.py` lines 367 to 391).
The daemon only reasserts a static frame every 20 seconds
(`DEFAULT_STATIC_HOLD_REFRESH_INTERVAL`).

So the protocol keeps the last frame's stream and declares a frame pump
(`frame_pump_interval`, 250 ms). On each tick `pump_frame_into` resends the
stream if a second has passed since the last frame or resend
(`WireKeepalive`). Pumped commands are frame traffic: a transient write
error is logged and the lane carries on, where a failed keepalive would end
the session. While frames flow the pump never fires, and the single-lane USB
actor runs a pending display frame ahead of a pump tick.

A frame that fails to encode clears the stored stream, so a bad frame is
never resent. A new session's init clears it too.

### 5.4 Timing

| Parameter | Value | Source |
|-----------|-------|--------|
| Handshake settle before the read | 200 ms | `_base.py` line 99 |
| Handshake read timeout | 5 s | `_base.py` line 64 |
| Report pacing | none | `hid_lcd.py` lines 357 to 369 |
| Settle after a frame | 1 ms | `hid_lcd.py` lines 59 and 370 |
| Keepalive silence | 1 s | `core/ports.py` lines 381 to 391 |
| Frame pump tick | 250 ms | This driver; bounds a resend at about 1.25 s of silence |
| Frame interval | 33 ms (`max_fps` 30) | Baseline, matching the daemon's raw-display cap. Raise or lower on measurement. |

### 5.5 Shutdown

Nothing is sent. The panel falls back to its own logo once the frames stop,
which is the hand-back TRCC relies on too.

### 5.6 Brightness and rotation

`supports_brightness` is false; the daemon scales pixels in software. No
rotate command is sent; the quarter turn of section 3.5 is the only one the
driver applies, and the user's mounting rotation turns the composed image
before it.

## 6. Topology and Orientation

The device exposes one display segment named `Display`, 320x240, not
circular, in `DisplayFrameFormat::Rgb`, with no LEDs. Every model the
driver accepts shares that geometry, so the segment never changes after the
handshake; only the pixel byte order does.

At mounting rotation "Upright" the image should read upright on a Frozen
Warframe, because the driver turns it exactly as TRCC does at orientation 0.
Thermalright mounts the same glass differently across coolers, which is
what the daemon's mounting rotation is for (section 11 asks the reporter to
check).

## 7. The Firmware 4.07 Variant

Some units on this ID run firmware with `bcdDevice` 4.07. trcc records,
from a reporter's own driver (`core/models.py` lines 535 to 544, and
`hid_lcd.py` lines 252 to 307):

- The handshake reboots the panel, so trcc skips it, waits 3 seconds, and
  reads a reply the panel volunteers on its own.
- The reply is the eight-byte short shape.
- The raster is upright portrait: the firmware turns the image, so the frame
  must not be pre-rotated.
- Reopening the device wedges it until a replug, so trcc keeps one session
  for the life of the process.

The release is not the panel. Several different coolers ship firmware 4.07,
and the ones that answer the ordinary handshake (a 1280x480 Trofeo Vision, a
PM 52 panel) behave like any other unit (`hid_lcd.py` lines 264 to 273 and
209 to 218; trcc changelog `v9.9.6`, lines 1945 to 1955). Only the reply a
streaming unit volunteers identifies it.

**This driver claims no unit on release 4.07.** The descriptor's release
predicate (`release != 0x0407`, `THERMALRIGHT_STREAMING_FIRMWARE_RELEASE`)
keeps the lookup from matching, so the device lands in the unclaimed
hardware list like any unsupported one, and Hypercolor never writes to it.
The alternative was worse: every connect would send the handshake, reboot a
streaming panel, fail, and reconnect into another reboot, and each reopen
risks the wedge. Experimental descriptors claim devices automatically, so
every owner of such a panel who upgraded would hit that loop. Panels on 4.07
that would have answered the ordinary handshake are refused too; that is the
price of not being able to tell them apart without risking the reboot.

The release reaches the lookup from nusb's `device_version`: the scanner and
the hotplug watcher pass it, and the scanner stores it as `bcd_device`
metadata so the backend's adoption makes the same match. A descriptor with
a release predicate never matches a device whose release is unknown.

Driving these units faithfully needs three things the HAL does not have:

- **A read-before-write init step.** Settle 3 seconds after open, then read
  for up to 5 seconds without sending anything. A volunteered short reply
  identifies the streaming variant; silence means some other 4.07 panel,
  which then gets the ordinary handshake.
- **A portrait-native profile:** a 240x320 surface sent unturned, chosen by
  that reply.
- **Single-session lifecycle.** The session must survive sleep, write
  errors, and the daemon's reconnect policy without ever closing and
  reopening the device, short of the device leaving the bus.

None of it is worth building before a 4.07 owner can test it.

## 8. HAL Integration

### 8.1 Files

| File | Role |
|------|------|
| `drivers/thermalright/hid_lcd.rs` | `HidLcdProtocol`, the handshake and frame-header structs with size assertions, `parse_handshake_reply`, `model_pixel_format`, the report layout |
| `drivers/thermalright/devices.rs` | The descriptor, its release predicate, and `build_hid_lcd_protocol` |
| `drivers/thermalright/mod.rs` | Module docs and re-exports |
| `display/repack.rs` | `QuarterTurn` on `LineRepack`, `Packed16Format::Rgb565BigEndian`, and `repack_rgb888_into` for a framebuffer inside a larger buffer |
| `database.rs` | Registration, and the release argument on the lookups |
| `registry.rs` | `DeviceDescriptor::release_predicate` and `accepts_release` |
| `crates/hypercolor-core/src/device/{usb_scanner,usb_hotplug,usb_backend}.rs` | Pass `bcdDevice` to the lookup; the scanner records it as `bcd_device` metadata for adoption |
| `udev/99-hypercolor.rules` | `hidraw` grant on `0416:5302` interface 0 |
| `data/drivers/vendors/thermalright.toml` | One `supported` row (unverified) and two `researched` rows for the other panels on the ID |
| `crates/hypercolor-core/src/device/conflicts/catalog.toml` | TRCC (`TRCC.exe`, the TRCC Linux entry points) and linux-thermalright-lcd-control |

The shared repack stage gained the turn rather than the driver carrying its
own loop: a turned framebuffer is a line layout like Push 2's, with lines
built from source columns.

### 8.2 Descriptor

```rust
DeviceDescriptor {
    vendor_id: 0x0416,
    product_id: 0x5302,
    name: "Thermalright LCD",
    family: DeviceFamily::new_static("thermalright", "Thermalright"),
    transport: resolve_current_transport(TransportIntent::Hid(HidTransportIntent {
        access: HidAccessMode::HostManaged,
        interface: 0,
        report_id: 0x00,
        report_mode: HidRawReportMode::OutputReportWithReportId,
        max_report_len: 513,
        usage_page: None,
        usage: None,
    }))
    .expect("..."),
    protocol: ProtocolBinding {
        id: "thermalright/hid-lcd",
        build: build_hid_lcd_protocol,
    },
    firmware_predicate: None,
    release_predicate: Some(drives_release), // release != 0x0407
    serial_quirk: None,
    stability: DriverStability::Experimental,
}
```

The driver ID is `thermalright`, a new HAL family enabled by default like
every experimental family (spec 89). Every packet the protocol emits is 513
bytes, the report ID first, which is what `OutputReportWithReportId` takes on
both backends.

### 8.3 Platforms

- **Windows:** `HostManaged` resolves to `UsbHidApi`. The host HID driver
  stays bound, which is what the reporter's unit already has. hidapi strips
  the zero report ID from input reports, so the reply starts at the magic.
- **Linux:** resolves to `UsbHidRaw`. The udev rule grants the hidraw node
  of interface 0 only.
- **macOS:** resolves to `UsbHidApi`, untested.

Spec 89's safety question holds: if the encoder is wrong, the panel shows
the wrong picture or its logo. Nothing here sends a command that persists
on the device, and code 4 has no representation at all.

## 9. Open Questions and Risks

1. **Unconfirmed on Hypercolor.** Every byte here comes from other
   implementations. The row stays `unverified` and the descriptor
   experimental until section 11 passes.
2. **Firmware 4.07 units are unclaimed** (section 7), including any that
   would have answered the ordinary handshake.
3. **Output report length on Windows.** `HidUsb` takes writes of exactly the
   descriptor's output report length. trcc on Windows reports 513 bytes
   written per 512-byte chunk (`hid_lcd.py` lines 360 to 365), which
   supports 512-byte reports, but the report descriptor itself is unseen.
4. **HID collection selection.** The descriptor matches interface 0 with no
   usage page. If interface 0 exposes more than one top-level collection,
   hidapi may open the wrong one.
5. **Throughput.** A frame is 301 writes, each a separate call through the
   transport. The link speed and the endpoint interval are unknown, so 30
   fps is a ceiling to check, not a measurement; `fps_sent` and
   `avg_write_ms` show the truth.
6. **Serial identity.** Whether the USB serial string is unique per unit is
   unknown. A fixed placeholder would collapse two panels into one device
   and needs a `serial_quirk`.
7. **Reply length on Linux.** The hidraw transport reads at most 91 bytes,
   so a 512-byte reply is truncated there. Everything the driver reads sits
   in the first 36 bytes.
8. **Firmware revisions besides 4.07.** Nothing upstream says another
   release behaves like the streaming firmware. If one does, the handshake
   times out with no reply in the log, and its release joins the predicate.
9. **The skipped pre-handshake pause** (section 5.1). If the first
   handshake after plug-in times out while a later one succeeds, that pause
   is the first suspect.
10. **Keepalive and slow links.** If one frame takes longer than a second to
    write, a pump tick can resend the old frame between two new ones when no
    new frame is pending. The actor serves a pending frame first, so this
    only costs throughput on a link already too slow for the cadence.

## 10. Testing Strategy

`crates/hypercolor-hal/tests/thermalright_hid_lcd_tests.rs` covers, with no
hardware:

- The handshake: one 513-byte command, the report ID, the magic, `01 00 00
  00` at report byte 12, zeros elsewhere, the 200 ms settle, the 5-second
  read, and an empty shutdown.
- Reply parsing: the standard shape with and without a serial, bad magic,
  too short, a full report without the echo, and the 4.07 short reply, bare
  and zero-padded, refused with the message naming the variant and model.
- The model table: 50 to 53 and 58 little-endian, 49 big-endian, and a set
  of other models refused. An unknown model fails `parse_response` and
  leaves the panel unidentified.
- Topology: one 320x240 RGB segment, and a new session forgetting both the
  panel and the stored frame.
- Frames: 301 reports of 513 bytes, report ID zero, fire-and-forget, the
  1 ms settle on the last report only, the 20 header bytes exactly, and the
  last report's 20 pixel bytes followed by 492 zeros.
- Orientation: three corners in three colors land where a clockwise turn
  puts them, with nothing else lit, so any other turn or a mirror fails; an
  interior pixel lands at (239 - y, x); model 49 packs high byte first.
- Command buffer reuse across frames with no stale bytes, and refusals for
  wrong geometry, JPEG, and short payloads.
- The keepalive: nothing to pump before a frame or right after one, the full
  frame resent after a second of silence, the clock restarting on a resend,
  and no resend after a frame that failed to encode.
- The vocabulary: every command from three sessions (models 58, 49, and
  51), including resends, carries only device info or picture at the
  command positions, and never 4. No brightness command exists.
- Registration: one experimental descriptor on interface 0 with the
  resolved transport for the build platform, matched at releases such as
  1.00 and 4.08, never at 4.07 (even with the driver explicitly enabled),
  and never when the release is unknown.

`database_tests.rs` covers the release predicate itself: firmware routing on
the Lian Li AL is unchanged at any release, the Lian Li TL Fan Hub on the
same vendor ID resolves at every release, and a predicate matches only a
known release it accepts. `usb_backend/tests.rs` checks that adoption
rebuilds the same match from the scanner's `bcd_device` metadata.

`display_layer_tests.rs` covers the shared additions: big-endian wire bytes,
a clockwise turn on a 3x2 frame with line filler, and the exact-length rule
of `repack_rgb888_into`. `udev_rules_tests.rs` holds the new grant to
interface 0, and `software_conflict_tests.rs` the TRCC spellings.

## 11. Field Test Plan

These are the asks for the issue #599 reporter, on Windows 11.

**Step 1: identity and competing software.** Quit TRCC from its tray icon,
then run in PowerShell and paste the output:

```powershell
Get-Process | Where-Object { $_.ProcessName -match 'trcc' } |
  Format-Table ProcessName, Id, Path -AutoSize
Get-PnpDevice -PresentOnly |
  Where-Object { $_.InstanceId -match 'VID_0416&PID_5302' } |
  Format-List FriendlyName, InstanceId, Class, Status
Get-PnpDevice -PresentOnly |
  Where-Object { $_.InstanceId -match 'VID_0416&PID_5302' } |
  Get-PnpDeviceProperty -KeyName DEVPKEY_Device_HardwareIds |
  Format-List InstanceId, Data
```

The first command should print nothing. The hardware IDs carry
`REV_xxxx`, the firmware revision. On `REV_0407` Hypercolor leaves the panel
alone by design (section 7): it shows up in the unclaimed hardware list, and
the rest of this plan does not apply. Report the revision either way.

**Step 2: patched build with debug logging.** Quit Hypercolor from its tray
icon so only this daemon runs, then start it from PowerShell:

```powershell
$env:RUST_LOG = "warn,hypercolor_hal=debug,hypercolor_core=debug,hypercolor_daemon=info"
& "<install dir>\hypercolor-daemon.exe" 2>&1 | Tee-Object -FilePath "$HOME\Desktop\hypercolor-5302.log"
```

Then report:

- The log lines mentioning `Thermalright LCD`: the `handshake reply` debug
  line (its hex is the raw reply) and the `identified` line with `pm`,
  `sub`, `shape`, `format`, and `raster`. If the panel is refused, the
  warning names the model instead, and that is the most useful line of all.
- Whether the panel leaves the Thermalright logo once an effect runs, and
  whether the colors are right: a red effect should be red, not blue or
  speckled. Wrong colors point at byte order.
- **Rotation check.** Show something with an obvious top and a readable
  word, such as a clock or text display face. Say whether it reads upright
  and whether text reads forwards. If it is turned, set the panel's
  mounting rotation in its device settings to 90° clockwise, upside down,
  and 90° counter-clockwise in turn and say which reads upright. Text that
  reads backwards at every setting means the turn is mirrored, which no
  setting can fix and the driver must.
- **Static hold.** Leave a still image up for a minute. The panel should
  keep it rather than dropping back to the logo after a few seconds.
- The output of `hypercolor diagnose --system -j` after a minute of a moving
  effect. The panel's entry under `device_output` carries `fps_sent`,
  `avg_write_ms`, `frames_dropped`, and `last_error`.

**Step 3, only if the panel stays on its logo: capture TRCC.** Install
Wireshark with the USBPcap component and reboot. With Hypercolor stopped,
start a capture on the USBPcap root hub the cooler sits behind, then open
TRCC so the capture includes its handshake. In TRCC, show a solid red image,
wait five seconds, switch to solid green, wait five seconds, then stop the
capture and attach the zipped `.pcapng` with this transcript:

```powershell
$tshark = "C:\Program Files\Wireshark\tshark.exe"
$addr = & $tshark -r trcc-5302.pcapng -Y "usb.idVendor == 0x0416 && usb.idProduct == 0x5302" `
  -T fields -e usb.device_address | Select-Object -First 1
& $tshark -r trcc-5302.pcapng -Y "usb.device_address == $addr && usb.capdata" `
  -T fields -e frame.number -e frame.time_relative -e usb.endpoint_address `
  -e usb.data_len -e usb.capdata > trcc-5302-transcript.tsv
```

The two images differ only in pixel bytes, so the capture pins TRCC's
handshake, header, report size, and pacing against sections 3 and 5.

## References

- Hypercolor issue #599 and its diagnostics bundle (`platform-probe.txt`,
  USB driver bindings)
- Lexonight1/thermalright-trcc-linux at `41a79a7`:
  - `src/trcc/adapters/device/hid_lcd.py`: wire constants (lines 45 to 68),
    the streaming-firmware connect (252 to 307), `_portrait_native` (320 to
    332), `_write_frame` (344 to 381), the handshake packet (385 to 394),
    reply validation (396 to 409), reply parsing (411 to 444), the frame
    header (446 to 477)
  - `src/trcc/adapters/device/_base.py`: handshake timeout and retries
    (lines 64 to 65, 94), settle delays (98 to 99), `_init_exchange` (357 to
    366), `_handshake_retry` (368 to 405)
  - `src/trcc/core/protocol.py`: `FBL_PROFILES` (lines 145 to 203),
    `ENCODE_ROTATIONS` and the default arm (314 to 387), `_PM_TO_FBL_OVERRIDES`
    (420 to 446), `pm_to_fbl` (541 to 554), `resolve_encode_angle` (683 to
    703), `wire_angle` (755 to 797)
  - `src/trcc/core/models.py`: `DeviceQuirks` (lines 499 to 528), the
    4.07 row (534 to 548), and `quirks_for` (551 to 557)
  - `src/trcc/core/ports.py`: `needs_keepalive` and `keepalive_interval`
    (lines 367 to 391), `build_frame` (2336 to 2370), `encode_payload` (2398
    to 2423)
  - `src/trcc/adapters/render/qt.py`: `rotate` (lines 209 to 214)
  - `doc/PROTOCOL_USBLCDNEW.md`: the device table (lines 38 to 44), command
    constants (46 to 58), the `0416:5302` handshake (153 to 180), the
    comparison tables (440 to 470)
  - `doc/REFERENCE_TECHNICAL.md`: the LED packet layout on the same magic
    (lines 293 to 303)
  - `doc/CHANGELOG.md`: PM 49 (lines 1124 to 1128), the 4.07 entries in
    `v9.9.8`, `v9.9.6`, and `v9.9.3` (from line 1874), the HID byte order
    and rotation fix in `v5.0.8` (4940 to 4946), the header fix in `v5.0.4`
    (4970 to 4975)
- rejeb/linux-thermalright-lcd-control at `e249ed9`,
  `src/thermalright_lcd_control/device_controller/display/`:
  `hid_devices.py` (lines 51 to 72), `device_probe.py`
  (`parse_hid_t2_response`, lines 147 to 155), `image_encoder.py` (lines 26
  to 34)
- Spec 84: the Lian Li Universal Screen, the nearest display driver
- Spec 89: experimental drivers and device verification
