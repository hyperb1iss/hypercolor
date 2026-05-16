# hypercolor-driver-govee

*Govee network driver for Hypercolor — LAN UDP streaming and cloud fallback for Govee smart lighting.*

This driver targets Govee LED strips, panels, and bulbs over two transport paths. The
primary path is local-area UDP using Govee's proprietary LAN control protocol (port 4003);
select SKUs support a Govee/Razer-protocol high-speed streaming variant for higher frame
rates. The secondary path is the Govee Developer API v1 (REST over HTTPS) used for device
inventory enrichment and as a fallback for devices not reachable on the LAN. Discovery
combines known-IP probing (tracking previously discovered MACs) with an optional cloud
inventory fetch when an account API key is configured. Pairing involves entering a Govee
API key from the Govee Home app; the key is validated against the cloud before being stored.
A per-SKU capability database maps model numbers to LED counts, topology, and protocol flags.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-types`, `anyhow`, `async-trait`,
  `base64`, `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `govee` feature)
- LAN discovery uses UDP broadcast; does not use mDNS

## Key Public Surface

- `GoveeDriverModule` — `DriverModule` implementation; `new`, `with_credential_store`,
  `with_cloud_base_url`
- `DESCRIPTOR: DriverDescriptor` — static descriptor (`id = "govee"`)
- `GoveeCapabilities`, `SkuFamily`, `SkuProfile`, `profile_for_sku`, `fallback_profile`,
  `known_sku_count`, `known_cloud_sku_count` — SKU capability database
- `GoveeLanDevice`, `GoveeKnownDevice`, `GoveeLanScanner`, `build_device_info`,
  `parse_scan_response` — LAN discovery primitives
- `govee_driver_control_surface`, `govee_device_control_surface` — control surface builders
- `resolve_govee_probe_devices`, `resolve_govee_probe_devices_from_sources`,
  `merge_cloud_inventory`, `build_cloud_discovered_device` — discovery helpers
- Sub-modules: `backend` (output backend), `capabilities` (SKU/capability database),
  `cloud` (REST client with rate limiting), `lan` (LAN discovery and streaming)

## Devices and Protocol

Targets Govee LED strips, panels, and bulbs identified by model SKU strings (e.g. `H6199`).
LAN transport uses the Govee UDP LAN control protocol on port 4003; compatible SKUs
additionally support the Razer streaming variant at higher frame rates. Cloud transport
uses the Govee Developer API v1 at `https://developer-api.govee.com` with an account-level
API key.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
