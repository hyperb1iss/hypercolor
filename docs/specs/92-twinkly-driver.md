# Spec 92: Twinkly Driver

> Network driver for Twinkly lights (strings, clusters, icicles, curtains,
> Squares, Flex, and the other Wi-Fi products) over the local REST API and the
> UDP real-time protocol. Covers discovery, the login and verify handshake, the
> gestalt and firmware reads, LED layout to spatial positions, the three
> real-time datagram versions, RGB and RGBW LED profiles, session upkeep when
> another client takes the token, and the frame rate.

**Status:** Implemented, experimental (unverified on hardware)
**Crate:** `hypercolor-driver-twinkly`
**Module path:** `hypercolor_driver_twinkly`
**Feature flag:** `hypercolor-driver-builtin/twinkly`, part of the default `network` bundle
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Device Registry](#2-device-registry)
3. [Discovery](#3-discovery)
4. [REST API](#4-rest-api)
5. [Authentication](#5-authentication)
6. [Real-time Protocol](#6-real-time-protocol)
7. [Layout to Spatial Positions](#7-layout-to-spatial-positions)
8. [Timing and Frame Rate](#8-timing-and-frame-rate)
9. [Color Encoding](#9-color-encoding)
10. [Driver Integration](#10-driver-integration)
11. [Testing Strategy](#11-testing-strategy)
12. [Risks and Open Questions](#12-risks-and-open-questions)
- [References](#references)

---

## 1. Overview

Twinkly sells Wi-Fi LED products where every LED is individually addressable.
Each device runs a small HTTP server for configuration and accepts real-time
frames as UDP datagrams. The driver:

1. finds devices with a UDP discovery datagram on port 5555, or probes
   configured addresses directly;
2. reads the gestalt and firmware version over REST (both unauthenticated);
3. at connect, logs in, verifies the token, reads the LED layout, and switches
   the device to real-time mode (`rt`);
4. streams every frame as one or more UDP datagrams on port 7777, in the
   datagram version the firmware expects;
5. keeps the session alive: it re-sends the last frame during silence, logs in
   again before the token expires or after another client takes it, and puts
   the device back in `rt` mode when something switched it away.

Sources: the community xled-docs (MIT), cited for every wire fact below; the
xled Python library (MIT), read for facts only and never copied; and, for
corroboration of individual facts, xled_plus (MIT), ttls (BSD-2-Clause), the
Home Assistant integration (Apache-2.0), and packet-capture notes in xLights
and LedFx issue threads (GPL-3.0, facts only). Twinkly publishes no official
local API documentation.

The driver is **experimental**: it is tested against an in-repo fake device,
not yet on real Twinkly hardware. Experimental is a label, not a gate. The
module is enabled by default like the other network drivers, and the label is
carried by the driver descriptor once the shared stability vocabulary lands
(Spec 89).

---

## 2. Device Registry

Twinkly devices identify themselves through the gestalt (section 4.3), so the
driver has no product table to match against: any device that answers the
gestalt and reports an RGB or RGBW profile is driven. Known products:

| Product code | Product | LEDs | Profile | Firmware family |
|---|---|---|---|---|
| TW105SEUP06 | Generation I Strings | 105 | RGB | D |
| TWS250STP | Strings 250 Multicolor | 250 (2 x 125) | RGB | F |
| TWS600STP | Strings 600 | 600 | RGB | (unknown) |
| TWC400STP | Cluster 400 | 400 | RGB | F |
| TWF020STP | Festoon 20 | 20 | RGB | F |
| TWW210SPP | Curtain or Wall Special Edition (10 x 21) | 210 | RGBW | G |
| TWI190SPP | Icicle Special Edition | 190 | RGBW | G |
| TWQ064STW | Squares panel (8 x 8, chains of up to 16) | 64 per panel | RGB | (unknown) |
| TWFL200STW, TWFL300STW | Flex tube | 200, 300 | RGB | (unknown) |
| TWW500STP | Matrix | 500 | RGB | (unknown) |
| TWS250GOP | Strings Gold Edition | 250 | AWW | (unknown) |

Product codes follow `TW` + product letter (`S` strings, `C` cluster, `F`
festoon, `W` wall or curtain or matrix, `I` icicle, `Q` squares, `FL` flex) +
LED count + edition (`ST` multicolor RGB, `SP` special edition RGBW, `GO`
gold) plus regional suffixes. The driver shows the product code as the model
and does not decode it.

Generations:

| Generation | Firmware family | Gestalt `fw_family` | Firmware range | Real-time datagram |
|---|---|---|---|---|
| I | D | absent | 1.99.18 to 2.3.8 | version 1 |
| II | F, G | present | 2.2.1 to 2.8.x and later | version 2 before 2.4.14, version 3 from 2.4.14 |

Out of scope: Twinkly Pro controllers (product code `TWPROCTRL...`) speak a
different `/xled/v2/` API whose replies nest the result code; the version 1
gestalt they do not answer makes the probe fail and the device is skipped.
Gold Edition devices report the `AWW` (amber, warm, cold white) profile, whose
byte order no source documents; the probe rejects the profile and the device
is skipped with a warning.

---

## 3. Discovery

### 3.1 Discovery datagram

The host sends one UDP datagram to port 5555, as an IPv4 broadcast (the
limited broadcast address and each interface's directed broadcast) when
broadcast discovery is enabled:

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | Version | 0x01 | Request version |
| 1 | 8 | Word | ASCII `discover` | 0x64 0x69 0x73 0x63 0x6f 0x76 0x65 0x72 |

Each device answers to the sender's address and port:

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 4 | Address | The device's IPv4 address with the octets reversed: byte 0 is the last octet |
| 4 | 2 | Marker | ASCII `OK` (0x4f 0x4b) |
| 6 | n | Device id | ASCII, for example `Twinkly_33AE5C` |
| 6+n | 1 | Terminator | 0x00 |

The driver takes the device address from the datagram's source address and
uses the reply only to learn that a device is there; the embedded address and
id are logged. A reply shorter than six bytes or without the marker is
ignored. The listen window is half the host's scan budget, clamped to 300 ms
to 2 s.

There is no mDNS advertisement (Home Assistant matches only DHCP hostnames
`twinkly_*` and `twinkly-*`).

### 3.2 Probing

Every address found by the datagram, every configured `device_ips` entry, and
every previously tracked device (at its recorded endpoint) is probed with two
unauthenticated REST reads, `GET /xled/v1/gestalt` and
`GET /xled/v1/fw/version`. Results are deduplicated by MAC address.

**Discovery never logs in.** A login invalidates every earlier token (section
5.2), so a login during a periodic scan would cut off an active Hypercolor
stream or the Twinkly app. Authentication waits for connect.

### 3.3 Bounds on discovery

Discovery replies are unauthenticated and anyone on the LAN can send them, so
a scan bounds what they can make it do:

| Bound | Value | Why |
|-------|-------|-----|
| Addresses admitted from replies | 128 per scan, one per source IP | A large holiday display runs a few dozen controllers; refused replies leave a coverage gap |
| Probes in flight | 16 | Each probe is two short HTTP requests on its own client; 128 candidates finish in eight rounds |
| Probe phase | the host's scan budget (at least one full probe round) | Unfinished probes are cancelled and leave a coverage gap |
| LED count | 1 to 4096, at most 255 on version 1, and a version 2 frame within one 65 507-byte datagram | Section 6.5; a device outside these is refused before it is published or anything is allocated |

Configured and tracked addresses are probed first and never count against
the reply cap.

A configured or tracked address that fails its probe has left, so it does not
affect coverage. A device that answered the datagram and then fails its probe
is alive, so it holds the device tracked at that address. With none tracked
there it could be any device, so it leaves a gap across the target and the
host retires nothing that scan.
Rejecting every datagram send leaves a gap too; one failed interface is
tolerated, since known devices are probed directly.

---

## 4. REST API

### 4.1 Basics

HTTP/1.1 on TCP port 80, base path `/xled/v1/`, JSON bodies. Every request
except `login`, `gestalt`, `fw/version`, and `status` needs the
`X-Auth-Token` header. A missing or stale token gets HTTP 401 with the body
`Invalid Token.`. An unknown endpoint gets HTTP 404 with `Resource not
found.`.

Replies carry an application `code` that does not track the HTTP status:

| Code | Meaning |
|------|---------|
| 1000 | Ok |
| 1001, 1102 | Error |
| 1101 | Invalid argument value |
| 1103 | Value too long or missing key |
| 1104 | Malformed JSON, or no movie set when switching to `movie` |
| 1105 | Invalid argument key |

The driver treats any code other than 1000 as an error, and HTTP 401 as a
distinct "token rejected" error that the upkeep loop answers with a fresh
login (section 5.3).

Two client rules protect against a misbehaving peer at a device address:

- **Reply size.** Bodies are read in chunks and refused past 256 KiB (a
  declared `Content-Length` over the cap is refused before reading). The
  largest legitimate reply is a layout of at most 4096 coordinates, about
  200 KiB. A layout listing more coordinates than 4096 is refused before the
  typed coordinate list is built.
- **No redirects.** The API lives at one fixed address, so redirects are never
  followed. reqwest strips standard credentials on cross-origin redirects but
  would forward the custom `X-Auth-Token` header.

### 4.2 Endpoints used

| Method | Path | Auth | Request body | Reply fields used |
|--------|------|------|--------------|-------------------|
| GET | `gestalt` | no | none | see 4.3 |
| GET | `fw/version` | no | none | `version` |
| POST | `login` | no | `{"challenge": "<base64 of 32 random bytes>"}` | `authentication_token`, `authentication_token_expires_in`, `challenge-response` |
| POST | `verify` | yes | `{"challenge-response": "<from login>"}` | `code` |
| GET | `led/mode` | yes | none | `mode` |
| POST | `led/mode` | yes | `{"mode": "rt"}` | `code` |
| GET | `led/layout/full` | yes | none | `source`, `coordinates` |

### 4.3 Gestalt fields

| Field | Type | Use |
|-------|------|-----|
| `mac` | string, six colon-separated hex octets | Identity (fingerprint and portable claim), lowercased |
| `number_of_led` | number | LED count |
| `led_profile` | `"RGB"`, `"RGBW"`, or `"AWW"` | Bytes and order per LED (section 9) |
| `bytes_per_led` | number, 3 or 4 | Profile fallback when `led_profile` is missing |
| `fw_family` | string, `"F"` or `"G"`; absent on generation I | Datagram version (section 6.4) |
| `device_name` | string | Display name |
| `product_code` | string | Model |
| `hardware_version` | numeric string | Diagnostics |
| `frame_rate` | number | Diagnostics only (section 8) |
| `product_name`, `uuid`, `max_supported_led`, `movie_capacity`, `led_type`, `wire_type` | various | Ignored |

### 4.4 LED modes

`off`, `color`, `demo`, `effect`, `movie`, `playlist` (2.5.6 and later), and
`rt`. Only `rt` displays real-time datagrams. After some time with no
datagrams the device leaves `rt` and returns to its movie (or, in a sync
group, the previous effect). No source states the timeout; clients that
re-arm do so after 50 to 60 s of idleness.

---

## 5. Authentication

### 5.1 Handshake

1. Generate 32 random bytes and `POST login` with their base64 as
   `challenge`.
2. The reply carries `authentication_token` (base64 of 8 bytes),
   `authentication_token_expires_in` (seconds; 14400 observed), and
   `challenge-response` (40 hex characters).
3. `POST verify` with the token in `X-Auth-Token` and the
   `challenge-response` echoed back. Verify must be the first call made with
   the new token; until it succeeds the token is not usable.

The `challenge-response` is an RC4 and SHA-1 construction over the challenge
and the device MAC that lets a client check the device is genuine. The driver
does not check it: nothing it does depends on device authenticity, and the
check needs the access point MAC rather than the LAN MAC on generation II.

The 8 decoded token bytes go into every real-time datagram (section 6).
`POST logout` exists but does not invalidate anything, so the driver never
calls it.

### 5.2 One token per device

A device honors only its most recent token. Any login, by the Twinkly app,
Home Assistant (which polls every 30 s and logs in again on 401), or a second
Hypercolor, invalidates the earlier token. Real-time datagrams carrying a dead
token are dropped silently; only REST calls reveal it, with a 401.

### 5.3 Upkeep

While Hypercolor holds the login, a background task runs every 5 s. It
always checks ownership first, with an authenticated `GET led/mode`:

| Result of the check | Action |
|---------------------|--------|
| 401 | Another client logged in: yield at once (section 5.4). Never log in again here |
| A mode other than `rt` | `POST led/mode rt` |
| `rt`, token within 5 minutes (or half its lifetime, whichever is less) of expiry | Log in, verify, hand the new token to the frame encoder, `POST led/mode rt`; a 401 anywhere in that refresh is a loss |
| Any other failure | Count it; after 3 consecutive failures the session reports a write error, and the host's lifecycle reconnects the device |

Checking first means a proactive refresh can never take the login back from
a client that has just logged in, even when the device reports a token
lifetime short enough to refresh on every cycle.

### 5.4 Ownership

A device honors one login, so any login by another client (the Twinkly app,
Home Assistant's integration, a second Hypercolor) signs Hypercolor out, and
logging in again would sign that client out in turn. Rate thresholds cannot
tell a rival from a glance: a client that logs in every 90 s never trips "3
losses in 2 minutes" and gets fought forever. The session therefore yields
on the first loss:

| State | Behavior |
|-------|----------|
| Owning | Section 5.3 |
| Yielded | No requests to the device; frames answered `Declined` (the device keeps what the other app set); metadata `control_state = yielded_to_another_app`; a warning that another app took control |

| Transition | Rule |
|------------|------|
| Owning to Yielded | Any 401 the session did not cause: the ownership check or a refresh |
| Yielded to Owning | When the pause ends, one resume: log in, verify, `POST led/mode rt`. Only when every step succeeds does the session clear `yielded` and own the lights again |
| Resume fails with a 401 | Another client is still active: yield again with the next, longer pause |
| Resume fails otherwise | Re-arm the same pause; the failure counts toward the 3-failure limit |

Pauses start at 30 s and double after each loss, up to 10 minutes; holding
the login for 10 minutes without a loss is the quiet period that resets the
pause to 30 s. The resume is the only way to learn whether the other client
went quiet, because every authenticated request needs a login; against a
persistent rival it costs one login per pause. A user action that reconnects
the device (Refresh Layout, or disabling and enabling it) starts a fresh
session that owns the lights immediately.

---

## 6. Real-time Protocol

### 6.1 Setup

Log in and verify (section 5.1), `POST led/mode {"mode":"rt"}`, then send
datagrams to UDP port 7777 from one persistent socket per device (a new socket
per frame exhausts ephemeral ports at 30 to 60 fps).

### 6.2 Frame body

A frame is every LED in wiring order (index 0 is closest to the controller),
each LED in the profile's byte order (section 9). Frame length is
`number_of_led x bytes_per_led`.

### 6.3 Datagram versions

All multi-byte values are single bytes; there is no endianness.

**Version 1** (generation I), one datagram per frame:

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | Version | 0x01 | |
| 1 | 8 | Token | decoded token | The 8 bytes, not the base64 text |
| 9 | 1 | LED count | 1 to 255 | LEDs in the body |
| 10 | n | Body | frame | Whole frame |

Generation I caps at 255 LEDs, matching the one-byte count; the encoder
refuses a larger device.

**Version 2** (generation II before firmware 2.4.14), one datagram per frame:

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | Version | 0x02 | |
| 1 | 8 | Token | decoded token | |
| 9 | 1 | Reserved | 0x00 | |
| 10 | n | Body | frame | Whole frame |

**Version 3** (generation II from firmware 2.4.14), frames split into
fragments of at most 900 bytes, each in its own datagram:

| Offset | Size | Field | Value | Description |
|--------|------|-------|-------|-------------|
| 0 | 1 | Version | 0x03 | |
| 1 | 8 | Token | decoded token | |
| 9 | 2 | Reserved | 0x00 0x00 | |
| 11 | 1 | Fragment | 0, 1, 2, ... | Fragment index within the frame |
| 12 | 1 to 900 | Body | frame slice | Bytes `900 x fragment` onward |

A 750-LED RGB frame (2250 bytes) is three datagrams of 900, 900, and 450 body
bytes. 900 bytes is exactly 300 RGB or 225 RGBW LEDs, which is where packet
captures of the official app cut. Fragments of one frame are sent back to
back. The last fragment is not padded.

### 6.4 Version selection

| `fw_family` | Firmware | Version |
|-------------|----------|---------|
| `"D"` | any | 1 |
| absent | before 2.4.14 | 1 (family D omits the field and stops at 2.3.8) |
| absent | 2.4.14 or later | 3 (cannot be family D) |
| any other | before 2.4.14 | 2 |
| any other | 2.4.14 or later | 3 |

No firmware is known between 2.4.6 (documented as version 2) and 2.4.14
(documented as version 3); the driver sends version 2 there.

Firmware versions are dotted numbers compared part by part (`2.10.0` is newer
than `2.9.99`); missing parts are zero. A version with a suffix such as
`2.0.22-mqtt` does not parse and fails the probe.

### 6.5 LED count limits

The LED count comes from an unauthenticated gestalt, so it is validated
before discovery publishes the device and before any buffer is sized:

| Limit | Value |
|-------|-------|
| Any device | 1 to 4096 LEDs (generation II reports at most 1200, Pro controllers 1500) |
| Version 1 | at most 255, the one-byte count |
| Version 2 | the whole frame plus its 10-byte header within one 65 507-byte UDP datagram |
| Version 3 | covered by the 4096 cap: at most 19 fragments |

A device outside the limits fails its probe with a warning and is never
published.

---

## 7. Layout to Spatial Positions

`GET led/layout/full` returns the mapping the user ran in the Twinkly app:

| Field | Meaning |
|-------|---------|
| `source` | `"linear"` (no mapping), `"2d"`, or `"3d"` |
| `synthesized` | Whether the app generated the layout rather than measuring it |
| `coordinates` | One `{x, y, z}` per LED, in frame order |

`x` and `z` are nominally in `[-1, 1]` and `y` in `[0, 1]`, with `y` growing
upward from the base. In `"2d"` layouts `z` is a detection flag (1 detected,
-1 estimated), not depth.

Mapping:

- `"2d"` or `"3d"` with exactly one coordinate per LED: one segment of custom
  positions (`LedTopology::Custom`). Each axis is scaled by its actual
  extent into `[0, 1]` (the nominal ranges are not trusted); `y` is flipped
  because canvas `y` points down; `z` is dropped, so 3D layouts are seen from
  the front. An axis with no extent puts every LED at 0.5.
- `"linear"`, a coordinate count that does not match the LED count, or a
  layout the device will not return: one strip segment in wiring order.

Discovery cannot read the layout (it needs a token), so discovery reports a
strip and the connected `DeviceInfo` replaces it with the layout. The
**Refresh Layout** device action reconnects to pick up a new mapping.

---

## 8. Timing and Frame Rate

### 8.1 Frame rate

No source documents a real-time frame rate ceiling. The gestalt `frame_rate`
field is undocumented; observed values fall as LED count rises (Festoon 20 at
200, Strings 250 at 24 to 30, Cluster 400 at 13, Strings 600 at 9, firmware
2.4.6 reporting a bogus 1), and they describe movie playback rather than what
the real-time path accepts: LedFx streams Squares at 62 fps by default and
users report 30 to 60 fps streaming.

The driver therefore reports a cadence of `max_fps` (default 60, the host's
default output cadence; configurable 1 to 120) and does not derive a lower cap
from `frame_rate`, which it publishes as diagnostics only. The host's output
queue is latest-value: when the device or network is slower than the cadence,
the newest frame replaces a pending one and nothing stale is queued.

### 8.2 Keepalive

The cadence carries a maximum frame silence of 1 s, so the host re-sends the
last frame when the effect is static and the device never times out of `rt`.

### 8.3 Timeouts

| Operation | Timeout |
|-----------|---------|
| One REST request | 3 s (scans use half the scan budget, 250 ms to 3 s) |
| Whole connect | 10 s |
| Discovery listen window | half the scan budget, 300 ms to 2 s |

---

## 9. Color Encoding

| Profile | Bytes per LED | Order |
|---------|---------------|-------|
| RGB | 3 | red, green, blue |
| RGBW | 4 | **white first**, then red, green, blue |

Engine frames are RGB. RGBW devices get a zero white byte, matching the color
kernel's `DevicePixelLayout::RgbwZeroWhite`; real white extraction is engine
work for later. Brightness from the host scales every channel in software with
`Rgb::scale`, so the device's own brightness setting (which persists in the
Twinkly app) is left alone. Frames shorter than the LED count are padded with
black and longer ones truncated, with a rate-limited warning.

---

## 10. Driver Integration

### 10.1 Module

`TwinklyDriverModule` implements `DriverModule` with id `twinkly`, transport
`Network`, discovery, config, controls, presentation, and an owned output
backend. No pairing: the login needs no user action.

### 10.2 Identity

Fingerprint `net:twinkly:<mac>` (lowercase, colon separated), device id minted
from it, and a portable MAC claim with the device address as the peer. A
connect re-reads the gestalt and refuses a device whose MAC no longer matches
the adopted id (a DHCP lease moved to a different light).

### 10.3 Configuration

```toml
[drivers.twinkly]
enabled = true
device_ips = []             # probed directly, for networks that filter broadcast
broadcast_discovery = true  # send the discovery datagram as a broadcast
max_fps = 60                # real-time frames per second, 1 to 120
```

### 10.4 Discovery metadata

| Key | Example |
|-----|---------|
| `ip`, `http_port`, `rt_port` | `192.168.1.40`, `80`, `7777` |
| `mac` | `98:f4:ab:1c:c1:90` |
| `firmware`, `fw_family` | `2.8.3`, `F` |
| `product_code`, `hardware_version` | `TWS250STP`, `100` |
| `led_profile`, `rt_protocol` | `RGB`, `v3` |
| `gestalt_frame_rate` | `23.81` |
| `layout_source` (after connect) | `3d` |

### 10.5 Backend

`TwinklyBackend` adopts the endpoint from metadata, opens a session on
connect (section 5.1 plus layout and `rt`), and hands the host a
`DeviceFrameSink` that encodes and sends datagrams.

Connect reads `GET led/mode` right after login and remembers the mode unless
it is already `rt`. Disconnect stops the upkeep task, closes the sink, and
puts that mode back with `POST led/mode`, bounded to 2 s and best effort (a
failure is logged at debug level). The restore is skipped when the session
is yielded to another app, so Hypercolor never takes the lights from it on
the way out, and when the session already failed.

### 10.6 Controls

Driver surface: `device_ips`, `broadcast_discovery`, `max_fps`. Device
surface: address, MAC, product code, hardware version, LED profile, real-time
protocol, layout source, reported frame rate, model, firmware, LED count, max
fps, state, and the Refresh Layout action.

---

## 11. Testing Strategy

Nothing touches the LAN. Every test binds 127.0.0.1.

- **Golden bytes** (`twinkly_protocol_tests.rs`): the discovery request,
  reply decoding (reversed octets, missing terminator, short and unmarked
  datagrams), version 1, 2, and 3 datagrams for RGB and RGBW, the 750-LED
  three-fragment split, padding and truncation, the 255-LED version 1 limit,
  firmware comparison, and the version selection table.
- **Golden JSON** (`twinkly_api_tests.rs`): login, verify, and mode bodies;
  login reply and token decoding; application error codes; both gestalt
  generations; layout normalization, Y flip, degenerate axes, and the strip
  fallbacks.
- **Loopback fake device** (`tests/support`): an HTTP server, a discovery
  responder, and a real-time receiver on 127.0.0.1, written independently of
  the crate's encoders. It enforces login then verify, invalidates the old
  token on every login, answers 401 to stale tokens, and records every request
  and datagram.
- **Scanner** (`twinkly_scanner_tests.rs`): the discovery datagram sent to the
  fake's loopback port, unicast probing, MAC deduplication, metadata, and the
  rule that scans never log in.
- **Backend** (`twinkly_backend_tests.rs`): the connect sequence, custom
  positions from a 2D layout, version 3 fragments with the verified token,
  RGBW white-first order on version 2, brightness scaling, re-login after a
  revoked token, restoring `rt` mode, disconnect stopping upkeep, refusing a
  different MAC at the adopted address, and the layout fallback.
- **Driver** (`twinkly_driver_tests.rs`): descriptor, defaults, validation,
  probe target merging, and both control surfaces.

---

## 12. Risks and Open Questions

1. **Real-time idle timeout** is undocumented. The 1 s keepalive and the
   upkeep's mode check cover it either way.
2. **Firmware 2.4.7 to 2.4.13** is unclassified; version 2 is a guess.
3. **Version 3 fragment index width**: documented as one byte after two zero
   bytes; some notes speculate a three-byte index. One byte covers 230 KB
   frames, far beyond any device.
4. **Maximum real-time rate** is unknown (section 8.1); the configurable
   `max_fps` is the escape hatch.
5. **Token contention**: Hypercolor yields to the Twinkly app or Home
   Assistant on the first lost login and retries after a growing pause
   (section 5.4); the two cannot share a device.
6. **Sync groups**: a group master's layout covers its members; streaming to
   a grouped device is untested in every source.
7. **AWW profile and the `/xled/v2` Pro API** are unsupported (section 2).

---

## References

| Source | URL | License | Used for |
|--------|-----|---------|----------|
| xled-docs, protocol details | https://xled-docs.readthedocs.io/en/latest/protocol_details.html | MIT | Discovery, handshake, LED modes, frame format, real-time versions 1 to 3, groups |
| xled-docs, REST API reference | https://xled-docs.readthedocs.io/en/latest/rest_api.html | MIT | Endpoints, auth exceptions, application codes, gestalt, layout, LED mode |
| xled-docs, firmware and hardware pages | https://github.com/scrool/xled-docs | MIT | Firmware families and versions, generations |
| xled-docs issues #8, #22, #23 | https://github.com/scrool/xled-docs/issues | issue text | One token per device, coordinate ranges, group layouts |
| xled library | https://github.com/scrool/xled | MIT | Facts only: datagram construction cross-check |
| xled_plus | https://github.com/Anders-Holst/xled_plus | MIT | Facts only: 50 s re-arm, axis scales |
| ttls | https://github.com/jschlyter/ttls | BSD-2-Clause | Facts only: 401 handling, `/xled/v2` Pro API |
| Home Assistant twinkly integration | https://github.com/home-assistant/core/tree/dev/homeassistant/components/twinkly | Apache-2.0 | Facts only: DHCP hostname matching, 30 s polling |
| LedFx Twinkly Squares | https://github.com/LedFx/LedFx | GPL-3.0 | Facts only: 62 fps default, 8 x 8 panels |
| xLights Twinkly output | https://github.com/xLightsSequencer/xLights | GPL-3.0 | Facts only: 60 s re-arm, 900-byte fragments in captures |
| Twinkly help center, LED profiles | https://help.twinkly.com | proprietary | Facts only: RGB, RGBW, AWW profiles |
