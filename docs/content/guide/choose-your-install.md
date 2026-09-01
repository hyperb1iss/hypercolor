+++
title = "Choose your install"
description = "Pick the right Hypercolor install path for your OS and skill level: prebuilt, packaged, or source."
weight = 20
+++

Not every install path is right for every person. This page routes you to the correct one before you spend time on the wrong steps.

{% <callout type="info"> %}
Linux, Windows, and macOS are all supported install platforms. Linux
additionally gets udev rules and a systemd user service. macOS supports screen
capture and native host input, but it has no SMBus motherboard/DRAM RGB path.
Screen-lock and suspend behavior works on all three platforms; the idle-dimming
and laptop-lid settings are accepted but nothing emits those events yet.
{% </callout> %}

## Decide in 30 seconds

| I am... | My OS | Go here |
|---|---|---|
| A regular user who wants things to work | Linux | [Prebuilt one-liner](#prebuilt-linux) |
| A regular user who wants things to work | Windows | [Desktop installer](#windows-installer) |
| A regular user who wants things to work | macOS | [DMG or Homebrew](#macos-dmg) |
| An Arch Linux user | Linux | [AUR package](#aur) |
| A developer or contributor | Any | [Build from source](#build-from-source) |

If you are not sure whether you are a developer, you are not a developer. Start with the prebuilt path.

---

## Prebuilt one-liner (Linux) {% raw %}{#prebuilt-linux}{% endraw %}

The fastest path on Linux. Downloads the latest release binaries from GitHub,
verifies them, installs them to `~/.local/bin`, and sets up the systemd user
service. It never asks for `sudo`, so the udev rules and the `i2c-dev` kernel
module are left to you.

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash
```

No Rust toolchain required. The script is idempotent, so it is safe to re-run to upgrade.

**Supported platforms:** Linux x86_64 and aarch64, and macOS on both Apple Silicon (arm64) and Intel (x86_64). The [DMG](#macos-dmg) is the friendlier macOS path if you would rather not use a shell one-liner.

### Installer options

Pass flags after `--` to control the install:

```bash
# Pin any tagged release
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --version v0.4.0

# Skip service setup (useful for custom init systems)
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --no-service

# Skip the uninstall confirmation prompt
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --uninstall --yes

# Remove Hypercolor
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --uninstall
```

On macOS you can set `HYPERCOLOR_INSTALL_PREFIX` and `HYPERCOLOR_INSTALL_DIR` to move the install root. On Linux the prefix is fixed at `~/.local` and the binary directory at `~/.local/bin`; the script refuses anything else so the systemd unit's `%h/.local/bin/hypercolor-daemon` path always resolves.

### What the installer does

1. Detects your architecture and downloads the matching release tarball from GitHub.
2. Verifies the SHA256 checksum before extracting.
3. Installs `hypercolor`, `hypercolor-daemon`, `hypercolor-app`, `hypercolor-tui`, and `hypercolor-open` to `~/.local/bin`.
4. Installs the systemd **user** service to `~/.config/systemd/user/hypercolor.service` and enables it.

The release tarball carries the udev rules and the `i2c-dev` modules-load config, but the one-liner never applies them, because it never asks for `sudo`. To get USB device access and SMBus RGB working, run `just udev-install` from a checkout, or install the `.deb` or the AUR package, which place both for you.

After the installer finishes, see [First launch](@/guide/first-launch.md) to open the UI for the first time.

---

## Windows installer {% raw %}{#windows-installer}{% endraw %}

Download the installer from the [download page](@/download.md) and run it. The
install is per-machine and asks for administrator elevation (UAC). The same
elevated pass runs hardware setup: it installs the
[PawnIO](https://github.com/namazso/PawnIO) SMBus modules and registers the
`HypercolorSmBus` broker service. Windows builds are currently unsigned, so
SmartScreen may warn on first run; choose "More info" and then "Run anyway".
Tested on Windows 10 22H2 and Windows 11 23H2/24H2, x64.

USB-HID lighting (Razer, Corsair, Lian Li, and others) and network devices (Hue, WLED, Nanoleaf, Govee) work out of the box. Motherboard and DRAM SMBus lighting (ASUS Aura, MSI, Gigabyte) uses the PawnIO hardware support installed above; if that step was skipped or failed, re-run it from Settings → Device Discovery → Hardware Support.

---

## macOS {% raw %}{#macos-dmg}{% endraw %}

### DMG

When a release includes an accepted macOS build, download
`Hypercolor-<version>-arm64.dmg` (Apple Silicon) or `-x86_64.dmg` (Intel) from
the [download page](@/download.md), drag the app into `/Applications`, and
launch. Minimum macOS 15.2 (Sequoia).

{% <callout type="info"> %}
Public CI does not publish unsigned macOS packages. macOS artifacts are
promoted manually only after Developer ID signing, notarization, and the signed
physical acceptance checkpoint pass.
{% </callout> %}

The native ScreenCaptureKit, host-input, HDR, and multi-owner implementations
are present, but they are not release-qualified until the signed macOS physical
acceptance matrix ships with the release provenance. Development builds do not
establish durable TCC grants or hardware support claims. Screen Recording is
requested only after an explicit local capture action. Audio-reactive effects
still need the loopback setup described in [Audio setup](@/guide/audio-setup.md).

The pending qualification matrix covers the app sidecar, direct launchd,
Homebrew service, and standalone daemon as distinct TCC identities. It also
covers Apple Silicon HDR, Intel SDR, and Tahoe paired-reference diagnostics.
Until those signed receipts pass, use the packaged app sidecar for protected
macOS sources and treat the other topologies as experimental.

### Homebrew {% raw %}{#homebrew}{% endraw %}

The tap carries both a cask and a formula. Maintainers update both manually
after the matching signed artifacts pass acceptance:

```bash
# Desktop app (both Mac architectures)
brew install --cask hyperb1iss/tap/hypercolor-app

# Daemon and CLI only, with brew services support
brew install hyperb1iss/tap/hypercolor
```

The formula covers macOS arm64 and x86_64 plus Linux amd64 and arm64; the cask is the full desktop app for either Mac architecture.

The formula selects the Homebrew service topology when managed with
`brew services`. Install the cask when protected macOS permissions or the
system screen picker require the app UI.

---

## AUR (Arch Linux) {% raw %}{#aur}{% endraw %}

The `hypercolor-bin` package is live on the AUR and updates automatically on every tagged release:

```bash
yay -S hypercolor-bin
```

The AUR package installs the prebuilt binaries, sets up the systemd user service, and places both udev rules files in the correct system paths.

---

## Build from source {% raw %}{#build-from-source}{% endraw %}

Building from source is the right path for contributors, packagers, and people who need a custom build (e.g., with Servo HTML effect rendering enabled). It is not necessary for end users.

You need:
- Rust 1.94+ (Edition 2024), installed via `rustup`
- `just`, the task runner
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

{% <callout type="tip"> %}
The `just setup` and `just install` path uses the same install layout as the prebuilt one-liner. Both land in `~/.local` with the same systemd unit and udev rules; the only difference is that source builds compile everything on your machine.
{% </callout> %}

---

## After installing

Whichever path you took, your next stop is [First launch](@/guide/first-launch.md), which walks through the first-run wizard, device discovery, and opening the web UI for the first time.

If a USB device does not appear after install, the most common cause is udev rules not applied yet. Re-plug the device or log out and back in. The [Quick start](@/guide/quick-start.md) covers this and has a one-command health check.
