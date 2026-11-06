+++
title = "Hardware overview"
description = "The HAL and transport map; how Hypercolor talks to USB, SMBus, and network devices, and links to each setup flow."
sort_by = "weight"
template = "section.html"
weight = 60
+++

{{< img path="img/ui/ui-devices.webp" alt="Hypercolor device view showing connected hardware" />}}

Hypercolor talks to RGB hardware through two parallel systems: a **Hardware Abstraction Layer** (HAL) for local devices connected over USB or SMBus/I2C, and a set of **network driver crates** for LAN and cloud-connected devices. Both converge on the same render pipeline, so your effects see one unified canvas and the drivers handle the rest.

This section covers how the transport stack is organized, what device families are supported, and how to get each one working. The [compatibility matrix](@/hardware/compatibility.md) is the authoritative device list; start there if you want to confirm whether your hardware is supported.

---

## Device categories

### USB and SMBus devices

The `hypercolor-hal` crate implements protocol encoding for every device that plugs into your machine or sits on the system bus. It exports a `Protocol` trait (pure encoding, no I/O) and a `Transport` trait (async byte-level I/O) as separate layers, so the wire format for a Razer keyboard never reaches into network code, and the SMBus probe for an ASUS motherboard never touches USB.

Transport types live in the `TransportType` enum in `hal/src/registry.rs` and are the authoritative taxonomy:

| Transport | Used by |
|---|---|
| `UsbHidApi` | Live input devices (keyboards, mice); keeps the OS HID stack attached without claiming the interface |
| `UsbHidRaw` | Linux `/dev/hidraw*` nodes; direct feature/output reports without claiming the interface |
| `UsbControl` | HID feature reports over USB control transfers |
| `UsbHid` | HID interrupt endpoint transport |
| `UsbBulk` | Bulk-transfer transport with HID feature-report sideband (init/keepalive) |
| `UsbMidi` | Composite: MIDI control + USB bulk display (Ableton Push 2 pad display) |
| `UsbSerial` | CDC-ACM serial (Dygma Defy, currently blocked) |
| `I2cSmBus` | Linux `/dev/i2c-*` (PawnIO on Windows); ASUS Aura motherboard, GPU, and DRAM lighting |
| `UsbVendor` | Vendor-specific control transfers (Lian Li AL v1.0) |

Getting USB devices working on Linux requires udev rules. Getting SMBus/I2C devices working on Linux requires the `i2c-dev` kernel module and correct permissions on `/dev/i2c-*`. These are separate permission paths with separate failure modes. See [USB devices](@/hardware/usb-devices.md) and [SMBus/I2C](@/hardware/smbus-i2c.md) for each.

### Serial LED controllers

DIY strips behind an Arduino, ESP8266, ESP32, or RP2040 running Adalight or HyperSerial (AWA) firmware are driven by the `adalight` driver crate. These boards use generic USB-serial chips, so they are never detected by USB id: you list each board's port, baud rate, protocol, and LED count in the config, and Hypercolor opens only those ports. Output is paced to what the serial line can carry. See [Adalight serial controllers](@/hardware/adalight.md).

### Network devices

Network drivers live in separate crates behind the `hypercolor-driver-api` trait boundary. The `hypercolor-network` crate provides only the registry and orchestration shell; protocol logic belongs to each driver crate. Current network backends:

| Driver | Protocol | Discovery | Notes |
|---|---|---|---|
| Hue | Hue Entertainment API over DTLS | mDNS `_hue._tcp.local.`, then N-UPnP fallback | Requires link-button pairing; DTLS streams on port 2100 |
| Nanoleaf | HTTP pairing + UDP External Control | mDNS `_nanoleafapi._tcp.local.` | Hold power button 5-7 s to enter pairing mode |
| WLED | DDP (default) or E1.31/sACN | mDNS `_wled._tcp.local.` | No authentication needed |
| Govee | LAN UDP + optional cloud API | UDP multicast `239.255.255.250:4001` | LAN control must be enabled in the Govee Home app first |
| Pixel controllers | E1.31 (sACN), Art-Net 4, or DDP over unicast UDP | None: devices are listed in config | Falcon, FPP, ESPixelStick, and DMX nodes; experimental |
| LIFX | LIFX LAN protocol over UDP | UDP broadcast `GetService` to port 56700 | No authentication; experimental, not yet verified on hardware |
| Twinkly | Local REST API + real-time UDP frames | UDP broadcast discovery datagram to port 5555 | No pairing; experimental, not yet verified on hardware |
| WiZ | Local UDP JSON (`setPilot`) | UDP broadcast registration to port 38899 | No pairing; homes with verified controls need the signing key; experimental |
| Yeelight | LAN Control: TCP JSON plus music mode | SSDP-style search to `239.255.255.250:1982` | LAN Control must be enabled per light in the Yeelight app; experimental |
| OpenRGB bridge | OpenRGB SDK over TCP | Connects on demand to the Hypercolor-managed server on `127.0.0.1:6742` | Fallback bridge; drives only hardware no enabled native driver covers; disabled by default |

The ROLI Blocks bridge is another registered driver module, but it speaks to a local `blocksd` Unix socket rather than the network. Both bridges are covered further down this page.

See [network devices](@/hardware/network-devices.md) for the setup overview, then follow the per-vendor guide for pairing details.

### LCD and display devices

Some hardware carries a small LCD or display panel that Hypercolor can drive alongside its RGB zones. The Corsair LCD modules, Elite Capellix (`0x0C39`, `0x0C33`), iCUE LINK (`0x0C4E`), Nautilus RS (`0x0C55`), XC7 RGB Elite (`0x0C42`), and XD6 Elite (`0x0C43`), stream 480×480 JPEG frames over USB HID/HIDAPI reports at up to 30 fps. The Ableton Push 2 uses a composite MIDI + bulk transport for its pad-grid lighting and display. These are treated as ordinary device outputs in Hypercolor; effect output is composited from the same canvas as everything else.

### OpenRGB bridge

For hardware that Hypercolor does not yet support natively, the OpenRGB fallback bridge routes frames through an OpenRGB SDK server on `127.0.0.1:6742`. Hypercolor runs that server itself (`hypercolor openrgb start`) against a config directory it owns, with a detector partition that keeps OpenRGB away from every device a native driver drives; a conflict guard output-disables any bridge route that still lands on natively owned silicon. An OpenRGB you start by hand from its own config is a different thing: it detects everything and competes with native drivers, so treat it as [conflicting software](@/hardware/conflicting-software.md). `hypercolor devices coverage` shows which stack owns each device. See [OpenRGB fallback](@/hardware/openrgb-fallback.md) for the install matrix, managed server, zone sizes, and the hand-off flow, and [My device isn't supported](@/hardware/unsupported-devices.md) for hardware neither stack lights.

---

## Transport architecture

The render pipeline delivers zone colors to the HAL; the HAL turns them into wire-format packets and ships them to the device:

{% <mermaid> %}
graph TD
    C[SparkleFlinger canvas] --> S[SpatialEngine]
    S -->|per-zone colors| BM[BackendManager]
    BM --> HAL[hypercolor-hal]
    BM --> NET[Network drivers]
    BM --> ADA[Adalight serial]
    HAL --> USB[USB transports]
    HAL --> SMB[SMBus transport]
    NET --> HUE[Hue DTLS :2100]
    NET --> NL[Nanoleaf UDP :60222]
    NET --> WLED[WLED DDP :4048]
    NET --> GV[Govee LAN UDP :4003]
    NET --> LX[LIFX LAN UDP :56700]
    NET --> TW[Twinkly real-time UDP :7777]
    NET --> WZ[WiZ UDP :38899]
    NET --> YL[Yeelight TCP :55443]
    NET --> ORG[OpenRGB SDK :6742]
    USB --> Razer[Razer peripherals]
    USB --> Corsair[Corsair devices]
    USB --> ASUS_USB[ASUS USB HID]
    USB --> LianLi[Lian Li hubs]
    USB --> Other[QMK · PrismRGB · Nollie · Push 2]
    SMB --> ASUS_SMB[ASUS motherboard / GPU / DRAM]
{% </mermaid> %}

Device fingerprints are stable across reconnects: USB devices key on VID/PID plus descriptor heuristics; network devices key on MAC address (WLED: `net:wled:<mac>`, Govee: `net:govee:<mac>`, LIFX: `net:lifx:<serial>`) or bridge serial (Hue, Nanoleaf). A DHCP IP change does not lose pairing.
Device fingerprints are stable across reconnects: USB devices key on VID/PID plus descriptor heuristics; network devices key on MAC address (WLED: `net:wled:<mac>`, Govee: `net:govee:<mac>`, Twinkly: `net:twinkly:<mac>`) or bridge serial (Hue, Nanoleaf). A DHCP IP change does not lose pairing.
Device fingerprints are stable across reconnects: USB devices key on VID/PID plus descriptor heuristics; network devices key on MAC address (WLED: `net:wled:<mac>`, Govee: `net:govee:<mac>`, Twinkly: `net:twinkly:<mac>`, WiZ: `net:wiz:<mac>`) or bridge serial (Hue, Nanoleaf). A DHCP IP change does not lose pairing.
Device fingerprints are stable across reconnects: USB devices key on VID/PID plus descriptor heuristics; network devices key on MAC address (WLED: `net:wled:<mac>`, Govee: `net:govee:<mac>`, Twinkly: `net:twinkly:<mac>`, WiZ: `net:wiz:<mac>`) or device id (Yeelight: `net:yeelight:<id>`) or bridge serial (Hue, Nanoleaf). A DHCP IP change does not lose pairing.
Device fingerprints are stable across reconnects: USB devices key on VID/PID plus descriptor heuristics; network devices key on MAC address (WLED: `net:wled:<mac>`, Govee: `net:govee:<mac>`) or bridge serial (Hue, Nanoleaf). A DHCP IP change does not lose pairing. Adalight serial boards key on the `id` from their config entry (`usb:adalight:<id>`), so moving a board to another port keeps its layout.

---

## Supported hardware

The full device list lives in the [compatibility matrix](@/hardware/compatibility.md), which is generated from `data/drivers/vendors/*.toml` (32 vendor files). The summary below shows the driver families and their current status.

| Family | Transport | Highlights | Status |
|---|---|---|---|
| **Razer** | USB HID | Keyboards, mice, mousepads, laptops, headsets | Supported |
| **Corsair** | USB HID | Peripherals (Bragi and NXP/CUE), Lighting Node, iCUE LINK hub, LCD modules | Supported |
| **ASUS** | USB HID + SMBus/I2C | Aura USB peripherals; motherboard/GPU/DRAM Aura over SMBus | Supported |
| **Lian Li** | USB HID / USB Vendor | Uni Hub (ENE), TL Fan Hub; AL firmware 1.0 uses vendor transport, 1.7 uses HID | Supported |
| **PrismRGB** | USB HID | Custom chunked protocol; Prism 8 is a Nollie 8 v2 rebrand | Supported |
| **Nollie** | USB HID | ARGB controllers; distinct from PrismRGB despite overlapping SKUs | Supported |
| **QMK** | USB HID | Raw HID on any QMK keyboard | Supported |
| **Ableton Push 2** | USB MIDI + USB Bulk | Pad/button RGB via MIDI; display via bulk JPEG stream | Supported |
| **Philips Hue** | Network / DTLS | Entertainment API, gamut mapping, DTLS PSK streaming | Supported |
| **Nanoleaf** | Network / UDP | mDNS discovery, token pairing, UDP External Control | Supported |
| **WLED** | Network / UDP | DDP and E1.31/sACN; RGB and RGBW; no authentication | Supported |
| **Govee** | Network / UDP + Cloud | LAN UDP control; optional cloud API fallback | Supported |
| **LIFX** | Network / UDP | Bulbs, multizone strips and beams, Tile chains, Candle, Ceiling, and other matrix products over the LAN protocol | Experimental (unverified on hardware) |
| **Twinkly** | Network / HTTP + UDP | Per-LED strings, curtains, Squares, and trees; layout from the Twinkly app | Experimental (unverified on hardware) |
| **WiZ** | Network / UDP | Color, tunable white, and dimmable white bulbs, strips, and fixtures, one color per light | Experimental (unverified on hardware) |
| **Yeelight** | Network / TCP | Color and white bulbs, strips, and lamps, one color per light; music mode for real-time streaming | Experimental (unverified on hardware) |
| **OpenRGB bridge** | Network / TCP | Fallback for any hardware OpenRGB supports | Supported (opt-in) |
| **[ROLI Blocks bridge](@/hardware/roli-blocks.md)** | Unix socket / blocksd | Lightpad grids and 24 LUMI key colors | Supported with matching blocksd capabilities (Unix only) |
| **[Adalight serial](@/hardware/adalight.md)** | Serial port | Adalight sketches, WLED serial input, and HyperSerial (AWA) on Arduino, ESP8266, ESP32, RP2040; configured by hand | Supported (experimental) |
| **Dygma Defy** | USB Serial | Driver ready; lighting gated by firmware, not yet enabled | Blocked |

Neither bridge is counted among the 13 driver families with shipping device support. LIFX, the newest of the 13, ships enabled but is labeled experimental until someone verifies it on real lights. The OpenRGB bridge ships compiled in but its config entry is minted disabled; run `hypercolor devices coverage` first and enable it per [OpenRGB fallback](@/hardware/openrgb-fallback.md) only when a device reads `unclaimed` or `none`. The ROLI Blocks bridge scans by default (`discovery.blocks_scan = true`) and finds devices only when a `blocksd` socket is present; set `discovery.blocks_socket_path` if yours is not in the default location. Dygma is the fourteenth family implemented in the tree; it is excluded from the thirteen because no Dygma device lights up yet.
Neither bridge is counted among the 13 driver families with shipping device support. Twinkly, one of the 13, ships enabled but is labeled experimental until someone verifies it on real lights. The OpenRGB bridge ships compiled in but its config entry is minted disabled; run `hypercolor devices coverage` first and enable it per [OpenRGB fallback](@/hardware/openrgb-fallback.md) only when a device reads `unclaimed` or `none`. The ROLI Blocks bridge scans by default (`discovery.blocks_scan = true`) and finds devices only when a `blocksd` socket is present; set `discovery.blocks_socket_path` if yours is not in the default location. Dygma is the fourteenth family implemented in the tree; it is excluded from the thirteen because no Dygma device lights up yet.
Neither bridge is counted among the 14 driver families with shipping device support. Twinkly and WiZ, two of the 14, ship enabled but are labeled experimental until someone verifies them on real lights. The OpenRGB bridge ships compiled in but its config entry is minted disabled; run `hypercolor devices coverage` first and enable it per [OpenRGB fallback](@/hardware/openrgb-fallback.md) only when a device reads `unclaimed` or `none`. The ROLI Blocks bridge scans by default (`discovery.blocks_scan = true`) and finds devices only when a `blocksd` socket is present; set `discovery.blocks_socket_path` if yours is not in the default location. Dygma is the fifteenth family implemented in the tree; it is excluded from the fourteen because no Dygma device lights up yet.
Neither bridge is counted among the 15 driver families with shipping device support. Twinkly, WiZ, and Yeelight, three of the 15, ship enabled but are labeled experimental until someone verifies them on real lights. The OpenRGB bridge ships compiled in but its config entry is minted disabled; run `hypercolor devices coverage` first and enable it per [OpenRGB fallback](@/hardware/openrgb-fallback.md) only when a device reads `unclaimed` or `none`. The ROLI Blocks bridge scans by default (`discovery.blocks_scan = true`) and finds devices only when a `blocksd` socket is present; set `discovery.blocks_socket_path` if yours is not in the default location. Dygma is the sixteenth family implemented in the tree; it is excluded from the fifteen because no Dygma device lights up yet.

{% <callout type="warning"> %}
If another RGB manager (Aura Sync, openrazer daemon, iCUE via Wine, or an OpenRGB you started yourself) has a USB device open, Hypercolor cannot claim it. The device will appear in `lsusb` but not in `hypercolor devices list`. Close or disable the conflicting tool first. The OpenRGB server Hypercolor manages is exempt: it is configured to skip natively owned devices. See [conflicting software](@/hardware/conflicting-software.md).
{% </callout> %}

---

## Device discovery

Hypercolor discovers devices automatically at startup and whenever a rescan is triggered.

**USB/SMBus:** scans for known VID/PID combinations and probes `/dev/i2c-*` bus nodes at startup. Hotplug events trigger rescan automatically when the daemon is running.

**Network:** runs mDNS browsing for each network driver's service type. Devices can also be added by IP address in the config; the key differs per driver (`known_ips` for WLED and Govee, `bridge_ips` for Hue, `device_ips` for Nanoleaf and LIFX) for networks where mDNS is blocked across VLANs. See [network discovery troubleshooting](@/troubleshooting/network-discovery.md) for the exact snippets.
**Network:** runs mDNS browsing for each network driver's service type. Devices can also be added by IP address in the config; the key differs per driver (`known_ips` for WLED and Govee, `bridge_ips` for Hue, `device_ips` for Nanoleaf and Twinkly) for networks where mDNS is blocked across VLANs. See [network discovery troubleshooting](@/troubleshooting/network-discovery.md) for the exact snippets.
**Network:** runs mDNS browsing for each network driver's service type. Devices can also be added by IP address in the config; the key differs per driver (`known_ips` for WLED and Govee, `bridge_ips` for Hue, `device_ips` for Nanoleaf, Twinkly, and WiZ) for networks where mDNS is blocked across VLANs. See [network discovery troubleshooting](@/troubleshooting/network-discovery.md) for the exact snippets.
**Network:** runs mDNS browsing for each network driver's service type. Devices can also be added by IP address in the config; the key differs per driver (`known_ips` for WLED and Govee, `bridge_ips` for Hue, `device_ips` for Nanoleaf, Twinkly, WiZ, and Yeelight) for networks where mDNS is blocked across VLANs. See [network discovery troubleshooting](@/troubleshooting/network-discovery.md) for the exact snippets.

**Serial:** never scanned. Each discovery pass turns the `drivers.adalight.devices` entries into devices without opening a port; the port opens only when the device connects.

Trigger a manual scan:

```bash
# CLI: filter by target type
hypercolor devices discover
hypercolor devices discover --target wled --target hue

# REST: one scan at a time; concurrent requests return 409
curl -X POST http://localhost:9420/api/v1/devices/discover \
  -H 'Content-Type: application/json' \
  -d '{"targets": ["wled", "hue"], "timeout_ms": 5000}'
```

If a device does not appear after discovery, see [devices not found](@/troubleshooting/devices-not-found.md) for a transport-specific diagnosis checklist.

---

## Setup guides

Each transport path has its own setup page because the failure modes are different:

- [USB devices](@/hardware/usb-devices.md): udev rules, hidraw vs hidapi, replug, hotplug
- [SMBus/I2C](@/hardware/smbus-i2c.md): ASUS Aura motherboard, GPU, DRAM lighting; `i2c-dev` module; `/dev/i2c-*` permissions
- [Adalight serial controllers](@/hardware/adalight.md): port paths, baud rate and protocol, serial bandwidth limits, RGBW white handling
- [Network devices](@/hardware/network-devices.md): mDNS discovery, known-IP config, pairing overview
- [Philips Hue](@/hardware/hue.md): link-button pairing, DTLS streaming
- [Nanoleaf](@/hardware/nanoleaf.md): power-button pairing, panel layout
- [WLED](@/hardware/wled.md): DDP vs E1.31, RGB vs RGBW
- [Govee](@/hardware/govee.md): LAN control setup, Razer-streaming SKUs, cloud API key
- [LIFX](@/hardware/lifx.md): broadcast discovery, zone and tile mapping, the message budget
- [Twinkly](@/hardware/twinkly.md): discovery, the one-login rule, layouts, frame rate
- [WiZ](@/hardware/wiz.md): broadcast discovery, the verified-controls signing key, update rate
- [Yeelight](@/hardware/yeelight.md): LAN Control, music mode, the command quota
- [OpenRGB fallback](@/hardware/openrgb-fallback.md): coverage view, per-platform install, the managed server and detector partition, zone sizes
- [My device isn't supported](@/hardware/unsupported-devices.md): native status, unclaimed hardware, the bridge, and a prefilled device-support request
- [Device quirks](@/hardware/device-quirks.md): rebrands, firmware splits, known edge cases
- [Conflicting software](@/hardware/conflicting-software.md): openrazer, hand-run OpenRGB, Aura Sync, iCUE

---

## Adding a driver

Driver modules in `hypercolor-hal` are organized by silicon/OEM family, not by retail branding. Rebranded SKUs are model-enum variants within an existing driver, not new modules. New USB drivers implement the `Protocol` trait (pure encoding) and register a `DeviceDescriptor` with the appropriate `TransportType`; new network drivers implement the `hypercolor-driver-api` traits and register with the `DriverModuleRegistry`.

See [adding a driver](@/contributing/adding-a-driver.md) and [adding a network driver](@/contributing/adding-a-network-driver.md) for the full implementation checklist and wire-format conventions.
