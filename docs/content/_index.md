+++
title = "Hypercolor"
description = "Open-source RGB lighting orchestration for Linux. Your desk is the canvas. Effects are web pages."
template = "index.html"
sort_by = "weight"
weight = 0
+++

RGB lighting is fragmented. Vendor tools that don't talk to each other, half-working
daemons, and effects that look like they were designed in 2012. Hypercolor is the fix.

One daemon. Every device you own. Keyboards, mice, LED strips, case fans, smart lights —
all driven by the same engine at adaptive FPS up to 60. Effects aren't hardcoded routines.
They're web pages, rendered by an embedded [Servo](https://servo.org) browser and sampled
onto your physical LED layout every frame.

![Hypercolor dashboard running the Neon City effect](/img/ui/dashboard.webp)

---

## How it works

The render pipeline runs on a dedicated thread. Each tick:

1. Input sources — audio FFT, screen capture, keyboard — feed the renderer.
2. **SparkleFlinger** composes all active producers into one canonical RGBA canvas (640×480
   by default, tunable per scene).
3. The **spatial sampler** maps every LED's physical position to a pixel on that canvas.
4. Encoded frames go to devices over USB/HID, SMBus, or the network — all in parallel.

Effects use normalized `[0.0, 1.0]` coordinates and stay resolution-independent. Canvas size
and target FPS retune live; you never restart the daemon for it.

---

## Six things that make Hypercolor different

**Effects are web pages.** Author a lighting effect with HTML Canvas, WebGL2 and GLSL
shaders (via Servo), or the TypeScript SDK. Browse the [effect catalog](@/effects/_index.md)
or [author your own](@/effects/creating-effects.md).

**One engine, all your hardware.** Razer, Corsair, ASUS Aura, Lian Li, QMK, WLED, Philips
Hue, Nanoleaf, Govee — driven from one process. See the full
[compatibility matrix](@/hardware/compatibility.md).

**Audio-reactive out of the box.** Beat detection, FFT, mel-band analysis, chromagram, and a
200-bin spectrum. The render loop never blocks on audio; effects react to bass hits and BPM
without frame drops.

**Spatial layout engine.** Drag devices onto a 2D canvas in the [Studio](@/studio/_index.md),
define LED topologies, and the sampler resolves canvas pixels to physical LEDs with
configurable interpolation: nearest, bilinear, area average, or Gaussian.

**AI-native from the start.** The daemon exposes a built-in MCP server with 16 tools,
5 resources, and 3 prompt templates. Claude Code, Cursor, and any MCP-compatible assistant
can control your lights directly. See the [MCP server](@/api/mcp.md) docs to get started.

**Rust all the way down.** The daemon, CLI, TUI, tray, and HAL drivers are Rust. The web UI
is Rust compiled to WASM via Leptos. Even the embedded browser is Servo. 60fps render loop
with zero-copy frame encoding and adaptive FPS across 5 tiers.

---

## The interfaces

![Hypercolor Studio — zones, effects, and layout editor](/img/ui/studio.webp)

**Web UI** — served directly by the daemon at `http://localhost:9420`. Browse effects,
tweak live controls, design spatial layouts, manage scenes and zones. Ambient reactivity
tints the UI edges to match the active effect.

**Studio** — the [multi-zone workspace](@/studio/_index.md) inside the web UI. Assign
different effects to different zones across your rig. Scene switching with Oklab cross-fades.

**TUI** — a Ratatui terminal dashboard with true-color LED preview and fullscreen effect
rendering. Runs wherever you have a terminal. Launch with `hypercolor tui`.

**CLI** — `hypercolor effects list`, `hypercolor effects activate "Neon City"`,
`hypercolor devices`, `hypercolor scenes activate <id>`. JSON output for scripting.
Full reference at [API: CLI](@/api/cli.md).

---

## Get started

Choose your path:

- **New to Hypercolor?** Start at [Your first 10 minutes](@/guide/your-first-10-minutes.md)
  — install, launch, and get lights running.
- **Browsing effects?** The [Effects section](@/effects/_index.md) covers the catalog and
  every authoring path.
- **Building an effect?** [Creating effects](@/effects/creating-effects.md) walks through
  the TypeScript SDK from scratch.
- **Integrating with an AI assistant?** The [MCP server](@/api/mcp.md) is off by default;
  that page shows how to enable it and what the 16 tools do.
- **Hardware questions?** [Compatibility](@/hardware/compatibility.md) lists 414 tracked
  devices across 32 vendors, with driver status for each.

---

## Sections

- [Guide](@/guide/_index.md) — Installation, first launch, configuration, and the desktop app.
- [Studio](@/studio/_index.md) — Multi-zone workspace: scenes, zones, layers, layouts, and
  the effect cabinet.
- [Effects](@/effects/_index.md) — The TypeScript SDK, GLSL shaders, native Rust effects,
  the catalog, and publishing.
- [API](@/api/_index.md) — Full REST surface, WebSocket binary frames, CLI reference, and
  the [MCP server](@/api/mcp.md): 16 tools, 5 resources, and 3 prompt templates.
- [Hardware](@/hardware/_index.md) — Compatibility matrix, USB and SMBus devices, network
  drivers (Hue, Nanoleaf, WLED, Govee), and device quirks.
- [Troubleshooting](@/troubleshooting/_index.md) — Devices not found, audio not reacting,
  performance, and common issues.
- [Architecture](@/architecture/_index.md) — Render pipeline, event bus, and renderer internals.
- [Contributing](@/contributing/_index.md) — Adding effects, adding drivers, debugging.
- [Theme](@/theme/_index.md) — The Luminary design system as ported to this docs site.
