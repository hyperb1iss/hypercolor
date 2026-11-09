# hypercolor-driver-yeelight

*Yeelight network driver for Hypercolor: one color per frame to Yeelight bulbs, strips, and lamps over LAN Control and music mode.*

This driver follows the official Yeelight WiFi Light Inter-Operation Specification. It finds
lights with the SSDP-style search on UDP port 1982 (multicast, or unicast to configured
addresses) and reads the control address, id, model, and supported methods from the reply,
without opening a control connection. At connect it opens the TCP control connection on port
55443 and asks the light to connect back for music mode, which lifts the command quota; each
frame is then one `set_scene` on the music connection. Lights without music mode are paced
inside the documented 60 commands per minute. Every light needs "LAN Control" switched on in
the Yeelight app. The full wire format and design live in `docs/specs/94-yeelight-driver.md`.

The driver is experimental: it is tested against an in-repo fake device, but nobody has run
it on real Yeelight hardware yet.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-driver-support`, `hypercolor-color`,
  `hypercolor-types`, `anyhow`, `async-trait`, `serde`, `serde_json`, `thiserror`, `tokio`,
  `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `yeelight` feature)

## Key Public Surface

- `YeelightDriverModule`: `DriverModule` implementation
- `DESCRIPTOR: DriverDescriptor`: static descriptor (`id = "yeelight"`)
- `YeelightBackend`, `YeelightConfig`, `StreamMode`: backend and config types
- `YeelightScanner`, `YeelightScanOptions`, `discovered_device`: discovery
- `protocol`: the search, command lines, replies, capabilities, color conversion
- `yeelight_driver_control_surface`, `yeelight_device_control_surface`: control surfaces
- `resolve_yeelight_probe_targets`: unicast search targets from config and tracked devices

## Devices and Protocol

Targets every Yeelight Wi-Fi light with LAN Control: color and white bulbs, strips, and
lamps, each as a single-zone light. Uses UDP port 1982 (search) and TCP port 55443
(control), plus an ephemeral TCP port on the host for music mode.

## Testing

`cargo test -p hypercolor-driver-yeelight` runs everything against a loopback fake device;
nothing touches the LAN.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
