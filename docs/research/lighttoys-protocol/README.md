# Lighttoys Protocol Research

Working dossier for reverse-engineering the Pyroterra Lighttoys FT wire protocol so
hypercolor can drive the Lumi Wand FT2 (and other FT/FT2 props) natively from Linux,
bypassing LtComposer entirely.

> Status: protocol vocabulary decoded against live hardware. LtComposer Live
> Control command path captured on Windows; dynamic-mode and show-upload
> protocols remain open.
> Target: native Lighttoys driver over USB-CDC serial; exact crate placement
> should be decided in the driver spec.
> Sessions: bliss + nova, 2026-05-22 (research dossier) and 2026-05-23 (live decode).
>
> **For latest protocol details, read [`vocabulary.md`](vocabulary.md).**
> **For Live Control capture details, read [`activation-sequence.md`](activation-sequence.md).**
> **For pixel-streaming evidence, read [`streaming-investigation.md`](streaming-investigation.md).**
> **For continuing on Windows, read [`windows-handoff.md`](windows-handoff.md).**

## 🎯 TL;DR

- **Hardware in hand:** Lumi Wand FT2 plus new FT Remote (`ctrl2` / LTC-2.0.0),
  verified on Linux and Windows with LtComposer 4.4.
- **Why LtComposer-based OSC bridge was rejected:** LtComposer is Win/Mac only.
  Hypercolor runs on Linux. Adding a Mac-resident OSC bridge defeats the point.
- **Real plan:** native Rust driver that speaks USB-CDC ACM directly to the FT Remote
  on `/dev/ttyACM*`. The remote acts as an RF gateway to the wand.
- **No telemetry available.** The wand has no IMU, the FT Remote is one-way as far as
  motion goes, OSC in LtComposer is listen-only. Workarounds (phone IMU, camera blob
  tracking, audio-driven) exist but are separate hypercolor inputs, not wand readback.
- **Live Control path:** LtComposer sends `gmute 1` for BLACKOUT and compact
  `lprog 0,<cfgstr>` commands for colors/brightness. Example:
  `lprog 0,mM*B!0*A!0c0xff0000eCOb5`.
- **Direct replay confirmed:** with LtComposer closed, our own serial session can
  drive the wand red/green/blue via `lprog 0,<cfgstr>`.
- **Next concrete action:** capture dynamic Live Control modes and show upload.

## 🦋 Target Device: Lumi Wand FT2

- Family: **FT2** (Pyroterra Lighttoys, "Lumi" branding).
- LED density: 144 LEDs/m, 800 lumen output, 6 brightness levels.
- Body: carbon fiber core, polycarbonate shell, 65-70 cm long, 22mm diameter, 255-268g.
- Battery: ~3 hours runtime, 2.5 hour charge time.
- Programming: LtComposer 4.4 (proprietary, Win/Mac).
- **Why FT2 matters:** FT2 chips drive addressable digital LED strips (APA102, WS2812,
  SK6812, WS2813, etc.) and accept both live color control AND pre-uploaded image
  sequences. FT2 props are functionally pixel devices, but their host-control surface
  is A/B-zoned (two color segments), not per-pixel streaming.

Firmware on the wand itself (per `firmware.cfg`):

| Field | Value |
| --- | --- |
| firmware version | 20260323 (= 2026-03-23) |
| bootloader version | 0x3241 |
| app file | `ft2_slave-app-v10r136_260323.lbc` |
| bouper (recovery) file | `ft2_slave-bouper-v10r136_260323.lbc` |

## 🪄 LtComposer Architecture (binary analysis)

App bundle: `/Applications/LtComposer.app`, universal binary (x86_64 + arm64).

### Framework stack

- **libcinder 0.9.3** (creative coding C++ framework). Bundle id is
  `org.libcinder.LtComposer3`.
- **asio** for networking (used by OSC and likely for serial async).
- **wjwwood/serial** C++ library for USB-CDC serial I/O.
- **ffmpeg** (`libavcodec 60.6.101`, `libavformat 60.4.100`, `libavutil 58.4.100`,
  `libswresample 4.11.100`, `libswscale 7.2.100`) for audio and video file decoding
  in the timeline.
- **IOKit** for macOS-side USB enumeration.
- **AVFoundation, CoreAudio, AudioToolbox** for audio playback.

### Source paths (leaked via debug strings)

All paths under `/Users/tomassedlacek/cinderApps/LtComposer3/`:

```
src/comm/AbstractSerial.cpp           # base serial driver
src/comm/Ft2Device.{cpp,h}            # FT2 device class
src/comm/FtFactorySettings.cpp        # FT factory config
src/comm/FtFirmware.h
src/comm/FtMainVariables.h
src/comm/LsfCommand.cpp               # legacy .lsf format commands
src/comm/RemoteCmdLine.cpp            # remote command parser
src/comm/RemoteDevice.cpp             # remote (FT dongle) abstraction
src/comm/RemoteFactorySettings.cpp
src/comm/RemoteUpdater.{cpp,h}        # firmware OTA path
src/comm/RemoteUploader.cpp           # show upload path
src/comm/RemoteProUploader.{cpp,h}
src/comm/SerialUtils.cpp
src/comm/SlaveDevice.{cpp,h}          # paired prop device
src/comm/VpoiDevice.{cpp,h}           # Visual Poi USB-direct
src/comm/DeviceList.cpp
src/comm/FirmwareConfig.cpp           # parses firmware.cfg
src/comm/UploadLsfContainer.h
src/osc/Osc.cpp
src/osc/OscReceiver.cpp               # OSC listen-mode handler
src/LtComposer.cpp                    # main app entry
src/UpdateFwManager.{cpp,h}           # firmware update orchestration
src/UpdateChecker.h
src/LiveControlContent.h              # live control window
src/LiveControlVariables.cpp
src/DeviceType.cpp                    # device family taxonomy
```

Tomas Sedlacek is the developer (or at least the build user).

### Internal command flow

```
LtComposer GUI / OSC input
       │
       ▼
lt::RemoteCmdList    (a sequence of high-level commands)
       │
       ▼
lt::RemoteCmdQueue   (FIFO with async dispatch)
       │
       ▼
callbackListenLines(std::string)   ◄── line-oriented dispatch
       │
       ▼
lt::AbstractSerial   (wjwwood/serial)
       │
       ▼
/dev/cu.usbmodem*  (USB CDC ACM)
       │
       ▼
FT Remote (hardware bridge: USB ⇄ RF)
       │
       ▼
RF (~2.4 GHz, protocol TBD)
       │
       ▼
Lumi Wand FT2 (or other paired prop)
```

Key escape hatch: `lt::RemoteDevice::sendDirectCommand(std::string)` accepts a raw
line and pushes it directly into the command queue, bypassing the high-level
abstractions. This is what `/lighttoys/direct` OSC routes to.

## 📡 OSC Control Surface

LtComposer runs **OSC listen-mode only** (no outbound), UDP, default port `65535`
(placeholder — must be set explicitly in the OSC Console). Settings file at
`~/Library/Application Support/LtComposer3/setting.json`.

### Documented commands (per manual v4.4 + binary confirmation)

| OSC path | Args | Notes |
| --- | --- | --- |
| `/lighttoys/ping` | none | Broadcast PING to paired FTs |
| `/lighttoys/blackout` | none | LEDs off on all paired props |
| `/lighttoys/standby` | none | Standby mode |
| `/lighttoys/start` | `<show 1-4> <time ms> <delay ms> <brightness 1-6> <duration ms>` | Start pre-uploaded show |
| `/lighttoys/start/seconds` | same as above but seconds | |
| `/lighttoys/stop` | none | Stop running show |
| `/lighttoys/brightness` | `<level 1-6>` | 6 discrete levels, no float |
| `/lighttoys/color` | `<r1 g1 b1 r2 g2 b2>` | A and B segment color (0-255 each) |
| `/lighttoys/dynab` | `<0=AB, 1=A, 2=B>` | Select active controlled segment |
| `/lighttoys/dyncolorr` | `<0-255>` | Red value on controlled segment |
| `/lighttoys/dyncolorg` | `<0-255>` | Green value on controlled segment |
| `/lighttoys/dyncolorb` | `<0-255>` | Blue value on controlled segment |
| `/lighttoys/dynmode` | `<mode> <pos?>` | Documented as 1-5 but see hidden caps |
| `/lighttoys/dynpos` | `<1-8>` | Position inside dynamic effect |
| `/lighttoys/dynspeed` | `<1-8>` | Effect speed |
| `/lighttoys/newpair` | none | Start fresh pairing flow |
| `/lighttoys/addtogroup` | none | Pair extra unit to current group |
| `/lighttoys/stoppair` | none | Stop pairing |
| `/lighttoys/random` | `<delay> <speed> <brightness> <mode 1-3>` | Shuffle dynamic effects |

Two message formats are supported:
1. **Standard OSC**: address + typed args (`/lighttoys/color` with six `int32` args).
2. **Inline**: address contains everything (`/lighttoys/start/2/time/2000/delay/5000/brightness/1`).

Inline arguments can be reordered when prefixed with their token name (e.g.
`time`, `delay`, `duration`, `brightness`, `dyncolorr`, etc.).

### Undocumented commands (found via string extraction)

These are in the binary but absent from the manual:

| OSC path | Inferred behavior | Evidence |
| --- | --- | --- |
| `/lighttoys/direct <string>` | Pass arbitrary raw line to the FT Remote | Demangled symbol `lt::RemoteDevice::sendDirectCommand(std::string)` registered alongside other OSC handlers |
| `/lighttoys/dyncolorwheel <int>` | Set hue on color wheel (HSV-style) | Log string `Set dynamic color wheel cmd:` next to other `Set dynamic * cmd:` lines |

**`/lighttoys/direct` is the most interesting find.** Once we know the wire vocabulary,
this is the route to issue arbitrary commands via OSC bypassing the abstraction layer,
and equivalently it tells us that the underlying serial protocol *is* exposed as a
string-based command line.

### Error response

`ERR - Bad cmd` (visible in the binary, presumably sent by the FT Remote firmware
when it doesn't recognize a command).

## 🔌 Serial Wire Protocol (USB-CDC ACM)

### What we know

- **Transport:** USB CDC ACM. On macOS, the device shows up as `/dev/cu.usbmodem*`.
  On Linux it'll be `/dev/ttyACM*`.
- **Line-oriented text protocol.** Evidence:
  - `callbackListenLines(std::string)` in the binary.
  - `ERR - Bad cmd` error response (suggests textual command parser).
  - `lt::RemoteDevice::sendDirectCommand(std::string)` accepts a string.
  - `lt::RemoteCmdQueue` and `lt::RemoteCmdList` use string-typed callbacks.
- **Library:** wjwwood/serial (well-known C++ serial port library, blocking IO with
  cross-platform wrappers).
- Probably **115200 baud** (default for most CDC ACM virtual ports, very common in
  embedded), but unconfirmed.
- The `/lighttoys/direct` OSC command suggests **the wire commands are very close in
  shape to the OSC paths themselves** (otherwise why expose a string passthrough?).

### What we don't know yet

- Exact command tokens. Are they `ping\n` / `color 255 0 0 0 0 255\n`, or different?
- Frame delimiters. Probably `\r\n` (CRLF) given the strings showed CRLF terminators
  in the firmware HEX files. Possibly just `\n`.
- Baud rate. Likely 115200 but could be 460800 or 921600 for upload throughput.
- Whether show upload uses the same text protocol or a separate binary mode.
- The device's USB VID/PID for hypercolor's device probe.

### Hypothesis (to be verified)

Best guess based on evidence:

```
ping\r\n              → broadcast ping
blackout\r\n          → blackout
standby\r\n           → standby
color 255 0 0 0 255 0\r\n           → A red, B green
brightness 6\r\n      → max brightness
dynmode 20\r\n        → dynamic effect mode 20
dynpos 4\r\n          → position 4
dynspeed 6\r\n        → speed 6
dyncolorr 255\r\n     → red on active segment
start 1 0 0 6 0\r\n   → start show 1, time 0, delay 0, brightness 6, duration unlimited
stop\r\n              → stop show
```

If the vocabulary is more terse (single-letter mnemonics), error responses will tell us.

## 🌈 Hidden Capabilities

### Mode range is larger than documented

From the user's existing `setting.json`:

```json
"liveControl": {
  "ABmode": 1,
  "currentAB": [
    { "r": 0.5, "g": 0, "b": 1, "mode": 20, "speed": 0 },
    { "r": 1.0, "g": 0, "b": 1, "mode": 45, "speed": 0 }
  ],
  ...
}
```

The OSC `/lighttoys/dynmode` is documented 1-5, but Live Control persists modes **20**
and **45**. So either:
- Live Control uses a different command than OSC `dynmode`, OR
- `dynmode` accepts a wider range that the manual doesn't expose.

Either way, more effects are accessible than the OSC manual suggests. The wire-level
probe will reveal the full range.

### Multi-format ASCII upload

LtComposer can export sequences as `.lsf` (legacy) and `.ltp` (current, which is a ZIP).
The wand's image-sequence upload protocol over USB likely differs from the live-control
text protocol, but `RemoteUploader` shares the same `RemoteCmdQueue` plumbing — so it's
probably the same line protocol with an "enter upload mode" command followed by binary
data frames.

## 💾 Firmware Inventory

Location: `~/Library/Application Support/LtComposer3/firmware/`

All firmware is shipped plain (no encryption observed in the file headers). The `.lbc`
binaries have ARM Cortex-M vector tables at offset 0; `.lhex` files are vanilla Intel
HEX with CRLF line endings.

### Mapping (from `firmware.cfg`)

| Device | App file | Bouper (recovery) | Bootloader | Last update |
| --- | --- | --- | --- | --- |
| **FT2 slave (Lumi Wand)** | `ft2_slave-app-v10r136_260323.lbc` | `ft2_slave-bouper-v10r136_260323.lbc` | 0x3241 | 2026-03-23 |
| FT1 slave | `ft1_slave-app-v10r136_260323.lbc` | `ft1_slave-bouper-*-v10r136_260323.lbc` | 0x3241 | 2026-03-23 |
| FT1 Cube (twin slave) | `ft1-twin_slave-app-v10r101_240701.lbc` | `ft1-twin_slave-bouper-*` | 0x2ECC | 2024-07-01 |
| FT1 nRF52 (Bluetooth?) | `ft1nRF52-app-v10r136_260323.lbc` | `ft1nRF52-bouper-v10r136_260323.lbc` | 0x3350 | 2026-03-23 |
| FT1 Light | `ft1light-app-v10r136_260323.lbc` | `ft1light-bouper-v10r136_260323.lbc` | 0x3350 | 2026-03-23 |
| FT1 Light STM stage light | `ftstagelight_OTA_1.0.2.dfu` | — | — | 1.0.0 |
| FT VPOI slave (Visual Poi) | `ft1-vpoi_slave-app-v10r136_260323.lbc` | `ft1-vpoi_slave-bouper-*` | 0x3241 | 2026-03-23 |
| Visual Poi 5 (USB direct) | `vpoi5-app-v10r136_260323.lbc` | `vpoi5-bouper-v10r136_260323.lbc` | 0x3241 | 2026-03-23 |
| Visual Poi 4 | `vis-poi4-0.47.hex` | — | — | 0.47 |
| Visual Poi 5a | `vis-poi5a-1.08.hex` | — | — | 1.08 |
| Visual Poi (older) | `vis-poi-0.38.hex` | — | — | 0.38 |
| Old FT Remote | `ctrl-app-v10r136_260323.lhex` | — | — | 2024-07-01 |
| **New FT Remote (ctrl2)** | `ctrl2-app-v10r136_260323.lhex` | — | — | 2025-06-11 / 2026-03-23 |
| FT Remote Pro (Nordic) | `meshtek-app-v10r136_260323.lhex` | — | — | 2026-03-23 |
| FT Remote Pro (Cypress) | `ftremotepro_1.8.7.104_d98dff4c_OTA.fwu` | — | — | 1.8.7.104 |

### File formats

- `.lbc` — ARM Cortex-M raw binary firmware. First word is the initial SP
  (~`0x2003FFDC` for the wand, top of SRAM), second word is the Reset_Handler in flash
  (~`0x0001E604`, Thumb mode). Loads at some flash offset (probably 0x10000 = 64KB
  in, leaving room for the bootloader). **Plain, disassemblable in Ghidra directly.**
- `.lhex` / `.hex` — Intel HEX format, ASCII, CRLF terminators. Converts to binary
  with `arm-none-eabi-objcopy -I ihex -O binary input.lhex output.bin`.
- `.dfu` — USB DFU standard format (used for stage light OTA).
- `.fwu` — proprietary OTA blob (only used for FT Remote Pro Cypress, 1.6 MB).

### Strategic value

- **New FT Remote firmware (`ctrl2-app-v10r136_260323.lhex`, 251KB)** is the chip that
  speaks USB-CDC to the host PC. Disassembling this gives us the host-side command
  parser authoritative source-of-truth. Strings extraction was sparse; needs real
  Ghidra work to find the dispatch table.
- **Lumi Wand firmware (`ft2_slave-app-v10r136_260323.lbc`, 121KB)** is the radio-side
  receiver. Disassembling tells us the RF protocol if we ever want to skip the FT
  Remote entirely.
- **`meshtek-app-v10r136_260323.lhex`** — the "Pro" remote runs on a Nordic chip with
  BLE mesh. Interesting future angle: if the Pro remote supports BLE, hypercolor could
  potentially go BLE-direct without USB.

## 🔬 Capture / Probe Strategy

### Why USB capture on macOS failed

- macOS XHC USB capture works on Intel Macs but is **broken on Apple Silicon**
  (M1/M2/M3/M4). Bliss is on M4 Max, arm64. Apple removed the kernel-side hook around
  macOS 13. ChmodBPF helper is installed and bpf perms are fine, but no `XHC*` capture
  interface shows up in `dumpcap -D`.
- Workaround paths (SIP off + dtrace, or third-party USB sniffers like USBLyzer
  equivalents) are too invasive for one capture session.

### Path A: Linux + Wine + usbmon (clean pcap)

```bash
# On Linux box, plug in FT Remote, wand paired
sudo modprobe usbmon
lsusb | grep -iE "pyroterra|lighttoys|microchip|cypress|nordic"
# → note the bus and device, capture filtered:
sudo tshark -i usbmonN -w /tmp/lt-capture.pcapng \
  -f "usb.bus_id == X and usb.device_address == Y"

# In parallel, run Windows LtComposer under Wine
WINEPREFIX=~/.wine-lt wine /path/to/LtComposer4.4-setup.exe   # install once
wine ~/.wine-lt/drive_c/.../LtComposer.exe
# Wine maps /dev/ttyACM* to a virtual COM port for the Windows binary.
```

**Action script during capture** (do each with ~2s pause for visual separation):

1. Wait for device detection (init handshake).
2. Blackout.
3. Standby.
4. Color: both segments red (255, 0, 0 / 255, 0, 0).
5. Color: A green, B blue.
6. Brightness 1, then 6.
7. AB toggle: A+B, then A only, then B only.
8. Dynamic modes 1, 2, 3, 4, 5 in order.
9. Dynamic modes 20, 45 (from saved favorites in setting.json).
10. Position 1 through 8.
11. Speed 1 through 8.
12. Drag color wheel through full hue range.
13. Run Show bank 1, 2, 3, 4 (if shows uploaded).
14. Stop.

Save the `.pcapng` and a timeline log mapping seconds → action.

### Path B: Interactive Probe (recommended first)

Faster and informative because the wand itself is a visible oracle. On Linux with FT
Remote attached and wand paired:

```bash
ls /dev/ttyACM*
picocom -b 115200 /dev/ttyACM0
# (or 460800 / 921600 if 115200 doesn't elicit a response)
```

Try the OSC names as plain text commands, one per line, looking for `ERR - Bad cmd`
vs anything else, and watching the wand for visible state change. Start with safe
introspection:

```
ping
PING
?
help
HELP
version
VER
info
```

Then state-changing commands (wand will be visible):

```
blackout
standby
color 255 0 0 0 0 255
brightness 6
dynmode 1
dynmode 5
dynmode 20
dynmode 45
dyncolorr 255
dyncolorg 128
dyncolorb 64
dynpos 4
dynspeed 6
dynab 1
stop
```

**Caveats:**
- We might confuse the firmware. The FT Remote has a recovery (bouper) firmware so
  bricking is unlikely but worth respecting.
- Could need to send something to "enter direct mode" before commands work.
- May need CR+LF rather than just LF.
- Default baud may not be 115200.

If the OSC-mirror hypothesis fails, fall back to Path A.

### Probe automation tool (to write)

A short Python script that:
1. Opens `/dev/ttyACM*` at a configurable baud.
2. Iterates a candidate list of `(command, expected-behavior-description)` tuples.
3. Sends each, reads response with timeout, classifies as `OK / ERR / SILENT`.
4. Logs to a Markdown table for direct paste into this dossier.

Will live at `tools/lighttoys-probe/probe.py` (not yet written).

## 🎨 Hypercolor Integration Plan

### Tier 1 — REJECTED (OSC bridge via LtComposer)

Doesn't work for Bliss's actual setup (Linux primary). Documented for completeness only.

### Tier 2 — TARGET: Native CDC ACM driver

New native Lighttoys driver over USB-CDC serial. Crate placement is TBD in the
driver spec: the transport is serial/USB, but the FT Remote behaves more like a
gateway-backed smart-light driver than a plain local HID endpoint.

**Architecture:**
- USB-CDC transport via `serialport` crate (Rust equivalent of wjwwood/serial).
- Async command queue using `tokio` channels, mirroring `RemoteCmdQueue` structurally.
- Device probe by USB VID/PID (need to capture from connected FT Remote).
- Hypercolor device model: `Lumi Wand FT2` exposed as a 2-zone (A/B) color device.
  - Two color slots per device.
  - 6-level brightness control.
  - Dynamic mode selector (full range, not just 1-5).
  - Show-trigger as a special action (`/start` mapped to a hypercolor scene event).
- Spatial layout: A = top half, B = bottom half.
- Frame rate: probably 10-20 Hz ceiling over RF. Hypercolor's adaptive FPS controller
  already handles this.

**Capabilities surfaced:**
- Live A/B color from canvas sampler.
- Dynamic mode + position + speed (full range pending probe).
- Brightness slider (6-level snap).
- Pre-uploaded show trigger via hypercolor scene actions.
- Pairing flow (deferred — assume already paired).

**Out of scope for first cut:**
- Show upload from hypercolor (use LtComposer on Mac for that until we RE the binary
  protocol).
- Telemetry (none available).
- Per-pixel streaming (not supported by the protocol).

### Tier 3 — FUTURE: Skip LtComposer entirely with our own RF transmitter

Big project, months of work. Path:
1. Disassemble `ft2_slave-app-v10r136_260323.lbc` in Ghidra.
2. Identify the radio chip and protocol (likely nRF24L01 family or Nordic SoftDevice).
3. Build a hypercolor-side transmitter using nRF24 module on USB / SPI.
4. Implement the radio protocol from scratch.

Benefits: no Pyroterra hardware in the chain at all. Risk: protocol is complex,
includes pairing crypto.

Holding for now.

## 🪐 Open Questions

1. Full dynamic `lprog` token grammar: effects, speed, variants, and segment
   targeting beyond solid `!0`.
2. Show-upload protocol framing: `hswr` vs `ldswrite`, chunking, completion, and
   bank erase/update semantics.
3. Whether any hidden RF/slave command path accepts raw pixel frames, or whether
   pixel-level content is only stored-show playback.
4. Does the wand have any latent IMU/sensor hardware that the firmware doesn't
   currently expose? (Disassembly would answer.)
5. Does the FT Remote send any unsolicited messages (device-in-range / out-of-range /
   battery low) that hypercolor could subscribe to for stage cues?

## ✅ Next Steps

Done in 2026-05-23 session:

1. ☑ Plug FT Remote into Linux (vesper), identify device. **VID:PID = `0x1669:0x1026`**,
   `/dev/ttyACM0`, manufacturer "Pyroterra s.r.o.", product "Lighttoys Controller".
2. ☑ Write `tools/lighttoys-probe/probe.py` — Python serial probe.
3. ☑ Interactive probe session against live remote. Full vocabulary captured from
   the firmware's `help` command. See [`vocabulary.md`](vocabulary.md) for the
   complete protocol reference.
4. ☑ Decode key protocol details: 115200 8N1, line-oriented ASCII, CRLF responses,
   echo control via `mecho 0`, `ERROR <code>` framing, `CMD>OK/Warn/Wait/Done/Error`
   response classes. DTR/RTS must stay False on open (wedges otherwise).
5. ☑ Drive the wand visibly via `leach 0xFF,R,G,B,R,G,B` (one-time success — the
   activation question remains for sustained live control).

Open / next:

6. ☑ **Capture LtComposer USB traffic on Windows during a real Live Control
   session.** Capture:
   `captures/2026-05-23-162521-user-driven-livecontrol.pcapng`.
7. ☑ Decode the captured pcap and document the Live Control command path in
   [`activation-sequence.md`](activation-sequence.md).
8. ☑ Directly replay the captured `lprog 0,<cfgstr>` commands with LtComposer
   closed. Result: `CMD>OK`, and the wand visibly changed red/green/blue.
9. ☑ Safely probe `lds*` forms. Result: valid-looking `ldson`, `ldsspace`,
   and `ldswrite` probes return `ERROR 3` in current `prog` state, and firmware
   help identifies LDS as show-upload storage.
10. ☐ Capture an LtComposer show upload to decode the `hswr` / `ldswrite`
   file-write protocol
   (this is the Instagram/pre-rendered-video path, may be more useful than live
   streaming for most content).
11. ☐ Capture dynamic Live Control modes to decode the rest of the `lprog`
   token grammar.
12. ☐ Disassemble `ctrl2` / `ft2_slave` to confirm whether any hidden RF path
   accepts raw pixel frames, or whether pixel content is only stored-show
   playback.
13. ☐ Spec the native Lighttoys driver (add to `docs/specs/`).
14. ☐ Implement the driver as a feature flag in `hypercolor-driver-builtin`.
15. ☐ Add a Lumi Wand entry to `data/drivers/vendors/pyroterra.toml` for the
    device database.
16. ☐ Manual test on Bliss's actual stage setup.

## 📁 Companion Files

This folder will grow:

- `README.md` (this file) — main dossier.
- `vocabulary.md` — captured wire commands with arguments and responses.
- `activation-sequence.md` — LtComposer Live Control transcript and `lprog`
  grammar notes.
- `streaming-investigation.md` — evidence for/against hidden live pixel
  streaming.
- `captures/*.pcapng` — raw USB captures, intentionally gitignored.
- `firmware-notes.md` (TODO) — Ghidra findings from `.lbc` / `.lhex` disassembly.
- `radio-protocol.md` (TODO, far future) — RF protocol between FT Remote and props.

## 📚 References

- LtComposer 4.4 user manual: <https://www.lighttoys.cz/app/uploads/2026/03/LtComposer-4.4-user-manual.pdf>
- Lumi Wand FT2 product page: <https://www.lighttoys.cz/product/lumi-wand-ft2/>
- Visual Pixel Wand (different family): <https://www.lighttoys.cz/product/visual-wand-v5/>
- wjwwood/serial library: <https://github.com/wjwwood/serial>
- libcinder: <https://libcinder.org/>
- OSC spec: <https://opensoundcontrol.stanford.edu/spec-1_0.html>

---

_Last updated: 2026-05-23 by Nova (live decode session with Bliss)._
