+++
title = "Pixel controllers (sACN, Art-Net, DDP)"
description = "Stream to Falcon, FPP, ESPixelStick, and DMX nodes over E1.31 (sACN), Art-Net 4, or DDP. Devices are configured by hand: host, protocol, universe or offset, and pixel counts."
weight = 85
+++

The pixel controller driver streams to anything that accepts one of the three open lighting protocols: **E1.31 (sACN)**, **Art-Net 4**, and **DDP**. That covers Falcon controllers (F16V4, F16V5), Falcon Player (FPP), ESPixelStick, xLights-style props, and DMX fixtures behind an Art-Net or sACN node.

These controllers do not announce what is wired to them, so you describe each one in the config: where it lives, which protocol it speaks, which universe or offset it starts at, and how many pixels hang off each output. Hypercolor never scans the network for them and never sends multicast or broadcast traffic; every packet is unicast to the host you name.

{% <callout type="warning"> %}
This driver is **experimental**: it is built and tested against the published standards, but nobody has run it on a real controller yet. Reports from real rigs are very welcome.
{% </callout> %}

For WLED controllers, use the dedicated [WLED driver](@/hardware/wled.md) instead. It discovers WLED over mDNS and reads the LED count for you.

Hyperion.ng and HyperHDR send these protocols but do not accept them as input, so they are not targets for this driver.

## Prerequisites

- The controller has a fixed IP address (a DHCP reservation works) that the Hypercolor host can reach.
- The controller's input is set to the protocol you pick, and it accepts **unicast** packets on that protocol's port. Most controllers do by default; if yours only listens on sACN multicast, switch its input to unicast.
- You know the controller's start universe (E1.31 or Art-Net) or start channel (DDP), and the pixel count of each output.

## Add a controller

The driver is enabled by default and does nothing until you list a device. Add entries to `hypercolor.toml` (`hypercolor config path` prints its location):

```toml
[[drivers.pixel.devices]]
id = "tree"                 # stable identity, letters/digits/-/_
name = "Mega Tree"
host = "192.168.1.61"
protocol = "ddp"            # e131 | artnet | ddp
pixel_count = 1000
```

Then run a discovery pass, or restart the daemon:

```bash
hypercolor devices discover --target pixel
hypercolor devices list
```

The device appears like any other, ready for the layout editor. Its identity comes from `id`, so you can rename it or move it to a new IP address without losing its place in your layouts.

## Examples

### Falcon or FPP over E1.31

A controller whose outputs are mapped to consecutive universes starting at 1, with a second run of pixels that starts on a fresh universe:

```toml
[[drivers.pixel.devices]]
id = "roofline"
name = "Roofline Falcon"
host = "192.168.1.60"
protocol = "e131"
start_universe = 1

[[drivers.pixel.devices.segments]]
name = "Front"
pixel_count = 300           # universes 1-2

[[drivers.pixel.devices.segments]]
name = "Garage"
pixel_count = 150
start_universe = 4          # jump instead of continuing in universe 2
```

Each segment becomes its own zone in the layout editor. A segment without `start_universe` continues right after the previous one.

### ESPixelStick or FPP over DDP

DDP has no universes. `ddp_offset` is the zero-based channel where the device starts in the controller's channel space, so a model at start channel 1001 uses `ddp_offset = 1000`:

```toml
[[drivers.pixel.devices]]
id = "porch"
host = "192.168.1.62"
protocol = "ddp"
ddp_offset = 0
color_order = "grbw"        # RGBW pixels; white is extracted from RGB
pixel_count = 150
```

### DMX fixtures behind an Art-Net node

Four RGBW pars at DMX addresses 17, 21, 25, and 29 on Art-Net universe 1. Fixtures that share a universe must be segments (or pixels) of **one** device, because each device sends the whole universe:

```toml
[[drivers.pixel.devices]]
id = "stage-pars"
host = "192.168.1.70"
protocol = "artnet"
start_universe = 1
start_channel = 17
color_order = "rgbw"
pixel_count = 4
max_fps = 44                # the node re-emits real DMX512
```

## Settings reference

Driver-level settings sit directly under `[drivers.pixel]`:

| Setting | Default | Meaning |
|---|---|---|
| `source_name` | `"Hypercolor"` | E1.31 source name receivers display (at most 63 bytes) |
| `priority` | `100` | E1.31 priority, 0-200 |

Per-device settings under `[[drivers.pixel.devices]]`:

| Setting | Applies to | Default | Meaning |
|---|---|---|---|
| `id` | all | required | Stable identity, 1-64 letters, digits, `-` or `_` |
| `name` | all | the id | Display name |
| `host` | all | required | Controller IP address (unicast; IPv4 for Art-Net) |
| `protocol` | all | required | `e131` (or `sacn`), `artnet`, `ddp` |
| `port` | all | 5568, 6454, 4048 | UDP port |
| `color_order` | all | `rgb` | `rgb`, `grb`, `bgr` and the other orders, or `rgbw`, `grbw` and the other white orders |
| `channels_per_pixel` | all | 3 or 4 | Extra channels per pixel are sent as zero |
| `pixel_count` | all | | Pixels on a single-output device |
| `segments` | all | | Outputs, each with `name`, `pixel_count`, and optional `start_universe`, `start_channel`, `ddp_offset` |
| `start_universe` | e131, artnet | `1` | First universe; Art-Net uses the flat 15-bit Port-Address |
| `start_channel` | e131, artnet | `1` | First DMX channel in the start universe |
| `channels_per_universe` | e131, artnet | 510 RGB, 512 RGBW | Channels filled before wrapping to the next universe |
| `ddp_offset` | ddp | `0` | Byte offset of the first channel |
| `priority`, `source_name` | e131 | driver values | Per-device E1.31 overrides |
| `sync_universe` | e131 | none | Send an E1.31 sync packet after every frame so outputs latch together |
| `cid` | e131 | derived from this installation and `id` | Fixed sender UUID |
| `max_fps` | all | follow the render loop | Cap this device's frame rate |

Settings that do not apply to a device's protocol are rejected with a message naming the field, so a typo like a `priority` on a DDP device fails loudly instead of being ignored. One device can stream up to 4,194,304 channels (about 8,200 universes); split anything bigger into several devices.

You can also read and edit the whole device list through the driver's control surface:

```bash
hypercolor drivers controls pixel
```

## Protocol details

| | E1.31 (sACN) | Art-Net 4 | DDP |
|---|---|---|---|
| UDP port | 5568 | 6454 (also the source port) | 4048 |
| Addressing | Universe 1-63999 | Port-Address 0-32767 (Net, Sub-Net, Universe) | 32-bit byte offset |
| Pixels per packet | 170 RGB or 128 RGBW per universe | 170 RGB or 128 RGBW per universe | 480 RGB or 360 RGBW per packet |
| Frame latch | Optional sync packet (`sync_universe`) | None | Push flag on the last packet |
| On disconnect | Black frame, then 3 stream-terminated packets per universe | Black frame | Black frame |

By default 3-channel pixels use 510 channels per universe so no pixel straddles two universes. Set `channels_per_universe = 512` for controllers configured with full 512-channel universes.

## Frame rate

Each device follows the render loop, up to 60 fps. When nothing changes, Hypercolor resends the last frame every 800 ms so receivers do not time out (sACN receivers give up after 2.5 s). For sACN, a frame that stops changing still goes out three times at the render rate before the resends slow down, as the standard asks.

Physical DMX512 tops out around 44 Hz for a full universe. Pixel controllers drive pixels, not DMX lines, and keep up at the full render rate. If a receiver re-emits real DMX512, set `max_fps = 44` on that device.

## Troubleshooting

### The device never shows up

- Check the daemon log for a `pixel` configuration error. One invalid entry stops the whole list from loading, and the message names the device and the field.
- Run `hypercolor devices discover --target pixel` after editing the file.

### The device is connected but nothing lights

- Confirm the controller's input protocol matches `protocol`, and that it accepts unicast on that port.
- Check the start universe or offset against the controller's own config. The usual slip is off by one: E1.31 universes start at 1, and DDP offsets are zero-based.
- Make sure no firewall blocks outbound UDP from the Hypercolor host.

### Colors are swapped

Most pixel controllers set the color order per output. Leave `color_order = "rgb"` and fix it on the controller, or set the matching order here, but not both.

### Two devices flicker against each other

Two devices that send the same universe to the same controller overwrite each other every frame, and the config is rejected. Combine them into one device with a segment per fixture or output.

## Related pages

- [Network devices overview](@/hardware/network-devices.md): how the network drivers find and identify devices.
- [WLED](@/hardware/wled.md): the dedicated driver for WLED controllers.
- [Configuration](@/guide/configuration.md): where `hypercolor.toml` lives and how driver settings are stored.
- [Device compatibility](@/hardware/compatibility.md): the full supported-device matrix.
