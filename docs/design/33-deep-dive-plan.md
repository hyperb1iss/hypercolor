# 33 · Deep Dive Plan — Post-Refactor Improvements

**Status:** Ready · **Updated:** 2026-04-10
**Prerequisite:** `31-refactor-plan.md` Waves 1–7 complete

This plan targets the seven highest-payoff improvements identified during
the 2026-04-10 codebase review and subsequent refactor session. Unlike the
structural refactoring (which was mechanical move-only), these changes
modify runtime behavior and require benchmarks, verification, and careful
rollout.

---

## Benchmark Strategy

Every item below that touches a hot path must include before/after
measurements. Use the existing `core_backend_routing` bench as a template.

**Harness locations:**
- `crates/hypercolor-core/benches/` — render pipeline, spatial, audio
- `crates/hypercolor-daemon/benches/` — API handler throughput, WS relay
- Criterion for microbenchmarks, `tokio::time::Instant` spans for integration

**Wave exit criteria:** No commit merges if it introduces >5% regression on
any existing benchmark. New benchmarks establish their own baseline on the
first commit, and subsequent commits in the same wave must stay within 5%
of that baseline.

**Baseline capture:** Before starting each wave, run:
```bash
cargo bench --workspace -- --save-baseline pre-wave-N
```
After the wave completes:
```bash
cargo bench --workspace -- --save-baseline post-wave-N
cargo bench --workspace -- --baseline pre-wave-N
```
The comparison report is the wave's exit gate.

---

## Item 1 — AppState Facade Decomposition

**Why:** AppState has 31 flat fields. Every handler reaches into raw
subsystems (`state.backend_manager.lock()`, `state.lifecycle_manager...`).
This couples handlers to lock ordering, makes testing painful, and turns
every new feature into a "which five mutexes do I need?" puzzle.

**Target state:** AppState exposes four domain facades that encapsulate
lock acquisition and enforce the ordering from `32-lock-ordering.md`:

```rust
pub struct AppState {
    pub device: DeviceFacade,     // backend_manager, lifecycle, reconnect
    pub effects: EffectFacade,    // engine, registry, playlist runtime
    pub spatial: SpatialFacade,   // layouts, links, spatial engine
    pub library: LibraryFacade,   // favorites, presets, profiles
    pub bus: HypercolorBus,
    pub config: Arc<HypercolorConfig>,
}
```

### Implementation Steps

1. **Inventory every handler's state access patterns.** Grep
   `state.backend_manager`, `state.effect_engine`, etc. across all
   `api/` handlers. Build a table of which fields each handler touches.

2. **Define facade traits.** Each facade exposes high-level operations
   (e.g., `device.attach(id, layout)`) instead of raw lock access. The
   facade internally acquires locks in the documented order.

3. **Implement DeviceFacade first** (highest churn — 14 device handlers
   + discovery + lifecycle). Move all `backend_manager` / `lifecycle_manager`
   / `reconnect_tasks` access behind `DeviceFacade` methods.

4. **Migrate handlers one file at a time.** Each migration is a
   self-contained commit: `api/devices/mod.rs` → `state.device.X()`.

5. **Repeat for EffectFacade, SpatialFacade, LibraryFacade.**

6. **Delete the raw fields from AppState.** Once all handlers use facades,
   remove direct access. Compile errors guide any stragglers.

### Benchmark

Add `benches/api_handler_bench.rs`:
- Measure `list_devices` handler throughput (req/s) before and after
- Measure `apply_effect` handler latency (p50/p99) before and after
- Facade indirection should be zero-cost (inlined method calls), but
  verify there's no lock-ordering overhead from the facade acquiring
  locks it doesn't need

### Risk

- **High churn.** Every handler file gets touched.
- **Perf agent conflict.** The parallel perf agent works in daemon handlers.
  Coordinate timing — either pause perf work or run this in a worktree.
- **Test breakage.** Many tests construct AppState directly. Need to update
  test fixtures to build facades.

### Exit Criteria

- Zero raw `state.backend_manager` / `state.effect_engine` access outside
  facades
- Lock ordering from `32-lock-ordering.md` enforced by facade method
  signatures (a facade never exposes a raw lock guard)
- No handler throughput regression >5%

---

## Item 2 — WS Adaptive Backpressure

**Why:** When the daemon's WS relay channels fill (bounded at 64), frames
are silently dropped. Slow clients lose state awareness with no feedback.
The `BackpressureNotice` type already exists in the UI but is never sent
or displayed.

**Target state:** The daemon detects slow clients, downsamples their frame
rate, and notifies them. The UI renders a dismissible warning banner when
backpressure is active.

### Implementation Steps

1. **Add per-client throughput tracking** in `ws/relays.rs`. Track frames
   offered vs frames delivered per subscription. When delivery rate drops
   below 80% of offer rate for 2 consecutive seconds, mark the client as
   "slow".

2. **Implement adaptive downsampling** in `ws/relays.rs`. For slow clients:
   - Skip every other frame relay (halve effective FPS)
   - If still slow after 5s, skip 3 of 4 (quarter FPS)
   - If recovered for 10s, restore full rate

3. **Send BackpressureNotice** via the existing events channel when
   downsampling activates or deactivates. Message format:
   ```json
   {
     "type": "backpressure",
     "active": true,
     "effective_fps": 15,
     "reason": "client_slow"
   }
   ```

4. **Wire the UI banner.** `BackpressureNotice` is defined at
   `ui/src/ws.rs:232-240` but never displayed. Add a dismissible
   notification component in the dashboard that renders when
   `backpressure_notice.get().is_some()`.

5. **Add connection status badge.** Show reconnect attempt count in the
   header bar alongside the backpressure indicator.

### Benchmark

Add `benches/ws_relay_bench.rs`:
- Measure relay throughput (frames/sec to N clients) with and without
  slow-client simulation
- Measure overhead of throughput tracking (should be <1µs per frame)
- Verify fast clients see zero degradation when a slow client is present

### Risk

- **Subtle timing bugs.** Rate detection windows need hysteresis to avoid
  oscillating between normal and slow states.
- **Test coverage.** Property-based tests for rate transitions (normal →
  slow → very slow → recovery) using simulated time.

### Exit Criteria

- Slow clients receive downsampled frames instead of silent drops
- Fast clients see zero throughput impact from slow peers
- UI displays backpressure banner when active
- Relay benchmark shows <1µs overhead per frame for tracking

---

## Item 3 — Audio Analyzer Hot-Path Mutex Audit

**Why:** `core/input/audio/mod.rs` (1362 LOC) uses `std::sync::Mutex` in
the `InputSource::sample()` path, which runs on the render thread at up to
60 FPS. If the audio analysis thread holds the lock during a heavy FFT
window, the render frame stalls.

**Target state:** The render thread reads audio state lock-free via
`ArcSwap` or a double-buffer pattern. The audio analysis thread writes
without blocking the reader.

### Implementation Steps

1. **Measure current contention.** Add `tracing::instrument` spans around
   the lock acquisition in `sample()`. Run the daemon under load with audio
   active. Measure p99 lock-wait time.

2. **If contention is measurable (>100µs p99):**
   - Replace the `Mutex<AudioState>` with `arc_swap::ArcSwap<AudioState>`
   - Analysis thread: `store(Arc::new(new_state))`
   - Render thread: `load()` (returns an `Arc` snapshot, no blocking)
   - Requires `AudioState: Clone` (it should already be — it's analysis
     results, not raw buffers)

3. **If contention is NOT measurable (<100µs p99):**
   - Document the finding in `32-lock-ordering.md`
   - Add a benchmark that detects regression if contention grows
   - Skip the ArcSwap migration

4. **If ArcSwap is used:** Verify the analysis thread's write frequency
   (~43 Hz for 1024-sample windows at 44.1 kHz) doesn't cause excessive
   Arc allocations. Consider a fixed double-buffer if allocation pressure
   is visible.

### Benchmark

Add `benches/audio_input_bench.rs`:
- Measure `InputManager::sample_all()` latency with and without audio
  source active
- Measure under contention: spawn a thread that holds the audio lock for
  varying durations (10µs, 100µs, 1ms) and measure render-thread stall
- Before/after ArcSwap migration if it lands

### Risk

- **ArcSwap adds a dependency.** Check if it's already in the dep tree
  (likely yes via tokio or tracing). If not, evaluate `std::sync::atomic`
  double-buffer as a zero-dep alternative.
- **Stale reads.** ArcSwap gives eventual consistency — the render thread
  may see audio data that's one frame old. This is acceptable for audio
  reactivity (human perception threshold is ~20ms; one frame at 60 FPS
  is ~16ms).

### Exit Criteria

- Documented contention measurement (before)
- If migrated: lock-free read path confirmed via benchmark
- p99 `sample_all()` latency under 50µs with audio active
- No allocation regression from ArcSwap under sustained load

---

## Item 4 — Fix Lock Ordering Violation

**Why:** `32-lock-ordering.md` flagged one violation: `update_layout` in
`layouts.rs:247` nests `spatial_engine.read()` (level 6) inside a held
`layouts.write()` (level 11). This inverts the documented ordering. Not
currently exploitable but will deadlock if any code path ever acquires
`layouts` while holding `spatial_engine`.

**Target state:** `update_layout` acquires locks in the correct order:
spatial_engine first, then layouts.

### Implementation Steps

1. **Read the current `update_layout` function** in
   `crates/hypercolor-daemon/src/api/` (likely in `layouts.rs` or wherever
   layout CRUD lives).

2. **Identify what `spatial_engine.read()` is used for inside the
   `layouts.write()` scope.** Likely reading current zone positions or
   topology to validate the layout update.

3. **Reorder:** Read from spatial_engine BEFORE acquiring the layouts write
   lock. Store the result in a local variable, then acquire layouts.write()
   and use the pre-read data.

4. **Verify the pattern matches `delete_layout` and `list_layouts`** which
   the lock ordering doc says already follow the correct order.

### Benchmark

None needed — this is a correctness fix, not a performance change. The
reorder might even be slightly faster (shorter critical section on the
layouts write lock).

### Risk

- **TOCTOU.** If spatial state changes between the read and the write, the
  layout update uses stale spatial data. This is acceptable because layout
  updates are user-initiated (seconds between actions) and spatial state
  changes are device-driven (much slower).
- **Minimal.** The fix is literally reordering two lines.

### Exit Criteria

- `update_layout` acquires spatial_engine.read() before layouts.write()
- `32-lock-ordering.md` updated to remove the violation entry
- `cargo test` passes (specifically layout-related tests)

---

## Item 5 — Network Driver Retry with Backoff

**Why:** Hue pairing calls `pair_with_status()` once with no retry. If
the network hiccups during the 30-second button-press window, pairing
fails silently. WLED discovery hammers unreachable IPs indefinitely with
no backoff, wasting bandwidth and CPU.

**Target state:** All network operations that can transiently fail use
exponential backoff with jitter. Unreachable devices are marked stale
after N failures and probed less frequently.

### Implementation Steps

1. **Add a shared retry helper** in `crates/hypercolor-driver-api/src/`:
   ```rust
   pub async fn retry_with_backoff<F, Fut, T, E>(
       f: F,
       max_attempts: u32,
       base_delay: Duration,
       max_delay: Duration,
   ) -> Result<T, E>
   where
       F: Fn() -> Fut,
       Fut: Future<Output = Result<T, E>>,
   ```
   Use jittered exponential backoff: `min(base * 2^attempt + jitter, max)`.

2. **Wrap Hue pairing** in `crates/hypercolor-driver-hue/src/lib.rs`:
   - `pair_with_status()` → 3 attempts, 1s base, 10s max
   - Log each retry with `tracing::info!`
   - If all attempts fail, return the last error (not a generic "failed")

3. **Add staleness tracking to WLED discovery** in
   `crates/hypercolor-core/src/device/wled/`:
   - Track consecutive failures per IP
   - After 3 failures: probe every 30s instead of every scan
   - After 10 failures: probe every 5 minutes
   - On success: reset counter immediately

4. **Wrap Nanoleaf pairing** similarly to Hue (if applicable — check
   whether Nanoleaf has a similar one-shot pairing call).

### Benchmark

Not a throughput concern — this is robustness. Instead, add integration
tests:
- `tests/hue_retry_tests.rs`: mock server that fails N times then
  succeeds. Verify pairing eventually succeeds.
- `tests/wled_staleness_tests.rs`: mock unreachable IP. Verify probe
  frequency decreases. Verify recovery on success.

### Risk

- **Retry storms.** If 50 WLED devices are unreachable simultaneously,
  even with backoff, the discovery thread could be busy for minutes.
  Cap total concurrent probes per scan cycle.
- **Hue button-press window.** The 30-second window is wall-clock from
  the bridge's perspective. Retries consume some of that window. Ensure
  total retry budget (3 × ~2s = 6s) fits within the window.

### Exit Criteria

- Hue pairing survives 1-2 transient network failures
- WLED discovery backs off for unreachable devices (probe frequency drops)
- No retry storms under mass-unreachable conditions
- Integration tests for retry and staleness behavior

---

## Item 6 — USB Fingerprint Stability

**Why:** `34-device-fingerprints.md` identified that USB devices without
serial numbers fall back to topology path (`bus-port.port.port`), which
breaks when the user plugs the device into a different USB port. This
causes phantom device duplication — the "same" keyboard appears as two
devices with different DeviceIds.

**Target state:** Devices without serial numbers use a composite hash
of stable USB descriptors as a fallback fingerprint, resilient to port
changes.

### Implementation Steps

1. **Read the current fingerprint logic** in `crates/hypercolor-hal/` —
   likely in the USB transport or scanner module. Find where
   `DeviceFingerprint` is constructed for USB/HID devices.

2. **Identify available stable descriptors** from the USB device:
   - `idVendor` (VID) — always present
   - `idProduct` (PID) — always present
   - `iManufacturer` string descriptor — usually present
   - `iProduct` string descriptor — usually present
   - `bcdDevice` (device version) — always present
   - `iSerialNumber` — the one that's sometimes missing

3. **Implement composite hash fallback:**
   ```rust
   fn usb_fallback_fingerprint(vid: u16, pid: u16, manufacturer: &str, product: &str, bcd: u16) -> DeviceFingerprint {
       let mut hasher = DefaultHasher::new();
       vid.hash(&mut hasher);
       pid.hash(&mut hasher);
       manufacturer.hash(&mut hasher);
       product.hash(&mut hasher);
       bcd.hash(&mut hasher);
       DeviceFingerprint::from_hash("usb", hasher.finish())
   }
   ```

4. **Use the composite hash when serial number is absent.** Precedence:
   - Serial number present → use serial (current behavior, strongest)
   - Serial absent, descriptors present → use composite hash (new)
   - Descriptors absent → fall back to topology path (last resort)

5. **Handle the migration edge case.** Existing devices with topology-path
   fingerprints will get new composite-hash fingerprints on next discovery.
   Add a migration helper that checks if a topology-fingerprinted device
   matches a composite-hash device (same VID:PID + port) and merges them.

### Benchmark

None needed — fingerprint computation is once per device discovery, not
a hot path.

### Risk

- **Identical devices.** Two identical keyboards (same VID:PID:manufacturer:
  product:bcd, no serial) will get the SAME composite hash. This is
  correct — they're indistinguishable to the system. Document this as
  expected behavior. Users who need to distinguish identical devices must
  use ones with unique serial numbers.
- **Migration data loss.** Merging topology-fingerprinted devices into
  composite-hash devices could lose per-device settings if the merge logic
  is wrong. Keep the old fingerprint as an alias for one discovery cycle
  before retiring it.

### Exit Criteria

- Devices without serial numbers survive USB port changes
- Fingerprint spec (`34-device-fingerprints.md`) updated with new fallback
  tier
- No phantom device duplication in testing
- Migration from topology to composite hash is graceful

---

## Item 7 — Fix Derivable Default Impls

**Why:** Three `impl Default` blocks in `hypercolor-types` manually
implement what `#[derive(Default)]` would generate. Every agent in the
refactor session flagged this clippy lint. It's noise that makes real
issues harder to spot.

**Target state:** Replace manual Default impls with `#[derive(Default)]`
where the implementations are truly equivalent.

### Implementation Steps

1. **Read the three flagged locations:**
   - `crates/hypercolor-types/src/canvas.rs`
   - `crates/hypercolor-types/src/scene.rs`
   - (check for a third — agents mentioned 2-3 locations)

2. **For each, verify the manual impl matches derive semantics.** Compare
   field-by-field: does the manual impl set any field to a non-Default
   value? If yes, that impl is intentional and should keep `#[allow]`.
   If no, replace with `#[derive(Default)]`.

3. **Run `cargo clippy -p hypercolor-types -- -D warnings`** to confirm
   the lints are resolved.

### Benchmark

None needed.

### Risk

- **Behavioral change if manual impl differs from derive.** Always verify
  field-by-field before replacing. The common gotcha is a field like
  `enabled: bool` where the manual impl sets `true` but derive would give
  `false`.

### Exit Criteria

- Zero `derivable_impls` clippy warnings in `hypercolor-types`
- `cargo clippy --workspace -- -D warnings` passes for the types crate

---

## Execution Waves

### Wave A — Quick Wins (parallelize all three)

| Agent | Item | Scope | Risk |
|-------|------|-------|------|
| A.1 | Item 4: Lock ordering fix | 1 file, ~5 lines | Minimal |
| A.2 | Item 7: Derivable impls | 2-3 files, ~20 lines | Minimal |
| A.3 | Item 3: Audio mutex audit (measurement only) | Read + instrument, no code change | Zero |

**Exit gate:** A.1 and A.2 commit. A.3 reports contention measurements.

### Wave B — Behavioral Changes (2 parallel agents)

| Agent | Item | Scope | Risk |
|-------|------|-------|------|
| B.1 | Item 3: ArcSwap migration (if A.3 found contention) | core/input/audio/ | Medium |
| B.2 | Item 5: Network retry + backoff | driver-api, hue, wled | Medium |

**Exit gate:** Benchmarks show no regression. Retry integration tests
pass. Audio `sample_all()` p99 under 50µs.

### Wave C — Architectural (sequential, high-touch)

| Agent | Item | Scope | Risk |
|-------|------|-------|------|
| C.1 | Item 1: AppState facade — DeviceFacade | daemon/api/devices/, discovery/ | High |
| C.2 | Item 1: AppState facade — EffectFacade | daemon/api/effects/, ws/ | High |
| C.3 | Item 1: AppState facade — SpatialFacade + LibraryFacade | daemon/api/layouts/, library/ | Medium |

**Sequence:** C.1 first (most handlers). C.2 and C.3 can parallelize after
C.1 proves the pattern. Each agent runs benchmarks before committing.

**Exit gate:** Zero raw subsystem access outside facades. Handler
throughput within 5% of baseline.

### Wave D — UX Polish (parallelize)

| Agent | Item | Scope | Risk |
|-------|------|-------|------|
| D.1 | Item 2: WS adaptive backpressure | daemon/api/ws/relays, ws/cache | Medium |
| D.2 | Item 2: UI backpressure banner | hypercolor-ui/src/pages/dashboard/ | Low |
| D.3 | Item 6: USB fingerprint stability | hypercolor-hal/ USB transport | Medium |

**Exit gate:** Slow clients get downsampled frames. UI shows banner.
USB devices survive port changes.

---

## Coordination with Active Work

- **Render pipeline modernization** (`28-render-pipeline-modernization-plan.md`)
  owns `BackendManager` decomposition. Items 1 (AppState facade) and 3
  (audio mutex) must not conflict — Wave C should wait until the render
  pipeline work stabilizes.
- **Parallel perf agent** is actively editing `core/render_thread/`,
  `core/device/`, and `core/effect/`. Waves A and B avoid those paths.
  Wave C (AppState facade) will touch handler files the perf agent may
  also edit — coordinate timing.
- **Synth Horizon redesign** blocks SDK palette centralization. Not in
  this plan; revisit after Synth Horizon lands.

---

## Success Metrics

When this plan is complete:

- AppState exposes facades, not flat subsystems (31 fields → 6)
- Lock ordering from `32-lock-ordering.md` enforced by facade signatures
- Zero known lock-ordering violations
- Audio `sample_all()` p99 < 50µs under contention
- Slow WS clients receive adaptive downsampled frames
- Network pairing survives transient failures
- USB devices survive port changes without duplication
- Zero `derivable_impls` clippy warnings in the workspace
- Every behavioral change has a before/after benchmark
