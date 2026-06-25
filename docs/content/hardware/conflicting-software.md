+++
title = "Conflicting software"
description = "Another RGB tool holding the HID device means Hypercolor gets nothing. How to detect, diagnose, and resolve the conflict."
weight = 110
template = "page.html"
+++

Your device shows up in `lsusb` but Hypercolor cannot connect to it. The most common
cause is that another RGB manager got to the device first and is holding it open.
Hypercolor gets no connection, and no error the user would naturally see — just silence.

This page covers which programs conflict, how to confirm a conflict is the culprit, and
how to resolve it cleanly.

## Why conflicts happen

Hypercolor controls USB devices through two transport paths:

- **USB control / HID / bulk / MIDI transports** — these claim the USB interface
  directly. The kernel allows only one process to hold a claimed interface at a time.
  A second claimant fails immediately.
- **HIDRAW / HIDAPI transports** — these talk through `/dev/hidraw*` nodes without an
  exclusive interface claim, but still require a successful `open()` on the device file.
  If another process holds an exclusive file descriptor, the open fails with a permission
  error even when the udev rules are correct.

In both cases the error surfaces in Hypercolor's logs as `TransportError::PermissionDenied`
or `TransportError::IoError`, and the device stays in a disconnected state. The transport
layer maps any underlying error message containing "permission" to `PermissionDenied`;
everything else becomes `IoError`.

{% callout(type="warning") %}
The udev rules in `99-hypercolor.rules` grant your user *permission* to open the device
node. They do not prevent another process from opening the same node first. Access control
and exclusivity are separate concerns.
{% end %}

## Software known to conflict

### openrazer daemon and kernel modules

The openrazer kernel modules (`razerkbd`, `razermouse`, `razerfirefly`, `razercore`, and
others) claim Razer USB devices at kernel driver level, before any userspace process opens
a node. The `openrazer-daemon` then talks to those modules. Both the modules and the
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

Stopping the daemon is often not enough — the kernel modules may still hold the devices.
Unload them:

```bash
# Unload in dependency order: peripherals first, then core
sudo modprobe -r razerkbd razermouse razermousemat razerfirefly \
               razernaga razerkraken razermug razercore

# Verify they are gone
lsmod | grep razer
```

To prevent them from reloading at next boot:

```bash
echo "blacklist razerkbd
blacklist razermouse
blacklist razermousemat
blacklist razerfirefly
blacklist razernaga
blacklist razerkraken
blacklist razermug
blacklist razercore" | sudo tee /etc/modprobe.d/no-openrazer.conf

sudo update-initramfs -u
```

After unloading, replug the device so the kernel re-applies the udev ACL. Then run
`hypercolor devices discover`.

### OpenRGB

OpenRGB talks to many of the same USB devices Hypercolor supports. If OpenRGB has already
connected to a device, Hypercolor's scan will fail to open the same interface. Close
OpenRGB entirely before starting Hypercolor.

If you want to use OpenRGB for hardware Hypercolor does not yet support natively, configure
it as a bridge rather than a parallel controller. The [OpenRGB fallback driver](@/hardware/openrgb-fallback.md)
lets Hypercolor route output through a running OpenRGB SDK server on port 6742, with
explicit ownership partitioning so both tools target different controllers.

### ASUS Aura Sync / Armoury Crate (Wine or Proton)

Aura Sync running under Wine or Proton binds to ASUS HID and SMBus interfaces using the
same device paths that native Linux applications use. Exit the Wine prefix hosting
Armoury Crate before launching Hypercolor.

### Corsair iCUE (Wine or Proton)

iCUE under Wine claims Corsair HID interfaces. The driver paths are exclusive. Exit iCUE
or the Proton prefix hosting it, then restart Hypercolor.

### ckb-next

`ckb-next-daemon` controls Corsair keyboards and mice via the same HID nodes Hypercolor
uses. Stop it before running Hypercolor:

```bash
systemctl --user stop ckb-next-daemon
```

### liquidctl

`liquidctl` primarily handles cooling but can open Corsair or NZXT RGB controllers. If
running as a service, stop it:

```bash
sudo systemctl stop liquidcfg
```

### Other RGB managers

SignalRGB, Polychromatic, and similar tools running via Wine or natively follow the same
pattern: one owner per USB interface. Whichever application connects first wins; the rest
see a failure.

## Diagnosing a conflict

### Step 1: confirm the device is visible to the OS

```bash
lsusb
```

If your device does not appear here, the problem is physical (cable, port, power) or a
missing udev rule — not a software conflict. See [USB devices](@/hardware/usb-devices.md)
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

The `devices` checks report the tracked device-registry count, output-queue health, and
USB actor display-lane timing.

## Resolving a conflict

The resolution is always the same: only one application can own a USB device interface at
a time. Stop the competing software, then let Hypercolor discover the device.

**For background daemons:**

```bash
# openrazer
systemctl --user stop openrazer-daemon

# ckb-next
systemctl --user stop ckb-next-daemon

# liquidcfg
sudo systemctl stop liquidcfg
```

**For GUI applications:**

Close the application completely. On Linux, some apps keep a background process alive
after the window closes:

```bash
pkill openrgb
pkill ArmouryCrate
```

**After stopping the competing software:**

Replug the USB device. The kernel re-runs the udev rules and grants Hypercolor the ACL on
the device node. Then trigger a rescan:

```bash
hypercolor devices discover
```

Or via the web UI: open the Devices panel and click Scan.

{% callout(type="tip") %}
If you want to keep OpenRGB available for hardware Hypercolor does not natively support,
configure it as a bridge rather than a parallel controller. The OpenRGB fallback driver
lets Hypercolor route output through a running OpenRGB SDK server on port 6742, with
explicit ownership partitioning. See [OpenRGB fallback](@/hardware/openrgb-fallback.md).
{% end %}

## SMBus and I2C conflicts ⚡

ASUS motherboard, GPU, and DRAM lighting goes over `/dev/i2c-*` (SMBus) rather than USB
HID. The same exclusive-ownership principle applies: only one process should be issuing
SMBus transactions to a controller at a time. Two applications writing to the same I2C
address simultaneously corrupt device state — flickering, wrong colors, or the controller
locking up.

{% callout(type="danger") %}
Running Hypercolor and OpenRGB (or Aura Sync) simultaneously against the same SMBus
controllers can corrupt device state. On some ASUS DRAM controllers this requires a
physical power cycle to recover.
{% end %}

Check what is accessing your i2c nodes:

```bash
lsof /dev/i2c-* 2>/dev/null
```

If you are running OpenRGB alongside Hypercolor and both are configured to control ASUS
Aura hardware, disable SMBus scanning in one of them before enabling SMBus access in
Hypercolor. See [SMBus and I2C devices](@/hardware/smbus-i2c.md) for setup details.

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

## Still not connecting?

If you have stopped all competing software and the device still does not appear:

1. Verify udev rules are installed: `ls -la /etc/udev/rules.d/99-hypercolor.rules`
2. Reload rules and replug: `sudo udevadm control --reload && sudo udevadm trigger`
3. Check kernel messages: `sudo dmesg | grep -i "hid\|usb" | tail -20`
4. See [Devices not found](@/troubleshooting/devices-not-found.md) for the full
   device-not-found troubleshooting flow.

## Related pages

- [USB devices](@/hardware/usb-devices.md) — udev rules, hidraw vs hidapi, permissions
- [SMBus and I2C devices](@/hardware/smbus-i2c.md) — ASUS Aura motherboard and DRAM setup
- [OpenRGB fallback bridge](@/hardware/openrgb-fallback.md) — co-existing with OpenRGB via the SDK bridge
- [Devices not found](@/troubleshooting/devices-not-found.md) — per-transport diagnosis when discovery returns nothing
- [Debugging and diagnostics](@/contributing/debugging.md) — RUST_LOG targets and the diagnose endpoint
