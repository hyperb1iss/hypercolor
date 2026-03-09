# 20 - Installation Bootstrap

> One command to turn a source checkout into a usable local Hypercolor install.

## Problem

Hypercolor currently has pieces of an install story, but not one real path:

- `udev/99-hypercolor.rules` exists and can be installed manually
- the daemon can serve the web UI, but only auto-discovers it from the repo checkout
- desktop-integration docs assume systemd socket activation, D-Bus service activation, and a tray app that do not exist yet
- ASUS SMBus discovery depends on `i2c-dev`, and that module does not persist across reboot unless we install a `modules-load.d` entry

The March 8, 2026 Aura RAM regression was exactly this failure mode:

1. `i2c_i801` and `spd5118` loaded after reboot
2. `i2c_dev` did not
3. `/dev/i2c-*` and `/sys/class/i2c-dev` never appeared
4. `SmBusScanner` returned zero devices before probing Aura DRAM

Installer ownership is the fix. If Hypercolor needs host integration to function, install must set it up.

## Decision

Ship a source-install bootstrap script at `scripts/install.sh` that performs a user-local install under `~/.local` and applies the required system hooks with `sudo`.

This is the current supported install contract for the repo itself. Native distro packages can mirror the same work with package hooks later.

## V1 Scope

The installer should:

1. Build and install the daemon binary (`hypercolor`)
2. Build and install the CLI binary (`hyper`)
3. Build and install the web UI into a stable runtime path outside the repo
4. Install a systemd user service that starts the daemon with `--ui-dir`
5. Install a desktop launcher that opens the local web UI
6. Install shell completions for `hyper`
7. Install and reload udev rules
8. Install `/etc/modules-load.d/i2c-dev.conf`
9. Run `modprobe i2c-dev` during install so SMBus is available immediately

## V1 Install Layout

User-local files:

- `~/.local/bin/hypercolor`
- `~/.local/bin/hyper`
- `~/.local/bin/hypercolor-open`
- `~/.local/share/hypercolor/ui/`
- `~/.local/share/applications/hypercolor.desktop`
- `~/.config/systemd/user/hypercolor.service`

System files:

- `/etc/udev/rules.d/99-hypercolor.rules`
- `/etc/modules-load.d/i2c-dev.conf`

## Why User Service, Not System Service

Current Hypercolor is a desktop-session tool:

- config is user-scoped
- the API binds localhost
- device control belongs to the logged-in user session
- we do not need a privileged daemon

So the installer should create a systemd user service and enable it for the current user.

## Current Non-Goals

These belong to later work, not the first bootstrap script:

- systemd socket activation
- `Type=notify` watchdog integration
- D-Bus service activation
- tray autostart
- native desktop shell packaging
- SELinux/AppArmor policy install
- distro-specific package hooks

The codebase and docs mention several of these already, but the current daemon does not implement them yet.

## Packaging Constraints

The installer must reflect the codebase as it exists today:

- the daemon only auto-discovers the UI from `crates/hypercolor-ui/dist`
- installed service units must therefore pass `--ui-dir %h/.local/share/hypercolor/ui`
- `hypercolor-desktop` now builds as its own `hypercolor-desktop` binary, so it no longer collides with the daemon
- the installer still should not ship the Tauri shell until native desktop packaging is implemented

## Follow-On Work

After the bootstrap script lands, the next installation work should be:

1. Add a real `hyper permissions install` CLI path so the app can repair host integration after install
2. Add a diagnostics check that explicitly reports `i2c-dev` missing vs permission denied
3. Port the bootstrap logic into package-native hooks for Arch, Debian, Fedora, and Nix
