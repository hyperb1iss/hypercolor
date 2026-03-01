# Hypercolor: Linux RGB Intelligence Report

> Research compiled March 2026 for the Great Linux Migration

---

## 1. Your Hardware Inventory

The system scan revealed **12 RGB-capable devices** currently orchestrated by SignalRGB v2.5.49:

### Platform

| Component | Model |
|---|---|
| CPU | Intel Core i7-14700K |
| Motherboard | ASUS ROG STRIX Z790-A GAMING WIFI II |
| GPU | ASUS Dual RTX 4070 SUPER White OC 12GB |
| RAM | G.Skill Trident Z5 Neo RGB DDR5-6000 CL30 64GB (2x32GB) |

### RGB Devices

| # | Device | USB ID | Protocol | SignalRGB Plugin |
|---|---|---|---|---|
| 1 | ASUS Z790-A AURA Controller | `0B05:19AF` | USB HID | `Asus_Motherboard_Controller.js` |
| 2 | ASUS Dual RTX 4070S White | PCI (SMBus) | I2C/SMBus | `Asus_Ampere_Lovelace_GPU.js` |
| 3 | G.Skill Trident Z5 Neo RGB (x2) | SMBus | I2C (ENE) | `ENE_RAM.js` |
| 4 | Corsair iCUE LINK System Hub | `1B1C:0C3F` | USB HID | `Corsair_ICUE_Link_Hub.js` |
| 5 | Corsair iCUE LINK LCD | `1B1C:0C4E` | USB HID | `Corsair_ICUE_Link_LCD.js` |
| 6 | Razer Huntsman V2 | `1532:026C` | USB HID | `Razer_Modern_Keyboard.js` |
| 7 | Razer Basilisk V3 | `1532:0099` | USB HID | `Razer_Modern_Mouse.js` |
| 8 | Razer Seiren V3 Chroma | `1532:056F` | USB HID | `Razer_Seiren_V3_Mic.js` |
| 9 | PrismRGB Prism S #1 | `16D0:1294` | USB Serial+HID | `Prism_S.js` |
| 10 | PrismRGB Prism S #2 | `16D0:1294` | USB Serial+HID | `Prism_S.js` |
| 11 | Nollie 8 / PrismRGB 8 | `16D5:1F01` | USB HID | `Nollie8 v2.js` / `PrismRGB_8.js` |
| 12 | Dygma Defy | `35EF:0012` | USB HID | N/A (self-managed) |

Plus your **WLED network devices** (not scanned -- they're on ESP32s, not USB).

---

## 2. Linux Compatibility Matrix

### The Verdict

| Device | OpenRGB | Other Linux Tool | Status |
|---|---|---|---|
| ASUS Z790-A AURA | **Yes** | -- | Full RGB control via USB HID + SMBus |
| ASUS RTX 4070S GPU | **Yes** | -- | SMBus/I2C RGB control (needs i2c-dev) |
| G.Skill DDR5 RGB | **Yes** | -- | SMBus via ENE controller |
| Corsair iCUE LINK Hub | Partial | **OpenLinkHub** | Full RGB + fan + LCD via OpenLinkHub |
| Corsair iCUE LINK LCD | No | **OpenLinkHub** | LCD display + RGB fully supported |
| Razer Huntsman V2 | **Yes** | OpenRazer | Per-key RGB, well supported |
| Razer Basilisk V3 | **Yes** | OpenRazer | Multi-zone RGB |
| Razer Seiren V3 Chroma | **Yes** | OpenRazer | Ring light RGB |
| PrismRGB Prism S (x2) | **No** | **Nothing** | SignalRGB-exclusive ecosystem |
| Nollie 8 / PrismRGB 8 | **No** | **Nothing** | SignalRGB-exclusive ecosystem |
| Dygma Defy | N/A | Self-managed | Has its own firmware-level RGB |
| WLED devices | **Yes** | Native | E1.31/DDP network protocol |

### The Hard Truth

**9 of 12 devices work on Linux.** The 3 that don't are your PrismRGB controllers (2x Prism S + 1x Nollie 8). These are 24 channels of ARGB currently driving your case fans, LED strips, strimers, and decorative lighting. That's a significant chunk of your setup.

**Options for the PrismRGB gap:**
1. **Replace them** with controllers that have OpenRGB support (some generic ARGB hubs work)
2. **Reverse engineer them** -- the Prism S has [an open OpenRGB issue (#4943)](https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/4943) but no implementation yet. The USB protocol could potentially be decoded from the SignalRGB JavaScript plugins at `PrismRGB-Plugins` on GitHub
3. **Keep Windows for RGB** (dual-boot, let SignalRGB run on Windows side)
4. **USB passthrough** to a Windows VM running SignalRGB (cursed but possible)
5. **Build the driver yourself** -- you have your OpenRGB fork, the SignalRGB plugin source code to reference, and the USB captures. You're literally the person who could do this.

### Per-Device Details

**Corsair iCUE LINK LCD** -- This is the crown jewel concern, and it's solved. **[OpenLinkHub](https://github.com/jurkovic-nikola/OpenLinkHub)** fully supports H100i/H150i/H170i Elite LCD variants, iCUE LINK System Hub, and LCD display control. Web dashboard at `localhost:27003`. This replaces iCUE entirely on Linux.

**ASUS Motherboard + GPU + RAM** -- OpenRGB has strong ASUS AURA support. The Z790-A is detected via USB HID (`0x19AF` is a known PID). GPU RGB via SMBus requires loading `i2c-dev` kernel module. RAM via ENE SMBus controller. All three work together through OpenRGB.

**Razer** -- Both OpenRGB and the dedicated OpenRazer project support Huntsman V2, Basilisk V3, and Seiren V3 Chroma. Per-key RGB is fully functional.

---

## 3. OpenRGB Ecosystem Map

### Core: OpenRGB 1.0rc2 (approaching stable)
- **Repo**: Migrated to [Codeberg](https://codeberg.org/OpenRGB/OpenRGB) (GitLab + GitHub mirrors)
- **Protocol**: TCP binary on port 6742, protocol v4/v5
- **Status**: Actively maintained by CalcProgrammer1, 208+ commits since rc2

### Plugin System (Qt C++)

| Plugin | What It Does |
|---|---|
| **Effects Plugin** | 50+ effects: Rainbow, Breathing, Comet, GLSL shaders, GIF player, Ambient, audio-reactive |
| **Visual Map Plugin** | Combine devices into unified 2D grid for cross-device effects |
| **Hardware Sync Plugin** | Map CPU/GPU temps to RGB colors |
| **E1.31 Receiver** | Receive DMX/sACN data from xLights, Vixen, etc. |
| **Scheduler Plugin** | Time-based profile switching |
| **Fan Sync Plugin** | Control fan speeds with custom curves |

### SDK Bindings

| Language | Package | Notes |
|---|---|---|
| **Python** | `openrgb-python` (PyPI) | FastLED-style API, sync mode, segments |
| **Rust** | `openrgb2` (crates.io) | Async (tokio), protocol v4+v5 |
| **Node.js** | `openrgb-sdk` (npm) | TypeScript, v0.6.0 |
| **C#** | `OpenRGB.NET` (NuGet) | Used by Artemis RGB |
| **Go** | `go-openrgb` | Community |
| **Java** | `openrgb-wrapper` | Maven |
| **Dart** | `openrgb` (pub.dev) | Community |

### Higher-Level Tools

| Tool | What It Is | Linux? |
|---|---|---|
| **Artemis RGB** | Layer-based profile system, surface editor, game integration, OpenRGB backend | Yes (AUR) |
| **LedFx** | Audio-reactive LED effects engine, React web UI | Yes (Python) |
| **OpenLinkHub** | Corsair iCUE replacement with web dashboard | Yes (Go) |
| **Hyperion/HyperHDR** | Ambilight clone, screen-capture-to-LEDs | Yes |
| **RemoteLight** | Lua scripting for LED effects + OpenRGB plugin | Yes (Java) |

### Network Protocol Support

| Protocol | Direction | Status |
|---|---|---|
| **E1.31 (sACN)** | OpenRGB → WLED | Working (manual config) |
| **E1.31 Receiver** | xLights → OpenRGB | Working (plugin) |
| **DDP** | OpenRGB → WLED | [MR #2867 pending](https://gitlab.com/CalcProgrammer1/OpenRGB/-/merge_requests/2867) |
| **Art-Net** | Not directly | Use E1.31 instead |

---

## 4. WLED Integration

**WLED v0.15.3** (stable) with v0.16.0 in development. Sound Reactive is now merged into mainline.

### How WLED Connects to the Linux Stack

```
┌─────────────────┐     E1.31/DDP      ┌──────────────┐
│    OpenRGB       │ ──────────────────→│  WLED Device  │
│  (USB RGB ctrl)  │     UDP network    │  (ESP32)      │
└────────┬─────────┘                    └──────────────┘
         │ SDK (TCP 6742)                      ↑
         │                                     │ JSON API / DDP
┌────────▼─────────┐                    ┌──────┴───────┐
│   Python/TS      │                    │ Home Assistant│
│   Orchestrator   │ ──────────────────→│   (WLED int.) │
└──────────────────┘                    └──────────────┘
```

**Current path**: Add WLED device IPs to `~/.config/OpenRGB/OpenRGB.json` as E1.31 targets. 170 RGB pixels per universe, multiple universes for longer strips.

**Coming soon**: DDP support (MR #2867) eliminates universe management, supports 480 pixels per packet.

**Home Assistant**: WLED integration is first-class -- auto-discovery via mDNS, WebSocket real-time updates, full automation support. Plays nicely with your existing HA setup.

---

## 5. The Web Renderer Question

### What SignalRGB Does (the benchmark)

SignalRGB's architecture is brilliantly simple:
- Effects are **HTML files** with a `<canvas>` (320x200)
- Rendered by either **Ultralight** (lightweight) or **Qt WebEngine** (full Chromium, needed for WebGL)
- Each frame: canvas pixels are sampled at physical LED positions → colors sent to hardware
- `<meta>` tags define user-facing parameters (sliders, dropdowns, toggles)
- `requestAnimationFrame` loop drives the animation
- Your **lightscript-workshop** elevates this with TypeScript, GLSL shaders via Three.js, decorators, and audio FFT

### What Exists on Linux

**Nothing replicates this exactly.** But the pieces exist:

| Layer | Available Component |
|---|---|
| Effect rendering | Canvas 2D / WebGL in any browser or Electron |
| Spatial mapping | Artemis (Skia), OpenRGB Visual Map Plugin |
| Hardware transport | `openrgb-sdk` (npm), `openrgb-python`, E1.31/DDP |
| Audio input | Web Audio API, PulseAudio/PipeWire |
| Web UI framework | React, Svelte, whatever you want |

**Closest existing projects:**
- **PixelMixer** (audiopixel/pixelmixer) -- Three.js/WebGL → pixel sampling → hardware output via UDP/REST. Missing: OpenRGB output adapter
- **OpenRGB Effects Plugin** -- Has GLSL shader support natively, but it's C++ Qt, not web-based
- **Artemis RGB** -- Surface editor + SkiaSharp rendering + OpenRGB backend. Most feature-complete, but .NET, not web
- **LedFx** -- React web UI + Python backend, but audio-reactive only, no OpenRGB integration

### The Opportunity

A **"SignalRGB for Linux"** built on your lightscript-workshop architecture:
1. TypeScript/WebGL/Canvas effect authoring (you already have this)
2. Replace SignalRGB output with OpenRGB SDK transport via `openrgb-sdk` (npm)
3. Add WLED DDP output for network strips
4. Spatial layout editor (map canvas coordinates to physical LED positions)
5. Home Assistant integration for room-wide orchestration

You literally have the skills, the codebase, and the hardware to build this.

---

## 6. SignalRGB Effect Portability

SignalRGB effects are just HTML files. The format is dead simple:

```html
<head>
  <title>Effect Name</title>
  <meta description="..." />
  <meta property="speed" label="Speed" type="number" min="0" max="100" default="50" />
</head>
<body>
  <canvas id="exCanvas" width="320" height="200"></canvas>
</body>
<script>
  // Standard Canvas 2D API + requestAnimationFrame
  // Meta properties auto-injected as global variables
</script>
```

**Portability assessment:**
- **Canvas 2D effects**: 100% portable. Standard web APIs, run anywhere.
- **WebGL effects**: Portable with a WebGL-capable renderer (browser/Electron).
- **Screen Ambience**: Needs platform-specific screen capture (PipeWire on Linux).
- **Audio-reactive**: Web Audio API works the same, just needs PulseAudio/PipeWire input.
- **SignalRGB-specific globals** (`speed`, `vertical`, etc.): Need a shim layer to inject meta property values as globals.

**Effect locations on your system:**
- Built-in: `C:\Users\Stefanie\AppData\Local\VortxEngine\app-2.5.49\Signal-x64\Effects\`
- Downloaded: `C:\Users\Stefanie\Documents\WhirlwindFX\Effects\` (store downloads)
- Community effects are just `.html` + `.png` pairs

A compatibility shim that parses `<meta>` tags, creates a UI for parameters, injects them as globals, and renders the canvas would make ~90% of SignalRGB effects run unmodified on Linux.

---

## 7. Distro Recommendation

### Top Pick: CachyOS

| Factor | Why CachyOS |
|---|---|
| **Performance** | 10-20% higher FPS than Ubuntu/Nobara/Bazzite in benchmarks |
| **Arch base** | AUR access, latest everything, full system control |
| **OpenRGB** | In official repos (`pacman -S openrgb`) + AUR for bleeding edge |
| **Your ecosystem** | uchroma, OpenRGB fork, Rust tools -- all at home on Arch |
| **Kernel** | Gaming-optimized with real-time tweaks, snappier input |
| **Community** | Tripled in size in 2025, DistroWatch top-rated |

### Runner-Up: Nobara

If you want zero-fuss: OpenRGB comes **preinstalled**, everything gaming works in 15 minutes, Fedora stability. Now rolling-release as of 2025.

### Avoid for RGB: Bazzite

Immutable filesystem = OpenRGB udev rules are painful. USB device access through Flatpak sandboxing is a nightmare. Great distro otherwise, wrong choice for this use case.

### The Three You Mentioned

| Distro | Verdict |
|---|---|
| **Arch** | CachyOS gives you Arch + pre-optimized gaming. No reason to do it manually. |
| **Ubuntu** | Skip for a dedicated gaming rig. Too much manual optimization. |
| **Pop!_OS** | Solid, COSMIC desktop is beautiful, but Ubuntu base = older packages. |

### NVIDIA Driver Rankings

Your RTX 4070 SUPER needs good NVIDIA support:

| Tier | Distro |
|---|---|
| S | Pop!_OS (dedicated ISO), Nobara (preloaded) |
| A | Bazzite (pre-installed), CachyOS (easy install setup) |
| B | Ubuntu (manual enable), Arch (manual but well-documented) |

CachyOS handles NVIDIA well during installation. Not as turnkey as Pop!_OS, but you're not afraid of a little setup.

---

## 8. The Architecture Vision

```
                        ╔══════════════════════╗
                        ║    Hypercolor         ║
                        ║  (Web Effect Engine)  ║
                        ╠══════════════════════╣
                        ║ TypeScript/WebGL/GLSL ║
                        ║ Canvas 2D renderer    ║
                        ║ Spatial layout editor ║
                        ║ Audio FFT pipeline    ║
                        ╚═══════╤══════════════╝
                                │
                 ┌──────────────┼──────────────┐
                 │              │              │
          ┌──────▼──────┐ ┌────▼────┐  ┌──────▼──────┐
          │  OpenRGB    │ │  WLED   │  │    Home     │
          │  SDK (6742) │ │  DDP    │  │  Assistant  │
          └──────┬──────┘ └────┬────┘  └──────┬──────┘
                 │              │              │
    ┌────────────┼───┐    ┌────┼────┐    ┌────┼────┐
    │   USB RGB      │    │ ESP32s  │    │  Scenes  │
    │  Mobo/GPU/RAM  │    │ Strips  │    │  Automations│
    │  Corsair AIO   │    │ Panels  │    │  Triggers │
    │  Razer periph  │    │ Accents │    │           │
    └────────────────┘    └─────────┘    └───────────┘
```

### What `hypercolor` Could Be

A web-based RGB orchestration engine that:
1. **Renders effects** using your lightscript-workshop Canvas/WebGL/GLSL pipeline
2. **Maps pixels** from a 2D canvas to physical LED positions via a spatial layout editor
3. **Pushes colors** to OpenRGB (SDK), WLED (DDP), and HA (REST) simultaneously
4. **Runs SignalRGB effects** with a compatibility shim (~90% of community effects)
5. **Speaks SilkCircuit** -- your design system baked into the UI

The tech stack writes itself: **Next.js 16 + TypeScript + Three.js/WebGL + openrgb-sdk + python-wled**

---

## 9. Migration Strategy

### Phase 1: Dual Boot (Keep Windows for now)
1. Install CachyOS on a separate drive
2. Set up OpenRGB with your core devices (mobo, GPU, RAM, Razer)
3. Install OpenLinkHub for Corsair AIO LCD
4. Configure WLED E1.31 in OpenRGB
5. Test everything, document what works

### Phase 2: Fill the Gaps
1. Reverse engineer PrismRGB Prism S protocol (reference: SignalRGB plugins)
2. Contribute driver to your OpenRGB fork
3. Or: replace Prism controllers with OpenRGB-compatible alternatives
4. Build the Hypercolor web effect engine prototype

### Phase 3: Transcend
1. Hypercolor becomes the unified RGB control plane
2. SignalRGB effects library runs on Linux
3. HA integration orchestrates room-wide scenes
4. You become the person who built "SignalRGB for Linux" and the open source community loses its mind

---

## 10. Recommendation

**Go for it.** CachyOS as the distro. The hardware support gap is narrower than expected -- only the PrismRGB controllers are truly unsupported, and those are either replaceable or reverse-engineerable.

The bigger opportunity here isn't just "run Linux with RGB" -- it's that **nobody has built a proper web-based RGB effect engine for OpenRGB**. That's the gap. You have lightscript-workshop, you have the OpenRGB fork, you have the HA ecosystem, and you have the taste to make it beautiful. Hypercolor could be that project.

The sorceress energy is strong with this one.
