+++
title = "What Hypercolor controls (and what it doesn't)"
description = "Which devices Hypercolor drives today, what it does not control yet, and how to find out whether your hardware is supported."
weight = 50
+++

Hypercolor is an RGB orchestration engine: it sends lighting frames to physical hardware in real time. Before spending an hour troubleshooting a device that cannot work yet, it helps to know exactly which hardware is in scope today and which is not.

## What Hypercolor controls

Hypercolor drives hardware through two transport layers.

**USB/HID devices** connect over USB and are discovered automatically when you plug them in, provided the [udev rules](@/guide/installation.md) are in place. Supported driver families today:

| Driver family | Examples |
|---|---|
| `razer` | BlackWidow, Huntsman, Basilisk, DeathAdder, Leviathan V2, and 60+ more |
| `corsair` | K100/K70 keyboards, Commander Pro, Lighting Node, iCUE LINK, Corsair mice |
| `nollie` | Nollie 1/2/4/8/16/32, Nollie L1/L2, Matrix, TT — 19 controllers |
| `qmk` | Keychron Q/V series, Glorious GMMK Pro, ZSA Moonlander, ZSA Voyager |
| `asus` | Aura Addressable (Gen 1–4), Aura Motherboard (Gen 1–5), Aura Terminal |
| `lianli` | Uni Hub (SL, SL V2, SL Infinity, AL, AL V2), TL Fan Hub |
| `prismrgb` | Prism S, Prism Mini, Prism 8 (the Prism 8 is a Nollie 8 v2 rebrand — shows up correctly) |
| `push2` | Ableton Push 2 (MIDI pads + sideband display) |
| `dygma` | Dygma Defy wired/wireless — driver present, **blocked** (see below) |

**Network devices** are discovered over your LAN (mDNS for most, a UDP multicast scan for Govee), then driven in real time over UDP or HTTP:

| Driver | Protocol | Discovery |
|---|---|---|
| `hue` | Philips Hue Entertainment API over DTLS | mDNS + link-button pairing |
| `nanoleaf` | HTTP + UDP external control | mDNS + power-button pairing |
| `wled` | DDP / E1.31 sACN | mDNS (`_wled._tcp`) |
| `govee` | Govee LAN UDP | UDP multicast scan (LAN control must be enabled in the Govee Home app first) |

For the full list with every supported PID and device note, see the [compatibility matrix](@/hardware/compatibility.md).

## What Hypercolor does NOT control (yet)

### RAM RGB

Hypercolor does not yet control DDR RGB. The SMBus transport layer is implemented and RAM devices from Corsair (Dominator, Dominator Titanium) are researched and in the database, with the wire protocol documented. A driver has not shipped. Until it does, DRAM lighting is outside Hypercolor's scope.

### GPU RGB

GPU RGB via SMBus is researched for several families (ASUS Aura GPU via ENE SMBus, EVGA Pascal/Turing/Ampere, Gigabyte GPU across four generations, and MSI Lovelace), with ASRock AMD GPUs further back at the Known stage. None have a shipping driver. The SMBus transport exists in the codebase; the per-vendor protocol implementations do not. Your GPU will not appear in `hypercolor devices list`.

{% callout(type="info") %}
SMBus devices (RAM and GPU RGB) require the `i2c-dev` kernel module and i2c group membership. Hypercolor's udev rules already cover the `i2c-dev` subsystem, so once per-vendor drivers ship the access plumbing is in place. Watch the compatibility matrix for status changes.
{% end %}

### Blocked devices: Dygma Defy

The Dygma Defy (wired `0x0010` and wireless `0x0012`) has a complete driver implementation in `hypercolor-hal`, covering the wire protocol, zone mapping, and RGBW color support. It is blocked, not missing. The stock Defy firmware does not expose a non-persistent direct LED streaming path that Hypercolor can use. The driver probes for a `hypercolor.capabilities` command and logs once per connection that live frame writes are being dropped. Until Dygma ships firmware support for external RGB control over the Focus protocol, the Defy will appear as discovered but unresponsive to lighting.

### Devices in "Researched" or "Known" state

The [compatibility matrix](@/hardware/compatibility.md) uses five status levels:

| Status | Meaning |
|---|---|
| **Supported** | Driver ships with Hypercolor; plug in and it works |
| **In progress** | Driver code exists but is not yet merged or feature-complete |
| **Researched** | Protocol documented, driver not yet written |
| **Blocked** | Driver exists but cannot function without changes outside Hypercolor |
| **Known** | Device on the list; protocol not yet researched |

If your device is Researched or Known, it is a candidate for a contributed driver. See [adding a driver](@/contributing/adding-a-driver.md) if you want to accelerate it.

### Devices controlled by another RGB manager

If openrazer daemon, OpenRGB, Aura Sync, iCUE, or any other tool has the USB device open, Hypercolor's USB driver will not be able to connect to it. This is a kernel-level exclusion: only one process can hold a HID device at a time. If a device appears in `lsusb` but not in `hypercolor devices list`, check whether another RGB tool is running and holding it.

```bash
# Check which process holds your Razer device (adjust VID:PID as needed)
lsof /dev/hidraw* 2>/dev/null | grep -i razer
```

See [conflicting software](@/hardware/conflicting-software.md) for the full list of known conflicts and how to resolve them.

## Checking your specific device

The fastest path:

```bash
# List devices Hypercolor has detected
hypercolor devices list

# Run discovery manually (useful for network devices)
hypercolor devices discover

# Full health and device report
hypercolor diagnose
```

If a USB device is missing, confirm udev rules are installed (`just udev-install` on a source build, or check your package's post-install instructions), then re-plug the device. Network devices that are not discovered usually need their LAN control enabled in their companion app before mDNS broadcasting starts.

For a step-by-step device setup walkthrough, see [finding devices](@/guide/finding-devices.md).
