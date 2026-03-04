# Hypercolor Deep Backend Audit

**Date:** 2026-03-04
**Scope:** All backend crates (~66K lines of Rust across 7 crates)
**Method:** 10-agent parallel swarm — each agent specialized on a domain
**Agents:** types, HAL, core/device, core/effect, core/input, core/engine+bus+spatial+scene, daemon/API, daemon/MCP+infra, CLI, cross-crate architecture

---

## By the Numbers

| Metric | Count |
|--------|-------|
| Files reviewed | 140+ source + test files |
| Agents deployed | 10 (parallel) |
| Critical findings | 6 |
| High findings | 14 |
| Medium findings | 42 |
| Low findings | 53 |
| Positive findings | 13 (things done well) |

---

## Critical Findings (Fix Before Any Deployment)

### C1. CORS allows any origin — any website can control your LEDs

**File:** `daemon/api/mod.rs:391-394`

```rust
CorsLayer::new()
    .allow_origin(Any)
    .allow_methods(Any)
    .allow_headers(Any),
```

Any website loaded in a user's browser can make full API calls to the Hypercolor daemon. If the daemon is running on localhost, an attacker's page could enumerate devices, apply effects, modify configuration, and trigger discovery scans.

**Fix:** Restrict `allow_origin` to the daemon's own host (the served UI origin) or a configurable allowlist.

---

### C2. WebSocket commands bypass authentication

**File:** `daemon/api/ws.rs:1072`

When a client sends a `Command` message over WebSocket, `dispatch_command` builds a fresh internal request and routes it through a new router instance. The internal request has no `Authorization` header, no `ConnectInfo`, and no `X-Forwarded-For`. WS commands only work when security is disabled.

**Fix:** Propagate the authenticated identity from the WS handshake into internal requests, or use a separate internal router with authorization applied before dispatch.

---

### C3. Unbounded JSON queue per WS client → OOM

**File:** `daemon/api/ws.rs:568`

```rust
let (json_tx, mut json_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
```

The binary channel is bounded (`WS_BUFFER_SIZE = 64`), but the JSON channel is unbounded. A slow or malicious client causes memory exhaustion.

**Fix:** Replace with a bounded channel (256-512 messages). Drop messages when full, similar to binary backpressure handling.

---

### C4. Device registry bypasses state machine

**File:** `core/device/registry.rs:206-230`

`update_user_settings()` directly mutates `DeviceState`, bypassing `DeviceStateMachine` entirely. Creates split-brain state: the registry says "Known" but the lifecycle manager still holds a state machine in "Disabled" state.

```rust
// registry.rs:219-227 -- direct state mutation, no lifecycle coordination
if let Some(enabled) = enabled {
    if enabled {
        if entry.state == DeviceState::Disabled {
            entry.state = DeviceState::Known;
        }
    } else {
        entry.state = DeviceState::Disabled;
    }
}
```

**Fix:** Remove direct state mutation. Route all state changes through the lifecycle manager's `on_user_disable`/`on_user_enable` methods.

---

### C5. OpenRGB `write_bstring` silently truncates on overflow

**File:** `core/device/openrgb/proto.rs:370-377`

```rust
fn write_bstring(buf: &mut Vec<u8>, s: &str) {
    let length = (s.len() + 1) as u16;  // wraps on strings >65534 bytes
    buf.extend_from_slice(&length.to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
    buf.push(0x00);
}
```

The cast silently truncates, causing header/payload mismatch on the wire.

**Fix:** `u16::try_from(s.len() + 1).expect("bstring exceeds u16 length limit")`

---

### C6. Canvas overflow on extreme dimensions

**File:** `types/canvas.rs:379-390`

```rust
let len = width as usize * height as usize * BYTES_PER_PIXEL;
```

`width * height * 4` can overflow `usize` with malicious inputs. No maximum dimension guard exists. The daemon API also lacks canvas dimension bounds (`layouts.rs:325-330` only checks for zero).

**Fix:** Add `const MAX_CANVAS_PIXELS: u32 = 4_194_304;` guard and validate dimensions before allocation. Add upper bounds in the layouts API handler.

---

## High Findings

### H1. HAL transport `send_receive` lock gap

**File:** `hal/transport/control.rs:72-115`

Both `send()` and `receive()` individually acquire `self.op_lock`. The default `send_receive()` calls them sequentially, releasing the lock between operations. Another task could interleave, breaking request-response pairing.

**Fix:** Override `send_receive` in `UsbControlTransport` to hold the lock across both SET_REPORT and GET_REPORT operations.

---

### H2. Backend mutex held across async network I/O

**File:** `core/device/manager.rs:209-212`

```rust
let mut backend = backend.lock().await;
backend.write_colors(&device_id, &frame.colors).await
```

Lock held for entire `write_colors` duration (UDP send for WLED, TCP write for OpenRGB). All output queues sharing the same backend serialize through a single mutex. At 60fps with 5+ devices, this bottlenecks.

**Fix:** Redesign `DeviceBackend::write_colors` to take `&self` instead of `&mut self`, or give each device its own backend instance.

---

### H3. Dual source of truth for device state

**Files:** `core/device/registry.rs` vs `core/device/lifecycle.rs`

`DeviceRegistry` maintains its own `DeviceState` per device, while `DeviceLifecycleManager` maintains separate `DeviceStateMachine` instances. No synchronization between them. The registry can be mutated directly via `set_state()` or `update_user_settings()`.

**Fix:** Make `DeviceLifecycleManager` the sole state authority. Remove `set_state()` from the registry's public API.

---

### H4. OutputQueue swallows write errors silently

**File:** `core/device/manager.rs:231-238`

Backend write errors are logged with `warn!` and stored in metrics, but never propagated to the lifecycle manager. Devices that consistently fail writes continue receiving frames indefinitely.

**Fix:** Add an error channel from OutputQueue to lifecycle manager. Trigger reconnection after N consecutive failures.

---

### H5. OpenRGB single TCP stream serializes all operations

**File:** `core/device/openrgb/client.rs:119-134`

Single `TcpStream` for all operations, behind `Arc<Mutex<...>>`. Combined with H2, updates to device A block updates to device B.

**Fix:** Document as known limitation. For higher throughput, consider separate connections per controller.

---

### H6. `expect()` in HTTP handlers can panic the server

**Files:** `daemon/api/profiles.rs:77,149`, `daemon/api/layouts.rs:124,204,254`

Pattern: `profiles.get(&key).expect("resolved profile key must exist")`. Concurrent modification between resolve and get could panic, crashing the tokio task.

**Fix:** Return `ApiError::internal(...)` if the resolved key is unexpectedly missing.

---

### H7. Non-constant-time token comparison

**File:** `daemon/api/security.rs:280-292`

Standard string comparison short-circuits on first mismatched byte, enabling timing attacks.

**Fix:** Use `subtle::ConstantTimeEq` or equivalent constant-time comparison.

---

### H8. Config set allows arbitrary key creation via dot-path

**File:** `daemon/api/config.rs:167-188`

`set_json_path` creates intermediate objects if they don't exist. No allowlist of settable keys. Can modify `daemon.listen_address`, `daemon.port`, and any future security settings remotely.

**Fix:** Implement an allowlist of remotely-settable config keys.

---

### H9. No max WebSocket client connection limit

**File:** `daemon/api/ws.rs:34`

`WS_CLIENT_COUNT` tracks clients but never enforces a limit. Each connection spawns 5+ relay tasks.

**Fix:** Check against configurable max (e.g., 32) before upgrading. Return 503 when exceeded.

---

### H10. Tokens accepted in query string

**File:** `daemon/api/security.rs:343-352`

Tokens in URLs are logged by web servers, proxies, browser history, and referrer headers. The `TraceLayer` logs full URIs including tokens.

**Fix:** For WS auth, use first-message handshake or `Sec-WebSocket-Protocol`. Redact query params in trace logs.

---

### H11. Global singleton Servo worker with no recovery

**File:** `core/effect/servo_renderer.rs:43`

```rust
static SERVO_WORKER: OnceLock<Mutex<Option<Arc<ServoWorker>>>> = OnceLock::new();
```

If a previous effect corrupts Servo state, all subsequent effects inherit it. No invalidation/respawn mechanism.

**Fix:** Add `reset_servo_worker()` that drops the cached Arc and forces a fresh spawn.

---

### H12. Per-pixel color space conversions on hot render path

**Files:** `core/effect/builtin/rainbow.rs:50-59`, `core/effect/builtin/gradient.rs:78-92`

Rainbow calls Oklch→RgbaF32 per pixel. Gradient calls Oklab lerp + conversion per pixel. At 320x200@60fps, that's ~3.84M color space conversions/second.

**Fix:** Hoist endpoint conversions out of `tick()` (recompute on control change). Consider hue→RGB lookup table for rainbow.

---

### H13. CLI has no HTTP timeout

**File:** `cli/client.rs:24`

```rust
let http = reqwest::Client::new();
```

No connect or request timeout. CLI blocks indefinitely if daemon hangs.

**Fix:** `reqwest::Client::builder().connect_timeout(5s).timeout(30s).build()`

---

### H14. CLI test harness duplicates entire command structure

**File:** `cli/tests/cli_tests.rs:14-371`

371 lines of hand-rolled `clap::Command` builders that will drift from the actual `Cli` struct.

**Fix:** Convert to lib+bin pattern. Move CLI types to `lib.rs`, use `Cli::try_parse_from(...)` in tests.

---

## Medium Findings

### Audio Pipeline

| # | File | Issue |
|---|------|-------|
| M1 | `core/input/audio/mod.rs:76-77` | Hardcoded 48kHz sample rate — wrong for 44.1kHz devices |
| M2 | `core/input/audio/beat.rs:73-76` | EMA smoothing not frame-rate independent |
| M3 | `core/input/audio/fft.rs:233-236` | `realfft::process` allocates scratch buffer every frame |
| M4 | `core/input/audio/mod.rs:278-279` | Fixed `dt = 1/60` ignores actual frame timing |
| M5 | `core/input/audio/mod.rs:100-103` | Gain path allocates Vec in hot audio callback |
| M6 | `core/input/audio/features.rs:280-287` | Feature smoothing not frame-rate independent |
| M7 | `core/input/screen/smooth.rs:93` | Scene-cut threshold not normalized by zone count |

### Effect System

| # | File | Issue |
|---|------|-------|
| M8 | `types/effect.rs:283-285` | `to_js_literal` incomplete escaping — newlines/control chars bypass |
| M9 | `core/effect/lightscript.rs:369-529` | ~8KB string allocation per audio frame at 60fps |
| M10 | `core/effect/builtin/audio_pulse.rs:62` | Beat decay is frame-rate dependent |
| M11 | `core/effect/engine.rs:264` | `AudioData::clone()` with heap Vecs on every frame |
| M12 | `core/effect/paths.rs:23-50` | Path traversal possible in `resolve_html_source_path` |
| M13 | `core/effect/servo_renderer.rs:820-841` | Temp files leak on daemon crash |
| M14 | `core/effect/servo_renderer.rs:514,574` | 1ms busy-wait loops in script eval and page load |
| M15 | `core/effect/loader.rs:220-234` + `builtin/mod.rs:153-166` | Duplicated deterministic UUID hash function |

### HAL

| # | File | Issue |
|---|------|-------|
| M16 | `hal/drivers/razer/devices.rs:92-121` | Only 4 Razer devices registered |
| M17 | `hal/transport/control.rs:38-41` | `claim_interface` instead of `detach_and_claim_interface` — fails with kernel drivers |
| M18 | `hal/transport/control.rs:117-120` | `close()` doesn't release USB interface |
| M19 | `hal/drivers/razer/types.rs:63` | `ExtendedArgb` matrix type treats 4-byte ARGB as 3-byte RGB |
| M20 | `hal/drivers/razer/protocol.rs:226-276` | `build_packet` silently drops oversized payloads |
| M21 | `hal/protocol.rs:414` | `supports_brightness: true` but no brightness control implemented |

### Device Management

| # | File | Issue |
|---|------|-------|
| M22 | `core/device/state_machine.rs:289-293` | `on_hot_unplug` overrides user-set `Disabled` state |
| M23 | `core/device/lifecycle.rs:158-159` | `on_discovered` overwrites backend_id on active devices |
| M24 | `core/device/traits.rs:73` vs `discovery.rs:45` | `DeviceBackend::discover` returns `DeviceInfo` without fingerprints |
| M25 | `core/device/openrgb/client.rs:443-466` | `recv_expected_packet` has no iteration limit |
| M26 | `core/device/openrgb/proto.rs:473-474` | LED count truncated to u16 in `build_update_leds` |
| M27 | `core/device/wled/ddp.rs:80-81` | `debug_assert` for payload size instead of `assert` |

### Engine / Bus / Spatial / Scene

| # | File | Issue |
|---|------|-------|
| M28 | `core/scene/mod.rs:48,149` | `activation_history` grows without bound |
| M29 | `core/engine/mod.rs:160-164` | Render loop state inconsistency with atomic stop handle |
| M30 | `core/spatial/sampler.rs:108-112` | Asymmetric `AreaAverage` radius silently collapsed to square |
| M31 | `core/scene/transition.rs:178-201` | `_color_interp` parameter accepted but ignored |
| M32 | `core/config/paths.rs:20,43-46` | `expect("HOME must be set")` — panics in containers |

### Daemon Infrastructure

| # | File | Issue |
|---|------|-------|
| M33 | `daemon/startup.rs:717-730` | Signal handler only catches SIGINT, not SIGTERM |
| M34 | `daemon/startup.rs:266-321` | `start()` has no idempotency guard — double-call leaks threads |
| M35 | `daemon/discovery.rs:375-406` | Vanished device detection broadens across backends |
| M36 | `daemon/render_thread.rs:577-587` | Fallback canvas uses hardcoded dimensions |
| M37 | `daemon/render_thread.rs:196-199` | Write lock held during `tick()` blocks API readers |
| M38 | `daemon/library.rs:229-259` | Blocking `std::fs` I/O in async context |
| M39 | `daemon/playlist_runtime.rs` | 48-line stub — no playlist execution logic |

### Daemon API

| # | File | Issue |
|---|------|-------|
| M40 | `daemon/api/security.rs:354-382` | Rate limit client identity from spoofable `X-Forwarded-For` |
| M41 | `daemon/api/layouts.rs:213` | Mixed PUT/PATCH semantics — `description` silently cleared |
| M42 | Multiple list endpoints | Fake pagination — hardcoded `offset: 0, limit: 50, has_more: false` |

### Cross-Crate

| # | File | Issue |
|---|------|-------|
| M43 | `core/Cargo.toml:36` | `nusb` directly in core crate — layer violation, should go through HAL |
| M44 | `core/device/usb_backend.rs:293` + `usb_scanner.rs:159` | `usb_path()` function duplicated |

### CLI

| # | File | Issue |
|---|------|-------|
| M45 | `cli/client.rs:19-95` | No retry logic for transient failures |
| M46 | Multiple daemon endpoints | ~12 API endpoints have no CLI command coverage |
| M47 | `cli/commands/effects.rs:60-65` | `--speed`/`--intensity` accept 0..u32::MAX despite 0-100 docs |
| M48 | `cli/commands/library.rs:249,325,489` | Timestamps displayed as raw epoch milliseconds |

### Types

| # | File | Issue |
|---|------|-------|
| M49 | `types/audio.rs:33-66` | `AudioData` uses `Vec` for fixed-size arrays (heap allocs on every clone) |
| M50 | `types/spatial.rs:336` | `DeviceZone::size` uses `NormalizedPosition` for width/height |
| M51 | `types/canvas.rs:313-323` + `device.rs:327-340` | Duplicate color format enums without conversion impls |
| M52 | `types/event.rs:287-617` | `HypercolorEvent` (35+ variants) missing `#[non_exhaustive]` |

---

## Code Duplication Hotspots

| Pattern | Locations | Fix |
|---------|-----------|-----|
| `usb_path()` function | `usb_backend.rs` + `usb_scanner.rs` | Extract to `usb_util` module |
| `epoch_to_utc()` calendar math | `bus/mod.rs` + `ws.rs` + `envelope.rs` | Shared `time` module |
| Deterministic UUID hash | `effect/loader.rs` + `builtin/mod.rs` | Shared helper with seed param |
| Resolve-by-id-or-name | 6 API handlers | Generic trait/helper |
| Fake pagination defaults | 6+ list endpoints | `Pagination::single_page()` constructor |
| `parse_key_value()` | `effects.rs` + `library.rs` (CLI) | Extract to `commands/common.rs` |
| Sorting logic | `InMemoryLibraryStore` + `JsonLibraryStore` | Extract to methods on `InMemoryLibraryData` |

---

## Architecture — What's Done Well

1. **Zero `todo!()`, `unimplemented!()`, or `unwrap()` in source code** — lint discipline is excellent
2. **Lock-free event bus** — `broadcast` for events, `watch` for frame data, no mutex in hot path
3. **Clean crate layering** — no circular deps, correct trait placement across crates
4. **Config uses `ArcSwap`** — lock-free reads from the render loop
5. **Error types compose well** — `thiserror` in libraries, `anyhow` in applications, clean boundaries
6. **Mock device backend is production-quality** — failure injection, builder pattern, realistic modes
7. **FPS controller is a pure state machine** — no threads, no I/O, clean tier shifting
8. **Test coverage is strong** — ~20K lines of tests across 30+ test files
9. **Discovery flag guard uses RAII** — `DiscoveryFlagGuard` always resets `in_progress` on drop
10. **Spatial module separation** — topology generation cleanly separated from sampling
11. **Scene priority stack** — FIFO within same priority, clean blending pipeline
12. **CLI SilkCircuit integration** — consistent color palette for terminal output
13. **CLI e2e harness** — port reservation, graceful shutdown, health polling infrastructure

---

## Missing Test Coverage (Notable Gaps)

| Area | What's Missing |
|------|---------------|
| `ControlDefinition::validate_value()` | Most complex logic in types crate — zero tests |
| `SpectrumData::downsample()` | Edge cases (empty bins, target >= source) |
| Legacy Razer protocol path | Different transaction IDs, command classes, headers |
| Scalar and linear encoding | Only matrix/extended tested |
| Firmware predicate DB lookup | Path exists but entirely untested |
| USB transport | Zero test coverage (mock or otherwise) |
| 0-width/0-height canvas in effects | Code handles it correctly but untested |
| `prune_missing` in effect registry | Production-critical hot-reload path |
| Stateful MCP tool handlers | Only stubs tested, not live handlers |
| Discovery scan logic | Core `execute_discovery_scan` untested |
| CLI `parse_playlist_item_spec` | Tricky colon-delimited parser, no unit tests |
| Beat decay across multiple frames | Only single-frame comparison tested |
| Perimeter loop edge cases | Only checks count, not corner/winding variants |
| Pipeline error recovery | No tests for renderer/scanner returning Err |

---

## Top 10 Action Items (Priority Order)

1. **Fix CORS + WS auth bypass + unbounded queue** (C1, C2, C3) — Security trifecta, blocks any network exposure
2. **Unify device state authority** (C4, H3) — Registry and LifecycleManager must agree on who owns state
3. **Add HTTP timeouts + connection limits** (H13, H9) — Both CLI and daemon need resource bounds
4. **Make audio pipeline frame-rate independent** (M2, M4, M10) — Smoothing, beat detection, and decay all drift under load
5. **Fix transport lock gap** (H1) — `send_receive` must hold mutex across both USB operations
6. **Add canvas dimension bounds** (C6, daemon layouts) — Prevent OOM via API-supplied dimensions
7. **Handle SIGTERM for daemon** (M33) — Essential for systemd/Docker deployments
8. **Move `nusb` behind HAL boundary** (M43) — Clean layer violation, extract USB enumeration to HAL
9. **Pre-allocate hot-path buffers** (M3, M9, M11) — FFT scratch, LightScript strings, AudioData clones
10. **Add `#[non_exhaustive]` to `HypercolorEvent`** (M52) — Prevents semver breaks as event taxonomy grows
