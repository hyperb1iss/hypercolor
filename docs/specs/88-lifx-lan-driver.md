# Spec 88: LIFX LAN Driver

> Network driver for LIFX lights over the official LIFX LAN protocol. Covers
> the 36-byte header, discovery, capability resolution from the product
> registry, real-time output for single-zone bulbs, legacy and extended
> multizone strips, and matrix devices (Tile chains, Candle, Ceiling, Tube,
> Spot, Path, Luna, Mirror), HSBK color conversion, the device message budget,
> and how device topology becomes spatial layout zones.

**Status:** Implemented, experimental (unverified on hardware)
**Crate:** `hypercolor-driver-lifx`
**Module path:** `hypercolor_driver_lifx`
**Feature flag:** `hypercolor-driver-builtin/lifx`, part of the default `network` bundle
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Packet Format](#2-packet-format)
3. [Protocol Architecture](#3-protocol-architecture)
4. [Discovery and Query Messages](#4-discovery-and-query-messages)
5. [Output Messages](#5-output-messages)
6. [Topology Messages](#6-topology-messages)
7. [Topology to Spatial Layout](#7-topology-to-spatial-layout)
8. [Color Encoding](#8-color-encoding)
9. [Timing and Message Budget](#9-timing-and-message-budget)
10. [Product Registry](#10-product-registry)
11. [Driver Integration](#11-driver-integration)
12. [Testing Strategy](#12-testing-strategy)
13. [Risks and Open Questions](#13-risks-and-open-questions)
- [References](#references)

---

## 1. Overview

LIFX sells Wi-Fi lights that speak a documented binary protocol over UDP on
the local network. Every product answers the same header and the same
discovery and query messages; what differs is how many zones a device has
and which message family paints them:

| Device class | Examples | Zones | Paint message |
|---|---|---|---|
| Single zone | A19, BR30, GU10, Mini, Downlight, PAR38 | 1 | SetColor (102) |
| Legacy multizone | first generation LIFX Z, LIFX Z and Beam on firmware before 2.77 | up to 255 | SetColorZones (501) |
| Extended multizone | LIFX Z, Beam, Neon, String, Lightstrip, Permanent Outdoor | any | SetExtendedColorZones (510) |
| Matrix | Tile chain, Candle, Ceiling, Tube, Spot, Path, Luna, Mirror | width x height per tile | Set64 (715), plus CopyFrameBuffer (716) above 64 zones |

The driver discovers lights with a broadcast GetService (or unicast to
configured addresses), reads version, firmware, label, and topology, builds a
`DeviceInfo` whose segments mirror the physical zones, and streams frames as
unacknowledged Set messages. The engine stays RGB; the driver converts each
pixel to HSBK at the edge.

Sources: the official LIFX LAN documentation at lan.developer.lifx.com (cited
directly, it is vendor documentation), the LIFX product registry
(github.com/LIFX/products, facts only, see section 10), the legacy LIFX V2
protocol documentation for the message rate guideline, and the
lifx-emulator project, used only as a test peer. No third-party driver code
was read or copied.

The driver is **experimental** in the sense that nobody has run it on real
LIFX hardware yet. Experimental is a label, not a gate: like the Hue,
Nanoleaf, and WLED drivers, the module is enabled by default, and the label
is carried by the driver descriptor once the shared stability vocabulary
lands (Spec 89).

---

## 2. Packet Format

All multi-byte numbers are little-endian. A message is one UDP datagram: a
36-byte header followed by a type-specific payload. Fields named reserved are
sent as zero and ignored on receive (real devices put data there; the
documented StateLabel reply carries `LIFXV2` in the frame address reserved
bytes).

### 2.1 Frame header (bytes 0 to 7)

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 2 | size | header + payload | Total message length in bytes |
| 2 | 2 | flags | see below | protocol, addressable, tagged, origin packed in a u16 |
| 4 | 4 | source | client chosen, not 0 or 1 | Echoed in every reply and ack |

Flags as a little-endian u16:

| Bits | Field | Value |
|------|-------|-------|
| 0-11 | protocol | 1024 |
| 12 | addressable | 1 |
| 13 | tagged | 1 when the target is all zeros (discovery), else 0 |
| 14-15 | origin | 0 |

So unicast frames carry bytes `00 14` at offsets 2-3 and discovery frames
carry `00 34`.

Source 0 can make a device broadcast its reply to port 56700 instead of
answering the sender, and some firmware ignores source 1. The driver picks a
random source in `2..=u32::MAX` per client socket.

### 2.2 Frame address (bytes 8 to 23)

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 8 | 8 | target | serial + 2 zero bytes | Six-byte device serial (the device MAC, `d073d5xxxxxx`) left aligned; all zeros addresses every device |
| 16 | 6 | reserved | 0 | Ignored on receive |
| 22 | 1 | flags | bit 0 res_required, bit 1 ack_required | Bits 2-7 reserved on send (bit 3 is a Thread hint on receive) |
| 23 | 1 | sequence | wraps 255 to 0 | Incremented per message; replies echo it |

### 2.3 Protocol header (bytes 24 to 35)

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 24 | 8 | reserved | 0 | Ignored on receive |
| 32 | 2 | type | message type | u16 |
| 34 | 2 | reserved | 0 | |

### 2.4 Reply matching

A reply belongs to a request when its `source`, `sequence`, and `target`
match (for discovery the target is the replying device's serial, so only
source and sequence are compared). Devices reply to the address and port the
request came from, so a client socket bound to an ephemeral port receives
its own replies. Get messages always produce their State reply;
`res_required` only matters for Set messages. Set64 and SetUserPosition never
reply.

### 2.5 Worked examples from the documentation

Used verbatim as golden vectors:

| Example | Bytes (hex) |
|---|---|
| SetColor 102, source 2, sequence 1, ack_required, target d073d5001337, HSBK (21845, 65535, 65535, 3500), duration 0 | `3100001402000000d073d500133700000000000000000201000000000000000066000000005555ffffffffac0d00000000` |
| GetPower 20 header, source 0x46470300 as bytes `00 03 47 46`, res and ack required (plus one reserved bit) | `2400001400034746d073d500133700000000000000000701000000000000000014000000` |
| StateLabel 25 reply, label `cupboard` | `4400001487454e9ed073d5309d9e00004c49465856320101d078582cef7d010019000000637570626f617264000000000000000000000000000000000000000000000000` |

The GetPower example sets reserved bit 2 of byte 22, which is why decoders
must not reject reserved bits. The driver's own encoder emits `0x03` for
res and ack.

---

## 3. Protocol Architecture

### 3.1 Capability resolution

The device class comes from three reads, not from the product name:

1. `StateVersion` (33) gives `vendor` and `product`. Vendor 1 is LIFX.
2. `StateHostFirmware` (15) gives `(major, minor)`. Versions compare as that
   tuple: (2, 80) < (3, 60) < (3, 70).
3. The product registry entry (section 10) gives the base features and the
   firmware threshold, if any, at which extended multizone appears.

| Registry facts | Firmware | Class |
|---|---|---|
| matrix | any | Matrix |
| multizone, extended always | any | Extended multizone |
| multizone, extended from (a, b) | >= (a, b) | Extended multizone |
| multizone, extended from (a, b) | < (a, b) | Legacy multizone |
| multizone, extended never | any | Legacy multizone |
| color or white only | any | Single zone |
| no temperature range (switches) | any | Not a light; ignored |
| unknown product id | any | Single zone (SetColor is understood by every LIFX light) |
| vendor other than 1 | any | Ignored |

Only two products gain extended multizone through an upgrade: product 32
(LIFX Z) and product 38 (LIFX Beam), both at firmware 2.77. Products 31 and
300 never have it. Every product still accepts the legacy messages.

### 3.2 Zone counts come from the device

The registry has no zone counts or matrix sizes. Strip length comes from
`zones_count` in StateExtendedColorZones (512) or StateMultiZone (506) /
StateZone (503). Matrix size comes from the per-tile `width` and `height` in
StateDeviceChain (702). Product names such as "Ceiling 13x26" are never
parsed for geometry (its uplight sits at x = 15, outside a width of 13).

### 3.3 Messages the driver uses

| Type | Name | Direction | Purpose |
|---|---|---|---|
| 2 | GetService | send | Discovery |
| 3 | StateService | receive | Discovery reply |
| 14 | GetHostFirmware | send | Capability resolution |
| 15 | StateHostFirmware | receive | |
| 23 | GetLabel | send | Device name |
| 25 | StateLabel | receive | |
| 32 | GetVersion | send | Capability resolution |
| 33 | StateVersion | receive | |
| 45 | Acknowledgement | receive | Confirms SetLightPower at connect |
| 102 | SetColor | send | Single-zone output |
| 117 | SetLightPower | send | Power the light on at connect |
| 223 | StateUnhandled | receive | Some firmware answers unknown types with this |
| 501 | SetColorZones | send | Legacy multizone output |
| 502 | GetColorZones | send | Legacy zone count |
| 503 | StateZone | receive | |
| 506 | StateMultiZone | receive | |
| 510 | SetExtendedColorZones | send | Extended multizone output |
| 511 | GetExtendedColorZones | send | Extended zone count |
| 512 | StateExtendedColorZones | receive | |
| 701 | GetDeviceChain | send | Matrix geometry |
| 702 | StateDeviceChain | receive | |
| 715 | Set64 | send | Matrix output |
| 716 | CopyFrameBuffer | send | Matrix output above 64 zones per tile |

---

## 4. Discovery and Query Messages

### 4.1 GetService (2) and StateService (3)

GetService has an empty payload. For discovery it is sent with tagged = 1 and
an all-zero target, either as a UDP broadcast to port 56700 or unicast to a
configured address. Every device behind that address answers with one
StateService per service it offers.

StateService payload, 5 bytes:

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | service | 1 = UDP | Values 2-5 are reserved; ignore them |
| 1 | 4 | port | usually 56700 | Port for that service; 0 means temporarily unavailable |

The device address is the reply's source IP plus the advertised UDP port.
The device identity is the header `target` of the reply (its serial). Several
devices can sit behind one address and port (the emulator does this), so the
driver keys devices by serial, never by address.

The documentation's GetService entry once says tagged = 0, which contradicts
every other page; the driver sends 1.

The documentation gives no discovery frequency guidance. The driver sends one
GetService per target per scan, and scans only when the host asks.

### 4.2 Version, firmware, label

| Type | Name | Payload | Size |
|---|---|---|---|
| 32 | GetVersion | empty | 0 |
| 33 | StateVersion | vendor u32 @0, product u32 @4, reserved 4 @8 | 12 |
| 14 | GetHostFirmware | empty | 0 |
| 15 | StateHostFirmware | build u64 @0, reserved 8 @8, version_minor u16 @16, version_major u16 @18 | 20 |
| 23 | GetLabel | empty | 0 |
| 25 | StateLabel | label, 32 bytes @0 | 32 |

The label is UTF-8, NUL terminated unless it fills all 32 bytes. Decoding
stops at the first NUL and replaces invalid UTF-8.

### 4.3 Acknowledgement (45) and StateUnhandled (223)

Acknowledgement has an empty payload and echoes source, sequence, and target.
It is only sent when ack_required is set, just before the device processes
the message. StateUnhandled carries `unhandled_type: u16` and means the
device does not implement the requested type; the driver treats it as a
negative answer rather than waiting for a timeout.

### 4.4 Request policy

Probing a device issues GetVersion, GetHostFirmware, GetLabel, and one
topology request. Each request is retried up to three times with a 300 ms
reply timeout, since UDP drops packets. Probes of different devices run
concurrently.

---

## 5. Output Messages

Every output message is unicast, tagged = 0, target = the device serial,
res_required = 0, ack_required = 0, so each new frame supersedes the last
instead of generating reply traffic. Only SetLightPower at connect asks for
an acknowledgement.

### 5.1 HSBK

Eight bytes, used by every color message:

| Offset | Size | Field | Range |
|--------|------|-------|-------|
| 0 | 2 | hue | 0-65535 over 0-360 degrees, `round(65536 * deg / 360) mod 65536` |
| 2 | 2 | saturation | 0-65535 over 0-1 |
| 4 | 2 | brightness | 0-65535 over 0-1 |
| 6 | 2 | kelvin | product temperature range, for example 1500-9000 |

Kelvin only affects the white point at low saturation.

### 5.2 SetLightPower (117)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 2 | level | 0 off, 65535 on |
| 2 | 4 | duration | transition in ms |

Sent once at connect with level 65535, duration 0, ack_required = 1, so a
light that was switched off shows the stream. The acknowledgement doubles as
the reachability check. Nothing is sent at disconnect.

### 5.3 SetColor (102)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 1 | reserved | 0 |
| 1 | 8 | color | HSBK |
| 9 | 4 | duration | transition in ms |

Payload 13 bytes, message 49 bytes. On a multizone or matrix device it paints
every zone; the driver only uses it for single-zone lights.

### 5.4 SetColorZones (501), legacy multizone

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 1 | start_index | first zone, inclusive |
| 1 | 1 | end_index | last zone, inclusive |
| 2 | 8 | color | HSBK |
| 10 | 4 | duration | transition in ms |
| 14 | 1 | apply | 0 NO_APPLY, 1 APPLY, 2 APPLY_ONLY |

Payload 15 bytes, message 51 bytes. A per-zone frame is a run of these: every
message but the last uses NO_APPLY (buffered on the device), the last uses
APPLY (commits the buffer). The driver merges runs of identical adjacent
colors into one start..end message, so a solid frame costs one message and a
fully varied N-zone frame costs N. Zone 0 is nearest the controller.

### 5.5 SetExtendedColorZones (510)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 4 | duration | transition in ms |
| 4 | 1 | apply | 0 NO_APPLY, 1 APPLY, 2 APPLY_ONLY |
| 5 | 2 | zone_index | first zone this message covers |
| 7 | 1 | colors_count | meaningful entries, at most 82 |
| 8 | 656 | colors | 82 HSBK, unused entries zero |

Payload 664 bytes, message 700 bytes. Strips longer than 82 zones take
`ceil(zones / 82)` messages with zone_index 0, 82, 164, and so on; all but the
last use NO_APPLY and the last uses APPLY.

### 5.6 Set64 (715)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 1 | tile_index | chain position, 0 nearest the controller |
| 1 | 1 | length | tiles to apply to, starting at tile_index; the driver sends 1 |
| 2 | 1 | fb_index | frame buffer, 0 is visible |
| 3 | 1 | x | start column |
| 4 | 1 | y | start row |
| 5 | 1 | width | row width of the written rectangle (the tile width) |
| 6 | 4 | duration | transition in ms |
| 10 | 512 | colors | 64 HSBK, row major |

Payload 522 bytes, message 558 bytes. The 64 colors fill `64 / width` rows
starting at row y. All tiles of a chain are addressed through the chain's
single IP and serial with different tile_index values.

### 5.7 CopyFrameBuffer (716)

| Offset | Size | Field | Value |
|--------|------|-------|-------|
| 0 | 1 | tile_index | chain position |
| 1 | 1 | length | 1 |
| 2 | 1 | src_fb_index | 1 |
| 3 | 1 | dst_fb_index | 0 |
| 4 | 1 | src_x | 0 |
| 5 | 1 | src_y | 0 |
| 6 | 1 | dst_x | 0 |
| 7 | 1 | dst_y | 0 |
| 8 | 1 | width | tile width |
| 9 | 1 | height | tile height |
| 10 | 4 | duration | transition in ms, only used when dst is 0 |
| 14 | 1 | reserved | 0 |

Payload 15 bytes, message 51 bytes.

### 5.8 Matrix frame plan

For each tile, in chain order:

| Tile zones | Frame buffers (tile field) | Messages |
|---|---|---|
| width x height <= 64 | any | one Set64 to fb 0 at (0, 0), duration = transition |
| more than 64 | supported_frame_buffers > 1 | `ceil(height / (64 / width))` Set64 to fb 1, stepping y by `64 / width`, duration 0, then one CopyFrameBuffer fb 1 to fb 0 over the whole tile, duration = transition |
| more than 64 | 0 or 1 | the same Set64 run written straight to fb 0 (may tear between messages) |

The documentation describes the hidden-buffer approach in one paragraph on
CopyFrameBuffer, naming the 26 inch Ceiling; the exact message sequence is
this driver's reading of it. A `supported_frame_buffers` of 0 is treated as
one buffer: older protocol descriptions call that byte reserved, and the
lifx-emulator project leaves it zero.

---

## 6. Topology Messages

### 6.1 Legacy multizone count

GetColorZones (502) payload: `start_index u8 @0, end_index u8 @1`. The driver
asks for 0..255 and reads `zones_count` from the first reply:

| Type | Name | Payload | Size |
|---|---|---|---|
| 503 | StateZone | zones_count u8 @0, zone_index u8 @1, color HSBK @2 | 10 |
| 506 | StateMultiZone | zones_count u8 @0, zone_index u8 @1, colors 8 x HSBK @2 | 66 |

Further 506 replies for the same request are ignored.

### 6.2 Extended multizone count

GetExtendedColorZones (511) has an empty payload.

| Type | Name | Payload | Size |
|---|---|---|---|
| 512 | StateExtendedColorZones | zones_count u16 @0, zone_index u16 @2, colors_count u8 @4, colors 82 x HSBK @5 | 661 |

`zones_count` is the device total even when several 512 replies arrive.

### 6.3 Device chain

GetDeviceChain (701) has an empty payload. StateDeviceChain (702), 882 bytes:

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 1 | start_index | first chain index described |
| 1 | 880 | tile_devices | 16 Tile records of 55 bytes |
| 881 | 1 | tile_devices_count | how many records are real |

Tile record, 55 bytes:

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 2 | accel_meas_x | i16, orientation |
| 2 | 2 | accel_meas_y | i16 |
| 4 | 2 | accel_meas_z | i16 |
| 6 | 2 | reserved | |
| 8 | 4 | user_x | f32, tile center, unit = one tile width |
| 12 | 4 | user_y | f32 |
| 16 | 1 | width | zones per row |
| 17 | 1 | height | rows |
| 18 | 1 | supported_frame_buffers | count of frame buffers |
| 19 | 4 | device_version_vendor | u32 |
| 23 | 4 | device_version_product | u32 |
| 27 | 4 | reserved | |
| 31 | 8 | firmware_build | u64 |
| 39 | 8 | reserved | |
| 47 | 2 | firmware_version_minor | u16 |
| 49 | 2 | firmware_version_major | u16 |
| 51 | 4 | reserved | |

Tile chains hold up to 5 tiles; every other matrix product reports one.
Records with width or height 0 are rejected.

### 6.4 Orientation

From the accelerometer triple `(x, y, z)`:

| Condition (checked in order) | Orientation |
|---|---|
| x, y, z all equal -1 | invalid reading, treat as right side up |
| abs(x) > abs(y) and abs(x) > abs(z) | x > 0: rotated right, else rotated left |
| abs(z) > abs(x) and abs(z) > abs(y) | z > 0: face down, else face up |
| otherwise | y > 0: upside down, else right side up |

For an upright image pixel (x, y) on a W x W tile the documentation sends the
color to device zone:

| Orientation | Device zone (dx, dy) |
|---|---|
| right side up, face up, face down | (x, y) |
| upside down | (W-1-x, W-1-y) |
| rotated left | (W-1-y, x) |
| rotated right | (y, W-1-x) |

The driver keeps colors in device order and instead inverts this map to place
each device zone at its physical position (section 7.3). Orientation is only
applied to square tiles.

---

## 7. Topology to Spatial Layout

The engine samples the canvas at LED positions per segment; the driver's job
is to describe segments whose LED order matches the frame it will encode.
LED order inside a frame is always the concatenation of segments.

### 7.1 Single zone

One segment `Light`, 1 LED, `Point` topology.

### 7.2 Multizone

One segment `Zones`, N LEDs, `Strip` topology, zone 0 first.

### 7.3 Matrix

| Device | Segments |
|---|---|
| One tile, no quirk | `Matrix`: W x H LEDs, `Matrix { rows: H, cols: W }` |
| Chain of several tiles | `Tiles`: sum of W x H LEDs, `Custom` topology with a layout hint carrying per-LED positions |
| Candle quirk (5 x 6 tile) | `Candle`: 26 LEDs, `Custom` positions |
| Uplight quirk (registry uplight cell) | `Downlight`: W x H - 1 LEDs, `Custom` positions, then `Uplight`: 1 LED, `Point`, co-located |

Chain positions: each tile's center is `(user_x, -user_y)` in tile widths
(user_y is taken to grow upward, so it is negated for the canvas, which grows
downward). A device zone (dx, dy) is mapped back to its upright position
(x, y) by inverting the orientation table, and placed at
`center + ((x + 0.5) / W - 0.5, (y + 0.5) / W - 0.5 * H / W)`. All positions
are then normalized into [0, 1] over the chain's bounding box, and the hint's
preferred zone size keeps the chain's aspect ratio. When every tile reports
the same center (positions never set in the LIFX app), tiles are laid out
left to right in chain order instead.

Candle: the documentation describes 26 zones in a 5 x 6 matrix, five columns
of five plus one top zone, and says only the first of the first row's five
colors drives the top zone. The driver maps LED 0 to device zone (0, 0)
placed at the top center, and LEDs 1-25 to rows 1-5 as a 5 x 5 grid below
it. Device zones (1..4, 0) are written black.

Uplight: the registry names one matrix cell that drives the ceiling
fixture's uplight (x 7, y 7 on the 8 x 8 Ceiling; x 15, y 7 on the 16 x 8
Ceiling 13x26). That cell gets its own `Uplight` point segment instead of
being sampled from a corner of the downlight grid.

---

## 8. Color Encoding

The engine hands the driver sRGB-encoded RGB bytes (the device path applies
no extra gamma; see `docs/color-science-led-guide.md`, section 9). The
conversion per pixel:

| HSBK field | Derivation |
|---|---|
| hue | HSV hue of the sRGB-encoded bytes, so the hue wheel matches what every LIFX client sends |
| saturation | `1 - min / max` over the **linear-light** channels |
| brightness | max of the sRGB-encoded channels, times the device brightness scale |
| kelvin | configured white point (default 6500 K, the sRGB D65 white), clamped to the product's base temperature range |

Saturation is the share of the light that is chromatic rather than white.
Measuring it in linear light keeps a pastel's chroma: sRGB (255, 128, 128)
has encoded HSV saturation 0.50 but is 78 percent red light, and sending 0.50
would mix in far more white than the source pixel holds. Linear saturation
is never lower than encoded saturation, and any pixel with a zero channel is
fully saturated (65535), so kelvin, which only matters at low saturation,
can never wash out a saturated color. Pure greys and whites get saturation 0
and render at the configured kelvin. Black is (0, 0, 0, kelvin).

White-only products (no color feature) ignore hue and saturation, so the
driver sends saturation 0 and brightness from the pixel's relative luminance
(linear Y, re-encoded to sRGB), which keeps a saturated blue pixel dim
instead of full white.

Device brightness from the host (`set_brightness`) scales the HSBK
brightness field directly rather than the RGB bytes, so hue and saturation
keep full precision at low brightness.

---

## 9. Timing and Message Budget

### 9.1 The device constraint

The current LIFX documentation gives no number, only a warning that
"excessive message rates" cause unexpected behavior. The one figure LIFX has
published is in its legacy V2 protocol documentation: a maximum recommended
transmit rate of 20 messages per second to a device. The driver treats that
as the default per-device budget (`max_messages_per_second = 20`), exposes it
as an explicit user control, and never changes the render loop's own rate.

### 9.2 Messages per frame

| Class | Messages per frame | Frames per second at 20 msg/s |
|---|---|---|
| Single zone | 1 | 20 |
| Extended multizone, N zones | ceil(N / 82) | 20 for N <= 82 |
| Legacy multizone, N zones | one per run of equal adjacent colors, 1 to N | 20 for a solid frame; a frame of N different zones takes N / 20 s |
| Matrix, per tile <= 64 zones | 1 per tile | 20 / tiles (5-tile chain: 4) |
| Matrix, tile > 64 zones with spare buffer | ceil(H / (64 / W)) + 1 | 16 x 8 Ceiling: 3 messages, 6.6 |

### 9.3 A budget charged per message

The budget is charged per datagram as it is sent, whichever frame it belongs
to. It is a token bucket in its virtual-scheduling form (GCRA): each message
moves a theoretical arrival time forward by one message interval
(1 / `max_messages_per_second`), and a message may leave once that time,
less a burst tolerance of four intervals, has arrived. Up to five messages
can therefore leave back to back (one full frame of a five-tile chain, so
all tiles change together); after that every message waits its interval. In
any window of length L at most `5 + L × max_messages_per_second` messages
leave, and the long-run rate is the budget. With a budget below five
messages per second the burst shrinks to the budget.

A frame costs what its own messages cost. A solid legacy strip frame is one
message and runs at the full 20 per second; a frame of 80 different zones is
80 messages and takes about 3.8 s, and nothing else goes out meanwhile.

### 9.4 Coalescing, never queueing

The host runs one latest-value output queue per device (Spec 51 section 16):
while a device waits for its cadence slot, newer frames replace older ones
and only the newest is offered. The driver reports a cadence whose minimum
interval is the cheapest frame's cost (one message interval for bulbs and
legacy strips, `tiles` intervals for a chain), so the host never offers
frames faster than the device can take its cheapest one.

Waiting for the budget only ever happens where the frame is still
replaceable:

1. When the host offers a frame and the budget has no room for its first
   message, the sink declines it at once with `SuppressedCadence`. Nothing is
   encoded or sent, the frame stays in the host's slot (where a newer frame
   can replace it), and the host offers the newest frame again after one
   cadence interval.
2. Once a frame's first message leaves, its remaining messages are paced by
   the budget inside the same delivery. The frame is committed at that point
   (its earlier zones are already on the wire), and frames pushed meanwhile
   keep replacing each other in the host's slot; only the newest follows.

Direct writes outside the queue (identify flashes, temporary direct control)
have no slot behind them, so they wait for the budget instead of declining.

### 9.5 Reassertion

UDP output carries no acknowledgement, so a dropped packet would leave a
static scene wrong forever. The cadence asks the host to resend the cached
frame after one second of silence (or two frame intervals, whichever is
longer), which also repaints a light that was power cycled or changed from
another app.

### 9.6 Transition

`duration` on every color message comes from `transition_ms` (default 0, an
immediate change). A transition near the frame interval lets the light
interpolate between low-rate frames, at the cost of that much latency.

---

## 10. Product Registry

The LIFX products repository has no license file and its README reserves all
rights under the LIFX Developer Terms. The driver therefore does not vendor
`products.json`. It encodes only facts (product id, name, light class,
extended multizone rule, base kelvin range, uplight cell) as its own Rust
table, plus one Hypercolor-derived quirk flag for the Candle Color products.
Unknown product ids fall back to single zone with a 1500-9000 K range, so a
new bulb works before the table learns about it.

Registry fields used:

| Field | Meaning |
|---|---|
| color | color capable; white-only otherwise |
| multizone | linear zones |
| extended_multizone | base value |
| upgrades | entries `(major, minor, features)` applied when firmware >= that version |
| matrix | matrix zones |
| chain | product is chainable (only the Tile) |
| temperature_range | `[min, max]` kelvin; null on non-lights |
| uplight_coords | matrix cell driving an uplight |

Table, as encoded in `products.rs` (base kelvin range; upgrades that only
widen kelvin are not tracked, since the base range is always accepted):

| pid | name | class | chain | extended multizone | kelvin | quirk |
|---|---|---|---|---|---|---|
| 1 | Original | color | - | - | 2500-9000 | - |
| 2 | Original 1000 | color | - | - | 2500-9000 | - |
| 3 | Color 650 | color | - | - | 2500-9000 | - |
| 10 | LIFX White 800 (LV) | white | - | - | 2700-6500 | - |
| 11 | LIFX White 800 (HV) | white | - | - | 2700-6500 | - |
| 12 | LIFX LCMv4 White OEM (A19 LV) | white | - | - | 2700-2700 | - |
| 13 | LIFX LCMv4 White OEM (A19 HV) | white | - | - | 2700-2700 | - |
| 14 | LIFX Carbon Prototype | color | - | - | 2500-9000 | - |
| 15 | LIFX LCMv4 Color (A21) | color | - | - | 2700-6500 | - |
| 18 | LIFX White 900 (BR30) | white | - | - | 2700-6500 | - |
| 19 | LIFX White 900 (BR30) | white | - | - | 2700-6500 | - |
| 20 | LIFX Color 1000 (BR30) | color | - | - | 2500-9000 | - |
| 21 | LIFX LCMv4 Color (GU10) | color | - | - | 2500-9000 | - |
| 22 | LIFX Color 1000 | color | - | - | 2500-9000 | - |
| 23 | LIFX (A19) | color | - | - | 2500-9000 | - |
| 24 | LIFX (BR30) | color | - | - | 2500-9000 | - |
| 25 | LIFX+ (A19) | color | - | - | 2500-9000 | - |
| 26 | LIFX+ (BR30) | color | - | - | 2500-9000 | - |
| 27 | LIFX (A19) | color | - | - | 2500-9000 | - |
| 28 | LIFX (BR30) | color | - | - | 2500-9000 | - |
| 29 | LIFX+ (A19) | color | - | - | 2500-9000 | - |
| 30 | LIFX+ (BR30) | color | - | - | 2500-9000 | - |
| 31 | LIFX Z | multizone | - | never | 2500-9000 | - |
| 32 | LIFX Z | multizone | - | from 2.77 | 2500-9000 | - |
| 33 | LIFX Downlight | color | - | - | 2500-9000 | - |
| 36 | LIFX DL | color | - | - | 2500-9000 | - |
| 37 | LIFX DL | color | - | - | 2500-9000 | - |
| 38 | LIFX Beam | multizone | - | from 2.77 | 2500-9000 | - |
| 39 | LIFX DL WW 700lm | white | - | - | 2500-9000 | - |
| 40 | LIFX DL Colour 700lm | color | - | - | 2500-9000 | - |
| 43 | LIFX (A19) | color | - | - | 2500-9000 | - |
| 44 | LIFX (BR30) | color | - | - | 2500-9000 | - |
| 45 | LIFX+ (A19) | color | - | - | 2500-9000 | - |
| 46 | LIFX+ (BR30) | color | - | - | 2500-9000 | - |
| 49 | LIFX Mini C | color | - | - | 2500-9000 | - |
| 50 | LIFX Mini DD | white | - | - | 1500-4000 | - |
| 51 | LIFX Mini W | white | - | - | 2700-2700 | - |
| 52 | LIFX GU10 | color | - | - | 2500-9000 | - |
| 53 | LIFX GU10 | color | - | - | 2500-9000 | - |
| 54 | LIFX GU10 White | white | - | - | 2700-2700 | - |
| 55 | LIFX Tile | matrix | yes | - | 2500-9000 | - |
| 56 | LIFX Beam | multizone | - | always | 2500-9000 | - |
| 57 | LIFX Candle C | matrix | - | - | 2500-9000 | candle top zone |
| 59 | LIFX Mini C | color | - | - | 2500-9000 | - |
| 60 | LIFX Mini DD | white | - | - | 1500-4000 | - |
| 61 | LIFX Mini W | white | - | - | 2700-2700 | - |
| 62 | LIFX (A19) LCM3 | color | - | - | 2500-9000 | - |
| 63 | LIFX (BR30) LCM3 | color | - | - | 2500-9000 | - |
| 64 | LIFX+ (A19) LCM3 | color | - | - | 2500-9000 | - |
| 65 | LIFX+ (BR30) LCM3 | color | - | - | 2500-9000 | - |
| 66 | LIFX Mini W | white | - | - | 2700-2700 | - |
| 67 | LIFX Candle WW | white | - | - | 2200-6500 | - |
| 68 | LIFX Candle C | matrix | - | - | 2500-9000 | candle top zone |
| 70 | LIFX Switch | non-light | - | - | none | - |
| 71 | LIFX Switch | non-light | - | - | none | - |
| 72 | LIFX A19 | color | - | always | 1500-9000 | - |
| 81 | LIFX Candle WW | white | - | - | 2200-6500 | - |
| 82 | LIFX Filament Clear | white | - | - | 2200-2200 | - |
| 84 | LIFX Switch | non-light | - | - | none | - |
| 85 | LIFX Filament Amber | white | - | - | 2000-2000 | - |
| 86 | LIFX Filament Smokey | white | - | - | 2200-2200 | - |
| 87 | LIFX White | white | - | - | 2700-2700 | - |
| 88 | LIFX White | white | - | - | 2700-2700 | - |
| 89 | LIFX Switch | non-light | - | - | none | - |
| 90 | LIFX Clean A19 1100lm | color | - | - | 2500-9000 | - |
| 91 | LIFX Color 800lm | color | - | - | 2500-9000 | - |
| 92 | LIFX Colour 1000lm | color | - | - | 2500-9000 | - |
| 93 | LIFX Color A19 1100lm | color | - | - | 2500-9000 | - |
| 94 | LIFX Color BR30 1100lm | color | - | - | 2500-9000 | - |
| 96 | LIFX Candle WW | white | - | - | 2200-6500 | - |
| 97 | LIFX Colour A19 1200lm | color | - | - | 2500-9000 | - |
| 98 | LIFX Colour BR30 1100lm | color | - | - | 2500-9000 | - |
| 99 | LIFX Clean A19 1200lm | color | - | - | 2500-9000 | - |
| 100 | LIFX Filament Clear | white | - | - | 2200-2200 | - |
| 101 | LIFX Filament Amber | white | - | - | 2000-2000 | - |
| 102 | LIFX Filament Smokey | white | - | - | 2200-2200 | - |
| 109 | LIFX NV A19 1100lm | color | - | - | 2500-9000 | - |
| 110 | LIFX NV BR30 1100lm | color | - | - | 2500-9000 | - |
| 111 | LIFX NV A19 1200lm | color | - | - | 2500-9000 | - |
| 112 | LIFX NV BR30 1100lm | color | - | - | 2500-9000 | - |
| 113 | LIFX WW 800lm | white | - | - | 1500-4000 | - |
| 114 | LIFX WW 1000lm | white | - | - | 1500-4000 | - |
| 115 | LIFX Switch | non-light | - | - | none | - |
| 116 | LIFX Switch US | non-light | - | - | none | - |
| 117 | LIFX Z | multizone | - | always | 1500-9000 | - |
| 118 | LIFX Z | multizone | - | always | 1500-9000 | - |
| 119 | LIFX Beam | multizone | - | always | 1500-9000 | - |
| 120 | LIFX Beam | multizone | - | always | 1500-9000 | - |
| 121 | LIFX DL | color | - | - | 1500-9000 | - |
| 122 | LIFX DL | color | - | - | 1500-9000 | - |
| 123 | LIFX Mini 3.1 Color US | color | - | - | 1500-9000 | - |
| 124 | LIFX Mini 3.1 Color Intl | color | - | - | 1500-9000 | - |
| 125 | LIFX Mini 3.1 WW US | white | - | - | 1500-9000 | - |
| 126 | LIFX Mini 3.1 WW Intl | white | - | - | 1500-9000 | - |
| 127 | LIFX Mini 3.1 W US | white | - | - | 2700-2700 | - |
| 128 | LIFX Mini 3.1 W Intl | white | - | - | 2700-2700 | - |
| 129 | LIFX Color 800 | color | - | - | 1500-9000 | - |
| 130 | LIFX Colour 1000 | color | - | - | 1500-9000 | - |
| 131 | LIFX White to Warm 800 | white | - | - | 1500-9000 | - |
| 132 | LIFX White to Warm 1000 | white | - | - | 1500-9000 | - |
| 133 | LIFX White 650 | white | - | - | 2700-2700 | - |
| 134 | LIFX White 800 | white | - | - | 2700-2700 | - |
| 135 | LIFX GU10 Color US | color | - | - | 1500-9000 | - |
| 136 | LIFX GU10 Color Intl | color | - | - | 1500-9000 | - |
| 137 | LIFX Candle C | matrix | - | - | 1500-9000 | candle top zone |
| 138 | LIFX Candle C | matrix | - | - | 1500-9000 | candle top zone |
| 141 | LIFX Neon | multizone | - | always | 1500-9000 | - |
| 142 | LIFX Neon | multizone | - | always | 1500-9000 | - |
| 143 | LIFX String | multizone | - | always | 1500-9000 | - |
| 144 | LIFX String | multizone | - | always | 1500-9000 | - |
| 151 | LIFX Neon Outdoor | multizone | - | always | 1500-9000 | - |
| 152 | LIFX Neon Outdoor | multizone | - | always | 1500-9000 | - |
| 153 | LIFX PAR38 US | color | - | - | 1500-9000 | - |
| 154 | LIFX PAR38 Intl | color | - | - | 1500-9000 | - |
| 155 | LIFX Sunshine | color | - | - | 1500-9000 | - |
| 156 | LIFX A21 1000lm Intl | color | - | - | 1500-9000 | - |
| 161 | LIFX Neon Outdoor | multizone | - | always | 1500-9000 | - |
| 162 | LIFX Neon Outdoor | multizone | - | always | 1500-9000 | - |
| 163 | LIFX A19 | color | - | - | 1500-9000 | - |
| 164 | LIFX BR30 | color | - | - | 1500-9000 | - |
| 165 | LIFX A19 Intl | color | - | - | 1500-9000 | - |
| 166 | LIFX BR30 Intl | color | - | - | 1500-9000 | - |
| 167 | LIFX DL AU | color | - | - | 1500-9000 | - |
| 168 | LIFX DL US | color | - | - | 1500-9000 | - |
| 169 | LIFX A21 | color | - | - | 1500-9000 | - |
| 170 | LIFX A21 | color | - | - | 1500-9000 | - |
| 171 | LIFX Spot | matrix | - | - | 1500-9000 | - |
| 172 | LIFX Spot | matrix | - | - | 1500-9000 | - |
| 173 | LIFX Path | matrix | - | - | 1500-9000 | - |
| 174 | LIFX Path | matrix | - | - | 1500-9000 | - |
| 175 | LIFX PAR38 | color | - | - | 1500-9000 | - |
| 176 | LIFX Ceiling | matrix | - | - | 1500-9000 | uplight (7, 7) |
| 177 | LIFX Ceiling | matrix | - | - | 1500-9000 | uplight (7, 7) |
| 178 | LIFX Downlight US | color | - | - | 1500-9000 | - |
| 179 | LIFX Downlight US | color | - | - | 1500-9000 | - |
| 180 | LIFX Downlight US | color | - | - | 1500-9000 | - |
| 181 | LIFX Mini | color | - | - | 1500-9000 | - |
| 182 | LIFX Mini | color | - | - | 1500-9000 | - |
| 185 | LIFX Candle C | matrix | - | - | 1500-9000 | candle top zone |
| 186 | LIFX Candle C Intl | matrix | - | - | 1500-9000 | candle top zone |
| 187 | LIFX Candle | color | - | - | 1500-9000 | - |
| 188 | LIFX Candle Intl | color | - | - | 1500-9000 | - |
| 191 | LIFX Everyday A19 | color | - | - | 2700-6500 | - |
| 192 | LIFX Everyday A19 Intl | color | - | - | 2700-6500 | - |
| 201 | LIFX Ceiling 13x26 | matrix | - | - | 1500-9000 | uplight (15, 7) |
| 202 | LIFX Ceiling 13x26 Intl | matrix | - | - | 1500-9000 | uplight (15, 7) |
| 203 | LIFX String | multizone | - | always | 1500-9000 | - |
| 204 | LIFX String | multizone | - | always | 1500-9000 | - |
| 205 | LIFX Neon | multizone | - | always | 1500-9000 | - |
| 206 | LIFX Neon | multizone | - | always | 1500-9000 | - |
| 207 | LIFX Everyday Lightstrip | multizone | - | always | 2500-6500 | - |
| 208 | LIFX Everyday Lightstrip Intl | multizone | - | always | 2500-6500 | - |
| 211 | LIFX Everyday Permanent Outdoor US | multizone | - | always | 2700-6500 | - |
| 213 | LIFX Permanent Outdoor | multizone | - | always | 1500-9000 | - |
| 214 | LIFX Permanent Outdoor Intl | multizone | - | always | 1500-9000 | - |
| 215 | LIFX Candle C | matrix | - | - | 1500-9000 | candle top zone |
| 216 | LIFX Candle C Intl | matrix | - | - | 1500-9000 | candle top zone |
| 217 | LIFX Tube | matrix | - | - | 1500-9000 | - |
| 218 | LIFX Tube Intl | matrix | - | - | 1500-9000 | - |
| 219 | LIFX Luna | matrix | - | - | 1500-9000 | - |
| 220 | LIFX Luna Intl | matrix | - | - | 1500-9000 | - |
| 221 | LIFX Spot Intl | matrix | - | - | 1500-9000 | - |
| 222 | LIFX Path Intl | matrix | - | - | 1500-9000 | - |
| 223 | LIFX DL | color | - | - | 1500-9000 | - |
| 224 | LIFX DL Intl | color | - | - | 1500-9000 | - |
| 225 | LIFX PAR38 Intl | color | - | - | 1500-9000 | - |
| 226 | LIFX Dimmer Switch | non-light | - | - | none | - |
| 229 | LIFX Path Intl | matrix | - | - | 1000-10000 | - |
| 265 | LIFX Ceiling 13" | matrix | - | - | 1500-9000 | uplight (7, 7) |
| 266 | LIFX Ceiling 13" Intl | matrix | - | - | 1500-9000 | uplight (7, 7) |
| 267 | LIFX Mirror | matrix | - | - | 1500-9000 | - |
| 268 | LIFX Mirror Intl | matrix | - | - | 1500-9000 | - |
| 300 | LIFX Everyday Permanent Outdoor US | multizone | - | never | 2700-6500 | - |

---

## 11. Driver Integration

### 11.1 Crate layout

| File | Contents |
|---|---|
| `src/lib.rs` | `LifxDriverModule`, `DESCRIPTOR` (`id = "lifx"`), config, controls, presentation, discovery capability, probe target resolution |
| `src/config.rs` | `LifxConfig` |
| `src/protocol.rs` | header and message encoders, State decoders, message type constants |
| `src/color.rs` | `Hsbk` and the RGB conversion of section 8 |
| `src/products.rs` | the product table and capability resolution |
| `src/topology.rs` | device classes, tile geometry, LED-to-zone map, `DeviceInfo` builder |
| `src/encoder.rs` | frame to datagram plans per class (section 5) |
| `src/transport.rs` | UDP request and reply client with retries |
| `src/scanner.rs` | discovery and device probing |
| `src/backend.rs` | `LifxBackend`, device sessions, frame sink |
| `src/pacer.rs` | the per-device message budget (section 9.3) |

Dependencies: `hypercolor-driver-api`, `hypercolor-driver-support`,
`hypercolor-color`, `hypercolor-types`, `tokio`, `serde`, `serde_json`,
`anyhow`, `async-trait`, `tracing`, `if-addrs` (already a workspace
dependency, used to find per-interface IPv4 broadcast addresses). No new
third-party crates.

### 11.2 Module

| Capability | Provided |
|---|---|
| output backend | `LifxBackend`, backend id `lifx` |
| discovery | yes |
| config | yes |
| controls | driver surface and device surface |
| presentation | label `LIFX`, device class `Light` |
| pairing | no (the LAN protocol has no authentication) |

The module is enabled by default, and `hypercolor-driver-builtin` seeds a
default (enabled) config entry, the same as the Hue, Nanoleaf, and WLED
modules. Once the stability vocabulary from Spec 89 is available, the
descriptor is marked experimental as a label only; the label does not turn
the driver off.

### 11.3 Configuration

```toml
[drivers.lifx]
enabled = true
device_ips = ["192.168.1.40"]
broadcast_discovery = true
max_messages_per_second = 20
transition_ms = 0
white_kelvin = 6500
```

| Field | Default | Range | Apply impact |
|---|---|---|---|
| `device_ips` | `[]` | routable IPs | discovery rescan |
| `broadcast_discovery` | `true` | bool | discovery rescan |
| `max_messages_per_second` | 20 | 1-200 | backend rebind |
| `transition_ms` | 0 | 0-10000 | backend rebind |
| `white_kelvin` | 6500 | 1500-9000 | backend rebind |

Manual IPs are probed on port 56700 in addition to (or instead of)
broadcast. Tracked devices from earlier scans contribute their last known
address too, so a light keeps working on networks that filter broadcast.

### 11.4 Discovery

1. Bind an ephemeral UDP socket per address family.
2. When `broadcast_discovery` is on, enable SO_BROADCAST and send GetService
   to 255.255.255.255:56700 and to each up, non-loopback IPv4 interface's
   directed broadcast address.
3. Send GetService unicast to every probe target.
4. Collect StateService replies for half the request timeout, clamped to
   300 ms through 2 s (lights answer within milliseconds, so the rest of
   the budget is left for probing), keeping the first UDP service per
   serial.
5. Probe each serial concurrently (section 4.4) and drop non-lights, foreign
   vendors, and devices whose probe fails.

Metadata published per device: `ip`, `port`, `serial`, `vendor_id`,
`product_id`, `firmware`, `device_class`, `zones`, and `tiles` for matrices.
The fingerprint is `net:lifx:<serial>` and the portable identity claim is the
serial as a MAC address.

### 11.5 Connect, write, disconnect

- **Connect** re-probes the device at its adopted address (topology may have
  changed since discovery), sends SetLightPower on with an acknowledgement,
  and builds the session: a UDP socket, header template, output plan, and
  message budget. Connect timeout is 8 seconds.
- **Write** declines the frame when the budget has no room for its first
  message (section 9.4); otherwise it converts the frame to HSBK, pads or
  truncates to the device's LED count (warning at most once a minute),
  encodes the plan's datagrams into a reused buffer, and sends them, each
  one charged to the budget and paced by it.
- **Brightness** scales HSBK brightness (section 8).
- **Disconnect** drops the session.
- `connected_device_info` returns the topology read at connect.
- Output cadence and `target_fps` follow section 9.

### 11.6 Device controls

The device surface shows IP, port, serial, product, firmware, device class,
zone count, max FPS, and state, plus a `refresh_topology` action that
reconnects the device to re-read its zones or chain (needed after adding a
tile or extending a strip).

### 11.7 Device database

`data/drivers/vendors/lifx.toml` lists the device classes with transport
`network_udp`, driver `lifx`. Until the device database grows a verification
field, the notes say the driver is experimental and unverified on hardware.

---

## 12. Testing Strategy

No test sends a packet anywhere but loopback. Broadcast discovery is never
exercised in tests; scanner tests pass explicit loopback targets with
broadcast disabled.

| Suite | Asserts |
|---|---|
| `lifx_protocol_tests.rs` | golden bytes for the header and every sent message (the documentation's SetColor vector plus one vector per type in section 3.3), parsing of every received State message including the documentation's StateLabel reply and reserved-bit tolerance |
| `lifx_color_tests.rs` | primaries are fully saturated, greys have zero saturation, pastels keep linear-light saturation, hue scaling, brightness scaling, white-only luminance path, kelvin clamping |
| `lifx_products_tests.rs` | lookup, extended multizone thresholds (product 32 at 2.76 vs 2.77), non-light rejection, unknown product fallback |
| `lifx_topology_tests.rs` | segments for each class, chain positions with rotation, candle and uplight maps, messages per frame and cadence |
| `lifx_encoder_tests.rs` | datagram plans: run merging and apply flags for legacy, 82-zone chunking, Set64 per tile, frame buffer path above 64 zones, single-buffer fallback |
| `lifx_scanner_tests.rs` | an in-repo fake LIFX responder on 127.0.0.1 answers discovery and probes for bulb, legacy strip, extended strip, tile chain, and 16 x 8 ceiling; several devices behind one port |
| `lifx_backend_tests.rs` | adopt, connect (power ack), write, brightness, cadence, refreshed info, against the fake responder |
| `lifx_driver_tests.rs` | descriptor, default-enabled module, config validation, driver and device control surfaces, probe target merging |
| `lifx_emulator_tests.rs` | opt-in run against lifx-emulator bound to 127.0.0.1 (section 12.1) |

### 12.1 Emulator run

The lifx-emulator project (Python) emulates bulbs, multizone strips, and
matrix devices on one UDP port and binds 127.0.0.1 by default. It sends
nothing unprompted, and its HTTP API and mDNS stay off unless asked for.

```bash
just lifx-emulator-test
```

The recipe runs from an empty directory under `target/` (the emulator
auto-loads a `lifx-emulator.yaml` from its working directory), starts
`uvx lifx-emulator@4.11.0 --config crates/hypercolor-driver-lifx/tests/fixtures/lifx-emulator.yaml --bind 127.0.0.1 --port 56799`
(it also binds `[::1]` on the same port), runs the opt-in test with
`HYPERCOLOR_LIFX_EMULATOR=127.0.0.1:56799`, and stops the emulator. Without
that variable the test returns immediately, so `cargo test` never needs
Python or the network.

The fixture fleet is a color bulb, an extended Beam, a Beam pinned to
firmware 2.60 (legacy path), a three-tile chain, a Candle, an 8 x 8 Ceiling,
a 16 x 8 Ceiling, and a Switch. The test discovers the fleet by unicast,
connects each light through `LifxBackend`, streams one frame, and reads the
device state back (GetColor, GetExtendedColorZones, GetColorZones, Get64) to
compare every zone with the HSBK the driver computed. It then drives the
hidden-buffer path by hand and checks the Ceiling stays unchanged until
CopyFrameBuffer arrives.

---

## 13. Risks and Open Questions

| Risk | Status |
|---|---|
| Nobody has run the driver on real LIFX hardware | ships experimental and off by default |
| The 20 messages per second figure comes from 2016-era documentation | exposed as `max_messages_per_second`; revisit after hardware tests |
| Tile chain frame rate is 4 FPS at the default budget | a direct consequence of one Set64 per tile; users who accept the risk can raise the budget |
| A five-message burst may be more than some firmware likes | the burst is one Tile chain frame, the pattern LIFX's own clients send; the long-run rate stays at the budget |
| Saturation in linear light may look more saturated than the LIFX app for pastels | intentional (LEDs reward saturation); revisit with hardware |
| `user_y` direction is not stated in the documentation | taken as growing upward; users can rotate the zone in the layout editor |
| The frame-buffer sequence for matrices above 64 zones is an inference | Set64 to fb 1 then CopyFrameBuffer verified against the emulator by read-back; needs a real 26 inch Ceiling |
| Candle zone order beyond the top zone comes from a documentation image | row-major after the top row; the emulator stores the layout but cannot confirm the physical wrap |
| No liveness signal while streaming (no acks) | an unreachable light keeps its session until the next discovery or reconnect; candidate follow-up: periodic acknowledged probe |
| mDNS (`_lifx._udp`, firmware 4.110 and newer) is not used | broadcast works on every firmware; mDNS can be added later |

---

## References

- LIFX LAN documentation, packet contents: https://lan.developer.lifx.com/docs/packet-contents
- Encoding a packet (SetColor example): https://lan.developer.lifx.com/docs/encoding-a-packet
- Decoding a packet (GetPower and StateLabel examples): https://lan.developer.lifx.com/docs/decoding-a-packet
- Communicating with a device (discovery): https://lan.developer.lifx.com/docs/communicating-with-device
- Querying the device for data: https://lan.developer.lifx.com/docs/querying-the-device-for-data
- Changing a device (Set messages): https://lan.developer.lifx.com/docs/changing-a-device
- Information messages (State messages): https://lan.developer.lifx.com/docs/information-messages
- Field types (HSBK, Tile, enums): https://lan.developer.lifx.com/docs/field-types
- Representing color with HSBK: https://lan.developer.lifx.com/docs/representing-color-with-hsbk
- Multizone light control: https://lan.developer.lifx.com/docs/multizone-light-control
- Tile control (positions and orientation): https://lan.developer.lifx.com/docs/tile-control
- Candle: https://lan.developer.lifx.com/docs/candle
- Product registry: https://lan.developer.lifx.com/docs/product-registry and https://github.com/LIFX/products
- Legacy LIFX protocol documentation, "Limits" (20 messages per second): https://github.com/LIFX/lifx-protocol-docs
- lifx-emulator (test peer only): https://djelibeybi.github.io/lifx-emulator
- Hypercolor Spec 35 (network driver architecture), Spec 50 (config registry), Spec 51 (driver module API, render-path isolation), Spec 52 (control surfaces)
- `docs/color-science-led-guide.md`
