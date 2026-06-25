+++
title = "Changelog & versions"
description = "What's in each Hypercolor release, how versioning works, and how to check the version you have installed."
weight = 170
+++

Hypercolor is pre-1.0 software under active development. This page is the user-facing home for release notes, version numbering conventions, and SDK compatibility status.

## How to check your installed version

```bash
hypercolor --version
```

The daemon prints its version on startup and in the status response:

```bash
hypercolor status
```

The REST API surfaces it in every response envelope under `meta.api_version`.

## Version numbering

Hypercolor uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html): `MAJOR.MINOR.PATCH`.

Because the project is pre-1.0, the minor version (`0.x`) carries breaking-change weight. A bump from `0.1` to `0.2` may change the REST API envelope, the config file schema, or the effect SDK surface without a deprecation window. Patch releases (`0.1.x`) are safe to apply: they are bug fixes and security hardening only.

The TypeScript effect SDK (`sdk/`) and the Python client are **pre-release** and are not yet published to npm or PyPI. Until they are, consume them directly from source. Breaking SDK changes will be called out explicitly in the release notes below.

## Releases

### 0.1.0 (unreleased)

This is the first public release. Everything below shipped as a cohesive whole; there are no prior tagged versions to compare against.

**Core engine**

- Daemon with REST, WebSocket, and MCP control surfaces on `:9420`.
- Servo HTML effect renderer for Canvas API and WebGL2 effects. GLSL shaders run as WebGL2 fragments through Servo. There is no separate GPU/wgpu shader execution path for effects in this release.
- SparkleFlinger frame compositor: multiple producers (effects, overlays, screen captures) composited into one canonical 640×480 RGBA canvas each tick. Canvas resolution is configurable.
- Adaptive FPS controller: shifts between 10/20/30/45/60 fps tiers based on render budget. Default target is **30 fps**, not 60.

**Hardware support**

- USB/HID and SMBus driver families: Razer, Lian Li (ENE/TL), ASUS Aura, Corsair (Lighting Node/LINK/LCD), Dygma, Ableton Push 2, QMK, PrismRGB, Nollie.
- Network backends: Philips Hue (DTLS Entertainment API), Nanoleaf (HTTP + UDP external control), WLED (DDP / E1.31 sACN), Govee (LAN UDP).
- 179 devices supported across 12 driver families at launch. 414 devices total tracked in the device database.

**Interfaces**

- Leptos web UI served by the daemon on `:9420` (no separate process required for production installs).
- Ratatui terminal UI, launched via `hypercolor tui`.
- `hypercolor` CLI with tab-completion for bash, zsh, and fish.
- System tray applet.
- Unified desktop app shell (`hypercolor-app`) that supervises the daemon, owns the tray, and provides single-instance management.

**Effect SDK**

- TypeScript SDK for authoring HTML Canvas effects. Pre-release; installed from source via `bun install` in `sdk/`.
- 11 native built-in Rust effects registered in `crates/hypercolor-core/src/effect/builtin/` (solid color, gradient, rainbow, breathing, audio pulse, color wave, color zones, screen cast, media player, web viewport, calibration).
- 44 built-in HTML effects in the SDK pack, spanning ambient, audio-reactive, shader, and generative styles. These render through Servo and are separate from the native Rust effects above.

**MCP server**

- 16 tools, 5 resources, and 3 prompts exposed via the MCP server at `/mcp`.
- Disabled by default in the config (`[mcp] enabled = false`). See the [MCP server reference](@/api/mcp.md) for how to enable it.

**Packaging**

- Linux release tarballs with systemd user service, udev rules, shell completions, bundled UI assets, and bundled effects.
- AUR PKGBUILD (`hypercolor-bin`). Checksums populated at release time.
- Homebrew cask (`brew install --cask hyperb1iss/tap/hypercolor-app`).
- macOS and Windows installers are experimental at launch. Their runtime and installer gates do not yet match Linux.

**Security**

- Fail-closed daemon startup: a non-loopback bind without authentication enabled is a hard startup error.
- Credential store seed and encrypted payload permission hardening.

**Known limitations at launch**

- SDK packages are source-only until published to npm. Use the `file:` path specifier in your `package.json`.
- The Python client is source-only until published to PyPI.
- macOS and Windows are build and test paths, not supported runtime targets for end users.
- No GPU wgpu shader lane for effects. `EffectSource::Shader` bails; GLSL effects run via WebGL2 in Servo. Native wgpu rendering is future work.

---

## Planned: post-0.1.0 patch series

The following are in progress or queued for the patch series after the initial release. This list is not a commitment.

- AUR automated checksum update job (`update-aur` CI, currently a manual release-time step).
- Unified end-user installer consolidating `install-release.sh` and `get-hypercolor.sh` (only one is published at `install.hypercolor.dev`).
- Govee Cloud API path: LAN UDP ships at launch; the cloud fallback is in the driver codebase but gated pending access credential handling.
- Additional driver families from the researched backlog.

---

## Release channels

| Channel | How to get it | Stability |
|---|---|---|
| Stable release | GitHub Releases tarball, AUR, Homebrew | Safe for daily use |
| `main` branch | Build from source (`just build`) | Pre-release; may break |
| Feature branches | Build from source | Development only |

The `main` branch is the working development branch. Tagged releases are the stable surface. There is no separate nightly or beta channel at this time.

---

## SDK compatibility

The TypeScript SDK version tracks the workspace version in `sdk/package.json`. Breaking changes to the SDK effect API, control schema, or canvas bridge will increment the minor version and be called out in the release notes above.

Until the SDK is published to npm, there are no semver guarantees between commits on `main`. Lock to a git revision or tag when consuming the SDK in your own effects.

---

{% callout(type="info") %}
The canonical machine-readable changelog lives in `CHANGELOG.md` at the repository root and follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format. This page distills it into user-facing narrative. When in doubt, the file is the authority.
{% end %}
