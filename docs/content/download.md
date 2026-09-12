+++
title = "Download"
description = "Current Hypercolor release downloads and installer entry points."
weight = 15
template = "page.html"
+++

Hypercolor release artifacts are published on GitHub Releases. Use the release
page for Windows installers, Linux tarballs and `.deb` packages, checksums, and
release notes. Accepted macOS builds are attached manually after signing,
notarization, and physical acceptance:

[Open Hypercolor releases](https://github.com/hyperb1iss/hypercolor/releases)

The commands below install the latest tagged release. Not sure which path fits
your setup? [Choose your install](@/guide/choose-your-install.md) routes you to
the right one.

## Linux

The release installer downloads the matching tarball for your architecture and
verifies its SHA256 checksum before installing:

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash
```

Pin any tagged release with `--version` (replace `vX.Y.Z` with the tag):

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --version vX.Y.Z
```

The installer sets up the systemd user service without asking for `sudo`. It
does not install the udev rules or load `i2c-dev`, so USB and SMBus devices need
a separate permissions step. Debian and Ubuntu users should prefer the `.deb`
below, and Arch users should prefer the AUR package. Both install the system
hooks. Other distributions can follow the
[manual Linux permissions steps](@/guide/installation.md#linux-udev-rules-usb-and-input-device-access).

### Debian and Ubuntu (.deb)

Each release also ships a `.deb` package that installs the daemon, CLI, systemd
user service, udev rules, and shell completions through your package manager:

```bash
# x86_64
sudo apt install ./hypercolor_0.5.1_amd64.deb

# arm64
sudo apt install ./hypercolor_0.5.1_arm64.deb
```

### Arch Linux (AUR)

The `hypercolor-bin` AUR package updates automatically on every tagged release:

```bash
yay -S hypercolor-bin
```

## Windows

Download the NSIS installer (`Hypercolor_<version>_x64-setup.exe`) from the
release page. The installer is per-machine and asks for administrator elevation
(UAC). The same elevated pass also runs hardware setup: it installs the bundled
PawnIO SMBus modules and registers the HypercolorSmBus broker service, so
supported motherboard and DRAM RGB work without a second prompt later. Native
support currently covers ASUS Aura hardware; other vendors may work through
the separately installed [OpenRGB bridge](@/hardware/openrgb-fallback.md).

Windows builds are currently unsigned, so SmartScreen may warn on first run.
Choose "More info" and then "Run anyway" to continue.

## macOS

Public CI does not publish unsigned macOS packages. When a release includes an
accepted macOS build, download the matching signed and notarized DMG for Apple
Silicon or Intel from the release page. If a tag has no DMG, build from source
instead of installing an unqualified package.

The current stable release does not yet have an accepted macOS desktop build.
The latest signed DMG and Homebrew cask remain at 0.3.2 while the current
release completes the manual macOS acceptance lane.

On Apple Silicon, `install-release.sh` also works for a daemon-and-CLI install.
Homebrew carries both the `hypercolor` formula (CLI and daemon) and the
`hypercolor-app` cask (desktop app), with tap updates performed manually after
the matching signed artifacts pass acceptance. See
[Choose your install](@/guide/choose-your-install.md) for the tradeoffs.
