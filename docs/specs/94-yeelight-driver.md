# Spec 94: Yeelight Driver

> Network driver for Yeelight Wi-Fi bulbs, strips, and lamps over the
> official Yeelight WiFi Light Inter-Operation Specification: the SSDP-style
> search on UDP port 1982, newline-delimited JSON commands on TCP port
> 55443, the documented command quota, and music mode, where the device
> connects back to the host and the quota is lifted. Covers discovery,
> command encoding, color conversion, pacing, and the LAN Control
> prerequisite.

**Status:** Implemented, experimental (unverified on hardware)
**Crate:** `hypercolor-driver-yeelight`
**Module path:** `hypercolor_driver_yeelight`
**Feature flag:** `hypercolor-driver-builtin/yeelight`, part of the default `network` bundle
**Author:** Nova
**Date:** 2026-11-05

---

## Table of Contents

1. [Overview](#1-overview)
2. [Prerequisites](#2-prerequisites)
3. [Discovery](#3-discovery)
4. [Control Channel](#4-control-channel)
5. [Commands and Color](#5-commands-and-color)
6. [Timing, Quota, and Music Mode](#6-timing-quota-and-music-mode)
7. [Capabilities and Models](#7-capabilities-and-models)
8. [Driver Integration](#8-driver-integration)
9. [Testing Strategy](#9-testing-strategy)
10. [Risks and Open Questions](#10-risks-and-open-questions)
- [References](#references)

---

## 1. Overview

Yeelight publishes an official LAN control specification (the "Yeelight WiFi
Light Inter-Operation Specification"), cited directly here because it is
vendor documentation. Every Yeelight Wi-Fi light answers a search, takes JSON
commands over TCP, and shows one color at a time, so each light is a
single-zone device in Hypercolor, sampled like Hue lights and Govee bulbs.

The driver:

1. searches by multicast (or unicast to configured addresses) and reads the
   control address, id, model, and supported methods from the reply;
2. at connect, opens the control connection and asks the device to connect
   back for music mode;
3. streams one `set_scene` per frame on the music connection, or, without
   music mode, paces commands inside the documented quota.

Secondary sources, facts only: python-yeelight (BSD-2-Clause), the Home
Assistant yeelight integration (Apache-2.0 code, CC BY-NC-SA documentation),
Hyperion's Yeelight device (MIT), and Yeelight forum staff answers. No source
code was copied.

The driver is **experimental**: tested against an in-repo fake device, not
yet on real Yeelight hardware. The module is enabled by default like the
other network drivers; the label arrives with Spec 89.

---

## 2. Prerequisites

Every light needs **LAN Control** switched on in the Yeelight app (device,
"more" menu, LAN Control; older apps called it "Developer mode"). Without it
the light ignores the search and refuses TCP connections. Messages are
unencrypted; the LAN is the security boundary.

Several Xiaomi-branded products lost the LAN Control option through firmware
or app updates (Bedside Lamp 2 after firmware 2.0.6_0041, with user reports
for Bedside Lamp 1 and Mi Desk Lamp 1S). No official list exists. A light
without LAN Control cannot be driven by any local client.

---

## 3. Discovery

### 3.1 Search

The search is one UDP datagram, every line ending CRLF, with no trailing
blank line:

```
M-SEARCH * HTTP/1.1\r\n
HOST: 239.255.255.250:1982\r\n
MAN: "ssdp:discover"\r\n
ST: wifi_bulb\r\n
```

| Rule | Value |
|------|-------|
| Destination | Multicast `239.255.255.250:1982`, or unicast `<ip>:1982` |
| `MAN` | Exactly `"ssdp:discover"`, quotes included |
| `ST` | Exactly `wifi_bulb` |
| Case | Header names case-insensitive; the start line and values case-sensitive |
| Replies | Unicast to the requester's address and port; invalid requests are dropped |

The driver sends three rounds spread over the listen window (half the scan
budget, 500 ms to 3 s), to the multicast group when multicast discovery is
on and to every configured address and previously tracked device on port
1982.

### 3.2 Reply

```
HTTP/1.1 200 OK
Cache-Control: max-age=3600
Date:
Ext:
Location: yeelight://192.168.1.239:55443
Server: POSIX UPnP/1.0 YGLC/1
id: 0x000000000015243f
model: color
fw_ver: 18
support: get_prop set_default set_power toggle set_bright start_cf stop_cf set_scene cron_add cron_get cron_del set_ct_abx set_rgb
power: on
bright: 100
color_mode: 2
ct: 4000
rgb: 16711680
hue: 100
sat: 35
name: my_bulb
```

| Header | Use |
|--------|-----|
| `Location` | `yeelight://ip:port`, the TCP control endpoint |
| `id` | 64-bit hex device id: identity, lowercased |
| `model` | Model string (section 7) |
| `fw_ver` | Firmware, diagnostics |
| `support` | Space-separated methods the device accepts; anything else is rejected |
| `name` | Name stored on the device, used as the display name when set |
| `power`, `bright`, `color_mode`, `ct`, `rgb`, `hue`, `sat` | Current state; ignored |
| `Cache-Control`, `Date`, `Ext`, `Server` | SSDP compatibility; ignored |

The decoder splits each line on its first colon and trims, so empty values
such as `Date:` parse, and accepts `NOTIFY * HTTP/1.1` advertisements, which
carry the same headers. A reply without `Location` or `id` is rejected.

**Discovery never opens a control connection.** A device allows only four,
and the search reply already says everything the driver needs.

---

## 4. Control Channel

TCP to the `Location` port (55443 in practice). Every message is one JSON
object followed by CRLF.

| Kind | Direction | Shape |
|------|-----------|-------|
| Command | host to device | `{"id":1,"method":"set_power","params":["on","smooth",500]}` |
| Result | device to host | `{"id":1,"result":["ok"]}` |
| Error | device to host | `{"id":2,"error":{"code":-1,"message":"unsupported method"}}` |
| Notification | device to host | `{"method":"props","params":{"power":"on","bright":"10"}}` |

`params` is positional. Error code -1 is overloaded (`unsupported method`,
`client quota exceeded`, `method not supported`), so the message text is the
discriminator. `props` notifications are pushed to every connection on any
state change. A background task reads the control connection, skips results
and notifications, and acts on quota errors (section 6.1).

---

## 5. Commands and Color

### 5.1 Encoding

Commands are written by hand with a fixed key order:

| Command | Line (before CRLF) |
|---------|--------------------|
| Power on | `{"id":N,"method":"set_power","params":["on","sudden",30]}` |
| Power off | `{"id":N,"method":"set_power","params":["off","sudden",30]}` |
| Color scene | `{"id":N,"method":"set_scene","params":["color",65280,70]}` |
| Color | `{"id":N,"method":"set_rgb","params":[16711680,"sudden",30]}` |
| Brightness | `{"id":N,"method":"set_bright","params":[70,"sudden",30]}` |
| Music start | `{"id":N,"method":"set_music","params":[1,"192.168.0.2",54321]}` |
| Music stop | `{"id":N,"method":"set_music","params":[0]}` |

`rgb` is `R x 65536 + G x 256 + B`; brightness is 1 to 100. `sudden` ignores
the duration, but the positional value must be at least 30 ms. `set_scene`
takes no transition parameters because some older firmware rejects them.
Ids count up from 1 per connection.

### 5.2 Color conversion

The engine samples one LED per light (a `Point` segment); a longer frame is
averaged. Host brightness scales the engine color with `Rgb::scale` first.

| Light | State | Rule |
|-------|-------|------|
| Color | `Color { rgb, bright }` | `bright = round(max(R,G,B) x 100 / 255)`; `rgb` is the color scaled so its brightest channel is 255 |
| White | `White { bright }` | `bright` from Rec. 709 luma |
| Either | `Off` | the level rounds to 0 % |

`(128, 64, 0)` on a color light is `rgb 0xFF8000, bright 50`.

### 5.3 State to commands

| From | To | Commands |
|------|----|----------|
| any | Off | `set_power off` |
| any | Color, `set_scene` supported | `set_scene ["color", rgb, bright]` (turns the light on first if off) |
| off or unknown | Color, no `set_scene` | `set_power on`, `set_rgb`, `set_bright` |
| Color | Color, no `set_scene` | only the changed `set_rgb` and `set_bright` |
| off or unknown | White | `set_power on`, `set_bright` |
| White | White | `set_bright` |
| a state | the same state | nothing (`SuppressedDuplicate`) |

---

## 6. Timing, Quota, and Music Mode

### 6.1 Normal mode quota

The specification limits each device to 4 TCP connections, 60 commands per
minute per connection, and 144 per minute across the LAN. Overflow answers
`{"code":-1,"message":"client quota exceeded"}`; Yeelight staff said
responses count toward the limit. A known firmware bug stops the quota from
refilling after about 49.7 days of uptime until the light is power-cycled.

Without music mode the driver stays inside the per-connection quota:

- The cadence interval is one second per command for the device's worst-case
  frame (1 for `set_scene` lights, 2 for white lights, 3 for color lights
  without `set_scene`), reported as 1 fps.
- A write that arrives before the previous frame's commands have used up
  their seconds returns `SuppressedCadence` without touching the device.
- A quota error pauses output for 15 s.
- A 3 s maximum frame silence makes the host replay the newest frame, so a
  frame the guard skipped is still delivered; replaying an unchanged state
  costs no command.

### 6.2 Music mode

Music mode is the specification's streaming path:

1. Open a TCP listener on the host address the control connection uses
   (its local address, which the device can reach), on an ephemeral port.
2. Send `set_music [1, host, port]` on the control connection and wait up to
   2 s for the result.
3. Accept the device's connection within 3 s.
4. Send commands on that connection. The device checks no quota and sends
   nothing back.
5. Stop with `set_music [0]` on the control connection (sent on disconnect,
   best effort), or by closing the music connection.

Music mode needs `set_music` in `support`, IPv4, and a path from the device
back to the host. When any step fails, the driver logs why and streams in
normal mode.

No source documents a music-mode limit. Hyperion, the most deployed
streaming client, defaults to one update every 40 ms; one forum user found
4 Hz reliable over TCP on one lamp. The driver's music-mode cadence is
`max_fps`, default 25 (configurable 1 to 60), and the host's latest-value
queue never sends stale frames. A 5 s keepalive re-sends the current state
when nothing changes, so a dropped music connection fails the next write and
the host reconnects the device.

---

## 7. Capabilities and Models

From `support`:

| Methods | Meaning |
|---------|---------|
| `set_rgb` | Color light |
| `set_scene` | One-command color and brightness |
| `set_bright` | Brightness (every drivable light has it) |
| `set_music` | Music mode |

A light with neither `set_rgb` nor `set_bright` is skipped. Background
(`bg_*`) lights on dual-light fixtures, night light modes, and segment
methods (`set_segment_rgb` on `lamp15`, undocumented) are not driven.

Models seen in the wild include `mono`, `color` (with `color1` to `color6`),
`ct_bulb`, `stripe` and `strip1` to `strip6`, `bslamp1` to `bslamp3`,
`ceiling1` to `ceiling24`, and `lamp1` to `lamp19`. The driver keys behavior
on `support`, not on the model string, and shows the model as-is.

---

## 8. Driver Integration

### 8.1 Module

`YeelightDriverModule`: id `yeelight`, transport `Network`, discovery,
config, controls, presentation, owned output backend, no pairing.

### 8.2 Identity

Fingerprint `net:yeelight:<id>`, device id minted from it, and a portable
driver-identifier claim in the `yeelight` namespace with the device address
as the peer. Connect re-runs the search unicast against the adopted address
(returning on the first reply) and refuses a different id; a device that
does not answer keeps its adopted route and the control connection settles
reachability.

### 8.3 Configuration

```toml
[drivers.yeelight]
enabled = true
device_ips = []             # searched directly on port 1982
multicast_discovery = true  # send the search to 239.255.255.250:1982
music_mode = true           # stream through music mode when supported
max_fps = 25                # music-mode updates per second, 1 to 60
```

### 8.4 Metadata

`ip`, `port` (control), `search_port`, `yeelight_id`, `model`, `firmware`,
`support`, `light_class` (`color` or `white`), `music_mode` (`on`, `off`,
`unsupported`, or `failed` after connect), and `stream_mode` (`music` or
`normal`, after connect).

### 8.5 Backend

One control connection per device, plus the music connection when music
mode is on. Disconnect closes the sink, sends `set_music [0]`, and drops both
connections; the light keeps its last color.

### 8.6 Controls

Driver surface: `device_ips`, `multicast_discovery`, `music_mode`, `max_fps`.
Device surface: address, Yeelight id, light class, music mode, stream mode,
model, firmware, zone count, max fps, and state.

---

## 9. Testing Strategy

Every test binds 127.0.0.1.

- **Golden bytes** (`yeelight_protocol_tests.rs`): the search request and
  multicast address, reply decoding (case-insensitive headers, empty values,
  `NOTIFY`, malformed replies), a reply round trip, every command line,
  results, quota and other errors, notifications, capability flags, color
  and white conversion, and the state-to-command table with the worst-case
  command counts.
- **Loopback fake device** (`tests/support`): answers the search with a
  `Location` pointing at its control port, answers every command, connects
  back on `set_music` and records what arrives on the music connection, and
  can report quota exhaustion or never connect back.
- **Backend** (`yeelight_backend_tests.rs`): a search that opens no control
  connection, non-drivable devices skipped, music mode carrying four frames
  as `set_scene` and `set_power` with nothing on the control connection,
  duplicate suppression, normal-mode pacing, the quota pause, falling back
  when the device never connects back, white lights powering on then
  dimming, disconnect sending `set_music [0]`, and refusing a different id.
- **Driver** (`yeelight_driver_tests.rs`): descriptor, defaults, validation,
  probe targets, and both control surfaces.

---

## 10. Risks and Open Questions

1. **Music-mode throughput** varies by model (reports from 4 Hz to 25 Hz);
   `max_fps` is the escape hatch.
2. **Responses counting toward the quota** could halve the effective normal
   mode rate. The quota pause absorbs an overflow.
3. **The 49.7-day quota bug** needs a power cycle; the driver can only log
   it.
4. **UDP session mode** on port 55444 (`udp_sess_new`) is unimplemented; its
   quota status is undocumented.
5. **Background and segment lights** are not driven.

---

## References

| Source | URL | License | Used for |
|--------|-----|---------|----------|
| Yeelight WiFi Light Inter-Operation Specification | https://www.yeelight.com/download/Yeelight_Inter-Operation_Spec.pdf | vendor documentation | Search, reply headers, control framing, methods, quota, music mode |
| Yeelight Inter-Operation Specification, UDP R2 | https://github.com/Yeelight/Yeelight-Chroma-Connector/blob/main/Yeelight_Inter_Operation_Spec_UDP_R2_en.pdf | vendor documentation | UDP session mode (not implemented) |
| Yeelight LAN Control FAQ | https://www.yeelight.com/faqs/lan_control | vendor documentation | LAN Control toggle |
| Yeelight developer page | https://www.yeelight.com/en_US/developer | vendor documentation | Scope, security note |
| Yeelight forum: client quota exceeded | https://forum.yeelight.com/t/topic/15756 | forum posts, staff replies | Quota counting, the uptime bug |
| Yeelight forum: UDP vs TCP update rates | https://forum.yeelight.com/t/topic/29145 | forum posts, staff replies | Music-mode rates |
| Yeelight forum: Bedside Lamp 2 LAN control | https://forum.yeelight.com/t/topic/22664 | forum posts | Xiaomi-branded removal |
| python-yeelight | https://gitlab.com/stavros/python-yeelight | BSD-2-Clause-Views | Facts only: unicast search, model table, music flow, parsing caveats |
| Home Assistant yeelight integration and docs | https://github.com/home-assistant/core/tree/dev/homeassistant/components/yeelight , https://www.home-assistant.io/integrations/yeelight/ | Apache-2.0 code, CC BY-NC-SA 4.0 docs | Facts only: discovery cadence, Developer mode naming, music-mode caveats |
| Hyperion Yeelight device | https://github.com/hyperion-project/hyperion.ng/blob/master/libsrc/leddevice/dev_net/LedDeviceYeelight.cpp | MIT | Facts only: 40 ms music-mode default, `set_scene` without transitions, quota handling |
