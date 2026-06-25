+++
title = "Installation"
description = "Install Hypercolor on Linux, Windows, and macOS. Prebuilt packages and one-line installers first; source build is a labeled developer section."
weight = 30
template = "page.html"
+++

Most users should install a prebuilt package — no Rust toolchain required. Source builds are for contributors and platform porters.

Not sure which path fits? Read [Choose your install](@/guide/choose-your-install.md) first.

## Linux — prebuilt installer

The fastest path on any Linux distribution. The script downloads a signed release tarball from GitHub, installs the daemon and CLI to `~/.local/bin`, sets up a systemd user service, and applies udev rules for USB device access.

```bash
curl -fsSL https://install.hypercolor.dev | bash
```

The installer is idempotent: re-running it upgrades an existing install. To pin a specific version:

```bash
HYPERCOLOR_VERSION=0.1.0 curl -fsSL https://install.hypercolor.dev | bash
```

To install to a different prefix instead of `~/.local`:

```bash
HYPERCOLOR_INSTALL_PREFIX=/opt/hypercolor curl -fsSL https://install.hypercolor.dev | bash
```

{% callout(type="warning") %}
After installation, **re-plug your USB devices or log out and back in** so the new udev rules take effect. If your devices are still not detected, see [Devices not found](@/troubleshooting/devices-not-found.md).
{% end %}

{% callout(type="info") %}
AppImage and Flatpak packages do not automatically apply udev rules. If you use one of those formats, run `just udev-install` from a repo checkout or copy `udev/99-hypercolor.rules` to `/etc/udev/rules.d/` manually and reload with `sudo udevadm control --reload-rules`.
{% end %}

### Arch Linux (AUR)

An AUR package (`hypercolor-bin`) will be available at v0.1.0 release. Once published:

```bash
yay -S hypercolor-bin
```

The PKGBUILD installs binaries, the systemd user service, shell completions, and udev rules automatically as part of the package install hooks.

---

## Linux — udev rules (USB device access)

USB device access on Linux requires udev rules. The prebuilt installer and AUR package handle this automatically. If you are installing manually or from source:

```bash
just udev-install
```

This copies `udev/99-hypercolor.rules` to `/etc/udev/rules.d/`, reloads udev, and triggers a rescan of the `hidraw`, `usb`, `tty`, and `i2c-dev` subsystems. You will need to re-plug connected devices or log out and back in for group membership changes to propagate.

---

## Windows

Download the NSIS installer from [hypercolor.lighting/download](https://hypercolor.lighting/download). The installer:

- Bundles `hypercolor-daemon.exe` and the `hypercolor-app` desktop shell
- Registers the app for autostart at login
- Installs the PawnIO helper for SMBus access (motherboard and DRAM RGB)
- Creates Start menu and Desktop shortcuts

Run the installer and launch Hypercolor from the Start menu. The app supervises the daemon automatically, so there is no separate daemon window to manage.

{% callout(type="info") %}
For SMBus devices (ASUS Aura DRAM, some Gigabyte and MSI motherboards), the first-run wizard will prompt you to install the PawnIO kernel helper. Click "Install SMBus support" when prompted, and it runs an elevated helper that handles the driver install.
{% end %}

---

## macOS

Download the signed DMG from [hypercolor.lighting/download](https://hypercolor.lighting/download). Open the DMG, drag Hypercolor to Applications, and launch it. The app registers a LaunchAgent for autostart and supervises the daemon — no terminal setup required.

{% callout(type="info") %}
macOS hardware support covers USB-HID and network devices (Hue, Nanoleaf, WLED, Govee). SMBus/motherboard RGB is Linux and Windows only.
{% end %}

A Homebrew cask (`brew install --cask hyperb1iss/tap/hypercolor-app`) will be published at v0.1.0 release.

---

## The desktop app and autostart

On all platforms, Hypercolor ships a unified desktop app (`hypercolor-app`) built on Tauri. When you launch it:

1. The app checks if a daemon is already running on `127.0.0.1:9420`. If so, it connects to it.
2. On Linux, it checks for an enabled systemd user service (`hypercolor.service`) and defers to it.
3. If no daemon is found, the app spawns one as a supervised child process with a watchdog that restarts it on crash.
4. The tray icon appears, and the main window opens (or the app starts minimized if launched with `--minimized`).

Autostart is managed by the app's autostart plugin. On Linux it creates a `~/.config/autostart/` entry; on macOS it registers a LaunchAgent. Toggle it from the tray menu or from within the app's Settings page.

The app window is 1200×800 by default, with a minimum of 800×500. Close clicks hide the window rather than quit — Hypercolor stays in the tray. To fully quit, use the tray menu.

---

## Linux — systemd user service

The prebuilt installer and `just install` both install a systemd user service. Manage it with the CLI:

```bash
hypercolor service enable     # enable autostart on login
hypercolor service start      # start the daemon now
hypercolor service stop       # stop it
hypercolor service restart    # restart
hypercolor service status     # check current state
hypercolor service logs       # last 50 lines
hypercolor service logs --follow   # live tail
```

On Linux this wraps `systemctl --user` — it is a **user** service, not a system service. Never use `sudo systemctl` to manage it.

The unit file lives at `~/.config/systemd/user/hypercolor.service` and uses `%h/.local/bin/hypercolor-daemon` as the executable path.

---

## macOS — LaunchAgent

The macOS app install registers a LaunchAgent (`tech.hyperbliss.hypercolor`) in `~/Library/LaunchAgents`. The same `hypercolor service` subcommands work on macOS, wrapping `launchctl`.

---

## Verify the daemon is running

Regardless of install method, confirm the daemon is up:

```bash
curl http://localhost:9420/health
```

A `200 OK` response means the daemon is healthy and accepting connections. The web UI is available at `http://localhost:9420` in your browser.

---

## Developer install — build from source

This section is for contributors and platform porters. Ordinary users do not need to build from source.

### Prerequisites

- **Rust 1.94+** (Edition 2024). Install via [rustup](https://rustup.rs/).
- **`just`** — the task runner. `cargo install just` or your distro's package manager.
- **Bun** — required for the web UI and TypeScript SDK. `curl -fsSL https://bun.sh/install | bash`.
- **Platform libraries** — see the distribution-specific lists below.

### Bootstrap (recommended)

```bash
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor
just setup
```

`just setup` installs system packages, the Rust toolchain, the WASM target, cargo tools (`trunk`, `cargo-deny`, `sccache`), and frontend dependencies. It is idempotent: re-running only installs what is missing.

Setup flags:

```bash
just setup -- -y              # non-interactive (no sudo prompts)
just setup -- --minimal       # Rust + wasm target only
just setup -- --no-system     # skip system package install
just setup -- --with-servo    # include Servo HTML renderer build deps
```

On Windows the same recipe dispatches to `scripts/setup.ps1`. Use PowerShell-style flags: `-Yes`, `-Minimal`, `-NoSystem`, `-WithServo`.

### System libraries

**Debian / Ubuntu:**

```bash
sudo apt install build-essential pkg-config cmake nasm \
  libudev-dev libusb-1.0-0-dev libhidapi-dev \
  libasound2-dev libpulse-dev libpipewire-0.3-dev \
  libxdo-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev libssl-dev \
  clang lld
```

**Fedora:**

```bash
sudo dnf install gcc gcc-c++ pkg-config cmake nasm \
  systemd-devel libusb1-devel hidapi-devel \
  alsa-lib-devel pulseaudio-libs-devel pipewire-devel \
  libxdo-devel gtk3-devel webkit2gtk4.1-devel \
  libappindicator-gtk3-devel librsvg2-devel openssl-devel \
  clang lld
```

**Arch Linux:**

```bash
sudo pacman -S base-devel pkgconf cmake nasm \
  libusb hidapi alsa-lib libpulse pipewire \
  xdotool gtk3 webkit2gtk-4.1 \
  libappindicator-gtk3 librsvg openssl \
  clang lld
```

**macOS:**

```bash
xcode-select --install
brew install hidapi pkg-config cmake nasm
```

**Windows:** Install [Visual Studio 2022 Build Tools](https://visualstudio.microsoft.com/downloads/) with the "Desktop development with C++" workload.

### WASM target

Required for the web UI:

```bash
rustup target add wasm32-unknown-unknown
# or use the shortcut:
just setup-wasm
```

### Additional dev tools

```bash
cargo install --locked trunk cargo-deny    # required
cargo install --locked sccache             # optional; speeds rebuilds
```

### Frontend dependencies

```bash
cd crates/hypercolor-ui && bun install --frozen-lockfile   # Tailwind v4
cd ../../sdk && bun install --frozen-lockfile               # TypeScript SDK
```

### Build

```bash
just build           # debug build
just build-preview   # preview profile (optimized, fast compile)
just release         # full release bundle in dist/
just check           # type-check only, no artifact
just verify          # fmt + lint + test — run this before committing
```

### Install from source

After building, install the daemon and CLI to `~/.local/bin`, the web UI assets, the systemd user service, and udev rules:

```bash
just install
```

Then apply USB device permissions:

```bash
just udev-install
```

### Run the desktop app from source

```bash
just app
```

This builds the daemon and the Tauri app at the `preview` profile and launches `hypercolor-app`. The app supervisor handles starting the daemon.

To run the daemon directly without the app shell:

```bash
just daemon
```

The daemon starts on `127.0.0.1:9420` by default with debug logging enabled.

---

## What's next

With Hypercolor running, head to [First launch](@/guide/first-launch.md) to walk through the welcome wizard and connect your first device, or jump straight to the [Quick start](@/guide/quick-start.md) if you already know your way around.
