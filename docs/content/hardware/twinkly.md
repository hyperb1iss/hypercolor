+++
title = "Twinkly"
description = "Drive Twinkly strings, curtains, Squares, Flex, and trees per LED with Hypercolor over the local Twinkly API: discovery, the one-login rule, layouts from the Twinkly app, and the frame rate."
weight = 82
+++

Hypercolor drives Twinkly lights directly over the local API every Twinkly device runs.
No Twinkly account, cloud connection, or pairing step is involved: lights on the same
network are found by a UDP broadcast, Hypercolor logs in to each one, and every LED is
streamed in real time. Strings, clusters, icicles, curtains, Squares, Flex, and pre-lit
trees all work, and the layout you mapped in the Twinkly app becomes each LED's position
on the spatial canvas.

{% <callout type="warning"> %}
The Twinkly driver is **experimental**. It has been tested against a fake device that
follows the community protocol documentation, but not yet on real Twinkly hardware. It
is enabled by default like the other network drivers; if something looks wrong, please
open an issue with your product code and firmware version from the device diagnostics
panel.
{% </callout> %}

## Prerequisites

- The lights must be set up in the Twinkly app once so they join your Wi-Fi. Hypercolor
  does not provision new lights.
- The lights and the machine running Hypercolor must be on the **same local network** for
  automatic discovery, which uses a UDP broadcast. For lights on another VLAN, see
  [Manual IP configuration](#manual-ip-configuration).
- TCP port `80` and UDP ports `5555` and `7777` must not be blocked between the host and
  the lights.

## How discovery works

During a scan, Hypercolor:

1. **Broadcasts** the Twinkly discovery datagram to UDP port `5555` on every local IPv4
   network. Every Twinkly device answers.
2. **Probes known addresses**: every IP in `device_ips` and every Twinkly light found by an
   earlier scan, over HTTP, so lights stay reachable where broadcast is filtered.
3. Reads each device's details (product code, LED count, LED profile, firmware). This step
   needs no login.

```bash
hypercolor devices discover --target twinkly
```

Or use the **Discover** button on the Devices page in the web UI. Lights connect
automatically after discovery. On connect, Hypercolor logs in, reads the LED layout, and
switches the device to real-time mode.

## One login at a time

A Twinkly device honors only its most recent login. When the Twinkly app, Home Assistant's
Twinkly integration, or a second Hypercolor logs in, Hypercolor is signed out. Hypercolor
checks its session every 5 seconds, and the first time it finds itself signed out it
steps back instead of fighting for the lights: it stops sending colors, leaves the lights to
the other app, logs a warning that another app took control, and shows **Control** as
`yielded_to_another_app` in the device diagnostics.

After 30 seconds Hypercolor tries once to take the lights back. If the other app signs it
out again, the next pause doubles, up to 10 minutes; holding the lights for 10 minutes
resets it. Reconnecting the device (Refresh Layout, or disabling and enabling it) takes the
lights back right away.

- Close the Twinkly app while Hypercolor is driving the lights.
- Do not run Home Assistant's Twinkly integration against the same lights; it polls every
  30 seconds and keeps signing Hypercolor out.

Hypercolor also notices when something switches the lights to a movie or effect and puts
them back in real-time mode. When Hypercolor disconnects from the lights, for example
when you disable the device, it puts them back in the mode they were in before it connected.

## Supported devices

| Device | Examples | What you get |
|---|---|---|
| Strings and clusters | Strings 100 to 600, Cluster 400, Festoon, Icicle | Every LED, RGB or RGBW |
| Curtains and walls | Curtain, Wall Special Edition | Every LED at its mapped 2D position |
| Squares | 8x8 panels, up to 16 per chain | Every LED at its mapped position |
| Flex and Line | Flex tubes, light lines | Every LED along the strip or mapped shape |
| Trees and matrices | Pre-lit trees, Matrix | Every LED, 3D layouts seen from the front |

Both hardware generations are supported. The real-time format is picked from the firmware:
generation I devices use version 1 (up to 255 LEDs), generation II uses version 2 before
firmware 2.4.14 and version 3 from 2.4.14 on.

Not supported yet:

- **Gold Edition** lights with amber, warm, and cold white LEDs (the AWW profile). The byte
  order these take is undocumented, so they are skipped during discovery.
- **Twinkly Pro** controllers, which speak a different API.

## Layouts

Twinkly devices store the layout you mapped with the camera in the Twinkly app. Hypercolor
reads it at connect:

- A **2D or 3D mapping** places each LED at its measured position inside the device's
  zone. 3D layouts (trees) are viewed from the front.
- **No mapping** (a string straight out of the box) lays the LEDs out as a strip in wiring
  order, LED 1 nearest the controller.

After you remap the lights in the Twinkly app, open the device's diagnostics panel and run
**Refresh Layout** to reconnect and read the new layout. The zone keeps its place on the
canvas; only the LED positions inside it change.

## Frame rate

Twinkly does not document how fast its real-time path accepts frames, so Hypercolor streams
at the engine's default rate of 60 frames per second and always sends the newest frame.
The device's reported movie frame rate appears in diagnostics as **Reported Frame Rate**
for reference only. If a large installation stutters, lower the **Frame Rate** driver
setting:

```toml
[drivers.twinkly]
max_fps = 30
```

When an effect holds still, Hypercolor re-sends the last frame once a second so the lights
stay in real-time mode.

## Colors

Hypercolor sends 8-bit RGB per LED. On RGBW lights the white LED stays off for now, so whites
and pastels are mixed from the red, green, and blue LEDs. Hypercolor's device brightness
scales the frame itself and leaves the brightness you set in the Twinkly app untouched.

## Driver controls

| Setting | Default | Effect |
|---|---|---|
| Device IPs | empty | Addresses probed directly during discovery |
| Broadcast Discovery | on | Send the discovery datagram as a broadcast |
| Frame Rate | 60 | Real-time frames per second, 1 to 120 |

Per device, the diagnostics panel shows the IP and MAC address, product code, hardware
version, LED profile, real-time protocol version, layout source, firmware, and LED count.

## Manual IP configuration

```toml
# ~/.config/hypercolor/hypercolor.toml
[drivers.twinkly]
device_ips = ["192.168.1.40", "192.168.1.41"]
broadcast_discovery = true
```

Give your lights DHCP reservations so their addresses stay put. A light that moves to a new
address is still recognized by its MAC address.

## Port reference

| Port | Protocol | Purpose |
|---|---|---|
| 5555 | UDP | Discovery datagram and replies |
| 80 | TCP (HTTP) | Device details, login, layout, LED mode |
| 7777 | UDP | Real-time frames |

## Troubleshooting

**The lights are found but never change.** Another client probably holds the login. Close
the Twinkly app and disable other integrations, then wait a few seconds for Hypercolor to
log in again.

**The lights are not found.** Check that the host and lights share a subnet, or add the
lights to `device_ips`. Lights on a guest network with client isolation cannot be reached.

**Colors land in the wrong places.** Remap the lights in the Twinkly app, then run Refresh
Layout. Without a mapping, LEDs follow wiring order.

**A light is skipped with an "LED profile" warning.** It is a Gold Edition (AWW) device,
which is not supported yet.
