+++
title = "Guide"
description = "Get set up, get lit. Pick your install path and your interface, then go from zero to RGB."
sort_by = "weight"
weight = 10
template = "section.html"
+++

Hypercolor is an open-source RGB lighting orchestration engine for Linux, written in Rust. One daemon drives every RGB device you own — Razer peripherals, WLED strips, ASUS motherboards, Corsair gear, Philips Hue, Nanoleaf panels, Govee lights, and more — all running from the same render engine at up to 60 fps.

Effects are HTML Canvas pages rendered by an embedded Servo browser. Audio FFT, screen capture, and keyboard input feed the render every frame. A spatial sampler maps canvas pixels onto your physical LED positions. The result: one effect paints your whole rig, synchronized, regardless of how many devices or protocols are involved.

![Hypercolor dashboard showing the Neon City effect active across multiple devices](/img/ui/dashboard.webp)

## ⚡ Where to start

Your path through this guide depends on two questions: **how you're installing**, and **which interface you prefer**.

### Pick your install path

Not everyone needs to build from source. The right starting point depends on your OS and your goals.

| If you are… | Start here |
|---|---|
| A Linux user who wants a quick install | [Choose your install](@/guide/choose-your-install.md) → prebuilt one-liner |
| On Windows or macOS | [Choose your install](@/guide/choose-your-install.md) → desktop package |
| Running Arch Linux | [Choose your install](@/guide/choose-your-install.md) → AUR package |
| A developer hacking on Hypercolor itself | [Installation](@/guide/installation.md) → build from source |

### Pick your interface

Hypercolor has six entry points. Understanding which one you want saves a lot of confusion.

| Interface | What it is | When to use it |
|---|---|---|
| **Desktop app** | Tauri shell that owns the tray, supervises the daemon, and renders the web UI natively | The recommended starting point on Windows and macOS; available on Linux too |
| **Web UI** | Leptos app served by the daemon at `http://localhost:9420` | Daily use: browsing effects, tweaking controls, managing layouts |
| **TUI** | Ratatui terminal dashboard with live LED preview and audio spectrum | SSH sessions, headless setups, or if you live in the terminal |
| **CLI** | `hypercolor` binary for scripting and quick control | Automation, shell scripts, CI pipelines |
| **Tray applet** | System tray icon with a brightness submenu and quick actions | Minimal desktop footprint; change effects without opening a window |
| **REST + WebSocket API** | Daemon's full HTTP interface on `:9420` | Integrations, agents, and anything programmatic |

[The pieces](@/guide/the-pieces.md) walks through how these connect and which to open first.

## What's in this section

This section takes you from zero to a fully configured rig.

**Getting set up**

- [Choose your install](@/guide/choose-your-install.md) — prebuilt vs. packaged vs. source, by OS and skill level
- [Installation](@/guide/installation.md) — full install instructions for every path
- [The pieces](@/guide/the-pieces.md) — mental model: daemon, app, tray, TUI, CLI, web UI, and how they connect
- [Scope](@/guide/scope.md) — what Hypercolor does and does not control today

**First steps**

- [First launch](@/guide/first-launch.md) — what happens on first run: the welcome wizard, device discovery, autostart
- [Your first 10 minutes](@/guide/your-first-10-minutes.md) — opinionated happy path: open app → find devices → apply an effect → save a profile
- [Quick start](@/guide/quick-start.md) — zero to RGB in five minutes via CLI and web UI
- [First session](@/guide/first-session.md) — longer hands-on walkthrough covering the full feature set

**Going deeper**

- [The TUI](@/guide/tui.md) — terminal dashboard: LED preview, audio spectrum, fullscreen mode
- [Finding devices](@/guide/finding-devices.md) — USB discovery, network mDNS, pairing Hue and Nanoleaf, udev permission fixes
- [Audio setup](@/guide/audio-setup.md) — configure PipeWire or PulseAudio for audio-reactive effects
- [Profiles and scenes](@/guide/profiles-and-scenes.md) — save full rig state in a profile, automate lighting with scenes
- [Configuration](@/guide/configuration.md) — full config reference: `~/.config/hypercolor/hypercolor.toml`
- [Desktop app](@/guide/desktop-app.md) — the Tauri shell: tray menu, autostart, diagnostics, window controls

**Reference**

- [Uninstall](@/guide/uninstall.md) — remove Hypercolor cleanly
- [Changelog](@/guide/changelog.md) — what changed in each release

## How the engine works

{% mermaid() %}
graph LR
    subgraph Input
        A[Audio FFT]
        B[Screen capture]
        C[Keyboard / MIDI]
    end

    subgraph Engine
        D[Effect renderer<br>Servo · Canvas · GLSL]
        E[Canvas<br>640×480 default]
        SF[SparkleFlinger<br>compositor]
        F[Spatial sampler]
    end

    subgraph Hardware
        G[Razer · Corsair · ASUS<br>USB / HID / SMBus]
        H[WLED · Hue · Nanoleaf · Govee<br>UDP / REST / mDNS]
        I[QMK · Ableton Push 2<br>USB HID / MIDI]
    end

    A & B & C --> D
    D --> E --> SF --> F
    F --> G & H & I
{% end %}

Effects render into a virtual RGBA canvas (640×480 by default, tunable). **SparkleFlinger** — the render-thread compositor — latches the newest surface from each active layer at the frame boundary and blends them into one canonical frame per tick. The spatial engine samples that frame at each LED's physical position using normalized `[0.0, 1.0]` coordinates, so effects stay resolution-independent regardless of canvas size. Device output is queued asynchronously so a slow device never stalls the render loop.

The daemon runs an adaptive render loop across five FPS tiers (10 / 20 / 30 / 45 / 60). The default target is 30 fps. The loop downshifts fast on consecutive budget misses and upshifts slowly on sustained headroom, so Servo-heavy effects and simpler ones share the same engine without tuning.

## Supported hardware

Hypercolor currently ships working drivers for 179 devices across 12 driver families, with 233 more researched or in progress. See the full [compatibility matrix](@/hardware/compatibility.md) for the current state by vendor and device.

If you own hardware that is not yet supported, see [contributing a driver](@/contributing/adding-a-driver.md).

{% callout(type="warning") %}
**Remove conflicting RGB software before starting.** openrazer daemon, OpenRGB, Aura Sync, and iCUE all grab USB HID devices exclusively. If one of them is running when Hypercolor starts, your devices will appear in `lsusb` but not in `hypercolor devices list`. Stop them first — or run `hypercolor diagnose` to identify the conflict.
{% end %}

## Effects

Hypercolor renders effects through an embedded Servo browser (HTML Canvas and WebGL2 / GLSL) and a set of native Rust built-ins compiled directly into the engine. Browse the full library in the [effect catalog](@/effects/catalog.md), or head to the [effects section](@/effects/_index.md) to learn how to write your own.

The TypeScript SDK is published to npm as [`hypercolor`](https://www.npmjs.com/package/hypercolor) (early 0.1.x release). Scaffold a workspace with `bun create hypercolor` — see [effects setup](@/effects/setup.md).

![The Hypercolor effects browser](/img/ui/effects.webp)

## Interfaces

**Web UI** — served by the daemon at `http://localhost:9420` with no separate process needed. Browse effects, adjust controls live, manage devices, and design spatial layouts from any browser.

![Web UI showing the Studio zone editor](/img/ui/studio.webp)

**Studio** — the zone editor inside the web UI. Divide your canvas into zones, each running an independent effect with its own controls and priority. See the [Studio section](@/studio/_index.md) for the full walkthrough.

**TUI** — a Ratatui terminal dashboard with true-color LED preview, audio visualization, and fullscreen effect rendering.

![TUI dashboard with live preview and device table](/img/tui/tui-dashboard.png)

**CLI** — the `hypercolor` binary talks to the daemon over HTTP. Every action you can take in the UI is available via the CLI. See the [CLI reference](@/api/cli.md).

**MCP server** — 16 tools, 5 resources, and 3 prompts for AI assistant integration (Claude Code, Cursor, and friends). The MCP server is **disabled by default** — see the [MCP server reference](@/api/mcp.md) for how to enable it and connect your agent.

## Getting help

If something is not working, start with `hypercolor diagnose` — it runs a health check and prints a summary of what the daemon can see. From there, the [troubleshooting section](@/troubleshooting/_index.md) has symptom-first answers for the most common issues: devices not found, audio not reacting, network discovery failures, and performance problems.
