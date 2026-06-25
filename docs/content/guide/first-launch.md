+++
title = "First launch"
description = "What happens the first time Hypercolor runs: app starts, daemon boots, single-instance guard, the first-run wizard, and the tray appearing."
weight = 60
+++

# First launch ⚡

The first time you open Hypercolor, several things happen in quick succession. The app
shell starts, the daemon boots inside it, a single-instance guard fires, and a welcome
wizard guides you through the three setup decisions that matter most. This page walks you
through each stage so you know what to expect and can intervene if anything stalls.

---

## What "the app" actually is

Hypercolor ships as two cooperating processes: the **app** (`hypercolor-app`) and the
**daemon** (`hypercolor-daemon`). When you launch the desktop installer build, you are
launching the app, a Tauri native shell that owns the window, the system tray, and
autostart registration. The app *supervises* the daemon as a child process, starting it
automatically and keeping it running for you.

The daemon is the real engine: it runs the render loop, hosts the REST and WebSocket API
on port 9420, discovers devices, and applies effects. The app's built-in web UI connects
to the daemon at that address and is what you see in the window.

If you are running Hypercolor headless (no desktop app), you start `hypercolor-daemon`
directly, and the single-instance guard and first-run wizard are app-only features. See
[The pieces](@/guide/the-pieces.md) for the full mental model.

---

## Launch sequence

### 1. App starts

The app reads any command-line flags it was given:

- `--minimized` or `--hidden`: start with the window hidden (used by autostart).
- `--show`: if another instance is already running, bring its window to the front.
- `--quit`: if another instance is already running, tell it to exit.

On Linux, the WebKit environment is checked first; if the required environment variables
are absent the binary re-execs itself with the correct setup.

### 2. Single-instance guard

The app uses `tauri-plugin-single-instance`. If a second copy of the app launches while
one is already running, the new copy forwards its arguments to the running instance and
exits. This means:

- Launching from your application menu when the app is already in the tray brings the
  window to the front. It does not start a second daemon.
- The `--quit` flag routes through the same mechanism and gracefully stops the running
  instance.

The daemon has its own independent single-instance guard (`single_instance::SingleInstance`
keyed to `"hypercolor-daemon"` on Linux/Windows, and a lockfile under the system temp
directory on macOS). If you somehow launch `hypercolor-daemon` directly while the app's
supervised daemon is running, the second daemon prints `hypercolor-daemon is already
running; exiting` and stops immediately.

### 3. Daemon supervisor starts

Inside the app's `setup` hook, the daemon supervisor checks whether a daemon is already
answering on the API port. If one is, it reuses that instance; on Linux it will also try a
systemd user service. Otherwise it spawns the daemon child process and monitors it. The
daemon binds to `127.0.0.1:9420` by default (loopback only) and starts the render loop.
Bundle resources, including bundled effect HTML files, are unpacked to the data directory
if they have not been installed yet.

### 4. Tray icon appears

The tray icon registers immediately after the window is created. You will see the
Hypercolor mark in your notification area. The tray is live even when the window is hidden,
giving you quick access to effects, brightness, and controls without opening the full UI.

### 5. Webview window opens

The app opens a native window (default 1200×800 px, minimum 800×500 px) pointing at
`http://localhost:9420`, the daemon's embedded web UI. Links that open new pages route to
your system browser rather than opening additional in-app tabs.

---

## The first-run wizard

On a fresh install the web UI detects that the first-run marker file does not exist and
shows the **Welcome to Hypercolor** overlay before you reach the dashboard.

<!-- TODO screenshot: welcome overlay / first-run wizard -->

The wizard is a single centered card. It covers three orientation topics and one
preference toggle.

### Devices

> "Add network lights from the Devices page. mDNS picks most up automatically."

USB devices are discovered automatically once udev rules are installed. Network devices
(WLED, Nanoleaf, Hue, Govee) are picked up via mDNS without configuration on most
networks. If a device does not appear, see [Finding devices](@/guide/finding-devices.md).

### Effects

> "Pick a built-in look from Effects or wire one to an audio or capture source."

The effects library is waiting for you right after the wizard. Audio-reactive effects need
a monitor source configured. See [Audio setup](@/guide/audio-setup.md).

### Hardware support (Windows only)

> "On Windows, Settings → Device Discovery can install motherboard SMBus access."

USB-HID and network devices work on Windows without any extra step. Motherboard and DRAM
RGB zones use the SMBus bus, which requires an additional kernel-level driver called
**PawnIO**. The wizard surfaces a "Set up RGB hardware support" button when:

- The host is Windows.
- The motherboard vendor is one Hypercolor can drive (ASUS, MSI, Gigabyte, ASRock),
  detected automatically from WMI.
- The SMBus broker service (`HypercolorSmBus`) is not already running.

If those conditions are not met (you are on Linux or macOS, your board is not
RGB-capable, or PawnIO is already installed), the hardware support row does not appear.

Clicking "Set up RGB hardware support" dismisses the wizard and navigates you to
Settings → Device Discovery where an elevated installer flow handles PawnIO, the SMBus
broker service (`HypercolorSmBus`), and five kernel modules
(`SmbusI801.bin`, `SmbusPIIX4.bin`, `SmbusNCT6793.bin`, `IntelMSR.bin`, `AMDFamily17.bin`).
Installation requires a UAC elevation prompt (PowerShell running
`install-windows-hardware-support.ps1`).

{% callout(type="warning") %}
If another RGB tool is running (SignalRGB, Corsair iCUE, ASUS Armoury Crate, MSI Center,
Gigabyte RGB Fusion, Razer Synapse, and others), it may already hold the SMBus or the HID
device open.
The wizard detects and lists conflicting services so you can close them before clicking
"Install support." Two RGB managers fighting for the same bus produces unpredictable
behavior.
{% end %}

### Start at sign in

A toggle in the wizard card controls **autostart**. It defaults to on, since most users
who installed an RGB orchestration tool want it running with their session. The toggle
registers or removes a system-level autostart entry (LaunchAgent on macOS, the Tauri
autostart plugin mechanism on Windows and Linux). When autostart fires, the app starts
with `--minimized`, so it goes directly to the tray without opening the window.

You can change this preference at any time in Settings → Session.

### "Let's go"

Clicking "Let's go" (or pressing Escape) applies the autostart choice and writes a marker
file to disk. The overlay will not appear again on subsequent launches.

The marker file lives at:

| Platform | Path |
|---|---|
| Linux | `~/.local/share/hypercolor/first-run-complete` |
| Windows | `%LOCALAPPDATA%\hypercolor\first-run-complete` |
| macOS | `~/Library/Application Support/hypercolor/first-run-complete` |

The file is empty; its presence is the entire state machine. Deleting it (or using
"Show welcome again" in Developer settings) resets the wizard so it appears on the next
launch.

---

## After the wizard

The window lands on the Hypercolor dashboard. The daemon is running, the tray is live,
and device discovery is already in progress. From here:

- Head to **Devices** to confirm your hardware was found.
- Open **Effects** to apply your first look.
- Follow [Your first 10 minutes](@/guide/your-first-10-minutes.md) for an opinionated
  path through the most useful features.

If devices are missing, the [Finding devices](@/guide/finding-devices.md) guide covers
USB udev rules, network mDNS, and per-protocol pairing.

---

## Launching without the desktop app

If you installed the daemon only (Linux server, headless setup, or manual `just daemon`
from source), there is no wizard and no tray. The daemon starts, writes its config to
`~/.config/hypercolor/hypercolor.toml` on first run, and listens on `:9420`. You can
reach the web UI at `http://localhost:9420` in a browser, or use the CLI or TUI:

```bash
# Check that the daemon is up
hypercolor status

# Open the terminal UI
hypercolor tui
```

See [Quick start](@/guide/quick-start.md) for the CLI-first path.
