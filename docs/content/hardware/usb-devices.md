+++
title = "USB devices"
description = "Connect USB/HID/serial/MIDI devices on Linux: install udev rules, understand hidraw vs hidapi, fix access errors, and handle hotplug."
weight = 20
+++

# USB devices

USB-connected devices need a one-time permissions step on Linux before Hypercolor can talk to them. Without it the daemon finds the hardware but cannot open it, which looks like a silent no-op or a "device not found" error even though the device is physically present.

This page covers the setup, explains how Hypercolor reaches different device types, and walks through the most common failure modes.

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

## Install udev rules

Run once from the repo root (or from wherever Hypercolor was installed):

```bash
just udev-install
```

That recipe copies `udev/99-hypercolor.rules` to `/etc/udev/rules.d/99-hypercolor.rules`, reloads the ruleset, and triggers an `add` action for each relevant subsystem (`hidraw`, `usb`, `tty`, `i2c-dev`). It is equivalent to:

```bash
sudo cp udev/99-hypercolor.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=hidraw
sudo udevadm trigger --action=add --subsystem-match=usb
sudo udevadm trigger --action=add --subsystem-match=tty
sudo udevadm trigger --action=add --subsystem-match=i2c-dev
```

{% callout(type="warning") %}
**AppImage and Flatpak installs do not apply udev rules automatically.** You must run the copy command manually after installing through either of those distribution methods.
{% end %}

## Replug or reboot after install

`udevadm trigger` replays the rule against devices that are already connected, but the logind ACL (`TAG+="uaccess"`) can occasionally miss its replay for nodes that were opened before the rule was installed. If a device still fails after `just udev-install`, unplug it and plug it back in. A full reboot is the reliable fallback.

{% callout(type="danger") %}
**Device not found after udev install? Replug first.** If Hypercolor still cannot open the device after running `just udev-install`, unplug it and replug it before investigating further. The ACL that logind sets on `/dev/hidraw*` and `/dev/bus/usb/*` is applied at plug-in time, not at rule-reload time. A replug forces a fresh assignment.
{% end %}

## How the rules work

The rules file grants access to the physically logged-in user via `TAG+="uaccess"`. systemd-logind intercepts the udev event and sets a POSIX ACL on the device node so that your session user can open it without being in any special group. A `MODE="0660" GROUP="users"` fallback covers the rare case where ACL replay does not fire.

Two attributes matter for HID rules, and getting them backwards produces a rule that silently does nothing:

- `hidraw` rules use **`ATTRS{idVendor}`** (plural — walks up to the parent USB device node for the vendor/product match).
- `usb` rules use **`ATTR{idVendor}`** (singular) together with `ENV{DEVTYPE}=="usb_device"` to match the `/dev/bus/usb/` node directly.

The rules file also covers `SUBSYSTEM=="tty"` for serial devices (Dygma Focus-class keyboards, Nollie-class controllers) and `SUBSYSTEM=="i2c-dev"` for ASUS Aura motherboard and DRAM lighting over SMBus. See [SMBus/I2C devices](@/hardware/smbus-i2c.md) for that path.

## Which vendors get access

The file uses vendor-wide rules for brands where new product IDs should work without a rule update, and PID-scoped rules for vendors where that scope would be too broad.

| Vendor | VID | Scope |
|---|---|---|
| Razer | `1532` | Vendor-wide |
| Corsair | `1b1c` | Vendor-wide |
| ASUS Aura | `0b05` | Vendor-wide |
| Lian Li | `0416` | Vendor-wide |
| Ableton | `2982` | Vendor-wide |
| Dygma | `35ef` | Vendor-wide |
| Keychron (QMK) | `3434` | Vendor-wide |
| ZSA (QMK) | `3297` | Vendor-wide |
| Drop/OLKB (QMK) | `feed` | Vendor-wide |
| Glorious (QMK) | `320f` | Vendor-wide |
| PrismRGB / Nollie / GCS | `16d5` | PID-scoped |
| Lian Li (TL hubs) | `16d1`, `16d2`, `16d3` | PID-scoped |

PrismRGB, Nollie, and GCS share VID `16d5` with hardware that Hypercolor does not support, so those rules are scoped to known working PIDs. The `16d1`/`16d2`/`16d3` VIDs cover additional Lian Li TL-series hub generations, also PID-scoped.

## Transport types

Hypercolor's HAL defines a `TransportType` for every device in `crates/hypercolor-hal/src/registry.rs`. The USB-attached variants are below; SMBus/I2C is covered on its own [page](@/hardware/smbus-i2c.md). Understanding which one a device uses explains what the rules must cover and what can go wrong.

### UsbHidApi

The most common path for keyboards, mice, and combo input/lighting devices. Hypercolor talks through the OS HID stack via the `hidapi` library, which means the kernel `usbhid` driver stays attached and the device remains fully usable as an input device while lighting commands are sent. The daemon opens a `/dev/hidraw*` node that `hidapi` selects internally.

The `hidraw` rules in `99-hypercolor.rules` cover this path.

### UsbHidRaw

A lower-level Linux path that also targets `/dev/hidraw*` nodes, but goes through `async-hid` directly rather than through `hidapi`. The kernel `usbhid` driver stays attached here too. This is used for devices that need async feature-report or output-report I/O without the synchronous blocking model of `hidapi`.

The `hidraw` rules cover this path as well.

**UsbHidApi vs UsbHidRaw:** both paths leave the kernel HID stack intact. The difference is the Rust library used internally. From a permissions and user-setup standpoint they are identical — `just udev-install` handles both.

### UsbControl

Used by Razer peripherals. This path claims the USB interface directly via `nusb` and sends HID feature reports as USB Class control transfers. On Linux, the `detach_and_claim_interface` call detaches `usbhid` from the interface before claiming it, so the `usb` (not `hidraw`) rules in the file must be in place. The device node is in `/dev/bus/usb/`.

### UsbHid

Used by PrismRGB-class devices. Claims a HID interface directly and streams reports over its interrupt IN/OUT endpoints. Also uses `nusb`, also needs the `usb` rules.

### UsbVendor

Vendor-specific control-transfer transport. Claims the USB interface and drives the device through vendor control transfers rather than HID reports. Used by the Lian Li Uni Hub controllers. Needs the `usb` rules.

### UsbBulk

USB bulk-transfer transport with a HID feature-report sideband for initialization and keepalive commands, reserved for bulk display-class devices. Claims the USB interface, so it needs the `usb` rules.

### UsbMidi

Composite transport: MIDI for control commands plus USB bulk for display frames. Used by Ableton Push 2-class devices. Needs the `usb` rules and must not have another application (a DAW, for example) holding the MIDI port open at the same time.

### UsbSerial / CDC-ACM

Used by Focus-class serial devices (Dygma keyboards, some Nollie controllers). The device enumerates as a virtual serial port under `/dev/ttyACM*` or `/dev/ttyUSB*`. The `tty` rules in the file cover this path.

The baud rate for each device is set by its driver definition (115200 for the Focus-class controllers) and is applied automatically. You do not need to configure it manually.

## Verify access

After installing rules and replugging, confirm the daemon can see and open the device:

```bash
# List connected devices
hypercolor devices list

# Run a fresh discovery scan
hypercolor devices discover

# Check daemon logs for transport open/close events
RUST_LOG=hypercolor_hal=debug hypercolor-daemon
```

If a device appears in `devices list` but shows an error state, check the daemon log for a `TransportError::PermissionDenied` message. That means the udev rule matched the device node but the ACL was not applied — replug the device.

If the device does not appear at all, add `RUST_LOG=hypercolor_hal=debug` and look for the `NotFound` detail, which includes all candidate nodes that were considered and why they were filtered out.

## Hotplug

Hypercolor supports hotplug: devices plugged in after the daemon starts are discovered automatically. The daemon runs a background USB hotplug watcher that fires arrival and removal events as devices appear and disappear, so a newly connected supported device is enumerated without restarting the daemon.

If a hotplugged device is not picked up within a few seconds, trigger a manual discovery scan:

```bash
hypercolor devices discover
```

Or via the REST API:

```bash
curl -s -X POST http://localhost:9420/api/v1/devices/discover
```

Only one discovery scan can run at a time. A concurrent request returns HTTP 409 while a scan is in progress.

## Troubleshooting

### Device not found after udev install

This is the most common setup issue across every Linux RGB tool. Work through these steps in order:

1. Confirm the rule file is in place: `ls -l /etc/udev/rules.d/99-hypercolor.rules`
2. Replug the device — the ACL is assigned at plug-in time, not at rule-reload time
3. Check that the rule matched: `udevadm test /sys/class/hidraw/hidraw0 2>&1 | grep -E 'TAG|MODE|GROUP'`
4. Confirm the device VID is in the table above. If it is PID-scoped and your PID is not listed, open an issue with the VID:PID from `lsusb`
5. If another RGB tool has the device open (openrazer daemon, OpenRGB, Aura Sync), stop it first — Hypercolor cannot claim an interface that another app holds

**Permission denied on a hidraw node**

Run `ls -la /dev/hidraw*` and check that the ACL is set. If it shows only `crw-rw----`, logind has not assigned the ACL yet — replug or check that `systemd-logind` is running and that your session is recognized as active (`loginctl session-status`).

**Serial device not accessible**

Check `/dev/ttyACM*` and `/dev/ttyUSB*`. The `tty` rules use `ATTRS{idVendor}` (walk-up) just like the `hidraw` rules. If your user is not in the `users` group and logind ACL replay missed, add your user to `users` and log out/in:

```bash
sudo usermod -aG users $USER
```

**Another app is holding the device open**

Transports that claim the USB interface (`UsbControl`, `UsbHid`, `UsbVendor`, `UsbBulk`, `UsbMidi`) are exclusive. If another application — iCUE, Razer Synapse, an ALSA sequencer, or another RGB controller — has the interface open, Hypercolor's claim will fail. Stop the conflicting software first. See [conflicting software](@/hardware/conflicting-software.md) for per-vendor details.

**MIDI port already in use (Push 2)**

The `UsbMidi` transport connects to the Push 2 User MIDI port. If Ableton Live or another host has that port open, Hypercolor cannot connect. Close the DAW or remove the Push 2 from its MIDI device list.

## Related pages

- [SMBus/I2C devices](@/hardware/smbus-i2c.md) — ASUS Aura motherboard, GPU, and DRAM lighting
- [Compatibility matrix](@/hardware/compatibility.md) — supported devices by family and transport
- [Device quirks](@/hardware/device-quirks.md) — per-device notes and known limitations
- [Conflicting software](@/hardware/conflicting-software.md) — Synapse, iCUE, OpenRGB, and others
- [Troubleshooting: devices not found](@/troubleshooting/devices-not-found.md) — deeper diagnosis
