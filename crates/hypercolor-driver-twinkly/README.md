# hypercolor-driver-twinkly

*Twinkly network driver for Hypercolor: per-LED real-time output to Twinkly strings, curtains, Squares, and other Wi-Fi lights.*

This driver finds Twinkly devices with the UDP discovery datagram on port 5555 (or probes
configured addresses), reads each device's gestalt and firmware over the local REST API,
logs in with the challenge and verify handshake at connect, reads the LED layout the user
mapped in the Twinkly app, switches the device to real-time mode, and streams every frame
over UDP port 7777. The datagram version (1, 2, or 3) follows the firmware family and
version, and the LED profile (RGB, or RGBW with white first) sets the bytes per LED. A
background upkeep task logs in again when the token nears expiry or another client takes
it, and puts the device back in real-time mode when something switched it away. The full
wire format and design live in `docs/specs/92-twinkly-driver.md`.

The driver is experimental: it is tested against an in-repo fake device, but nobody has
run it on real Twinkly hardware yet.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-driver-support`, `hypercolor-color`,
  `hypercolor-types`, `anyhow`, `async-trait`, `base64`, `if-addrs`, `rand`,
  `serde`, `serde_json`, `thiserror`, `tokio`, `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `twinkly` feature)

## Key Public Surface

- `TwinklyDriverModule`: `DriverModule` implementation
- `DESCRIPTOR: DriverDescriptor`: static descriptor (`id = "twinkly"`)
- `TwinklyBackend`, `TwinklyConfig`: backend and config types
- `TwinklyScanner`, `TwinklyScanOptions`, `broadcast_targets`, `discovered_device`: discovery
- `api`: REST client, request bodies, and reply types
- `protocol`: discovery datagrams, firmware versions, LED profiles, real-time encoder
- `layout`: layout coordinates to spatial segments
- `twinkly_driver_control_surface`, `twinkly_device_control_surface`: control surfaces
- `resolve_twinkly_probe_targets`: REST endpoints from config and tracked devices

## Devices and Protocol

Targets every Twinkly Wi-Fi product with an RGB or RGBW profile, generation I and II.
Twinkly Pro controllers (`/xled/v2` API) and Gold Edition (AWW profile) devices are not
supported. Uses HTTP on port 80 and UDP on ports 5555 (discovery) and 7777 (frames).

## Testing

`cargo test -p hypercolor-driver-twinkly` runs everything against a loopback fake device;
nothing touches the LAN.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor), open-source RGB
lighting orchestration for Linux, Windows, and macOS. Licensed under
Apache-2.0.
