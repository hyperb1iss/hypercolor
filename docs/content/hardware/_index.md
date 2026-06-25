+++
title = "Hardware overview"
description = "The HAL and transport map; how Hypercolor talks to USB, SMBus, and network devices, and links to each setup flow."
sort_by = "weight"
template = "section.html"
weight = 60
+++

![Hypercolor device view showing connected hardware](/img/ui/ui-devices.webp)

Hypercolor talks to RGB hardware through two parallel systems: a **Hardware Abstraction Layer** (HAL) for local devices connected over USB or SMBus/I2C, and a set of **network driver crates** for LAN and cloud-connected devices. Both converge on the same render pipeline, so your effects see one unified canvas and the drivers handle the rest.

This section covers how the transport stack is organized, what device families are supported, and how to get each one working. The [compatibility matrix](@/hardware/compatibility.md) is the authoritative device list; start there if you want to confirm whether your hardware is supported.

---

## Device categories

### USB and SMBus devices

The `hypercolor-hal` crate implements protocol encoding for every device that plugs into your machine or sits on the system bus. It exports a `Protocol` trait (pure encoding, no I/O) and a `Transport` trait (async byte-level I/O) as separate layers, so the wire format for a Razer keyboard never reaches into network code, and the SMBus probe for an ASUS motherboard never touches USB.

Transport types live in the `TransportType` enum in `hal/src/registry.rs` and are the authoritative taxonomy:

| Transport | Used by |
|---|---|
| `UsbHidApi` | Live input devices (keyboards, mice) — keeps the OS HID stack attached without claiming the interface |
| `UsbHidRaw` | Linux `/dev/hidraw*` nodes — direct feature/output reports without claiming the interface |
| `UsbControl` | HID feature reports over USB control transfers |
| `UsbHid` | HID interrupt endpoint transport |
| `UsbBulk` | Bulk-transfer transport with HID feature-report sideband (init/keepalive) |
| `UsbMidi` | Composite: MIDI control + USB bulk display (Ableton Push 2 pad display) |
| `UsbSerial` | CDC-ACM serial (Dygma Defy — currently blocked) |
| `I2cSmBus` | Linux `/dev/i2c-*` — ASUS Aura motherboard, GPU, and DRAM lighting |
| `UsbVendor` | Vendor-specific control transfers (Lian Li AL v1.0) |

Getting USB devices working on Linux requires udev rules. Getting SMBus/I2C devices working requires the `i2c-dev` kernel module and correct permissions on `/dev/i2c-*`. These are separate permission paths with separate failure modes. See [USB devices](@/hardware/usb-devices.md) and [SMBus/I2C](@/hardware/smbus-i2c.md) for each.

### Network devices

Network drivers live in separate crates behind the `hypercolor-driver-api` trait boundary. The `hypercolor-network` crate provides only the registry and orchestration shell; protocol logic belongs to each driver crate. Current network backends:

| Driver | Protocol | Discovery | Notes |
|---|---|---|---|
| Hue | Hue Entertainment API over DTLS | mDNS `_hue._tcp.local.`, then N-UPnP fallback | Requires link-button pairing; DTLS streams on port 2100 |
| Nanoleaf | HTTP pairing + UDP External Control | mDNS `_nanoleafapi._tcp.local.` | Hold power button 5-7 s to enter pairing mode |
| WLED | DDP (default) or E1.31/sACN | mDNS `_wled._tcp.local.` | No authentication needed |
| Govee | LAN UDP + optional cloud API | UDP multicast `239.255.255.250:4001` | LAN control must be enabled in the Govee Home app first |

See [network devices](@/hardware/network-devices.md) for the setup overview, then follow the per-vendor guide for pairing details.

### LCD and display devices

Some hardware carries a small LCD or display panel that Hypercolor can drive alongside its RGB zones. The Corsair AIO LCD modules — Elite Capellix LCD (PIDs `0x0C39`, `0x0C33`) and iCUE LINK LCD (PID `0x0C4E`) — stream 480×480 JPEG frames chunked over HID at up to 30 fps. The Ableton Push 2 uses a composite MIDI + bulk transport for its pad-grid lighting and display. These are treated as ordinary device zones in Hypercolor; effect output is composited from the same canvas as everything else.

### OpenRGB bridge

For hardware that Hypercolor does not yet support natively, the OpenRGB fallback bridge connects to a user-managed OpenRGB SDK server (default port 6742) and routes frames through it. This lets you bring nearly any device into the render pipeline while native support is in progress. See [OpenRGB fallback](@/hardware/openrgb-fallback.md) for configuration and caveats around device ownership.

---

## Transport architecture

The render pipeline delivers zone colors to the HAL; the HAL turns them into wire-format packets and ships them to the device:

{% mermaid() %}
graph TD
    C[SparkleFlinger canvas] --> S[SpatialEngine]
    S -->|per-zone colors| BM[BackendManager]
    BM --> HAL[hypercolor-hal]
    BM --> NET[Network drivers]
    HAL --> USB[USB transports]
    HAL --> SMB[SMBus transport]
    NET --> HUE[Hue DTLS :2100]
    NET --> NL[Nanoleaf UDP :60222]
    NET --> WLED[WLED DDP :4048]
    NET --> GV[Govee LAN UDP :4003]
    NET --> ORG[OpenRGB SDK :6742]
    USB --> Razer[Razer peripherals]
    USB --> Corsair[Corsair devices]
    USB --> ASUS_USB[ASUS USB HID]
    USB --> LianLi[Lian Li hubs]
    USB --> Other[QMK · PrismRGB · Nollie · Push 2]
    SMB --> ASUS_SMB[ASUS motherboard / GPU / DRAM]
{% end %}

Device fingerprints are stable across reconnects: USB devices key on VID/PID plus descriptor heuristics; network devices key on MAC address (WLED: `net:<mac>`, Govee: `net:govee:<mac>`) or bridge serial (Hue, Nanoleaf). A DHCP IP change does not lose pairing.

---

## Supported hardware

The full device list lives in the [compatibility matrix](@/hardware/compatibility.md), which is generated from `data/drivers/vendors/*.toml` (32 vendor files). The summary below shows the driver families and their current status.

| Family | Transport | Highlights | Status |
|---|---|---|---|
| **Razer** | USB HID | Keyboards, mice, mousepads, laptops, headsets | Supported |
| **Corsair** | USB HID | Peripherals (Bragi + legacy HID), Lighting Node, iCUE LINK hub, LCD modules | Supported |
| **ASUS** | USB HID + SMBus/I2C | Aura USB peripherals; motherboard/GPU/DRAM Aura over SMBus | Supported |
| **Lian Li** | USB HID / USB Vendor | Uni Hub (ENE), TL Fan Hub — AL v1.0 uses vendor transport, v1.7+ uses HID | Supported |
| **PrismRGB** | USB HID | Custom chunked protocol; Prism 8 is a Nollie 8 v2 rebrand | Supported |
| **Nollie** | USB HID | ARGB controllers; distinct from PrismRGB despite overlapping SKUs | Supported |
| **QMK** | USB HID | Raw HID on any QMK keyboard | Supported |
| **Ableton Push 2** | USB MIDI + USB Bulk | Pad/button RGB via MIDI; display via bulk JPEG stream | Supported |
| **Philips Hue** | Network / DTLS | Entertainment API, gamut mapping, DTLS PSK streaming | Supported |
| **Nanoleaf** | Network / UDP | mDNS discovery, token pairing, UDP External Control | Supported |
| **WLED** | Network / UDP | DDP and E1.31/sACN; RGB and RGBW; no authentication | Supported |
| **Govee** | Network / UDP + Cloud | LAN UDP control; optional cloud API fallback | Supported |
| **OpenRGB bridge** | Network / TCP | Fallback for any hardware OpenRGB supports | Supported |
| **Dygma Defy** | USB Serial | Driver ready; lighting gated by firmware — not yet enabled | Blocked |

{% callout(type="warning") %}
If another RGB manager (OpenRGB, Aura Sync, openrazer daemon, iCUE via Wine) has a USB device open, Hypercolor cannot claim it. The device will appear in `lsusb` but not in `hypercolor devices list`. Close or disable the conflicting tool first. See [conflicting software](@/hardware/conflicting-software.md).
{% end %}

---

## Device discovery

Hypercolor discovers devices automatically at startup and whenever a rescan is triggered.

**USB/SMBus:** scans for known VID/PID combinations and probes `/dev/i2c-*` bus nodes at startup. Hotplug events trigger rescan automatically when the daemon is running.

**Network:** runs mDNS browsing for each network driver's service type. Devices can also be added by IP address in the config (`known_ips` per driver section) for networks where mDNS is blocked across VLANs.

Trigger a manual scan:

```bash
# CLI — filter by target type
hypercolor devices discover
hypercolor devices discover --target wled --target hue

# REST — one scan at a time; concurrent requests return 409
curl -X POST http://localhost:9420/api/v1/devices/discover \
  -H 'Content-Type: application/json' \
  -d '{"targets": ["wled", "hue"], "timeout_ms": 5000}'
```

If a device does not appear after discovery, see [devices not found](@/troubleshooting/devices-not-found.md) for a transport-specific diagnosis checklist.

---

## Setup guides

Each transport path has its own setup page because the failure modes are different:

- [USB devices](@/hardware/usb-devices.md) — udev rules, hidraw vs hidapi, replug, hotplug
- [SMBus/I2C](@/hardware/smbus-i2c.md) — ASUS Aura motherboard, GPU, DRAM lighting; `i2c-dev` module; `/dev/i2c-*` permissions
- [Network devices](@/hardware/network-devices.md) — mDNS discovery, known-IP config, pairing overview
- [Philips Hue](@/hardware/hue.md) — link-button pairing, DTLS streaming
- [Nanoleaf](@/hardware/nanoleaf.md) — power-button pairing, panel layout
- [WLED](@/hardware/wled.md) — DDP vs E1.31, RGB vs RGBW
- [Govee](@/hardware/govee.md) — LAN control setup, Razer-streaming SKUs, cloud API key
- [OpenRGB fallback](@/hardware/openrgb-fallback.md) — bridge config, ownership modes
- [Device quirks](@/hardware/device-quirks.md) — rebrands, firmware splits, known edge cases
- [Conflicting software](@/hardware/conflicting-software.md) — openrazer, OpenRGB, Aura Sync, iCUE

---

## Adding a driver

Driver modules in `hypercolor-hal` are organized by silicon/OEM family, not by retail branding. Rebranded SKUs are model-enum variants within an existing driver, not new modules. New USB drivers implement the `Protocol` trait (pure encoding) and register a `DeviceDescriptor` with the appropriate `TransportType`; new network drivers implement the `hypercolor-driver-api` traits and register with the `DriverModuleRegistry`.

See [adding a driver](@/contributing/adding-a-driver.md) and [adding a network driver](@/contributing/adding-a-network-driver.md) for the full implementation checklist and wire-format conventions.
