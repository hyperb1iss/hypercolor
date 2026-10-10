+++
title = "Conflicting software"
description = "Another RGB tool holding a device means Hypercolor gets nothing. Which programs conflict on Linux, Windows, and macOS, and how to detect and resolve it."
weight = 110
template = "page.html"
+++

Your device shows up in `lsusb` but Hypercolor cannot connect to it. The most common
cause is that another RGB manager got to the device first and is holding it open.
Hypercolor gets no connection, and no error the user would naturally see, just silence.

This page covers which programs conflict, how to confirm a conflict is the culprit, and
how to resolve it cleanly.

## Hypercolor checks for you

On Linux and Windows the daemon looks for known competing programs on its own. It lists
running processes (and, on Windows, running services) at startup, every 30 seconds, after
each discovery scan, and whenever a device fails to open or keeps failing writes. When it
finds one, you see it in several places:

- The **Devices** page opens with a warning banner that names each running program, what
  it competes for, and how to quit it. **Check again** runs a fresh scan. If you run a
  program on purpose, dismiss its warning. Hypercolor remembers the dismissal in this
  browser until it sees the program stop, so the warning comes back if the program starts
  again.
- Every device a running program competes for shows a short line on its card and in its
  detail panel, such as "SignalRGB is running and may be holding this device". Dismissing
  the program in the banner hides these lines too.
- On Windows, the desktop app's SMBus support card in **Settings** warns when a program
  that drives motherboard, RAM, or GPU lighting is running, so you can quit it before
  installing SMBus support.
- `hypercolor diagnose` reports a `devices.competing_software` warning that names each
  program, the process or service it matched, and what to do about it.
- The daemon log gets a `competing RGB software is running` warning when a program
  appears, and a device that fails to open gets an `other RGB software may be holding
  this device` line naming the likely culprit. Both land in **Export Diagnostics** bundles.
- A device that keeps failing right after reconnecting reports an error that ends with
  the same hint, such as "SignalRGB is running and may be holding this device".
- `GET /api/v1/system/conflicts` returns the latest scan, and
  `POST /api/v1/system/conflicts/scan` runs a fresh one.

The catalog covers the vendor suites and Linux daemons that touch hardware Hypercolor
drives: SignalRGB, L-Connect, Razer Synapse, Corsair iCUE, Armoury Crate, MSI Center,
RGB Fusion, Polychrome, NZXT CAM, TT RGB Plus, NollieRGB, openrazer, ckb-next,
OpenLinkHub, lian-li-linux, and CoolerControl. It lives in
`crates/hypercolor-core/src/device/conflicts/catalog.toml`. OpenRGB is not on it: running
OpenRGB beside Hypercolor is a supported setup, and the [OpenRGB section](#openrgb) below
explains how the two share hardware. macOS is not checked yet.

A clean check doesn't rule out a conflict. Software missing from the catalog can still
hold a device, so the manual steps below still apply. A daemon running in a container
sees only the container's processes, so it can't spot programs running on the host.

## Why conflicts happen

Hypercolor controls USB devices through two transport paths:

- **USB control / HID / bulk / MIDI transports**: these claim the USB interface
  directly. The kernel allows only one process to hold a claimed interface at a time.
  A second claimant fails immediately.
- **HIDRAW / HIDAPI transports**: these talk through `/dev/hidraw*` nodes without an
  exclusive interface claim, but still require a successful `open()` on the device file.
  If another process holds an exclusive file descriptor, the open fails with a permission
  error even when the udev rules are correct.

In both cases the error surfaces in Hypercolor's logs as `TransportError::PermissionDenied`
or `TransportError::IoError`, and the device stays in a disconnected state. The transport
layer maps the OS error kind, not the message text: permission-denied becomes
`TransportError::PermissionDenied`, a missing node becomes `NotFound`, a dropped
connection becomes `Disconnected`, and anything else becomes `IoError`.

{% <callout type="warning"> %}
The udev rules in `99-hypercolor.rules` grant your user *permission* to open the device
node. They do not prevent another process from opening the same node first. Access control
and exclusivity are separate concerns.
{% </callout> %}

## Software known to conflict

### openrazer daemon and kernel modules

The openrazer kernel modules (`razerkbd`, `razermouse`, `razerkraken`, and
`razeraccessory`) claim Razer USB devices at kernel driver level, before any userspace
process opens a node. The `openrazer-daemon` then talks to those modules. Both the modules and the
daemon must be out of the picture for Hypercolor's native Razer driver to open the
devices.

Hypercolor has its own complete Razer driver and does **not** need openrazer. If you have
openrazer installed, stop and disable it:

```bash
# User-scope service (most installations)
systemctl --user stop openrazer-daemon
systemctl --user disable openrazer-daemon

# System-scope service (some distro packages)
sudo systemctl stop openrazer-daemon
sudo systemctl disable openrazer-daemon
```

Stopping the daemon is often not enough: the kernel modules may still hold the devices.
Unload them:

```bash
sudo modprobe -r razerkbd razermouse razerkraken razeraccessory

# Verify they are gone
lsmod | grep razer
```

To prevent them from reloading at next boot:

```bash
echo "blacklist razerkbd
blacklist razermouse
blacklist razerkraken
blacklist razeraccessory" | sudo tee /etc/modprobe.d/no-openrazer.conf

sudo update-initramfs -u
```

After unloading, replug the device so the kernel re-applies the udev ACL. Then run
`hypercolor devices discover`.

### SignalRGB (Windows)

SignalRGB drives nearly everything Hypercolor does, over USB and SMBus, so running both
means two programs writing to every device. Closing its window leaves it running in the
tray: right-click the tray icon and choose Exit. Version 2.5 and later also install a
`SignalRgb.Service` Windows service.

### Lian Li L-Connect (Windows)

`L-Connect-Service.exe` holds Lian Li hubs, the wireless dongles, and the WinUSB LCD
panels, and WinUSB lets only one program open a device. Quit L-Connect from its tray
icon, then stop the **L-Connect Service Watcher** and the **L-Connect Service** in
`services.msc`. Stop the watcher first, because it restarts the service.

### Razer Synapse (Windows)

Razer Synapse holds exclusive HID access to Razer devices on Windows, and Razer is the
largest family Hypercolor supports (70 devices), so this is the most common Windows
conflict. Quit Synapse from its tray icon, then check Task Manager for Razer services
that keep running after the window closes. Once they are stopped, Hypercolor can drive
the hardware; run `hypercolor devices discover`.

### OpenRGB

OpenRGB is a conflict only when it detects hardware a native Hypercolor driver owns. The
server Hypercolor manages for its [OpenRGB fallback](@/hardware/openrgb-fallback.md)
never does: `hypercolor openrgb partition` writes a detector partition into a
Hypercolor-owned config directory that disables OpenRGB's detectors for the hardware
each native driver can drive once that driver is enabled and owns a device, and
`hypercolor openrgb start` launches the server against that
directory. Native drivers keep their devices, the bridge drives the rest, and a conflict
guard output-disables any bridge route that still lands on natively owned silicon.

The OpenRGB GUI launched from a desktop menu, or a server you started by hand without
`--config`, uses your own `~/.config/OpenRGB` and detects everything. Whichever process
opens a device first wins. Either stop that instance and let Hypercolor run the server, or
point yours at the managed config:

```bash
openrgb --server --server-host 127.0.0.1 --noautoconnect \
        --config ~/.local/share/hypercolor/openrgb
```

To have OpenRGB drive a device Hypercolor also supports natively, disable that device on
the Devices page (or the whole driver in config) so the native driver releases it, then
rewrite the partition and rescan. The conflict guard's reason string,
`native driver owns this device (<driver_id>)`, tells you which driver to disable.

### ASUS Aura Sync / Armoury Crate

On Windows, Armoury Crate and Aura Sync claim ASUS HID interfaces and poll the SMBus
lighting controllers. Stop the ASUS AURA SYNC lighting service (`LightingService`) and
the Armoury Crate Service in `services.msc`, or remove the suite with ASUS's official uninstall tool. The SMBus
path on Windows additionally contends through the PawnIO broker, so vendor SMBus tools
and Hypercolor must not poll simultaneously.

Under Wine or Proton, Aura Sync binds to ASUS HID and SMBus interfaces using the same
device paths that native Linux applications use. Exit the Wine prefix hosting Armoury
Crate before launching Hypercolor.

### Corsair iCUE

On Windows, iCUE claims Corsair HID interfaces and keeps background services running
after the window closes. Exit iCUE from the tray, stop the Corsair Service
(`CorsairService`) in `services.msc`, then restart Hypercolor.

Under Wine or Proton, iCUE claims the same interfaces through the prefix. Exit iCUE or
the Proton prefix hosting it, then restart Hypercolor.

### ckb-next

`ckb-next-daemon` controls Corsair keyboards and mice via the same HID nodes Hypercolor
uses. It installs as a system service, so stop it with `sudo`:

```bash
sudo systemctl stop ckb-next-daemon
```

### liquidctl and CoolerControl

`liquidctl` primarily handles cooling but can open Corsair, Lian Li, and ASUS controllers.
It usually runs once and exits, so it only conflicts while a command is running or if you
set up a service for it yourself (the liquidctl README's example unit is named
`liquidcfg`).

CoolerControl is different: `coolercontrold` keeps every liquidctl device it finds open
for as long as it runs. Stop it before starting Hypercolor:

```bash
sudo systemctl stop coolercontrold
```

### Other RGB managers

SignalRGB, Polychromatic, and similar tools running via Wine or natively follow the same
pattern: one owner per USB interface. Whichever application connects first wins; the rest
see a failure.

### macOS

Vendor RGB suites are rare on macOS. The usual conflict there is another instance of an
open-source tool holding the HID handle; quit the other tool and rescan.

## Diagnosing a conflict

### Step 1: confirm the device is visible to the OS

```bash
lsusb
```

If your device does not appear here, the problem is physical (cable, port, power) or a
missing udev rule, not a software conflict. See [USB devices](@/hardware/usb-devices.md)
for udev setup.

### Step 2: find which process holds the node

```bash
# List hidraw nodes and check what has them open
lsof /dev/hidraw* 2>/dev/null | grep -v "^COMMAND"
```

If `lsof` names a process, that is the conflict. You can also use `fuser` for a specific
node:

```bash
sudo fuser /dev/hidraw0
ps aux | grep <PID>
```

### Step 3: check for kernel driver attachment

For Razer devices, the kernel module may hold the device even without a userspace process
showing in `lsof`:

```bash
lsmod | grep razer
```

If any `razer*` modules appear, they are claiming Razer devices at the kernel level. Unload
them as described in [the openrazer section above](#openrazer-daemon-and-kernel-modules).

### Step 4: read Hypercolor's logs

```bash
RUST_LOG=hypercolor_hal=debug just daemon
```

A conflict typically surfaces as a `PermissionDenied` transport error, or as a `NotFound`
error when the busy node cannot be selected:

```
ERROR hypercolor_hal: TransportError::PermissionDenied { detail: "... permission denied ..." }
ERROR hypercolor_hal: hidraw node not found for 1532:XXXX interface 0 ...
```

See [Debugging and diagnostics](@/contributing/debugging.md) for the full logging reference
and log target list.

### Step 5: run the built-in diagnostics

```bash
hypercolor diagnose

# Or via REST
curl -s -X POST http://localhost:9420/api/v1/diagnose | jq
```

The `devices` checks report the tracked device-registry count, output-queue health, USB
actor display-lane timing, display-output encoder health, and any competing software the
daemon found running (`devices.competing_software`).

### Diagnosing on Windows

The same flow works on Windows with different tools:

1. Run `hypercolor diagnose` first; its `devices.competing_software` check names the
   vendor programs it recognizes. For anything it doesn't know, find vendor processes with
   Task Manager, or with `Get-Process` in PowerShell:
   `Get-Process | Where-Object { $_.Name -match "Razer|iCUE|Armoury|Asus" }`
2. Check `services.msc` for vendor services (Razer, Corsair, ASUS) and stop them.
3. Open Device Manager to see which driver has claimed the device.

Steps 4 and 5 (daemon logs and `hypercolor diagnose`) are identical on every platform.

## Resolving a conflict

The resolution is always the same: only one application can own a USB device interface at
a time. Stop the competing software, then let Hypercolor discover the device.

**For background daemons:**

```bash
# openrazer
systemctl --user stop openrazer-daemon

# ckb-next (a system service)
sudo systemctl stop ckb-next-daemon

# CoolerControl
sudo systemctl stop coolercontrold
```

**For GUI applications:**

Close the application completely. On Linux, some apps keep a background process alive
after the window closes:

```bash
pkill openrgb        # an OpenRGB you started yourself; the managed server stops with `hypercolor openrgb stop`
pkill ArmouryCrate
```

**After stopping the competing software:**

Replug the USB device. The kernel re-runs the udev rules and grants Hypercolor the ACL on
the device node. Then trigger a rescan:

```bash
hypercolor devices discover
```

Or via the web UI: open the Devices panel and click Scan.

{% <callout type="tip"> %}
You do not have to give up OpenRGB for hardware Hypercolor lacks a driver for. Let
Hypercolor run it: `hypercolor openrgb partition` then `hypercolor openrgb start` brings
up a loopback SDK server that skips natively owned devices, and the bridge drives what
remains. `hypercolor devices coverage` shows which stack owns each device. See
[OpenRGB fallback](@/hardware/openrgb-fallback.md).
{% </callout> %}

## SMBus and I2C conflicts ⚡

ASUS motherboard, GPU, and DRAM lighting goes over `/dev/i2c-*` (SMBus) rather than USB
HID. The same exclusive-ownership principle applies: only one process should be issuing
SMBus transactions to a controller at a time. Two applications writing to the same I2C
address simultaneously corrupt device state: flickering, wrong colors, or the controller
locking up.

{% <callout type="danger"> %}
Running Hypercolor and OpenRGB (or Aura Sync) simultaneously against the same SMBus
controllers can corrupt device state. On some ASUS DRAM controllers this requires a
physical power cycle to recover.
{% </callout> %}

Check what is accessing your i2c nodes:

```bash
lsof /dev/i2c-* 2>/dev/null
```

The managed OpenRGB server disables the `ENE SMBus DRAM` and `ASUS Aura SMBus Motherboard` detectors
whenever the native ASUS driver is enabled and owns a device, so the two never probe the
same bus. A hand-run `openrgb` still can, and the symptom is specific: DRAM LED counts come
back wrong, or sticks flicker, after OpenRGB detection ran while the daemon was up. Never
run `openrgb --list-devices` or the OpenRGB GUI while native SMBus drivers are enabled;
use `hypercolor openrgb start` instead, or disable the native SMBus drivers first. See
[SMBus and I2C devices](@/hardware/smbus-i2c.md) for setup details.

## Preventing conflicts on startup

If openrazer or another RGB daemon starts automatically at login, you can stop it before
the Hypercolor user service launches using a systemd drop-in:

```bash
mkdir -p ~/.config/systemd/user/hypercolor.service.d
```

Create `~/.config/systemd/user/hypercolor.service.d/stop-openrazer.conf`:

```ini
[Service]
ExecStartPre=/usr/bin/systemctl --user stop openrazer-daemon
```

Reload the user manager:

```bash
systemctl --user daemon-reload
```

This runs a best-effort stop before Hypercolor starts and will not fail the service if
openrazer is not installed.

On Windows, disable the vendor suite's autostart entry on Task Manager's Startup apps
page so the vendor tool does not reclaim devices at login.

## Still not connecting?

If you have stopped all competing software and the device still does not appear:

1. Verify udev rules are installed: `ls -la /etc/udev/rules.d/99-hypercolor.rules`
2. Reload rules and replug: `sudo udevadm control --reload && sudo udevadm trigger`
3. Check kernel messages: `sudo dmesg | grep -i "hid\|usb" | tail -20`
4. See [Devices not found](@/troubleshooting/devices-not-found.md) for the full
   device-not-found troubleshooting flow.

## Related pages

- [USB devices](@/hardware/usb-devices.md): udev rules, hidraw vs hidapi, permissions
- [SMBus and I2C devices](@/hardware/smbus-i2c.md): ASUS Aura motherboard and DRAM setup
- [OpenRGB fallback bridge](@/hardware/openrgb-fallback.md): the managed server, detector partition, and conflict guard
- [My device isn't supported](@/hardware/unsupported-devices.md): what to do when neither stack lights a device
- [Devices not found](@/troubleshooting/devices-not-found.md): per-transport diagnosis when discovery returns nothing
- [Debugging and diagnostics](@/contributing/debugging.md): RUST_LOG targets and the diagnose endpoint
