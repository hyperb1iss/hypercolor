+++
title = "Download"
description = "Current Hypercolor release downloads and installer entry points."
weight = 15
template = "page.html"
+++

Hypercolor release artifacts are published on GitHub Releases. Use the release
page for Windows installers, macOS DMGs, Linux tarballs and `.deb` packages,
checksums, and release notes:

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

Pin any tagged release with `--version`:

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh | bash -s -- --version v0.2.1
```

The installer sets up the systemd user service by default. USB and SMBus system
hooks require sudo; the installer prompts before installing udev rules and
persisting `i2c-dev`, or applies them automatically when run with `--yes`.

### Debian and Ubuntu (.deb)

Each release also ships a `.deb` package that installs the daemon, CLI, systemd
user service, udev rules, and shell completions through your package manager:

```bash
sudo apt install ./hypercolor_<version>_amd64.deb
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
motherboard and DRAM RGB work without a second prompt later.

Windows builds are currently unsigned, so SmartScreen may warn on first run.
Choose "More info" and then "Run anyway" to continue.

## macOS

Download the matching DMG for Apple Silicon or Intel from the release page.
Builds are ad-hoc signed but not notarized, so Gatekeeper requires
right-clicking the app and choosing **Open** on first launch.

On Apple Silicon, `install-release.sh` also works for a daemon-and-CLI install,
and Homebrew carries both the `hypercolor` formula (CLI and daemon) and the
`hypercolor-app` cask (desktop app). See
[Choose your install](@/guide/choose-your-install.md) for the tradeoffs.
