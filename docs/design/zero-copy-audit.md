# Zero-Copy Buffer Audit

**Date:** 2026-03-09
**Reviewed by:** Claude Opus 4.6 (author) + GPT-5.3 Codex (cross-model validation)
**Scope:** Render pipeline, spatial engine, bus, HAL layer, WS/preview pipeline

---

## Executive Summary

The render pipeline currently copies **~300-800KB of buffer data per frame** on the hot path.
At 60 FPS, that's up to **48 MB/s of unnecessary memcpy** — enough to thrash L2 cache and
create measurable latency spikes. Most of it traces back to three root causes: Canvas deep-clones,
layout deep-clones, and HAL buffer allocation churn.

The bus layer (`CanvasFrame::from_owned_canvas`, `Arc<Vec<u8>>` sharing) and spatial sampler
(`sample_into` with buffer reuse) are already well-designed. The violations are concentrated in
the render thread orchestration and HAL protocol encoding.

---

## Priority 1 — Canvas + FrameInputs Clone (Hot Path, Every Frame)

### 1a. Canvas Deep-Clone in `resolve_frame_canvas`

**Location:** `crates/hypercolor-daemon/src/render_thread.rs:436-447`

```rust
let canvas = if let (SkipDecision::ReuseCanvas, Some(previous)) =
    (skip_decision, cached_canvas.as_ref())
{
    previous.clone()           // 256KB memcpy — reuse path
} else if let Some(screen_canvas) = inputs.screen_canvas.clone() {
    *cached_canvas = Some(screen_canvas.clone());  // 256KB × 2 — screen capture path
    screen_canvas
} else {
    let rendered = render_effect(...).await;
    *cached_canvas = Some(rendered.clone());  // 256KB — fresh render path
    rendered
};
let mut canvas = canvas;
apply_output_brightness(&mut canvas, brightness);  // mutates in-place
```

**Problem:** All three paths clone a 320×200×4 = 256KB Canvas. The clone exists because:
- The cache needs the *pre-brightness* canvas (unmodified)
- `apply_output_brightness` mutates in-place at line 449
- So we can't simply swap/take — we need both a cached copy and a working copy

**Impact:** 256KB minimum per frame, 512KB on screen capture path. At 60 FPS = **15-30 MB/s**.

**Fix options:**

1. **Double-buffer swap:** Maintain two Canvas buffers. Cache holds pre-brightness; working buffer
   gets brightness applied. Swap roles each frame instead of cloning:
   ```rust
   // Render/capture into cache_canvas
   // Copy cache → working via fast memcpy into pre-allocated buffer
   // Apply brightness to working buffer only
   ```

2. **Arc + CoW:** Store `Arc<Canvas>` in cache. When brightness is needed, `Arc::make_mut()` gives
   a copy-on-write clone — same cost as today but only when brightness != 1.0, and zero-cost when
   brightness is at max.

3. **Deferred brightness:** Apply brightness during spatial sampling instead of mutating the canvas.
   Eliminates the need for the clone entirely, but changes the pipeline contract.

### 1b. FrameInputs Clone Every Frame

**Location:** `crates/hypercolor-daemon/src/render_thread.rs:311-317`

```rust
let inputs = match skip_decision {
    SkipDecision::None => {
        *cached_inputs = sample_inputs(state).await;
        cached_inputs.clone()  // Clone AudioData (3 Vecs) + Option<Canvas>
    }
    SkipDecision::ReuseInputs | SkipDecision::ReuseCanvas => cached_inputs.clone(),
};
```

**Problem:** `FrameInputs` contains `AudioData` with three heap Vecs:
- `spectrum`: 200 × f32 = 800 bytes
- `mel_bands`: 24 × f32 = 96 bytes
- `chromagram`: 12 × f32 = 48 bytes
- Plus `Option<Canvas>` (256KB when screen capture active)

Clone happens on *every* path — fresh sample and reuse.

**Impact:** ~1KB per frame normally, ~257KB when screen capture is active.

**Fix:** Borrow `&cached_inputs` instead of cloning. The inputs are only used by reference
in stages 2-5 (`resolve_frame_canvas` already takes `&FrameInputs`). GPT-5.3 confirmed no
async/lifetime blockers — the borrow lives within a single frame iteration.

### 1c. Effect Engine Double-Clone (Missed in initial audit)

**Location:** `crates/hypercolor-core/src/effect/engine.rs:331-332`

**Problem:** The effect engine clones audio and interaction data *again* inside its render tick,
on top of the FrameInputs clone. So audio data is cloned twice per frame on the hot path.

**Fix:** Pass by reference into the effect engine render path.

---

## Priority 2 — SpatialLayout Deep-Clone

**Location:** `crates/hypercolor-daemon/src/render_thread.rs:336` (also line 519)

```rust
let (zone_colors, layout) = {
    let spatial = state.spatial_engine.read().await;
    spatial.sample_into(&canvas, &mut recycled_frame.zones);
    let layout = spatial.layout().clone();  // Deep clone entire layout
    (&recycled_frame.zones, layout)
};
// ... RwLock dropped here ...
let write_stats = manager.write_frame(&zone_colors, &layout).await;
```

**Problem:** `SpatialLayout` contains:
- `id: String`, `name: String`, `description: Option<String>`
- `zones: Vec<DeviceZone>` where each zone has strings, topology, `Vec<NormalizedPosition>`
- Deep clone cost: 10-50KB for typical 10-20 zone configs

The clone exists to drop the `spatial_engine` read lock before acquiring the
`backend_manager` mutex lock (deadlock avoidance).

**Impact:** 10-50KB per frame × 60 FPS = **600KB - 3MB/s**.

**Fix:** Wrap the layout in `Arc<SpatialLayout>` inside `SpatialEngine`:
```rust
pub fn layout(&self) -> Arc<SpatialLayout> {
    Arc::clone(&self.layout)
}
```

Layout mutations (user edits) are cold-path — `Arc::make_mut()` or full replacement is fine.
`write_frame` takes `&SpatialLayout`, and `Arc<SpatialLayout>` auto-derefs. Drop-in change.

Both models confirmed this is sound and high-impact.

---

## Priority 3 — HAL Protocol Buffer Churn

### 3a. Protocol Trait Forces Fresh Allocations

**Location:** `crates/hypercolor-hal/src/protocol.rs:21, 107`

```rust
fn encode_frame(&self, colors: &[[u8; 3]]) -> Vec<ProtocolCommand>;
// Each ProtocolCommand owns:
pub struct ProtocolCommand {
    pub data: Vec<u8>,  // Fresh allocation per packet
}
```

Every driver allocates fresh `Vec<u8>` buffers for each USB packet, every frame.

**Representative drivers:**
- Razer: `protocol.rs:431` — `data: packet.to_vec()`
- Corsair LN: `protocol.rs:146` — `Vec::new()` per command
- PrismRGB: `protocol.rs:610` — `data: packet.to_vec()`
- ASUS: `protocol.rs:296` — `Vec::new()` per command set

**Impact:** ~200-400 heap allocations/sec per device.

### 3b. Color Frame Normalization Copies

**Location:** Multiple drivers

```rust
// razer/protocol.rs:299
return colors.to_vec();  // Full color array copy

// corsair/lighting_node/protocol.rs:48
return colors.to_vec();  // Full color array copy
```

When LED count doesn't match the device channel config, the entire color slice is
`.to_vec()`'d before packetization.

### 3c. Corsair LINK Keepalive Cache

**Location:** `crates/hypercolor-hal/src/drivers/corsair/link/protocol.rs:281`

Full command list cloned into keepalive cache every frame.

**Aggregate impact:** For a 3-device setup, ~600-1200 heap allocs/sec + color array copies.

**Fix:** Refactor `Protocol` trait to accept reusable buffers:
```rust
fn encode_frame_into(
    &self,
    colors: &[[u8; 3]],
    commands: &mut Vec<ProtocolCommand>,
    scratch: &mut Vec<u8>,
);
```

This is the highest-effort fix (touches every driver) but eliminates the most allocation churn.
Could be done incrementally — add `_into` variant, migrate drivers one at a time.

---

## Priority 4 — WebSocket Pipeline

### 4a. Binary Encoding Without Capacity

**Location:** `crates/hypercolor-daemon/src/api/ws.rs:1289, 1321`

```rust
fn encode_frame_binary(frame: &FrameData) -> Vec<u8> {
    let mut out = Vec::new();  // No capacity hint
    out.push(0x01);
    out.extend_from_slice(&frame.frame_number.to_le_bytes());
    // ... multiple extends cause 3-5 reallocations
}

fn encode_spectrum_binary(...) -> Vec<u8> {
    let mut out = Vec::new();  // No capacity hint
    // ... same pattern
}
```

Note: `encode_canvas_binary` at line 1353 already correctly uses `Vec::with_capacity()`.

**Fix:** Pre-calculate and use `Vec::with_capacity()`:
```rust
let capacity = 10 + included_zones.len() * (1 + MAX_ZONE_ID_LEN + 2 + zone.colors.len() * 3);
let mut out = Vec::with_capacity(capacity);
```

Trivial change, eliminates ~90 reallocations/sec per WS client.

### 4b. WS Relay Clones Before Subscription Check

**Location:** `crates/hypercolor-daemon/src/api/ws.rs:766-769, 842-845`

**Problem:** Frame and spectrum data are cloned from watch channels even for clients that only
subscribe to events (the default). Every connected client pays the clone cost regardless of
whether they consume the data.

**Fix:** Check subscription filter *before* cloning from watch channel.

### 4c. Spectrum Clone on Bus Publish

**Location:** `crates/hypercolor-daemon/src/render_thread.rs:728`

```rust
bins: audio.spectrum.clone(),  // 200 × f32 = 800 bytes per frame
```

**Impact:** Low — 800 bytes × 60 FPS = 48 KB/s.

**Fix:** `Arc<Vec<f32>>` for spectrum bins in `AudioData`, or accept the cost as negligible.

---

## Priority 5 — Display Output

### JPEG Arc Wrap/Unwrap Cycle

**Location:** `crates/hypercolor-daemon/src/display_output.rs:408, 443`

```rust
let jpeg = Arc::new(encoded);           // Wrap in Arc (allocation)
// ... sent to USB backend watch queue (usb_backend.rs:1269) ...
if let Some(reusable) = Arc::into_inner(jpeg) {  // Try to recover
    encode_state.jpeg_buffer = reusable;
}
```

**Problem (worse than initially assessed):** GPT-5.3 identified that `Arc::into_inner` likely
returns `None` most of the time because the USB backend holds a reference in its watch queue.
This means the buffer is **not being recycled** — it's re-allocated every frame.

**Impact:** JPEG buffer size varies (5-50KB typical) × 15 FPS per display target.

**Fix:** Requires queue/buffer-reuse redesign. The Arc exists for async safety into the USB
backend. Options:
- Ring buffer with fixed slots for JPEG data
- `tokio::sync::watch` already clones on receive — lean into that instead of Arc

---

## Already Well-Designed (No Action Needed)

| Component | Why It's Good |
|-----------|--------------|
| `CanvasFrame::from_owned_canvas` | Zero-copy canvas → `Arc<Vec<u8>>` via `into_rgba_bytes()` |
| `CanvasFrame` sharing | `Arc<Vec<u8>>` enables cheap clones across subscribers |
| `SpatialEngine::sample_into` | Reuses `Vec<ZoneColors>` buffer across frames |
| Frame recycling | `send_replace` returns previous frame for buffer reuse |
| Display resampling cache | Recent optimization (`ec21f5e`) caches resampling work |

---

## Implementation Roadmap

### Phase 1 — Render Loop (Highest Impact, Self-Contained)

1. Refactor `resolve_frame_canvas` to eliminate Canvas deep-clone
   - Evaluate Arc+CoW vs double-buffer vs deferred brightness
2. Borrow `&cached_inputs` instead of cloning `FrameInputs`
3. Pass audio/interaction by reference into effect engine

**Expected savings:** ~256-512KB eliminated per frame

### Phase 2 — Spatial Layout (High Impact, Easy)

4. Wrap `SpatialLayout` in `Arc` inside `SpatialEngine`
5. Return `Arc::clone()` from `layout()` instead of deep clone

**Expected savings:** ~10-50KB eliminated per frame

### Phase 3 — HAL Buffer Reuse (Medium Impact, High Effort)

6. Add `encode_frame_into` variant to `Protocol` trait
7. Migrate drivers incrementally (Razer → Corsair → ASUS → etc.)
8. Eliminate `normalize_colors` copies where possible

**Expected savings:** 200-400 heap allocations/sec eliminated per device

### Phase 4 — WebSocket Polish (Low-Medium Impact, Easy)

9. `Vec::with_capacity()` in frame/spectrum binary encoders
10. Check subscription before cloning watch channels
11. Consider `Arc<Vec<f32>>` for spectrum data

### Phase 5 — Display Pipeline Redesign (Medium Impact, Medium Effort)

12. Redesign JPEG buffer lifecycle to avoid Arc alloc/dealloc cycle
13. Consider ring buffer or direct ownership transfer

---

## Estimated Aggregate Impact

| Metric | Before | After (All Phases) |
|--------|--------|-------------------|
| Per-frame memcpy | 300-800 KB | < 10 KB |
| Per-frame heap allocs | 15-25 | 3-5 |
| Sustained copy bandwidth (60 FPS) | 18-48 MB/s | < 1 MB/s |
| HAL allocs/sec (3 devices) | 600-1200 | ~50 (cold-path only) |
