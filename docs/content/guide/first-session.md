+++
title = "Your first session"
description = "Hands-on walkthrough from daemon start to a saved, audio-reactive setup — devices, effects, layouts, profiles, scenes, and Studio."
weight = 90
template = "page.html"
+++

This tutorial assumes installation is complete and the `hypercolor` binary is in your `PATH`. If you haven't built Hypercolor yet, start with [Installation](@/guide/installation.md). Each section builds on the last. By the end you'll have a running daemon, connected devices, an active effect, a spatial layout, a saved profile, and a scene. That's the point at which Hypercolor is genuinely set up.

## 1. Start the daemon 🌊

The daemon is the process that drives everything: it talks to your hardware, runs the render loop, and serves the REST API on port 9420.

**Foreground mode** is useful during initial setup because it prints log output directly to your terminal:

```bash
just daemon
```

This runs with the `preview` build profile, optimized for runtime performance while keeping compile times fast. Press Ctrl-C to stop it. Once you're comfortable with the setup, the service commands below are the better everyday path.

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

## 2. Verify the daemon

Check the current system state:

```bash
hypercolor status
```

The output shows the render loop tier (which FPS target is active), the active effect (none yet), how many devices are connected, device and effect inventory counts, and frame budget telemetry. Everything should show healthy defaults before you add devices. Pass `--watch` to get a live-updating view:

```bash
hypercolor status --watch
```

The same information is available via the API if you want to script against it:

```bash
curl http://localhost:9420/api/v1/status | jq .data
```

## 3. Discover and connect devices

USB HID devices (keyboards, mice, headsets, and strips connected via USB) are discovered automatically at daemon startup, with no manual step required. Network devices like WLED controllers are discovered via mDNS on your local network.

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

You can also target a specific protocol family:

```bash
hypercolor devices discover --target wled
hypercolor devices discover --target hue
hypercolor devices discover --target usb
```

If a USB device is listed but shows a permission error, you may need to install the udev rules:

```bash
just udev-install
```

Re-plug the device or log out and back in after installing the rules. For more detail, see [Debugging](@/contributing/debugging.md).

{% callout(type="warning") %}
If a USB device appears in `lsusb` but not in `hypercolor devices list`, check whether another RGB manager such as OpenRGB, Aura Sync, or the openrazer daemon is running. These tools grab the HID device file and Hypercolor cannot take over until they release it.
{% end %}

**Identify a specific device** by making it flash a test pattern. This is useful when you have multiple devices and want to confirm which physical unit has which ID:

```bash
hypercolor devices identify <device-name-or-id>
```

The device pulses for five seconds by default. Pass `--duration 10` for a longer window.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

## 4. Browse the effect library

Hypercolor ships with a large catalog of built-in effects spanning audio-reactive visualizers, ambient gradients, particle systems, and procedural animations. The exact count grows with every release, so browse the catalog for the full picture. See the [Effects section](@/effects/_index.md).

Get the full list via the CLI:

```bash
hypercolor effects list
```

A few highlights worth trying first:

- `borealis` — a slow aurora curtain, good for ambient setups
- `audio-pulse` — a VU meter that reacts to system audio
- `digital-rain` — falling green characters in the matrix style
- `nyan-dash` — nyan cat riding a rainbow
- `breathing` — a gentle sine-wave fade, popular for focus work
- `color-wave` — a smooth hue rotation across the whole canvas

Filter the list to audio-reactive effects only:

```bash
hypercolor effects list --audio
```

Search by name or description:

```bash
hypercolor effects list --search aurora
```

Filter by category:

```bash
hypercolor effects list --category ambient
```

If the daemon is serving the embedded web UI, you can browse the catalog visually at `http://localhost:9420`. Search, filter by category, and see a live canvas preview before applying anything to hardware.

## 5. Apply your first effect

Pick an effect from the list and apply it by name. Names are fuzzy-matched, so partial names work:

```bash
hypercolor effects activate borealis
```

The daemon begins rendering immediately. Verify it took:

```bash
hypercolor status
```

The "Effect" field should now show `borealis`. Your connected devices will already be showing the new effect.

To stop the active effect and return to a dark state:

```bash
hypercolor effects stop
```

**Apply an effect with initial parameters.** Use `--speed` and `--intensity` as shorthand controls, and `--param key=value` (repeatable) for any other named parameter:

```bash
hypercolor effects activate breathing --speed 30 --intensity 70
hypercolor effects activate audio-pulse --param sensitivity=80
```

**Show what parameters an effect exposes:**

```bash
hypercolor effects info borealis
```

## 6. Adjust brightness and effect controls

### Global brightness

Global brightness scales the final output of every device from 0 (off) to 100 (full):

```bash
hypercolor brightness set 80
hypercolor brightness get    # read back the current value
```

This is the coarse knob. Most effects also have their own intensity parameter that works independently.

### Patching controls on a running effect

Use `effects patch` to change one or more parameters on the currently running effect without re-applying it. The `--param` flag is repeatable:

```bash
hypercolor effects patch --param speed=60
hypercolor effects patch --param speed=40 --param intensity=90
```

Reset all controls to the effect's defaults:

```bash
hypercolor effects reset
```

### Via REST

Changes apply on the next rendered frame:

```bash
curl -X PATCH http://localhost:9420/api/v1/effects/current/controls \
  -H "Content-Type: application/json" \
  -d '{"controls": {"speed": 3}}'
```

To see what parameters the active effect exposes:

```bash
curl http://localhost:9420/api/v1/effects/active | jq '.data.controls'
```

The web UI's controls panel lets you adjust these with sliders, which is easier for exploration. The REST PATCH and `effects patch` are the right paths for scripting or keyboard macros.

![Effect control panel in the Hypercolor Studio](/img/ui/ui-effect-controls.webp)

## 7. Build a spatial layout

Without a layout, every device zone gets the same averaged color from the canvas, so all your LEDs pulse together as one blob. A layout tells Hypercolor where each zone sits in the virtual canvas, so gradients actually flow across your desk from left to right, and a vertical strip gets the top of the canvas while a keyboard gets the bottom.

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

From the CLI, you can create a layout by providing a JSON definition:

```bash
hypercolor layouts create --name "My Desk" --data '{"zones": [...]}'
```

Apply an existing named layout:

```bash
hypercolor layouts apply "My Desk"
```

Preview a layout without making it active:

```bash
hypercolor layouts preview "My Desk"
```

Delete a layout you no longer need:

```bash
hypercolor layouts delete "Old Layout"
```

{% callout(type="tip") %}
The Studio workspace offers a full visual zone editor for spatial layouts. Open `http://localhost:9420`, click the Studio tab, and use the zone canvas to drag device zones into position. See [Studio overview](@/studio/overview.md) and [Layouts](@/studio/layouts.md) for the full walkthrough.
{% end %}

## 8. Set up audio-reactive effects

Audio-reactive effects like `audio-pulse`, `cymatics`, and `frequency-cascade` analyze your system audio in real time and drive the render loop from it. The daemon captures audio via a PipeWire/PulseAudio monitor source — the loopback of your speakers, not a microphone.

Apply an audio-reactive effect and watch the lights respond to whatever is playing:

```bash
hypercolor effects activate audio-pulse
```

If the effect doesn't react, the most common cause is that the audio capture device is wrong. First confirm the daemon sees an audio source at all — `status` exposes an `audio_available` flag:

```bash
curl http://localhost:9420/api/v1/status | jq '.data.audio_available'
```

Then list the capture devices the daemon can see and check which one it picked up:

```bash
hypercolor audio devices
```

You can also inspect the live spectrum in the terminal UI (see the [terminal UI guide](@/guide/tui.md)). The spectrum panel updates in real time and is the fastest way to confirm audio capture is working.

For a full guide to configuring audio sources, see [Audio setup](@/guide/audio-setup.md).

## 9. Save a profile

Once you have an effect and brightness level you like, save the whole state as a named profile:

```bash
hypercolor profiles create "Gaming"
```

This captures the active effect, all control values, brightness, device assignments, and spatial layout. Switching profiles is near-instant.

Add a description when you want to annotate what the profile is for:

```bash
hypercolor profiles create "Focus" --description "Dim breathing for deep work"
```

**Switch between profiles:**

```bash
hypercolor profiles list
hypercolor profiles apply "Gaming"
```

Profile switching is immediate. The `--transition` flag is reserved for profile
crossfades, but only `0` is accepted today.

**Delete a profile** when you no longer need it (requires `--yes` to confirm):

```bash
hypercolor profiles delete "Gaming" --yes
```

**Inspect a profile's contents:**

```bash
hypercolor profiles info "Gaming"
```

The daemon restores the last active profile on startup by default. See [Configuration](@/guide/configuration.md) for how to change the `start_profile` behavior.

## 10. Create a scene

Scenes are whole-rig lighting configurations that can be activated manually or switched programmatically. A scene holds the full lighting state (active effect, control values, layout, and zones) and can be recalled at any time by name.

**Create a scene:**

```bash
hypercolor scenes create "Focus Mode"
```

Add an optional description and set the mutation mode:

```bash
hypercolor scenes create "Late Night" --description "Dim amber for late sessions"
```

**Activate a scene manually:**

```bash
hypercolor scenes activate "Focus Mode"
```

Scene activation uses the stored scene transition. The CLI `--transition` flag
is accepted for forward compatibility, but the activate endpoint does not
override the stored value yet.

**Return to the Default scene:**

```bash
hypercolor scenes deactivate
```

**List configured scenes:**

```bash
hypercolor scenes list
```

**Check the currently active scene:**

```bash
hypercolor scenes active
```

**Delete a scene** (requires `--yes`):

```bash
hypercolor scenes delete "Old Scene" --yes
```

For the conceptual difference between profiles and scenes, and how to configure scene groups, zone overrides, and priorities, see [Profiles and scenes](@/guide/profiles-and-scenes.md).

{% callout(type="tip") %}
Studio is the visual authoring surface for scenes. The Studio workspace lets you build multi-zone scenes with per-zone effects, inspect the composition in a live canvas preview, and manage scene groups without writing JSON. See [Studio scenes](@/studio/scenes.md) and [Studio zones](@/studio/zones.md).
{% end %}

![The scene switcher in the Hypercolor web UI](/img/ui/ui-scenes.webp)

## 11. Tour the web UI

Open `http://localhost:9420` in a browser. The daemon serves the embedded Leptos web UI directly, with no separate server process required.

![Hypercolor dashboard — effects browser, canvas preview, device status panel](/img/ui/dashboard.webp)

The UI is organized into panels:

- **Effects browser** — search and filter the full effect catalog, preview effects in the canvas panel before committing to hardware
- **Controls panel** — per-parameter sliders for the active effect; equivalent to `effects patch` but much easier for exploration
- **Devices panel** — connection status for every discovered device, per-device brightness, and an identify button to flash a test pattern
- **Layouts editor** — drag-and-drop zone positioning with topology selection (strip, matrix, ring); the canonical way to build spatial layouts
- **Profiles** — save, apply, and delete named configurations
- **Scenes** — create and manage scenes
- **Studio** — the full multi-zone authoring workspace (see below)

The canvas preview shows exactly what the render loop is producing, updated at the daemon's current target FPS.

## 12. Try Studio

Studio is the advanced authoring workspace inside the web UI. It goes beyond applying a single effect to all devices: in Studio you build per-zone effects that compose into a full rig, with a live canvas preview showing you exactly what the render will produce before it hits hardware.

Open Studio from the top navigation, or navigate directly to `http://localhost:9420` and click the Studio tab.

![Hypercolor Studio — zone canvas with live preview and effect layers](/img/ui/studio.webp)

What Studio adds on top of the basic effects browser:

- **Zone canvas** — position and resize device zones visually; zones are flexible canvas partitions, not rooms
- **Per-zone effect layers** — assign a different effect to each zone, with independent control values and priority ordering
- **Layer compositing** — stack effects with blend modes to create layered lighting compositions
- **Scene authoring** — save the full multi-zone composition as a scene that can be recalled with one click
- **Display faces** — when an effect supports it, you can target specific display faces (front, top, rear) of a device; see [Display faces](@/effects/display-faces.md) for how effects declare and use face geometry

For the full Studio walkthrough, see [Studio overview](@/studio/overview.md) and the [workspace tour](@/studio/workspace-tour.md).

## 13. Tour the terminal UI ⚡

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

## 14. What's next

You now have a fully configured Hypercolor setup. Where to go from here:

- [Profiles and scenes](@/guide/profiles-and-scenes.md) — conceptual guide and full CLI/API reference for saved configurations and scene management
- [Audio setup](@/guide/audio-setup.md) — configure PipeWire/PulseAudio monitor sources for audio-reactive effects
- [Studio overview](@/studio/overview.md) — multi-zone authoring, scene groups, the visual zone canvas, and the Studio workspace tour
- [Creating effects](@/effects/creating-effects.md) — write custom effects with the TypeScript SDK and see them appear in the effect browser immediately
- [Display faces](@/effects/display-faces.md) — how effects target specific geometry faces on multi-face devices
- [Hardware: Compatibility matrix](@/hardware/compatibility.md) — check driver status for every supported device family
- [Configuration](@/guide/configuration.md) — full config reference: audio device selection, daemon network settings, the profile and scene JSON schemas, and config hot-reload behavior
- [API reference](@/api/rest.md) — full REST and WebSocket API documentation for scripting and third-party integration
