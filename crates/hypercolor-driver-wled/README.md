# hypercolor-driver-wled

*WLED network driver for Hypercolor — DDP and E1.31 streaming to ESP-based LED controllers.*

WLED is open-source ESP8266/ESP32 firmware for LED strips with a large hobbyist user base.
This driver discovers WLED controllers via mDNS (`_wled._tcp`) and known IP probing,
then streams real-time pixel data using DDP (Distributed Display Protocol) or E1.31/sACN.
DDP is the default and preferred choice; E1.31 support includes multi-universe spanning for
large LED counts. No pairing is required — WLED has no authentication by default. Both RGB
and RGBW color formats are supported, and per-device protocol selection is exposed via a
control surface field so individual devices can override the driver-level default. A runtime
cache preserves probe IPs and device info across daemon restarts.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-types`, `anyhow`, `async-trait`,
  `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`, `uuid`
- Consumed by: `hypercolor-driver-builtin` (via the `wled` feature)

## Key Public Surface

- `WledDriverModule` — `DriverModule` implementation; `new(mdns_enabled)`
- `DESCRIPTOR: DriverDescriptor` — static descriptor (`id = "wled"`)
- `WledBackend`, `WledColorFormat`, `WledDevice`, `WledDeviceInfo`,
  `WledLiveReceiverConfig`, `WledProtocol`, `WledSegmentInfo`
- `WledConfig`, `WledProtocolConfig` — driver config types
- `DdpPacket`, `DdpSequence`, `build_ddp_frame` — DDP frame builder
- `E131Packet`, `E131SequenceTracker`, `E131_PIXELS_PER_UNIVERSE_RGB`,
  `E131_PIXELS_PER_UNIVERSE_RGBW`, `universes_needed` — E1.31/sACN support
- `WledScanner`, `WledKnownTarget` — discovery scanner
- `build_wled_backend` — backend factory merging config and cached hints
- `resolve_wled_probe_ips_from_sources`, `resolve_wled_probe_targets_from_sources`
- `wled_driver_control_surface`, `wled_device_control_surface` — control surface builders

## Devices and Protocol

Targets WLED controllers running on ESP8266/ESP32 hardware over Wi-Fi. DDP streams on UDP
port 4048; E1.31/sACN uses standard UDP multicast or unicast with multi-universe support
for strips longer than a single universe. Discovery uses mDNS `_wled._tcp` plus known-IP
probing; device info (LED count, firmware, RGBW flag) is fetched from `/json/info`.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
