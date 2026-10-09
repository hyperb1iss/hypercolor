# 84 -- Lian Li Universal Screen 8.8" Driver

> Native support for the Lian Li Universal Screen 8.8" in LCD mode
> (`0x1CBE:0xA088`): a 1920x480 bar panel whose controller takes 480x1920
> portrait JPEG frames over USB bulk behind a DES-wrapped 512-byte header.
> The header is the "WinUSB" variant of the format spec 80 documents for the
> wireless LCD receivers. Requested in GitHub issue #361.

**Status:** Implemented, awaiting hardware confirmation
**Crate:** `hypercolor-hal`
**Module path:** `hypercolor_hal::drivers::lianli::{universal_screen, winusb}`
**Author:** Nova
**Date:** 2026-10-08

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [WinUSB Command Header](#3-winusb-command-header)
4. [Command Vocabulary and Replies](#4-command-vocabulary-and-replies)
5. [Session Model](#5-session-model)
6. [Topology and Orientation](#6-topology-and-orientation)
7. [HAL Integration](#7-hal-integration)
8. [Open Questions and Risks](#8-open-questions-and-risks)
9. [Testing Strategy](#9-testing-strategy)
10. [Hardware Validation Plan](#10-hardware-validation-plan)

[References](#references)

---

## 1. Overview

The Universal Screen 8.8" is a Lian Li accessory that puts three USB
functions in one enclosure: an 8.8-inch IPS bar (1920x480 as mounted), a
60-LED ring frame, and the hub that joins them. Issue #361 reported it on
Windows with the product string `8.8" Universal Screen-1.5`.

The panel controller is TURZX silicon. Lian Li's product IDs mirror TURZX's
own (`0xA088` against TURZX's 8.8" `0x0088`, `0xA092` against the 9.2"
`0x0092`), the DES key is the ASCII string `slv3tuzx`, and in desktop mode
the panel re-enumerates as a WCH display device that lian-li-linux logs as
the "TURZX streaming device".

The driver was derived from these sources. No code was copied; each was read
for protocol facts, and every byte position below is backed by at least two
of them.

| Source | Contribution | Hardware evidence |
|--------|--------------|-------------------|
| [sgtaziz/lian-li-linux](https://github.com/sgtaziz/lian-li-linux) (Rust, MIT) at `7c0de10` | The whole A088 path: header, init, frame, flow control | The maintainer owns a unit on firmware `lianli88_0001_0018`, and several users report it working |
| [elniko/lianli-python-lcd-overlay](https://github.com/elniko/lianli-python-lcd-overlay) ("Linx", Python) at `7bc6098` | Independent reverse engineering of L-Connect 3's `lianli.lcd207.dll`: the `CmdType` enum and the three USB identities | Written against a real unit |
| [mathoudebine/turing-smart-screen-python](https://github.com/mathoudebine/turing-smart-screen-python) (GPL-3.0) at `2b33ab4` | The same header and trailer on TURZX-branded `0x1CBE` panels (`0x0028` to `0x0123`) | Shipping support for those panels |
| [hieubui2409/pc-screens](https://github.com/hieubui2409/pc-screens) at `ffbfae4` | A port of lian-li-linux; confirms endpoints and the desktop-mode IDs | Run on a real A088 under Linux |
| [hello-nexus/nexus-service](https://github.com/hello-nexus/nexus-service) at `bc2b8aa` | A C# implementation of the header | Its own docs say no unit was ever run against it; layout cross-check only |

No USB capture of L-Connect was available. Section 10 lists the captures
that would close the gaps in section 8.

## 2. Device Registry

### 2.1 USB identity map

| Function | VID:PID | Notes |
|----------|---------|-------|
| LCD, LCD mode | `1CBE:A088` | Device class `0xFF`. Interface 0 with bulk OUT `0x01` and bulk IN `0x81`, 512-byte max packet. Vendor string `LIANLI`; product `8.8" Universal Screen-1.0` or `-1.5`. The one serial on record is 16 hex digits (`611d797784da8705`); no shared-serial quirk is known. |
| LCD, desktop mode | `1A86:AD21` (also reported as `1A86:ACE1`) | WCH USB display sink, driven through EVDI on Linux by lian-li-linux. Out of scope. |
| LED ring | `0416:8050` | 60 LEDs in three groups of 20. A separate device, not driven here. |
| Internal hub | `1A86:8095` | Joins the three functions. |

Firmware strings observed in the wild: `lianli88_0001_0018` (lian-li-linux
issue 55, also the maintainer's unit) and `lianli88_0001_0023` (lian-li-linux
issue 241).

### 2.2 Variant matrix

| PID | Product | Wire geometry | Protocol |
|-----|---------|---------------|----------|
| `0xA088` | Universal Screen 8.8" | 480x1920 | `lianli/universal-screen` |

lian-li-linux drives these sibling panels with the same header. They are not
registered here: each has its own init variant or geometry, and none has a
Hypercolor tester yet.

| PID | Product | Geometry (lian-li-linux `screen.rs`) |
|-----|---------|--------------------------------------|
| `0xA092` | Vision 9.2" | 464x1920 |
| `0xA021`, `0xA034` | HydroShift II LCD Circle, Square | 480x480 |
| `0xA065` | Lancool 207 Digital | 720x1472 |
| `0xA068` | HydroShift II OLED Curve | 1080x2288, PNG frames |
| `0xA018`, `0xA019` | TL and SL Infinity Flex LCD | 400x400 |

### 2.3 The shared vendor ID

`0x1CBE` is the Luminary Micro (TI) vendor ID and also appears on unrelated
development boards. Hypercolor's other claims on it are the wireless LCD
receivers at `0x0005` and `0x0006` (spec 80 section 7). `0xA088` collides
with neither, and the udev rule is scoped to the product like theirs.

There is one descriptor and no firmware predicate.

## 3. WinUSB Command Header

Every command, frames included, opens with this 512-byte header.

### 3.1 Plaintext (500 bytes)

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | Command | section 4.1 | |
| 1 | 1 | Reserved | `0x00` | |
| 2 | 2 | Magic | `1A 6D` | |
| 4 | 4 | Timestamp | u32 little-endian | Milliseconds since the session began |
| 8 | 492 | Parameters | | Command-specific, zero-padded |

The timestamp must strictly increase across a session. When two commands
land in the same millisecond the second is sent as the previous value plus
one; the firmware is reported to drop repeats.

### 3.2 Encryption and wire layout

The 500-byte plaintext is encrypted with DES in CBC mode, PKCS#7 padding,
with the key and the IV both the ASCII bytes `slv3tuzx`
(`73 6C 76 33 74 75 7A 78`). PKCS#7 pads 500 bytes to 504 with four `0x04`
bytes, so the ciphertext is 504 bytes long.

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 504 | Ciphertext | |
| 504 | 6 | Reserved | `0x00` |
| 510 | 2 | Trailer | `A1 1A` |

Spec 80 section 7.1 covers the wireless receivers' variant, which encrypts
504 plaintext bytes into a full 512-byte ciphertext with no trailer. CBC
chains left to right, so the two variants agree on every block before the
padding block for the same plaintext prefix.

turing-smart-screen-python zero-pads the plaintext to 504 bytes instead of
applying PKCS#7, which suggests the firmware ignores the padding block's
contents. This driver follows lian-li-linux and Linx, which both use PKCS#7;
Linx read the vendor DLL directly.

Known answers from OpenSSL 3 (`openssl enc -des-cbc -K 736c763374757a78
-iv 736c763374757a78 -provider legacy -provider default`):

| Header | Ciphertext bytes | Value |
|--------|------------------|-------|
| GetVer, timestamp 1, no parameters | 0 to 15 | `f1 32 a5 d4 e3 cf f8 57 48 b8 2a aa ca c6 8f 8a` |
| GetVer, timestamp 1, no parameters | 488 to 503 | `9d 69 87 a8 6a 3d 99 54 b1 3e c4 2b b4 83 67 04` |
| PushJpg, timestamp 42, size 123,456 | 0 to 15 | `6e df 63 cc 8e 6c 3f e4 3f 32 42 8b 39 cc e2 2c` |
| PushJpg, timestamp 42, size 123,456 | 496 to 503 | `27 38 27 32 b8 3e f6 43` |

The key ships in every L-Connect install and several public repositories.
This is obfuscation, not security.

## 4. Command Vocabulary and Replies

### 4.1 Commands

Parameters start at plaintext offset 8.

| Byte | Name | Parameters | Sent by this driver | Notes |
|------|------|------------|---------------------|-------|
| `0x0A` | GetVer | none | init | Reply carries the firmware string |
| `0x0B` | Reboot | none | no | Drops this panel into desktop mode |
| `0x0D` | Rotate | `[8]` = rotation & 3 | no | Untested on this panel |
| `0x0E` | Brightness | `[8]` = 0 to 100 | init, 100 | |
| `0x0F` | FrameRate | `[8]` = fps | init, 120 | |
| `0x11` | GetH264Block | none | no | H.264 chunk-size negotiation |
| `0x33` | SetClock | year (u16 big-endian), month, day, hour, minute, second, mode | init, mode 2 | Local wall-clock time |
| `0x34` | StopClock | `[8]` = 0 | init | Stops the clock overlay |
| `0x65` | PushJpg | `[8..12]` = JPEG size, u32 big-endian | every frame, init clear | Opaque background layer; the JPEG follows the header in the same write |
| `0x66` | PushPng | `[8..12]` = PNG size, u32 big-endian | init clear | Overlay layer composited above the JPEG layer |
| `0x67` | ClearPng | none | no | |
| `0x79` | StartPlay | chunk length (u32 BE), last flag, play count, play tick (u32 BE) | no | H.264 streaming |
| `0x7A` | QueryBlock | none | no | Reply `[8]` = buffered frames |
| `0x7B` | StopPlay | none | init | Stops H.264 playback |
| `0x80` | LcdRevision | none | no | The wireless receivers' "CheckNewLcd" |
| `0x96` | SwitchToDesktop | none | no | Followed by Reboot to enter desktop mode |

### 4.2 Replies

Replies arrive on bulk IN `0x81` in plaintext, at most 512 bytes:

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 1 | Command echo | The command being answered |
| 1 | 1 | Status | `0xC8` for success |
| 8 | varies | Body | Command-specific |

GetVer carries the firmware as NUL-terminated ASCII in bytes 8 to 39.
PushJpg and QueryBlock carry the panel's buffered-frame count at byte 8.

The panel answers every command, frames included. A frame that never gets
an answer is the signature of a JPEG the firmware rejected, usually for the
wrong geometry (lian-li-linux issues 23 and 55).

## 5. Session Model

### 5.1 Init

| Step | Command | Parameters | Why |
|------|---------|------------|-----|
| 1 | StopPlay | none | Stop any H.264 playback another host left running, then pause 150 ms |
| 2 | GetVer | none | Read the firmware string |
| 3 | FrameRate | 120 | What lian-li-linux sends |
| 4 | Brightness | 100 | Normalize the backlight; the daemon's software brightness is the runtime authority |
| 5 | SetClock | local time, mode 2 | What lian-li-linux sends before stopping the clock |
| 6 | StopClock | 0 | Keep the clock overlay off the frames |
| 7 | PushPng | transparent 480x1920 RGBA PNG | Clear any overlay a previous host left |
| 8 | PushJpg | black 480x1920 JPEG, quality 95 | Leave the vendor idle animation before the first frame |

Each step is followed by one optional status read of up to 512 bytes with a
2-second timeout. StopPlay is also followed by a 150 ms pause so playback
winds down before the next command, matching lian-li-linux's `WAKE_STEP`
(`winusb/lcd/core.rs` lines 522 and 733). The two clear images are encoded
once per process.

The order follows lian-li-linux's `Slv3WinUsbLcd::do_init`
(`winusb/lcd/slv3.rs` lines 50 to 64) with two changes. Its GetH264Block
step is dropped, because it only negotiates H.264 chunk sizes and the
maintainer's earlier working sequence (issue 55) omitted it. Brightness is
added, matching the wireless LCD driver; the maintainer calls it optional
and the issue 55 reporter's unit acked it.

### 5.2 Frames

A frame is one bulk write with no padding:

| Offset | Size | Content |
|--------|------|---------|
| 0 | 512 | PushJpg header carrying the JPEG size |
| 512 | JPEG length | The JPEG, verbatim |

The JPEG must be 480x1920 and baseline; the daemon's TurboJPEG encoder
produces baseline 4:2:0. The largest accepted JPEG is 512,000 bytes, the
`max_payload` lian-li-linux uses for this panel. Anything larger is refused
with `DisplayEncodeError::PayloadTooLarge`, and `max_display_frame_len`
advertises the cap so the daemon's encoder budget stays under it.

Each frame is followed by one optional status read of up to 512 bytes with a
200 ms timeout, the value the lian-li-linux maintainer gives for this panel.
A late reply is consumed by the next frame's read, which is harmless because
nothing but GetVer's body changes driver behavior.

### 5.3 Flow control

The frame reply's byte 8 counts frames buffered on the panel. lian-li-linux
waits when it exceeds 3, polling QueryBlock every 50 ms until it reaches 2
or 30 seconds pass (`winusb/lcd/core.rs` lines 1017 to 1056 and 1127 to
1158).

This driver records the level (`UniversalScreenProtocol::last_buffer_level`)
and logs at debug when it crosses 3, but does not hold frames. Delivery is
capped at 30 fps against a panel told to run at 120, so the buffer should
stay near empty. If hardware logs show it climbing, the protocol's
`frame_pump_interval` and `pump_frame_into` hooks are the path: hold the
newest frame while the level is above 3 and release it when a QueryBlock
reply reads 2 or less.

### 5.4 Timing

| Parameter | Value | Source |
|-----------|-------|--------|
| Frame interval | 33 ms (`max_fps` 30) | Baseline; the firmware accepts up to 120. Raise on measurement. |
| Init read timeout | 2 s | lian-li-linux `READ_TIMEOUT` |
| Frame reply timeout | 200 ms | lian-li-linux maintainer, issue 55 |
| Settle after StopPlay | 150 ms | lian-li-linux `WAKE_STEP` |
| Other inter-command delays | none | lian-li-linux sends the rest back to back |

### 5.5 Shutdown

Nothing is sent. The panel keeps its last frame. Reboot would drop it into
desktop mode, and there is no command that hands it back to the vendor idle
animation. The next session's init clears both layers.

### 5.6 Brightness and rotation

`supports_brightness` is false: the daemon applies brightness in software,
and init pins the backlight at 100. Rotate is never sent; the firmware's
behavior under rotation is unverified, and portrait is the only geometry
known to decode.

## 6. Topology and Orientation

The device exposes one display segment named `Display`: 480x1920, not
circular, JPEG, no LEDs.

The firmware decodes only portrait images. A 1920x480 JPEG is dropped with
no reply and no error while the idle animation keeps playing; lian-li-linux
hit this (issue 23, fixed in `270b86a`), and the maintainer gave the same
diagnosis for a macOS implementer's identical symptom (issue 55). The panel
sits turned a quarter inside its frame, so the image a user sees on a
horizontally mounted bar is the portrait buffer turned 90 degrees.

The HAL declares the wire geometry. Reading upright is a per-device user
setting: `DeviceUserSettings::display_rotation` set to a quarter turn makes
the daemon turn the scene viewport for this panel.

There is a known gap on the daemon side. Display faces render at the
surface geometry and are then turned, so a face on a quarter-turned panel is
laid out portrait rather than landscape. The fix belongs in the daemon:
swap the face geometry on quarter turns. Nothing in this driver depends on
it.

## 7. HAL Integration

### 7.1 Files

| File | Role |
|------|------|
| `drivers/lianli/winusb.rs` | Header codec: zerocopy plaintext and wire structs with size assertions, `wrap_winusb_header`, `WinUsbHeaderBuilder` |
| `drivers/lianli/universal_screen.rs` | `UniversalScreenProtocol`, the command enum, `clock_params` |
| `drivers/lianli/devices.rs` | Descriptor and `build_universal_screen_protocol` |
| `udev/99-hypercolor.rules` | `1cbe:a088` usb rule |
| `data/drivers/vendors/lianli.toml` | Device entry, status `in_progress` |

The codec reuses `DES_KEY`, `MAGIC`, `PARAMS_OFFSET`, and the timestamp
clock (`HeaderBuilder::next_timestamp`) from `wireless::crypto`.

### 7.2 Descriptor

```rust
DeviceDescriptor {
    vendor_id: 0x1CBE,
    product_id: 0xA088,
    name: "Lian Li Universal Screen 8.8\"",
    family: DeviceFamily::new_static("lianli", "Lian Li"),
    transport: TransportType::UsbBulk { interface: 0, report_id: 0 },
    protocol: ProtocolBinding {
        id: "lianli/universal-screen",
        build: build_universal_screen_protocol,
    },
    firmware_predicate: None,
    serial_quirk: None,
}
```

The driver ID is `lianli`, so the panel is enabled and disabled with the
rest of the Lian Li family. `UsbBulk` discovers the endpoints from interface
0 and sends each command as one transfer. The report ID is unused.

### 7.3 Module placement

The silicon-alignment rule would put TURZX controllers in a `turzx` module.
The driver lives in `drivers::lianli` instead because that is where the
tree already keeps this DES header family (the wireless receivers), every
registered `0x1CBE` product is Lian Li branded, and the driver ID keeps
user configuration in one place. The day a TURZX-branded panel (`0x0088`
and friends) gets a descriptor, the codec in `winusb.rs` and the receivers'
`crypto.rs` should move to a shared `turzx` module, with the Lian Li models
as variants of it.

### 7.4 Platforms

- **Linux:** the udev rule grants access; the transport detaches any kernel
  driver from interface 0 before claiming it.
- **Windows:** nusb needs WinUSB bound to the device. L-Connect drives the
  panel through a WinUSB transport class, so WinUSB is expected, and section
  10 asks the reporter to confirm it. WinUSB access is exclusive, so
  L-Connect must be fully closed while Hypercolor runs.
- **macOS:** untested. No kernel driver claims a class `0xFF` device there
  (lian-li-linux issue 55).

## 8. Open Questions and Risks

1. **Unconfirmed on Hypercolor.** Every fact here comes from other
   implementations. The device stays `in_progress` until section 10 passes.
2. **JPEG versus PNG.** Linx reports that PushJpg goes unanswered for images
   over about 2 KB under libusb and pushes everything as PNG. The
   lian-li-linux maintainer reports PushJpg acknowledged on the same
   firmware and attributes unanswered frames to landscape geometry. Linx
   sends portrait images, so the disagreement is unexplained. If frames are
   dropped on the reporter's unit, PNG frames through PushPng are the
   fallback; that needs a PNG payload path the daemon does not have.
3. **FrameRate semantics.** Whether 120 changes how fast the panel drains
   queued JPEG frames, or only affects H.264 playback, is unknown.
4. **Brightness scale.** lian-li-linux clamps to 0 to 100, while
   turing-smart-screen-python scales to 0 to 102 for TURZX panels. 100 is
   at or near full on either reading.
5. **Flow control** (section 5.3).
6. **Desktop mode.** A panel left in desktop mode enumerates as
   `1A86:AD21`, and Hypercolor will not see `0xA088`. Linx wakes it with a
   HID report carrying ASCII `5f3759df`; that path is out of scope.
7. **The LED ring** (`0416:8050`) needs its own descriptor and protocol,
   which lian-li-linux has (commit `b1e69de`).
8. **Reply draining.** The references flush the IN pipe after every reply;
   this driver relies on short-packet termination of a single read of up
   to 512 bytes. If the panel ever answers with exactly 512 bytes followed
   by a zero-length packet, later reads would shift by one reply. Harmless
   while only GetVer's body matters, but the flow control in section 5.3
   would need a drain first.
9. **Unanswered frames throttle quietly.** If the panel stops answering
   PushJpg, each frame waits out the 200 ms reply timeout, so delivery
   settles near 4 to 5 fps with no error. That number in `fps_sent` is the
   symptom to look for (section 10).
10. **Timestamp ceiling.** The shared `HeaderBuilder` clock saturates at
    `u32::MAX` milliseconds (about 49.7 days of one unbroken session) and
    then alternates between that value and zero. The references wrap
    instead. Neither behavior is tested against the firmware.

## 9. Testing Strategy

`crates/hypercolor-hal/tests/lianli_universal_screen_tests.rs` covers, with
no hardware:

- The header against the OpenSSL known answers in section 3.2, the six zero
  bytes, and the trailer.
- A DES-CBC decrypt of every emitted header back to plaintext: command,
  reserved byte, magic, little-endian timestamp, big-endian size.
- Parameters past 492 bytes dropped rather than overflowing, and strictly
  increasing timestamps from the builder.
- `clock_params` for a fixed instant.
- The init sequence: eight commands in order, their parameters, increasing
  timestamps, and status-read plans.
- The two clear images decode as 480x1920 at their header sizes, and the PNG
  is fully transparent.
- Frames: one write at header plus JPEG length, no padding on 512-aligned
  totals, the payload verbatim, the 512,000-byte cap exact and one over,
  command buffer reuse across frames, and RGB payloads refused.
- Reply parsing: firmware recorded from a GetVer reply and not from garbage,
  forgotten on a new session; the buffer level from a frame reply; short,
  empty, and non-OK replies tolerated.
- Topology, capabilities, the empty LED path, and the single descriptor on
  bulk interface 0.

## 10. Hardware Validation Plan

These are the asks for the issue #361 reporter, on Windows.

**Step 1: driver binding.** Close L-Connect completely (including its tray
icon). WinUSB lets only one program hold the panel, so also check that
nothing from L-Connect is still running, then run in PowerShell and paste
the output:

```powershell
Get-Process | Where-Object { $_.ProcessName -match 'connect|lian' } |
  Format-Table ProcessName, Id, Path -AutoSize
Get-Service | Where-Object { $_.DisplayName -match 'connect|lian' } |
  Format-Table Name, DisplayName, Status -AutoSize
Get-PnpDevice -PresentOnly |
  Where-Object { $_.InstanceId -match 'VID_1CBE&PID_A088|VID_0416&PID_8050|VID_1A86' } |
  Format-List FriendlyName, InstanceId, Class, Status
Get-PnpDevice -PresentOnly |
  Where-Object { $_.InstanceId -match 'VID_1CBE&PID_A088' } |
  Get-PnpDeviceProperty -KeyName DEVPKEY_Device_Service, DEVPKEY_Device_DriverInfPath, DEVPKEY_Device_BusReportedDeviceDesc |
  Format-Table KeyName, Data -AutoSize
```

`DEVPKEY_Device_Service` should read `WinUSB` (any capitalization).
Anything else (`libusbK`, `libusb0`, blank) means Hypercolor cannot open the
panel until the driver changes.

**Step 2: descriptor dump.** Run Uwe Sieber's USB Device Tree Viewer
(USBTreeView), select the "8.8" Universal Screen" node, and attach the text
report from the right-hand pane.

**Step 3: patched build with logs.** With L-Connect closed, start the
daemon from PowerShell with debug logging for the HAL:

```powershell
$env:RUST_LOG = "warn,hypercolor_hal=debug,hypercolor_core=debug,hypercolor_daemon=info"
& "<install dir>\hypercolor-daemon.exe" 2>&1 | Tee-Object -FilePath "$HOME\Desktop\hypercolor-a088.log"
```

Then report:

- Whether the panel leaves the Lian Li idle animation and goes black within
  a few seconds of the daemon starting.
- Whether an effect shows on the panel, and which way it is turned. Then
  set the panel's display rotation in its device settings to 90 and to 270
  and say which reads upright.
- The log lines mentioning `Universal Screen`, especially the firmware
  line, any "frame buffer is filling" line, and any "unexpected status"
  line.
- The output of `hypercolor diagnose --system -j` after a minute of an
  effect running; the panel's entry under `device_output` carries
  `fps_sent`, `avg_write_ms`, `frames_dropped`, and `last_error`. An
  `fps_sent` stuck near 4 to 5 with no errors means the panel is not
  answering frames (section 8, item 9).

**Step 4, only if the panel stays on the idle animation: capture
L-Connect.** Install Wireshark with the USBPcap component and reboot. With
Hypercolor stopped, start a capture on the USBPcap root hub the panel sits
behind (unplug and replug the panel while capturing so the descriptors are
included). Open L-Connect, set the Universal Screen to a solid red image,
wait five seconds, switch to a solid green image, wait five seconds, then
stop the capture. Export a transcript of the panel's traffic and attach both
the transcript and the `.pcapng` (zipped):

```powershell
$tshark = "C:\Program Files\Wireshark\tshark.exe"
$addr = & $tshark -r a088.pcapng -Y "usb.idVendor == 0x1cbe && usb.idProduct == 0xa088" `
  -T fields -e usb.device_address | Select-Object -First 1
& $tshark -r a088.pcapng -Y "usb.device_address == $addr && usb.capdata" `
  -T fields -e frame.number -e frame.time_relative -e usb.endpoint_address `
  -e usb.data_len -e usb.capdata > a088-transcript.tsv
```

The red and green images differ only in their JPEG bytes, so the capture
pins the header Lian Li's own software sends for a frame, the init sequence
it uses, and any timing between commands.

## References

- Hypercolor issue #361: "[device] Unknown vendor 8.8" Universal Screen-1.5"
- Spec 80 section 7: the wireless LCD receivers and the 504-byte header
- sgtaziz/lian-li-linux at `7c0de10c`:
  - `crates/lianli-devices/src/crypto.rs`: key (line 6), command bytes
    (lines 11 to 27), `build_winusb` (lines 165 to 199),
    `jpeg_header_winusb` (line 201), `sync_clock_header_winusb` (line 259),
    `stop_clock_header_winusb` (line 278)
  - `crates/lianli-devices/src/winusb/lcd/slv3.rs`: `do_init` (lines 50 to
    64) and its constants (lines 67 to 69)
  - `crates/lianli-devices/src/winusb/lcd/mod.rs`: PID dispatch (line 201),
    StopPlay support (line 210), screen table (line 287)
  - `crates/lianli-devices/src/winusb/lcd/core.rs`: firmware reply check
    (line 49), `WAKE_STEP` (line 522), the StopPlay pause (line 733),
    buffer level (line 550), `read_firmware` (line 878),
    `clear_layers` (line 970), `send_frame` (line 1017), `wait_buffer`
    (line 1127)
  - `crates/lianli-shared/src/screen.rs`: `UNIVERSAL_SCREEN` (line 98)
  - Issues 23, 55, 177, 241, and 245, and commit `270b86a` (portrait fix)
- elniko/lianli-python-lcd-overlay at `7bc6098`, `Linx/linx.py`: USB IDs
  (line 35), wake magic (line 48), command enum (lines 51 to 72),
  `_make_header` (line 98); `Linx/README.md` known issues
- mathoudebine/turing-smart-screen-python at `2b33ab4`,
  `library/lcd/lcd_comm_turing_usb.py`: product table (line 44), reply
  check (line 67), header builder (line 435), encryption (lines 445 and
  452)
- hieubui2409/pc-screens at `ffbfae4`, `lianli88.py`
- hello-nexus/nexus-service at `bc2b8aa`,
  `src/Peripherals/BulkPanels/UniversalScreen88Protocol.cs`
