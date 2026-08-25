# hypercolor-types

*Shared data vocabulary for the entire Hypercolor workspace.*

Every domain structure that crosses a crate boundary lives here. The crate
is intentionally narrow: serializable vocabulary plus the validation and
canonicalization that keep those values coherent, with no I/O or async
runtime. Other workspace crates can depend on it without pulling in
application services.

## Workspace position

**Depends on:** `hypercolor-color`, `serde`, `serde_json`, `thiserror`,
`uuid`, and `strum`. The optional `schema` feature also enables `utoipa`
and schema support in `hypercolor-color`.

**Depended on by:** most workspace crates — `hypercolor-core`,
`hypercolor-hal`, `hypercolor-driver-api`, all network driver crates,
`hypercolor-daemon`, `hypercolor-tui`, `hypercolor-ui`,
and others. Crates that operate purely at a higher abstraction layer (e.g.
`hypercolor-cli`, infrastructure adapters, proc-macro crates) may not depend
on it directly.

## Key types

| Module | Notable types |
|---|---|
| `device` | `DeviceId` (UUIDv7), `DeviceInfo`, `DeviceCapabilities`, `DeviceFamily`, `DeviceState` |
| `spatial` | `SpatialLayout`, `Output`, `OutputComponent`, `LedTopology`, `NormalizedPosition`, `ZoneShape` |
| `canvas` | `Canvas`, `PublishedSurface` — the 640×480 (configurable) RGBA pixel buffer |
| `audio` | `AudioData`, `AudioPipelineConfig` — per-frame spectrum/beat snapshot |
| `effect` | `EffectMetadata`, `ControlValue`, `EffectId` |
| `layer` | `SceneLayer`, `SceneLayerId`, `LayerSource`, `BlendMode` |
| `scene` | `Scene`, `Zone`, `ZoneId`, `ZoneRole`, `DisplayFaceTarget` |
| `event` | `HypercolorEvent`, `FrameData`, `ZoneColors`, `SpectrumData` — event bus payloads |
| `sensor` | `SystemSnapshot` — CPU/GPU/memory telemetry |
| `config` | `DaemonConfig` — top-level TOML configuration |
| `server` | `ApiMeta`, `ControlUpdate` — REST envelope and patch types |
| `session` | `SessionState` — logind/screensaver awareness |
| `viewport` | `ViewportConfig`, `ScreenRegion` |
| `library` | `FavoriteEntry` |
| `attachment` | `ComponentTemplate`, `ComponentSlot`, `ComponentBinding`, `DeviceComponentProfile` |
| `controls` | `ControlSurface`, `ControlSurfaceInput` |

All modules are flat re-exports; import via the module path that matches
the domain you are working in.

## Feature flags

`schema` enables OpenAPI schema derives for the daemon. Other consumers use
the shared runtime vocabulary without pulling in `utoipa`.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux. Licensed under Apache-2.0.
