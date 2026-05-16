# hypercolor-driver-builtin

*Compile-time driver bundle that wires all built-in drivers into one registry for the daemon.*

This crate is the wiring harness between the daemon and the individual driver
crates. The daemon calls `build_driver_module_registry` to get a fully populated
`DriverModuleRegistry` containing every driver compiled in. Feature flags gate
each driver family independently, keeping unused drivers out of the binary. This
indirection means the daemon never imports driver crates directly and stays free
of per-driver branching.

## Position in the Workspace

- Depends on: `hypercolor-core`, `hypercolor-driver-api`, `hypercolor-network`,
  `hypercolor-types`, `anyhow`; and optionally `hypercolor-driver-govee`,
  `hypercolor-driver-hue`, `hypercolor-driver-nanoleaf`, `hypercolor-driver-wled`,
  `hypercolor-hal`
- Consumed by: `hypercolor-daemon`
- Defines no driver logic itself; all logic lives in the individual driver crates

## Key Public Surface

- `build_driver_module_registry(config, credential_store) -> Result<DriverModuleRegistry>` —
  primary entry point; constructs and returns the populated registry
- `register_driver_modules(registry, config, credential_store) -> Result<()>` —
  registers into an existing registry instance
- `normalize_driver_config_entries(config)` — ensures every compiled-in driver has
  a matching config entry in the daemon's configuration

## Cargo Features

| Feature | Default | What it enables |
|---|---|---|
| `network` | yes | All four network drivers (`hue`, `nanoleaf`, `wled`, `govee`) |
| `hal` | yes | `hypercolor-hal` bridged as `HalCatalogDriverModule` entries |
| `hue` | via `network` | Philips Hue driver |
| `nanoleaf` | via `network` | Nanoleaf driver |
| `wled` | via `network` | WLED driver |
| `govee` | via `network` | Govee driver |

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
