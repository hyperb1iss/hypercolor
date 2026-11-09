+++
title = "WiZ"
description = "Drive WiZ bulbs, strips, and fixtures with Hypercolor over the local WiZ UDP API: broadcast discovery, one color per light, verified-controls signing, and the update rate."
weight = 84
+++

Hypercolor drives WiZ lights (including Philips "WiZ Connected" products) directly over the
local UDP API every WiZ device runs. No WiZ account, cloud connection, or pairing step is
involved: lights on the same network are found by a UDP broadcast and follow your effects
in real time. Each WiZ light shows one color at a time, so each one becomes a single point
on the spatial canvas, the way Hue lights and Govee bulbs do.

{% <callout type="warning"> %}
The WiZ driver is **experimental**. It has been tested against a fake device built from
the community protocol documentation, but not yet on real WiZ hardware. It is enabled by
default like the other network drivers; if something looks wrong, please open an issue
with the module name and firmware version from the device diagnostics panel.
{% </callout> %}

## Prerequisites

- The lights must be set up in the WiZ app once so they join your Wi-Fi.
- **Local communication** must be allowed in the WiZ app (Settings, your home, Security).
  When it is disabled, lights ignore every local command.
- The lights and the machine running Hypercolor must be on the **same local network** for
  automatic discovery. For lights on another VLAN, see
  [Manual IP configuration](#manual-ip-configuration).
- UDP port `38899` must not be blocked between the host and the lights.

## How discovery works

During a scan, Hypercolor broadcasts a WiZ registration request to UDP port `38899` on
every local IPv4 network, probes every address in `device_ips` and every WiZ light found
earlier, and reads each light's module name, MAC address, and firmware. Smart plugs answer
too and are skipped.

```bash
hypercolor devices discover --target wiz
```

Lights connect automatically. The WiZ local API carries no light names, so lights appear as
`WiZ Color 1A2B3C` (the end of the MAC address); rename them in Hypercolor.

## Verified controls and the signing key

Newer WiZ firmware can require signed commands when your home's security setting in the
WiZ app is **Only verified controls**. Lights in such a home are discovered normally but
ignore Hypercolor's colors, and the daemon log shows a "WiZ device rejected a frame"
warning that mentions the signing key. Either:

- allow all local controls in the WiZ app's security settings, or
- export your home's signing key from the WiZ app (Home settings, Integrations, Local
  integrations). The export is a small JSON file; copy its `udp_signing_key` value, 32
  hexadecimal characters, into the config file:

```toml
# ~/.config/hypercolor/hypercolor.toml
[drivers.wiz]
signing_key = "0123456789abcdef0123456789abcdef"
```

One key covers every light in the home. Treat it as a secret: anyone with it can control
your lights on the local network. Hypercolor keeps it out of the driver settings panel and
its logs. If you rotate the key in the WiZ app, update the config.

## Supported devices

| Class | Examples | What Hypercolor sends |
|---|---|---|
| Color | A19/A60 color bulbs, GU10, filament color, light strips, fixtures | One color per frame, brightness as the WiZ dimming level |
| Tunable white | Tunable white bulbs and spots | Brightness from the frame; color temperature stays where you set it |
| Dimmable white | Dimmable bulbs and fan lights | Brightness from the frame |

WiZ light strips are a single color over the local API, including RGBIC strips. Smart plugs
are not lights and are skipped.

## Update rate

WiZ does not publish a rate limit. Hypercolor sends up to **20 color updates per second**
to each light, the top of the range several independent tools report as reliable, and
always sends the newest color. When an effect holds still, the last color is re-sent every
2 seconds in case a packet was lost. To change the rate:

```toml
[drivers.wiz]
max_fps = 10
```

## Colors

A WiZ color light keeps hue and saturation separate from brightness. Hypercolor sends the
frame's color at full strength plus a dimming percentage, so a dark red becomes full red at
a low dimming level. WiZ lights cannot dim below their minimum (usually 10%), so very dark
colors show at that minimum and black turns the light off. The white LEDs inside color
bulbs stay off for now, so whites are mixed from red, green, and blue.

## Driver controls

| Setting | Default | Effect |
|---|---|---|
| Device IPs | empty | Addresses probed directly during discovery |
| Broadcast Discovery | on | Send the registration broadcast |
| Frame Rate | 20 | Color updates per second, 1 to 60 |

`signing_key` is set only in the config file. Per device, the diagnostics panel shows the
IP and MAC address, module name, light class, minimum dimming, and firmware.

## Manual IP configuration

```toml
# ~/.config/hypercolor/hypercolor.toml
[drivers.wiz]
device_ips = ["192.168.1.50", "192.168.1.51"]
broadcast_discovery = true
```

Give your lights DHCP reservations so their addresses stay put. A light that moves to a new
address is still recognized by its MAC address.

## Port reference

| Port | Protocol | Purpose |
|---|---|---|
| 38899 | UDP | Discovery, configuration queries, and color updates |

## Troubleshooting

**Lights are found but never change.** Check the daemon log for "WiZ device rejected a
frame". A hint about the signing key means your home uses verified controls; see
[above](#verified-controls-and-the-signing-key). Also make sure local communication is not
disabled in the WiZ app.

**Lights are not found.** Check that the host and lights share a subnet, or add the lights
to `device_ips`.

**Colors lag or skip.** Lower `max_fps`. Some lights fade between colors on their own,
which softens fast effects.
