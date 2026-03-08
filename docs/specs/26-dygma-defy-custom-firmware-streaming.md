# 26 -- Dygma Defy Custom Firmware Streaming

> Custom firmware extension for Dygma Defy that adds a non-persistent live RGB streaming path for Hypercolor. Uses a lightweight Focus handshake for capability discovery and a separate binary serial protocol for frame transport, with banked neuron-to-keyscanner forwarding.

**Status:** Draft
**Crate:** `hypercolor-hal`
**Firmware repo:** `~/dev/NeuronWireless_defy`
**Module path:** `hypercolor_hal::drivers::dygma`
**Author:** Codex
**Date:** 2026-03-08

---

## Table of Contents

1. [Overview](#1-overview)
2. [Current Firmware Limitation](#2-current-firmware-limitation)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Protocol Architecture](#4-protocol-architecture)
5. [Firmware Changes](#5-firmware-changes)
6. [Hypercolor Changes](#6-hypercolor-changes)
7. [Performance Budget](#7-performance-budget)
8. [Compatibility and Rollout](#8-compatibility-and-rollout)
9. [Testing Strategy](#9-testing-strategy)
10. [Recommended First Milestone](#10-recommended-first-milestone)

---

## 1. Overview

Stock Defy firmware does not provide a safe live per-LED streaming path to the keyboard halves. The working lighting paths are:

- serialized built-in LED modes delivered over `MODE_LED`
- EEPROM-backed palette and colormap synchronization
- brightness control

Hypercolor's current Dygma driver attempted to use `led.theme` as a direct frame transport. That does not light the halves on stock firmware, because those writes only mutate neuron-local LED state.

This spec defines a **custom firmware extension** that makes live Hypercolor output possible without EEPROM writes and without abusing Bazecor's persistence APIs.

The design has two planes:

- **Control plane:** existing Focus text commands for capability discovery and fallback-safe negotiation
- **Data plane:** new binary serial framing for full-board RGB frames, followed by banked forwarding from neuron to keyscanners

The data plane is intentionally binary. A text protocol is too expensive for sustained 10 fps streaming over 115200 baud CDC serial.

---

## 2. Current Firmware Limitation

### 2.1 `led.theme` only mutates neuron-local LED state

In `LEDControlDefy.cpp`, the `THEME` Focus command loops over LEDs and calls `LEDControl::setCrgbAt(...)`, but it never forwards those colors to the halves:

- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/plugin/LEDControlDefy.cpp`

### 2.2 Defy `syncLeds()` does not flush side LED banks

Both neuron implementations track changed left/right LED banks, but `syncLeds()` only handles enable/brightness and neuron LED state:

- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/device/dygma/defyWN/DefyWN.cpp`
- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/device/dygma/keyboardManager/KeyboardManager.cpp`

That is the direct reason Hypercolor frame writes are silent on stock firmware.

### 2.3 Raw side LED commands are explicitly unfinished upstream

The side protocol already reserves LED-oriented command space:

- `~/dev/NeuronWireless_defy/libraries/Communications/src/Communications_protocol.h`

But the protocol docs mark `LED` and `LED_BANK` as `TBD`:

- `~/dev/NeuronWireless_defy/libraries/Communications/docs/docs.md`

So the missing functionality is real, not a Hypercolor bug.

### 2.4 Bank sizing constrains the internal wire format

Each side uses 11 banks of 8 LEDs:

- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/device/dygma/keyboardManager/universalModules/Hand.h`

The communications payload budget is 28 bytes:

- `~/dev/NeuronWireless_defy/libraries/Communications/src/Communications_protocol.h`

Implication:

- 8 LEDs * RGBW = 32 bytes -> does **not** fit in one packet
- 8 LEDs * RGB = 24 bytes -> **does** fit in one packet

This spec therefore uses **RGB banks on the internal side protocol**, with RGBW derivation optional on the keyscanner side.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- Enable live non-persistent Hypercolor streaming on Defy.
- Support both Defy wired and Defy wireless custom firmware trees.
- Preserve stock Bazecor behavior for palette, colormap, and brightness.
- Avoid EEPROM / flash writes during live streaming.
- Sustain at least 10 fps full-board updates at 176 LEDs.
- Keep the host-side protocol self-identifying so Hypercolor can distinguish stock and custom firmware cleanly.

### 3.2 Non-Goals

- Backporting live streaming to stock Defy firmware without flashing custom firmware.
- Replacing Bazecor's persistence-oriented palette / colormap workflow.
- Exact RGBW transport from host to sides in v1.
- Supporting BLE transport for live Hypercolor frames in v1.
- Designing a generic Dygma-wide protocol for non-Defy devices.

---

## 4. Protocol Architecture

### 4.1 Capability Discovery via Focus

Add a new Focus command:

```text
hypercolor.capabilities
```

Response example:

```text
stream_v1 rgb 176 10
```

Fields:

- protocol tag: `stream_v1`
- frame format: `rgb`
- LED count: `176`
- target fps: `10`

Rationale:

- safe to probe on the stock text channel
- easy for Hypercolor to detect custom firmware support
- does not require Bazecor changes

If the command is absent, Hypercolor treats Defy as non-streaming.

### 4.2 Binary Host <-> Neuron Streaming Protocol

After capability discovery, Hypercolor switches from Focus text writes to a custom binary serial protocol.

#### 4.2.1 Packet Framing

All packets begin with a non-ASCII sentinel so they cannot be confused with Focus commands:

```text
Offset  Size  Field
0       1     0xFF sentinel
1       1     'H'
2       1     'C'
3       1     version = 0x01
4       1     opcode
5       1     sequence
6       2     payload_len_le
8       2     crc16_le(header[1..8] + payload)
10      N     payload
```

#### 4.2.2 Opcodes

| Opcode | Name | Direction | Purpose |
|--------|------|-----------|---------|
| `0x01` | `CapabilitiesRequest` | Host -> Neuron | Optional binary probe |
| `0x02` | `CapabilitiesResponse` | Neuron -> Host | Mirrors Focus capability data |
| `0x10` | `FrameRgb176` | Host -> Neuron | Full 176-LED RGB frame |
| `0x11` | `Ack` | Neuron -> Host | Sequence acknowledgment |
| `0x12` | `Error` | Neuron -> Host | CRC / shape / unsupported error |

#### 4.2.3 `FrameRgb176` Payload

```text
Offset  Size  Field
0       2     led_count_le = 176
2       176*3 RGB triples in Defy physical order
```

Physical order is the existing Hypercolor Defy logical order:

1. left keys (35)
2. right keys (35)
3. left underglow (53)
4. right underglow (53)

The two reserved neuron slots are excluded from the host frame.

#### 4.2.4 Reliability Model

- Host sends one frame packet.
- Neuron validates CRC and payload length.
- Neuron responds with `Ack(sequence)` after the frame has been accepted into its staging buffer.
- Lost or malformed frames are dropped; latest-wins semantics are acceptable.

This is a streaming protocol, not a transactional config channel.

#### 4.2.5 Stream Session Rules

- Streaming becomes active after the first valid `FrameRgb176`.
- The neuron records `last_stream_at` for every accepted frame.
- If no valid frame arrives for `500 ms`, the neuron exits streaming mode and restores the current built-in LED mode.
- Receiving Focus persistence commands while streaming is allowed, but those commands must not repaint the sides until streaming has timed out or been explicitly cleared.

The timeout is deliberately short enough to recover quickly when Hypercolor exits, but long enough to tolerate occasional host-side scheduling jitter at 10 fps.

### 4.3 Internal Neuron <-> Keyscanner Protocol

Add a new internal command to `Communications_protocol::Commands`:

```cpp
HYPERCOLOR_LED_BANK
```

Payload shape:

```text
Offset  Size  Field
0       1     bank_index (0..10)
1       1     flags
2       24    8 RGB triples
```

Flags:

- bit 0: `apply_now`
- bit 1: `compute_white`

The `device` field in the packet header already targets left or right side, so no side byte is required in the payload.

Why banked RGB:

- 24 RGB bytes + 2 metadata bytes = 26 bytes
- safely fits under the 28-byte payload limit
- avoids fragmentation in v1

Neuron sends 22 bank packets per frame:

- 11 to left keyscanner
- 11 to right keyscanner

Bank mapping is fixed:

- host LEDs `0..87` map to left-hand banks `0..10`
- host LEDs `88..175` map to right-hand banks `0..10`

---

## 5. Firmware Changes

### 5.1 Serial Dispatcher

Extend the neuron serial handling so binary Hypercolor packets are recognized before Focus text dispatch.

Target files:

- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/plugin/FocusSerial.cpp`
- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/plugin/FocusSerial.h`

Required behavior:

- peek first byte
- if first byte is `0xFF`, parse a Hypercolor binary packet
- otherwise continue through normal Focus command handling

### 5.2 Neuron Frame Staging and Forwarding

Add a neuron-side frame buffer:

```cpp
struct HypercolorFrameState {
  uint8_t seq;
  bool valid;
  uint8_t rgb[176][3];
};
```

Responsibilities:

- accept validated `FrameRgb176` packets
- split into left/right 8-LED banks
- send `HYPERCOLOR_LED_BANK` packets to the keyscanners
- rate-limit forwarding to the negotiated target fps if needed

Target files:

- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/device/dygma/keyboardManager/KeyboardManager.cpp`
- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/device/dygma/defyWN/DefyWN.cpp`
- `~/dev/NeuronWireless_defy/libraries/Communications/src/device/neuron/defyN2/CommunicationsN2.cpp`
- `~/dev/NeuronWireless_defy/libraries/Communications/src/device/neuron/defyWN/CommunicationsWN.cpp`

### 5.3 Keyscanner Receive Path

Implement `HYPERCOLOR_LED_BANK` receive handling in the keyscanner communications layer.

Target file:

- `~/dev/NeuronWireless_defy/libraries/Communications/src/device/keyScanner/CommunicationsKS.cpp`

Required behavior:

- decode bank index
- copy 8 RGB colors into the appropriate bank in the side LED buffer
- optionally derive white channel when `compute_white` is set
- mark LED state dirty
- apply on `apply_now`

The apply path must be non-persistent and must not touch EEPROM-backed palette / layer storage.

### 5.4 Focus Capability Command

Add `hypercolor.capabilities` to the Focus command set.

Target file:

- `~/dev/NeuronWireless_defy/libraries/Kaleidoscope/src/kaleidoscope/plugin/LEDControlDefy.cpp`

This command only advertises support. It does not carry frame data.

### 5.5 Streaming Ownership Rules

While Hypercolor streaming is active:

- neuron should suppress palette/colormap refreshes from overwriting the live frame
- normal built-in mode updates may remain selected logically, but must not repaint the sides until the stream is released

Recommended approach:

- introduce a `hypercolor_stream_active` flag
- when set, `syncLeds()` and LED-mode refresh paths treat live stream state as authoritative

When the stream times out or disconnects:

- clear `hypercolor_stream_active`
- restore current LED mode via existing `LEDControl.set_mode(...)`

### 5.6 Timeout and Recovery

Recommended neuron-side state:

```cpp
struct HypercolorStreamLease {
  bool active;
  uint8_t last_seq;
  uint32_t last_frame_at_ms;
};
```

Recommended behavior:

- ignore duplicate `sequence` values
- accept out-of-order newer frames on a latest-wins basis
- on timeout, clear the lease and repaint from the active firmware LED mode
- on malformed packets, leave the current displayed frame untouched and return `Error`

---

## 6. Hypercolor Changes

### 6.1 HAL Capability Probe

The Dygma driver should:

1. run normal identity probes
2. send `hypercolor.capabilities`
3. if present and valid, switch to a custom-firmware streaming variant
4. otherwise treat Defy as non-direct

The stock-firmware behavior added in Hypercolor remains the fallback:

- `supports_direct = false`
- no live frame writes

### 6.2 New Protocol Variant

Add a custom protocol variant in `hypercolor-hal`:

```rust
enum DygmaVariant {
    DefyWired,
    DefyWireless,
    DefyWiredHypercolorV1,
    DefyWirelessHypercolorV1,
}
```

Or equivalently keep the public variant enum stable and store a runtime transport mode in the protocol state.

### 6.3 Encoder

For custom firmware:

- `encode_frame()` emits a single `FrameRgb176` binary packet
- `expects_response = true`
- response parser accepts `Ack` / `Error`

Brightness commands may remain on the Focus text channel unless later migrated.

The host should not pipeline multiple outstanding frames in v1. One frame in flight keeps the firmware implementation simple and bounds serial buffering.

### 6.4 Discovery and API Surface

Device info should report:

- `supports_direct = true` only when custom streaming firmware is detected
- `max_fps = 10`
- same 4 Defy zones as today

Optional:

- expose firmware capability string in daemon metadata for debugging

### 6.5 Runtime Policy

Recommended host policy:

- capability probe once per fresh serial connection
- stream only after explicit positive capability detection
- wait up to `250 ms` for `Ack`
- if `Ack` times out or `Error` is returned, downgrade the device to disconnected / failed-stream state and stop sending frames until rediscovery

This avoids silently flooding the CDC port when the custom firmware is absent or unhealthy.

---

## 7. Performance Budget

### 7.1 Host <-> Neuron serial bandwidth

Full RGB frame:

- 176 LEDs * 3 bytes = 528 bytes
- header + CRC ~= 10 bytes
- total ~= 538 bytes / frame

At 10 fps:

- ~= 5.4 KB/s

115200 baud CDC serial provides roughly 11.5 KB/s practical payload budget, so this is acceptable.

### 7.2 Neuron <-> Keyscanner bandwidth

Per frame:

- 22 bank packets
- each packet = 26 bytes payload + 4-byte header = 30 bytes on the internal side protocol
- ~= 660 bytes / frame

At 10 fps:

- ~= 6.6 KB/s aggregate internal traffic

This is materially better than attempting full-board ASCII Focus writes.

---

## 8. Compatibility and Rollout

### 8.1 Stock Firmware Compatibility

Stock Defy firmware remains unsupported for direct Hypercolor streaming.

Hypercolor must not send binary stream packets unless:

- `hypercolor.capabilities` exists
- the response explicitly advertises `stream_v1`

### 8.2 Bazecor Compatibility

Bazecor should continue to work unchanged because:

- palette / colormap / brightness Focus commands remain intact
- the new stream protocol is opt-in and capability-gated

### 8.3 Flashing Scope

This feature requires flashing all participating Defy firmware components:

- neuron firmware
- left keyscanner firmware
- right keyscanner firmware

Shipping only the neuron change is insufficient because the keyscanner side must understand the new bank packet.

---

## 9. Testing Strategy

### 9.1 Firmware-level

- `hypercolor.capabilities` returns the expected descriptor.
- binary parser rejects bad CRC and bad lengths.
- one `FrameRgb176` updates all 176 LEDs without EEPROM writes.
- timeout restores normal LED mode behavior.
- unplug / reconnect re-enters non-streaming mode cleanly.

### 9.2 Hypercolor HAL

- stock firmware path keeps `supports_direct = false`
- custom capability response upgrades to streaming mode
- `encode_frame()` emits one binary packet of expected length
- response parser handles `Ack` and `Error`
- brightness commands still work for both wired and wireless variants

### 9.3 End-to-end

- wired Defy custom firmware streams live Hypercolor frames at 10 fps
- wireless Defy custom firmware streams live Hypercolor frames at 10 fps over USB-connected neuron
- disconnect during stream does not wedge the serial parser
- exiting Hypercolor restores normal keyboard LED mode

---

## 10. Recommended First Milestone

Implement v1 in this order:

1. `hypercolor.capabilities` Focus command
2. binary host <-> neuron parser with `FrameRgb176`
3. `HYPERCOLOR_LED_BANK` internal side command
4. keyscanner bank apply path
5. Hypercolor HAL custom-firmware detection and binary encoder

That sequence gives an incremental path where every stage is testable in isolation.
