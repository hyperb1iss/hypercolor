# hypercolor-types

*Shared data vocabulary for the entire Hypercolor workspace.*

Every data structure that crosses a crate boundary lives here. The crate
is intentionally narrow: pure structs, enums, and serde derives — no
logic, no I/O, no async. All other workspace crates can depend on it
without pulling in any runtime cost.

## Workspace position

**Depends on:** `serde`, `serde_json`, `thiserror`, `uuid`, `strum`,
`utoipa` — no workspace crates.

**Depended on by:** most workspace crates — `hypercolor-core`,
`hypercolor-hal`, `hypercolor-driver-api`, all network driver crates,
`hypercolor-daemon`, `hypercolor-tui`, `hypercolor-tray`, `hypercolor-ui`,
and others. Crates that operate purely at a higher abstraction layer (e.g.
`hypercolor-cli`, infrastructure adapters, proc-macro crates) may not depend
on it directly.

## Key types

| Module | Notable types |
|---|---|
| `device` | `DeviceId` (UUIDv7), `DeviceInfo`, `DeviceCapabilities`, `DeviceFamily`, `DeviceState` |
| `spatial` | `SpatialLayout`, `DeviceZone`, `LedTopology`, `NormalizedPosition`, `ZoneShape` |
| `canvas` | `Canvas`, `PublishedSurface` — the 640×480 (configurable) RGBA pixel buffer |
| `audio` | `AudioData`, `AudioPipelineConfig` — per-frame spectrum/beat snapshot |
| `effect` | `EffectMetadata`, `ControlValue`, `EffectId` |
| `scene` | `SceneConfig`, `DisplayFaceTarget`, `RenderGroupId`, `DisplayFaceBlendMode` |
| `event` | `HypercolorEvent`, `FrameData`, `ZoneColors`, `SpectrumData` — event bus payloads |
| `sensor` | `SystemSnapshot` — CPU/GPU/memory telemetry |
| `config` | `DaemonConfig` — top-level TOML configuration |
| `server` | `ApiMeta`, `ControlUpdate` — REST envelope and patch types |
| `session` | `SessionState` — logind/screensaver awareness |
| `viewport` | `ViewportConfig`, `ScreenRegion` |
| `palette` | `Palette`, `ColorStop` |
| `library` | `FavoriteEntry` |
| `attachment` | `AttachmentProfile`, `AttachmentSlot` |
| `controls` | `ControlSurface`, `ControlSurfaceInput` |

All modules are flat re-exports; import via the module path that matches
the domain you are working in.

## Feature flags

None. The crate has no optional feature gates.

---

Part of [Hypercolor](https://github.com/hyperb1iss/hypercolor) — open-source
RGB lighting orchestration for Linux. Licensed under Apache-2.0.
