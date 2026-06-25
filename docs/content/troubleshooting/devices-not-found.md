+++
title = "Devices not found"
description = "USB device missing from Hypercolor? udev rules, logout/login, conflicting software, USB hubs, and the HAL discovery path explained."
weight = 10
template = "page.html"
+++

![Device discovery in the Hypercolor web UI](/img/ui/ui-devices.webp)

You ran `hypercolor devices list` (or checked the web UI) and the device you just
plugged in is nowhere. Here are the five reasons that happen, in order of how often
they come up.

## How Hypercolor finds USB devices

Hypercolor discovers USB devices through a HAL scan that enumerates `/dev/hidraw*`
nodes and matches VID/PID pairs against its compiled device database. The HAL transport
(`hypercolor-hal`) tries to open each matching node; if `open()` fails, the device
does not appear in the registry and no error is visible at the default `info` log
level. That makes permission problems especially silent.

## 1. udev rules are not installed

This is the most common cause by a wide margin. Without the rules those nodes are
root-only, and the daemon cannot open them.

Install the rules:

```bash
just udev-install
```

That copies `udev/99-hypercolor.rules` to `/etc/udev/rules.d/`, reloads the udev
database, and triggers a re-evaluation for hidraw, USB, tty, and i2c-dev subsystems.

If you installed from a prebuilt binary or are using an AppImage, the rules are
**not** installed automatically, so you must run this step by hand. Grab the rules file
from the [Hypercolor releases page](https://github.com/hyperb1iss/hypercolor/releases)
and then:

```bash
sudo cp 99-hypercolor.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=hidraw
sudo udevadm trigger --action=add --subsystem-match=usb
```

Verify the file landed and check a device node:

```bash
ls -l /etc/udev/rules.d/99-hypercolor.rules
ls -l /dev/hidraw*
```

{% callout(type="info") %}
The rules grant access via `TAG+="uaccess"`, so systemd-logind gives the physically
logged-in user access to the device nodes. A `GROUP="users" MODE="0660"` fallback is
also set for cases where the logind ACL replay misses an already-plugged device. Both
are in the same rules file.
{% end %}

{% callout(type="warning") %}
`just udev-install` re-triggers events for devices already plugged in, but on some
systems the logind `uaccess` ACL is not replayed for existing nodes. If re-triggering
does not help, re-plug the device or reboot.
{% end %}

## 2. You have not logged out and back in

If the udev rules install added you to a new group (or if `uaccess` was not in effect
at login time), your running session does not have the new permissions. The cleanest
fix is to log out and log back in.

Confirm what groups your current session has:

```bash
id
```

If `users` is missing and the device node is `GROUP=users MODE=0660`, you will not be
able to open it until you start a new login session.

## 3. udev rules are in the wrong location

Rules placed anywhere other than `/etc/udev/rules.d/` are silently ignored.
Double-check the path:

```bash
ls /etc/udev/rules.d/99-hypercolor.rules
```

Some distributions keep distribution-installed rules in `/lib/udev/rules.d/`.
Hypercolor's rules belong in `/etc/udev/rules.d/` so they take precedence. If you
copied the file there manually, reload and re-trigger by hand:

```bash
sudo udevadm control --reload-rules
sudo udevadm trigger --action=add --subsystem-match=hidraw
```

Then re-plug the device.

## 4. Another tool is holding the device

If the device node is accessible but Hypercolor still cannot connect, another
application has likely claimed the USB interface first. When this happens the HAL
transport receives a permission-denied error from the OS and reports it at `debug`
level as:

```
permission denied opening hidraw node ...
```

Common offenders:

- **openrazer daemon** — grabs Razer USB HID interfaces on boot
- **OpenRGB** — can hold HID interfaces for any vendor it supports
- **ASUS Aura Sync / Armoury Crate** (via Wine or native) — holds ENE SMBus and
  ASUS USB devices
- **iCUE** (via a Wine layer) — claims Corsair interfaces

Check whether another process has the device open:

```bash
# Which processes have a hidraw node open?
lsof /dev/hidraw*
```

Stop any conflicting daemon, then trigger a new discovery scan:

```bash
# openrazer
sudo systemctl stop openrazer-daemon.service

# OpenRGB (if running as a service)
pkill openrgb

# Trigger a USB rescan
hypercolor devices discover --target usb
```

{% callout(type="warning") %}
Hypercolor has its own built-in Razer driver and communicates directly with Razer
hardware over USB HID. Do **not** install openrazer to make Razer devices work with
Hypercolor; it is unnecessary and conflicts. If openrazer is running, the two
daemons fight over the same HID interface and neither behaves correctly.
{% end %}

## 5. Passive USB hub hiding the device

Some passive (non-powered) USB hubs do not correctly enumerate device descriptors,
which causes the kernel to miss VID/PID matching for udev rules. Powered hubs
generally work fine. If a device works plugged directly into a motherboard port but
not through a hub, the hub is the problem.

Front-panel USB headers also have marginal signal integrity on some cases. Try a rear
motherboard port directly when troubleshooting.

Check for kernel-level enumeration errors:

```bash
sudo dmesg | grep -i "usb\|hid" | tail -30
```

## Quick diagnostic flow

Run these in order to narrow down the cause:

```bash
# 1. Is the kernel aware of the device at all?
lsusb

# 2. Are hidraw nodes present?
ls /dev/hidraw*

# 3. Do you have permission to open them?
ls -l /dev/hidraw*

# 4. Is something else holding the device?
lsof /dev/hidraw*

# 5. Full daemon health check (devices, render, config)
hypercolor diagnose

# 6. Discovery with debug logging
RUST_LOG=hypercolor_hal=debug hypercolor devices discover --target usb
```

The daemon exposes the same health checks via REST while it is running:

```bash
curl -s -X POST http://localhost:9420/api/v1/diagnose | jq '.data.checks'
```

## Device visible in lsusb but still not discovered

If `lsusb` shows the device and the hidraw node is accessible, but
`hypercolor devices list` is still empty, the device's USB VID/PID may not be in
Hypercolor's device database. Run the daemon at `debug` level to see exactly what the
scanner finds:

```bash
RUST_LOG=hypercolor_hal=debug just daemon
```

Look for lines containing your device's vendor or product ID. A miss looks like:

```
hidraw node not found for XXXX:YYYY interface 0 (serial=<none>, usb_path=<unknown>, ...)
```

Check the [compatibility matrix](@/hardware/compatibility.md) for your device. If it
is listed as "Researched" or "In progress" rather than "Supported", driver support has
not shipped yet. Open an issue with your `lsusb -v` output.

## SMBus / motherboard RGB not found

SMBus access (for ASUS motherboard, GPU, and DRAM lighting) is a separate transport
from USB HID. It uses `/dev/i2c-*` nodes and requires the `i2c-dev` kernel module.
`just udev-install` grants access to those nodes, but the module must be loaded:

```bash
# Load i2c-dev if not already present
sudo modprobe i2c-dev

# Persist across reboots (distro-dependent)
echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf

# Verify the nodes exist
ls /dev/i2c-*
```

See [SMBus and I2C setup](@/hardware/smbus-i2c.md) for the full setup flow.

## Network devices (Hue, Nanoleaf, WLED, Govee)

Network devices use mDNS discovery, pairing credentials, and LAN transport, not
udev. If a network device is not appearing, the cause is almost always network
configuration, not permissions. See [network devices](@/hardware/network-devices.md)
for setup and [network discovery troubleshooting](@/troubleshooting/network-discovery.md)
for diagnosis.

## Related pages

- [USB devices](@/hardware/usb-devices.md) — full udev setup, transport variants, replug behavior
- [Hardware compatibility](@/hardware/compatibility.md) — the full supported-device matrix
- [Conflicting software](@/hardware/conflicting-software.md) — stopping openrazer, OpenRGB, and other daemons
- [Common issues](@/troubleshooting/common-issues.md) — port conflicts, systemd service failures
- [Debugging and diagnostics](@/contributing/debugging.md) — `RUST_LOG` targets, `hypercolor diagnose`, USB packet traces
