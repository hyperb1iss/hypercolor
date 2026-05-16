# hypercolor-network

*Driver module registry and capability-filtered dispatch for Hypercolor.*

This crate owns the host-side registry of compiled-in driver modules. It provides
`DriverModuleRegistry` — a `BTreeMap`-backed store of `Arc<dyn DriverModule>` instances
keyed by stable driver ID — along with capability-filtered accessors for discovery,
pairing, controls, protocol catalogs, and presentation. At registration time the registry
enforces API schema version compatibility and rejects duplicate IDs via a typed error.
The crate has no knowledge of any concrete driver; all it holds is the `DriverModule`
trait imported from `hypercolor-driver-api`. Concrete drivers are assembled in
`hypercolor-driver-builtin` and handed off to the daemon through this registry type.

## Position in the Workspace

- Depends on: `hypercolor-driver-api`, `hypercolor-types`, `thiserror`
- Consumed by: `hypercolor-driver-builtin` (which populates the registry),
  `hypercolor-daemon` (which receives the populated registry and dispatches through it)

## Key Public Surface

- `DriverModuleRegistry` — the central registry type
  - `register(module) -> Result<(), DriverModuleRegistryError>`
  - `register_shared(module: Arc<dyn DriverModule>)`
  - `get(id) -> Option<Arc<dyn DriverModule>>`
  - `ids()`, `descriptors()`, `module_descriptors()`
  - `discovery_drivers()`, `pairing_drivers()`, `control_drivers()`,
    `protocol_catalog_drivers()`, `presentation_drivers()` — capability-filtered vecs
- `DriverModuleRegistryError` — `thiserror` enum with `DuplicateDriverId` and
  `SchemaVersionMismatch` variants

## Cargo Features

None.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source RGB
lighting orchestration for Linux. Licensed under Apache-2.0.
