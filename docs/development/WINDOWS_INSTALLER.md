# Windows Installer

The Windows app ships as an NSIS installer that `cargo tauri bundle` renders
from `crates/hypercolor-app/installer.nsi`, with the lifecycle hooks in
`crates/hypercolor-app/installer-hooks.nsh`. This page covers what the
installer lays down, what it guarantees on upgrade, how to drive it from
automation, and how to maintain the template.

## What it installs

```text
%ProgramFiles%\Hypercolor\
  Hypercolor.exe            stable launcher: every shortcut and the Run value
  uninstall.exe
  units\<digest>\           the release: programs, UI, effects, tools
  update\slot-a.exe         recovery bootstrap, in a managed build
  update\slot-b.exe         retained bootstrap, in a managed build
%ProgramData%\Hypercolor\
  logs\                     the SMBus broker's and hardware setup's logs
  update\                   install-journal.json, active.json, installation.json
```

Nothing writes inside a unit, and nothing but the installer and the updater
changes what names one. The launcher reads `active.json` and starts that
unit's app.

The installer carries one unit, built by `scripts/produce-windows-unit.ps1`
from the finished (and, on a signing run, signed) programs, and its digest,
compiled into the installer. It unpacks the unit to a temporary directory and
runs that unit's own CLI, `hypercolor __install-release`, which does the
install as one journaled transaction:

1. creates both roots, or takes over the ones there, owned by Administrators;
2. finishes any transaction a killed installer left open;
3. copies the unit into `units\`, proving every byte against the digest;
4. moves the broker's `ImagePath` and the firewall rules onto the unit and
   points `active.json` at it, rolling all of that back on failure;
5. writes `installation.json` for the user at the desktop;
6. in a managed build, gives that user the coordinator's directory, writes
   both bootstrap slots, proves they load, and registers `HypercolorUpdate`
   (delayed auto-start, failure actions, and a security descriptor that lets
   the recorded user start it). In any other build it removes that service.
7. drops every unit nothing refers to and any leftover of a killed run.

Only a build with the `managed-update` feature of the `hypercolor` package,
which only the official release sets, registers the update service
(`scripts/build-windows-installer.ps1 -ManagedUpdate`).

Each step finds what exists and does what is missing, so a killed installer
is finished by running it again. Only after step 7 does the installer write
the launcher, uninstaller, registry entries and shortcuts, and the old flat
layout goes last of all, so a Hypercolor that starts stays on disk until
something else can.

An app that runs from a unit follows `HypercolorUpdate` for as long as it is
registered. The service runs only while an update or a recovery does, and
each time it starts it publishes a fresh pipe name in
`update\activator\pipe.json`, which only the recorded user can read. The app
waits for each start through the service control manager, connects to that
pipe once the service's own process proves to serve it, and on `quit` closes
its window, ends its daemon and exits, as the tray's Quit does. Otherwise it
waits for the service's next start, which the service control manager reports
even when it happened while the app was busy with the last run, so nothing
polls. In a build without the service the app finds nothing registered and
follows nothing.

## Upgrade contract

Running a newer installer over an older Hypercolor upgrades it in place,
whether a person double-clicks the download or a script runs it silently. A
flat install from before units always counts as older. An upgrade:

- installs into the existing install directory (an explicit `/D=` still wins,
  and would leave the old install behind, so automation should not pass it)
- moves the Start menu and desktop shortcuts it finds onto the launcher, and
  otherwise leaves them exactly as the user left them
- moves the `Hypercolor` value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which is how
  "start with Windows" is stored, onto the launcher, for every account signed
  in at the time. An account that is not signed in keeps a value naming the
  old layout, which starts nothing, until that account next opens Hypercolor.
- keeps app data under `%APPDATA%` and `%LOCALAPPDATA%`
- closes a running Hypercolor first, then stops any daemon still running from
  the install directory. A daemon registered as a Windows service is removed
  once the new release is in: the program it names does not outlive the
  install, and the app supervises the daemon from then on. An install that
  fails leaves the registration.
- stops the `HypercolorSmBus` broker first, then registers and starts it from
  the new unit
- reinstalls the WebView2 runtime if it has gone missing

The installation lives in Program Files, so the wizard has no directory page.
A `/D=` folder any standard user could change is refused, because only
administrators may be able to change what the update service runs.

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
| Other     | The install did not complete. Whatever ran before still runs, and running the installer again finishes it. |

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

The fork's `!uninstfinalize` line compares the signing command's exit
code, which upstream's does not, so a release with
[Windows signing](RELEASING.md#windows-signing) turned on fails instead of
shipping an unsigned uninstaller. A new NSIS plugin call in the template or
hooks also needs its DLL added to `$ExtraNsisPlugins` in
`scripts/sign-windows-artifacts.ps1` unless Tauri already signs it.

When editing the hooks, keep two NSIS limits in mind. Every string,
command lines included, is capped at 1024 characters and silently
truncated past that, so logic longer than a one-liner belongs in a
script the hooks embed (as `installer-daemon-services.ps1` is), and
`installer_hook_commands_fit_the_nsis_string_limit` checks every hook
command against long paths. `CheckIfAppIsRunning` spells its labels from a
line number, so each section or function can hold only one copy of it; the
fork calls it from a function instead. The installer is also a 32-bit process, so
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

`scripts/test-windows-installer-upgrade.ps1` is the proof. `-Mode Fresh`
installs on a clean machine. `-Mode Silent` and `-Mode Interactive` install a
previous release, give it a user's state (autostart on, desktop shortcut
removed, files the new release no longer ships), upgrade it, and check the
contract above; `Interactive` clicks through the wizard the way a person does.
`-Mode Killed` kills the installer while it publishes the unit, inside the
transaction and after it committed, runs the Start menu shortcut after each
kill, then finishes with one more run. A kill that misses its moment throws
that round away.
`-Mode Replace` replaces one unit with another, and `Fresh` first checks the
installer refuses a folder standard users can change. Every mode checks the ACLs and
owners of the units and records, that a standard user can start the launcher,
and with `-Managed` the update service's registration, and every mode ends by
uninstalling and checking nothing stayed behind.

The Windows Installer lane runs all of them on every change to the installer,
from the last release with a flat layout, against a dev build without Servo,
which takes minutes where a release build takes hours. The release lane (tag
builds and `release_artifacts: full` dispatches) runs the upgrade modes against
the release build and the highest published release older than it, and skips
with a notice when there is none. Locally the script needs an elevated shell
in a desktop session and replaces your Hypercolor install.

The template and hooks also compile and run off Windows, which is much the
quickest way to catch a mistake in either. With `makensis` on `PATH`, run
`cargo tauri bundle --bundles nsis --target x86_64-pc-windows-msvc --no-sign
--no-binary-patching` from `crates/hypercolor-app` with both `--config` files,
over a stub `target/bundle-stage/windows-unit` and `windows-unit.digest`. The
installer runs under Wine, against a real flat release too. Wine cannot prove
the store's ACLs, so a stub CLI stands in for the transaction.

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
