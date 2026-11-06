# hypercolor-driver-pixel

*Generic pixel controller driver for Hypercolor: E1.31 (sACN), Art-Net 4, and DDP streaming to manually configured receivers.*

Pixel controllers do not announce themselves, so every device is an entry in the
driver config: a stable id, a host, a protocol, and either a pixel count or a list
of outputs (segments) with optional universe or offset jumps. Discovery turns those
entries into devices without sending a packet. Output is unicast only: the driver
rejects multicast and broadcast hosts and never polls the network. One driver covers
Falcon controllers, Falcon Player (FPP), ESPixelStick, xLights-style props, and DMX
fixtures behind Art-Net or sACN nodes.

Each device's effective config travels in its discovery metadata, so the backend is
config-free and a config edit reaches the next connection without a backend rebuild.
Frames follow the render loop unless a device sets `max_fps`, and the last frame is
reasserted every 800 ms to hold receivers inside their data-loss timeouts. On
disconnect the driver sends a black frame and, for E1.31, three `Stream_Terminated`
packets per universe.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-driver-support`,
  `hypercolor-pixel-protocols`, `hypercolor-types`, `anyhow`, `async-trait`,
  `serde`, `serde_json`, `thiserror`, `tokio`, `tracing`, `uuid`
- Consumed by: `hypercolor-driver-builtin` (via the `pixel` feature)

## Key Public Surface

- `PixelDriverModule`, `DESCRIPTOR` (`id = "pixel"`): driver module
- `PixelBackend`, `KEEPALIVE_INTERVAL`, `METADATA_DEVICE_CONFIG`: UDP backend
- `PixelConfig`, `PixelDeviceConfig`, `PixelSegmentConfig`, `PixelProtocol`,
  `PixelConfigError`: config schema and validation errors
- `resolve_config`, `resolve_device`, `PixelDevicePlan`, `derived_cid`: output
  planning (universe routing, DDP spans, E1.31 source identity)
- `ColorOrder`, `encode_pixels`: channel order encoding with RGBW extraction
- `configured_devices`: config entries as discovery results
- `pixel_driver_control_surface`, `pixel_device_control_surface`: control surfaces

## Devices and Protocol

E1.31 on UDP 5568 (universes 1-63999, priority, source name, optional universe
synchronization), Art-Net 4 ArtDmx on UDP 6454 (15-bit Port-Address), and DDP on UDP
4048 (byte offsets, push on the final packet). Packet layouts, universe math, and
references live in `docs/specs/87-pixel-protocols-driver.md`; setup is documented in
`docs/content/hardware/pixel-controllers.md`.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
