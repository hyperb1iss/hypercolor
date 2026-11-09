+++
title = "LIFX"
description = "Drive LIFX bulbs, strips, beams, Tiles, Candles, and Ceilings with Hypercolor over the LIFX LAN protocol: broadcast discovery, manual IPs, zone mapping, and the message budget."
weight = 75
+++

{{< img path="img/ui/ui-devices.webp" alt="Device discovery in the Hypercolor web UI" />}}

Hypercolor drives LIFX lights directly over the official LIFX LAN protocol. No LIFX
account, cloud connection, or pairing step is involved: any LIFX light on the same network
is found by a UDP broadcast and streamed to in real time. Bulbs, multizone strips and
beams, and matrix products (Tile chains, Candle, Ceiling, Tube, Spot, Path, Luna, Mirror)
are all supported, and every zone becomes an addressable LED in the spatial canvas.

{% <callout type="warning"> %}
The LIFX driver is **experimental**. It has been tested against a protocol emulator but
not yet on real LIFX hardware. It is enabled by default like the other network drivers;
if something looks wrong, please open an issue with your product name and firmware
version from the device diagnostics panel.
{% </callout> %}

## Prerequisites

- The LIFX lights and the machine running Hypercolor must be on the **same local network**
  and subnet, because discovery uses a UDP broadcast. If your lights live on another VLAN,
  see [Manual IP configuration](#manual-ip-configuration).
- The lights need to be set up in the LIFX app once so they join your Wi-Fi. Hypercolor
  does not provision new lights.
- UDP port `56700` must not be blocked between the host and the lights.

## How discovery works

During a scan, Hypercolor sends a LIFX `GetService` message:

1. **Broadcast**: to `255.255.255.255:56700` and to each network interface's broadcast
   address. Every LIFX light on the subnet answers.
2. **Known IPs**: unicast to every address in `device_ips` and to every LIFX light found by
   an earlier scan, so lights stay reachable on networks that filter broadcast.

For each light that answers, Hypercolor reads its product, firmware version, label, and
zone layout (strip length, or the size and position of every tile in a chain). The label
you set in the LIFX app becomes the device name.

```bash
hypercolor devices discover --target lifx
```

Or use the **Discover** button on the Devices page in the web UI. Lights connect
automatically after discovery; when a light connects, Hypercolor turns it on.

LIFX Switches answer discovery too, but they have no lights and are skipped.

## Supported devices

| Device class | Examples | How Hypercolor paints it |
|---|---|---|
| Single zone | A19, BR30, GU10, Mini, Downlight, PAR38, Filament | One color per frame |
| Multizone strip | LIFX Z, Beam, Neon, String, Lightstrip, Permanent Outdoor | Every zone, 82 per message |
| Older multizone | First generation LIFX Z, and Z or Beam on firmware before 2.77 | Every zone, merged into runs of equal color |
| Matrix | Tile chains (up to 5), Candle, Ceiling, Ceiling 13x26, Tube, Spot, Path, Luna, Mirror | Every zone, one message per tile |

White-only lights (the LIFX White and White to Warm ranges) follow the brightness of the
effect, since they cannot show hue.

## Zones and layout

Each light appears as a device whose zones map into the spatial canvas:

- **Bulbs** are a single point.
- **Strips and beams** are a strip, zone 0 at the end nearest the controller.
- **Single matrix devices** (Tube, Spot, Path, Luna, Mirror) are a grid of their real size.
- **Tile chains** become one zone whose LED positions follow the arrangement you set in the
  LIFX app, including each tile's rotation. If you never arranged the tiles, they line up
  left to right in chain order.
- **Candle** maps its top zone above a five by five wrap.
- **Ceiling** gets two zones: the downlight grid and its uplight, so the uplight is not
  sampled from a corner of the grid.

If you add a tile or extend a strip, use **Refresh Topology** in the device diagnostics
panel (or run the `refresh_topology` action) so Hypercolor re-reads the layout.

```bash
hypercolor devices action <device-id> refresh_topology --yes
```

## Frame rate and the message budget

LIFX recommends sending a light no more than **20 messages per second**. Hypercolor counts
every message it sends a light against that budget, lets at most five go out back to back
(one frame of a five-tile chain), and spaces the rest. It always sends the newest frame:
frames that arrive while a light is busy replace each other instead of queueing, and a
quick frame (a solid color, an identify flash) takes over from a long one mid-way, so a
light never lags behind the effect. The render loop itself keeps running at full rate; only the
light's own update rate depends on how many messages each frame needs:

| Device | Messages per frame | Updates per second |
|---|---|---|
| Bulb | 1 | 20 |
| Strip or beam up to 82 zones | 1 | 20 |
| Tile chain of 5 | 5 | 4 |
| Candle, Ceiling, Tube | 1 | 20 |
| Ceiling 13x26 (128 zones) | 2 to 3 | 6 to 10 |
| Older multizone, N zones | one per run of equal colors | 20 for solid colors; a frame with N different zones takes N / 20 seconds |

The **Max FPS** value in a device's diagnostics panel shows the rate for the device's
cheapest frame. If you accept the risk of exceeding LIFX's recommendation, you can raise
`max_messages_per_second`. Setting a short `transition_ms` (close to the frame interval)
lets the light fade between frames, which smooths slow updates at the cost of that much
latency.

Because LIFX does not acknowledge streamed frames, Hypercolor also resends the current
frame after a second without changes, which repairs a dropped packet and repaints a light
that was power cycled or changed from the LIFX app.

## Colors

Effects render in RGB; Hypercolor converts every zone to LIFX's hue, saturation,
brightness, and kelvin. Saturated colors stay fully saturated, and pastels keep the share
of white light they have on screen. Whites and greys render at the `white_kelvin` setting
(6500 K by default, the white point of sRGB), limited to the range each light supports.

## Driver controls

| Field | Default | Effect |
|---|---|---|
| `device_ips` | `[]` | Addresses probed directly on every scan |
| `broadcast_discovery` | `true` | Find lights with a UDP broadcast; turn off to rely on `device_ips` only |
| `max_messages_per_second` | `20` | Per-light message budget (1 to 200) |
| `transition_ms` | `0` | Fade time the light applies to each frame (0 to 10000) |
| `white_kelvin` | `6500` | Kelvin for whites and greys (1500 to 9000) |
| `refresh_topology` (action) | | Reconnects one device and re-reads its zones or tiles |

Find these under **Devices → LIFX driver** in the web UI, or with:

```bash
hypercolor devices controls <device-id>
```

## Manual IP configuration

If broadcast does not reach your lights (separate VLAN, AP isolation, or a router that
drops broadcast), list their addresses:

```toml
# ~/.config/hypercolor/hypercolor.toml
[drivers.lifx]
enabled = true
device_ips = ["192.168.10.80", "192.168.10.81"]
```

Give the lights DHCP reservations so their addresses stay put. Hypercolor identifies each
light by its serial, so an address change alone does not create a duplicate device.

To stop Hypercolor from broadcasting at all, set `broadcast_discovery = false` and list
every light in `device_ips`. To turn the driver off, set `enabled = false`.

## Port reference

| Purpose | Protocol | Port |
|---|---|---|
| Discovery, control, and streaming | UDP | 56700 |

## Troubleshooting

**No LIFX lights found**

- Check that the lights respond in the LIFX app on the same network.
- Confirm the host and lights share a subnet, or add their addresses to `device_ips`.
- Lengthen the scan: `hypercolor devices discover --target lifx --timeout 15`.

**A light is found but shows the wrong layout**

Run **Refresh Topology** after physical changes. For Tile chains, arrange the tiles in the
LIFX app; Hypercolor reads the stored positions. The LIFX documentation does not say which
way the vertical tile coordinate points, so Hypercolor assumes it grows upward. If your
chain appears upside down, flip or rotate its zone in the layout editor, and please report
it so the default can be corrected.

**Colors update slowly on a Tile chain**

That is the message budget at work: a five-tile chain needs five messages per frame. See
[Frame rate and the message budget](#frame-rate-and-the-message-budget).

**Enable debug logging:**

```bash
RUST_LOG=hypercolor_driver_lifx=debug just daemon
```

See also: [Network devices overview](@/hardware/network-devices.md),
[Network discovery troubleshooting](@/troubleshooting/network-discovery.md).
