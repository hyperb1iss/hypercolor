+++
title = "Quick start"
description = "Zero to RGB in five minutes, including a no-hardware simulator path and a conflicting-software callout."
weight = 70
+++

This guide assumes you have completed [installation](@/guide/installation.md) and understand the [pieces](@/guide/the-pieces.md) (daemon, CLI, web UI). If you just ran the installer, the daemon is probably already running. If not, start it now:

```bash
just daemon
# or, for a packaged install:
hypercolor service start
```

Then verify it is up:

```bash
curl -s http://localhost:9420/health
```

A `200 OK` response means the daemon is running and the REST API is reachable.

{% callout(type="warning") %}
**Close other RGB software first.** OpenRGB, Aura Sync, openrazer daemon, iCUE wine layers, and similar tools all grab USB HID devices exclusively. If one of them is running, Hypercolor cannot claim the same device. Stop those tools before continuing. If a USB device appears in `lsusb` but not in `hypercolor devices list`, a conflicting tool is the most likely cause. See [conflicting software](@/hardware/conflicting-software.md) for diagnosis steps.
{% end %}

## 1. Check device discovery

The daemon discovers USB HID devices automatically on startup and rescans when you plug in hardware. Network devices (WLED via mDNS, Hue, Nanoleaf, Govee) are discovered on the same background loop.

```bash
hypercolor devices list
```

You should see a table of connected devices with their name, driver, output route, LED count, status, and firmware. If the list is empty and you have hardware plugged in, check the [udev rules](@/guide/installation.md) and confirm no conflicting software is running.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

{% callout(type="info") %}
**No hardware? Use a simulated display.** The daemon has a built-in virtual display simulator — no physical LEDs required. Create one via the REST API and it will appear in `devices list` just like real hardware:

```bash
curl -s -X POST http://localhost:9420/api/v1/simulators/displays \
  -H "Content-Type: application/json" \
  -d '{"name": "My Simulator", "width": 32, "height": 8}'
```

You can then apply effects to it and preview frames at `GET /api/v1/simulators/displays/{id}/frame` (returns JPEG). Delete it when you are done with `DELETE /api/v1/simulators/displays/{id}`. The web UI canvas preview at `http://localhost:9420` works without any physical devices at all.
{% end %}

## 2. Browse effects

The library ships with a large collection of built-in effects spanning audio-reactive visualizers, ambient gradients, particle systems, and more. Browse the full gallery in the [effect catalog](@/effects/_index.md).

```bash
# List all effects
hypercolor effects list

# Filter to audio-reactive effects only
hypercolor effects list --audio

# Search by name or description
hypercolor effects list --search borealis

# Filter by category
hypercolor effects list --category ambient
```

The web UI at `http://localhost:9420` has a visual effect browser with search, category filters, and canvas preview before you apply anything.

![Web UI dashboard showing the effect browser](/img/ui/dashboard.webp)

## 3. Apply an effect

Effect names are fuzzy-matched — you do not need an exact ID.

```bash
# Activate by name (fuzzy match)
hypercolor effects activate borealis

# With shorthand controls
hypercolor effects activate audio-pulse --speed 60 --intensity 80

# With arbitrary control parameters
hypercolor effects activate color-wave --param palette=SilkCircuit --param speed=40

# Effect switches are immediate today
hypercolor effects activate nebula-drift
```

Via REST:

```bash
curl -X POST http://localhost:9420/api/v1/effects/borealis/apply \
  -H "Content-Type: application/json" \
  -d '{"controls": {"speed": 50, "intensity": 75}}'
```

## 4. Tweak controls in real time

Most effects expose configurable parameters — speed, color palette, intensity, audio sensitivity. Changes apply immediately without re-applying the effect:

```bash
# Patch one or more controls on the running effect
hypercolor effects patch --param speed=30 --param palette=Midnight

# Reset all controls to their defaults
hypercolor effects reset
```

Via REST:

```bash
curl -X PATCH http://localhost:9420/api/v1/effects/current/controls \
  -H "Content-Type: application/json" \
  -d '{"controls": {"speed": 30, "palette": "Midnight"}}'
```

The render loop picks up new values on the next frame. It targets up to 60 fps and adapts down across five tiers under load, so a change lands in roughly 17 to 100 ms depending on the active tier.

## 5. Try the TUI

The terminal UI gives you a full interactive control surface in one pane — device list, effect browser, live canvas preview, and spectrum meter — all over the same WebSocket connection the web UI uses.

```bash
hypercolor tui
```

![TUI dashboard view](/img/tui/tui-dashboard.png)

See [the TUI guide](@/guide/tui.md) for keyboard shortcuts and layout details.

## 6. Stop the effect

```bash
hypercolor effects stop
```

Or via REST:

```bash
curl -X POST http://localhost:9420/api/v1/effects/stop
```

## What's next

- [Your first 10 minutes](@/guide/your-first-10-minutes.md) — an opinionated golden path through the web UI
- [First session](@/guide/first-session.md) — a longer hands-on walkthrough: devices, layouts, profiles, and scenes
- [Audio setup](@/guide/audio-setup.md) — configure PipeWire/PulseAudio so reactive effects respond to your music
- [Profiles and scenes](@/guide/profiles-and-scenes.md) — save your lighting state and switch between setups
- [Effect catalog](@/effects/_index.md) — browse every built-in effect with parameters and previews
- [Configuration](@/guide/configuration.md) — tune FPS, canvas size, network access, and more
