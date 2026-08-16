# Spec 76: Internal API Unification

**Status:** LOCKED rev 5 (2026-08-15) — converged after five cross-model review rounds (finding trajectory 29 → 12 → 2 → 2 → 1; full dispositions in §10).
**Findings base:** `docs/review/tech-debt-review-2026-08-15.md` — six-lane audit with file:line receipts. Local audit artifact (`docs/review/` is gitignored); receipts were verified against the tree on 2026-08-15 and line numbers should be re-checked at execution time.
**Authorship model:** Fable writes and owns the contracts here and holds final signoff. Opus 5 workers execute mechanical refactors and tests per wave. Codex reviews inline per PR.

---

## 0. Goals, non-goals, compat doctrine

**Goals**
1. One canonical home per concept: color math, config authority, API contracts, control values, device topology, discovery results.
2. Transport-agnostic business logic: REST, MCP, WS, CLI consume the same domain functions and typed contracts.
3. Contracts enforced by compiler and CI (round-trip tests, shared vectors, golden fixtures, completeness tests), never by doc comments.
4. APIs that read as their semantics: linearization visible in names, boot-vs-live visible in types, fenced-vs-unfenced mutation visible in ownership.

**Non-goals:** no rendering behavior changes except where divergence is itself the bug (named per wave); no performance regressions (baselines are product contracts; engine waves are benchmark-gated).

**Compat doctrine (normative for every wave):**
- **Persisted files** (`scenes.json`, profiles, presets, `hypercolor.toml`, device settings): the on-disk serialization never changes silently. A shape change ships as an explicit versioned migration with before/after fixtures, and old shapes stay readable for one release minimum. **Writers keep emitting the legacy representation until every supported reader (daemon, UI, TUI, CLI within the pinned compat window) accepts the new one; the write-side flip is its own reviewed wave** — read-both/write-old first, flip second, never both at once.
- **REST v1:** a compatibility matrix artifact (method, path, request, success body, error body, headers, status) is produced before Phase 2 and kept green by tests. Legacy paths (`/effects/{id}/apply`, `/effects/current/*`, `/scenes/{id}/groups/*`, `/config/get|set`) remain routed and **serve legacy projections** — top-level `current` on 412s, the old `pagination` block — while canonical routes serve the new contracts. Serde aliases are a reader tool only; they never justify changing what the server emits on a v1 path.
- **WS:** every binary tag and byte layout is frozen by golden fixtures before any WS refactor. JSON message changes ship dual-accept (old and new forms) behind a version field in the subscribe handshake; `interval_ms` vs `fps` is a **unit conversion**, normalized internally into one cadence type, both accepted on v1.

---

## 1. `hypercolor-color` — the color kernel crate

**Position:** new workspace crate at the bottom of the graph. `hypercolor-types` depends on it and re-exports the pixel data carriers, so existing `hypercolor_types::canvas::{Rgb, Rgba}` imports keep compiling during migration.

**Features:** `serde` · `schema`. The crate is std-only; a `std` feature gets minted only when a real no_std consumer exists. No `palettes` feature at introduction: `types/palette.rs` has zero consumers (verified both import paths) and is deleted in Phase 0. The crate grows a palette runtime only when a native consumer exists; `sdk/shared/palettes.json` remains the TS-side source of truth meanwhile.

**Third-party stance:** no runtime dep on `palette`/`csscolorparser`. `palette` is a **dev-dependency oracle**: property tests assert every kernel matches reference math within tolerance.

### 1.1 Conventions (normative for every function)
- Hue is degrees, wrapped `rem_euclid(360.0)` at every entry point.
- s/v/l/alpha are 0.0–1.0. Percent and 0–255 adapters live at call sites.
- Float→u8 is always `.mul(255.0).round().clamp(0.0, 255.0) as u8`.
- **Linearization appears in the name.** Nothing converts color space implicitly.
- Value types are `Copy + PartialEq + Debug`; serde behind the `serde` feature.

### 1.2 Types

```rust
pub struct Rgb  { pub r: u8, pub g: u8, pub b: u8 }               // encoded sRGB
pub struct Rgba { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }    // encoded sRGB, straight alpha
pub struct LinearRgba { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }  // linear-light, straight alpha
pub struct Hsv { pub h: f32, pub s: f32, pub v: f32 }
pub struct Hsl { pub h: f32, pub s: f32, pub l: f32 }
pub struct Oklab  { pub l: f32, pub a: f32, pub b: f32, pub alpha: f32 }   // alpha-bearing, as today
pub struct Oklch  { pub l: f32, pub c: f32, pub h: f32, pub alpha: f32 }
```

Alpha is carried through every perceptual conversion and interpolation — round-tripping `LinearRgba → Oklab → LinearRgba` is lossless in opacity (today's `canvas.rs` types already do this; rev 1 regressed it). `RgbaF32` is renamed `LinearRgba` with a deprecated alias for one release.

### 1.3 Conversions

```rust
impl Hsv { pub fn to_rgb(self) -> Rgb; pub fn from_rgb(rgb: Rgb) -> Hsv; }
impl Hsl { pub fn to_rgb(self) -> Rgb; pub fn from_rgb(rgb: Rgb) -> Hsl; }
impl Rgb {
    pub fn to_linear(self) -> LinearRgba;            // alpha = 1.0
    pub fn luma_encoded(self) -> f32;                // BT.709 on encoded values (screen fast paths)
}
impl LinearRgba {
    pub fn to_encoded(self) -> Rgba;
    pub fn luma(self) -> f32;                        // BT.709 on linear values — default luminance
    pub fn lerp(self, other: Self, t: f32) -> Self;
    pub fn to_oklab(self) -> Oklab;
}
impl Oklab { pub fn to_linear(self) -> LinearRgba; pub fn lerp(self, other: Self, t: f32) -> Self; }
impl Oklch {
    pub fn to_oklab(self) -> Oklab; pub fn from_oklab(lab: Oklab) -> Oklch;
    pub fn to_linear(self) -> LinearRgba;
    pub fn lerp(self, other: Self, t: f32) -> Self;   // shortest-path hue arc
}
impl LinearRgba { pub fn to_oklch(self) -> Oklch; }

pub fn srgb_to_linear(c: f32) -> f32;
pub fn linear_to_srgb(c: f32) -> f32;
pub mod lut { pub fn srgb_u8_to_linear(c: u8) -> f32; pub fn linear_to_srgb_u8(c: f32) -> u8; }
pub const LUMA_R: f32 = 0.2126; pub const LUMA_G: f32 = 0.7152; pub const LUMA_B: f32 = 0.0722;
```

### 1.4 Hex

```rust
pub enum ColorParseError { BadLength(usize), BadDigit }

impl Rgb  { pub fn from_hex(s: &str) -> Result<Rgb, ColorParseError>; pub fn to_hex(self) -> String; }
impl Rgba { pub fn from_hex(s: &str) -> Result<Rgba, ColorParseError>; }
impl LinearRgba { pub fn from_hex_srgb(s: &str) -> Result<LinearRgba, ColorParseError>; } // parse THEN linearize
```

Grammar: optional single `#`, then exactly 3/4/6/8 hex digits (CSS shorthand expansion). Anything else errors — no silent white/black/green fallbacks anywhere. Callers pick fallbacks explicitly. Accepted digit counts are per target type: `Rgba` and `LinearRgba` take all four forms; `Rgb::from_hex` takes 3/6 only and rejects alpha-bearing forms rather than silently dropping alpha.

### 1.5 Pixel blending and device encoding

```rust
// Pixel-kernel blend modes only — exactly the alpha-composable set the in-tree kernel implements.
pub enum PixelBlendMode { Normal, Add, Screen, Multiply, Overlay, SoftLight, ColorDodge, Difference }
impl LinearRgba {
    /// `self` is the SOURCE, blended over `dst` at `opacity`.
    pub fn blend_over(self, dst: LinearRgba, mode: PixelBlendMode, opacity: f32) -> LinearRgba;
}
```

The authored `BlendMode` (11 variants incl. `Replace`/`Tint`/`LumaReveal`) **stays in `hypercolor-types`** as the single authored enum (merging `LayerBlendMode`/`DisplayFaceBlendMode`); compositor-level modes map into `PixelBlendMode` exactly where `layer.rs:242` does today. The color crate never learns scene semantics.

```rust
pub enum DevicePixelLayout { Rgb, Grb, Rbg, RgbwZeroWhite }   // RgbwZeroWhite: 4 bytes, W=0 — today's behavior, named honestly
pub struct EncodedChannels { pub bytes: [u8; 4], pub len: u8 }
impl Rgb {
    pub fn scale(self, factor: f32) -> Rgb;                    // THE brightness scaler: round-clamp
    pub fn encode(self, layout: DevicePixelLayout) -> EncodedChannels;
}
```

Buffer length lives in the type (`EncodedChannels`), not in caller discipline. Real RGBW white extraction is future work with its own `DevicePixelLayout` variant. `hypercolor_types::device::DeviceColorFormat` keeps device-facing variants (incl. `Jpeg`) and gains `fn pixel_layout(self) -> Option<DevicePixelLayout>`.

### 1.6 TypeScript mirror + shared vectors

- `sdk/packages/core/src/color/index.ts`, exported from the `hypercolor` barrel: `hexToRgb(s, fallback)` (explicit fallback, 3/4/6/8-digit), `hslToRgb`, `hsvToRgb`, `rgbToHsl`, `rgbToHsv`, `rgbToHex`; re-exports `clamp`/`mix`/`saturate` from `math/lerp.ts`. `audio/helpers.ts` re-exports from here (deprecated path).
- **`sdk/shared/color-vectors.json`**: ~200 `(op, input, expected)` triples: hex parse incl. malformed, HSL/HSV↔RGB incl. hue wrap and negatives, sRGB transfer, Oklab round-trips. Rust test `include_str!`s it; Bun test imports it; drift fails CI in both languages.
- GLSL: `sdk/shared/glsl/color.glsl` gets injected by the effect build (prelude concat in `tooling/build.ts`); the four inline `hsv2rgb` copies die. GLSL is a fourth implementation covered only by effect snapshot tests — accepted looseness.

---

## 2. Domain service layer (`hypercolor-daemon/src/domain/`)

### 2.1 `DomainError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("{kind} not found: {id}")]
    NotFound { kind: ResourceKind, id: String },
    #[error("{message}")]
    Validation { message: String, field: Option<String> },
    #[error("{message}")]
    Conflict { message: String },
    #[error("version mismatch: expected {expected}, current {current}")]
    PreconditionFailed { resource: ResourceKind, expected: u64, current: u64 },
    #[error("device {device_id} unavailable: {reason}")]
    DeviceUnavailable { device_id: DeviceId, reason: String },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
```

- `impl IntoResponse for DomainError` — the codebase's first. Canonical routes render the single error envelope `{ error: { code, message, details }, meta }`; `PreconditionFailed` → 412 with `ETag` set by the REST adapter. **Legacy paths keep their legacy error projections** (top-level `current`) via a thin per-path shim, per §0.
- `impl From<DomainError> for McpToolError`; WS command results serialize the same `ApiErrorBody` on the v2 handshake, legacy string form on v1.
- `ApiError` (the `Response` factory) is deleted; `Result<T, Response>` is forbidden and lint-gated post-migration.

### 2.2 Service signatures

```rust
// Phase 2 (transition): receiver is &AppState. Phase 6.4 narrows mechanically to per-domain contexts.
pub async fn apply_effect(state: &AppState, cmd: ApplyEffect, meta: MutationContext)
    -> Result<EffectApplied, DomainError>;

pub struct MutationContext { pub trigger: ChangeTrigger /* Api | Mcp | Ws | Cli | Session | Startup */ }
```

- Command/outcome types live in `hypercolor-types::api` **when genuinely isomorphic to the wire**; otherwise the domain type is canonical and the wire type is a projection (§4.1). Transport provenance rides in `MutationContext`, never inside command payloads.
- No Axum, no `serde_json::Value`, no `Response` in domain signatures. Every mutating service returns a typed outcome — the 44 untyped `json!` payloads become named types mechanically.
- Target receivers after Phase 6.4: `apply_effect(ctx: &EffectContext, …)`. Workers do not invent contexts early.

### 2.3 Scene mutation: owned candidate, brief lock scopes, durability receipts

The existing durability model is generation-based convergence: admitted writes that fail persist stay authoritative and retry (`persistence.rs:637`, `scene_store.rs:84`). The mutation API matches it — no drop-rollback, no lock held across I/O:

```rust
pub struct SceneMutation { /* owned candidate scene state + base revision + pending events */ }

impl AppState {
    pub async fn begin_scene_mutation(&self) -> SceneMutation;          // snapshot under brief read lock
}
impl SceneMutation { /* intent methods: upsert_primary_group, set_zone_controls, … — record events, bump versions structurally */ }

pub enum CommitDurability { Written, Superseded, Retrying }
pub async fn commit_scene(state: &AppState, m: SceneMutation)
    -> Result<SceneCommit, DomainError>;   // CAS on base revision under brief write lock → admit → release → persist/publish
                                            // Err = rejected BEFORE admission (validation, revision conflict).
                                            // SceneCommit carries CommitDurability for post-admission state.
```

Dropping an uncommitted `SceneMutation` discards a local candidate — nothing to roll back. The 49 open-coded rollback/admit/save/publish rituals collapse into `commit_scene`; `controls_version`/`groups_revision` bumps live inside intent methods. Phase 6 re-points commit at the widened frame-boundary transaction without changing callers.

**Commit ordering:** a commit generation is assigned at admission, under the CAS lock. Plan publication and pending events route through one ordered commit sequencer, so two commits that admit in sequence can never publish in reverse order after their async persistence completes out of order. Events are stamped with the commit generation; a superseded commit's events are dropped by the sequencer. Event ordering and persistence durability (`CommitDurability`) are separate axes.

### 2.4 Transport adapters

- **REST:** parse → service → envelope. ETag attached by one layer keyed on `trait Versioned { fn version(&self) -> u64; }`.
- **MCP:** arg parse + one service call; fuzzy name-matching stays an MCP-adapter concern. The 17 dead stateless stubs and `execute_tool` are deleted.
- **WS commands:** call services directly (HTTP-replay bridge deleted); `Versioned` carries versions in-band.
- **CLI:** gains `hypercolor-types`; deserializes `ApiResponse<Outcome>`; renders `error.code`/`error.message`.

---

## 3. Config authority

### 3.1 One load path

```rust
pub struct ConfigSources { pub file: Option<PathBuf>, pub cli: CliOverrides, pub env: EnvOverrides }
pub struct LoadedConfig { pub boot: BootConfig, pub manager: ConfigManager }

impl ConfigManager {
    pub async fn load(sources: ConfigSources) -> Result<LoadedConfig, ConfigError>;
    // parse → migrate → normalize (INCLUDING builtin driver-entry seeding) → overlay cli/env → validate
}
```

`startup::load_config` is deleted; precedence (CLI > env > file > defaults) is defined once; `HYPERCOLOR_CONFIG` is honored by daemon and CLI alike. **Every** path that materializes a config — load, set, reset — runs the same normalize.

**Full reset** is defined as: defaults **plus an explicit copy of the current `drivers` map, flattened extension sections, and the `include` list**, then normalize. The preserved set is exactly the user-authored, unreconstructable state — anything a save would silently destroy. Regression fixtures prove arbitrary driver settings (including nested tables and array-of-tables), unknown extension documents, and the include list survive reset round-trips. (Re-normalizing alone cannot reconstruct settings — normalization only inserts missing defaults.) Full reset deliberately skips driver-scope validation: pre-existing invalid entries are preserved, never a reason to refuse the reset — the escape hatch must not be lockable by bad state it didn't create.

### 3.2 Boot/Live split

- `LoadedConfig.boot` is **consumed by value** during `DaemonState::initialize` — after init, no live handle to a `BootConfig` exists; that is the enforcement, not a runtime check.
- The retained manager exposes: `desired_boot()` (what the file now says), `pending_restart()`, and `live() -> LiveConfigSnapshot` — a zero-copy wrapper implementing `Deref<Target = LiveConfig>` so ArcSwap never leaks into the public contract.
- **Boot provenance:** per-layer source provenance (default/file/env/CLI) is retained from load. Boot-key state reports `(effective value, persisted value, overriding source, activation status)`, and `pending_restart()` is computed after re-applying sticky overlays (env/CLI that will persist across the next restart) — it never reports a restart that would change nothing, and never omits a persisted change an overlay masks.
- `subscribe(section: LiveSection) -> watch::Receiver<()>` replaces the zero-subscriber `ConfigChanged`. Storing a `LiveConfig` clone in a long-lived struct is review-rejected.

### 3.3 Key registry: macro-generated descriptors, wildcard namespaces

A static leaf table cannot cover dynamic driver IDs or flattened extensions, and per-leaf enumeration rots. The registry is **generated from the config types** by one declarative macro at section granularity, with wildcard namespaces for dynamic maps:

```rust
pub struct ConfigKeyDescriptor {
    pub pattern: KeyPattern,          // typed path or wildcard: "audio.*", "daemon.target_fps", "drivers.<id>.*"
    pub apply: ApplyPolicy,           // Live(LiveSection) | NextScan | Restart
    pub validate: fn(&serde_json::Value) -> Result<(), String>,
    pub redaction: Redaction,         // Plain | Secret; dynamic namespaces are DENY-BY-DEFAULT,
}                                     // driver-declared secret metadata (e.g. govee api_key) folds in
```

- Completeness test inspects **generated schema metadata** (every closed section resolves to descriptors; every dynamic namespace has a wildcard owner), not a serialized default instance.
- Daemon live-apply dispatch, `requires_restart`, and redaction all derive from descriptors; the four hand predicates and the UI mirror are deleted; clients read `GET /api/v1/config/schema`.
- Config REST becomes resource-shaped on canonical routes (`GET /config`, `GET/PUT/DELETE /config/keys/{key}`, `POST /config/reset`, `GET /config/schema`); `/config/get|set` remain as legacy aliases serving legacy shapes.

### 3.4 Storage tiers, path migration contract, persistence primitives

Three tiers: `config/` (user-authored TOML), `state/` (daemon-owned machine-local: runtime-state, driver-inventory, device-aliases, display-preferences, device-settings) at `XDG_STATE_HOME`, `data/` (user content: scenes, profiles, layouts, library, assets, attachments).

**Path migration contract (blocks any tier move):** per-file old→new path table; precedence when both exist (newest schema wins, ties → new path); one-time import with backup of the old file; atomic copy + fsync ordering; restart idempotence; rollback = old file untouched until first successful new-path write. `driver_inventory.rs:98` is the in-tree template. The migration harness lands **before** any store constructor changes. Two harness rulings are contract: **un-retired legacy files are residue, by design** — retirement happens only on an importing run, after that run's canonical write is durable; a crash in between leaves both copies readable with canonical authoritative, and later runs report the migration complete without retrying retirement (self-healing is rejected because it would make a read-only outcome mutating). And **a superseded import yields the winner**: when a concurrent admission supersedes the migration's write, the harness discards the imported payload, re-reads canonical, and returns the winning document — a superseded payload is one the retry supervisor will never persist, so proceeding with it in memory is data loss. Legacy sources that are projections of a file another store owns use a retain disposition (no backup, source never mutated).

**Persistence: share the primitive, not the policy.** Stores keep their domain-specific behaviors (forward-schema refusal, quarantine, backups, load normalization — device_settings and driver_inventory earn their differences). What unifies:

```rust
// shared: the atomic two-phase writer (exists), registered so flush_all covers everything incl. hypercolor.toml
pub struct MutationReceipt<R> { pub result: R, pub durability: CommitDurability /* Written | Superseded | Retrying */ }
// shared codec helpers + a thin optional wrapper offering:
//   inspect<R>(&self, f: impl FnOnce(&State) -> R) -> R          // no guard escapes across awaits
//   try_mutate<R>(&self, f: impl FnOnce(&mut State) -> Result<R, DomainError>) -> Result<MutationReceipt<R>, DomainError>
```

Only pre-admission failures are `Err`; post-admission write failures surface as `Retrying` (matching the retry supervisor). The six `Arc<RwLock<HashMap>>`+`PathBuf` sibling pairs in AppState become owned store types. `global_brightness` gets one persisted home (`device-settings`) with the watch channel as live projection (§6.6).

Client shadows: one `servers.toml` parser shared by app + tray; `cli.toml`/`tui.toml` adopt `core::config::paths` (killing the literal-`~` fallback) and drop shadowed keys.

---

## 4. Shared contracts: `types::api` and wire projection

### 4.1 Coverage and projection rules

- `types::api` grows to full domain coverage (~28 modules). The 22 daemon↔UI mirrors, 3 TUI shadows, and the UI's divergent `ActiveEffectResponse` are deleted.
- **Projection rule:** a wire struct exists only when the wire shape differs from the domain shape. Conversions are `From<&Domain> for Wire` (outbound, infallible) and `TryFrom<Wire> for Domain` (inbound, validating). Round-trip tests state which direction is lossless. Enforcement is review discipline plus the round-trip tests — no pretend grep-CI.

### 4.2 Identity — matched to reality

```rust
// Uuid-backed (true today): DeviceId, SceneId, ZoneId, EffectId, SceneLayerId, AssetId, PresetId, PlaylistId, PlaylistItemId
uuid_id!(DeviceId, SceneId, ZoneId, EffectId, SceneLayerId, AssetId, PresetId, PlaylistId, PlaylistItemId);
// String-backed opaque (reality is stringy): LayoutId ("default", slugs), ProfileId ("prof_…"),
// LayoutDeviceId (driver-derived stable strings), BackendId
string_id!(LayoutId, ProfileId, LayoutDeviceId, BackendId);
```

Both macros give `Display + FromStr + serde + ToSchema(feature)`; uuid ids expose `AsRef<Uuid>` (no `AsRef<str>` — a Uuid newtype cannot lend a string). `string_id!` takes a **per-identity validator and canonicalization policy** (grammar, trimming, case, forbidden delimiters): `FromStr` enforces it for newly minted ids, while legacy persisted forms load through a migration reader — constructors are never weakened to admit history. Physical routing gets `pub struct OutputRef { pub backend: BackendId, pub device: DeviceId }` with `Display`/`FromStr` for the `"backend:device"` wire form — `FromStr` splits on the first `:` only and `BackendId`'s grammar forbids `:`, so the form is unambiguous. Layout identity keeps `LayoutDeviceId` and is **not** rewritten to OutputRef.

### 4.3 Envelope, pagination, errors — canonical routes only

```rust
pub struct ApiResponse<T> { pub data: T, pub meta: ResponseMeta }
pub struct ApiErrorBody  { pub error: ApiErrorDetail, pub meta: ResponseMeta }
pub struct ListResponse<T> { pub items: Vec<T>, pub total: u64, pub page: Option<PageInfo> }  // None = complete, honestly
pub struct PageInfo { pub offset: u64, pub limit: u64, pub has_more: bool }
```

Canonical routes adopt these; **v1 paths keep emitting the current `pagination { offset, limit, total, has_more }` block and current error shapes** until deprecation, enforced by the compat matrix tests. `/health` keeps its bare probe shape.

### 4.4 Naming

- `apply` vs `activate` is a real semantic split and both stay: `apply` = layer a thing onto current state (effects, layouts, profiles, presets); `activate` = switch the exclusive current (scenes, playlists). Documented, not renamed.
- `current` → `active`: new canonical routes use `active`; `/effects/current/*` remains a legacy alias.
- Scene render groups are `zones` in every canonical path and message; `/groups/` paths remain legacy aliases.
- Path params: `{id}` for the resource's own id; children as `{zone_id}`-style.

### 4.5 Unified `ControlValue` — canonical semantic contract

One canonical value algebra in `types::control`; both current systems become projections of it. Canonical decisions, made once:

The canonical algebra is the **typed union of both real algebras — variant identity is preserved**, so every VALID driver-surface and effect-control value round-trips byte-identically on its own wire (legacy values that fail canonical validation stay readable as raw legacy values per §0 — kept, surfaced as diagnostics, never dropped). (The driver algebra verified at `controls.rs`: `Null, Bool, Integer(i64), Float(f64), String, SecretRef(String), ColorRgb([u8;3]), ColorRgba([u8;4]), IpAddress, MacAddress, DurationMs(u64), Enum, Flags(Vec<String>), List, Object, Unknown`.)

```rust
pub enum ControlValue {
    Null,                              // driver algebra; effect projection REJECTS inbound
    Bool(bool),
    Int(i64),                          // canonical width; effect wire narrows to i32 via range-checked TryFrom
    Float(f64),                        // canonical width; effect wire narrows to f32, finite-checked
    Text(String),
    SecretRef(SecretRef),              // reference into the credential store — the secret itself never transits; distinct from Text for redaction
    Ip(IpText),                        // validated-on-construction string wrapper (.addr() -> IpAddr); ORIGINAL TEXT PRESERVED for byte-equal round-trips
    Mac(MacText),                      // validated-on-construction string wrapper; original text preserved
    Duration(std::time::Duration),     // driver wire: DurationMs(u64)
    ColorRgb(Rgb),                     // encoded sRGB bytes — driver identity preserved
    ColorRgba(Rgba),                   // encoded sRGB bytes + alpha — driver identity preserved
    ColorLinear(LinearRgba),           // effect algebra's linear-light f32 color
    Gradient(Vec<GradientStop>),       // effect-only; driver projection REJECTS
    Rect(NormalizedRect),              // effect-only; driver projection REJECTS
    Enum(String),
    Flags(Vec<String>),                // driver algebra is ordered — Vec, not a set
    List(Vec<ControlValue>),
    Map(BTreeMap<String, ControlValue>),
    Unknown,                           // unit — round-trips at enum identity; the legacy deserializer already drops unrecognized payloads at the wire
}
```

**Conversion matrix** (typed errors, never silent coercion):
- **Finite-only floats are an invariant, not a loss:** serde_json serializes NaN/Infinity as `null`, so a non-finite float silently degrades on every JSON wire and can never round-trip today; canonical `Float` is validated finite at construction, and sensor-resolution sanitizes (clamp or reject) before values enter the set.
- canonical → **effect wire** (externally tagged snake_case, f32/i32): fails on width overflow, `Null`, and driver-only variants (`SecretRef`, `Ip`, `Mac`, `Duration`, `ColorRgb`, `ColorRgba`, `Flags`, `List`, `Map`, `Unknown`). Effect "color" maps to `ColorLinear`.
- canonical → **driver wire** (`kind`/`value` tagged, f64/i64): fails on effect-only variants (`ColorLinear`, `Gradient`, `Rect`). Each driver variant projects to its existing `kind` tag byte-identically.
- **Color conversions between the three color variants** are explicit methods through `hypercolor-color` (`ColorRgb ↔ ColorLinear` etc.); `u8 → linear f32 → u8` round-trips exactly (property-tested in the color vectors). Projections never convert color space implicitly — variant identity is what round-trips.
- **Persisted files** (scenes, profiles, presets, device-settings) keep their current encodings as projections; the write side flips per §0 doctrine only after all supported readers accept canonical.

The full per-variant validation table (ranges, defaults, widget mapping) is authored by Fable in wave 2.0 before any worker consumes it.

### 4.6 OpenAPI

One mechanism: route registration goes through a helper that records `(method, path, operation)` into a catalog; a test asserts catalog ≡ documented operations; raw `.route(` calls are grep-gated. The 142-entry hand table is deleted. (Walking an Axum router is not a public API; the helper is the implementable path.)

---

## 5. WebSocket protocol promotion

All client-visible WS types move to `hypercolor-leptos-ext::ws` (feature `ws-core`), including the metrics payload tree (deleting the UI's 16 mirrors).

```rust
// One declarative registry generates WsChannel, typed config/patch enums, validation,
// ack projection, and relay dispatch — runtime string dispatch from compile-time decls:
define_ws_topics! {
    events            { key: (),        config: EventsConfig,        codec: JsonOnly },
    frames            { key: (),        config: FramesConfig,        codec: FramesCodec },      // owns tag 0x01
    spectrum          { key: (),        config: SpectrumConfig,      codec: SpectrumCodec },    // owns 0x02
    metrics           { key: (),        config: MetricsConfig,       codec: JsonOnly },
    display_preview   { key: DeviceId,  config: DisplayPreviewConfig, codec: PreviewCodec },    // PreviewCodec owns its full tag SET (0x03, 0x05–0x11: canvas/screen/viewport/display/zone/wide/chunk/cancel/extended; 0x04 unassigned)
    interactive_preview { key: PreviewId, config: InteractiveConfig,  codec: PreviewCodec },
    /* … */
}
pub struct Subscription<T: WsTopic> { pub key: T::Key, pub config: T::Config }  // T::Key = () for unkeyed — invalid states unrepresentable
```

- Registry entries declare the **full** associated-type set: `Key`, `Config`, `Patch` (with transactional tri-state application — a patch validates completely before any field lands), `Item` (relay payload), the relay source, and the ack projection — so the macro generates dispatch and validation with no hand-unrolled blocks left.
- A codec owns its complete tag set (preview topics legitimately span multiple tags); a registry-wide assertion enforces **unique wire-tag ownership** across topics — display and interactive preview share `PreviewCodec` mechanics but own disjoint tag subsets. Every tag and byte layout is frozen by golden fixtures **before** extraction. The hand-written spectrum header is replaced by `SpectrumFrame::encode` (layout already proven equal by test).
- Cadence: one internal `Cadence` type; v1 accepts both `fps` and `interval_ms`, v2 subscribe (negotiated via a version field in the handshake) speaks `fps` only.
- Keyed subscriptions unify `display_preview` (today 1-per-connection) and `interactive_preview` (today a bespoke session protocol) into N-concurrent keyed subscriptions on v2; v1 message forms stay dual-accepted through deprecation.
- Adding a topic = one `define_ws_topics!` entry + one relay fn.

---

## 6. Engine contracts

### 6.1 Commit-stable plan snapshot; render-local frame state

```rust
pub struct ScenePlanSnapshot { pub generation: u64, /* commit-stable ONLY: scene structure, control sets, layouts, transition SPECS */ }
pub struct SnapshotReader(/* ArcSwap */);
impl SnapshotReader { pub fn load(&self) -> Guard<Arc<ScenePlanSnapshot>>; }   // borrowed guard held for the frame — no per-frame Arc clone
pub struct FrameState { /* render-thread-local: transition progress, elapsed, frame token, adaptive FPS */ }
```

Per-frame-varying state (transition progress advances every frame) never lives in the published snapshot. The control plane republishes on every `commit_scene`. Transitions publish as a `TransitionPlan` with a **stable identity** (epoch + from/to endpoints); `FrameState::reconcile(old, new)` preserves progress iff the transition identity is unchanged — a control-only commit mid-transition continues it; re-activating the same scene mints a fresh epoch and restarts. The render loop takes **zero locks**; `RenderLoop`/`PerformanceTracker` internalize, exposing atomics/ArcSwap outward. **Benchmark gate:** frame-time p99 vs baseline capture.

### 6.2 Output data-plane split — display lanes included

Control plane: `RwLock` routing table, short holds. Data plane: per-device output queues (existing `DeviceOutputCoordinator`) own I/O; no manager-wide lock across hardware writes. **Display-frame delivery joins the same data plane**: display payload lanes share queue generation, retry classification, and telemetry with LED output — `display_output/worker.rs`'s parallel pipeline (own retry, own sink lookup, `BackendIo` fallback) is absorbed, closing the second-output-pipeline seam.

### 6.3 Manager idiom

```rust
#[derive(Clone)]
pub struct SceneService(Arc<SceneServiceInner>);   // lock INSIDE; intent methods; owned snapshots out
impl SceneService { pub fn subscribe(&self) -> SceneEventReceiver; }   // observation only — the EventSink is PRIVATE, injected at construction
```

Mutations publish internally; callers cannot forge, reorder, or double-publish. Conversion order: SceneManager → SpatialEngine → EffectRegistry → InputManager. `SessionWatcher`/`usb_hotplug` side-buses become bus lanes or documented internal transports.

### 6.4 Domain contexts

`SceneContext`, `DeviceContext`, `EffectContext`, `OutputContext`, `PlatformContext` — `#[derive(Clone)]` handle structs. `DaemonState`/`AppState`/`RenderThreadState`/`DaemonDriverHost` become context holders; one construction path; the 22-arg constructor and the 74-line manual mirror die. Service receivers narrow from `&AppState` to per-domain contexts here (§2.2).

### 6.5 Controls: one authority, delta delivery

```rust
pub struct ControlSet { /* BTreeMap<ControlId, ControlValue> + set_revision: SetRevision */ }  // ONE authority, owned by the effect slot
pub struct ControlDeltaBatch<'a> {
    pub set_revision: SetRevision,     // authoritative Zone.controls_version — bumps on patch/binding changes
    pub resolution_seq: u64,           // orders sensor-resolved value changes BETWEEN patches (same revision, new values)
    pub changes: &'a [(ControlId, ControlValue)],
}
// renderer contract (replaces set_control):
fn initialize_controls(&mut self, revision: SetRevision, controls: &ControlSet) -> Result<(), ControlError>;
    // REQUIRED before the first frame after create/rebuild — the snapshot-first baseline (today's sync_layer_state replay, made contractual)
fn apply_controls(&mut self, batch: &ControlDeltaBatch<'_>) -> Result<(), ControlError>;
    // ATOMIC: all-or-nothing. Failure = renderer invalidation + snapshot replay via initialize_controls, never partial application.
```

The slot's `ControlSet` is authoritative; renderers receive **changed resolved values in ordered atomic batches** (matching today's change-gated dispatch at `pool.rs:471`; `resolution_seq` orders sensor-driven changes that don't bump the authoritative revision) and may keep compiled typed fields as derived caches — never as authoritative storage. `ControlSet` does not ride in `FrameInput` (that would create a second authority and per-frame map interpretation at 60fps). The EffectSlot mirror-diffing, in-place registry-metadata mutation, and write-only renderer state die. Depends on the ControlValue unification (§4.5 → wave 2.0).

### 6.6 Lanes and power

```rust
pub struct WatchLane<T> { sender: watch::Sender<T>, published: AtomicU64, revision: AtomicU64 }
// receiver count comes from sender.receiver_count() — no duplicate atomic to drift
pub struct BusLanes {                       // heterogeneous — typed aggregate, not a uniform map
    frame: WatchLane<FrameData>,
    spectrum: WatchLane<SpectrumData>,
    previews: EnumMap<PreviewKind, WatchLane<CanvasFrame>>,   // homogeneous subset only
    screen_zones: WatchLane<ScreenZonesFrame>,
    zone_preview: WatchLane<Arc<[ZonePreviewFrame]>>,
}
```

Internal-vs-external preview demand is modeled by subscription guards outside the lane. `PreviewRuntimeSnapshot` becomes per-lane stats keyed by kind.

`OutputPower`: owning type over the watch channel + private transition mutex + device-settings store. `set_global_brightness` writes the store first, then publishes; failure leaves the channel unpublished. The 12 free functions become methods; the three-home brightness collapses to one persisted home + live projection.

---

## 7. Driver boundary contracts

### 7.1 Registration and output binding

```rust
pub trait DriverModule: Send + Sync {
    fn descriptor(&self) -> DriverModuleDescriptor;
    fn discovery(&self) -> Option<&dyn DiscoveryCapability>;
    fn output(&self) -> OutputBinding<'_>;
    fn controls(&self) -> Option<&dyn DriverControlProvider>;   // reachable by ALL families
    fn config(&self) -> Option<&dyn DriverConfigProvider>;
}
pub enum OutputBinding<'a> {
    Owned { id: BackendId, factory: &'a dyn DeviceBackendFactory },  // factory, not a live backend; the id is DECLARED
    Shared(BackendId),                     // HAL fan-in: N modules name one usb/smbus/blocks backend
    None,
}
pub trait DeviceBackendFactory: Send + Sync {
    fn build(&self, host: &dyn DriverHost, config: DriverConfigView<'_>) -> Result<Arc<dyn DeviceBackend>, DriverError>;
}
```

**Registry finalization rules:** exactly one provider per `BackendId` (duplicate providers are a registration error); a `Shared(id)` with no provider is rejected at finalization, not discovered at first write; a provider is built iff its own module or any enabled consumer names it.

`DeviceBackend` methods move to **`&self`** (self-synchronizing handles — per-device actors/queues internal), which is what makes `Arc<dyn DeviceBackend>` sound; the `Arc<Mutex<Box<dyn>>>` wrapper and its lock-across-I/O die with it. This lands with the §6.2 data-plane split. `usb`/`smbus`/`blocks` become driver modules; the daemon's hardcoded backend blocks and scanner factory are deleted; HAL devices gain `ControlSurfaceDocument` surfaces (un-stranding `DeviceFeatures`/scroll; DPI gets a home when it lands).

### 7.2 Discovery

- `TransportScanner`, `DriverDiscoveredDevice`, `DriverModuleScanner`, `DeviceBackend::discover` — deleted. `DiscoveryCapability` produces `DiscoveredDevice`; the registry is the only inventory; backends learn devices via `fn adopt_device(&self, d: &DiscoveredDevice) -> Result<(), DeviceError>` (required, fallible).
- One fingerprint constructor: `Fingerprint::mint(ns: FingerprintNamespace /* Usb|SmBus|Net|Cloud|Bridge */, driver: &str, key: &str)`. Existing persisted fingerprints are grandfathered; per-driver compat is documented.

### 7.3 Typed errors, whole boundary

`DeviceBackend` fallible methods return `Result<_, DeviceError>` (`#[non_exhaustive]`; gains `Timeout { after }`, `NotAdopted`; `fn recoverability(&self)`). Discovery, pairing, config, and factory construction return `Result<_, DriverError>` (non-exhaustive, same treatment) — retry/lifecycle policy branches on variants **before** device I/O begins, and both substring classifiers die. Retry ladders consolidate under `DeviceLifecyclePolicy`; backends stop running private reconnect loops.

### 7.4 Hygiene

`hypercolor-driver-api` = traits + types only; new `hypercolor-driver-support` = credentials, mdns, control-surface builders, pairing plumbing. Four dead `hypercolor-core` deps deleted. `ProtocolZone` deleted (`Protocol::zones() -> Vec<ZoneInfo>`; 4 byte-identical converters die). `DevicePlugin` deleted. `health_check` **deleted** (decided; `DeviceStateMachine::on_comm_error` is the future home if ever needed). `GoveeConfig` → govee crate via `DriverConfigProvider`; `PortableIdentityClaim` carries a driver-supplied namespace.

---

## 8. Implementation plan

### 8.1 Team model

- **Fable:** owns this spec; writes the contract-bearing code first in each phase; final signoff on every wave.
- **Opus 5 workers:** one per wave, isolated worktrees (`~/dev/worktrees/hypercolor/nova/<branch>`), mechanical migration + tests against locked contracts. Contract friction escalates to Fable — never patched around.
- **Codex:** inline review per PR (`codex review --base main`); focused security pass on credential-store and redaction waves.
- **Gates per wave:** `just verify` green; contract round-trip + golden fixtures green; vectors green (color); benchmark gate (engine); codex findings resolved or explicitly waived by Fable; Fable signoff → merge.

### 8.2 Phases and waves

**Phase 0 — quick strikes + compat foundations** (small PRs; foundations block later phases)
0.1 Fix config-reset data loss: defaults + explicit copy of current `drivers`/`extensions` + normalize; regression fixtures. **Ships first.**
0.2 Delete 4 dead `hypercolor-core` driver deps.
0.3 Delete `color_wave.rs` LUT copy (use `blend_math` pubs).
0.4 Delete dead MCP stateless dispatch (17 stubs + `execute_tool`).
0.5 Delete `DevicePlugin` and `health_check`; delete `types/palette.rs` (zero consumers).
0.6 Fix literal-`~` config fallback (CLI/TUI adopt `core::config::paths`).
0.7 **REST v1 compat matrix** artifact + tests freezing current v1 shapes (pagination block, 412 bodies, legacy paths).
0.8 **WS golden fixtures** freezing every binary tag (0x01, 0x02, 0x03, 0x05–0x11, and the leptos-ext RPC tags 0x80/0x81; 0x04 is deliberately unassigned) and byte layout.
0.9 **Path-migration harness** (old→new path, precedence, backup, idempotence) modeled on `driver_inventory.rs:98`.

**Phase 1 — color** (after 0.3; independent otherwise)
1.1 Fable: `hypercolor-color` crate + full API (§1) + palette-oracle property tests + vectors schema.
1.2 Worker: Rust hex parsers (11) + luminance (6) + `scale` (6; named behavior change: truncating sites shift ≤1 LSB) + `encode` (2).
1.3 Worker: HSV/HSL sites (14) with per-surface adapters; `blend_over` + Oklab absorption; transfer LUTs.
1.4 Worker: TS color module + barrel + effect migrations (10 hex, 8 HSL, 11 clamp) + GLSL prelude wiring.
1.5 Worker: `color-vectors.json` population + dual-language CI.

**Phase 2 — contract conventions + domain services** (conventions FIRST — consumers after foundations)
2.0 Fable: `types::api` conventions (`ApiResponse`, `ApiErrorBody`, `ListResponse`, `uuid_id!`/`string_id!`, `OutputRef`) **and** unified `types::control` ControlValue (internal type + legacy wire projections per §0 compat doctrine; persisted-file migration with fixtures).
2.1 Fable: `DomainError` + `IntoResponse` + `Versioned` + `MutationContext` + adapter conventions + legacy-projection shims.
2.2 Worker: kill `Result<T, Response>` (~28 sites) and unify error shapes on canonical routes.
2.3a Worker: `SceneMutation`/`commit_scene` + lift `apply_effect` + `activate_scene`; delete their MCP twins.
2.3b Worker: lift `create_scene`, zone mutation, `set_display_face`; migrate the remaining ritual sites (49 total across a+b).
2.4 Worker: name the 44 untyped payloads into `types::api`; CLI gains `hypercolor-types` (~130 typed sites).

**Phase 3 — contract rollout** (after 2)
3.1 Worker(s): grow `types::api` to full coverage **in per-domain batches** (devices+scenes; effects+library; layouts+displays+assets; drivers+system), daemon + UI + TUI mirrors deleted in the same PR per batch.
3.2 Fable: `define_ws_topics!` registry + codec/tag contracts. Worker: promote WS types to leptos-ext, migrate subscribes, keyed v2 subscriptions with v1 dual-accept — split extraction / topics / clients into separate PRs.
3.3 Worker: honest pagination + naming aliases on canonical routes (legacy projections stay); registration-helper OpenAPI catalog (142-entry table dies).

**Phase 4 — config** (after 0.9)
4.1 Fable: `ConfigSources`/`LoadedConfig`, Boot/Live types, descriptor macro + completeness design.
4.2 Worker: single load path; delete `startup::load_config`.
4.3 Worker: descriptor-driven apply dispatch + `/config/schema` + resource routes; delete predicates + UI mirror.
4.4 Worker(s): storage tiers **per-store batches** behind the migration harness; `flush_all` totality; `servers.toml` unification; dead sections removed.

**Phase 5 — types restructure** (after 3)
5.1 Worker: evictions (surface pool → daemon; DeviceHandle → core; SerialNormalizerRegistry → core; capture validation → core; Zone legacy codec → migration layer).
5.2 Worker: merges (authored BlendMode; topology; `Rect<T>`; ColorFormat; audio configs; telemetry); `config.rs` → per-section files; gate `utoipa`; single import path.

**Phase 6 — engine** (after 2; benchmark-gated)
6.1 Fable: `ScenePlanSnapshot`/`FrameState` contract. Worker: zero-lock render loop.
6.2 Worker: output data-plane split **including display lanes**; `&self` backend handles land here with §7.1.
6.3 Worker: manager idiom conversions (SceneManager first) + private-sink events.
6.4 Worker: domain contexts; God structs shrink; service receivers narrow.
6.5a Worker: `BusLanes`/`WatchLane` + `OutputPower` + worker-handle triple.
6.5b Worker: `ControlSet` authority + `apply_control` delta migration (12 renderer impls). Depends on 2.0.

**Phase 7 — drivers** (after 0; 7.1-7.2 after 6.2's `&self` conversion)
7.1 Fable: `DriverModule`/`OutputBinding`/`DeviceBackendFactory` + `DeviceError`/`DriverError` finals.
7.2 Worker: usb/smbus/blocks as modules; hardcoded factory deleted; HAL control surfaces.
7.3 Worker: discovery collapse; `adopt_device`; fingerprint constructor.
7.4 Worker: typed errors boundary-wide; substring classifiers die; retry consolidation.
7.5 Worker: driver-api/support split; `ProtocolZone` deletion; vendor eviction from types.

Dependency graph: 0 → everything; **1.1 → 2.0** (the canonical ControlValue's color variants need `hypercolor-color`'s types — the crate skeleton lands before wave 2.0 compiles); 2.0 → {2.4, 3.1, 6.5b}; 2.1 → 4.3 (canonical config routes need the response contracts and adapter shims); {2.1, 2.3a} → 4.4 (`DomainError`/`CommitDurability` in the persistence primitives); **2.0 → 4.4's device-settings batch** (the ControlValue content migration lands before the path relocation — one store never migrates content and location in the same release without combined upgrade/crash-restart/rollback/both-path fixtures); 2 → {3, 6}; 3 → 5; 6.2 → 7.1/7.2. Phase 1 runs fully parallel to the 2→3 track; Phase 4's 4.1–4.2 run parallel, 4.3–4.4 wait on their Phase-2 edges. Every wave is an atomic-commit PR from a lane worktree, sized for single-PR review.

## 9. Decisions resolved at lock

1. **Spec number:** 76 — confirmed against `docs/specs/` (highest existing: 75). Internal-repo numbering is independent; the internal repo consumes this repo as the `oss/` submodule.
2. **Legacy Zone serde codec retirement:** migrate-on-load rewrite plus one release of read compatibility, then delete. Wave 5.1 executes the move to the migration layer; the deletion rides the release after the rewrite ships.
3. **RGBW white extraction:** out of scope for this program. It ships as its own feature with its own `DevicePixelLayout` variant after Phase 1; `RgbwZeroWhite` names today's behavior honestly until then.

## 10. Review history

- **Round 1 (2026-08-15):** three lens-locked codex passes (gpt via `codex exec`, high effort, read-only) over rev 1 + author self-pass. 29 codex findings + 11 self-pass items; verdicts NEEDS REWORK on §§2.3, 3.1-3.4, 4.2-4.4, 5, 6, 7, 8. Dispositions: all 29 adopted (several merged/adjusted: A5 partially — phased receivers kept, `MutationContext` split adopted; C8+A4 merged with author's prefix simplification into macro-generated descriptors; B5 merged with author's lane-key design). Author self-pass items all applied, incl. palette.rs deletion, verb-split retention, OpenAPI single-mechanism. Key load-bearing findings independently verified against the tree before adoption (string ids, dual ControlValue encodings, retry-convergence durability, `&mut self` backend receivers).
- **Round 2 (2026-08-15):** two verification passes over rev 2. Ledger: all 18 contract-quality findings (A1–A8, B1–B10) ADDRESSED; 10/11 compat findings ADDRESSED, C3 short (writer flip unspecified). 12 new items, all adopted in rev 3: writer-flip doctrine (§0), canonical ControlValue contract (§4.5), commit-sequencer ordering (§2.3), boot provenance (§3.2), string_id validators + OutputRef grammar (§4.2), WS registry full associated types + tag-uniqueness assertion (§5), TransitionPlan identity + reconcile (§6.1), snapshot-first control protocol + atomic delta batches (§6.5), Owned BackendId + registry finalization (§7.1), Phase-4 dependency edges + 2.0→4.4 migration ordering (§8.2). Finding character shifted from wrong-contract to underspecified-edge — converging.
- **Round 3 (2026-08-15):** certification pass over rev 3. Ledger: 11/12 round-2 items ADDRESSED; two residues, both adopted in rev 4: §4.5's canonical enum rebuilt as the typed union of both real algebras (verified against `controls.rs` — `SecretRef` reference semantics, `ColorRgb`/`ColorRgba`/`ColorLinear` identity preservation, `Ip`/`Mac`/`Duration`/unit-`Unknown`), and the missing `1.1 → 2.0` dependency edge added.
- **Round 4 (2026-08-15):** narrow check on the two rev-4 fixes. Dependency edge PASS; §4.5 two residues fixed in rev 5: `Ip`/`Mac` become validated-string wrappers preserving original text for byte-equal round-trips, and finite-only floats documented as a wire invariant with sanitization at sensor resolution.
- **Round 5 (2026-08-15):** final micro-check. `IpText`/`MacText` PASS; one factual correction adopted verbatim from the reviewer (serde_json emits `null` for non-finite floats rather than erroring — the invariant holds, with the corrected mechanism). **Converged: rev 5 locked.** Convergence trajectory: 29 findings → 12 → 2 → 2 sub-points → 1 factual citation.
- **Round 6 (2026-08-16, implementation reconciliation):** corrections forced by the tree during wave 0.8 and wave 1.1, folded back so spec and code agree. §1: `std` feature dropped (std-only until a no_std consumer exists). §1.3: `Oklch::{to_linear, lerp}` and `LinearRgba::to_oklch` added — shortest-path Oklch interpolation exists in-tree and is load-bearing. §1.4: per-type accepted hex digit counts made explicit. §1.5: `PixelBlendMode` is the kernel's real alpha-composable set (`ColorDodge`/`Difference`, not the sketch's `Lighten`/`Darken`). §5/§8.2: WS tag inventory corrected to the code's enumeration (0x04 unassigned; RPC tags 0x80/0x81 exist in leptos-ext and are frozen). Phase 0 execution rulings folded into contracts: §3.1's preserved set gains the `include` list plus the validation-skip escape-hatch rule (wave 0.1); §3.4 gains the residue-by-design retirement contract and superseded-import-yields-the-winner rule (wave 0.9).
