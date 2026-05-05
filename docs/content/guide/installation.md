+++
title = "Installation"
description = "Build Hypercolor from source and set up system dependencies"
weight = 1
template = "page.html"
+++

## Prerequisites

Hypercolor needs **Rust 1.85+** (Edition 2024) and a few system libraries to talk to your hardware and capture audio. The fast path: clone the repo, install [just](https://github.com/casey/just), run `just setup`.

## Quick Start (Recommended)

```bash
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor

# Install just if you don't have it
cargo install just     # or: brew install just / winget install Casey.Just

# Bootstrap everything: system packages, Rust toolchain, cargo tools, bun, frontend deps
just setup
```

`just setup` is **idempotent** — re-running it only installs what's missing. It detects your platform (Debian/Ubuntu, Fedora, Arch, macOS, Windows) and uses the right package manager.

### Setup Flags

```bash
just setup -- -y              # don't prompt for sudo / package installs
just setup -- --minimal       # rust + wasm target only (skip system pkgs, frontend, etc.)
just setup -- --no-system     # skip system packages (no sudo prompts)
just setup -- --with-servo    # include extra deps for the Servo HTML renderer
```

On Windows the same recipe dispatches to `scripts/setup.ps1` — flags are PowerShell-style: `-Yes`, `-Minimal`, `-NoSystem`, `-WithServo`.

## Manual Setup

If you'd rather install pieces individually, here's what `just setup` does for you.

### System Dependencies

**Debian / Ubuntu:**

```bash
sudo apt install build-essential pkg-config cmake nasm \
  libudev-dev libusb-1.0-0-dev libhidapi-dev \
  libasound2-dev libpulse-dev libpipewire-0.3-dev \
  clang lld
```

**Fedora:**

```bash
sudo dnf install gcc gcc-c++ pkg-config cmake nasm \
  systemd-devel libusb1-devel hidapi-devel \
  alsa-lib-devel pulseaudio-libs-devel pipewire-devel \
  clang lld
```

**Arch Linux:**

```bash
sudo pacman -S base-devel pkgconf cmake nasm \
  libusb hidapi alsa-lib libpulse pipewire \
  clang lld
```

**macOS:**

```bash
xcode-select --install
brew install hidapi pkg-config cmake nasm
```

**Windows:** install [Visual Studio 2022 Build Tools](https://visualstudio.microsoft.com/downloads/) with the "Desktop development with C++" workload.

### Rust Toolchain

If you don't have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh    # Linux/macOS
winget install Rustlang.Rustup                                    # Windows
```

Hypercolor also needs the WASM target for the web UI:

```bash
rustup target add wasm32-unknown-unknown
# or, as a shortcut:
just setup-wasm
```

### Dev Tools

```bash
cargo install --locked trunk cargo-deny    # required
cargo install --locked sccache             # optional but recommended
```

### Frontend Dependencies

```bash
cd crates/hypercolor-ui && npm ci          # Tailwind v4
cd ../../sdk && bun install                # SDK
cd ../e2e && npm ci                        # Playwright e2e (optional)
```

## Building from Source

```bash
just build          # Debug build
just release        # Full release bundle in dist/
just release-bin    # Release binaries only
just check          # Type-check without building
just verify         # Format check + lint + test — run this after changes
```

### Direct Cargo Commands

```bash
cargo build --workspace              # Debug build
cargo build --workspace --release    # Release build
cargo test --workspace               # Run all tests
cargo clippy --workspace --all-targets -- -D warnings   # Lint
```

## USB Device Access

Hypercolor needs permission to access USB HID devices. Install the udev rules:

```bash
just udev-install
```

This copies the rules to `/etc/udev/rules.d/` and triggers a reload. You may need to re-plug your devices or log out and back in for group membership changes to take effect.

## Running the Daemon

Start the daemon in preview mode with debug logging:

```bash
just daemon
```

The daemon starts on port **9420** by default. Verify it's running:

```bash
curl http://localhost:9420/health
```

You should get a `200 OK` response.

{% callout(type="tip", title="Preview profile") %}
`just daemon` uses the `preview` build profile — optimized for runtime performance while keeping reasonable compile times. For maximum performance, use `just daemon-release`.
{% end %}

## What's Next

With the daemon running, head to the [Quick Start](@/guide/quick-start.md) to connect a device and apply your first effect.
