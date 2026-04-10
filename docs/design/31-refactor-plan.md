# 31 · Hypercolor Workspace Refactor Plan

**Status:** Active · **Owner:** Swarm-orchestrated · **Updated:** 2026-04-10

This document tracks the structural refactor initiative identified by the
2026-04-10 multi-agent codebase review. It complements (not replaces) the
in-flight `28-render-pipeline-modernization-plan.md` — see Coordination
Notes for the overlap boundary.

---

## Context

A nine-agent parallel review audited the entire workspace (core, HAL,
daemon, network drivers, TUI, UI, SDK, secondary binaries, cross-cutting)
and surfaced a consistent set of pressure points:

- **File bloat.** Three files exceed 2500 LOC and a dozen more exceed
  1500 LOC. Growth is concentrated in the daemon API layer, the
  `BackendManager`, and the Servo renderer.
- **Architectural god objects.** `AppState` (31 flat fields) and
  `BackendManager` (registration + staging + routing + brightness + queue
  dispatch) have become hard to reason about.
- **Protocol-trait inconsistency in HAL.** `CommandBuffer` adoption is
  uneven, `encode_brightness` lands in 3 of 13 drivers, and RwLock
  poisoning panics cascade through the driver stack.
- **Two security issues** — Hue DTLS skip-verify and an unbounded
  WebSocket command body read.
- **WebSocket envelope schism** — REST uses `{ data, meta }`, WS commands
  use `{ id, status, data?, error? }`, WS events use `{ type, payload }`.
- **Duplicated SDK math** — `clamp`, `mix`, `smoothApproach`,
  `smoothAsymmetric`, HSV↔RGB, hash/noise are copy-pasted across 5+
  effects in both TypeScript and GLSL.
- **Disconnected design tokens** — `hypercolor-ui`'s Tailwind config is
  empty of token imports; hardcoded hex values live in component code.
- **Compliance gap** — `hypercolor-desktop` has zero tests, violating
  the CLAUDE.md mandate.

The full review findings are available in agent reports from the
2026-04-10 session. This plan translates them into actionable,
parallelizable work.

---

## Execution Model

Work is organized into **six phases** sequenced roughly by dependency,
and dispatched to agents in **five waves** to maximize parallelism while
keeping main buildable between checkpoints.

Phases describe scope and outcome. Waves describe how many agents run at
once and on which files. The distinction matters because multiple phases
can progress in parallel if their file scopes don't overlap.

### Agent Hygiene Rules

Every swarm agent MUST:

1. Own a **non-overlapping** file scope declared in its prompt. Reading
   adjacent files is fine; modifying them is forbidden.
2. Commit with `git commit -m "msg" -- <specific-files>` to bypass the
   staging area for unrelated WIP.
3. Use conventional commit format (`fix(scope):`, `refactor(scope):`,
   `feat(scope):`, `test(scope):`, `docs(scope):`).
4. Include the `Co-Authored-By: Claude Opus 4.6 (1M context)
   <noreply@anthropic.com>` trailer on every commit.
5. Verify compilation with scoped `cargo check -p <crate>` before
   committing. Never run full `just verify` — the orchestrator does that
   between waves.
6. Never `git push`, `git add -A`, `git add .`, `git stash`, or
   `git reset` against files outside the declared scope.
7. Report back with commit hash(es), files modified, a one-line
   per-commit summary, and any deferred or surprising work.

Between waves, the orchestrator runs `just check` on main, synthesizes
the agent reports, and decides whether to dispatch the next wave or
paper over gaps first.

---

## Phase 0 — Urgent Fixes

Small, targeted corrections. Ships as Wave 1.

| # | Fix | File(s) | Change |
|---|---|---|---|
| 0.1 | Hue DTLS safety | `crates/hypercolor-core/src/device/hue/streaming.rs:64` | Document PSK-auth boundary or remove `insecure_skip_verify` depending on library semantics |
| 0.2 | WS body cap | `crates/hypercolor-daemon/src/api/ws.rs:1720` | Replace `usize::MAX` with 1 MB cap |
| 0.3 | WS router caching | `crates/hypercolor-daemon/src/api/ws.rs:1674` | Cache router once via `OnceCell` if scope-contained |
| 0.4 | TUI aspect-fit | `crates/hypercolor-tui/src/views/dashboard.rs:460`, `views/effect_browser.rs:902` | Restore `aspect_fit()` call |
| 0.5 | TUI `Component::update()` | `crates/hypercolor-tui/src/app.rs:245-247` | Forward actions to active screen's `update()` |
| 0.6 | TUI resize thread shutdown | `crates/hypercolor-tui/src/app.rs:100-111` | Add shutdown signal or drop-based cleanup |
| 0.7 | Config RwLock recovery | `crates/hypercolor-core/src/config/paths.rs:22,54,65,101` | Replace `.expect("poisoned")` with graceful recovery |
| 0.8 | Track SparkleFlinger doc | `docs/design/30-sparkleflinger-implementation.md` | `git add` the untracked file |

**Exit criteria:** `just verify` passes. No regression in
`backend_manager_tests`, `pipeline_integration_tests`, or the
`core_backend_routing` bench (~0.89 µs cached / ~1.60 µs layout baseline
per `fa34dd7`).

---

## Phase 1 — Daemon Architectural Decomposition

The daemon has outgrown its flat module layout. Three files dominate and
`AppState` is a god object. This is the highest-leverage refactor in the
plan.

### 1.1 Split `api/ws.rs` (3727 LOC)

Target layout under `crates/hypercolor-daemon/src/api/ws/`:

```
mod.rs          — public WS route handler + re-exports
protocol.rs     — Hello, Subscribe, Unsubscribe, Event, Ack message types
session.rs      — upgrade, lifecycle, hello negotiation
relays.rs       — event/frame/spectrum fanout from HypercolorBus
cache.rs        — binary payload cache, preview brightness precompute
command.rs      — command dispatch via cached router
backpressure.rs — adaptive downsampling + slow-client detection
```

Sequence within the split: `protocol.rs` first (pure types, no deps),
then `session.rs`, then peel `relays.rs` / `cache.rs` (already touched
by perf commits `100cee6`, `0cf483d`, `a463ed2`). `backpressure.rs` lands
last — it's new functionality and can be scaffolded with a pass-through
implementation initially.

### 1.2 Split `api/devices.rs` (2948 LOC)

Target layout under `crates/hypercolor-daemon/src/api/devices/`:

```
mod.rs          — core CRUD (list, get, forget)
attachments.rs  — attach/detach/remap endpoints
logical.rs      — logical device group management
pairing.rs      — pairing flows (Hue button, Nanoleaf token, WLED open)
discovery.rs    — trigger/status endpoints for scanners
```

### 1.3 Decompose `AppState` (`api/mod.rs:87-206`, 31 fields)

Replace flat field access with domain facades:

```rust
pub struct AppState {
    pub device: DeviceFacade,      // backend_manager, lifecycle, reconnect
    pub effects: EffectFacade,     // engine, registry, playlist runtime
    pub spatial: SpatialFacade,    // layouts, layout links
    pub library: LibraryFacade,    // favorites, presets, profiles
    pub bus: HypercolorBus,
    pub config: ConfigFacade,
}
```

Handlers migrate from `state.backend_manager.lock().attach(...)` to
`state.device.attach(...)`. Each facade documents its own lock ordering.

### 1.4 Split other oversized daemon files

- `startup.rs` (1612 LOC) → `startup/{config,services,daemon,signals}.rs`
- `mcp/tools.rs` (1783 LOC) → `mcp/tools/{effects,devices,scenes,library,system}.rs`
- `discovery.rs` (2846 LOC) → `discovery/{usb,network,mdns,dispatcher}.rs`
- `display_output.rs` (1512 LOC) → review and split by subsystem
- `api/library.rs` (1032 LOC) → `library/{favorites,presets,playlists}.rs`

### 1.5 Normalize the WebSocket envelope

Two options:

- **A (lower risk):** Document the three shapes explicitly in
  `docs/specs/`, add unified TypeScript type guards in the SDK so clients
  can unpack uniformly.
- **B (higher payoff):** Wrap all WS messages in a single
  `{ kind, data, meta }` envelope. Breaking change; sync with TUI + UI
  clients.

**Recommendation:** Option A now. Revisit Option B when TUI/UI client
refactors land in Phase 4.

**Exit criteria:** No single file >1500 LOC in
`crates/hypercolor-daemon/src/`. `AppState` exposes only facades. Lock
ordering documented per facade.

---

## Phase 2 — HAL Consistency & Hardening

### 2.1 Remove RwLock poisoning cascade

Every `.expect("...poisoned")` in HAL is a live grenade. Affected:

- `crates/hypercolor-hal/src/drivers/corsair/link/protocol.rs:108,211`
- `crates/hypercolor-hal/src/drivers/asus/smbus.rs:361,374,384,393,456,469,501,531`
- `crates/hypercolor-hal/src/drivers/lianli/protocol.rs` (full audit)

Replace with a helper that either converts poison into
`ProtocolError::StatePoisoned` (reconnect-triggering) or migrates the
affected locks to `parking_lot::RwLock` (which doesn't poison). Audit
workspace dependencies first — parking_lot may already be pulled in
transitively.

### 2.2 Fix ASUS ENE overflow guards

`crates/hypercolor-hal/src/drivers/asus/smbus.rs:249-250, 295-296` —
move the overflow bound check upstream to before the loop body instead
of `.expect()` on `checked_add`.

### 2.3 Fix Corsair LINK silent truncation

`crates/hypercolor-hal/src/drivers/corsair/link/protocol.rs:114-134` —
`normalize_colors()` must return `Result<Vec<Rgb>,
ProtocolError::FrameSizeMismatch>` instead of silently dropping input.
Also audit `crates/hypercolor-hal/src/drivers/corsair/framing.rs:71-82`
for the `build_link_packet()` buffer size off-by-one risk.

### 2.4 Split `lianli/protocol.rs` (1367 LOC) into ENE and TL

ENE and TL share a trait but have fundamentally different wire formats.
Currently multiplexed via `match variant` on every method.

```
crates/hypercolor-hal/src/drivers/lianli/
  mod.rs
  ene.rs     — EneProtocol (Sl, Al, SlV2, AlV2, SlInfinity, SlRedragon)
  tl.rs      — TlProtocol (TlFan)
  common.rs  — shared helpers
```

### 2.5 Split `razer/devices.rs` (1821 LOC) by device family

```
crates/hypercolor-hal/src/drivers/razer/devices/
  mod.rs            — registry / dispatch
  keyboards.rs      — Huntsman, BlackWidow, Cynosa, Ornata
  mice.rs           — Basilisk, DeathAdder, Mamba, Viper
  peripherals.rs    — Tartarus, Nostromo, keypads, headsets
  laptops.rs        — Blade 14/15/17
  mousepads.rs      — Firefly, Goliathus, Strider
```

### 2.6 Standardize `CommandBuffer` adoption

Currently only Razer and Lian Li reuse buffers. Adopt `CommandBuffer`
across Corsair, ASUS, Dygma, QMK, Push2, PrismRGB — or promote it to a
required associated type on the `Protocol` trait so deviation is a
compile error.

### 2.7 Complete the `encode_brightness` contract

Only 3 of 13 drivers implement it. Make it an optional default-`None`
trait method with a documented meaning (software brightness compensation
applies when `None`), or implement it everywhere hardware supports it.

### 2.8 Split `push2/protocol.rs` (878 LOC)

```
crates/hypercolor-hal/src/drivers/push2/
  protocol.rs    — core Protocol impl
  led_palette.rs — palette encoding + cache
  display.rs     — JPEG slicing / XOR frame prep
```

### 2.9 Add display encoder integration tests

Create:

- `crates/hypercolor-hal/tests/corsair_lcd_display_tests.rs`
- `crates/hypercolor-hal/tests/push2_display_tests.rs`

Use known-good reference frames and byte-compare against expected wire
format.

**Exit criteria:** Zero `.expect()` on RwLock in HAL. All drivers use
`CommandBuffer`. `lianli` and `razer` subtrees compile without
`match variant` dispatch hot paths. Display encoders have wire-format
test coverage.

---

## Phase 3 — Network Driver Hardening (Spec 33 Completion)

### 3.1 Version the DriverDescriptor

`crates/hypercolor-driver-api/src/lib.rs` — add
`schema_version: u32` to `DriverDescriptor`. Host checks version at load
and rejects mismatches. Defines the stability contract for out-of-tree
drivers.

### 3.2 IP/port validation at discovery

Each driver's `resolve_*_probe_*_from_sources()` should reject port 0,
reserved ports (<1024 unless explicitly allowed), link-local
(169.254.x.x), multicast, broadcast, and invalid CIDR. Centralize in
`crates/hypercolor-driver-api/src/validation.rs`.

### 3.3 Retry with exponential backoff

- Hue bridge pairing (`bridge.rs:309`) — 3 attempts, 1s base
- Nanoleaf pairing window — formalize the natural retry shape
- WLED discovery — mark unreachable IPs stale after N failures

### 3.4 Surface health checks via `DeviceBackend` trait

WLED already has `health_check` at `wled/backend.rs:1015`, not exposed.
Add:

```rust
trait DeviceBackend {
    async fn health_check(&self) -> Result<HealthStatus>;
    // default: Ok(Healthy)
}
```

Daemon's reconnect task calls `health_check()` on a timer; unhealthy
backends trigger reconnection.

### 3.5 Document fingerprint scheme

Create `docs/specs/34-device-fingerprints.md`. Define format
`net:<driver>:<stable_id>`, per-driver derivation rules (Hue: bridge_id,
WLED: hostname or MAC, Nanoleaf: auth token hash), and collision
handling.

### 3.6 Split `wled/backend.rs` (1262 LOC)

```
crates/hypercolor-driver-wled/src/backend/
  mod.rs       — DeviceBackend impl
  protocol.rs  — DDP vs E1.31 state machines
  cache.rs     — device_ids, IPs, info cache
  health.rs    — keepalive + health check
```

### 3.7 Expand network driver error-path tests

Per-driver test files are currently ~80 LOC happy-path. Add:

- Pairing timeout simulation (Hue 30-second button window)
- Malformed credential cache handling
- Network failure during credential storage
- Discovery of unreachable devices

**Exit criteria:** `DriverDescriptor::schema_version` enforced. IP/port
validation lives in one place. Health checks exposed via trait.
Fingerprint spec committed.

---

## Phase 4 — SDK & UI Cleanup

### 4.1 Export shared math from SDK

`clamp`, `mix`, `smoothApproach`, `smoothAsymmetric` are redefined in
frequency-cascade, frost-crystal, fiberflies, cymatics, and digital-rain.
Move to:

```
sdk/src/math/
  index.ts       — barrel export
  easing.ts      — smoothApproach, smoothAsymmetric
  lerp.ts        — clamp, mix, lerp, saturate
```

### 4.2 Create shared GLSL library

```
sdk/shared/glsl/
  math.glsl       — hash12, hash22, fbm, valueNoise
  color.glsl      — HSV↔RGB, IQ cosine palette interpolation
  noise.glsl      — simplex, perlin variants
```

Effects use a glob-style include in the bundler to inline these at
build time.

### 4.3 Centralize palettes

`sdk/shared/palettes.json` already exists. Migrate effects to reference
palette keys instead of inlining 7-slot color arrays. Biggest wins:
Synth Horizon (12×7 = 84 inline vectors) and Frost Crystal.

### 4.4 Wire SilkCircuit tokens into Tailwind

`crates/hypercolor-ui/tailwind.config.js` is empty except for content
globs. Import tokens from `crates/hypercolor-ui/tokens/` into
`index.html` and define Tailwind `extend.colors` keyed to CSS variables.
Replace hardcoded hex in
`crates/hypercolor-ui/src/components/control_panel.rs:25-28`
(`QUICK_COLOR_SWATCHES`) with token references.

### 4.5 Extract `layout_geometry.rs` (1563 LOC) to a shared crate

Create `crates/hypercolor-layout-math/` — pure functions for device
footprints, zone shapes, polygon math. Used by both `hypercolor-ui` and
`hypercolor-daemon`. Keeps the UI crate under WASM bundle budget.

### 4.6 Decompose large UI components

| File | LOC | Split |
|---|---|---|
| `src/pages/dashboard.rs` | 1492 | `DashboardHeader`, `DashboardCharts`, `DashboardTimeline`, `DashboardGauges` |
| `src/components/control_panel.rs` | 1270 | Extract `control_kind.rs` (6 control types), context provider for shared metadata |
| `src/components/layout_palette.rs` | 1212 | `ZoneGrid`, `ZonePalette`, `ZoneColorPicker` |
| `src/ws.rs` | 1007 | `ws_connection.rs`, `ws_messages.rs`, `ws_preview.rs` |

### 4.7 Fix signal inflation

`crates/hypercolor-ui/src/app.rs:368-372` wraps `HashMap<String,
ControlValue>` and `HashSet<String>` directly in signals. Use
`RwSignal<Rc<HashMap<...>>>` to avoid cloning on every update. Memoize
downstream derivations.

### 4.8 Display `BackpressureNotice` to users

Defined at `crates/hypercolor-ui/src/ws.rs:232-240`, never shown. Add a
dismissible banner in `src/pages/dashboard.rs` plus a connection-status
badge showing reconnect attempt count.

### 4.9 Add ARIA labels to SVG charts

`crates/hypercolor-ui/src/components/perf_charts.rs` — add `role="img"`
and `aria-label` to each chart container.

### 4.10 TUI view decomposition

| File | LOC | Split |
|---|---|---|
| `src/app.rs` | 1086 | Extract canvas protocol management, action dispatch, initialization |
| `src/views/effect_browser.rs` | 1518 | Extract control panel, list filtering, search |
| `src/views/dashboard.rs` | 767 | Extract device table, panel renderers |

**Exit criteria:** SDK math/GLSL/palettes deduplicated. SilkCircuit
tokens live in Tailwind. No UI or TUI file >800 LOC. Accessibility
labels in place.

---

## Phase 5 — Core Engine Hardening

Runs parallel with Phases 1–2.

### 5.1 Servo worker circuit breaker

`crates/hypercolor-core/src/effect/servo_renderer.rs:53,406` globally
poisons the shared worker on any failure. Add:

```rust
struct ServoCircuitBreaker {
    failures: AtomicU32,
    next_retry: Mutex<Option<Instant>>,
    state: AtomicU8,  // Closed, Open, HalfOpen
}
```

Thresholds: 3 consecutive failures → open, 30s cooldown → half-open, 1
success → closed.

### 5.2 Extract `ServoWorkerClient` state machine

Split `servo_renderer.rs` (2397 LOC) into:

```
crates/hypercolor-core/src/effect/servo/
  mod.rs           — re-exports
  renderer.rs      — EffectRenderer impl
  worker.rs        — thread spawn/teardown
  worker_client.rs — Idle→Loading→Running→Stopping state machine
  circuit_breaker.rs
  delegate.rs      — (already exists)
```

### 5.3 Decompose `BackendManager` — **COORDINATE with Phase 28**

`device/manager.rs` (2222 LOC) splits into:

```
crates/hypercolor-core/src/device/
  manager.rs          — registration, staging
  queue_dispatcher.rs — output queue dispatch
  frame_pipeline.rs   — brightness compensation, normalization
  routing.rs          — device routing, remapping (owns cached routing
                         plans from fa34dd7)
  lifecycle_bridge.rs — state machine action execution
```

**This overlaps directly with the "render pipeline modernization" task
(`28-render-pipeline-modernization-plan.md`).** Do not dispatch a
refactor agent for this in parallel — merge the work into the active
task instead. The cached backend routing plans from `fa34dd7` live
naturally in `routing.rs`; the frame pipeline work belongs in
`frame_pipeline.rs`.

### 5.4 Split `effect/builtin/mod.rs` (1768 LOC)

One file per builtin effect.

### 5.5 Audit audio analyzer hot-path Mutex

`crates/hypercolor-core/src/input/audio/mod.rs` (1362 LOC) uses
`.lock()` in the update path. Measure contention under tight render
loops. If real, replace with `ArcSwap` for analysis results
(read-mostly) or split mutable state into a smaller inner Mutex.

### 5.6 Spatial sampler split

`crates/hypercolor-core/src/spatial/sampler.rs` (845 LOC) — extract LUT
generation from sampling dispatch:

```
spatial/
  sampler.rs  — dispatch by sampling strategy
  lut.rs      — LUT construction and caching
  resample.rs — pixel resampling primitives
```

### 5.7 Document Mutex lock ordering workspace-wide

Create `docs/design/32-lock-ordering.md`. List every Mutex/RwLock in
`AppState` and core subsystems, define acquisition order, flag any code
that violates it.

**Exit criteria:** Servo worker recovers from failures automatically.
`servo_renderer.rs` and `device/manager.rs` both <800 LOC. Audio hot
path has no lock contention under load. Lock ordering doc committed.

---

## Phase 6 — Test & Compliance

Ongoing, backfills across all other phases.

- **6.1** `hypercolor-desktop` baseline tests (CLAUDE.md compliance)
- **6.2** WebSocket protocol state machine tests (property-based)
- **6.3** Core contention tests (`tokio::test` for concurrent frame writes)
- **6.4** Network driver error-path tests (see §3.7)
- **6.5** HAL display encoder tests (see §2.9)
- **6.6** Split oversized test files:
  - `crates/hypercolor-daemon/tests/api_tests.rs` (4687 LOC)
  - `crates/hypercolor-daemon/tests/render_thread_tests.rs` (2744 LOC)
  - `crates/hypercolor-core/tests/backend_manager_tests.rs` (2676 LOC)
  - `crates/hypercolor-core/tests/pipeline_integration_tests.rs` (1556 LOC)
  - `crates/hypercolor-core/tests/wled_tests.rs` (1538 LOC)

---

## Wave Map (Agent Dispatch Plan)

### Wave 1 — Phase 0 urgent fixes · 3 agents parallel

| Agent | Scope | Files |
|---|---|---|
| A1.1 | Security | `core/.../hue/streaming.rs`, `daemon/.../ws.rs` |
| A1.2 | TUI regressions | `tui/src/app.rs`, `views/dashboard.rs`, `views/effect_browser.rs` |
| A1.3 | Config + track doc | `core/src/config/paths.rs`, `docs/design/30-sparkleflinger-implementation.md` |

### Wave 2 — Structural decomposition · 5 agents parallel

| Agent | Scope |
|---|---|
| A2.1 | `daemon/api/ws.rs` → submodule split (§1.1) |
| A2.2 | HAL RwLock purge + Corsair/ASUS correctness (§2.1–2.3) |
| A2.3 | `lianli/protocol.rs` ENE/TL split (§2.4) |
| A2.4 | `razer/devices.rs` split by device family (§2.5) |
| A2.5 | `servo_renderer.rs` split + circuit breaker (§5.1–5.2) |

### Wave 3 — Remaining daemon + core splits · 4 agents parallel

| Agent | Scope |
|---|---|
| A3.1 | `daemon/api/devices.rs` submodule split (§1.2) |
| A3.2 | `core/effect/builtin/mod.rs` per-effect split (§5.4) |
| A3.3 | `daemon/mcp/tools.rs` per-cluster split (§1.4) |
| A3.4 | `daemon/startup.rs` split (§1.4) |

**Deferred:** A3.5 (BackendManager decomposition, §5.3) overlaps with
the active render pipeline modernization task and will be merged into
that work instead of dispatched as a standalone agent.

### Wave 4 — Network hardening + SDK/UI cleanup · 5 agents parallel

| Agent | Scope |
|---|---|
| A4.1 | `DriverDescriptor` versioning + validation + health trait (§3.1–3.4) |
| A4.2 | `wled/backend.rs` split (§3.6) |
| A4.3 | SDK math/GLSL/palettes consolidation (§4.1–4.3) |
| A4.4 | SilkCircuit Tailwind token wiring (§4.4) |
| A4.5 | UI component decomposition (§4.6–4.9) |

### Wave 5 — Tests + TUI split · 4 agents parallel

| Agent | Scope |
|---|---|
| A5.1 | `hypercolor-desktop` test baseline (§6.1) |
| A5.2 | WS protocol state machine tests (§6.2) |
| A5.3 | Core contention tests (§6.3) |
| A5.4 | TUI file splits (§4.10) |

---

## Dependency Graph

```
Phase 0 (Wave 1)
    │
    ▼
Phase 1 daemon (Wave 2 + 3) ─┐
    │                         │
Phase 2 HAL (Wave 2)          ├─► Phase 4 UI/SDK (Wave 4) ─► Phase 5 tests (Wave 5)
    │                         │
Phase 3 network (Wave 4)      │
    │                         │
Phase 5 core (Wave 2 + 3)  ───┘
  └─► coordinate with Phase 28 render pipeline modernization
```

Phases 1, 2, 3, and 5 parallelize freely if scopes stay non-overlapping.
Phase 4 needs Phase 1's WS envelope decision. Phase 6 weaves through.

---

## Coordination Notes

- **Phase 28 (render pipeline modernization)** is active. It owns
  `BackendManager` evolution including cached routing plans (`fa34dd7`),
  spectrum buffer reuse (`606dd92`), canvas pack-in-place (`100cee6`),
  and streaming spectrum binary encoding (`0cf483d`). Phase 5.3 here
  (BackendManager decomposition) will merge into Phase 28, not compete.
- **Synth Horizon effect redesign** is also active
  (`sdk/src/effects/synth-horizon/`). Phase 4.3 (palette centralization)
  should wait until Synth Horizon lands so the biggest palette consumer
  stabilizes first.
- **hypercolor-ui is excluded from the workspace.** Agents touching UI
  files must run `cd crates/hypercolor-ui && trunk build` or
  `cargo check` from within the crate, not from the workspace root.

## Risk & Rollback

- Every phase lands as small commits, never a mega-PR.
- Between waves, run `just verify` + `just deny`.
- Regression canaries: `backend_manager_tests`, `pipeline_integration_tests`,
  `core_backend_routing` bench (~0.89 µs cached baseline).
- For structural splits, preserve public APIs during the split commit,
  deprecate the old surface in a follow-up. Never both in one commit.
- Agents report back before committing large splits so the orchestrator
  can sanity-check scope creep.

---

## Success Metrics

When this plan is complete:

- Zero source file >1500 LOC across the workspace
- Zero `.expect()` on RwLock in HAL
- `AppState` exposes facades, not flat subsystems
- All 13 HAL drivers use `CommandBuffer`
- `DriverDescriptor` carries a `schema_version`
- SDK math/GLSL deduplicated across all effects
- SilkCircuit tokens wired into Tailwind
- `hypercolor-desktop` has tests
- WS command body reads capped, Hue DTLS safety documented
- Lock ordering documented in `docs/design/32-lock-ordering.md`
