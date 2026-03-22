<p align="center">
  <img src="docs/images/dashboard.png" alt="Hypercolor — Neon City effect live on dashboard" width="800">
</p>

<h1 align="center">Hypercolor</h1>

<p align="center">
  <strong>Open-Source RGB Lighting Engine for Linux</strong><br>
  <sub>✦ Effects are web pages. Your desk is the canvas. ✦</sub>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024_Edition-e135ff?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/Servo-Web_Engine-80ffea?style=for-the-badge&logo=servo&logoColor=black" alt="Servo">
  <img src="https://img.shields.io/badge/Leptos-WASM_UI-ff6ac1?style=for-the-badge&logo=webassembly&logoColor=white" alt="Leptos">
  <img src="https://img.shields.io/badge/TypeScript-Effect_SDK-f1fa8c?style=for-the-badge&logo=typescript&logoColor=black" alt="SDK">
  <img src="https://img.shields.io/badge/wgpu-GPU_Shaders-50fa7b?style=for-the-badge&logo=vulkan&logoColor=white" alt="wgpu">
</p>

<p align="center">
  <a href="https://github.com/hyperb1iss/hypercolor/blob/main/LICENSE">
    <img src="https://img.shields.io/github/license/hyperb1iss/hypercolor?style=flat-square&logo=apache&logoColor=white" alt="License">
  </a>
</p>

<p align="center">
  <a href="#-the-vision">Vision</a> •
  <a href="#-how-it-works">How It Works</a> •
  <a href="#-the-effect-sdk">Effect SDK</a> •
  <a href="#-features">Features</a> •
  <a href="#-quickstart">Quickstart</a> •
  <a href="#-the-ui">The UI</a> •
  <a href="#️-the-tui">The TUI</a> •
  <a href="#-architecture">Architecture</a> •
  <a href="#-contributing">Contributing</a>
</p>

---

## 🔮 The Vision

RGB lighting on Linux has always been fragmented — a patchwork of single-vendor tools, half-working
daemons, and effects that look like they were designed in 2012. Meanwhile, the best effects live
inside proprietary Windows-only apps.

**Hypercolor changes that.**

A single Rust daemon that orchestrates every RGB device on your desk — keyboards, mice, LED strips,
case lighting — unified under one engine. Effects aren't hardcoded C++ routines. They're
**web pages** — HTML Canvas, WebGL, GLSL shaders — rendered by an embedded Servo browser and
sampled onto your physical LED layout at 60fps.

Write an effect in TypeScript. Watch it run on your keyboard.

## ⚡ How It Works

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  Effect SDK  │────▶│   Canvas     │────▶│   Spatial     │
│  (TS/GLSL)   │     │  320 × 200   │     │   Sampler     │
└──────────────┘     └──────────────┘     └──────┬───────┘
                                                  │
               ┌───────────┬──────────┬───────────┐
               ▼           ▼          ▼           ▼
         ┌──────────┐ ┌────────┐ ┌────────┐ ┌────────┐
         │  Razer   │ │Corsair │ │  ASUS  │ │  WLED  │
         │  USB/HID │ │USB/HID │ │USB/I2C │ │UDP/DDP │
         └──────────┘ └────────┘ └────────┘ └────────┘
         ┌──────────┐ ┌────────┐ ┌────────┐ ┌────────┐
         │ PrismRGB │ │  Hue   │ │Nanoleaf│ │  QMK   │
         │  USB/HID │ │  REST  │ │  REST  │ │USB/HID │
         └──────────┘ └────────┘ └────────┘ └────────┘
```

1. **Effects render to a virtual canvas** — a 320×200 pixel buffer, using HTML Canvas, WebGL, or native GLSL shaders
2. **The spatial engine samples that canvas** at each LED's physical position using bilinear interpolation
3. **Color data flows to hardware** over USB and UDP — every device gets the right pixels from the right part of the canvas
4. **Audio, screen capture, and keyboard input** feed back into effects in real time

The result: one effect paints the whole room. Your keyboard, your LED strip, your case fans — all
synchronized, all from the same visual source.

## ✦ The Effect SDK

Effects are TypeScript. The SDK provides a declarative API where **the shape of your data defines
the control type** — no boilerplate, no decorators, no XML manifests.

**A complete shader effect in 11 lines:**

```typescript
import { effect } from '@hypercolor/sdk'
import shader from './fragment.glsl'

export default effect('Borealis', shader, {
    speed:          [1, 10, 5],       // → slider
    intensity:      [0, 100, 82],     // → slider
    curtainHeight:  [20, 90, 55],     // → slider
    palette:        ['Northern Lights', 'SilkCircuit', 'Cyberpunk', 'Sunset'],  // → dropdown
}, {
    description: 'Aurora borealis — layered curtains of light',
})
```

**Or go pure GLSL — a single file is a complete effect:**

```glsl
#pragma hypercolor "Plasma Engine" by "Hypercolor"
#pragma control speed "Speed" float(1, 10) = 5
#pragma control palette "Palette" enum("Fire", "Ice", "Neon")

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    // Your shader code — full Shadertoy compatibility
}
```

**Four progressive tiers** meet you where you are:

| Tier | What | For |
|------|------|-----|
| **GLSL** | Single `.glsl` file with `#pragma` controls | Shader artists — zero JS needed |
| **`effect()`** | One-liner shader binding with typed controls | Most effects — 87% less code than legacy patterns |
| **`canvas()`** | Stateless or stateful Canvas 2D draw functions | Generative art, particle systems, text effects |
| **Full OOP** | Class-based with lifecycle hooks | Complex multi-scene effects, advanced state |

The SDK implements the **LightScript API** — a clean, well-documented interface for effect
authoring. Audio data, control values, and canvas context are injected automatically. Effects
compile to self-contained HTML files with embedded metadata, ready to drop into the engine.

### 🎵 Audio-Reactive Effects

```typescript
import { effect, audio } from '@hypercolor/sdk'

// In shaders: { audio: true } injects 18 uniforms automatically
// iAudioBass, iAudioMid, iAudioTreble, iAudioBeat, iAudioBpm...

// In canvas: pull model
const data = audio()  // → { bass, mid, treble, beat, bpm, spectrum... } | null
```

### 🎨 Built-In Effects

Hypercolor ships with **30+ handcrafted effects** spanning ambient, audio-reactive, generative,
and interactive categories:

| | | | |
|---|---|---|---|
| Borealis | Neon City | Digital Rain | Meteor Storm |
| Shockwave | Voronoi Glass | Bubble Garden | Spectral Fire |
| Plasma Engine | Synth Horizon | Deep Current | Lava Lamp |
| Poisonous | Fiberflies | Ember Glow | Frost Crystal |
| Nebula Drift | Nyan Dash | Retro Rink | Frequency Cascade |

Every effect is open source, well-documented, and serves as a reference for writing your own.

## 🌈 Features

### 🔌 Device Backends

| Backend | Protocol | Devices |
|---------|----------|---------|
| **Razer** | USB HID (reverse-engineered) | Huntsman V2, Basilisk V3, Blade 14/15, Seiren Emote |
| **Corsair** | USB HID (Link / Lighting Node / LCD) | iCUE LINK System Hub, Lighting Node, LCD displays |
| **ASUS** | USB HID / SMBus I2C | Aura motherboards, GPUs, DRAM |
| **WLED** | UDP DDP + mDNS discovery | Any WLED-compatible LED strip or controller |
| **PrismRGB** | USB HID | PrismRGB 8/S/Mini controllers |
| **Philips Hue** | REST / mDNS | Hue Bridge-connected lights |
| **Nanoleaf** | REST / mDNS | Light Panels, Canvas, Shapes |
| **Dygma Defy** | USB HID | Dygma Defy split keyboard |
| **QMK** | USB HID (raw) | Any QMK-compatible keyboard |
| **Ableton Push 2** | USB Bulk | Push 2 pad/button grid |

### 🖥️ Dual Render Path

- **Servo (embedded browser)** — Full HTML/Canvas/WebGL rendering for SDK effects. Runs the
  complete web platform headless at 60fps. Existing community effects work unmodified.
- **wgpu (native GPU)** — WGSL/GLSL shaders compiled to Vulkan/OpenGL/Metal. For
  Hypercolor-native effects that need maximum performance.

### 🗺️ Spatial Layout Engine

Map your physical desk layout in the UI. Drag devices onto a 2D canvas, define LED topologies
(strips, matrices, rings), and the spatial sampler handles the rest — bilinear interpolation,
area averaging, or Gaussian sampling at every LED position.

### 🎧 Audio Pipeline

Real-time FFT with beat detection, mel-band analysis, chromagram, and spectral features. Effects
can react to bass hits, BPM, spectral centroid, or the full 200-bin spectrum. Lock-free triple
buffering ensures the render loop never blocks on audio.

### ✨ More

- **Scene engine** with priority stacking, Oklab cross-fades, and automation rules
- **REST API + WebSocket** on port 9420 for full programmatic control
- **MCP server** for AI agent integration (Claude, Cursor, etc.)
- **CLI tool** (`hyper`) with table/JSON output and shell completions
- **Hot-reload** — edit an effect, see it live instantly
- **Screen capture** input for ambient backlighting
- **D-Bus integration** for desktop automation triggers

## 🚀 Quickstart

### Prerequisites

- Rust 1.85+ (edition 2024)
- Bun (for SDK effect development)

### Build & Run

```bash
# Clone
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor

# Install locally under ~/.local
./scripts/install.sh

# Build (release)
cargo build --release

# Run the daemon
cargo run --release -p hypercolor-daemon

# Open the UI
open http://localhost:9420
```

The installer builds the daemon, CLI, and web UI, installs a systemd user
service, installs the launcher desktop entry, reloads udev rules, and persists
`i2c-dev` so SMBus RGB devices survive reboot.

### Using Just (recommended)

```bash
just build           # Debug build
just daemon          # Run daemon with preview profile
just tui             # Run the TUI, auto-starting a local daemon if needed
just verify          # fmt + lint + test
just ui-dev          # Leptos UI dev server with hot reload on :9430
just sdk-dev         # SDK dev server with HMR
```

### CLI

```bash
# List effects
hyper effects list

# Activate an effect
hyper effects activate "Neon City"

# With parameters
hyper effects activate "Borealis" --param palette="Cyberpunk" --param speed=8

# Show connected devices
hyper devices

# System status
hyper status
```

### SDK Development

```bash
# Install dependencies
just sdk-install

# Dev server with hot module replacement
just sdk-dev

# Build all effects to self-contained HTML
just effects-build

# Build a single effect
just effect-build borealis
```

## 💎 The UI

A **Leptos 0.8 CSR** web app compiled to WASM, served directly by the daemon.

<table>
  <tr>
    <td align="center">
      <img src="docs/images/effects-browser.png" alt="Effects Browser" width="400"><br>
      <sub>Effects browser with live preview</sub>
    </td>
    <td align="center">
      <img src="docs/images/effect-controls.png" alt="Effect Controls" width="400"><br>
      <sub>Effect controls with real-time canvas</sub>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="docs/images/layout-editor.png" alt="Layout Editor" width="400"><br>
      <sub>Spatial layout editor</sub>
    </td>
    <td align="center">
      <img src="docs/images/devices.png" alt="Devices" width="400"><br>
      <sub>Device management</sub>
    </td>
  </tr>
</table>

- **Effects browser** — search, filter by category/author, favorites, audio-reactive filter
- **Live canvas preview** — the active effect streams in the sidebar and control panel
- **Auto-generated controls** — sliders, dropdowns, color pickers, toggles — all derived from
  effect metadata
- **Spatial layout editor** — drag-and-drop device placement on a 2D canvas
- **Ambient reactivity** — the UI subtly tints its borders and edges to match the active effect
- **Dark/light themes** — dark by default, because the light is the hero
- **Command palette** (⌘K) for keyboard-driven navigation

## 🖥️ The TUI

A **Ratatui** terminal UI with true-color LED preview, audio visualization, and fullscreen effect rendering.

<table>
  <tr>
    <td align="center">
      <img src="docs/images/tui-dashboard.png" alt="TUI Dashboard" width="400"><br>
      <sub>Dashboard with live preview, device table, and quick actions</sub>
    </td>
    <td align="center">
      <img src="docs/images/tui-effects.png" alt="TUI Effects Browser" width="400"><br>
      <sub>Effects browser with control sliders and presets</sub>
    </td>
  </tr>
  <tr>
    <td align="center">
      <img src="docs/images/tui-fullscreen-bubbles.png" alt="Fullscreen Preview — Bubble Garden" width="400"><br>
      <sub>Fullscreen preview — Bubble Garden</sub>
    </td>
    <td align="center">
      <img src="docs/images/tui-fullscreen-cymatics.png" alt="Fullscreen Preview — Cymatics" width="400"><br>
      <sub>Fullscreen preview — Cymatics</sub>
    </td>
  </tr>
</table>

- **Live effect preview** — the active effect rendered in true-color half-block characters
- **Fullscreen mode** (F11) — effect preview fills the entire terminal
- **Effects browser** — search, navigate categories, switch effects with presets
- **Audio spectrum** — real-time level meter and beat indicators in the status bar
- **Device overview** — connected devices with LED counts, types, and status
- **Quick actions** — number keys for instant effect switching

## 🏗️ Architecture

```
crates/
  hypercolor-types/    # Pure data types — zero deps, no logic
  hypercolor-core/     # Engine: traits, render loop, spatial, audio, effects
  hypercolor-hal/      # Hardware abstraction — USB/HID drivers
  hypercolor-daemon/   # Binary: REST API + WebSocket + embedded UI
  hypercolor-cli/      # Binary: `hyper` CLI tool
  hypercolor-ui/       # Leptos 0.8 WASM web UI (Trunk)
sdk/                   # TypeScript SDK (Bun monorepo)
  packages/core/       # @hypercolor/sdk — effect authoring API
  src/effects/         # Built-in effect library
```

**Key design decisions:**

- **Rust** for safety and 60fps render loop performance
- **Servo** for full web platform compatibility in a headless embedded browser
- **wgpu** for GPU abstraction across Vulkan, OpenGL, and Metal
- **Tokio** async runtime with lock-free channels for the hot path
- **Oklab** color space for perceptually uniform transitions and blending
- **Edition 2024**, `#![forbid(unsafe_code)]`, clippy pedantic

### 🔗 API

The daemon exposes a REST + WebSocket API on `:9420`:

```
GET    /api/v1/effects              # List all effects
GET    /api/v1/effects/:id          # Effect detail with controls
POST   /api/v1/effects/:id/apply    # Apply effect to devices
PATCH  /api/v1/effects/current/controls  # Update control values
GET    /api/v1/devices              # Connected devices
GET    /api/v1/layouts              # Spatial layouts
POST   /api/v1/layouts/:id/apply    # Apply a layout
WS     /api/v1/ws                   # Real-time state + frame streaming
```

Full API documentation: [`docs/development/`](docs/development/)

## 📡 Status

Hypercolor is in active development (v0.1.0). The core engine, effect SDK, web UI, and several
device backends are functional. We use Hypercolor daily — every screenshot in this README was
captured from a live instance with real hardware.

**What works today:**
- Daemon with 60fps render loop
- 30+ SDK effects (shader + canvas)
- 10 device backends: Razer, Corsair, ASUS, PrismRGB, WLED, Hue, Nanoleaf, Dygma, QMK, Push 2
- Leptos web UI with live effect preview
- Ratatui TUI with fullscreen preview and audio spectrum
- REST API + WebSocket
- MCP server for AI assistant integration
- CLI with all subcommands
- Spatial layout engine with visual editor
- Audio-reactive pipeline with beat detection
- Hot-reload for effects

**Coming soon:**
- Lian Li Uni Hub device support
- Scene automation engine
- Effect marketplace
- Wasmtime plugin system for community backends

## 💜 Contributing

We welcome contributions! Whether it's new device drivers, effects, UI improvements, or
documentation — there's plenty to build.

```bash
# Fork, clone, then:
just verify              # Make sure everything passes
cargo test --workspace   # Run all tests
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for guidelines.

**Writing effects** is the easiest way to contribute — the SDK makes it trivial to create something
beautiful. Check [`docs/sdk-effect-guide.md`](docs/sdk-effect-guide.md) for the full authoring guide.

## 📄 License

Apache-2.0 — See [LICENSE](LICENSE)

---

<p align="center">
  <a href="https://github.com/hyperb1iss/hypercolor">
    <img src="https://img.shields.io/github/stars/hyperb1iss/hypercolor?style=social" alt="Star on GitHub">
  </a>
  &nbsp;&nbsp;
  <a href="https://ko-fi.com/hyperb1iss">
    <img src="https://img.shields.io/badge/Ko--fi-Support%20Development-ff5e5b?logo=ko-fi&logoColor=white" alt="Ko-fi">
  </a>
</p>

<p align="center">
  <sub>
    If Hypercolor lights up your desk, give us a ⭐ or <a href="https://ko-fi.com/hyperb1iss">support the project</a>
    <br><br>
    ✦ Built with obsession by <a href="https://hyperbliss.tech"><strong>Hyperbliss</strong></a> ✦
  </sub>
</p>
