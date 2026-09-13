+++
title = "Download"
description = "Current Hypercolor release downloads and installer entry points."
weight = 15
template = "page.html"
+++

Hypercolor release artifacts are published on GitHub Releases. Use the release
page for Windows installers, macOS disk images, Linux tarballs and `.deb`
packages, checksums, and release notes:

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
version=X.Y.Z
sudo apt install "./hypercolor_${version}_amd64.deb"

# arm64
sudo apt install "./hypercolor_${version}_arm64.deb"
```

### Arch Linux (AUR)

The `hypercolor-bin` AUR package updates automatically on every tagged release:

```bash
yay -S hypercolor-bin
```

### NixOS and Nix

The repository is a flake that wraps the same release tarball and ships a
NixOS module. Try it without installing anything:

```bash
nix run github:hyperb1iss/hypercolor -- devices
```

On NixOS, add the flake as an input and enable the module. It installs the
package, the udev rules, the `i2c-dev` kernel module, and a hardened systemd
user service that starts the daemon with every graphical login:

```nix
{
  inputs.hypercolor.url = "github:hyperb1iss/hypercolor";

  outputs = { nixpkgs, hypercolor, ... }: {
    nixosConfigurations.rig = nixpkgs.lib.nixosSystem {
      modules = [
        hypercolor.nixosModules.default
        { services.hypercolor.enable = true; }
      ];
    };
  };
}
```

Options live under `services.hypercolor`: `autoStart` (default `true`),
`logLevel`, `extraArgs`, `smbus.enable` (default `true`), and
`input.allDevices` (default `false`; grants every keyboard and mouse event
node to the seated user, which is a session-wide keylogging grant, so read the
description before turning it on). Screen-reactive effects on Wayland capture
through the desktop portal, so the module enables `xdg.portal` by default.
Log out and back in after the first rebuild so logind replays the device ACLs.

Outside NixOS, `nix profile install github:hyperb1iss/hypercolor` installs the
binaries, and the package ships a user unit with store paths already filled
in. systemd does not scan the Nix profile, so link the unit in and copy the
udev rules yourself:

```bash
mkdir -p ~/.config/systemd/user
ln -sf ~/.nix-profile/lib/systemd/user/hypercolor.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now hypercolor.service
sudo cp ~/.nix-profile/lib/udev/rules.d/*hypercolor*.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
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

Download the DMG for Apple Silicon or Intel from the release page, then drag
Hypercolor into Applications. You can also install the desktop app with
Homebrew:

```bash
brew install --cask hyperb1iss/tap/hypercolor-app
```

The `hypercolor` formula installs the CLI and daemon. See
[Choose your install](@/guide/choose-your-install.md) for the tradeoffs.
