+++
title = "Yeelight"
description = "Drive Yeelight bulbs, strips, and lamps with Hypercolor over Yeelight LAN Control: enabling LAN Control, discovery, music mode for real-time streaming, and the command quota."
weight = 86
+++

Hypercolor drives Yeelight Wi-Fi lights over the official Yeelight LAN Control protocol. No
Yeelight or Xiaomi account and no cloud connection is involved. Each Yeelight light shows one
color at a time, so each one becomes a single point on the spatial canvas, the way Hue
lights and Govee bulbs do.

{% <callout type="warning"> %}
The Yeelight driver is **experimental**. It has been tested against a fake device that
follows the official specification, but not yet on real Yeelight hardware. It is enabled by
default like the other network drivers; if something looks wrong, please open an issue with
the model and firmware from the device diagnostics panel.
{% </callout> %}

## Turn on LAN Control first

Every Yeelight light must have **LAN Control** switched on, one light at a time, in the
Yeelight app: open the light, tap the **more** menu, choose **LAN Control**, and switch it on.
Older versions of the app called this **Developer mode**. A light with LAN Control off does
not answer discovery and refuses connections, so Hypercolor never sees it.

Some Xiaomi-branded lights (for example the Bedside Lamp 2 on newer firmware) no longer offer
LAN Control. Those lights cannot be driven by Hypercolor or any other local integration.

## Prerequisites

- LAN Control on for every light (above).
- The lights and the machine running Hypercolor on the **same local network** for automatic
  discovery, which uses multicast. For lights on another VLAN, see
  [Manual IP configuration](#manual-ip-configuration).
- UDP port `1982` and TCP port `55443` open from the host to the lights, and, for music mode,
  the lights able to open a TCP connection back to the host (see [Music mode](#music-mode)).

## How discovery works

During a scan, Hypercolor sends the Yeelight search to the multicast group
`239.255.255.250:1982` and directly to every address in `device_ips` and every Yeelight light
found earlier. Each light answers with its control address, id, model, and the commands it
supports. Discovery never opens a connection to the light.

```bash
hypercolor devices discover --target yeelight
```

Lights connect automatically. The name you set on the light (if any) becomes its name in
Hypercolor; otherwise it shows the model and the end of its id.

## Music mode

Outside music mode, a Yeelight light accepts **60 commands per minute** on a connection,
which is one color change per second. Music mode lifts that limit: Hypercolor asks the light
to connect back to it, and colors then stream with no quota.

Music mode is on by default for every light that supports it. When a light connects,
Hypercolor:

1. opens a temporary listening port on the address the light can reach,
2. asks the light to connect to it,
3. streams colors over that connection.

If the light cannot connect back (a host firewall blocking incoming TCP, or a light on another
VLAN that cannot route back), Hypercolor logs why and falls back to one update per second.
The diagnostics panel shows **Stream Mode** as `music` or `normal`, and **Music Mode** as
`failed` when the fallback happened. Allow incoming TCP from the lights to the Hypercolor host
to fix it.

In music mode Hypercolor sends up to **25 color updates per second**, matching the most widely
used Yeelight streaming setup. Yeelight does not document a music-mode limit, and some lamps
handle less; lower the rate if colors lag:

```toml
[drivers.yeelight]
max_fps = 10
```

## The command quota outside music mode

Without music mode, Hypercolor paces itself at one command per second per light and pauses
for 15 seconds if a light reports its quota exhausted. A known Yeelight firmware bug stops the
quota from resetting after about 50 days of uptime; if a light stops responding to everything
after a long uptime, power-cycle it.

## Supported devices

| Light | Examples | What Hypercolor sends |
|---|---|---|
| Color | Color bulbs (`color`, `color1` to `color6`), light strips (`strip1` to `strip6`), Bedside Lamp, color lamps | One color and brightness per frame |
| White | White and tunable white bulbs (`mono`, `ct_bulb`), ceiling lights, desk lamps | Brightness from the frame |

Background (ambient) lights on dual-light fixtures, night light modes, and per-segment strips
are not driven yet; the main light follows your effects.

## Colors

Hypercolor sends each frame's color at full strength plus a brightness percentage, so a dark
red becomes full red at a low brightness. Black turns the light off. White lights follow the
frame's brightness and keep the color temperature you set in the Yeelight app.

## Driver controls

| Setting | Default | Effect |
|---|---|---|
| Device IPs | empty | Addresses searched directly during discovery |
| Multicast Discovery | on | Send the search to the multicast group |
| Music Mode | on | Stream without the quota on lights that support it |
| Frame Rate | 25 | Music-mode color updates per second, 1 to 60 |

Per device, the diagnostics panel shows the IP address, Yeelight id, light class, music mode,
stream mode, model, and firmware.

## Manual IP configuration

```toml
# ~/.config/hypercolor/hypercolor.toml
[drivers.yeelight]
device_ips = ["192.168.1.60", "192.168.1.61"]
multicast_discovery = true
```

Each address is searched directly on UDP port 1982. Give your lights DHCP reservations; a
light that moves is still recognized by its Yeelight id.

## Port reference

| Port | Protocol | Purpose |
|---|---|---|
| 1982 | UDP | Discovery search and replies |
| 55443 | TCP | Control connection to the light |
| ephemeral | TCP | Music mode: the light connects back to the Hypercolor host |

## Troubleshooting

**The light is not found.** LAN Control is almost always the reason: switch it on in the
Yeelight app. Then check that the host and light share a subnet, or add the light to
`device_ips`.

**The light changes only once a second.** Music mode failed or is unsupported. Check Stream
Mode in the diagnostics panel and allow incoming TCP connections from the light to the host.

**The light stops responding after weeks of uptime.** Power-cycle it to clear the firmware's
quota bug.
