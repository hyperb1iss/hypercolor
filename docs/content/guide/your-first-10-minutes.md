+++
title = "Your first 10 minutes"
description = "Open the app, see your devices, apply an effect, dial in brightness, and save a profile — from zero to lit in under ten minutes."
weight = 80
+++

This page is the opinionated golden path. It skips the deep-dive and gets your lights on fast. If something doesn't behave as expected, [Finding devices](@/guide/finding-devices.md) and [Common issues](@/troubleshooting/common-issues.md) have the details.

No hardware yet? [Jump to the simulator path](#no-hardware-use-the-simulator) and come back to the device steps when you're ready.

---

## Before you start

Hypercolor needs its daemon running before any client (app, TUI, CLI, web UI) can talk to it. The desktop app starts the daemon automatically. If you installed via the CLI only, start it manually:

```bash
hypercolor service start
```

Verify it's up:

```bash
hypercolor status
```

You should see daemon version, uptime, and a device count. If you get a connection error, check that port 9420 is not in use by another process.

{% callout(type="warning") %}
If you have OpenRGB, Aura Sync, or another RGB manager running, it may be holding your USB devices. Stop those tools first — two apps cannot share the same HID device.
{% end %}

---

## Step 1 — See your devices

### In the web UI or desktop app

Open a browser to `http://localhost:9420`, or use the desktop app. The Devices panel lists every device Hypercolor has discovered. Each entry shows the device name, driver, LED count, and connection status.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

If the list is empty, run a scan:

```bash
hypercolor devices discover
```

For network devices (WLED, Hue, Nanoleaf, Govee) add a timeout:

```bash
hypercolor devices discover --target wled --timeout 15
```

To flash a test pattern on a specific device so you can physically identify it:

```bash
hypercolor devices identify "Razer Huntsman"
```

The device name is fuzzy-matched, so close is usually close enough.

### In the CLI

```bash
hypercolor devices list
```

This prints a table with columns for Device, Driver, Route, LEDs, Status, and Firmware. If a device you expect is missing, see [Devices not found](@/troubleshooting/devices-not-found.md).

---

## Step 2 — Apply an effect

### Browse available effects

```bash
hypercolor effects list
```

To filter by category or search by name:

```bash
hypercolor effects list --category ambient
hypercolor effects list --search borealis
```

To see only audio-reactive effects:

```bash
hypercolor effects list --audio
```

The [Effects](@/effects/_index.md) section covers every authoring path and the built-in library.

### Activate one

```bash
hypercolor effects activate borealis
```

Effect names are fuzzy-matched. The daemon applies the effect across all connected devices immediately.

![Effects panel showing Borealis running across connected devices](/img/ui/effects.webp)

You can pass initial controls at activation time:

```bash
hypercolor effects activate borealis --speed 60 --intensity 80
```

To tweak controls on a running effect without re-applying it:

```bash
hypercolor effects patch --param speed=40 --param intensity=90
```

To reset controls to their defaults:

```bash
hypercolor effects reset
```

To stop the effect entirely:

```bash
hypercolor effects stop
```

{% callout(type="tip") %}
Effect switches are immediate today. The `--transition` flag is reserved for
future crossfades, and nonzero values are rejected until that renderer path
lands.
{% end %}

---

## Step 3 — Set brightness

Global brightness scales all device output uniformly, independent of the effect's own color values.

```bash
# Check current level
hypercolor brightness get

# Set to 70%
hypercolor brightness set 70
```

The value is clamped to 0–100. 0 turns all LEDs off while keeping the effect running.

In the desktop app, the tray menu's Brightness submenu lets you adjust this without opening the full window.

---

## Step 4 — Save a profile

A profile captures the current state — active effect, controls, brightness, scene configuration — so you can restore it later or switch between setups.

```bash
hypercolor profiles create "Gaming"
```

To list your saved profiles:

```bash
hypercolor profiles list
```

To restore a profile:

```bash
hypercolor profiles apply "Gaming"
```

Profiles are fuzzy-matched by name. Add a description to keep them organized:

```bash
hypercolor profiles create "Work" --description "Cool whites, low brightness"
```

By default the daemon restores your last session on startup (`start_profile = "last"` in your config), so the effect, controls, and brightness you left running come back automatically after a reboot.

See [Profiles and scenes](@/guide/profiles-and-scenes.md) for the full picture on scenes and automated triggers.

---

## No hardware? Use the simulator

The simulator is the documented no-hardware path. It creates a virtual display device that the render engine treats exactly like a real one — effects, controls, and the canvas preview all work.

Create a simulated display via the REST API:

```bash
curl -X POST http://localhost:9420/api/v1/simulators/displays \
  -H "Content-Type: application/json" \
  -d '{"name": "My Simulator", "width": 160, "height": 32}'
```

The simulator appears in `hypercolor devices list` alongside physical hardware. Apply effects to it, adjust brightness, and save profiles exactly as you would with real devices.

To preview the output as a live JPEG frame:

```bash
curl http://localhost:9420/api/v1/simulators/displays/<id>/frame --output frame.jpg
```

Replace `<id>` with the `id` field returned when you created the simulator.

To remove the simulator:

```bash
curl -X DELETE http://localhost:9420/api/v1/simulators/displays/<id>
```

Simulated display configs persist across daemon restarts. Width and height must each be between 1 and 4096.

---

## Success checkpoint

At the end of this walkthrough you should have:

- At least one device (real or simulated) visible in `hypercolor devices list`
- An effect running — confirmed by seeing the name in `hypercolor status` or the web UI
- Brightness at a level you set deliberately
- A saved profile you can restore with `hypercolor profiles apply`

If you hit a wall, [Common issues](@/troubleshooting/common-issues.md) covers port conflicts, missing devices, and audio setup. The [TUI](@/guide/tui.md) gives you a live dashboard view of everything in the terminal.

---

## Where to go next

- [Finding devices](@/guide/finding-devices.md) — USB permissions, network discovery, pairing network devices
- [Audio setup](@/guide/audio-setup.md) — configuring PipeWire/PulseAudio for audio-reactive effects
- [Profiles and scenes](@/guide/profiles-and-scenes.md) — scheduling, triggers, and multi-scene rigs
- [Studio](@/studio/overview.md) — the full zone editor for per-LED spatial control
- [Effects](@/effects/_index.md) — the built-in library and every effect authoring path
