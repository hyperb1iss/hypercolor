+++
title = "My device isn't supported"
description = "The ladder for hardware Hypercolor does not light up: confirm native status, read the coverage view, bridge through OpenRGB, then file a device-support request with the facts prefilled."
weight = 95
+++

Your device is plugged in, the OS sees it, and Hypercolor does nothing with it. This page
walks the four rungs in order, from cheapest to most involved, and each rung ends either
with the device lit or with a clear reason to climb to the next one. By the bottom you
have either working lighting or a device-support request that already carries every fact
a driver author needs.

{% <callout type="tip"> %}
Before anything else, rule out a permission or conflict problem. A device that is
Supported but invisible is a [devices not found](@/troubleshooting/devices-not-found.md)
case, not a missing driver.
{% </callout> %}

---

## Rung 1: check native support

Look the device up in the [compatibility matrix](@/hardware/compatibility.md). The status
column decides what happens next:

| Status | What it means for you |
|---|---|
| **Supported** | A native driver ships. Stop here and fix discovery: [devices not found](@/troubleshooting/devices-not-found.md). |
| **In progress** | Driver code exists but is not merged or complete. Skip to rung 3 for now; the issue tracker already has the request. |
| **Researched** | The protocol is documented, no driver yet. Rung 3 gets you lit today; rung 4 with a "willing to test" answer speeds the driver up. |
| **Blocked** | A driver exists but cannot work without a change outside Hypercolor (firmware, vendor). Rung 3 may still work. |
| **Known** | The device is on the list and unresearched. Rungs 3 and 4 both apply. |
| Not listed | Nobody has tracked this device. Rung 2 confirms what the OS sees, then rungs 3 and 4. |

Also check that the device's driver family is enabled. Every native driver can be turned
off in config, and a disabled driver leaves its hardware in exactly the same place as a
missing one.

```bash
hypercolor drivers list
```

---

## Rung 2: read the coverage view

The daemon keeps an inventory of USB hardware it saw and could not match, and a
per-device view of who owns what:

```bash
# USB devices no driver matched
hypercolor devices unclaimed

# Every physical device with native, bridge, and active ownership
hypercolor devices coverage
```

An unclaimed row carries the vendor id, product id, manufacturer and product strings,
serial, bus path, interface classes, and `claimable_by`. Two cases:

- **`claimable_by` names a driver.** A native driver has a descriptor for this device and
  is disabled. Enable it (`hypercolor config set drivers.<driver_id>.enabled true`) and
  rescan. No OpenRGB needed.
- **`claimable_by` is empty.** No native protocol exists for this VID:PID. Keep the row
  handy: it is the exact data rung 4 asks for, and the Devices page offers a **Request
  support** button on it that prefills the form.

The same rows appear under **Unclaimed hardware** on the Devices page, refreshed live as
devices come and go. If the device does not appear in `devices unclaimed` at all, the USB
scanner never saw it: check `lsusb` (Linux), System Report → USB (macOS), or Device
Manager (Windows) before going further. SMBus hardware (motherboard, DRAM, GPU) is not
enumerated this way; a missing SMBus device goes straight to rung 3.

---

## Rung 3: bridge through OpenRGB

OpenRGB supports hundreds of controllers Hypercolor has no native driver for, and
Hypercolor can drive any of them through its bridge while keeping native drivers in charge
of everything they support. Hypercolor guides the install, writes an OpenRGB configuration
that skips natively owned hardware, and runs the server on loopback.

```bash
hypercolor openrgb hints          # install command for your platform
hypercolor openrgb partition      # write the managed OpenRGB config
hypercolor openrgb start          # run the server on 127.0.0.1:6742
hypercolor config set drivers.openrgb.enabled true
hypercolor config set drivers.openrgb.ownership.mode open_rgb_owned
hypercolor devices discover --target openrgb
hypercolor devices coverage       # the device should now read active = bridge
```

The managed configuration is written per device, not per brand. A native driver that owns
one of your devices keeps OpenRGB away from the USB devices that driver can drive, so a
Razer headset Hypercolor has no protocol for stays bridgeable next to a natively driven
Razer base station. Two cases stay conservative and are withheld for the whole brand while
its native driver owns a device: RAM and motherboard controllers on SMBus (they have no
USB id to match), and OpenRGB detector names from a different OpenRGB release than the one
Hypercolor's detector map was built from. If your device falls in one of those, disable
that native driver to hand the brand to OpenRGB.

The full flow, including per-platform install, Linux udev and I2C setup, Windows PawnIO
notes, and zone sizing for ARGB hubs, is on the [OpenRGB fallback](@/hardware/openrgb-fallback.md)
page.

If OpenRGB does not list the device either, two possibilities remain. A GPU whose PCI
subsystem id OpenRGB does not know is a request for the OpenRGB project. Anything else
goes to rung 4.

{% <callout type="info"> %}
A bridged device is a working device, not a second-class one: it joins layouts, scenes,
and effects like any native device. Native support still matters for latency, stable
identity, and running without a second process, which is why rung 4 is worth the two
minutes even when rung 3 lit the device.
{% </callout> %}

---

## Rung 4: file a device-support request

The [device-support issue form](https://github.com/hyperb1iss/hypercolor/issues/new?template=device-support.yml)
asks which product you own, what lighting or display you want to control, and how you
control it today. Put the retail brand and model in the title. If you cannot identify
the product, say unknown and share what you can, such as a product link or a photo of
its label with the serial number covered.

The **Request device support** button on an unclaimed row fills in the USB IDs, detected
device details, operating system, and running Hypercolor version when available. USB
names such as "USB Keyboard" are kept in the detected details; they do not identify the
retail product or prove it has controllable lighting. If the device has no lighting or
display, it does not need Hypercolor support.

The shipped `rig-setup` skill can also prepare a form link and look for existing requests.
Open the link, answer the product and lighting questions, and submit through GitHub.
You do not need packet captures or protocol knowledge to ask for support.

### What makes a request actionable

- **Product and behavior.** Name the actual product and describe its lights or screen,
  what you want to change, and whether an app, physical buttons, or keyboard shortcuts
  control it today. "I am not sure" is a useful answer too.
- **Software and version.** Name any app that controls it and what works. Include your
  running daemon version from Settings > About or `hypercolor status`.
- **Connection details.** USB IDs help match the device. For network or serial hardware,
  describe the connection instead. Leave this blank if you do not know.
- **Optional evidence.** Diagnostics, photos, and existing captures help. Review diagnostics
  before attaching them publicly. The [protocol research](@/contributing/adding-a-driver.md)
  workflow explains captures if a driver author needs them later.
- **Testing availability.** Tell us whether you can test a build on the device or would
  need instructions. You can still request support if you cannot test.

---

## Related pages

- [OpenRGB fallback](@/hardware/openrgb-fallback.md): the bridge in depth
- [Hardware compatibility](@/hardware/compatibility.md): the full device matrix with status per PID
- [Devices not found](@/troubleshooting/devices-not-found.md): when a Supported device is missing
- [Adding a driver](@/contributing/adding-a-driver.md): turn your request into a native driver
