+++
title = "Download"
description = "Current Hypercolor release downloads and installer entry points."
weight = 15
template = "page.html"
+++

Hypercolor release artifacts are published on GitHub Releases. Use the release
page for Windows installers, macOS DMGs, Linux tarballs, checksums, and release
notes:

[Open Hypercolor releases](https://github.com/hyperb1iss/hypercolor/releases)

## Linux

The release installer downloads the matching tarball for your architecture and
verifies its SHA256 checksum before installing:

```bash
curl -fsSL https://install.hypercolor.dev | bash
```

Pin a specific tagged release with `--version`:

```bash
curl -fsSL https://install.hypercolor.dev | bash -s -- --version v0.1.0
```

The installer sets up the systemd user service by default. USB and SMBus system
hooks require sudo; the installer prompts before installing udev rules and
persisting `i2c-dev`, or applies them automatically when run with `--yes`.

## Windows

Download the NSIS installer from the release page. The app installs per-user and
only asks for elevation when you opt into SMBus and RAM RGB support through the
PawnIO helper.

## macOS

Download the matching DMG for Apple Silicon or Intel from the release page.
Current builds are unsigned while the Developer ID and notarization rollout
finishes, so Gatekeeper may require right-clicking the app and choosing
**Open** on first launch.
