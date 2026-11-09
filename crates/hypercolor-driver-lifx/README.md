# hypercolor-driver-lifx

*LIFX network driver for Hypercolor: real-time output to LIFX lights over the official LAN protocol.*

This driver discovers LIFX lights with a UDP broadcast GetService (or unicast to configured
addresses), reads each device's product, firmware, label, and zone topology, and streams
frames as unacknowledged Set messages. Single-zone bulbs take SetColor; strips, beams, Neon,
and String lights take SetExtendedColorZones, falling back to SetColorZones on older
firmware; Tile chains, Candles, Ceilings, and the other matrix products take Set64 per
tile, with a hidden frame buffer and CopyFrameBuffer above 64 zones. Engine RGB is
converted to HSBK at the edge. Every datagram is charged to a per-device message budget
as it is sent; frames wait for the budget in the host's latest-value output queue, where
a newer frame can still replace them. The full wire format
and design live in `docs/specs/88-lifx-lan-driver.md`.

The driver is experimental: it is tested against an in-repo fake responder and the
lifx-emulator project, but nobody has run it on real LIFX hardware yet.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-driver-support`, `hypercolor-color`,
  `hypercolor-types`, `anyhow`, `async-trait`, `if-addrs`, `serde`, `serde_json`,
  `thiserror`, `tokio`, `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `lifx` feature)

## Key Public Surface

- `LifxDriverModule`: `DriverModule` implementation
- `DESCRIPTOR: DriverDescriptor`: static descriptor (`id = "lifx"`)
- `LifxBackend`, `LifxConfig`: backend and config types
- `LifxScanner`, `LifxScanOptions`, `broadcast_targets`, `discovered_device`: discovery
- `protocol`: header and message encoders, State decoders
- `color::HsbkEncoder`: RGB to HSBK conversion
- `products`: product facts and capability resolution
- `topology`: device classes, tile geometry, segments and LED positions
- `encoder`, `pacer`: frame encoding and the per-message token-bucket budget
- `lifx_driver_control_surface`, `lifx_device_control_surface`: control surface builders
- `resolve_lifx_probe_targets`: unicast discovery targets from config and tracked devices

## Devices and Protocol

Targets LIFX bulbs, multizone strips and beams, and matrix devices. Uses the LIFX LAN
protocol over UDP port 56700. Discovery is a broadcast GetService plus unicast probes of
configured and previously seen addresses.

## Testing

`cargo test -p hypercolor-driver-lifx` runs everything against loopback fakes. The opt-in
emulator run needs `uv`:

```bash
just lifx-emulator-test
```

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
