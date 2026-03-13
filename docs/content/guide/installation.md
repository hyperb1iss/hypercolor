+++
title = "Installation"
description = "Build Hypercolor from source and set up system dependencies"
weight = 1
template = "page.html"
+++

## Prerequisites

Hypercolor requires **Rust 1.85+** (Edition 2024) and a handful of system libraries for USB/HID device communication and audio capture.

### System Dependencies

**Debian / Ubuntu:**

```bash
sudo apt install build-essential pkg-config \
  libudev-dev libusb-1.0-0-dev libhidapi-dev \
  libasound2-dev
```

**Fedora:**

```bash
sudo dnf install gcc pkg-config \
  systemd-devel libusb1-devel hidapi-devel \
  alsa-lib-devel
```

**Arch Linux:**

```bash
sudo pacman -S base-devel pkgconf \
  libusb hidapi \
  alsa-lib
```

### Rust Toolchain

If you don't have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Hypercolor also needs the WASM target for the web UI:

```bash
rustup target add wasm32-unknown-unknown
```

## Building from Source

Clone the repository and build:

```bash
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor
```

### Using the Justfile (Recommended)

Hypercolor uses [just](https://github.com/casey/just) as its command runner. Install it if you haven't:

```bash
cargo install just
```

Then build:

```bash
just build          # Debug build
just release        # Release build (optimized)
just verify         # Format + lint + test — run this after changes
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

## Full Setup

For a one-shot setup that installs Rust targets, Bun (for the SDK), and all frontend dependencies:

```bash
just setup
```

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
