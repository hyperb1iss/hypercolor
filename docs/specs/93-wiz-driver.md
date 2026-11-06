# Spec 93: WiZ Driver

> Network driver for WiZ (Signify) Wi-Fi bulbs, strips, and fixtures over the
> local UDP JSON API. Covers the message envelope, discovery by registration
> broadcast, the configuration queries, module name classes, color to
> `setPilot` conversion, request signing for homes set to "Only verified
> controls", and the update rate.

**Status:** Implemented, experimental (unverified on hardware)
**Crate:** `hypercolor-driver-wiz`
**Module path:** `hypercolor_driver_wiz`
**Feature flag:** `hypercolor-driver-builtin/wiz`, part of the default `network` bundle
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Packet Format](#2-packet-format)
3. [Discovery](#3-discovery)
4. [Configuration Queries](#4-configuration-queries)
5. [Request Signing](#5-request-signing)
6. [Module Classes](#6-module-classes)
7. [Timing and Update Rate](#7-timing-and-update-rate)
8. [Color Encoding](#8-color-encoding)
9. [Driver Integration](#9-driver-integration)
10. [Testing Strategy](#10-testing-strategy)
11. [Risks and Open Questions](#11-risks-and-open-questions)
- [References](#references)

---

## 1. Overview

WiZ lights (bulbs, filaments, spots, strips, and fixtures sold under WiZ and
Philips "WiZ Connected") listen for JSON requests on UDP port 38899. Every
device is one light with one color: the local API has no per-LED addressing,
so each WiZ device is a single-zone light in Hypercolor, sampled like Hue
lights and Govee bulbs.

The driver finds devices with a `registration` broadcast (or unicast probes),
reads `getSystemConfig` and `getModelConfig`, and streams one fire-and-forget
`setPilot` per frame.

Signify publishes no local API documentation. Sources: pywizlight (MIT), the
reference implementation behind Home Assistant's integration, including its
request signing support merged on 2026-11-04; the Home Assistant wiz
integration (Apache-2.0); homebridge-wiz-lan (Apache-2.0); and, facts only,
the openHAB WiZ binding (EPL-2.0) and community streaming tools. No source
code was copied.

The driver is **experimental**: tested against an in-repo fake device, not
yet on real WiZ hardware. The module is enabled by default like the other
network drivers; the label arrives with Spec 89.

---

## 2. Packet Format

### 2.1 Requests

One UDP datagram per request, UTF-8 JSON, no terminator:

| Key | Required | Meaning |
|-----|----------|---------|
| `method` | yes | Case-sensitive method name |
| `params` | yes | Object; `{}` for queries |
| `id` | no | Never needed; the driver omits it |
| `env`, `hmac` | signed requests only | Section 5 |

The driver writes every request by hand with a fixed key order, so the bytes
are reproducible and the signed `params` are exactly the bytes sent:

| Request | Bytes |
|---------|-------|
| Discovery | `{"method":"registration","params":{"phoneMac":"AAAAAAAAAAAA","register":false,"phoneIp":"1.2.3.4","id":"1"}}` |
| System configuration | `{"method":"getSystemConfig","params":{}}` |
| Model configuration | `{"method":"getModelConfig","params":{}}` |
| Color frame | `{"method":"setPilot","params":{"r":255,"g":128,"b":0,"dimming":50}}` |
| Color frame after off | `{"method":"setPilot","params":{"state":true,"r":255,"g":128,"b":0,"dimming":50}}` |
| White frame | `{"method":"setPilot","params":{"dimming":40}}` |
| Off | `{"method":"setPilot","params":{"state":false}}` |

### 2.2 Replies

Replies go to the request's source address and port:

```json
{"method":"setPilot","env":"pro","result":{"success":true}}
{"method":"getModelConfig","env":"pro","error":{"code":-32601,"message":"Method not found"}}
```

`method` echoes the request (replies to other methods are ignored), `env` is
usually `"pro"`, and exactly one of `result` or `error` is present. Some
clients append a NUL byte; the decoder trims trailing NULs.

| Error code | Meaning |
|------------|---------|
| -32700 | Parse error |
| -32601 | Method not found (older firmware, for example `getModelConfig` before about 1.22) |
| -32602 | Invalid params; also every unsigned `setPilot` in a verified-only home |
| -32605 | Security error: signature present but wrong |

### 2.3 Request and reply

Probes send a request and wait 400 ms for the matching reply, three attempts.
Streaming never waits: every frame is one datagram, and a lost frame is
superseded by the next.

---

## 3. Discovery

The discovery request (section 2.1) is a registration with `register` false,
which every device answers without subscribing the host to `syncPilot`
pushes. It is broadcast to UDP port 38899 on the limited broadcast address
and each interface's directed broadcast, from an ephemeral port, when
broadcast discovery is enabled. Replies arrive at that port:

```json
{"method":"registration","env":"pro","result":{"mac":"a8bb50000001","success":true}}
```

The driver takes the device address from the reply's source and probes it
(section 4), together with every configured `device_ips` entry and every
previously tracked device. The listen window is half the scan budget, 300 ms
to 2 s. Registration with `register` true (push subscription to port 38900)
is not used: control works without it.

### 3.1 Bounds on discovery

Registration replies are unauthenticated, so a scan bounds what they can
make it do:

| Bound | Value | Why |
|-------|-------|-----|
| Reply validity | `method` is `registration`, `success` is not false, `result.mac` is 12 hexadecimal digits | Junk and invented replies are dropped |
| Admission | each MAC once and each source IP once, at most 512 per scan | Replies from many ports or with many MACs cannot multiply probes; 512 matches the LIFX driver and covers the largest WiZ homes, and refused replies leave a coverage gap |
| Probes in flight | 32 | Each probe is one socket and two short requests |
| Probe phase | the host's scan budget (at least one round) | Unfinished probes are cancelled and leave a coverage gap |

Configured and tracked addresses are probed first and never count against
the admission cap.

A configured or tracked address that fails its probe has left, so it does not
affect coverage. A device that registered and then fails its probe is alive,
so it leaves a coverage gap and the host retires nothing that scan. Rejecting
every registration send leaves a gap too; one failed interface is tolerated,
since known devices are probed directly.

---

## 4. Configuration Queries

`getSystemConfig` (every firmware):

| Field | Example | Use |
|-------|---------|-----|
| `mac` | `a8bb5006033d` | Identity, canonicalized to `a8:bb:50:06:03:3d` |
| `moduleName` | `ESP01_SHRGB1C_31` | Class (section 6) and model; absent on very old firmware |
| `fwVersion` | `1.25.0` | Diagnostics |
| `homeId`, `roomId`, `typeId`, `drvConf` | | Ignored |

`getModelConfig` (firmware 1.22 and later; older firmware answers -32601):

| Field | Example | Use |
|-------|---------|-----|
| `minDimLevel` | `10` or `1` | Lowest `dimming` the device renders; 10 when absent |
| `nowc` | `1` or `2` | White channel count; recorded only |
| `cctRange` | `[2200,2700,6500,6500]` | Recorded only |

A device that does not answer `getModelConfig` keeps the defaults; one that
does not answer `getSystemConfig` fails the probe.

---

## 5. Request Signing

Newer firmware (reports cover 1.31 to 1.39 on ESP25 modules) rejects
unsigned `setPilot` with -32602 when the home's security setting in the WiZ
app is "Only verified controls". Queries and registration still work
unsigned. A signed request needs the home's signing key, which the user
exports from the WiZ app (Home settings, Integrations, Local integrations) as
a JSON file whose `udp_signing_key` field is 32 hexadecimal characters
(16 bytes). One key covers every device in a home.

Signing a `setPilot`:

1. Append `"sigTs":<Unix seconds>` as the last key inside `params`.
2. Take the compact `params` object text, braces included.
3. HMAC-SHA256 those UTF-8 bytes with the decoded key; base64 the digest.
4. Send `{"method":"setPilot","env":"pro","params":<that text>,"hmac":"<base64>"}`.

Golden example with key `00112233445566778899aabbccddeeff` and timestamp
1730000000:

```json
{"method":"setPilot","env":"pro","params":{"state":true,"r":255,"g":0,"b":0,"dimming":100,"sigTs":1730000000},"hmac":"lv1P36lNJKDafo7cIJYXrTNw+8eqew5V624gc67o2Ak="}
```

Devices accepted a `sigTs` 30 s old and rejected one 60 s old; future
timestamps and exact replays were accepted. Whether the device re-serializes
before verifying is unknown, so the driver signs the exact bytes it sends.
HMAC is implemented over the workspace `sha2` crate (RFC 2104) and checked
against RFC 4231 vectors; no new dependency.

The key lives in the driver config file as `signing_key`. It is validated on
load, never written into a control surface, and redacted from the debug
output of both the key and the driver config.
Moving it into the encrypted credential store is a follow-up (section 11).

---

## 6. Module Classes

The class comes from the second underscore-separated token of `moduleName`;
substrings are checked in order and the first match wins:

| Order | Substring | Class | Driven as |
|-------|-----------|-------|-----------|
| 1 | `RGB` | Color | color plus dimming |
| 2 | `DDTW`, `TW` | Tunable white | dimming from luma |
| 3 | `SOCKET` | Smart plug | skipped: not a light |
| 4 | anything else, or no module name | Dimmable white (including `FANDIMS` fan lights) | dimming from luma |

Examples: `ESP01_SHRGB1C_31`, `ESP03_SHRGBP_31`, `ESP20_DHRGB_01` (dual head),
`ESP25_SHORGB_01` (strip) are color; `ESP15_SHTW1_01I` is tunable white;
`ESP05_SHDW_21` and `ESP01_DIMTRIACS_01` are dimmable white; `ESP10_SOCKET_06`
is a plug.

---

## 7. Timing and Update Rate

### 7.1 Rate

WiZ publishes no rate limit. Community evidence, all fire and forget:

| Rate | Source |
|------|--------|
| 1 Hz | WLED wizlights usermod default |
| 10 Hz | tastywizzy ambilight default |
| 10 to 20 Hz | the range several independent clients report as reliable |
| 30 Hz | two screen-sync tools, unmeasured |
| "bursts can stop the device responding for a while" | openHAB binding docs, no number |

The driver reports a cadence of `max_fps` (default 20, configurable 1 to 60).
The host's output queue is latest-value, so a slow device sees the newest
color and nothing stale is queued. Nothing in the render loop changes.

### 7.2 Refresh

The cadence carries a maximum frame silence of 2 s: when the scene is
static, the host re-sends the last color so one lost datagram does not leave
the light wrong.

### 7.3 Power

`setPilot` with color or dimming turns a light on, but the driver adds
`"state":true` to the first lit frame after connect or after an off frame, to
be explicit. A frame that rounds to 0 % sends `{"state":false}`.

**Hardware validation item.** An effect that alternates black and lit frames
(a strobe, or a chase that leaves a single-zone light dark between hits)
produces an off and on pair per cycle, up to 20 power transitions a second at
the default rate. How WiZ firmware handles that is unmeasured: power changes
may run the device's own fade (`fadeIn` and `fadeOut`, 450 to 500 ms in
`getUserConfig`), lag behind color changes, or wear the relay or flash if a
model persists power state. Before the driver leaves experimental, measure
alternating black frames on real bulbs. If transitions prove costly, the
cheap fix is to send the dimming floor instead of off for black frames that
last less than a few hundred milliseconds.

---

## 8. Color Encoding

The engine samples one LED per light (a `Point` segment). A longer frame is
averaged to one color.

**Color lights.** The device keeps hue and saturation in `r`, `g`, `b` and
brightness in `dimming`, so the driver separates them:

```
peak    = max(R, G, B)
dimming = round(peak x 100 / 255), raised to minDimLevel; 0 means off
r, g, b = round(channel x 255 / peak)
```

`(128, 64, 0)` becomes `r 255, g 128, b 0, dimming 50`. Colors dimmer than
`minDimLevel` (usually 10 %) render at that floor, the dimmest the device
shows. The white LEDs (`c`, `w`) are left alone, matching the color kernel's
zero-white policy for RGBW output; white extraction is engine work for later.

**White lights** take Rec. 709 luma of the engine color as `dimming`, with
the same floor and off rule. The color temperature stays where the user set
it.

Host brightness scales the engine color with `Rgb::scale` before conversion.

---

## 9. Driver Integration

### 9.1 Module

`WizDriverModule`: id `wiz`, transport `Network`, discovery, config,
controls, presentation, owned output backend, no pairing.

### 9.2 Identity

Fingerprint `net:wiz:<mac>`, device id minted from it, portable MAC claim
with the device address as the peer. Connect re-probes and refuses a
different MAC at the adopted address and any smart plug. The local API has no
user-assigned name, so the display name is the class and MAC tail, for
example `WiZ Color 000001`; users rename in Hypercolor.

### 9.3 Configuration

```toml
[drivers.wiz]
enabled = true
device_ips = []              # probed directly on port 38899
broadcast_discovery = true   # send the registration broadcast
max_fps = 20                 # color updates per second, 1 to 60
# signing_key = "..."        # 32 hex characters from the WiZ app, verified-only homes
```

### 9.4 Discovery metadata

`ip`, `port`, `mac`, `module_name`, `module_class` (`rgb`, `tunable_white`,
`dimmable_white`), `firmware`, `min_dimming`.

### 9.5 Backend

One connected UDP socket per device. A watcher task reads replies to
streamed frames and logs, at most once a minute, when the device rejects
them, with a hint that names the signing key when the code points there.
Disconnect stops the watcher and closes the sink; the light keeps its last
color.

### 9.6 Controls

Driver surface: `device_ips`, `broadcast_discovery`, `max_fps`. Device
surface: address, MAC, module, class, minimum dimming, model, firmware, zone
count, max fps, state.

---

## 10. Testing Strategy

Every test binds 127.0.0.1.

- **Golden JSON** (`wiz_protocol_tests.rs`): every request string, the signed
  `setPilot` above, reply decoding with and without a NUL, device errors and
  mismatched methods, both configuration replies, the class table, color and
  white conversion including the floor and off cases, RFC 4231 HMAC vectors,
  and key parsing and redaction.
- **Loopback fake device** (`tests/support`): answers registration and both
  queries, optionally reports -32601 for `getModelConfig`, and in
  verified-only mode checks each `setPilot` signature with an HMAC computed
  independently in the test.
- **Scanner**: the registration path against the fake's loopback port,
  unicast probing, MAC deduplication, plugs skipped, silent addresses
  skipped, and the discovered device shape.
- **Backend**: the `setPilot` sequence (power on first, plain color, off,
  power on again), brightness, white bulbs, signed frames accepted, unsigned
  frames rejected by a verified home, disconnect, and a different MAC at the
  adopted address.
- **Driver**: descriptor, defaults, validation including the key, probe
  targets, and that the key never reaches a control surface.

---

## 11. Risks and Open Questions

1. **Verified-only homes**: the firmware boundary and whether new homes
   default to verified mode are unknown. The driver signs when a key is set
   and warns with a hint when frames are rejected.
2. **Signing key storage**: the key sits in the config file. A follow-up
   should move it to the credential store through a pairing-style flow.
3. **Rate**: no measured safe rate exists; `max_fps` is the escape hatch.
4. **RGBIC strips** (`ESP25_MHORGB_01`) accept up to 12 colored regions
   through an undocumented custom-scene `elm` payload verified on one device.
   The driver treats them as single-color strips.
5. **Dual-head fixtures** are driven as one light.
6. **Discovery reply port**: pywizlight receives replies on the sending
   socket; one source claims port 38900. The driver listens on the sending
   socket only.
7. **Power transitions** for alternating black frames are unmeasured
   (section 7.3).

---

## References

| Source | URL | License | Used for |
|--------|-----|---------|----------|
| pywizlight | https://github.com/sbidy/pywizlight | MIT | Envelope, methods, discovery packet, module classes, minimum dimming, signing (PR #216, issue #213) |
| Home Assistant wiz integration | https://github.com/home-assistant/core/tree/dev/homeassistant/components/wiz | Apache-2.0 | Facts only: white channel exposure, rediscovery cadence |
| homebridge-wiz-lan | https://github.com/kpsuperplane/homebridge-wiz-lan | Apache-2.0 | Registration reply shape |
| openHAB WiZ binding | https://github.com/openhab/openhab-addons | EPL-2.0 | Facts only: registration semantics, burst warning, firmware 1.22 boundary |
| openHAB WiZ binding docs | https://www.openhab.org/addons/bindings/wiz/ | EPL-2.0 project | Facts only: burst warning |
| WiZ FAQ 548, local network communication | https://faq.wizconnected.com/hc/en/7-wiz-v2/faq/548-disabling-local-network-communication | proprietary | Facts only: verified controls setting |
| WLED wizlights usermod | https://github.com/eibanez/wled-usermod-wizlights | EUPL-1.2 | Facts only: 1 Hz default |
| tastywizzy | https://github.com/L0R3Z/tastywizzy | none stated | Facts only: 10 Hz default |
| wiz-connected-screen-sync | https://github.com/shreyaskothakonda/wiz-connected-screen-sync | none stated | Facts only: 30 Hz claim |
| WizScreenSyncController | https://github.com/TechAntohere/WizScreenSyncController | none stated | Facts only: RGBIC `elm` payload |
| RFC 2104, RFC 4231 | https://www.rfc-editor.org/rfc/rfc2104 , https://www.rfc-editor.org/rfc/rfc4231 | IETF | HMAC construction and test vectors |
