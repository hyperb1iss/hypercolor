+++
title = "Device quirks & rebrands"
description = "Known rebrands, firmware-split behavior, and devices that appear in the compatibility list but are not yet fully routed."
weight = 100
+++

The compatibility matrix tells you which devices are supported. This page covers the cases
where the matrix entry alone is not enough: hardware that was sold under multiple names,
devices whose firmware version determines which protocol is used, and devices that enumerate
correctly but whose full feature routing is not yet enabled.

---

## PrismRGB Prism 8 is a Nollie 8 v2 rebrand

A PrismRGB Prism 8 appears in the device list as **"PrismRGB Prism 8"** under the
PrismRGB family, but it is driven by the `nollie` driver because the Prism 8 is a hardware
rebrand of the Nollie 8 v2. Hypercolor identifies it by USB VID/PID and routes it through
the Nollie driver, which is correct.

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
Both are handled by the `nollie` driver module, which registers them as separate
descriptors with their own protocol bindings. The 0.75 brightness scale is applied
automatically; you do not need to configure anything.

{% <callout type="info"> %}
In the compatibility matrix the shared entry is listed as
"Nollie 8 v2 / PrismRGB Prism 8" because one matrix row covers both badges. The device
list keeps them apart. Each VID gets its own name and family even though the underlying
silicon is the same hardware regardless of the badge.
{% </callout> %}

---

## Lian Li Uni Hub AL: firmware determines the transport

The **Lian Li Uni Hub AL** (PID `0xA101`) ships across two distinct firmware generations
that use different USB protocols. Hypercolor supports both, but if your hub uses the older
firmware, it will enumerate via a different transport path.

| Firmware version | USB transport | Notes |
|---|---|---|
| v1.0 (AL10) | USB vendor protocol (`usb_vendor`) | Older units; distinct packet framing |
| v1.7 | USB HID (`usb_hid`) | Current shipping firmware; matched exactly |

Source: `data/drivers/vendors/lianli.toml`, `pid = 0xA101` notes field.

If a Uni Hub AL does not appear in `hypercolor devices list` after the udev rules are
installed, check which firmware version is on the hub. The hub's firmware version is
visible in Lian Li's L-Connect software on Windows, or sometimes printed on a label
inside the hub housing.

{% <callout type="warning"> %}
Both firmware versions share the same PID (`0xA101`). Hypercolor keeps two database
entries for that PID and picks the right one by matching the hub's reported firmware
version (`1.7` selects the HID path, `1.0` selects the vendor path), not a manual
setting. If the device does not appear, verify that `udev/99-hypercolor.rules` is
installed and that you restarted the daemon after plugging in the hub.
{% </callout> %}

The Uni Hub AL V2 (PID `0xA104`) is a separate device and always uses the HID transport;
the firmware split applies only to the original AL.

---

## Corsair NXP/CUE keyboards and mice: firmware layout

The older Corsair generation (K65, K70, and K95 RGB, the Lux and MK.2 boards, K95
Platinum, Strafe RGB, K55, M65, Scimitar, the MM800 Polaris, and the ST100) is
driven through its vendor command interface over hidraw, so the keyboard or mouse
keeps working while Hypercolor sets its lighting. Hypercolor never detaches the
kernel input driver from these devices. Nobody has run these on hardware with
Hypercolor yet, so reports are welcome.

That interface only exists as a hidraw node on firmware with the two-interface
layout, seen on 3.x firmware. Some 2.0x firmware (seen on a K95 RGB, a Strafe RGB,
and an M65) puts the command interface on an OUT-only endpoint that the kernel does
not bind, so Hypercolor cannot reach the lighting and the device stays unlit. If
newer firmware exists for your model, updating it with Corsair's software may move
it to the two-interface layout.

The K66 has no backlight, and the legacy K65, K70, K90, and K95 boards and the M95
take only USB vendor control requests, which no input-safe path carries. They stay
**Researched**.

---

## Corsair Bragi wireless dongles: not routed

Corsair wireless peripherals connect to the host via a USB receiver dongle.
Hypercolor lists these dongles in the device database, but it does not register
them, so the wireless peripheral connected through one does not receive color
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

{% <callout type="info"> %}
Hypercolor already builds the routed packets a dongle forwards to its paired device.
What is missing is protocol knowledge: how a dongle answers a query for an empty slot,
how it reports a paired device going to sleep, and how a paired device should appear as
its own Hypercolor device. Wired Bragi devices work fully.
{% </callout> %}

---

## Understanding device status in the compatibility matrix

The [compatibility matrix](@/hardware/compatibility.md) uses six status values. Here is
what each means from a user perspective:

| Status | What it means |
|---|---|
| **Supported** | A working driver is compiled in. Plug the device in and it gets discovered and controlled. |
| **In progress** | Active development: a driver is being written or a protocol spec is being finalized. |
| **Planned** | Support is on the roadmap but work has not started. No device currently carries this status. |
| **Researched** | The protocol is documented in the TOML but no driver code exists yet. Community contributions welcome. |
| **Blocked** | A driver skeleton exists on Hypercolor's side, but the device itself prevents control, typically a firmware limitation outside Hypercolor's reach. |
| **Known** | The device is in the database but protocol research has not started. |

The Corsair Bragi dongles above are marked **Researched**: the protocol is mostly
understood, but the parts that route color to a paired device are not yet documented
well enough to build on.

---

## Related pages

- [USB devices](@/hardware/usb-devices.md): udev rules, hidraw access, replug requirements
- [Compatibility matrix](@/hardware/compatibility.md): full supported device list
- [Devices not found](@/troubleshooting/devices-not-found.md): diagnosis steps when a device does not appear
- [Conflicting software](@/hardware/conflicting-software.md): other RGB tools that can claim a device before Hypercolor
