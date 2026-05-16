+++
title = "Your First Session"
description = "A hands-on walkthrough from daemon start to saved profile — everything you do once, in order, after installing Hypercolor"
weight = 3
template = "page.html"
+++

This tutorial assumes installation is complete and the `hypercolor` binary is in your `PATH`. If you haven't built Hypercolor yet, start with [Installation](@/guide/installation.md).

Each section builds on the last. By the end you'll have a running daemon, connected devices, an active effect, a spatial layout, a saved profile, and a scene — the point at which Hypercolor is genuinely set up.

## 1. Start the Daemon 🌊

The daemon is the process that drives everything: it talks to your hardware, runs the render loop, and serves the REST API on port 9420.

**Foreground mode** is useful during initial setup because it prints log output directly to your terminal:

```bash
just daemon
```

This runs with the `preview` build profile — optimized for runtime performance while keeping compile times fast. Press Ctrl-C to stop it. Once you're comfortable with the setup, the service commands below are the better everyday path.

**Background service** (the normal everyday path):

```bash
hypercolor service start
hypercolor service status
```

Confirm the daemon is responding:

```bash
curl http://localhost:9420/health
```

A `200 OK` response means the daemon is up. All other commands in this tutorial communicate with it over `localhost:9420`.

**Enable autostart on login** so the daemon starts automatically after every reboot or login:

```bash
hypercolor service enable
```

On Linux this calls `systemctl --user enable hypercolor`. On macOS it loads the LaunchAgent plist. See [Installation](@/guide/installation.md) for the full service setup including pre-built packages.

## 2. Verify the Daemon

Check the current system state:

```bash
hypercolor status
```

The output shows the render loop tier (which FPS target is active), the active effect (none yet), how many devices are connected, and whether audio capture is running. Everything should show healthy defaults before you add devices.

The same information is available via the API if you want to script against it:

```bash
curl http://localhost:9420/api/v1/status | jq .data
```

## 3. Discover and Connect Devices

USB HID devices (keyboards, mice, headsets, and strips connected via USB) are discovered automatically at daemon startup — no manual step required. Network devices like WLED controllers are discovered via mDNS on your local network.

List everything the daemon found:

```bash
hypercolor devices list
```

The table shows each device's name, driver, route (USB path or IP address), LED count, connection status, and firmware version.

If a device you expect is missing, trigger a manual scan:

```bash
hypercolor devices discover
```

For network devices on slower or segmented networks, extend the scan window:

```bash
hypercolor devices discover --timeout 15
```

If a USB device is listed but shows a permission error, you may need to install the udev rules:

```bash
just udev-install
```

Re-plug the device or log out and back in after installing the rules. For more detail, see [Debugging](@/contributing/debugging.md).

**Identify a specific device** by making it flash a test pattern. This is useful when you have multiple devices and want to confirm which physical unit has which ID:

```bash
hypercolor devices identify <device-name-or-id>
```

The device pulses for five seconds by default. Pass `--duration 10` for a longer window.

## 4. Browse the Effect Library

Hypercolor ships with 40+ built-in effects spanning audio-reactive visualizers, ambient gradients, particle systems, and procedural animations. Get the full list:

```bash
hypercolor effects list
```

A few highlights worth trying first:

- `borealis` — a slow aurora curtain, good for ambient setups
- `audio-pulse` — a VU meter that reacts to system audio
- `digital-rain` — falling green characters in the matrix style
- `nyan-dash` — nyan cat riding a rainbow (yes, really)
- `breathing` — a gentle sine-wave fade, popular for focus work
- `color-wave` — a smooth hue rotation across the whole canvas

You can also filter the list. Show only audio-reactive effects:

```bash
hypercolor effects list --audio
```

Search by name:

```bash
hypercolor effects list --search aurora
```

If the daemon is serving the embedded web UI, you can browse the catalog visually at `http://localhost:9420` — search, filter by category, and see a live canvas preview before applying anything to hardware.

## 5. Apply Your First Effect

Pick an effect from the list and apply it by name. Names are fuzzy-matched, so partial names work:

```bash
hypercolor effects activate borealis
```

The daemon begins rendering immediately. Verify it took:

```bash
hypercolor status
```

The "Active effect" field should now show `borealis`. Your connected devices will already be showing the new effect.

To stop the active effect and return to a dark state:

```bash
hypercolor effects stop
```

To apply an effect with specific parameters from the start, pass them inline:

```bash
hypercolor effects activate breathing --speed 30 --intensity 70
```

## 6. Adjust Brightness and Effect Controls

### Global brightness

Global brightness scales the final output of every device from 0 (off) to 100 (full):

```bash
hypercolor brightness set 80
hypercolor brightness get    # read back the current value
```

This is the coarse knob. Most effects also have their own intensity parameter that works independently.

### Effect control parameters

Effects expose typed parameters — speed, color palette, particle density, audio sensitivity, and more. These are patched in real time via the REST API:

```bash
curl -X PATCH http://localhost:9420/api/v1/effects/current/controls \
  -H "Content-Type: application/json" \
  -d '{"controls": {"speed": 3}}'
```

Changes apply on the next rendered frame. To see what parameters the active effect exposes:

```bash
curl http://localhost:9420/api/v1/effects/active | jq '.data.controls'
```

The web UI's controls panel lets you adjust these with sliders, which is easier for exploration. The REST PATCH is the right path for scripting or keyboard macros.

## 7. Build a Spatial Layout

Without a layout, every device zone gets the same averaged color from the canvas — all your LEDs pulse together as one blob. A layout tells Hypercolor where each zone sits in the virtual canvas, so gradients actually flow across your desk from left to right, and a vertical strip gets the top of the canvas while a keyboard gets the bottom.

Canvas coordinates are normalized to `[0.0, 1.0]`, so positions stay valid regardless of the configured canvas resolution.

**View existing layouts:**

```bash
hypercolor layouts list
```

**Check the active layout:**

```bash
hypercolor layouts active
```

**Via REST** (full detail):

```bash
curl http://localhost:9420/api/v1/layouts | jq .data.items
curl http://localhost:9420/api/v1/layouts/active | jq
```

**Create a layout** from the web UI or via REST with a JSON zone definition. The web UI is the recommended tool here: open `http://localhost:9420`, navigate to Layouts, and use the drag-and-drop zone editor to position your device zones on the canvas. Topology options include strip, matrix, and ring configurations. Once you're satisfied, save it from the UI.

From the CLI, you can apply an existing named layout:

```bash
hypercolor layouts apply "My Layout"
```

And delete one you no longer need:

```bash
hypercolor layouts delete "Old Layout"
```

## 8. Save a Profile

Once you have an effect and brightness level you like, save the whole state as a named profile:

```bash
hypercolor profiles create "Gaming"
```

This captures the active effect, all control values, brightness, device assignments, and spatial layout. Switching profiles is near-instant.

**Switch between profiles:**

```bash
hypercolor profiles list
hypercolor profiles apply "Gaming"
```

**Delete a profile** when you no longer need it (requires `--yes` to confirm):

```bash
hypercolor profiles delete "Gaming" --yes
```

The profile system is also available via the REST API — see [Configuration](@/guide/configuration.md) for the full schema reference.

## 9. Create a Scene

Scenes add a layer of automation on top of profiles. A scene is a named lighting state that can be activated manually or triggered automatically based on time of day, audio activity, or other conditions.

**Create a scene for focused work:**

```bash
hypercolor scenes create "Focus Mode"
```

The scene name is a positional argument. You can also add a description:

```bash
hypercolor scenes create "Late Night" --description "Dim amber for late sessions"
```

**Activate a scene manually:**

```bash
hypercolor scenes activate "Focus Mode"
```

**Return to the default lighting state:**

```bash
hypercolor scenes deactivate
```

**List configured scenes:**

```bash
hypercolor scenes list
```

Automated triggers — time-based switching, audio-reactive activation, and transition curves — are configured via the REST API or the web UI. See [Configuration](@/guide/configuration.md) for the full scene JSON schema including the `transition` and `controls` fields.

## 10. Tour the Web UI

Open `http://localhost:9420` in a browser. The daemon serves the embedded Leptos web UI directly — no separate server process required.

The UI is organized into panels:

- **Effects browser** — search and filter the full effect catalog, preview effects in the canvas panel before committing to hardware
- **Controls panel** — per-parameter sliders for the active effect; equivalent to the REST PATCH endpoint but much easier for exploration
- **Devices panel** — connection status for every discovered device, per-device brightness, and an identify button to flash a test pattern
- **Layouts editor** — drag-and-drop zone positioning with topology selection (strip, matrix, ring); the canonical way to build spatial layouts
- **Profiles** — save, apply, and delete named configurations
- **Scenes** — create and manage scenes, configure triggers and transitions

The canvas preview in the top right shows exactly what the render loop is producing, updated at the daemon's current target FPS.

## 11. Tour the Terminal UI ⚡

Hypercolor also ships an interactive terminal dashboard built with Ratatui. Launch it:

```bash
hypercolor tui
```

The TUI connects to the running daemon via WebSocket and shows:

- A live canvas preview rendered with the terminal's best graphics protocol — Kitty, Sixel, or iTerm2 — with a text-block fallback for terminals without graphics support
- A spectrum analyzer panel showing the live audio FFT, updated in real time
- A device list and status sidebar
- An effect selector with keyboard navigation — navigate with arrow keys, activate with Enter
- All changes sync to the daemon immediately, so the web UI and any REST clients see the same state

Exit with `q` or Ctrl-C.

## 12. What's Next

You now have a fully configured Hypercolor setup. Where to go from here:

- [Configuration](@/guide/configuration.md) — full config reference: audio device selection, daemon network settings, the profile and scene JSON schemas, and config hot-reload behavior
- [Creating Effects](@/effects/creating-effects.md) — write custom effects with the TypeScript SDK and see them appear in the effect browser immediately
- [Hardware: Compatibility Matrix](@/hardware/compatibility.md) — check driver status for every supported device family
- [API Reference](@/api/rest.md) — full REST and WebSocket API documentation for scripting and third-party integration