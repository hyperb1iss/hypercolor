# hypercolor-driver-hue

*Philips Hue network driver for Hypercolor — real-time light streaming via the Hue Entertainment API.*

This driver targets Philips Hue Bridge (v2) and connected lights. It discovers
bridges via mDNS (`_hue._tcp`) and the Hue Nupnp cloud lookup service, pairs with
them through the physical link-button flow, and streams RGBA color data over the
Hue Entertainment API using DTLS 1.2 on UDP. The `use_cie_xy` config flag switches
between CIE xy+brightness and raw RGB output; CIE xy is the default and gives the
most accurate color reproduction across Hue bulb gamuts. Per-bridge credentials
(API key and client key) are stored keyed by bridge ID and IP.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-types`, `anyhow`, `async-trait`,
  `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`, `webrtc-dtls`, `webrtc-util`,
  `rustls` (aws_lc_rs backend)
- Consumed by: `hypercolor-driver-builtin` (via the `hue` feature)
- Only network driver that requires DTLS; brings in a TLS dependency

## Key Public Surface

- `HueDriverModule` — `DriverModule` implementation; `new(credential_store, mdns_enabled)`
- `DESCRIPTOR: DriverDescriptor` — static descriptor (`id = "hue"`)
- `HueBackend`, `HueConfig` — backend type and deserialized driver config
- `HueBridgeClient`, `HueNupnpBridge`, `DEFAULT_HUE_API_PORT`, `DEFAULT_HUE_STREAM_PORT`
- `HueStreamSession`, `encode_packet_into` — DTLS streaming session
- `CieXyb`, `ColorGamut`, `GAMUT_A`, `GAMUT_B`, `GAMUT_C`, `rgb_to_cie_xyb` — color math
- `HueScanner`, `HueKnownBridge` — bridge discovery
- `HueBridgeIdentity`, `HueEntertainmentConfig`, `HueLight`, `HuePairResult`,
  `build_device_info`, `choose_entertainment_config`
- `pair_hue_bridge_at_ip`, `resolve_hue_probe_bridges_from_sources` — pairing helpers
- `hue_driver_control_surface`, `hue_device_control_surface` — control surface builders

## Devices and Protocol

Targets Philips Hue Bridge v2 and lights addressed via Entertainment Groups. Streaming
uses the Hue Entertainment API v2 over UDP under DTLS 1.2 (default port 2100). Discovery
combines mDNS `_hue._tcp` with `https://discovery.meethue.com` Nupnp fallback.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
