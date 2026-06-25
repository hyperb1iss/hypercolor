+++
title = "Choose your install"
description = "Pick the right Hypercolor install path for your OS and skill level — prebuilt, packaged, or source."
weight = 20
+++

# Choose your install

Not every install path is right for every person. This page routes you to the correct one before you spend time on the wrong steps.

{% callout(type="info") %}
Linux is the fully supported install platform for v0.1.0. Windows and macOS ship
desktop packages with their own hardware scope. Source builds on macOS are
useful for development but not required by end users.
{% end %}

## Decide in 30 seconds

| I am... | My OS | Go here |
|---|---|---|
| A regular user who wants things to work | Linux | [Prebuilt one-liner](#prebuilt-linux) |
| A regular user who wants things to work | Windows | [Desktop installer](#windows-installer) |
| A regular user who wants things to work | macOS | [DMG or Homebrew Cask](#macos-dmg) |
| An Arch Linux user | Linux | [AUR package](#aur) |
| A developer or contributor | Any | [Build from source](#build-from-source) |

If you are not sure whether you are a developer, you are not a developer. Start with the prebuilt path.

---

## Prebuilt one-liner (Linux) {#prebuilt-linux}

The fastest path on Linux. Downloads the latest release binaries from GitHub,
installs them to `~/.local/bin`, sets up the systemd user service, and prompts
before installing udev rules for USB device access or persisting the `i2c-dev`
kernel module for SMBus RGB hardware.

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash
```

No Rust toolchain required. The script is idempotent, so it is safe to re-run to upgrade.

**Supported platforms:** Linux x86_64 and Linux aarch64.

### Installer options

Pass flags after `--` to control the install:

```bash
# Install a specific version
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --version v0.1.0

# Skip service setup (useful for custom init systems)
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --no-service

# Remove Hypercolor
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --uninstall
```

You can also set `HYPERCOLOR_INSTALL_PREFIX` to override the install root (default: `~/.local`).

### What the installer does

1. Detects your architecture and downloads the matching release tarball from GitHub.
2. Verifies the SHA256 checksum before extracting.
3. Installs `hypercolor-daemon` and `hypercolor` to `~/.local/bin`.
4. Installs the systemd **user** service to `~/.config/systemd/user/hypercolor.service` and enables it.
5. Prompts to copy `udev/99-hypercolor.rules` to `/etc/udev/rules.d/` (requires `sudo`) and reloads udev if approved.
6. When system hooks are approved, loads `i2c-dev` immediately and persists it via `/etc/modules-load.d/i2c-dev.conf` (requires `sudo`).

After the installer finishes, see [First launch](@/guide/first-launch.md) to open the UI for the first time.

---

## Windows installer {#windows-installer}

Download the installer from the [download page](@/download.md) and run it.
Per-user install — no UAC prompt unless you opt into SMBus/RAM RGB hardware
support, which installs the [PawnIO](https://github.com/namazso/PawnIO) kernel
driver via a one-click flow. Tested on Windows 10 22H2 and Windows 11 23H2/24H2,
x64.

USB-HID lighting (Razer, Corsair, Lian Li, and others) and network devices (Hue, WLED, Nanoleaf, Govee) work out of the box. Motherboard and DRAM SMBus lighting (ASUS Aura, MSI, Gigabyte) requires the optional PawnIO install — Hypercolor prompts you only if compatible hardware is detected.

---

## macOS {#macos-dmg}

### DMG

Download `Hypercolor-<version>-arm64.dmg` (Apple Silicon) or `-x86_64.dmg`
(Intel) from the [download page](@/download.md), drag the app into
`/Applications`, and launch. Minimum macOS 11 (Big Sur).

{% callout(type="warning") %}
Current builds are unsigned while the Developer ID and notarization rollout completes. Gatekeeper will block the app on first launch. Right-click the app and choose **Open** to confirm.
{% end %}

On first run, Hypercolor will prompt for Microphone and Screen Recording permissions only if you enable audio- or screen-reactive effects.

### Homebrew Cask {#homebrew}

Once the tap is published at v0.1.0:

```bash
brew install --cask hyperb1iss/tap/hypercolor-app
```

Check the [releases page](https://github.com/hyperb1iss/hypercolor/releases) for the latest tap availability status. The formula is auto-updated by CI on each tagged release.

---

## AUR (Arch Linux) {#aur}

A `hypercolor-bin` PKGBUILD is ready in the repo and will be published to the AUR at v0.1.0. Once available:

```bash
yay -S hypercolor-bin
```

The AUR package installs the prebuilt binaries, sets up the systemd user service, and places udev rules in the correct system paths. Check the [releases page](https://github.com/hyperb1iss/hypercolor/releases) for the current status before trying `yay`.

---

## Build from source {#build-from-source}

Building from source is the right path for contributors, packagers, and people who need a custom build (e.g., with Servo HTML effect rendering enabled). It is not necessary for end users.

You need:
- Rust 1.94+ (Edition 2024) — install via `rustup`
- `just` — the task runner
- System libraries for your OS (USB, audio, GTK, WebKit)

```bash
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor
cargo install just
just setup
just install
```

`just setup` bootstraps the Rust toolchain, system packages, Bun, Trunk, cargo-deny, and frontend dependencies. It detects your Linux distribution (Debian/Ubuntu, Fedora, Arch) and uses the right package manager. It is idempotent, so it is safe to re-run.

`just install` builds the daemon, CLI, and web UI at release profile, installs binaries to `~/.local/bin`, enables the systemd user service, installs udev rules, and persists `i2c-dev`.

Full system dependency lists and optional flags (`--minimal`, `--no-system`, `--with-servo`) are in the [Installation reference](@/guide/installation.md).

{% callout(type="tip") %}
The `just setup` and `just install` path uses the same install layout as the prebuilt one-liner. Both land in `~/.local` with the same systemd unit and udev rules; the only difference is that source builds compile everything on your machine.
{% end %}

---

## After installing

Whichever path you took, your next stop is [First launch](@/guide/first-launch.md), which walks through the first-run wizard, device discovery, and opening the web UI for the first time.

If a USB device does not appear after install, the most common cause is udev rules not applied yet. Re-plug the device or log out and back in. The [Quick start](@/guide/quick-start.md) covers this and has a one-command health check.
