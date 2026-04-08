# 15. Community & Ecosystem Strategy

> How Hypercolor becomes the default RGB lighting engine on Linux.

---

## Table of Contents

1. [Open Source Strategy](#1-open-source-strategy)
2. [Community Building](#2-community-building)
3. [Effect Marketplace](#3-effect-marketplace)
4. [Plugin Ecosystem](#4-plugin-ecosystem)
5. [Hardware Partnerships](#5-hardware-partnerships)
6. [Competitive Positioning](#6-competitive-positioning)
7. [Documentation Strategy](#7-documentation-strategy)
8. [Marketing & Visibility](#8-marketing--visibility)
9. [Sustainability](#9-sustainability)
10. [Success Metrics](#10-success-metrics)
11. [Timeline & Horizons](#11-timeline--horizons)

---

## 1. Open Source Strategy

### 1.1 License Architecture

Hypercolor uses **MIT/Apache-2.0 dual licensing** for all original code, with GPL-licensed components isolated behind process boundaries.

```
hypercolor (MIT/Apache-2.0)
├── hypercolor-core/          MIT/Apache-2.0
├── hypercolor-daemon/        MIT/Apache-2.0
├── hypercolor-tui/           MIT/Apache-2.0
├── hypercolor-cli/           MIT/Apache-2.0
├── web/ (SvelteKit)          MIT/Apache-2.0
│
├── Servo integration         MPL-2.0 (file-level copyleft, no viral spread)
│
└── hypercolor-openrgb-bridge (GPL-2.0)  ← separate binary, separate process
    └── openrgb2 crate        GPL-2.0
```

**Rationale for dual MIT/Apache-2.0:**

- **Maximum adoption.** MIT is the most recognized permissive license. Apache-2.0 adds explicit patent grants -- critical for a project that touches hardware protocols and USB HID. Dual licensing lets downstream consumers pick whichever fits their needs.
- **Corporate-friendly.** Hardware manufacturers and commercial integrators can embed Hypercolor without legal anxiety. This matters for the partnership strategy (Section 5).
- **Community-compatible.** Contributors don't face license friction. No CLA required for simple contributions -- the inbound=outbound principle applies.
- **GPL isolation is architectural, not philosophical.** The `openrgb2` crate is GPL-2.0. Rather than debating license compatibility, we run it as a separate process (`hypercolor-openrgb-bridge`) communicating over gRPC/Unix socket. Clean boundary. No contamination. OpenRGB users get full functionality; the core engine stays permissive.

**MPL-2.0 (Servo):** File-level copyleft. Only modified Servo source files carry MPL obligations. Our wrapper code, our effect engine code, our spatial sampler -- all MIT/Apache-2.0. The Servo integration is a dependency, not a derivative work of our codebase.

### 1.2 Contribution Model

**Contribution Flow:**

```
Idea → Discussion → Issue → Fork → Branch → PR → Review → Merge
```

**GitHub Discussions** serve as the front door:

| Category | Purpose |
|---|---|
| **Ideas** | Feature proposals, brainstorming, "wouldn't it be cool if..." |
| **Q&A** | Setup help, troubleshooting, "how do I..." |
| **Show & Tell** | Screenshots, setups, effect demos, community creations |
| **Effects** | Effect development questions, sharing WIP effects |
| **Plugins** | Plugin development, backend requests, integration ideas |

**Issue Templates:**

- **Bug Report** -- Structured: device info, expected/actual behavior, logs, reproduction steps
- **Feature Request** -- Problem statement, proposed solution, alternatives considered
- **Device Support Request** -- Hardware model, USB VID/PID, existing driver links, willingness to test
- **Effect Bug** -- Effect name/URL, renderer path (wgpu/Servo), expected/actual visual output

**Pull Request Process:**

1. **Small fixes** (typos, docs, single-file changes): Direct PR, single reviewer, fast merge.
2. **Medium changes** (new effect, backend tweak, UI component): PR with description, one maintainer review.
3. **Large changes** (new device backend, architecture change, API modification): RFC first (see 1.4).

**Branch Strategy:**

```
main              ← stable, releasable at all times
dev               ← integration branch for upcoming release
feature/*         ← individual feature branches
fix/*             ← bug fix branches
backend/*         ← device backend work
effect/*          ← effect contributions
```

### 1.3 Governance

**Phase 1 (Year 1): BDFL**

Bliss (hyperb1iss) is the project founder, architect, and sole decision-maker. This is appropriate for a project in its formative phase. Decisions are fast. Vision is coherent. The architecture doc is the constitution.

**Phase 2 (Year 2-3): BDFL + Maintainer Team**

As the project grows, delegate subsystem ownership:

| Subsystem | Maintainer Role |
|---|---|
| **Core engine** (render loop, event bus) | Project lead (Bliss) |
| **Device backends** | Backend maintainer(s) -- one per major backend family |
| **Effect system** (wgpu + Servo) | Effect engine maintainer |
| **Web UI** (SvelteKit) | Frontend maintainer |
| **TUI/CLI** | CLI/TUI maintainer |
| **Documentation** | Docs maintainer |
| **Community effects** | Effect curator(s) |

Maintainers get merge rights for their subsystem. Cross-cutting changes still go through Bliss.

**Phase 3 (Year 3-5): Technical Steering Committee**

If Hypercolor reaches critical mass (50+ regular contributors, multiple corporate sponsors), formalize governance:

- 5-7 member TSC elected by active contributors
- Bliss retains veto on architectural direction (emeritus BDFL)
- TSC handles release management, CoC enforcement, sponsorship allocation
- Inspired by the Rust project's governance model -- teams with clear scope

**The key insight:** Don't over-govern too early. A premature committee structure slows down a project that needs velocity. Let governance emerge from actual need.

### 1.4 Decision-Making: RFC Process

Major changes require an **RFC (Request for Comments)** before implementation:

**What requires an RFC:**
- New device backend architecture
- Effect API changes that break existing effects
- New IPC protocol or API surface
- Plugin system modifications
- License changes to any component
- New hard dependencies

**What does NOT require an RFC:**
- Bug fixes
- New effects (just submit them)
- Documentation improvements
- Performance optimizations that don't change APIs
- New device support using existing backend patterns

**RFC Template:**

```markdown
# RFC: [Title]

## Summary
One paragraph.

## Motivation
Why is this change needed? What problem does it solve?

## Design
Technical details. Code examples. Architecture diagrams.

## Alternatives Considered
What else was evaluated? Why was it rejected?

## Migration Path
How do existing users/effects/plugins adapt?

## Unresolved Questions
What needs further discussion?
```

RFCs live in `docs/rfcs/` and are numbered sequentially. Discussion happens on the PR. After 2 weeks of discussion (or consensus earlier), the project lead makes the call.

### 1.5 Code of Conduct

Adopt the **Contributor Covenant v2.1** -- the de facto standard for open source projects. It's well-understood, legally reviewed, and signals that Hypercolor is a welcoming space.

**Enforcement ladder:**
1. Private warning with specific explanation
2. Temporary interaction ban (1-30 days)
3. Permanent ban

**Enforcement team:** Initially Bliss. Expand to 3 people (diverse backgrounds) once the community reaches 20+ active contributors. CoC enforcement should never be a solo responsibility long-term.

**Key principle:** The RGB enthusiast community skews young and male. Be proactive about making the space welcoming to everyone -- gamers, artists, home automation folks, hardware hackers, embedded developers. The diversity of the user base IS the strength.

---

## 2. Community Building

### 2.1 Target Communities

**Primary targets (high overlap with Hypercolor's value prop):**

| Community | Size | Why They Care | Entry Strategy |
|---|---|---|---|
| **r/linux_gaming** | 600K+ | RGB on Linux is a known pain point | "Open-source RGB lighting for Linux" post |
| **r/pcmasterrace** | 7M+ | RGB enthusiasts, setup showcases | Demo videos of effects running on Linux |
| **r/homeassistant** | 500K+ | Smart home + RGB integration | HA integration announcement |
| **r/WLED** | 30K+ | LED strip enthusiasts, already open source | Native WLED DDP support |
| **r/OpenRGB** | 10K+ | Existing Linux RGB users | Backend compatibility, effect engine |
| **r/unixporn** | 400K+ | Aesthetic Linux setups | TUI screenshots, SilkCircuit theming |

**Secondary targets:**

| Community | Entry Strategy |
|---|---|
| **r/MechanicalKeyboards** | Per-key RGB control, OpenRGB keyboard support |
| **r/battlestations** | Full-setup effect sync demos |
| **r/selfhosted** | Daemon architecture, REST API, headless mode |
| **Lemmy (linux communities)** | Cross-post for Fediverse reach |
| **Hacker News** | Architecture post ("HTML effects engine in Rust") |

**Discord / Matrix communities to engage:**

- OpenRGB Discord -- collaborate, don't compete
- WLED Discord -- effect sharing, DDP protocol coordination
- Rust community (r/rust, Rust Discord) -- showcase wgpu + Servo integration
- Home Assistant Discord -- integration development

### 2.2 Community Hub: Discord + Matrix Bridge

**Discord** is where the RGB enthusiast community lives. **Matrix** is where the open source purists live. Bridge them.

**Discord Server Structure:**

```
HYPERCOLOR
├── #welcome              Rules, getting started, role selection
├── #announcements        Releases, blog posts, events (read-only)
│
├── GENERAL
│   ├── #general          Main chat
│   ├── #showcase         Setup photos, effect demos, "look what I made"
│   └── #off-topic        Gaming, hardware, life
│
├── SUPPORT
│   ├── #help             Troubleshooting, setup questions
│   ├── #device-support   "Does Hypercolor work with my [device]?"
│   └── #bug-reports      Quick triage before GitHub issues
│
├── DEVELOPMENT
│   ├── #dev-general      Architecture discussion, PR reviews
│   ├── #effects          Effect authoring, sharing WIP
│   ├── #backends         Device backend development
│   ├── #web-ui           SvelteKit frontend work
│   └── #tui-cli          Terminal interface development
│
├── EFFECTS LAB
│   ├── #effect-requests  "I wish there was an effect that..."
│   ├── #effect-jams      Community creation events
│   └── #effect-gallery   Curated finished effects with previews
│
└── VOICE
    └── #dev-chat         Voice for pairing, demos, architecture talks
```

**Roles:**
- `@Contributor` -- has merged a PR
- `@Effect Author` -- has published an effect
- `@Backend Dev` -- works on device backends
- `@Maintainer` -- subsystem maintainer
- `@Architect` -- core team / TSC member

**Matrix Bridge:** Use [mautrix-discord](https://github.com/mautrix/discord) or [matrix-appservice-discord](https://github.com/matrix-org/matrix-appservice-discord) to bridge key channels. Matrix users get first-class participation without needing Discord.

### 2.3 Documentation-First Culture

Every PR that changes behavior must update documentation. This is enforced by CI -- a doc linter checks that:

- New CLI flags have corresponding docs
- New API endpoints have OpenAPI annotations
- New effect APIs have JSDoc/rustdoc
- New config options appear in the configuration reference

**"If it's not documented, it doesn't exist."** This isn't gatekeeping -- it's a gift to future contributors (including yourself in 6 months).

### 2.4 First-Time Contributor Experience

**"Good First Issue" program:**

Maintain a curated list of issues tagged `good-first-issue` that are:
- Well-scoped (completable in 1-3 hours)
- Well-described (clear acceptance criteria, relevant code pointers)
- Not blocking anything critical (no pressure)
- Spread across the codebase (core, CLI, effects, docs, tests)

**Examples of good first issues:**
- "Add shell completions for fish"
- "Write a builtin static-color effect"
- "Add device name to TUI status bar"
- "Fix typo in spatial layout documentation"
- "Add color temperature control to Hue backend"

**First PR experience checklist:**
1. `CONTRIBUTING.md` explains setup in < 5 minutes
2. `cargo build` works on a clean clone (no hidden dependencies)
3. CI gives clear, actionable feedback (not cryptic failures)
4. First review happens within 48 hours
5. Merged PRs get a thank-you message and contributor credit

### 2.5 Mentorship

**Office Hours:** Monthly 1-hour video call (Discord voice or YouTube Live) where maintainers walk through the codebase, answer questions, and pair on issues. Record and publish for async consumption.

**"Adopt a Backend" Program:** Pair experienced Rust developers with newcomers to implement a device backend together. Device backends are ideal mentorship projects -- they're self-contained, testable, and produce visible results (LEDs light up!).

**Contributor Ladder:**

```
First-time contributor
  → Regular contributor (3+ merged PRs)
    → Trusted contributor (10+ PRs, review access)
      → Subsystem maintainer (merge rights for their area)
        → Core maintainer (cross-cutting merge rights)
```

Each level comes with explicit expectations and explicit recognition.

### 2.6 Release Cadence

**Release schedule:**

| Track | Cadence | Purpose |
|---|---|---|
| **Nightly** | Daily (automated) | Latest `dev` branch, for testing |
| **Beta** | Every 2 weeks | Pre-release, feedback window |
| **Stable** | Every 6 weeks | Production release |
| **LTS** | Every 6 months | Long-term support for distro packagers |

**Release naming:** Semantic versioning (`0.x.y` pre-1.0, `x.y.z` post-1.0). No cute names until 1.0 -- let the software earn its personality.

**Release process:**
1. Feature freeze on `dev` (1 week before release)
2. Beta cut → community testing
3. Bug fixes only during beta
4. Stable release with changelog, blog post, announcement
5. Distro package updates (AUR, PPA, COPR, Nix)

---

## 3. Effect Marketplace

### 3.1 Architecture: GitHub-Based Effect Repository

The effect marketplace is a **GitHub repository** (`hypercolor/effects`) that serves as both a gallery and a distribution channel.

```
hypercolor/effects/
├── registry.toml                    # Master index of all effects
├── featured.toml                    # Curated featured effects list
│
├── effects/
│   ├── aurora-wave/
│   │   ├── effect.toml              # Metadata (name, author, tags, controls)
│   │   ├── aurora-wave.html         # The effect file
│   │   ├── preview.gif              # Animated preview (required)
│   │   ├── preview.png              # Static thumbnail
│   │   └── README.md                # Description, usage, credits
│   │
│   ├── neon-pulse/
│   │   ├── effect.toml
│   │   ├── neon-pulse.wgsl          # Native wgpu shader
│   │   ├── preview.gif
│   │   └── README.md
│   │
│   └── ...
│
├── collections/                     # Curated effect bundles
│   ├── starter-pack.toml            # Ships with Hypercolor
│   ├── audio-reactive.toml          # Best audio-reactive effects
│   └── ambient.toml                 # Screen capture / ambient effects
│
└── templates/                       # Effect authoring templates
    ├── canvas2d-template/
    ├── webgl-template/
    ├── threejs-template/
    └── wgsl-template/
```

**Effect metadata format (`effect.toml`):**

```toml
[effect]
id = "aurora-wave"
name = "Aurora Wave"
version = "1.2.0"
description = "Northern lights simulation with audio reactivity"
author = "hyperb1iss"
license = "MIT"
tags = ["audio-reactive", "ambient", "organic"]
renderer = "servo"          # "servo" | "wgpu"
min_hypercolor = "0.3.0"    # Minimum compatible version

[effect.audio]
required = false            # Works without audio, enhanced with it
bands = ["bass", "mid"]     # Which audio bands it uses

[controls]
speed = { type = "number", label = "Wave Speed", min = 0.1, max = 5.0, default = 1.0, step = 0.1 }
palette = { type = "combobox", label = "Color Palette", values = ["Aurora", "Sunset", "Ocean", "Neon"], default = "Aurora" }
intensity = { type = "number", label = "Intensity", min = 0.0, max = 1.0, default = 0.7 }
mirror = { type = "boolean", label = "Mirror Mode", default = false }
```

### 3.2 Quality Standards

**Submission requirements:**

1. **Must render at 60fps** on the reference hardware profile (integrated Intel GPU minimum)
2. **Must include animated preview** (GIF or WebM, 3-5 seconds, 320x200 or scaled)
3. **Must include `effect.toml`** with complete metadata
4. **Must declare renderer** -- effects that claim `wgpu` must not secretly require Servo
5. **Must handle missing audio gracefully** if `audio.required = false`
6. **Must not access network, filesystem, or external resources** (Servo sandbox enforced)
7. **Must have a license** (default: MIT, but any OSI-approved license accepted)

**Review process:**

1. Author submits PR to `hypercolor/effects`
2. Automated CI checks:
   - `effect.toml` validates against schema
   - Effect renders without errors on CI (headless wgpu or Servo)
   - Preview assets exist and are reasonably sized (< 2MB)
   - No external resource loading
3. Human review:
   - Visual quality check (does it look good?)
   - Performance check (no frame drops on reference hardware)
   - Control labels make sense
   - Code is not obfuscated
4. Merge → automatically available in Hypercolor's effect browser

**Quality tiers:**

| Tier | Badge | Criteria |
|---|---|---|
| **Community** | -- | Passes automated checks, merged by maintainer |
| **Reviewed** | Checkmark | Human-reviewed for quality and performance |
| **Featured** | Star | Curated by the effect team for exceptional quality |
| **Builtin** | Ship icon | Ships with Hypercolor by default |

### 3.3 Featured Effects Program

Each release highlights **3-5 featured effects** in the release notes and on the web UI's effect browser. Featured effects:

- Demonstrate Hypercolor's capabilities (audio reactivity, multi-zone, advanced visuals)
- Represent diverse styles (ambient, energetic, minimalist, complex)
- Credit the author prominently
- Are showcased in demo videos

**Monthly "Effect of the Month":** Community vote on Discord for the best new effect. Winner gets featured status and a shoutout in the release notes.

### 3.4 Effect Jams

**Quarterly community creation events** with themes:

- **"Neon Nights"** -- Effects inspired by cyberpunk/retrowave aesthetics
- **"Nature's Palette"** -- Organic, nature-inspired effects
- **"Audio Alchemy"** -- Best audio-reactive effect wins
- **"One Shader"** -- Create the most impressive effect in a single wgsl/glsl file

**Structure:**
1. Theme announced 2 weeks before
2. 48-hour creation window (weekend jam)
3. Submissions via PR to `hypercolor/effects` with `[JAM]` tag
4. Community voting on Discord (1 week)
5. Winners featured in next release

**Prizes:** Recognition (featured status, Discord role, contributor profile badge). No monetary prizes until the project has sustainable funding -- recognition-driven communities are healthier.

### 3.5 Attribution & Credit

Every effect displays its author in the UI. The `effect.toml` format supports:

```toml
[effect]
author = "neonartist42"
author_url = "https://github.com/neonartist42"
contributors = ["fixmaster99", "shaderguru"]

[effect.origin]
ported_from = "other-platform"      # If ported from another platform
original_author = "OriginalCreator"
original_url = "https://..."
```

**Credit is non-negotiable.** If an effect is ported from another platform, the original author MUST be credited. If the original license doesn't permit redistribution, the effect cannot be included.

### 3.6 Porting HTML Effects: Legal Considerations

**The landscape:**
- LightScript effects are HTML files. Many are authored by community members.
- Community effects often have no explicit license. This is legally ambiguous.
- WhirlwindFX's built-in effects are proprietary.

**Policy:**

1. **Built-in proprietary effects:** Do NOT port. These are proprietary to their respective owners. Period.
2. **Community effects with explicit open-source licenses:** Port freely with attribution.
3. **Community effects with no license:** Contact the author. Request permission and a license grant. Document the permission in the effect's `effect.toml`.
4. **Effects from the Lightscript Workshop or similar open frameworks:** Check the framework license. If MIT/Apache/similar, ports are fine with attribution.
5. **Inspired-by effects:** Writing a new effect that produces a similar visual result is always fine. You can't copyright a visual style. Just don't copy code.

**"Compatibility, not piracy."** The Servo renderer can RUN existing HTML effects, but the marketplace should contain properly licensed content. Users can point Hypercolor at their own effect files -- that's their prerogative.

---

## 4. Plugin Ecosystem

### 4.1 Plugin Architecture (Phased)

The architecture doc defines three phases of extensibility:

| Phase | Mechanism | When | For Whom |
|---|---|---|---|
| **Phase 1** | Compile-time traits + feature flags | v0.1+ | Core team, known backends |
| **Phase 2** | Wasm plugins (Wasmtime + WIT) | v0.5+ | Community developers |
| **Phase 3** | gRPC process bridge | v0.3+ (for OpenRGB) | GPL isolation, polyglot plugins |

### 4.2 Plugin Registry

**GitHub-based registry** (similar to effects):

```
hypercolor/plugins/
├── registry.toml
│
├── plugins/
│   ├── backend-nanoleaf/
│   │   ├── plugin.toml            # Metadata
│   │   ├── src/                   # Rust source (compiled to Wasm)
│   │   ├── README.md
│   │   └── tests/
│   │
│   ├── input-midi/
│   │   ├── plugin.toml
│   │   └── src/
│   │
│   └── integration-mqtt/
│       ├── plugin.toml
│       └── src/
```

**Plugin metadata (`plugin.toml`):**

```toml
[plugin]
id = "backend-nanoleaf"
name = "Nanoleaf Backend"
version = "0.1.0"
description = "Control Nanoleaf panels and shapes via local API"
author = "community_dev"
license = "MIT"
type = "device-backend"         # "device-backend" | "input-source" | "integration"
min_hypercolor = "0.5.0"

[plugin.capabilities]
discovery = true                # Supports auto-discovery
streaming = true                # Supports real-time frame push
configuration = true            # Has configurable settings

[plugin.dependencies]
network = true                  # Needs network access (Wasm capability)
usb = false                     # Needs USB HID access
```

### 4.3 Plugin Categories

**Device Backends (highest demand):**

| Category | Examples | Difficulty |
|---|---|---|
| **Network LED controllers** | Nanoleaf, Govee, Yeelight, LIFX | Medium |
| **USB peripherals** | SteelSeries, Logitech, HyperX | Medium-Hard |
| **Smart home bridges** | Hue (additional features), Zigbee, Z-Wave | Medium |
| **Display devices** | Govee Dreamview, Ambilight | Medium |
| **Custom hardware** | Arduino, ESP32 (non-WLED), Teensy | Easy-Medium |

**Input Sources:**

| Plugin | Purpose |
|---|---|
| **MIDI input** | Map MIDI controllers to effect parameters |
| **Game integration** | Game state → effect triggers (via D-Bus or file watches) |
| **Sensor input** | Temperature, weather, system metrics → color data |
| **Network audio** | Receive audio from remote sources (Snapcast, PulseAudio network) |

**Integrations:**

| Plugin | Purpose |
|---|---|
| **Home Assistant** | Two-way HA integration (scenes, automations, state sync) |
| **MQTT** | Generic IoT integration |
| **OBS** | Scene change → effect change |
| **Spotify/MPD** | Now-playing → effect palette |

### 4.4 Developer Documentation Quality

Plugin developers need:

1. **Quick Start guide** -- "Build your first backend plugin in 30 minutes"
2. **WIT interface reference** -- Complete documentation of the Wasm plugin API
3. **Example plugins** -- Well-commented reference implementations for each plugin type
4. **Testing harness** -- `hypercolor-plugin-test` crate that simulates the host environment
5. **Dev server** -- `hypercolor dev plugin ./my-plugin` that hot-reloads Wasm on file change

**The bar:** A competent Rust developer should go from zero to a working plugin in an afternoon. If it takes longer, the docs have failed.

### 4.5 Plugin Verification

**Levels:**

| Level | Meaning | Badge |
|---|---|---|
| **Unverified** | Community submission, use at your own risk | -- |
| **Tested** | Passes automated test suite on CI | Checkmark |
| **Verified** | Human-reviewed by maintainer, security audit | Shield |
| **Official** | Maintained by Hypercolor core team | Star |

**Wasm sandboxing** provides baseline safety -- plugins can't access the filesystem, network, or USB without explicit capability grants defined in `plugin.toml`. This is a significant advantage over native plugin systems.

### 4.6 Plugin Development Incentives

- **Featured plugin** status for high-quality contributions
- **"Plugin of the Month"** highlight
- **Hardware testing program** -- lend devices to plugin developers who need them (funded by sponsors)
- **Plugin bounties** -- community-funded bounties for specific device support (via Open Collective)

---

## 5. Hardware Partnerships

### 5.1 Partnership Tiers

```
Tier 1: "Works with Hypercolor" (Passive)
  → We support their hardware via reverse engineering or public APIs
  → No formal relationship needed
  → Most hardware falls here

Tier 2: "Hypercolor Compatible" (Collaborative)
  → Manufacturer provides documentation or test hardware
  → We provide official backend support
  → Joint announcement

Tier 3: "Hypercolor Certified" (Active)
  → Manufacturer integrates Hypercolor testing into QA
  → Co-branded marketing
  → Priority support channel
```

### 5.2 Target Manufacturers

**High Priority (already open-source-friendly or Linux-aware):**

| Manufacturer | Product Line | Approach |
|---|---|---|
| **WLED community** | ESP32 LED controllers | Already open source. Ensure DDP support is flawless. Contribute upstream if needed. Cross-promote. |
| **Lian Li / PrismRGB** | Strimer cables, Prism 8, Nollie 8 | We've already reverse-engineered their protocols. Offer them a reference Linux driver. Propose Tier 2 partnership -- they gain Linux support with zero engineering cost. |
| **Corsair** | iCUE LINK ecosystem | Engage via OpenLinkHub project. Corsair has historically been hostile to open source, but iCUE LINK is gaining community support. |
| **Razer** | Keyboards, mice, accessories | OpenRazer exists and is well-supported. Coordinate with OpenRazer maintainers for Hypercolor integration. |

**Medium Priority (large user base, closed ecosystems):**

| Manufacturer | Strategy |
|---|---|
| **ASUS (AURA)** | OpenRGB already supports most ASUS hardware. Focus on ensuring Hypercolor works seamlessly with OpenRGB's ASUS backend. |
| **MSI (Mystic Light)** | Same as ASUS -- lean on OpenRGB. |
| **Gigabyte (RGB Fusion)** | Same approach. |
| **Nanoleaf** | Local API is documented. Community plugin opportunity. |
| **Govee** | LAN API exists (unofficial). Community plugin opportunity. |
| **Philips Hue** | Official API, well-documented. First-party backend in roadmap. |

**Strategic (moonshot):**

| Manufacturer | Why |
|---|---|
| **Valve** | SteamOS is Linux. If Hypercolor works on Steam Deck, Valve might promote it. |
| **System76** | Linux hardware company. Natural partner for "RGB that just works on Linux." |
| **Framework** | Linux-first laptop. LED module integration? |

### 5.3 Engaging PrismRGB / Lian Li

This is the most strategic early partnership. Bliss has already reverse-engineered the Prism S, Prism 8, and Prism Mini protocols. The pitch:

**What we offer Lian Li:**
- First-ever Linux support for PrismRGB controllers
- Open-source driver code (MIT/Apache-2.0) they can reference or bundle
- Testing and bug reports from a dedicated community
- Marketing: "PrismRGB now works on Linux" is a headline

**What we ask:**
- Protocol documentation (to validate our reverse engineering)
- Test hardware for new products (before launch if possible)
- Permission to use "PrismRGB" and "Strimer" trademarks in compatibility claims
- A link to Hypercolor from their support pages

**Cold outreach template:**

> Subject: Open-Source Linux Driver for PrismRGB Controllers
>
> Hi [contact],
>
> I'm the developer of Hypercolor, an open-source RGB lighting engine for Linux. I've reverse-engineered the USB HID protocols for Prism S, Prism 8, Prism Mini, and Nollie 8, and I have working Linux drivers for all four devices.
>
> PrismRGB currently has zero Linux support. I'd love to collaborate to make this official. I'm offering:
> - MIT/Apache-2.0 licensed driver code you can reference freely
> - Community testing across Linux distributions
> - "PrismRGB Certified" status in our compatibility list
>
> In return, protocol documentation and test hardware for new products would help us maintain quality.
>
> I founded CyanogenMod (now LineageOS), which brought Android to 50M+ devices through community-driven open source. I'd love to do something similar for RGB on Linux.

### 5.4 WLED Community Integration

WLED is the most natural ally. It's already open source, already has a massive community, and already speaks DDP/E1.31.

**Integration points:**
- Hypercolor's WLED backend uses DDP (480 pixels/packet, no universe management)
- mDNS auto-discovery finds WLED devices on the network
- WLED's JSON API for configuration (segment management, preset activation)
- Hypercolor can act as a "WLED orchestrator" -- sync multiple WLED devices to a single effect

**Community actions:**
- Submit DDP improvements upstream to WLED if we find protocol edge cases
- Write a "Hypercolor + WLED" setup guide
- Create WLED-optimized effects (long strips, matrix panels)
- Participate in WLED Discord

### 5.5 "Works with Hypercolor" Certification

**Certification process:**

1. Device backend exists and is at least "Tested" quality
2. Device discovery works reliably (USB enumeration or mDNS)
3. Frame delivery is consistent at 60fps
4. Graceful handling of disconnect/reconnect
5. At least 3 community members have verified with the specific hardware

**Certification badge:** A simple "Works with Hypercolor" graphic that hardware reviewers and community members can reference. Not a formal certification program (too early) -- just a community-verified compatibility list.

**Compatibility database:** A searchable table on the Hypercolor website/docs:

```
| Device              | Backend    | Status      | Verified By        |
|---------------------|------------|-------------|--------------------|
| WLED ESP32          | wled (DDP) | Certified   | Core team + 50 users |
| PrismRGB Prism 8    | hid        | Certified   | hyperb1iss         |
| PrismRGB Prism S    | hid        | Certified   | hyperb1iss         |
| Razer Huntsman V2   | openrgb    | Verified    | 12 users           |
| Nanoleaf Shapes     | nanoleaf   | Community   | 3 users            |
```

---

## 6. Competitive Positioning

### 6.1 Competitive Landscape Map

```
                    Open Source
                        |
                        |
            Hypercolor  |  OpenRGB
               *        |     *
                        |
     Modern ────────────┼──────────── Legacy
     Architecture       |             Architecture
                        |
               LedFx    |  Artemis
                *       |     *
                        |
                   Closed Source
                        |
               Proprietary (Windows)
                      *
```

### 6.2 vs. Proprietary Alternatives

**Their strengths:** 230+ effects, polished UI, extensive hardware support, it just works on Windows.

**Their weaknesses:** Windows-only, closed source, subscription model for premium, no Linux support ever coming, proprietary renderers.

**Hypercolor's pitch:**

> "Everything proprietary tools do, but open source, Linux-native, and yours to keep."

| Dimension | Closed-Source Alternative | Hypercolor |
|---|---|---|
| **Platform** | Windows only | Linux-first (future: cross-platform) |
| **License** | Proprietary | MIT/Apache-2.0 |
| **Effect compatibility** | Native | ~90% via Servo compatibility layer |
| **Effect format** | HTML/Canvas (proprietary) | HTML/Canvas + native wgpu shaders |
| **Performance** | 60fps (proprietary renderer) | 60fps (Servo) + 1000s fps (wgpu) |
| **Customization** | Limited (closed source) | Infinite (modify anything) |
| **Price** | Free tier + subscription | Free forever |
| **Community effects** | Curated store | Open marketplace (GitHub) |
| **Smart home** | None | Home Assistant integration |

**Key narrative:** Hypercolor doesn't need to beat closed-source tools on Windows. It needs to be **so good on Linux** that Windows users consider switching. The existence of a compelling open-source alternative creates pressure regardless.

### 6.3 vs. OpenRGB

**Their strengths:** Widest hardware support (1000+ devices), established community, approaching 1.0, cross-platform.

**Their weaknesses:** C++/Qt (hard to contribute), basic effects plugin (no web effects), dated UI, no web-based control.

**Hypercolor's positioning:** Not a competitor -- a **complement**.

> "OpenRGB handles the hardware. Hypercolor handles the art."

| Dimension | OpenRGB | Hypercolor |
|---|---|---|
| **Focus** | Hardware control | Effect rendering + orchestration |
| **Effect system** | Basic (50 effects, C++ plugin) | Advanced (HTML/WebGL/wgpu, unlimited) |
| **Architecture** | Monolithic C++/Qt | Modular Rust + Web |
| **UI** | Qt desktop app | Web UI + TUI + CLI |
| **Hardware support** | 1000+ devices | Uses OpenRGB as a backend! |
| **Smart home** | None | Home Assistant integration |
| **Audio** | Basic (effects plugin) | Full Lightscript audio API |

**Strategy:** Position Hypercolor as the effect engine that sits ON TOP of OpenRGB. OpenRGB handles device enumeration and protocol translation. Hypercolor handles effect rendering, spatial mapping, and multi-device orchestration. The gRPC bridge makes this relationship explicit.

**Community diplomacy:** Engage CalcProgrammer1 (OpenRGB lead) early. Show that Hypercolor drives more users TO OpenRGB, not away from it. Contribute PrismRGB support upstream to OpenRGB as a goodwill gesture.

### 6.4 vs. Artemis

**Their strengths:** Layer-based profile system, surface editor, game integrations, .NET ecosystem.

**Their weaknesses:** C#/.NET (not native Linux performance), Windows-centric design ported to Linux, complex setup, small development team.

**Hypercolor's pitch:**

> "Artemis tried to bring Windows RGB to Linux. Hypercolor was born here."

| Dimension | Artemis | Hypercolor |
|---|---|---|
| **Language** | C# (.NET) | Rust |
| **Performance** | Good (Skia) | Superior (wgpu native) |
| **Architecture** | Desktop app | Daemon + Web UI + TUI |
| **Effect format** | C# plugins, Lua scripts | HTML/WebGL + wgpu shaders |
| **Linux integration** | Ported from Windows | Native (systemd, D-Bus, PipeWire) |
| **Smart home** | None | Home Assistant integration |

### 6.5 vs. LedFx

**Their strengths:** Audio-reactive focus is excellent, Python is accessible, React web UI, active community.

**Their weaknesses:** Python performance ceiling, audio-only (no static/screen effects), no OpenRGB integration, limited device support.

**Hypercolor's positioning:** Superset.

> "LedFx perfected audio-reactive effects. Hypercolor does audio-reactive AND everything else."

| Dimension | LedFx | Hypercolor |
|---|---|---|
| **Scope** | Audio-reactive LEDs | All RGB use cases |
| **Performance** | Python (adequate) | Rust (no ceiling) |
| **Effects** | Audio-reactive only | Audio, static, screen capture, scripted |
| **Hardware** | WLED, some others | OpenRGB + WLED + HID + Hue + plugins |
| **UI** | React web app | SvelteKit web + Ratatui TUI + CLI |

### 6.6 Hypercolor's Unique Value Propositions

No other project offers this combination:

1. **HTML effects engine.** Effects are web pages. The entire web platform is your creative canvas. Write effects in HTML/CSS/Canvas/WebGL/Three.js. Compatible with the 230+ community LightScript effects.

2. **Dual-path rendering.** wgpu for maximum performance (native shaders at 1000+ fps). Servo for maximum compatibility (run existing HTML effects unmodified). Choose per-effect.

3. **Lightscript compatibility.** The only open-source engine that can run existing community HTML effects with minimal or no modification.

4. **Rust performance + safety.** 60fps render loop, zero-copy frame delivery, safe USB HID access, no GC pauses, no Python GIL.

5. **Daemon-first architecture.** Runs headless on a server, NAS, or Raspberry Pi. Control via web UI, TUI over SSH, CLI scripts, D-Bus, or REST API. No GUI required.

6. **Smart home integration.** Home Assistant, MQTT, D-Bus. Your RGB is part of your home automation, not a standalone island.

7. **Founded by CyanogenMod's creator.** Not just another weekend project. Built by someone who has shipped open-source platform software to millions of devices.

---

## 7. Documentation Strategy

### 7.1 Documentation Architecture

```
docs.hypercolor.dev (mdBook)
├── Getting Started
│   ├── Installation
│   ├── Quick Start (5-minute guide)
│   ├── Your First Setup
│   └── Migrating from Windows
│
├── User Guide
│   ├── Configuration Reference
│   ├── Device Setup
│   │   ├── WLED
│   │   ├── OpenRGB
│   │   ├── PrismRGB
│   │   ├── Philips Hue
│   │   └── USB HID devices
│   ├── Spatial Layout Editor
│   ├── Effect Browser
│   ├── Profiles & Scenes
│   ├── Audio Setup (PipeWire/PulseAudio)
│   ├── Screen Capture Setup
│   ├── Web UI
│   ├── TUI
│   ├── CLI Reference
│   └── Troubleshooting
│
├── Effect Authoring Guide
│   ├── Your First Effect (Canvas 2D)
│   ├── WebGL Effects (Three.js)
│   ├── Native Shaders (wgpu/WGSL)
│   ├── Audio-Reactive Effects
│   ├── Effect Controls & Parameters
│   ├── Lightscript API Reference
│   ├── Performance Optimization
│   ├── Publishing to the Marketplace
│   └── Porting HTML Effects
│
├── Plugin Development Guide
│   ├── Plugin Architecture Overview
│   ├── Your First Device Backend
│   ├── WIT Interface Reference
│   ├── Input Source Plugins
│   ├── Integration Plugins
│   ├── Testing & Debugging
│   └── Publishing to the Registry
│
├── Architecture Guide (for contributors)
│   ├── System Overview
│   ├── Render Loop
│   ├── Event Bus
│   ├── Spatial Engine
│   ├── Device Backend Traits
│   ├── Servo Integration
│   ├── wgpu Pipeline
│   ├── IPC Protocol
│   └── Build System
│
├── API Reference
│   ├── REST API (OpenAPI/Swagger)
│   ├── WebSocket Protocol
│   ├── D-Bus Interface
│   └── Unix Socket Protocol
│
└── Community
    ├── Contributing (CONTRIBUTING.md)
    ├── Code of Conduct
    ├── RFC Process
    ├── Release Notes
    └── FAQ
```

### 7.2 The Hypercolor Book

**Tool:** [mdBook](https://github.com/rust-lang/mdBook) -- the same tool used by the Rust Book, the wgpu guide, and dozens of Rust projects. It's the ecosystem standard.

**Why mdBook:**
- Rust-native (dogfood the ecosystem)
- Markdown source (easy to contribute)
- Built-in search
- GitHub Pages deployment (free hosting)
- Dark mode (essential for an RGB project)
- Custom themes (SilkCircuit-themed)

**Deployment:** `docs.hypercolor.dev` on GitHub Pages, auto-deployed on every merge to `main`.

### 7.3 Documentation Quality Bar

**Every feature must have:**
1. **Conceptual docs** -- What is it? Why does it exist?
2. **Tutorial** -- Step-by-step guide to using it
3. **Reference** -- Complete API/configuration documentation
4. **Examples** -- Working code/config examples

**The "explain it to a gamer" test:** If a PC gaming enthusiast who has never used a terminal can't follow your getting-started guide, rewrite it.

### 7.4 Video Content

**YouTube channel:** `Hypercolor RGB`

**Content calendar (post-launch):**

| Month | Video | Purpose |
|---|---|---|
| Launch | "Hypercolor: Open-Source RGB for Linux" (5 min) | Project introduction, demo reel |
| +1 | "Setting Up Hypercolor with WLED" (10 min) | Most common use case |
| +2 | "Create Your First Effect" (15 min) | Effect authoring tutorial |
| +3 | "Hypercolor + OpenRGB: Full Setup" (10 min) | Complete desktop RGB |
| +4 | "Audio-Reactive Effects Deep Dive" (15 min) | Showcase audio system |
| +5 | "Building a Device Plugin" (20 min) | Plugin development |
| +6 | "6 Months of Hypercolor" (10 min) | Progress update, community showcase |

**Demo GIFs:** Every effect, every feature, every device should have a demo GIF in the docs. RGB is visual -- show, don't tell.

### 7.5 Migrating from Windows Guide

A dedicated guide for the primary migration path:

1. **Inventory your devices** -- Which ones work with OpenRGB/Hypercolor?
2. **Export your effects** -- Copy HTML effect files from the previous tool's effect directory
3. **Set up Hypercolor** -- Install, configure devices, import effects
4. **Recreate your layout** -- Spatial layout editor walkthrough
5. **Known differences** -- What works differently, what's not supported yet
6. **Getting help** -- Discord, GitHub, community resources

This guide is a strategic asset. Every Windows RGB user who considers Linux is a potential Hypercolor user.

---

## 8. Marketing & Visibility

### 8.1 GitHub README as Landing Page

The README is the most important marketing asset. It should:

1. **Lead with a GIF.** An animated demo of Hypercolor in action -- effects running, LEDs lighting up, spatial layout editor in use. This is the hook.
2. **One-sentence pitch.** "Open-source RGB lighting engine for Linux. HTML effects, 60fps, every device."
3. **Feature grid.** Icons + short descriptions of key capabilities.
4. **Quick start.** `curl | sh` or `cargo install` -- get running in 60 seconds.
5. **Screenshots.** Web UI, TUI, spatial layout editor.
6. **Device compatibility table.** "Does it work with MY stuff?"
7. **Comparison table.** vs. proprietary alternatives, OpenRGB, Artemis, LedFx.
8. **Community links.** Discord, Discussions, contributing guide.

**README anti-patterns to avoid:**
- Wall of text with no visuals
- Badges overload (keep to 5-6 max)
- "Under construction" warnings (ship when ready)
- Feature lists with no demos

### 8.2 Launch Strategy

**Pre-launch (1-2 months before):**
- Teaser posts on r/linux_gaming, r/rust: "Building an RGB engine in Rust"
- Development blog posts on dev.to / personal blog
- Early access Discord for testers
- Collect demo footage of effects running on real hardware

**Launch day:**

| Platform | Content |
|---|---|
| **Reddit (r/linux)** | "I built an open-source RGB lighting engine for Linux" (demo GIF + story) |
| **Reddit (r/rust)** | "Hypercolor: wgpu + Servo for real-time RGB LED control" (technical focus) |
| **Reddit (r/pcmasterrace)** | "RGB on Linux that doesn't suck" (visual focus, demo video) |
| **Hacker News** | "Show HN: Hypercolor -- HTML effects engine for RGB LEDs, written in Rust" |
| **Lobsters** | Technical announcement |
| **Twitter/X** | Thread: problem → solution → demo → link |
| **Mastodon** | Same thread, cross-posted to Fediverse |
| **YouTube** | 5-minute demo video |

**Post-launch (first month):**
- Respond to EVERY GitHub issue and discussion
- Daily presence in Discord
- Follow-up posts on r/homeassistant, r/WLED, r/OpenRGB
- Reach out to Linux YouTube channels (The Linux Experiment, Chris Titus Tech, Brodie Robertson)

### 8.3 Conference Talks

**Target conferences:**

| Conference | When | Talk Angle |
|---|---|---|
| **FOSDEM** | February | "RGB Lighting as a Linux-First Problem" -- embedded systems devroom |
| **Linux Plumbers Conference** | September | "USB HID RGB Device Support in Linux" -- kernel/userspace devroom |
| **SCALE** | March | "Open Source RGB: From Reverse Engineering to Community" |
| **RustConf** | September | "Embedding Servo for Real-Time LED Control" -- Rust ecosystem talk |
| **FOSDEM Rust devroom** | February | "wgpu + Servo: A Dual-Path Render Engine for IoT" |
| **All Things Open** | October | "Building Sustainable Open Source Hardware Projects" |

**Talk formats:**
- 25-minute technical talk with live demo (effects running on stage hardware)
- Lightning talk (5 min) at local Rust meetups
- Workshop (2 hours): "Build an RGB Effect in HTML" at maker events

### 8.4 Blog & Content Strategy

**Hypercolor Blog** (hosted on GitHub Pages or dev.to):

| Cadence | Content Type | Example |
|---|---|---|
| Weekly | Development update | "This Week in Hypercolor: Prism S support lands" |
| Bi-weekly | Technical deep dive | "How Servo Renders HTML Effects at 60fps" |
| Monthly | Community spotlight | "Effect of the Month: Aurora Wave by neonartist42" |
| Quarterly | Roadmap update | "Q3 2027: Where Hypercolor is Heading" |
| As needed | Device guides | "Setting Up Your Corsair iCUE LINK with Hypercolor" |

### 8.5 Social Media Presence

**Channels:**
- **GitHub** (primary) -- All development, all discussions, all releases
- **Discord** -- Community hub, real-time chat
- **YouTube** -- Demo videos, tutorials, conference talks
- **Reddit** -- Community engagement (don't spam, add value)
- **Twitter/X + Mastodon** -- Release announcements, demo clips, community shares
- **Dev.to** -- Technical blog posts

**Tone:** Technical but approachable. Enthusiastic but not hype-driven. Show working software, not roadmap promises.

### 8.6 "Awesome Hypercolor" List

A curated `awesome-hypercolor` repository:

```markdown
# Awesome Hypercolor

## Effects
- [Aurora Wave](link) - Northern lights simulation with audio reactivity
- [Neon Grid](link) - Retrowave grid effect with beat detection
- ...

## Plugins
- [Nanoleaf Backend](link) - Control Nanoleaf panels
- [MIDI Input](link) - Use MIDI controllers for effect parameters
- ...

## Setups
- [Full Tower + WLED Strips](link) - u/gamer42's battlestation with Hypercolor
- [Server Rack Ambient](link) - Headless Hypercolor on a homelab
- ...

## Guides & Tutorials
- [Hypercolor + Home Assistant](link) - Room-wide RGB automation
- [Writing WebGL Effects with Three.js](link) - Advanced effect tutorial
- ...

## Tools
- [Effect Preview Tool](link) - Browser-based effect previewer
- [Layout Exporter](link) - Export spatial layouts to JSON
- ...
```

---

## 9. Sustainability

### 9.1 Funding Model

**Year 1: Bootstrapped**

No funding needed. The project is a passion project by Bliss, developed in personal time. The infrastructure cost is near-zero (GitHub free tier, GitHub Pages, Discord free tier).

**Year 2: Community Funding**

| Source | Platform | Target |
|---|---|---|
| **Individual sponsors** | GitHub Sponsors | $500-2K/month |
| **Community fund** | Open Collective | $200-1K/month |

**Sponsor tiers (GitHub Sponsors):**

| Tier | Amount | Perks |
|---|---|---|
| **Pixel** | $2/mo | Name in SPONSORS.md, sponsor badge on Discord |
| **LED** | $5/mo | Above + early access to beta releases |
| **Strip** | $15/mo | Above + vote on feature priorities |
| **Controller** | $50/mo | Above + monthly 1:1 with maintainer (15 min) |
| **Rig** | $200/mo | Logo in README, priority support, dedicated Discord channel |

**Year 3+: Corporate Sponsors**

| Sponsor Type | What They Get | What They Pay |
|---|---|---|
| **Hardware manufacturers** | "Works with Hypercolor" testing, logo placement | Hardware donations + $500-5K/mo |
| **Linux distros** | Package maintenance priority, integration testing | In-kind (packaging, CI infrastructure) |
| **Cloud/infra companies** | Logo in README, conference sponsorship credit | CI infrastructure + $1-5K/mo |

**What funding pays for:**
- CI infrastructure (build times for Servo are significant)
- Test hardware acquisition (devices for compatibility testing)
- Conference travel
- Part-time maintainer stipend (prevent burnout)
- Domain and hosting costs

### 9.2 Preventing Maintainer Burnout

**This is the #1 existential risk for open-source projects.** A burned-out maintainer is worse than no maintainer -- they become a bottleneck.

**Structural protections:**

1. **Delegate early.** Don't wait until you're overwhelmed. Appoint subsystem maintainers as soon as trustworthy contributors emerge.
2. **Set boundaries.** Hypercolor is not a 24/7 support channel. Response time expectations: issues (1 week), PRs (2 weeks), security (48 hours).
3. **Automate ruthlessly.** CI/CD, release automation, changelog generation, stale issue cleanup. Every hour saved on maintenance is an hour available for creation.
4. **Take breaks.** Announce maintenance windows. "Hypercolor is in maintenance mode for August. Bug fixes only." The community will survive.
5. **Say no.** Not every feature request is valid. Not every issue is a bug. "Won't fix" is a complete sentence (with a brief explanation).

**Burnout early warning signs:**
- Dreading GitHub notifications
- Snapping at contributors
- PRs sitting unreviewed for > 2 weeks
- Skipping releases
- Working on Hypercolor instead of sleeping

**If burnout hits:** Announce a hiatus. Appoint an interim maintainer. The project will still be there when you come back.

### 9.3 Bus Factor Mitigation

**Current bus factor: 1.** This is expected for a new project but must improve.

**Mitigation strategies:**

| Action | Timeline |
|---|---|
| Architecture docs are comprehensive and current | Day 1 (already done) |
| Build system works on a clean clone with `cargo build` | Day 1 |
| CI produces release artifacts automatically | Month 1 |
| Second person has admin access to GitHub org | Month 3 |
| Two people can cut a release | Month 6 |
| Three people can review and merge PRs | Year 1 |
| Succession plan documented | Year 1 |

**Succession plan template:**

> If hyperb1iss becomes permanently unavailable, the project transitions to:
> 1. [Co-maintainer] assumes lead maintainer role
> 2. GitHub org ownership transfers via GitHub's dormant account policy
> 3. The project continues under the same license and governance model
> 4. If no successor exists, the project enters "community maintained" status

### 9.4 Versioning & LTS Strategy

**Pre-1.0 (`0.x.y`):**
- Breaking changes allowed between minor versions
- No LTS promises
- Move fast, learn, iterate
- Clearly communicate: "This is pre-1.0 software. APIs will change."

**Post-1.0 (`x.y.z`):**
- Semantic versioning strictly enforced
- Major version = breaking changes (effect API, plugin WIT, config format)
- Minor version = new features, backward compatible
- Patch version = bug fixes only

**LTS policy (post-1.0):**

| Track | Support Window | Purpose |
|---|---|---|
| **Current** | 6 weeks | Latest features |
| **LTS** | 12 months | Distro packagers, stable deployments |
| **Security** | 24 months | Critical security fixes only |

**Target: reach 1.0 within 18 months of first public release.** The effect API, plugin WIT interface, and configuration format should be stable by then.

---

## 10. Success Metrics

### 10.1 North Star Metric

**Monthly Active Installations (MAI):** The number of unique Hypercolor daemon instances that phone home a lightweight, anonymous, opt-in usage ping (or estimated via package download stats if telemetry is rejected by the community).

### 10.2 Growth Metrics

| Metric | Year 1 Target | Year 3 Target | Year 5 Target |
|---|---|---|---|
| **GitHub stars** | 2,000 | 10,000 | 25,000 |
| **Contributors** (all-time) | 30 | 150 | 500 |
| **Monthly active contributors** | 5-10 | 20-40 | 50-100 |
| **Package downloads** (monthly) | 500 | 5,000 | 20,000 |
| **Discord/Matrix members** | 500 | 3,000 | 10,000 |

### 10.3 Ecosystem Metrics

| Metric | Year 1 Target | Year 3 Target | Year 5 Target |
|---|---|---|---|
| **Marketplace effects** | 30 | 200 | 1,000 |
| **Community plugins** | 0 (Phase 2 not started) | 15 | 50 |
| **Supported devices** (unique models) | 20 | 100 | 300 |
| **Device backends** | 4 (WLED, HID, OpenRGB, Hue) | 8 | 15 |

### 10.4 Quality Metrics

| Metric | Target | How to Measure |
|---|---|---|
| **First PR review time** | < 48 hours | GitHub metrics |
| **Issue response time** | < 1 week | GitHub metrics |
| **CI pass rate** | > 95% | GitHub Actions dashboard |
| **Release cadence adherence** | No missed stable releases | Calendar |
| **Documentation coverage** | 100% of public APIs | Doc coverage tooling |

### 10.5 Community Health Metrics

| Metric | Target | Red Flag |
|---|---|---|
| **New contributor retention** | 30% make a second PR | < 10% = onboarding problem |
| **Issue close rate** | 70% of issues closed within 30 days | < 50% = overwhelmed |
| **Discussion engagement** | 5+ replies per discussion (avg) | < 2 = dead community |
| **Discord daily active users** | 10% of total members | < 5% = ghost town |
| **Contributor diversity** | 20%+ non-male contributors | Track but don't publicize |

### 10.6 Competitive Position Metrics

| Milestone | When | Significance |
|---|---|---|
| **"Mentioned in OpenRGB discussions"** | Month 3 | Awareness in target community |
| **"Included in distro repos"** (AUR, Nix) | Month 6 | Package ecosystem presence |
| **"Recommended on r/linux_gaming"** | Year 1 | Community endorsement |
| **"More GitHub stars than Artemis"** | Year 2 | Overtaking closest competitor |
| **"Hardware manufacturer acknowledges us"** | Year 2 | Industry recognition |
| **"Default RGB recommendation for Linux"** | Year 3 | Category leadership |
| **"Windows RGB users cite us as reason to try Linux"** | Year 5 | Platform-shifting influence |

---

## 11. Timeline & Horizons

### Year 1: Foundation & First Users

**Goal:** Working software that solves a real problem for real users.

```
Q1: Core Engine
├── Ship v0.1 (wgpu renderer + WLED DDP + CLI)
├── GitHub repo public with ARCHITECTURE.md
├── r/rust + r/linux_gaming announcement
├── Discord server live
└── 10 GitHub stars → 100 → 500

Q2: Hardware Expansion
├── v0.2 (PrismRGB backends + OpenRGB bridge + audio)
├── AUR package
├── "Good first issue" program launches
├── First external contributor
└── Lightscript compatibility demo (run HTML effects)

Q3: Web Compatibility
├── v0.3 (Servo integration + Web UI + Lightscript API)
├── Effect marketplace repository created
├── First community effect submission
├── Reach out to Lian Li / PrismRGB
└── First conference talk proposal submitted

Q4: Community Inflection
├── v0.4 (spatial layout editor + TUI + profiles)
├── 20+ marketplace effects
├── 10+ external contributors
├── Blog and YouTube channel launch
└── GitHub Sponsors launched
```

### Year 3: Ecosystem & Growth

**Goal:** Hypercolor is the default RGB answer for Linux users.

```
Year 2:
├── v1.0 release (stable APIs)
├── Wasm plugin system live (Phase 2)
├── 15+ community plugins
├── 200+ marketplace effects
├── Hardware partnerships formalized
├── Conference talks at FOSDEM, RustConf
├── Part-time maintainer funded
└── 10,000+ GitHub stars

Year 3:
├── Home Assistant official integration
├── Mobile-friendly web UI
├── Cross-platform support begins (macOS, then Windows)
├── "Works with Hypercolor" certification program
├── First corporate sponsor
├── Technical Steering Committee formed
└── Distro packages in Fedora COPR, Nix, Debian
```

### Year 5: Platform & Legacy

**Goal:** Hypercolor is the RGB platform, not just a Linux tool.

```
Year 4:
├── Cross-platform (Linux, macOS, Windows)
├── Effect development IDE (VS Code extension or standalone)
├── Plugin marketplace with 50+ plugins
├── Multiple corporate sponsors
├── 25,000+ GitHub stars
└── Annual "HyperConf" community event

Year 5:
├── Hypercolor is to RGB what Home Assistant is to smart homes
├── Hardware manufacturers ship Hypercolor support
├── Effect authoring is a creative medium (artists, not just developers)
├── Self-sustaining community (not dependent on any single person)
└── "How we lit up Linux" retrospective blog post
```

### The Endgame

Hypercolor succeeds when the question "How do I control RGB on Linux?" has one answer: **Hypercolor**.

Not because it's the only option, but because it's the obvious one. The way Home Assistant is the obvious answer for open-source home automation. The way VS Code became the obvious editor. The way Blender became the obvious 3D tool.

The path:
1. **Solve the pain** (RGB on Linux sucks → Hypercolor fixes it)
2. **Build the community** (contributors, effect authors, plugin developers)
3. **Win the ecosystem** (hardware partnerships, distro integration, smart home)
4. **Transcend the platform** (Linux-first → everywhere)

Hypercolor isn't just software. It's the argument that open source can do creative, visual, hardware-integrated things better than closed-source alternatives. And it's built by someone who has proven that argument before.

---

*Design document authored for the Hypercolor project, March 2026.*
