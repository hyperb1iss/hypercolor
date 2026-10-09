# Windows Installer

The Windows app ships as an NSIS installer that `cargo tauri build` renders
from `crates/hypercolor-app/installer.nsi`, with the lifecycle hooks in
`crates/hypercolor-app/installer-hooks.nsh`. This page covers what the
installer guarantees on upgrade, how to drive it from automation, and how
to maintain the template.

## Upgrade contract

Running a newer installer over an older Hypercolor upgrades it in place,
whether a person double-clicks the download or a script runs it silently.
An upgrade:

- installs into the existing install directory and skips the directory
  page (an explicit `/D=` still wins, and would leave the old install
  behind, so automation should not pass it)
- keeps the Start menu and desktop shortcuts exactly as the user left them
- keeps the `Hypercolor` value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which is how
  "start with Windows" is stored
- keeps app data under `%APPDATA%` and `%LOCALAPPDATA%`
- closes a running Hypercolor first, then stops any daemon still running
  from the install directory, including a daemon registered as a Windows
  service, and starts again exactly the daemon services it stopped
- stops the `HypercolorSmBus` broker before replacing files, then
  re-registers it and refreshes the firewall rules
- replaces `ui\` and `effects\bundled\` wholesale, so files a previous
  release shipped and this one dropped do not linger
- reinstalls the WebView2 runtime if it has gone missing

The installer never runs the previous version's uninstaller for an
upgrade. Installing the same version again, or an older one, keeps
upstream Tauri's behavior: run interactively, it offers to uninstall
first, and that uninstall removes shortcuts and the autostart value like
any other uninstall.

### Running it from automation

The installer is per-machine and must run elevated. Use a silent run for
unattended upgrades, and let the installer detect the upgrade itself:

```powershell
Hypercolor_<version>_x64-setup.exe /S /R
```

A bare `/S` over an older install gets every guarantee above. Adding
`/UPDATE` forces update mode even when the installed version is not
older, and also skips installing a missing WebView2 runtime, as upstream
does.

| Flag      | Effect                                                                                                              |
| --------- | ------------------------------------------------------------------------------------------------------------------- |
| `/S`      | Silent. No UI; a running Hypercolor is closed without asking; a reboot request from PawnIO answers No.              |
| `/P`      | Passive. Shows progress and skips the wizard's questions, though a reboot request from PawnIO still asks.           |
| `/UPDATE` | Update mode even when the installed version is not older. As upstream, it also skips installing a missing WebView2. |
| `/R`      | Relaunch Hypercolor after a silent or passive install, with `/ARGS "<args>"` passed through.                        |
| `/NS`     | Create no shortcuts. Only matters for a fresh install; upgrades never touch shortcuts.                              |

| Exit code | Meaning                                                                                                    |
| --------- | ---------------------------------------------------------------------------------------------------------- |
| 0         | Installed.                                                                                                 |
| 3010      | Installed, but PawnIO needs a Windows restart before motherboard lighting and CPU temperature come online. |
| Other     | The install did not complete.                                                                              |

The installer never restarts the machine under `/S`; a caller that sees
3010 should ask the user to restart.

## The template fork

`installer.nsi` is tauri-bundler 2.10.1's NSIS template with Hypercolor's
changes marked by `Hypercolor:` comments. The bundler fills the template
with its own Handlebars data, so installers must be built with tauri-cli
2.12.1, the release that pins that bundler. CI and the setup scripts
install exactly that version, `scripts/build-windows-installer.ps1` and
the Windows `just app-bundle` recipe refuse to build with any other, and a
packaging test keeps every pin and the template header in step.

To move to a newer tauri-cli:

1. Find the tauri-bundler version the new tauri-cli pins, and take its
   `src/bundle/windows/nsis/installer.nsi`.
2. Reapply every `Hypercolor:` hunk from the current fork onto it.
3. Update the version in the template header, `ci.yml`,
   `scripts/build-windows-installer.ps1`, the Windows `app-bundle` recipe
   in the `justfile`, `scripts/setup.sh`, `scripts/setup.ps1`, and
   `nsis_template_fork_builds_with_its_pinned_tauri_cli`.
4. Build an installer and run the upgrade check below.

When editing the hooks, keep two NSIS limits in mind. Every string,
command lines included, is capped at 1024 characters and silently
truncated past that, so logic longer than a one-liner belongs in a
script the hooks embed (as `installer-daemon-services.ps1` is), and
`installer_hook_commands_fit_the_nsis_string_limit` checks every hook
command against long paths. The installer is also a 32-bit process, so
`nsExec` runs 32-bit PowerShell, where `Get-Process` cannot read a
64-bit process's path; use `Win32_Process` through `Get-CimInstance`
instead.

## Wizard art

`crates/hypercolor-app/icons/nsis-header.bmp` and `nsis-sidebar.bmp` are
masters at three times their 100% size (450x171 and 492x942), generated by
`uv run assets/brand/build.py installer`. MUI would stretch them to the
wizard's controls with nearest-neighbour sampling, so the template's GUI
init and page SHOW callbacks replace each one with a copy resampled to the
control's real pixel size using HALFTONE filtering. That keeps the art
sharp from 100% to 300% display scaling.

## Testing

`scripts/test-windows-installer-upgrade.ps1` installs a previous release,
gives it a user's state (autostart on, desktop shortcut removed, files the
new release no longer ships), upgrades it, and checks the contract above.
`-Mode Interactive` clicks through the wizard the way a person does, which
is the path where upgrades used to uninstall first; `-Mode Silent` runs a
bare `/S`. The release lane (tag builds and `release_artifacts: full`
dispatches) runs both modes against the highest published release older
than the build, and skips with a notice when there is none. Locally the
script needs an elevated shell in a desktop session and replaces your
Hypercolor install.

To look at wizard pages without installing anything, render the template
with `cargo tauri bundle --bundles nsis`, copy
`target/release/nsis/x64/installer.nsi`, change
`RequestExecutionLevel admin` to `user`, add `Abort` as the first line of
`Section EarlyChecks`, and compile the copy with `makensis` from
`%LOCALAPPDATA%\tauri\NSIS`. The result runs unelevated and stops before
writing any file, with one exception: with the same or a newer version
installed, choosing to uninstall on the "already installed" page runs the
installed uninstaller for real. To see the fresh-install pages instead,
point `UNINSTKEY` in the copy at a key nothing has written.
