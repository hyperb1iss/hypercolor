+++
title = "Uninstall & reset"
description = "Remove Hypercolor cleanly: stop the daemon, remove binaries, udev rules, systemd units, launchd plists, autostart entries, and config directories."
weight = 160
+++

Hypercolor touches several system integration points on install: binaries, a
systemd user service or launchd agent, udev rules, desktop autostart, and a few
data directories. A clean uninstall needs to reach all of them. This page walks
through the process for each install method and platform.

{% callout(type="tip") %}
If you want to keep your lighting setup and reinstall a newer version, run the
installer again: it is idempotent and will overwrite the binaries and service
unit without touching your configuration.
{% end %}

---

## Stop the daemon first

Before removing anything, stop the running daemon so its lock files and output
streams are released cleanly.

**Linux (systemd):**

```bash
hypercolor service stop
systemctl --user stop hypercolor.service
```

**macOS (launchd):**

```bash
hypercolor service stop
launchctl unload ~/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist
```

**Windows:** quit Hypercolor from the tray menu (**Quit** exits the app and
stops the supervised daemon). The NSIS uninstaller also stops the
`HypercolorSmBus` broker service itself.

---

## Linux: curl installer (prebuilt path)

If you installed via `scripts/install-release.sh`, the installer's `--uninstall`
flag handles everything in one pass:

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh \
  | bash -s -- --uninstall
```

The script will ask for confirmation, then remove:

- Binaries from `~/.local/bin/`
- Bundled UI and effects from `~/.local/share/hypercolor/`
- The systemd user service file from `~/.config/systemd/user/hypercolor.service`
- The desktop launcher from `~/.local/share/applications/hypercolor.desktop`
- Shell completions for bash, zsh, and fish
- App icons from `~/.local/share/icons/hicolor/`
- The udev rules (`99-hypercolor.rules` and `70-hypercolor-input.rules`) from
  `/etc/udev/rules.d/`, when you approve the sudo-backed system hook removal

Your configuration at `~/.config/hypercolor/` is preserved by default. The
script will print an explicit reminder and the command to remove it if you want
a full purge.

To skip the confirmation prompt (useful in scripts):

```bash
curl -fsSL ... | bash -s -- --uninstall --yes
```

---

## Linux: .deb package

If you installed the release `.deb` on Debian or Ubuntu, remove it with apt:

```bash
sudo apt remove hypercolor
```

Package removal takes the binaries, service unit, udev rules, and completions
with it. Your configuration at `~/.config/hypercolor/` is left in place; see
[Remove configuration and cache](#remove-configuration-and-cache) below for the
full purge.

---

## Linux: build-from-source path

If you installed from source using `scripts/uninstall.sh` (which `just install`
sets up), run:

```bash
./scripts/uninstall.sh
```

Optional flags:

```
--system    Also remove udev rules (/etc/udev/rules.d/99-hypercolor.rules)
            and i2c-dev module persistence (/etc/modules-load.d/i2c-dev.conf).
            Both require sudo.

--purge     Implies --system; also deletes ~/.config/hypercolor and
            ~/.cache/hypercolor.
```

Full purge example:

```bash
./scripts/uninstall.sh --purge
```

---

## Linux: manual removal

If neither script is available, remove each component by hand.

**1. Stop and disable the service:**

```bash
systemctl --user stop hypercolor.service
systemctl --user disable hypercolor.service
systemctl --user daemon-reload
```

**2. Remove the service unit:**

```bash
rm -f ~/.config/systemd/user/hypercolor.service
```

**3. Remove binaries:**

```bash
rm -f ~/.local/bin/hypercolor \
       ~/.local/bin/hypercolor-daemon \
       ~/.local/bin/hypercolor-app \
       ~/.local/bin/hypercolor-tray \
       ~/.local/bin/hypercolor-tui \
       ~/.local/bin/hypercolor-open
```

**4. Remove data, desktop integration, and completions:**

```bash
rm -rf ~/.local/share/hypercolor
rm -f  ~/.local/share/applications/hypercolor.desktop
rm -f  ~/.local/share/icons/hicolor/scalable/apps/hypercolor.svg
rm -f  ~/.local/share/icons/hicolor/48x48/apps/hypercolor.png
rm -f  ~/.local/share/icons/hicolor/128x128/apps/hypercolor.png
rm -f  ~/.local/share/icons/hicolor/256x256/apps/hypercolor.png
rm -f  ~/.local/share/bash-completion/completions/hypercolor
rm -f  ~/.local/share/zsh/site-functions/_hypercolor
rm -f  ~/.config/fish/completions/hypercolor.fish
```

**5. Remove udev rules (requires sudo):**

```bash
sudo rm -f /etc/udev/rules.d/99-hypercolor.rules
sudo rm -f /etc/udev/rules.d/70-hypercolor-input.rules
sudo udevadm control --reload-rules
```

**6. Remove i2c-dev module persistence (if installed, requires sudo):**

```bash
sudo rm -f /etc/modules-load.d/i2c-dev.conf
```

---

## Linux: AUR package

The AUR package is named `hypercolor-bin`:

```bash
# yay or paru
yay -R hypercolor-bin
# or
paru -R hypercolor-bin
```

Add `-n` to delete package-owned config files instead of saving `.pacsave`
backups, or `-s` to also remove now-unneeded dependencies. The package installs
the service unit to
`/usr/lib/systemd/user/hypercolor.service`, both udev rules files to
`/usr/lib/udev/rules.d/`, and the i2c-dev module config to
`/usr/lib/modules-load.d/i2c-dev.conf`, so removing the package handles all of
those automatically. Your config at `~/.config/hypercolor/` is left in place.

---

## Windows: installed app

Uninstall from **Settings → Apps → Installed apps** (the classic Add/Remove
Programs entry works too). The NSIS uninstaller runs elevated and removes:

- The application files under Program Files, including the bundled PawnIO
  module blobs in the install directory's `tools\pawnio\` folder
- The `HypercolorSmBus` broker service registration (stopped, then deleted)
- The Windows Firewall rules ("Hypercolor Daemon" and "Hypercolor App")

Two things are deliberately not removed:

- **PawnIO itself stays installed.** It is a shared system component other
  software may rely on. Remove it separately from Programs & Features if you
  want it gone.
- **Your configuration and data are preserved**: config at
  `%APPDATA%\hypercolor\` and data plus logs at `%LOCALAPPDATA%\hypercolor\`.
  Delete both folders for a full purge (see below).

If you set the daemon up as a Windows service (an advanced, non-default setup
via `scripts/install-windows-service.ps1`), remove that service from an
elevated PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\uninstall-windows-service.ps1
```

The script stops the `Hypercolor` service and deletes its registration.

---

## macOS: curl installer

```bash
curl -fsSL https://raw.githubusercontent.com/hyperb1iss/hypercolor/main/scripts/install-release.sh \
  | bash -s -- --uninstall
```

On macOS the script removes:

- Binaries from `~/.local/bin/`
- The launchd agent plist from `~/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist`
- Bundled UI and effects from `~/.local/share/hypercolor/`
- Shell completions for bash, zsh, and fish

Your configuration at `~/Library/Application Support/hypercolor/` is preserved.

---

## macOS: Homebrew

The formula ships a `brew services` definition, so stop the service before you
uninstall:

```bash
brew services stop hypercolor
brew uninstall hypercolor
```

`brew services` owns its own launch agent (`homebrew.mxcl.hypercolor`) and cleans
it up on `stop` and `uninstall`, so there is no `tech.hyperbliss` plist to remove
by hand for the formula. The service writes its log to
`$(brew --prefix)/var/log/hypercolor/hypercolor.log`.

If you installed the desktop app via the cask, use `--zap` to also remove its
application-support, cache, log, and launch-agent files:

```bash
brew uninstall --zap --cask hypercolor-app
```

---

## macOS: manual removal

**1. Unload and remove the launchd agent:**

```bash
launchctl unload ~/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist 2>/dev/null || true
rm -f ~/Library/LaunchAgents/tech.hyperbliss.hypercolor.plist
```

The launchd label is `tech.hyperbliss.hypercolor`. Logs are written to
`~/Library/Logs/hypercolor/hypercolor.log`.

**2. Remove binaries:**

```bash
rm -f ~/.local/bin/hypercolor \
       ~/.local/bin/hypercolor-daemon \
       ~/.local/bin/hypercolor-app \
       ~/.local/bin/hypercolor-tray \
       ~/.local/bin/hypercolor-tui \
       ~/.local/bin/hypercolor-open
```

**3. Remove data:**

```bash
rm -rf ~/.local/share/hypercolor
rm -rf ~/Library/Logs/hypercolor
```

---

## Remove configuration and cache

Both install scripts preserve config by default to protect your lighting setup,
connection profiles, and scenes. When you are ready to wipe it:

| Platform | Directory | Contents |
|---|---|---|
| Linux | `~/.config/hypercolor/` | `hypercolor.toml`, `cli.toml` connection profiles |
| Linux | `~/.local/share/hypercolor/` | Bundled UI, effects, logs, first-run marker |
| Linux | `~/.cache/hypercolor/` | Servo runtime cache, transient state |
| Windows | `%APPDATA%\hypercolor\` | `hypercolor.toml`, `cli.toml` connection profiles |
| Windows | `%LOCALAPPDATA%\hypercolor\` | Bundled UI, effects, logs, first-run marker |
| macOS | `~/Library/Application Support/hypercolor/` | Config, scenes, effects, logs |
| macOS | `~/Library/Logs/hypercolor/` | Daemon log output |

Linux purge:

```bash
rm -rf ~/.config/hypercolor ~/.local/share/hypercolor ~/.cache/hypercolor
```

macOS purge:

```bash
rm -rf ~/Library/Application\ Support/hypercolor
rm -rf ~/Library/Logs/hypercolor
```

Windows purge (PowerShell):

```powershell
Remove-Item -Recurse -Force "$env:APPDATA\hypercolor", "$env:LOCALAPPDATA\hypercolor"
```

---

## Reset without uninstalling

To reset the first-run wizard without uninstalling, delete the marker file under
the data directory:

```bash
# Linux
rm -f ~/.local/share/hypercolor/first-run-complete

# macOS
rm -f ~/Library/Application\ Support/hypercolor/first-run-complete
```

```powershell
# Windows
Remove-Item "$env:LOCALAPPDATA\hypercolor\first-run-complete"
```

The next time the desktop app launches, it will run the welcome wizard again
(hardware support, autostart, and device discovery steps).

To reset only your lighting configuration while keeping the binaries and
service intact:

```bash
rm -rf ~/.config/hypercolor
```

The daemon will recreate a default `hypercolor.toml` on next startup.

---

## Verify the removal

After uninstalling, confirm nothing is still running:

```bash
# Linux
systemctl --user status hypercolor.service

# macOS
launchctl list tech.hyperbliss.hypercolor
```

Both commands should report that the unit is not loaded. Check that no orphaned
processes remain:

```bash
pgrep -l hypercolor
```

On Windows, confirm the processes and broker service are gone:

```powershell
Get-Process hypercolor* -ErrorAction SilentlyContinue
Get-Service HypercolorSmBus -ErrorAction SilentlyContinue
```

---

## See also

- [Installation](@/guide/installation.md) to install or reinstall Hypercolor
- [Configuration](@/guide/configuration.md) for the config file reference
- [Desktop app](@/guide/desktop-app.md) for autostart and tray settings
- [Troubleshooting: common issues](@/troubleshooting/common-issues.md) if uninstall leaves the daemon running
