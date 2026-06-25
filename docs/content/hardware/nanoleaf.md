+++
title = "Nanoleaf"
description = "Set up Nanoleaf panels with Hypercolor: mDNS discovery, power-button token pairing, and UDP External Control streaming."
weight = 60
+++

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

Hypercolor discovers Nanoleaf controllers over mDNS, pairs via the Open API token flow
(hold the power button 5–7 seconds), and streams per-panel color data over UDP External
Control. Every addressable panel becomes an individually controllable zone that maps into
the spatial canvas.

## Prerequisites

- The Nanoleaf controller and the machine running Hypercolor must be on the **same local
  network** (same subnet, mDNS must not be blocked). If they are on separate VLANs or
  mDNS is unreliable on your network, see [Manual IP configuration](#manual-ip-configuration).
- No other application (the Nanoleaf desktop app, a Home Assistant integration, etc.) should
  hold an active External Control session at the same time. Only one streaming client can
  drive a Nanoleaf controller at once.

## How discovery works

Hypercolor runs two complementary discovery paths simultaneously:

1. **mDNS**: listens for `_nanoleafapi._tcp.local.` service records. The controller
   advertises its IP, port, model, and firmware over mDNS TXT records (`name`/`nm`,
   `model`/`md`, `firmware`/`fw`). Results from both paths are merged automatically.
2. **Known-IP probe**: any IP address you add manually is probed directly at the API port
   (default `16021`), regardless of mDNS availability.

During a scan, Hypercolor fetches device info and the panel layout from each reachable
candidate (`GET /api/v1/<token>` and `GET /api/v1/<token>/panelLayout/layout`). If stored
credentials are found the device is set to auto-connect; otherwise it appears as unpaired
and awaits the pairing flow below.

Trigger a scan from the CLI:

```bash
hypercolor devices discover --target nanoleaf
```

Or use the **Discover** button on the Devices page in the web UI.

## Pairing ⚡

Nanoleaf uses a physical-confirmation token flow. The controller must be in pairing mode
before Hypercolor can request an auth token.

**Steps:**

1. Hold the **power button** on your Nanoleaf controller for **5–7 seconds** until the
   panels flash, confirming the controller has entered pairing mode.
2. Within the pairing window, run:

   ```bash
   hypercolor devices pair <device-id>
   ```

   Or click **Pair Device** next to the unpaired device in the web UI.

3. Hypercolor posts `POST http://<ip>:16021/api/v1/new`. If the controller is in pairing
   mode, it responds with an `auth_token`. That token is stored in the credential store
   under both the device serial number key and an `ip:<address>` fallback entry.
4. Confirm the device is now connected:

   ```bash
   hypercolor devices list
   ```

   The device should show state `Connected` and its panel count.

{% callout(type="warning") %}
The pairing window closes when the controller exits pairing mode, usually within a few
seconds of inactivity. Start the Hypercolor pair command before the window closes. If the
command returns `ActionRequired`, the controller was not in pairing mode. Hold the power
button again and retry immediately.
{% end %}

## Port reference

| Purpose | Protocol | Port |
|---|---|---|
| Open API (REST) | HTTP | 16021 |
| External Control (streaming) | UDP | 60222 |

Both ports are set by the Nanoleaf firmware and cannot be changed from Hypercolor.

## Streaming

Once paired, Hypercolor enables External Control by calling:

```
PUT http://<ip>:16021/api/v1/<token>/effects
{ "write": { "command": "display", "animType": "extControl", "extControlVersion": "v2" } }
```

After that, it opens a UDP socket bound to `0.0.0.0:<ephemeral>` and connects it to
`<device-ip>:60222`. Frames are sent once per render tick.

Each UDP frame is a compact binary packet:

```
[panel_count: 2 bytes big-endian]
  for each panel:
    [panel_id: 2 bytes BE]  [R]  [G]  [B]  [W=0]  [transition_time: 2 bytes BE]
```

Panels with no user-visible LEDs (controllers, rhythm modules, power connectors) are
excluded from the panel ID list and do not appear in frames.

The `transition_time` field defaults to the driver-level **Transition Time** setting
(configurable in the driver controls, default `1`). Setting it to `0` gives immediate
color changes; higher values trigger hardware-side crossfades between frames.

## Panel topology

On first connect and after a topology refresh, Hypercolor fetches the full panel layout
and builds a zone for every addressable panel:

- **Shape type** determines how a panel is categorized. Light strips
  (`LightLines`, `LightLinesSingleZone`, `FourDLightstrip`) get `Strip` topology; all
  other shapes (triangles, squares, hexagons, skylights) get `Point` topology.
- **Position data** (`x`, `y`, orientation `o`) is stored alongside the panel entry.
  Hypercolor uses it for spatial canvas mapping when a layout is configured.
- Non-addressable panels (`Rhythm`, `ShapesController`, `LinesConnector`, `ControllerCap`,
  `PowerConnector`) are filtered out and do not become zones.

Each addressable panel becomes one zone named `Panel <id>` with a single LED, addressable
individually by effects.

Supported shape types:

| Shape | Type ID | Topology |
|---|---|---|
| Triangle Light Panels | 0 | Point |
| Canvas (Square) | 2 | Point |
| Hexagon Shapes | 7 | Point |
| Triangle Shapes | 8 | Point |
| Mini Triangle | 9 | Point |
| Elements Hexagon | 14 | Point |
| Elements Hexagon Corner | 15 | Point |
| Light Lines | 17 | Strip |
| Light Lines Single Zone | 18 | Strip |
| 4D Lightstrip | 29 | Strip |
| Skylight Panel | 30 | Point |

## Driver controls

The Nanoleaf driver exposes two configurable fields and one device-level action:

| Field | Scope | Effect |
|---|---|---|
| `device_ips` | Driver | List of static IPs to probe; triggers a discovery rescan when changed |
| `transition_time` | Driver | Hardware crossfade time per frame (default `1`; `0` = immediate) |
| `refresh_topology` (action) | Device | Reconnects the device and reloads the panel layout |

Access these in the web UI under **Devices → Nanoleaf driver → Connection / Output**, or
via the CLI:

```bash
hypercolor devices controls <device-id>
```

## Refreshing the topology

If you physically add or remove panels, the zone list will be stale until you refresh.

```bash
hypercolor devices action <device-id> refresh_topology --yes
```

Or click **Refresh Topology** in the device diagnostics panel in the web UI.

{% callout(type="info") %}
Refresh Topology triggers a brief reconnect. The device will stop receiving color data for
a moment while the new layout is fetched.
{% end %}

## Manual IP configuration

If mDNS is unavailable (separate VLAN, `systemd-resolved` stub conflicts, Docker bridge
network), add the device IP directly to the Nanoleaf driver config:

```toml
# ~/.config/hypercolor/config.toml
[drivers.nanoleaf]
enabled = true
device_ips = ["192.168.10.42"]
transition_time = 1
```

Or set it through the driver controls in the web UI (**Devices → Nanoleaf driver →
Connection → Device IPs**). Changing `device_ips` triggers a discovery rescan automatically.

## Clearing credentials

To unpair a device and remove its stored token:

```bash
curl -X DELETE http://localhost:9420/api/v1/devices/<device-id>/pair
```

Or use the **Clear Credentials** action in the device panel in the web UI. Both the
`device_key` and `ip:<address>` credential entries are removed and the device disconnects.
You will need to pair again before Hypercolor can stream to it.

## Troubleshooting

**Device not found after discovery scan**

- Confirm the controller is powered and reachable on the same subnet. Try pinging its IP
  from the Hypercolor host.
- If mDNS is unreliable on your network, add the IP manually under
  [Manual IP configuration](#manual-ip-configuration).
- Extend the discovery timeout: `hypercolor devices discover --target nanoleaf --timeout 10`.

**Pairing returns `ActionRequired`**

The controller was not in pairing mode when Hypercolor posted `POST /api/v1/new`. Hold
the power button for the full 5–7 seconds until the panels flash, then retry the pair
command immediately.

**Device connects but panels show wrong colors or no output**

Verify no other application holds an External Control session. The Nanoleaf desktop app,
the Home Assistant Nanoleaf integration, and similar tools all compete for the same UDP
stream. Only one streaming client can be active at a time.

**Enable debug logging for detailed diagnosis:**

```bash
RUST_LOG=hypercolor_driver_nanoleaf=debug hypercolor daemon
```

See also: [Network discovery troubleshooting](@/troubleshooting/network-discovery.md),
[Devices not found](@/troubleshooting/devices-not-found.md),
[Network devices overview](@/hardware/network-devices.md).
