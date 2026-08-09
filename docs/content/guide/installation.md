+++
title = "Installation"
description = "Install Hypercolor on Linux, Windows, and macOS. Prebuilt packages and one-line installers first; source build is a labeled developer section."
weight = 30
template = "page.html"
+++

Most users should install a prebuilt package; no Rust toolchain required. Source builds are for contributors and platform porters.

Not sure which path fits? Read [Choose your install](@/guide/choose-your-install.md) first.

## Linux: prebuilt installer

The fastest path on any Linux distribution. The script downloads a release
tarball from GitHub, verifies its SHA256 checksum, installs the daemon and CLI
to `~/.local/bin`, sets up a systemd user service, and prompts before applying
udev rules and `i2c-dev` setup for USB and SMBus device access.

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash
```

The installer is idempotent: re-running it upgrades an existing install. Pin any tagged release with `--version`:

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --version v0.2.1
```

To install to a different prefix instead of `~/.local`:

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | HYPERCOLOR_INSTALL_PREFIX=$HOME/apps/hypercolor bash
```

`HYPERCOLOR_INSTALL_PREFIX` moves the whole install root, and
`HYPERCOLOR_INSTALL_DIR` overrides just the binary directory (default:
`<prefix>/bin`). A system-wide prefix such as `/opt/hypercolor` works too, but
the script then needs root privileges to write there.

{% callout(type="warning") %}
If you installed the system hooks, **re-plug your USB devices or log out and
back in** so the new udev rules take effect. If your devices are still not
detected, see [Devices not found](@/troubleshooting/devices-not-found.md).
{% end %}

### Debian and Ubuntu (.deb)

Each release ships a `.deb` package for amd64. It installs the daemon, CLI,
systemd user service, udev rules, and shell completions through `apt`:

```bash
sudo apt install ./hypercolor_<version>_amd64.deb
```

Remove it later with `sudo apt remove hypercolor`.

### Arch Linux (AUR)

The `hypercolor-bin` AUR package updates automatically on every tagged release:

```bash
yay -S hypercolor-bin
```

The PKGBUILD installs binaries, the systemd user service, shell completions, and udev rules automatically as part of the package install hooks.

---

## Linux: udev rules (USB and input device access)

USB and input device access on Linux requires udev rules. The prebuilt
installer prompts for these hooks, and the `.deb` and AUR packages handle them
automatically. If you are installing manually or from source:

```bash
just udev-install
```

This copies both rules files (`udev/99-hypercolor.rules` for USB and hidraw
access, `udev/70-hypercolor-input.rules` for input capture) to
`/etc/udev/rules.d/`, reloads udev, and triggers a rescan of the `hidraw` and
`usb` subsystems. You will need to re-plug connected devices or log out and
back in for group membership changes to propagate.

---

## Windows

Download the NSIS installer (`Hypercolor_<version>_x64-setup.exe`) from the
[download page](@/download.md). The install is per-machine and asks for
administrator elevation (UAC). In that one elevated pass the installer:

- Bundles `hypercolor-daemon.exe` and the `hypercolor-app` desktop shell
- Registers the app for autostart at login
- Runs hardware setup: installs the bundled PawnIO SMBus modules and registers
  the `HypercolorSmBus` broker service (motherboard and DRAM RGB)
- Adds Windows Firewall rules so mDNS discovery works without a first-run prompt
- Creates Start menu and Desktop shortcuts

Run the installer and launch Hypercolor from the Start menu. The app supervises the daemon automatically, so there is no separate daemon window to manage.

{% callout(type="info") %}
Windows builds are currently unsigned, so SmartScreen may warn when you run the
installer. Choose "More info" and then "Run anyway" to continue.
{% end %}

If hardware setup did not complete during install (the installer notes this in
its details log), USB and network lighting still work. Re-run the SMBus setup
later from Settings → Device Discovery → Hardware Support.

---

## macOS

Download the DMG from the [download page](@/download.md). Open the DMG, drag
Hypercolor to Applications, and launch it. The app registers a LaunchAgent for
autostart and supervises the daemon; no terminal setup is required.

{% callout(type="warning") %}
Current builds are ad-hoc signed but not notarized, so Gatekeeper will block
the app on first launch. Right-click the app and choose **Open** to confirm.
{% end %}

{% callout(type="info") %}
macOS hardware support covers USB-HID and network devices (Hue, Nanoleaf, WLED, Govee). SMBus/motherboard RGB is Linux and Windows only.
{% end %}

Homebrew users can install the desktop app as a cask
(`brew install --cask hyperb1iss/tap/hypercolor-app`) or the daemon and CLI as
a formula (`brew install hyperb1iss/tap/hypercolor`, with `brew services`
support). Both update automatically on every tagged release.

---

## The desktop app and autostart

On all platforms, Hypercolor ships a unified desktop app (`hypercolor-app`) built on Tauri. When you launch it:

1. The app checks if a daemon is already running on `127.0.0.1:9420`. If so, it connects to it.
2. On Linux, it checks for an enabled systemd user service (`hypercolor.service`) and defers to it.
3. If no daemon is found, the app spawns one as a supervised child process with a watchdog that restarts it on crash.
4. The tray icon appears, and the main window opens (or the app starts minimized if launched with `--minimized`).

Autostart is managed by the app's autostart plugin. On Linux it creates a `~/.config/autostart/` entry; on macOS it registers a LaunchAgent; on Windows it writes a Run key in the current user's registry. Toggle it from the tray menu or from within the app's Settings page.

The app window is 1200×800 by default, with a minimum of 800×500. Close clicks hide the window rather than quit; Hypercolor stays in the tray. To fully quit, use the tray menu.

---

## Linux: systemd user service

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

On Linux this wraps `systemctl --user`: it is a **user** service, not a system service. Never use `sudo systemctl` to manage it.

The unit file lives at `~/.config/systemd/user/hypercolor.service` and uses `%h/.local/bin/hypercolor-daemon` as the executable path.

---

## macOS: LaunchAgent

The macOS app install registers a LaunchAgent (`tech.hyperbliss.hypercolor`) in `~/Library/LaunchAgents`. The same `hypercolor service` subcommands work on macOS, wrapping `launchctl`.

---

## Verify the daemon is running

Regardless of install method, confirm the daemon is up:

```bash
curl http://localhost:9420/health
```

A `200 OK` response means the daemon is healthy and accepting connections. The web UI is available at `http://localhost:9420` in your browser.

---

## Developer install: build from source

This section is for contributors and platform porters. Ordinary users do not need to build from source.

### Prerequisites

- **Rust 1.94+** (Edition 2024). Install via [rustup](https://rustup.rs/).
- **`just`**, the task runner. `cargo install just` or your distro's package manager.
- **Bun**, required for the web UI and TypeScript SDK. `curl -fsSL https://bun.sh/install | bash`.
- **Platform libraries**: see the distribution-specific lists below.

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
just verify          # fmt + lint + test; run this before committing
```

### Install from source

After building, install the daemon and CLI to `~/.local/bin`, the web UI assets,
the systemd user service, and, by default, udev rules plus `i2c-dev` setup:

```bash
just install
```

Pass `--skip-system-hooks` if you want to skip the sudo-backed udev and SMBus
setup:

```bash
just install -- --skip-system-hooks
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
