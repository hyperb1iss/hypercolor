+++
title = "Low FPS / stuttering"
description = "Diagnose Servo overhead, USB-bus saturation, and render budget problems using hypercolor diagnose and the daemon's built-in metrics."
weight = 60
+++

# Low FPS / stuttering ⚡

Hypercolor's render loop targets up to 60 FPS using an adaptive five-tier controller that shifts between tiers based on measured frame budget. When the system is healthy it runs at **Full (60 FPS)**; sustained budget misses drive it down through **High (45) → Medium (30) → Low (20) → Minimal (10)**. The first sign of a performance problem is usually the tier sitting lower than expected, not a hard crash.

This page walks you through reading the `diagnose` output, identifying which part of the pipeline is the bottleneck, and fixing the root cause.

{% callout(type="warning") %}
Never lower the FPS cap, canvas resolution, or LED output rate as a "fix." Those are product baselines and performance contracts. The right approach is to identify the actual bottleneck using the metrics below and address it there.
{% end %}

---

## Step 1: run the diagnostics

```bash
hypercolor diagnose
```

This calls `POST /api/v1/diagnose` against the running daemon. By default it runs four check groups: `daemon`, `render`, `devices`, and `config`. Use `--check` to focus on one:

```bash
hypercolor diagnose --check render
hypercolor diagnose --check devices
```

Add `--system` to include verbose system information (GPU, kernel, audio version, and daemon uptime).

To save a full report for a bug report:

```bash
hypercolor diagnose --system --report ~/hypercolor-bug-report.json
```

Output is a styled table by default. Use `--format plain` for a one-line text summary or `--format json` (shorthand: `-j`) for machine-readable JSON.

---

## Step 2: read the render checks

The `render` category has three checks for FPS problems.

### `render_loop`

```
render_loop   state=running, tier=medium
```

The `tier` field shows where the FPS controller has landed. `medium` (30 FPS) or lower means the loop has downshifted due to budget pressure. If `state` is anything other than `running`, the render loop has stopped — see [common issues](@/troubleshooting/common-issues.md).

**Tier reference:**

| Tier | FPS | Frame budget |
|------|-----|-------------|
| minimal | 10 | 100 ms |
| low | 20 | 50 ms |
| medium | 30 | 33.3 ms |
| high | 45 | 22.2 ms |
| full | 60 | 16.6 ms |

The controller downshifts after 2 consecutive budget misses and only upshifts after sustained headroom, so a stuck tier is a real signal.

### `frame_liveness`

```
frame_liveness   frame_token=18432, frame_age_ms=47.30
```

`frame_age_ms` is how stale the last completed frame is. A value of 2000 ms or more triggers a warning; 10 000 ms or more is a failure, meaning the loop is stalled or not producing output. Check daemon logs for Servo errors or device write failures.

### `led_freshness`

```
led_freshness   output_source=current_frame, gpu_sample_stale=false, devices_written=4, total_leds=512, sample_us=1200, push_us=3400
```

Key flags to watch:

- `gpu_sample_stale=true` — the GPU-side canvas sample was not ready in time and the compositor reused the previous frame's sample. Sustained stale GPU samples accumulate in the `recent_window` counters (step 4).
- `gpu_sample_wait_blocked=true` — the render loop blocked on the GPU sample queue, meaning the sampling path is a bottleneck.
- `gpu_sample_queue_saturated=true` — the GPU import slot pool is exhausted; multiple frames are queued behind a single GPU fence.
- `output_source` — `current_frame` is healthy. `published_frame` or `routed_reuse` means the loop is serving a cached frame because the current render is not keeping up.
- `sample_us` / `push_us` — time in microseconds for spatial sampling (canvas to LED positions) and writing to devices. Elevated `push_us` across many devices points to USB saturation (step 3).

---

## Step 3: check device output queues

The `devices` category includes two checks relevant to USB-bus and output latency.

### `output_queues`

```
output_queues   queues=6, usb_queues=4, lagging=1, worker_finished=0, dropped_total=0, errors_total=0
```

- `lagging` — queues where `fps_sent` is significantly behind `fps_queued`. This is the primary indicator of USB-bus saturation: the daemon is producing frames faster than the USB subsystem can flush them.
- `worker_finished` — a worker thread exited unexpectedly. Any non-zero value is a hard failure and appears as `fail` status.
- `dropped_total` — cumulative frame drops across all device queues.

For the detailed per-device breakdown, use `-j` and inspect `snapshot.device_output.items`. Each item includes `fps_sent`, `fps_queued`, `fps_target`, `avg_write_ms`, `avg_queue_wait_ms`, and `last_error`.

### `usb_actor_display_lane`

```
usb_actor_display_lane   display_frames=1240, delayed_for_led=3, wait_avg_ms=0.12, wait_max_ms=1.80
```

The USB actor shares a single bus arbitration lane between LED output and display-face rendering. If `wait_max_ms` reaches 2 ms or higher, the check flips to warning, meaning LED writes are consistently blocking the display lane. This typically means too many USB devices competing for the same controller.

**Fix for USB saturation:** spread devices across multiple USB controllers (different root hubs, not just different ports on the same hub). Check your topology with `lsusb -t`. Adding a PCIe USB card is the reliable fix when you have more than four or five high-frequency RGB devices on a single system.

---

## Step 4: identify Servo overhead

If you are running HTML effects (TypeScript SDK effects or any effect with an `.html` source), the [render pipeline](@/architecture/render-pipeline.md) passes through Servo, a full browser engine embedded in the daemon. Servo renders on a dedicated OS thread with its own event loop, paint, and readback phases, and that overhead adds directly to your frame budget.

Run the JSON report and examine `snapshot.render.recent_window`:

```bash
hypercolor diagnose -j | jq '.data.snapshot.render.recent_window'
```

Fields to watch:

- `gpu_sample_stale` / `gpu_sample_deferred` — count of frames in the recent window where Servo's frame was not ready. A high ratio to `frames` means Servo is your bottleneck.
- `gpu_sample_cpu_fallback` — Servo fell back from GPU import to a CPU readback path. This adds a full-frame pixel readback cost and is significantly slower.
- `push_p95_ms` / `publish_p95_ms` — 95th-percentile device write and event-bus publish latency. Healthy values sit well under the frame budget for your current tier.

For Servo-specific timings, the daemon status response (`GET /api/v1/status`) carries an effect-health section with fields including `render_frame_max_us`, `render_evaluate_scripts_max_us`, `render_paint_max_us`, and `render_readback_max_us`. A `render_frame_max_us` value near or above the tier budget is a clear Servo bottleneck.

### Servo circuit breaker

Servo uses a circuit breaker to protect the daemon when the effect renderer fails repeatedly. After 3 consecutive failures it opens, blocking new render attempts for a 30-second cooldown with exponential backoff up to 5 minutes. The breaker state is visible in the Servo telemetry fields:

- `breaker_opens_total` non-zero — Servo has tripped the breaker at least once this session.
- `soft_stalls_total` — frames that stalled without a fatal failure.

When the breaker is open, effects fall back to a black/idle canvas, meaning your LEDs stop updating or hold the last frame. Switching to a native (non-HTML) effect clears the dependency on Servo immediately.

**Fixes for Servo overhead:**

- Switch to one of the native built-in effects if frame timing is critical; they run in compiled Rust with no browser overhead. Browse what is available in the [effects section](@/effects/_index.md).
- If you want to keep an HTML effect, reduce the effect's JavaScript complexity. Heavy `requestAnimationFrame` work, large canvas operations, and expensive audio FFT processing are common culprits. See [debugging and diagnostics](@/contributing/debugging.md) for Servo profiling techniques.
- If `render_gpu_import_fallback_reason` appears in the JSON output, the GPU zero-copy import path is unavailable and Servo is doing full CPU readbacks every frame. Verify that your GPU drivers support the required Vulkan or OpenGL extensions for your platform.

---

## Step 5: check the render budget breakdown

The `snapshot.render.latest_frame` object in the JSON report gives a per-stage breakdown of the most recent completed frame in microseconds:

```json
{
  "input_us": 120,
  "render_us": 8400,
  "producer_us": 9100,
  "composition_us": 310,
  "sample_us": 1200,
  "push_us": 3400,
  "publish_us": 90,
  "overhead_us": 200,
  "total_us": 22820
}
```

At Full tier the budget is 16 666 µs. If `total_us` consistently exceeds the budget, the loop cannot hold 60 FPS regardless of what tier it is targeting.

| Field | What it measures | High value means |
|-------|-----------------|-----------------|
| `render_us` | Effect renderer time (Servo or native) | Servo effect is slow; switch or optimize |
| `producer_us` | Canvas producer pipeline (includes Servo wait) | Effect frame is arriving late |
| `sample_us` | Spatial sampling (canvas to LED positions) | Many LEDs, complex layout, or slow GPU sample |
| `push_us` | Writing colors to device queues | USB saturation or slow device firmware |
| `composition_us` | SparkleFlinger compositor | Usually negligible; a spike indicates many active layers |

---

## Common scenarios

### Tier stuck at `medium` or below, no device errors

The render loop is consistently overrunning its budget. Inspect `total_us` in `latest_frame`. If `render_us` or `producer_us` dominates, the bottleneck is the effect renderer — most likely Servo. If `push_us` is high, it is device output. Address each at its root; do not lower the tier ceiling.

### `gpu_sample_stale` climbing in `recent_window`

Servo is not delivering frames fast enough to keep up with the render loop. Check `render_frame_max_us` and `render_evaluate_scripts_max_us` in the Servo telemetry to isolate whether JavaScript evaluation or paint is the slow phase.

### Devices showing `lagging=N` in output queues

N device queues are receiving frames faster than they can send them. This is USB bandwidth exhaustion. Use `-j` and check `avg_write_ms` per device: values significantly above `1000 / fps_target` indicate the device itself is slow to acknowledge writes.

### `worker_finished=N` in output queues

N device worker threads have exited. This is a hard error: those devices are no longer receiving output. Check `last_error` in the JSON. Common causes are USB disconnect, permission errors, or device firmware lockup. Try replugging the device. If errors persist, see [USB devices](@/hardware/usb-devices.md) for udev and hot-plug guidance.

### Servo breaker open

Check daemon logs for Servo-related errors or `session_create_failures`. If the breaker is cycling (opens, cools down, opens again), the effect itself is broken — likely a JavaScript runtime error. Switch effects to confirm, then file a bug with the `--report` output attached.

---

## Getting more detail

For a deeper look at the render architecture that produced these metrics, see [render pipeline](@/architecture/render-pipeline.md) and [renderer internals](@/architecture/renderer-internals.md).

If device output looks healthy but you still see stuttering at the hardware level, check [USB devices](@/hardware/usb-devices.md) for controller topology and permission guidance.

To attach a full diagnostic report to a bug report:

```bash
hypercolor diagnose --system --report ~/hypercolor-bug-report.json
```
