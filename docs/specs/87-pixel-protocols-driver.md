# 87 -- Pixel Protocols Driver

> Generic network driver for pixel controllers that speak the open streaming
> protocols: E1.31 (sACN), Art-Net 4 ArtDmx, and DDP. Byte layouts for every
> packet the driver emits, sequencing, universe math, frame pacing, the
> configuration schema, and how the driver and its shared encoder crate plug
> into the driver registry.

**Status:** Implemented (experimental: not yet run against real controllers)
**Crates:** `hypercolor-pixel-protocols`, `hypercolor-driver-pixel`
**Module path:** `hypercolor_driver_pixel`, `hypercolor_pixel_protocols::{ddp, e131, artnet, dmx}`
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Packet Formats](#2-packet-formats)
3. [Protocol Architecture](#3-protocol-architecture)
4. [E1.31 Wire Format](#4-e131-wire-format)
5. [Art-Net Wire Format](#5-art-net-wire-format)
6. [DDP Wire Format](#6-ddp-wire-format)
7. [Universe and Offset Math](#7-universe-and-offset-math)
8. [Color Encoding](#8-color-encoding)
9. [Timing and Frame Pacing](#9-timing-and-frame-pacing)
10. [Configuration Schema](#10-configuration-schema)
11. [Driver Integration](#11-driver-integration)
12. [Testing Strategy](#12-testing-strategy)
13. [Hardware Validation](#13-hardware-validation)
14. [References](#references)

---

## 1. Overview

Three open standards carry most hobbyist and professional pixel traffic.
E1.31 (Streaming ACN, or sACN) and Art-Net both move DMX512 universes of up to
512 channels over UDP. DDP moves an offset-addressed byte stream with no
universe ceiling. Falcon controllers (F16V4, F16V5), Falcon Player (FPP),
ESPixelStick, and WLED accept all three as input, and Art-Net or sACN nodes
put DMX fixtures on the same footing. One generic driver therefore reaches a
large installed base that Hypercolor could not drive before.

These receivers do not describe their outputs over the network in a way a
sender can trust, so the user configures every device by hand: host,
protocol, universe or offset, channel order, and pixel counts per output. The
driver's discovery turns those entries into devices and sends nothing.

Hyperion.ng and HyperHDR are often grouped with these receivers, but neither
accepts E1.31, Art-Net, or DDP as input; both implement them only as LED
outputs. HyperHDR's raw UDP server on port 5568 takes headerless RGB
triplets and is not sACN. Neither is a target for this driver.

Before this spec, the only DDP and E1.31 encoders lived inside the WLED
driver. They now live in `hypercolor-pixel-protocols`, a leaf crate that both
drivers consume (section 11.1). Everything here was derived from the public
standards listed under [References](#references); no GPL source was read or
transliterated.

---

## 2. Packet Formats

| Packet | Protocol | Port | Size | Sent when |
|---|---|---|---|---|
| E1.31 data packet | E1.31 | 5568 | 126 + n (n = 1-512 slots) | Once per universe per frame |
| E1.31 universe synchronization | E1.31 | 5568 | 49 | After each frame's data packets, when `sync_universe` is set |
| E1.31 data packet, `Stream_Terminated` | E1.31 | 5568 | 126 + n | Three per universe on disconnect |
| ArtDmx | Art-Net 4 | 6454 (source and destination) | 18 + n (n even, 2-512) | Once per universe per frame |
| DDP data packet | DDP | 4048 | 10 + n (n up to 1440) | One or more per frame |

The driver emits no other packet types. In particular it never sends
ArtPoll, ArtSync, E1.31 universe discovery, DDP queries, or anything to a
multicast or broadcast address.

---

## 3. Protocol Architecture

The driver encodes every frame into one contiguous **channel stream**: each
pixel's channels in wire order, segment after segment (section 8). A
per-device **output plan**, resolved and validated from config before any
packet is sent, maps ranges of that stream onto the wire:

| Protocol | Plan | Addressing unit |
|---|---|---|
| E1.31 | universes, slot runs, source identity | universe 1-63999, slot 1-512 |
| Art-Net | universes, slot runs | Port-Address 0-32767, slot 1-512 |
| DDP | byte spans | 32-bit byte offset |

E1.31 and Art-Net share the DMX universe math in `hypercolor_pixel_protocols::dmx`
and differ only in packet framing, universe range, and sequencing. DDP has
no universes.

**Unicast only.** E1.31 receivers "shall also respond to E1.31 data received
on its unicast address" (E1.31-2018 section 9.1.1), and Art-Net 4 requires
ArtDmx to be unicast. DDP data "should not" be broadcast unless every
destination wants the same data. The driver therefore supports unicast
alone. Hosts are canonicalized first, so an IPv4-mapped IPv6 address such as
`::ffff:239.1.2.3` is judged, compared, and sent as the IPv4 address it
carries; a dual-stack socket would otherwise send it as IPv4 multicast.
Validation then rejects unspecified addresses, every multicast range (IPv4
224.0.0.0/4, IPv6 ff00::/8 including link-local multicast), the IPv4
limited broadcast, and the IPv4 "this network" block 0.0.0.0/8, and connect
repeats the check. A directed subnet broadcast cannot be recognized without
the netmask, but no output socket enables `SO_BROADCAST`, so the kernel
refuses it. E1.31 multicast (239.255.UH.UL) is a possible follow-up; it is
excluded here because it reaches every receiver on the segment, WLED
included.

**Not implemented:**

- **ArtPoll.** Art-Net 4 says controllers "must regularly ArtPoll" and unicast
  ArtDmx only to subscribed nodes. ArtPoll is a broadcast. The driver unicasts
  to the configured node without polling, which is how manual-IP setups in
  sequencers work in practice. This is a deliberate deviation.
- **ArtSync.** Art-Net 4 does not allow controllers to unicast ArtSync; it
  must be a directed broadcast. Excluded with ArtPoll.
- **E1.31 universe discovery.** Multicast on universe 64214. Not needed for
  manual configuration.

---

## 4. E1.31 Wire Format

Source: ANSI E1.31-2018, Table 4-1 (data packet) and Table 4-2
(synchronization packet). All multi-byte fields are big-endian. Every
flags-and-length field carries `0x7` in the high nibble and the PDU length in
the low 12 bits.

### 4.1 Data Packet

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 2 | Preamble Size | `0x0010` | Root layer |
| 2 | 2 | Post-amble Size | `0x0000` | |
| 4 | 12 | ACN Packet Identifier | `41 53 43 2D 45 31 2E 31 37 00 00 00` | `ASC-E1.17` plus three NULs |
| 16 | 2 | Flags and Length | `0x7000 \| (110 + n)` | Counts from offset 16 to the end |
| 18 | 4 | Vector | `0x00000004` | `VECTOR_ROOT_E131_DATA` |
| 22 | 16 | CID | UUID | Sender component identifier |
| 38 | 2 | Flags and Length | `0x7000 \| (88 + n)` | Framing layer, counts from offset 38 |
| 40 | 4 | Vector | `0x00000002` | `VECTOR_E131_DATA_PACKET` |
| 44 | 64 | Source Name | UTF-8, NUL padded | At most 63 bytes, always NUL terminated |
| 108 | 1 | Priority | 0-200 | Default 100 |
| 109 | 2 | Synchronization Address | 0 or 1-63999 | 0 means unsynchronized |
| 111 | 1 | Sequence Number | 0-255 | Per universe, wraps 255 to 0 |
| 112 | 1 | Options | bit field | Section 4.3 |
| 113 | 2 | Universe | 1-63999 | |
| 115 | 2 | Flags and Length | `0x7000 \| (11 + n)` | DMP layer, counts from offset 115 |
| 117 | 1 | Vector | `0x02` | `VECTOR_DMP_SET_PROPERTY` |
| 118 | 1 | Address Type and Data Type | `0xA1` | |
| 119 | 2 | First Property Address | `0x0000` | |
| 121 | 2 | Address Increment | `0x0001` | |
| 123 | 2 | Property Value Count | `n + 1` | Slots plus the START code, `0x0001`-`0x0201` |
| 125 | 1 | DMX512-A START Code | `0x00` | Null START code |
| 126 | n | Property Values | channel data | Slot 1 at offset 126 |

A full universe is 638 bytes. The driver sends `n` = one past the highest slot
any of the device's runs writes in that universe; lower unused slots go out as
zero because DMX addressing is positional (section 7.3).

### 4.2 Universe Synchronization Packet

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 2 | Preamble Size | `0x0010` | |
| 2 | 2 | Post-amble Size | `0x0000` | |
| 4 | 12 | ACN Packet Identifier | as above | |
| 16 | 2 | Flags and Length | `0x7021` | 33 bytes from offset 16 |
| 18 | 4 | Vector | `0x00000008` | `VECTOR_ROOT_E131_EXTENDED` |
| 22 | 16 | CID | UUID | Same CID as the data packets |
| 38 | 2 | Flags and Length | `0x700B` | 11 bytes from offset 38 |
| 40 | 4 | Vector | `0x00000001` | `VECTOR_E131_EXTENDED_SYNCHRONIZATION` |
| 44 | 1 | Sequence Number | 0-255 | Own sequence space |
| 45 | 2 | Synchronization Address | 1-63999 | Universe being latched |
| 47 | 2 | Reserved | `0x0000` | |

When a device sets `sync_universe`, every data packet carries it at offset
109 and one sync packet follows each frame's data packets. Section 6.3.2 of
the standard gives sync packets a sequence counter with "no implied
relationship" to data packet sequences on the same universe, so the driver
keeps a separate counter. The standard describes multicast delivery to the
synchronization universe's group and has no explicit unicast rule for sync
packets; section 9.1 permits unicast in general, so the driver unicasts sync
packets to the device host. Receivers that ignore synchronization ignore a
nonzero sync address (section 6.2.4.1).

### 4.3 Options Octet (offset 112)

| Bit | Mask | Name | Driver use |
|---|---|---|---|
| 7 | `0x80` | `Preview_Data` | Never set |
| 6 | `0x40` | `Stream_Terminated` | Set on the three shutdown packets |
| 5 | `0x20` | `Force_Synchronization` | Never set |
| 0-4 | | Reserved | Transmitted as 0 |

Section 6.2.6: "Three packets containing this bit set to 1 shall be sent by
sources upon terminating sourcing of a universe." Receivers enter data loss
immediately and ignore the property values.

### 4.4 Sequencing and Identity

- **Sequence:** one `u8` counter per universe, starting at 1 and wrapping
  from 255 to 0. Keep-alive repeats and shutdown packets keep incrementing,
  as section 6.6.2 requires.
- **Priority:** 0-200. The default is the standard's 100. The WLED driver
  keeps its own 150 so it wins over default-priority sources.
- **Source name:** the 64-byte field holds at most 63 bytes of UTF-8 plus a
  NUL. Validation rejects longer names, and the encoder truncates on a
  character boundary as a second line of defense.
- **CID:** section 5.6 requires a UUID per source and says equipment
  "should maintain the same CID for its entire lifetime". The daemon already
  persists an instance id (a UUID in its data directory, also used for its
  mDNS identity) and exposes it to drivers as `DriverHost::installation_id`.
  Discovery derives each device's CID as a UUID v5, under a fixed Hypercolor
  namespace, of `<installation id>/<device id>`, and writes it into the
  device's effective config. The CID is therefore stable across restarts,
  and two installations that reuse a device id still get different CIDs. A
  device may pin an explicit `cid`. A host without an installation id hashes
  the device id alone.

---

## 5. Art-Net Wire Format

Source: Art-Net 4 specification, revision 1.4dp, ArtDmx field table.

### 5.1 ArtDmx

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 8 | ID | `41 72 74 2D 4E 65 74 00` | `Art-Net` plus NUL |
| 8 | 2 | OpCode | `00 50` | `0x5000` OpDmx, low byte first |
| 10 | 1 | ProtVerHi | `0x00` | |
| 11 | 1 | ProtVerLo | `0x0E` | Protocol revision 14 |
| 12 | 1 | Sequence | 1-255 | 0 would disable reordering |
| 13 | 1 | Physical | `0x00` | Informational input port |
| 14 | 1 | SubUni | low byte of Port-Address | Sub-Net (high nibble), Universe (low nibble) |
| 15 | 1 | Net | bits 14-8 of Port-Address | |
| 16 | 1 | LengthHi | high byte | Data length, high byte first |
| 17 | 1 | Length | low byte | Even, 2-512 |
| 18 | n | Data | channel data | Slot 1 at offset 18, no START code |

### 5.2 Port-Address and Length

The Port-Address is 15 bits: `Net` (bits 14-8), `Sub-Net` (bits 7-4), and
`Universe` (bits 3-0). The driver's `start_universe` for Art-Net is that flat
value, so `0x0123` means Net 1, Sub-Net 2, Universe 3. Art-Net 4 gives the
valid range as 1-32767 and says "the value of zero is deprecated to enhance
sACN compatibility." The default start universe is therefore 1, but 0 stays
configurable for nodes still set to `0:0:0`.

Length "should be an even number in the range 2 - 512". The encoder pads an
odd count with one zero channel and sends two zero channels for an empty
universe.

### 5.3 Sequencing

One counter per Port-Address, incremented "in the range 0x01 to 0xff". The
driver wraps 255 back to 1 and never sends 0, which would disable receiver
reordering.

### 5.4 Source Port and Addressing

Art-Net 4 says "Art-Net uses only one port of 0x1936" as both source and
destination, so ArtDmx goes out from a socket bound to `0.0.0.0:6454`. The
socket sets `SO_REUSEADDR`, plus `SO_REUSEPORT` on Unix, so it coexists
with another Art-Net application on the host that sets them too. If the bind
still fails, the driver logs one warning and sends ArtDmx from the shared
ephemeral IPv4 socket, which some nodes ignore. The driver never reads from
the 6454 socket. When another application shares the port with reuse
flags, the kernel may deliver some unicast Art-Net traffic meant for it to
this socket instead, where it is dropped; nodes that see two Art-Net
senders on one host should be pointed at one of them.

Art-Net 4 is an IPv4 protocol, so validation rejects IPv6 hosts for
`artnet` devices (an IPv4-mapped address is accepted as its IPv4 form).

---

## 6. DDP Wire Format

Source: the 3waylabs DDP specification. Multi-byte fields are MSB first.

### 6.1 Header

| Offset | Size | Field | Value | Description |
|---|---|---|---|---|
| 0 | 1 | Flags | `0x40` or `0x41` | `VV x T S R Q P`: version 01, push bit 0 |
| 1 | 1 | Sequence | 1-15 | Low nibble; 0 means unused |
| 2 | 1 | Data Type | `0x0B`, `0x1B`, or `0x00` | `C R TTT SSS` |
| 3 | 1 | Destination ID | `0x01` | Default output device |
| 4 | 4 | Data Offset | bytes | Position of the first data byte |
| 8 | 2 | Data Length | bytes | Payload length, at most 1440 here |
| 10 | n | Data | channel data | No timecode field (T = 0) |

Flag bits: VV (bits 7-6) version, T (`0x10`) timecode present, S (`0x08`)
storage, R (`0x04`) reply, Q (`0x02`) query, P (`0x01`) push. The driver only
ever sets version 1 and push.

### 6.2 Data Type

TTT is the pixel type (000 undefined, 001 RGB, 010 HSL, 011 RGBW, 100
grayscale) and SSS the element size (011 is 8 bits). The driver sends
`0x0B` (RGB, 8-bit) for three-channel orders, `0x1B` (RGBW, 8-bit) for
four-channel orders, and `0x00` ("zero if not used or undefined") when
`channels_per_pixel` pads each pixel beyond its color channels. The type does
not encode channel order; receivers apply their own. WLED reads only the RGBW
type to choose four bytes per pixel; FPP ignores the byte.

### 6.3 Fragmentation, Push, and Sequence

The 1440-byte payload ceiling is not normative; it comes from the
specification's efficiency table and sample code (`DDP_MAX_DATALEN (480*3)`)
and keeps a packet inside a 1500-byte Ethernet MTU. The driver splits each
span into chunks of the largest whole-pixel multiple at or under 1440 bytes
(1440 for 3- and 4-channel pixels, 1435 for 7-channel pixels), because WLED
converts byte offsets to pixel indices and a chunk boundary inside a pixel
would misalign the rest of the frame.

The DDP specification says the sequence number "should be incremented with
each new packet sent" and that "a receiver can ignore duplicates received
back-to-back", so the driver gives every packet the next number, 1 through
15, wrapping to 1 and never sending 0. Only the final packet of the frame
sets push, which tells the receiver "to display its data buffer
immediately". Spans with explicit offsets (section 7.4) go out in segment
order inside the same frame.

The WLED driver keeps one sequence number per frame, the framing it has
always sent, through `build_ddp_frame`. WLED's receiver (`handleDDPPacket`
in `wled00/e131.cpp`) only drops sequence numbers just behind the last one
it recorded, so that framing passes there at any frame size, and WLED stays
byte-identical.

---

## 7. Universe and Offset Math

### 7.1 Channel Stream

For a device with `P` pixels and `C` channels per pixel, the stream is
`P * C` bytes. Segment `k` occupies the stream range that starts after the
channels of segments `0..k`.

### 7.2 Channels per Universe

`channels_per_universe` (CPU) is how many slots of each universe the device
fills before wrapping to the next universe. The default is the largest
multiple of `C` that fits 512, so no pixel straddles two universes:

| Pixel channels | Default CPU | Pixels per universe |
|---|---|---|
| 3 (RGB orders) | 510 | 170 |
| 4 (RGBW orders) | 512 | 128 |
| 5 | 510 | 102 |

Controllers such as ESPixelStick can be set to 512-channel universes where
RGB pixels straddle boundaries; `channels_per_universe = 512` matches them.
CPU must be 1-512 and at least `C`.

### 7.3 Absolute Slot Addressing

A position is an absolute slot `A = universe * CPU + (channel - 1)`. A run
of `L` channels starting at `A` is split into universe runs: universe
`floor(A / CPU)`, first slot `A mod CPU`, and `min(CPU - first slot,
remaining)` channels, repeated until the run is consumed.

- The device starts at `start_universe` (default 1) and `start_channel`
  (default 1, at most CPU).
- A segment without addressing continues at the slot after the previous
  segment.
- A segment with `start_universe` jumps there, at its `start_channel`
  (default 1). `start_channel` without `start_universe` is rejected.
- Every universe the device touches is sent each frame. Its length is one
  past the highest slot written; unwritten slots below that are zero.

Validation rejects any run past the protocol's universe range (E1.31 63999,
Art-Net 32767) and any two segments that write the same slot.

Example: `start_universe = 10`, segments of 100, 100, and 10 RGB pixels, the
third with `start_universe = 20, start_channel = 4`:

| Stream range | Universe | First slot (0-based) |
|---|---|---|
| 0-299 | 10 | 0 |
| 300-509 | 10 | 300 |
| 510-599 | 11 | 0 |
| 600-629 | 20 | 3 |

### 7.4 DDP Offsets

The device starts at byte `ddp_offset` (default 0). A segment without
`ddp_offset` continues after the previous one; a segment with it jumps to
that absolute byte offset. FPP and Falcon treat the DDP offset as the
zero-based absolute channel number, so a model at start channel 1001 uses
`ddp_offset = 1000`. Spans must fit the 32-bit offset space and must not
overlap.

### 7.5 Cross-Device Ownership

Each device sends whole universe packets, so two devices that send the same
universe to the same receiver (same host, port, and protocol) would zero out
each other's channels every frame. Validation rejects that and asks for one
device with two segments instead. Two DDP devices may share a receiver when
their byte ranges do not overlap.

---

## 8. Color Encoding

Hypercolor renders RGB. `color_order` names the wire order:

| Orders | Channels | Encoding of RGB `(r, g, b)` |
|---|---|---|
| `rgb`, `rbg`, `grb`, `gbr`, `brg`, `bgr` | 3 | Permutation of `r, g, b` |
| `rgbw`, `rbgw`, `grbw`, `gbrw`, `brgw`, `bgrw` | 4 | `w = min(r, g, b)`; permutation of `r - w, g - w, b - w`, then `w` |

Names parse case-insensitively. Pixel controllers usually apply color order
per output port themselves, in which case `rgb` is correct. DMX fixtures with
a fixed channel map need the matching order.

`channels_per_pixel` (default 3 or 4 from the order) may exceed the color
channels for fixtures with extra channels per head; the extra channels are
sent as zero. Fixtures that need a dimmer channel held high should be set to
a mode without one.

Frames shorter than the device pixel count are padded with black and longer
frames are truncated, with a rate-limited warning, matching the WLED driver.

---

## 9. Timing and Frame Pacing

### 9.1 Frame Rate

A device follows the render loop by default: its output cadence has no
minimum interval, so it sends whatever rate the render loop produces, up to
the 60 fps top tier. `max_fps` (1-1000) caps a single device; 0 or unset
means no cap.

E1.31-2018 section 6.6.1 says sources "shall not transmit packets for a given
universe number at a rate which exceeds the maximum refresh rate specified in
E1.11 unless configured by the user to do so", and that sources "shall
provide a user-configurable option" for higher rates. The E1.11 limit depends
on slot count. With the minimum transmitted break (92 us) and mark after
break (12 us) and 44 us per slot, a full 513-slot frame takes
`92 + 12 + 513 * 44 = 22,676 us`, about 44 Hz; Art-Net 4 states the same
"maximum DMX512 rate of 44Hz". Short universes allow far higher rates.

That limit is a property of a physical DMX512 output. Pixel controllers
drive WS281x-class pixel outputs, not DMX512 lines, and routinely accept
40-60 fps. Capping every device at 44 Hz would lower Hypercolor's output
below its render baseline for the common case. The driver therefore treats a
configured pixel device as the user's choice to run at the render rate, and
`max_fps` is the configurable option the standard asks for. For a receiver
that re-emits physical DMX512 with full universes, set `max_fps = 44`. This
interpretation is the one open standards question for review.

### 9.2 Keep-Alive and Suppression

Hypercolor's output queue already suppresses unchanged frames upstream of the
driver and reasserts the last payload after a configurable silence. The
driver sets that silence to 800 ms:

- E1.31 section 6.6.2 asks for "a single keep-alive packet ... at intervals
  of between 800mS and 1000mS" once transmission is suppressed, and
  receivers declare data loss after `E131_NETWORK_DATA_LOSS_TIMEOUT` (2.5 s).
- Art-Net 4 recommends an 800-1000 ms retransmit for idle inputs.
- DDP defines no timeout; WLED's realtime timeout defaults to 2.5 s.

Section 6.6.2 also says "three packets containing the non-changing
Property Values ... shall be sent before the initiation of transmission
suppression." E1.31 devices declare that through the output cadence
(`OutputCadence::with_min_identical_sends(3)`): the output queue lets an
unchanged payload through until it has been delivered three times, then
suppresses it. Each repeat waits for the previous copy to be delivered, so a
coalesced or failed attempt never counts, and a changed payload restarts
the count. Repeats ride the render loop's existing cadence; nothing is
paced lower. Art-Net and DDP keep plain suppression, and every other backend
keeps the default of one send.

### 9.3 Shutdown

On disconnect, if the device sent anything, the driver sends one black frame
(with its sync packet when synchronized) and then, for E1.31, three
`Stream_Terminated` packets per universe so receivers release immediately
instead of waiting out the 2.5 s timeout. Art-Net and DDP have no
termination signal and stop after the black frame. All shutdown sends are
best effort.

---

## 10. Configuration Schema

Settings live under `[drivers.pixel]` and flow through the driver config
registry (Spec 50). The driver control surface (Spec 52) exposes the same
fields, so `GET /api/v1/drivers/pixel/controls` and
`hypercolor drivers set-control` edit them with full validation.

```toml
[drivers.pixel]
enabled = true
source_name = "Hypercolor"   # default E1.31 source name
priority = 100               # default E1.31 priority, 0-200

[[drivers.pixel.devices]]
id = "roofline"              # stable identity; required
name = "Roofline Falcon"
host = "192.168.1.60"
protocol = "e131"            # e131 | artnet | ddp
start_universe = 1

[[drivers.pixel.devices.segments]]
name = "Front"
pixel_count = 300

[[drivers.pixel.devices.segments]]
name = "Garage"
pixel_count = 150
start_universe = 4
```

### 10.1 Driver Fields

| Field | Type | Default | Notes |
|---|---|---|---|
| `devices` | list of device objects | `[]` | At most 256 |
| `source_name` | string | `"Hypercolor"` | At most 63 bytes |
| `priority` | integer | 100 | 0-200 |

### 10.2 Device Fields

| Field | Protocols | Default | Rules |
|---|---|---|---|
| `id` | all | required | 1-64 of `A-Z a-z 0-9 - _`; unique ignoring case |
| `name` | all | `id` | At most 128 characters |
| `host` | all | required | Unicast IPv4 or IPv6 (IPv4 only for `artnet`); IPv4-mapped forms count as IPv4; loopback and link-local allowed |
| `protocol` | all | required | `e131` (alias `sacn`), `artnet` (alias `art-net`), `ddp` |
| `port` | all | 5568, 6454, 4048 | 1024-65535 |
| `color_order` | all | `rgb` | Section 8 |
| `channels_per_pixel` | all | 3 or 4 | Color channels to 512 |
| `pixel_count` | all | none | Single output; exclusive with `segments` |
| `segments` | all | none | At most 256 of `{ name, pixel_count, start_universe, start_channel, ddp_offset }` |
| `start_universe` | e131, artnet | 1 | E1.31 1-63999, Art-Net 0-32767 |
| `start_channel` | e131, artnet | 1 | 1 to `channels_per_universe` |
| `channels_per_universe` | e131, artnet | 510 or 512 | 1-512, at least `channels_per_pixel` |
| `ddp_offset` | ddp | 0 | Byte offset |
| `priority` | e131 | driver `priority` | 0-200 |
| `source_name` | e131 | driver `source_name` | At most 63 bytes |
| `sync_universe` | e131 | none | 1-63999 |
| `cid` | e131 | UUID v5 of installation id and `id` | UUID string |
| `max_fps` | all | 0 | 0-1000, 0 follows the render loop |

Fields that do not apply to the device's protocol are rejected rather than
ignored, so a `priority` on a DDP device or a `ddp_offset` on an E1.31 device
fails validation with a message naming the field.

A device may stream at most 4,194,304 channels (4 MiB per frame, about
8,200 full universes or 1.39 million RGB pixels), which bounds the encode
buffer and packet plan one config entry can request. Within that bound the
protocol limits apply: the last addressed universe must stay inside the
E1.31 or Art-Net range, and DDP spans inside the 32-bit offset space. Both
limits are checked with the device's totals before any universe run or
span is built.

### 10.3 Apply Behavior

All driver fields declare `ApplyImpact::DiscoveryRescan`. A rescan
re-materializes the device list with the new settings; a new device connects
right away, and an edited device picks up its new plan the next time it
connects. Device control surfaces are read-only diagnostics (config id, host,
port, protocol, universes, pixel count, color order).

---

## 11. Driver Integration

### 11.1 Shared Encoder Crate

`hypercolor-pixel-protocols` depends only on `uuid`. It owns:

- `ddp`: `DdpPacket`, `encode_ddp_packet` (buffer reuse), `DdpSequence`,
  `DdpSpan`, `build_ddp_frame`, `build_ddp_spans`, `pixel_aligned_chunk_len`
- `e131`: `E131Packet` with sync address and option setters,
  `E131SyncPacket`, `E131SequenceTracker`, `universes_needed`, range constants
- `artnet`: `ArtDmxPacket`, `PortAddress`, `ArtDmxSequenceTracker`
- `dmx`: `split_into_universes`, `universes_spanned`,
  `pixel_aligned_universe_channels`

The WLED driver re-exports its previous names from it, keeps priority 150 as
a WLED-local constant, and its protocol tests pass unchanged. A driver never
depends on another driver.

### 11.2 Module and Registration

`hypercolor-driver-pixel` registers `PixelDriverModule` (`id = "pixel"`,
`DriverTransportKind::Network`) through `hypercolor-driver-builtin` behind the
`pixel` feature, which the default `network` feature includes.
`normalize_driver_config_entries` seeds an enabled `drivers.pixel` entry with
no devices. The module is enabled by default like every other network
driver; with no devices configured it sends nothing. It carries the
experimental label (Spec 89) once `DriverDescriptor::experimental()` lands.

Capabilities: config, discovery, controls, presentation, and an owned output
backend. No pairing, no credentials, no runtime cache.

### 11.3 Discovery

`discover` parses and validates the config and returns one
`DiscoveredDevice` per entry. It performs no I/O.

| Field | Value |
|---|---|
| Fingerprint | `net:pixel:<id>`, so a host or name change keeps the device and its layout |
| Name | `name`, else `id` |
| Vendor, family | `Pixel Controller` |
| Model | Protocol label (`E1.31 (sACN)`, `Art-Net`, `DDP`) |
| Segments | One strip per configured segment; RGBW for four-channel orders |
| Capabilities | Direct control and brightness; `max_fps` or 60 |
| Connect behavior | `AutoConnect` |
| Identity claim | None: identity comes from config, not from hardware |
| Metadata | `ip`, `port`, `protocol`, `universes`, and `pixel_device` |

`pixel_device` is the device's effective config as JSON, with the
driver-level E1.31 defaults already applied. The daemon adopts a device from
its stored discovery metadata before every connect, so the backend resolves
the plan from that JSON and needs no config of its own.

### 11.4 Backend

`PixelBackend` keeps adopted plans and connected sessions. A session holds
the plan, a reusable channel stream buffer, and per-universe packets whose
fixed headers are built once at connect. E1.31 and DDP share one IPv4 and
one IPv6 socket, bound lazily on the unspecified address with an ephemeral
port; Art-Net uses the 6454 socket from section 5.4. No socket enables
`SO_BROADCAST`. Writes lock the device session, so frames to one device
never interleave. `frame_sink` returns a cloneable lane over the same
session, and `deliver_colors_shared_observed` reports transport start once
the session is held.

The core output queue keeps a device's frame sink after disconnect until it
removes the queue, so the session carries a closed flag. Disconnect sets it
while holding the session lock, then sends the shutdown frames. Every later
write through the backend or a retained sink fails with `Disconnected`
before any transport I/O, and the sink's `is_closed` reports it without
blocking.

---

## 12. Testing Strategy

No test sends a packet off the host. Every receiver binds `127.0.0.1`.

**Golden bytes** (`hypercolor-pixel-protocols/tests`). Expected packets are
assembled field by field from the tables in sections 4-6, independent of the
encoder: E1.31 data packets at 3 and 512 slots, sync address and option bits,
UTF-8 source name truncation, the 49-byte sync packet, ArtDmx headers,
Port-Address composition, even-length padding, the 530-byte full universe,
DDP headers, multi-span frames with one push, and whole-pixel chunking.

**Planning** (`hypercolor-driver-pixel/tests/config_tests.rs`). Universe
splits at 510 and 512 channels, start channel zero-fill, segment jumps,
overlap and range rejection, DDP spans and data types, protocol-specific
field rejection, cross-device conflicts, and the TOML example from the user
docs parsed through the same `[drivers]` table the daemon reads. Bounds
tests reject a `u32::MAX` pixel count in 3-channel universes, segments whose
sum overflows `u32`, and a 4 GiB DDP device in well under a second, and a
million-universe device on the universe check before any run exists.
Address tests reject IPv4-mapped multicast, broadcast, and unspecified forms
plus 0.0.0.0/8 and IPv6 multicast, resolve `::ffff:10.0.0.5` to IPv4, catch
its universe conflict with `10.0.0.5`, and reject IPv6 Art-Net hosts.
`driver_tests.rs` runs discovery through a host with an installation id and
checks that derived CIDs are stable per installation, differ across
installations, and yield to an explicit `cid`.

**Loopback integration** (`backend_tests.rs`). The real backend is adopted
from discovery output and streams to a loopback receiver: multi-universe
E1.31 with per-universe sequences, sync packets with their own sequence,
black frame plus three terminated packets per universe on disconnect, silence
when nothing was sent, ArtDmx with Port-Address `0x0123` and zero-filled
lower slots, ArtDmx sent from source port 6454 while E1.31 keeps an
ephemeral port, DDP fragmentation with per-packet sequence numbers and push
on the last packet, segment offsets, black padding, frame sink delivery, a
retained sink rejected after disconnect with no transport start and no
packet, and cadence (no cap plus 800 ms keep-alive, or a 25 ms interval at
`max_fps = 40`, and three identical sends for E1.31 only).

**Output queue** (`hypercolor-core/tests/backend_manager_tests.rs`). A
backend that opts into three identical sends gets exactly three writes of an
unchanged frame and three more after a change; the existing test that an
opted-out backend sends an unchanged frame once still passes.

**WLED regression.** The WLED suite passes unchanged after the extraction:
93 tests across its two test files. The gate skips its two mDNS scanner
tests because they use real multicast DNS on every host interface.

**Wireshark dissectors.** `dumpcap` was not executable by the test user, so
live capture on `lo` was not possible. Instead the ignored
`wire_capture_fixture` test drives the real backend through one frame and
shutdown for each protocol, records what the loopback receivers got, and
writes text2pcap dumps; `text2pcap -4 127.0.0.1,127.0.0.1 -u <port>,<port>`
wraps them on the standard ports and `tshark -r` (Wireshark 4.7.4) dissects
the files:

| Protocol | Dissector | Frames | Result |
|---|---|---|---|
| E1.31 | stock ACN (`--enable-heuristic acn`, `acn.dmx_enable:TRUE`) | 12: data, sync, black, terminated | All decode (universe, sequence, sync universe 9, `Stream Terminated: True`); 0 malformed, 0 expert warnings |
| Art-Net | stock Art-Net | 4 ArtDMX | All decode (Universe 3 and 4, Length 510 and 4, sequences 1-2); 0 malformed |
| DDP | none in Wireshark (its `ddp` is AppleTalk) | 4 | Decoded by a 40-line Lua dissector written from the section 6.1 table (sequences 1-4, push on packets 2 and 4); 0 malformed |

Each check has a negative control that does fire: an E1.31 packet with a
corrupted root length, a truncated ArtDmx, and a DDP packet whose length
field disagrees with its payload are all reported malformed.

---

## 13. Hardware Validation

Nobody has run this driver against a real controller yet, so it ships with
the experimental label. Open items for the first hardware run:

1. Falcon F16V4/V5 and FPP over E1.31 unicast, including sync packets.
2. ESPixelStick over Art-Net and DDP with RGBW pixels.
3. An Art-Net node driving a DMX fixture at a nonzero start channel.
4. Whether common receivers accept unicast E1.31 sync packets.
5. Frame rate headroom at 60 fps across 10 or more universes.

---

## References

- ANSI E1.31-2018, *Entertainment Technology: Lightweight streaming protocol
  for transport of DMX512 using ACN*, ESTA. Tables 4-1 and 4-2; sections
  5.6, 6.2.3-6.2.7, 6.3, 6.6.1, 6.6.2, 6.7, 9.1, Appendix A.
  <https://tsp.esta.org/tsp/documents/docs/ANSI_E1-31-2018.pdf>. ESTA also
  lists an E1.31-2025 edition, which was not reviewed.
- ANSI E1.11-2008 (R2018), *USITT DMX512-A*, ESTA. Transmitter break, mark
  after break, and slot timing.
- *Art-Net 4 Specification*, Artistic Licence, document revision 1.4dp
  (2025-10-23). ArtDmx, ArtSync, Port-Address, and packet strategy sections.
  <https://art-net.org.uk/downloads/art-net.pdf>
- *DDP: Distributed Display Protocol*, 3waylabs (updated 2022-09-05).
  <http://www.3waylabs.com/ddp/>
- Falcon F16V4 and F16V5 product pages (E1.31, Art-Net, and DDP input).
  <https://pixelcontroller.com/store/products/70-f16v4.html>
- Falcon Player receiver bridge (E1.31, Art-Net, DDP input).
  <https://github.com/FalconChristmas/fpp>
- ESPixelStick input modules. <https://github.com/forkineye/ESPixelStick>
- WLED E1.31 and DDP interface documentation.
  <https://kno.wled.ge/interfaces/e1.31-dmx/>, <https://kno.wled.ge/interfaces/ddp/>
- Hyperion.ng and HyperHDR source trees, confirming E1.31, Art-Net, and DDP
  are output drivers only. <https://github.com/hyperion-project/hyperion.ng>,
  <https://github.com/awawa-dev/HyperHDR>
- Spec 35 (network driver architecture), Spec 50 (config registry), Spec 51
  (driver module API), Spec 52 (control surfaces), Spec 89 (experimental
  label).
