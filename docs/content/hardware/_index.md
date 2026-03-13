+++
title = "Hardware"
description = "Supported devices, drivers, and the hardware abstraction layer"
sort_by = "weight"
template = "section.html"
+++

Hypercolor controls RGB devices through a Hardware Abstraction Layer (HAL) that normalizes the wildly different protocols used by each manufacturer into a uniform interface. The HAL lives in the `hypercolor-hal` crate and implements the `Protocol` trait for each device family.

## Supported Hardware

| Brand | Transport | Protocol | Status |
|---|---|---|---|
| **Razer** | USB HID | Chroma HID protocol | Implemented |
| **PrismRGB / Nollie** | USB HID | Custom chunked protocol | Implemented |
| **WLED** | Network (UDP) | DDP / E1.31 (sACN) | Implemented |
| **Lian Li** | USB HID | Uni Hub protocol | Implemented |
| **ASUS** | USB HID / I2C | Aura protocol | Implemented |
| **Ableton Push 2** | USB Bulk | Pad/button RGB protocol | Implemented |
| **ROLI Blocks** | USB HID | Lightpad protocol | In progress |
| **Dygma Defy** | USB HID | Custom keyboard protocol | Implemented |
| **QMK** | USB HID | QMK raw HID | Implemented |
| **Corsair** | USB HID | iCUE protocol | Planned |

## Architecture

The HAL is structured in layers:

{% mermaid() %}
graph TD
    A[Effect Engine] -->|RGB frame| B[Spatial Sampler]
    B -->|per-zone colors| C[Device Registry]
    C --> D[Output Queue]
    D --> E1[Razer Driver]
    D --> E2[PrismRGB Driver]
    D --> E3[WLED Driver]
    D --> E4[ASUS Driver]
    D --> E5[Push 2 Driver]
    E1 -->|USB HID| F1[Razer Keyboard]
    E1 -->|USB HID| F2[Razer Mouse]
    E2 -->|USB HID| F3[Lian Li Strimer]
    E3 -->|UDP DDP| F4[WLED Strip]
    E4 -->|USB/I2C| F5[ASUS Motherboard]
    E5 -->|USB Bulk| F6[Ableton Push 2]
{% end %}

### Key Abstractions

**`Protocol` trait** — Every device family implements this trait to translate RGB color arrays into device-specific wire-format packets.

**`Transport` trait** — Handles the physical communication channel (USB HID, USB Bulk, UDP, I2C). Protocols are transport-agnostic.

**`DeviceRegistry`** — Tracks discovered devices, manages connections, and routes color frames to the correct driver.

**`CommandBuffer`** — Reusable buffer for building device commands without per-frame allocation. Drivers use `push_struct` to write zerocopy packet structs directly into the buffer.

### Zero-Copy Frame Encoding

Frame encoding runs at 30-60 FPS per device. The HAL minimizes allocations in this hot path:

- **Zerocopy packet structs** — Wire-format packets are `#[repr(C)]` structs with `FromZeros` + `IntoBytes` derives. No manual byte-offset indexing.
- **`CommandBuffer::push_struct`** — Writes structs directly into a reusable command buffer without intermediate `Vec<u8>` allocations.
- **`encode_frame_into`** — The `_into` variant reuses the command vector across frames instead of allocating a new one every tick.
- **`Cow` normalization** — Borrows the input color slice when the LED count matches; only allocates when truncation/padding is needed.

## USB Device Access

Linux requires udev rules to grant non-root access to USB HID devices. Hypercolor ships with a rules file:

```bash
just udev-install
```

This installs `/etc/udev/rules.d/99-hypercolor.rules` and triggers a reload. You may need to re-plug devices after installation.

## Device Discovery

Hypercolor discovers devices through multiple mechanisms:

- **USB HID enumeration** — Scans for known vendor/product IDs at startup and on hotplug events
- **mDNS/Bonjour** — Discovers network devices (WLED) automatically on the local network
- **Manual configuration** — Network devices can also be added by IP address in the config

Trigger a manual discovery sweep:

```bash
# Via CLI
hyper devices discover

# Via REST API
curl -X POST http://localhost:9420/api/v1/devices/discover
```

## Adding New Hardware

New device drivers are added to `crates/hypercolor-hal/src/drivers/`. Each driver must:

1. Define wire-format packet structs with `zerocopy` derives and compile-time size assertions
2. Implement the `Protocol` trait for frame encoding
3. Implement `encode_frame_into` (not just `encode_frame`) for buffer reuse
4. Use `CommandBuffer` with `push_struct` — never build `Vec<ProtocolCommand>` with fresh allocations per frame
5. Use `Cow` normalization for the input color slice
6. Include tests in the crate's `tests/` directory

See the [Contributing](@/contributing/_index.md) guide for the full protocol implementation checklist.
