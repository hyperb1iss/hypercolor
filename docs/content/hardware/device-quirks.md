+++
title = "Device quirks & rebrands"
description = "Known rebrands, firmware-split behavior, and devices that appear in the compatibility list but are not yet fully routed."
weight = 100
+++

# Device quirks & rebrands

The compatibility matrix tells you which devices are supported. This page covers the cases
where the matrix entry alone is not enough: hardware that was sold under multiple names,
devices whose firmware version determines which protocol is used, and devices that enumerate
correctly but whose full feature routing is not yet enabled.

---

## PrismRGB Prism 8 is a Nollie 8 v2 rebrand

If you plug in a PrismRGB Prism 8 controller and the device list reads **"Nollie 8 v2"**,
nothing is wrong. The Prism 8 is a hardware rebrand of the Nollie 8 v2. Hypercolor
identifies it by USB VID/PID and routes it through the Nollie driver, which is correct.

The specifics, sourced from `data/drivers/vendors/prismrgb.toml` and `nollie.toml`:

| Detail | Value |
|---|---|
| PrismRGB USB VID | `0x16D5` |
| Nollie 8 v2 USB VID | `0x16D2` |
| Shared PID | `0x1F01` |
| Driver | `nollie` |
| Wire format | GRB byte order |
| Host brightness scale | **0.75** (not 1.0; the Prism 8 has less hardware headroom) |

The Nollie 8 v2 and PrismRGB Prism 8 share PID `0x1F01` but enumerate on different VIDs.
Both are handled by a single `nollie` driver entry. The 0.75 brightness scale is applied
automatically; you do not need to configure anything.

{% callout(type="info") %}
In the driver database this device is named "Nollie 8 v2 / PrismRGB Prism 8", so the
device list may show it under the Nollie name even though the box says PrismRGB. This is
expected: the underlying silicon is the same hardware regardless of the badge.
{% end %}

---

## Lian Li Uni Hub AL: firmware determines the transport

The **Lian Li Uni Hub AL** (PID `0xA101`) ships across two distinct firmware generations
that use different USB protocols. Hypercolor supports both, but if your hub uses the older
firmware, it will enumerate via a different transport path.

| Firmware version | USB transport | Notes |
|---|---|---|
| v1.0 (AL10) | USB vendor protocol (`usb_vendor`) | Older units; distinct packet framing |
| v1.7 and later | USB HID (`usb_hid`) | Current shipping firmware |

Source: `data/drivers/vendors/lianli.toml`, `pid = 0xA101` notes field.

If a Uni Hub AL does not appear in `hypercolor devices list` after the udev rules are
installed, check which firmware version is on the hub. The hub's firmware version is
visible in Lian Li's L-Connect software on Windows, or sometimes printed on a label
inside the hub housing.

{% callout(type="warning") %}
Both firmware versions share the same PID (`0xA101`). Hypercolor keeps two database
entries for that PID and picks the right one by matching the hub's reported firmware
version (`1.7` selects the HID path, `1.0` selects the vendor path), not a manual
setting. If the device does not appear, verify that `udev/99-hypercolor.rules` is
installed and that you restarted the daemon after plugging in the hub.
{% end %}

The Uni Hub AL V2 (PID `0xA104`) is a separate device and always uses the HID transport;
the firmware split applies only to the original AL.

---

## Corsair Bragi wireless dongles: enumerated but not routed

Corsair wireless peripherals connect to the host via a USB receiver dongle. Hypercolor
enumerates these dongles and lists them in the device database, but **child routing is not
yet enabled**. This means the dongle itself appears in `hypercolor devices list` as a
researched device, but the wireless peripheral connected through it will not receive color
output.

Affected dongles (all `status = "researched"`, sourced from `data/drivers/vendors/corsair.toml`):

| Device name | PID |
|---|---|
| K57 Wireless Dongle | `0x1B62` |
| Ironclaw RGB Wireless Dongle | `0x1B66` |
| Harpoon Wireless Dongle | `0x1B65` |
| Dark Core RGB Pro Wireless Dongle | `0x1B81` |
| Dark Core RGB Pro SE Wireless Dongle | `0x1B7F` |
| Generic Bragi Dongle | `0x1BA6` |

If you have a Corsair wireless keyboard or mouse that connects via one of these dongles,
the wired USB connection mode on those peripherals does work through the standard Bragi
protocol. Check the [compatibility matrix](@/hardware/compatibility.md) for the wired
entry of your specific model. The Harpoon, Ironclaw, and Dark Core RGB Pro SE all have
supported wired-mode entries in the Bragi peripheral list.

{% callout(type="info") %}
Child routing (the dongle forwarding Hypercolor color packets to the wireless peripheral)
is the feature that is not yet implemented, not the Bragi protocol itself. Wired Bragi
devices work fully. This is a protocol engineering gap, not a hardware limitation.
{% end %}

---

## Understanding device status in the compatibility matrix

The [compatibility matrix](@/hardware/compatibility.md) uses five status values. Here is
what each means from a user perspective:

| Status | What it means |
|---|---|
| **Supported** | A working driver is compiled in. Plug the device in and it gets discovered and controlled. |
| **In progress** | Active development: a driver is being written or a protocol spec is being finalized. |
| **Researched** | The protocol is documented in the TOML but no driver code exists yet. Community contributions welcome. |
| **Blocked** | A driver skeleton exists on Hypercolor's side, but the device itself prevents control, typically a firmware limitation outside Hypercolor's reach. |
| **Known** | The device is in the database but protocol research has not started. |

The Corsair Bragi dongles above are marked **Researched**: the protocol is understood,
but the child-routing feature has not been built yet.

---

## Related pages

- [USB devices](@/hardware/usb-devices.md) — udev rules, hidraw access, replug requirements
- [Compatibility matrix](@/hardware/compatibility.md) — full supported device list
- [Devices not found](@/troubleshooting/devices-not-found.md) — diagnosis steps when a device does not appear
- [Conflicting software](@/hardware/conflicting-software.md) — other RGB tools that can claim a device before Hypercolor
