# hypercolor-driver-nanoleaf

*Nanoleaf network driver for Hypercolor — UDP streaming to light panel controllers.*

This driver targets Nanoleaf light panel controllers (Canvas, Shapes, Lines, Elements,
Aurora). It discovers controllers via mDNS (`_nanoleafapi._tcp`) and known IP probing,
pairs using the Nanoleaf Open API token flow (hold power button to enter pairing mode,
then call `/api/v1/new`), and streams per-panel color data over Nanoleaf's UDP External
Control protocol. On connect the driver fetches the panel topology layout from
`/panelLayout/layout` and caches it so the engine can map LED positions to physical
panel coordinates. A `refresh_topology` control action reconnects the device to reload
the layout on demand.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-types`, `anyhow`, `async-trait`,
  `reqwest`, `serde`, `serde_json`, `tokio`, `tracing`
- Consumed by: `hypercolor-driver-builtin` (via the `nanoleaf` feature)

## Key Public Surface

- `NanoleafDriverModule` — `DriverModule` implementation; `new(credential_store, mdns_enabled)`
- `DESCRIPTOR: DriverDescriptor` — static descriptor (`id = "nanoleaf"`)
- `NanoleafBackend`, `NanoleafConfig` — backend and config types
- `NanoleafScanner`, `NanoleafKnownDevice` — device discovery
- `NanoleafStreamSession`, `encode_frame_into`, `DEFAULT_NANOLEAF_API_PORT`,
  `DEFAULT_NANOLEAF_STREAM_PORT` — UDP streaming
- `NanoleafShapeType` — panel shape enum used in topology mapping
- `NanoleafDeviceInfo`, `NanoleafDiscoveredDevice`, `NanoleafPanelLayout`,
  `panel_ids_from_layout` — device info and topology types
- `pair_device_with_status`, `pair_nanoleaf_device_at_ip`, `NanoleafPairResult`,
  `StoredNanoleafPairingResult` — pairing helpers
- `nanoleaf_driver_control_surface`, `nanoleaf_device_control_surface` — control surface builders
- `resolve_nanoleaf_probe_devices_from_sources` — discovery helper

## Devices and Protocol

Targets Nanoleaf panel controllers (Canvas, Shapes, Lines, Elements, Aurora). Uses the
Nanoleaf Open API over HTTP on port 16021 by default. Realtime color streaming uses the
UDP External Control protocol on port 60222 by default. Discovery uses mDNS
`_nanoleafapi._tcp` plus known-IP probing.

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
