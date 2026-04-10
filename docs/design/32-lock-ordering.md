# 32 · Lock Ordering

Defines every `Mutex` and `RwLock` in `AppState` and core subsystems, establishes
a canonical acquisition order to prevent deadlocks, and flags code that violates it.

## Lock Inventory

### AppState Locks (Daemon)

| # | Field | Type | Guards | File |
|---|-------|------|--------|------|
| 1 | `render_loop` | `tokio::RwLock` | Frame timing, FPS tier, start/stop | `api/mod.rs:109` |
| 2 | `scene_manager` | `tokio::RwLock` | Scene stack, transitions, render groups | `api/mod.rs:100` |
| 3 | `effect_engine` | `tokio::Mutex` | Active renderer, controls, scene generation | `api/mod.rs:97` |
| 4 | `effect_registry` | `tokio::RwLock` | Effect catalog, metadata, rescan | `api/mod.rs:92` |
| 5 | `input_manager` | `tokio::Mutex` | Audio/screen/interaction capture lifecycle | `api/mod.rs:133` |
| 6 | `spatial_engine` | `tokio::RwLock` | Active layout, zone positions, sampling | `api/mod.rs:112` |
| 7 | `backend_manager` | `tokio::Mutex` | Device backends, routing, frame writes | `api/mod.rs:115` |
| 8 | `lifecycle_manager` | `tokio::Mutex` | Device state machine, connect/disconnect | `api/mod.rs:124` |
| 9 | `performance` | `tokio::RwLock` | Rolling frame metrics snapshot | `api/mod.rs:121` |
| 10 | `profiles` | `tokio::RwLock` | Saved lighting profile store | `api/mod.rs:139` |
| 11 | `layouts` | `tokio::RwLock` | Persisted spatial layout store | `api/mod.rs:160` |
| 12 | `layout_auto_exclusions` | `tokio::RwLock` | Per-layout discovery exclusion sets | `api/mod.rs:166` |
| 13 | `logical_devices` | `tokio::RwLock` | Physical-to-logical device segments | `api/mod.rs:172` |
| 14 | `effect_layout_links` | `tokio::RwLock` | Effect-to-layout associations | `api/mod.rs:178` |
| 15 | `attachment_registry` | `tokio::RwLock` | Attachment templates (built-in + user) | `api/mod.rs:142` |
| 16 | `attachment_profiles` | `tokio::RwLock` | Per-device attachment profile store | `api/mod.rs:145` |
| 17 | `device_settings` | `tokio::RwLock` | Per-device user settings, global brightness | `api/mod.rs:148` |
| 18 | `playlist_runtime` | `tokio::Mutex` | Active playlist worker state | `api/mod.rs:196` |
| 19 | `reconnect_tasks` | `std::Mutex` | Active reconnect JoinHandle map | `api/mod.rs:127` |
| 20 | `scene_transactions` | `std::Mutex` (inner) | Frame-boundary layout/resize queue | `scene_transactions.rs:18` |
| 21 | `security_state.rate_limiter` | `tokio::Mutex` | API rate-limiting window | `api/security.rs:38` |

### Core Crate Internal Locks

| Field | Type | Guards | File |
|-------|------|--------|------|
| `DeviceRegistry::inner` | `tokio::RwLock` | Device map, fingerprint index | `device/registry.rs:49` |
| `CredentialStore::cache` | `tokio::RwLock` | Cached network credentials | `device/net/credentials.rs:66` |
| `BackendManager` per-backend | `tokio::Mutex` | Individual `dyn DeviceBackend` handle | `device/manager.rs:30` |
| `UsbBackend::last_async_error` | `std::Mutex` | Latest async write error string | `device/usb_backend.rs:76` |
| `UsbBackend::prism_s` | `tokio::RwLock` | PrismS device config cache | `device/usb_backend.rs:214` |
| `AudioCaptureManager::analyzer` | `std::Mutex` | Audio FFT/beat analyzer state | `input/audio/mod.rs:265` |
| `EvdevInputSource::shared` | `std::Mutex` | Keyboard/evdev latest snapshot | `input/evdev.rs:42` |
| `InteractionInputSource::shared` | `std::Mutex` | Mouse/interaction latest snapshot | `input/interaction/mod.rs:31` |
| `WaylandScreenCapture::latest_snapshot` | `std::Mutex` | Latest screen capture frame | `input/screen/wayland.rs:34` |
| `ServoDelegate::last_url` | `std::Mutex` | Servo navigation URL | `effect/servo/delegate.rs:37` |
| `ServoDelegate::console_messages` | `std::Mutex` | Servo console message ring | `effect/servo/delegate.rs:38` |
| `SERVO_WORKER` | `std::Mutex` (static) | Shared Servo worker thread lifecycle | `effect/servo/worker.rs:51` |
| `ServoWorkerClient::state` | `std::Mutex` | Client-side Servo render state | `effect/servo/worker_client.rs:109` |
| `CircuitBreaker::next_retry` | `std::Mutex` | Servo crash retry timestamp | `effect/servo/circuit_breaker.rs:60` |
| `DATA_DIR_OVERRIDE` | `std::RwLock` (static) | Test-only config path override | `config/paths.rs:12` |
| `CONFIG_DIR_OVERRIDE` | `std::RwLock` (static) | Test-only config path override | `config/paths.rs:13` |

### Daemon-Only Internal Locks

| Field | Type | Guards | File |
|-------|------|--------|------|
| `WS_CANVAS_BINARY_CACHE` | `std::Mutex` (sharded) | Per-shard binary canvas encode cache | `api/ws/cache.rs:44` |
| `WS_FRAME_PAYLOAD_CACHE` | `std::Mutex` (sharded) | Per-shard frame JSON payload cache | `api/ws/cache.rs:55` |
| `WS_SPECTRUM_PAYLOAD_CACHE` | `std::Mutex` (sharded) | Per-shard spectrum binary cache | `api/ws/cache.rs:66` |
| `WS_PREVIEW_SCALE_LUT_CACHE` | `std::Mutex` (static) | Preview brightness lookup table | `api/ws/cache.rs:76` |
| `WS_COMMAND_ROUTER_CACHE` | `std::Mutex` (static) | Cached WS command dispatch router | `api/ws/cache.rs:82` |
| `JsonLibraryStore::data` | `tokio::RwLock` | Persisted favorites/presets/playlists | `library.rs:196` |
| `IncrementalDiscoveryState` | `tokio::Mutex` | Discovery scan incremental merge state | `discovery/scan.rs:401` |

## Acquisition Order

All tokio async locks in AppState are assigned a level number. When acquiring
multiple locks, always acquire in ascending level order (lower number first).
Release order does not matter for async locks (Tokio cooperative scheduling
prevents the "hold-and-yield" deadlock pattern that affects OS mutexes), but
releasing eagerly is still preferred for throughput.

```
Level 1: render_loop
Level 2: scene_manager
Level 3: effect_engine
Level 4: effect_registry
Level 5: input_manager
Level 6: spatial_engine
Level 7: backend_manager
Level 8: lifecycle_manager
Level 9: performance
Level 10: profiles
Level 11: layouts
Level 12: layout_auto_exclusions
Level 13: logical_devices
Level 14: effect_layout_links
Level 15: attachment_registry
Level 16: attachment_profiles
Level 17: device_settings
Level 18: playlist_runtime
```

### Rules

1. **Never acquire level N while holding level N+k** (lower-numbered locks first).
2. **Drop before re-acquiring.** When code needs a lock it released earlier in the
   same function, that is fine as long as no higher-numbered lock is still held.
3. **Per-backend device locks** (inside `BackendManager`) are all at the same
   sub-level beneath level 7. Never hold two per-backend locks simultaneously.
4. **`std::Mutex` locks** (`reconnect_tasks`, `scene_transactions`, WS caches) must
   never be held across `.await` points. They are not assigned levels because they
   cannot participate in async deadlocks. Keep critical sections minimal.
5. **`DeviceRegistry::inner`** is self-contained. Its internal RwLock should not
   be held while acquiring any AppState-level lock, and no AppState lock should be
   held while calling registry methods that take the internal lock. This is
   naturally satisfied because the registry exposes an async API that borrows
   internally.
6. **`CredentialStore::cache`** is leaf-only. Acquire it without holding other locks.

### Render Thread Lock Sequence

The render pipeline is the highest-frequency multi-lock consumer. Within a single
frame, `execute_frame` acquires locks in this order:

```
render_loop.write()        [L1]  — tick gate
  (dropped)
scene_manager.read()       [L2]  — transition tick / snapshot
  (dropped)
effect_engine.lock()       [L3]  — canvas resize (if pending transaction)
  (dropped)
effect_engine.lock()       [L3]  — scene snapshot demand query
  (dropped)
effect_registry.read()     [L4]  — render group demand query (alt path)
  (dropped)
input_manager.lock()       [L5]  — reconcile audio/screen capture
  (dropped)
input_manager.lock()       [L5]  — sample_inputs
  (dropped)
effect_engine.lock()       [L3]  — render effect into canvas (via render_effect_into)
  (dropped)
effect_registry.read()     [L4]  — render group scene (alt path, inside compose)
  (dropped)
backend_manager.lock()     [L7]  — write_frame
  (dropped)
performance.write()        [L9]  — record frame metrics
  (dropped)
render_loop.write()        [L1]  — frame_complete + FPS tier adjustment
  (dropped)
```

Each lock is acquired and released in isolation. No two AppState locks are held
simultaneously within the render thread. This is the ideal pattern.

### API Handler Multi-Lock Sequences

Key handlers that acquire more than one AppState lock:

**`apply_effect`** (`effects.rs`):
`effect_registry.read [L4]` -> drop -> `effect_engine.lock [L3]` -> drop ->
`effect_layout_links.read [L14]` -> drop -> `layouts.read [L11]` -> drop ->
`spatial_engine.write [L6]` -> drop -> `effect_engine.lock [L3]` -> drop ->
`spatial_engine.read [L6]` -> drop.
All sequential, no nesting. Correct.

**`apply_profile_snapshot`** (`profiles.rs`):
`layouts.read [L11]` -> drop -> `effect_registry.read [L4]` -> drop ->
`effect_engine.lock [L3]` -> drop -> `spatial_engine.write [L6]` -> drop ->
`device_settings.write [L17]` -> drop.
All sequential, no nesting. Correct.

**`snapshot_profile`** (`profiles.rs`):
`spatial_engine.read [L6]` -> drop -> `effect_engine.lock [L3]` -> drop.
Sequential, no nesting. Correct.

**`persist_runtime_session`** (`api/mod.rs`):
`effect_engine.lock [L3]` -> drop -> `spatial_engine.read [L6]` -> drop.
Sequential, no nesting. Correct.

**`update_layout`** (`layouts.rs`):
`layouts.write [L11]` -> **`spatial_engine.read [L6]` while L11 held** -> drop both.
**VIOLATION**: L11 > L6. See Known Violations below.

**`delete_layout`** (`layouts.rs`):
`spatial_engine.read [L6]` -> drop -> `layouts.write [L11]` -> drop ->
`layout_auto_exclusions.write [L12]` -> drop. Correct.

**`list_layouts`** (`layouts.rs`):
`spatial_engine.read [L6]` -> drop -> `layouts.read [L11]` -> drop. Correct.

**`sync_active_layout_for_renderable_devices`** (`discovery/auto_layout.rs`):
`spatial_engine.read [L6]` -> drop -> `layout_auto_exclusions.read [L12]` -> drop ->
`backend_manager.lock [L7]` -> drop -> `logical_devices.read [L13]` -> drop ->
`lifecycle_manager.lock [L8]` -> drop -> `spatial_engine.write [L6]` -> drop ->
`layouts.write [L11]` -> drop. All sequential. Correct.

**`display_targets`** (`display_output/mod.rs`):
`spatial_engine.read [L6]` -> drop -> `logical_devices.read [L13]` -> drop.
Reads are independent. Correct.

**`reconcile_display_workers`** (`display_output/mod.rs`):
`backend_manager.lock [L7]` -> drop. Single lock. Correct.

**`execute_lifecycle_actions`** (`discovery/lifecycle.rs`):
Acquires `lifecycle_manager.lock [L8]` and `backend_manager.lock [L7]` in various
branches, but always sequentially (never nested). Correct.

## Known Violations

### V1: `update_layout` nests `spatial_engine.read` inside `layouts.write`

**File:** `crates/hypercolor-daemon/src/api/layouts.rs:247-249`

```rust
// layouts.write() is held here (acquired at line 200)
let active_layout_id = {
    let spatial = state.spatial_engine.read().await;  // L6 under L11
    spatial.layout().id.clone()
};
```

**Risk:** Low-to-moderate. The render thread never holds `spatial_engine` while
waiting on `layouts`, so this cannot currently deadlock. However, it violates the
ordering invariant and could become a deadlock if future code acquires `layouts`
while holding `spatial_engine`. The `delete_layout` and `list_layouts` handlers
demonstrate the correct pattern: read `spatial_engine` first, then acquire `layouts`.

**Fix:** Move the `spatial_engine.read()` call above the `layouts.write()` acquisition,
matching the pattern used by `delete_layout`:

```rust
let active_layout_id = {
    let spatial = state.spatial_engine.read().await;
    spatial.layout().id.clone()
};
let mut layouts = state.layouts.write().await;
// ... mutate layout ...
```

### V2: `update_layout` nested read is a latent hazard only

No current code path acquires `layouts` while holding `spatial_engine`, so V1
cannot deadlock today. The render thread uses `scene_transactions` (a std::Mutex
queue) to communicate layout changes rather than holding `spatial_engine` and
reaching for `layouts`, which is the key architectural decision that keeps this safe.
The violation is still worth fixing to maintain the invariant.

## Design Notes

**Why `effect_engine` is Mutex, not RwLock:** `dyn EffectRenderer` is `Send` but
not `Sync` (Servo's renderer is single-threaded). This forces `Mutex` even though
most API reads could share.

**Why the render thread is safe:** Every lock acquisition in the frame pipeline is
a short-lived scope guard that is dropped before the next lock is acquired. The
`SceneTransactionQueue` (`std::Mutex<VecDeque>`) bridges the API/render boundary
without requiring both sides to hold the same tokio locks.

**Why `DeviceRegistry` is not in the ordering:** It wraps its own internal
`RwLock` and exposes an async API. No external code touches the inner lock directly,
and registry methods do not reach back into AppState locks, so it cannot participate
in a deadlock cycle with the AppState locks.

**`std::Mutex` discipline:** All `std::Mutex` instances in the codebase
(`reconnect_tasks`, `scene_transactions`, WS caches, audio analyzer, etc.) are
held for microsecond-scale critical sections and never across `.await` points. They
are not assigned ordering levels because blocking mutexes cannot interleave with
tokio's cooperative scheduler in a way that creates async deadlocks. The invariant
to maintain is: never `.await` while holding a `std::Mutex` guard.
