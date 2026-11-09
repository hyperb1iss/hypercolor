# hypercolor-driver-wiz

*WiZ network driver for Hypercolor: one color per frame to WiZ bulbs, strips, and fixtures over the local UDP API.*

This driver finds WiZ (Signify) lights with the `registration` broadcast on UDP port 38899
(or probes configured addresses), reads each device's system and model configuration,
classifies it from its module name, and streams every frame as one fire-and-forget
`setPilot` datagram. Color lights take hue and saturation in `r`, `g`, `b` and brightness in
`dimming`; white lights follow the frame's luma. Homes set to "Only verified controls" in
the WiZ app need the home signing key, which signs each `setPilot` with HMAC-SHA256. The
full wire format and design live in `docs/specs/93-wiz-driver.md`.

The driver is experimental: it is tested against an in-repo fake device, but nobody has
run it on real WiZ hardware yet.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-driver-support`, `hypercolor-color`,
  `hypercolor-types`, `anyhow`, `async-trait`, `base64`, `if-addrs`, `serde`, `serde_json`,
  `sha2`, `thiserror`, `tokio`, `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `wiz` feature)

## Key Public Surface

- `WizDriverModule`: `DriverModule` implementation
- `DESCRIPTOR: DriverDescriptor`: static descriptor (`id = "wiz"`)
- `WizBackend`, `WizConfig`: backend and config types
- `WizScanner`, `WizScanOptions`, `broadcast_targets`, `discovered_device`: discovery
- `protocol`: request bytes, reply decoding, module classes, `Pilot` conversion, signing
- `transport`: request and reply exchange for probes
- `wiz_driver_control_surface`, `wiz_device_control_surface`: control surfaces
- `resolve_wiz_probe_targets`: unicast probe targets from config and tracked devices

## Devices and Protocol

Targets every WiZ light: color, tunable white, and dimmable white bulbs, strips, and
fixtures, each as a single-zone light. Smart plugs are skipped. Uses UDP port 38899.

## Testing

`cargo test -p hypercolor-driver-wiz` runs everything against a loopback fake device;
nothing touches the LAN.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
