+++
title = "Low FPS / stuttering"
description = "Diagnose Servo overhead, USB-bus saturation, and render budget problems using hypercolor diagnose and the daemon's built-in metrics."
weight = 60
+++

Hypercolor's render loop targets your configured `daemon.target_fps` ceiling using an adaptive five-tier controller (Minimal 10 → Low 20 → Medium 30 → High 45 → Full 60) that shifts between tiers based on measured frame budget. Healthy means holding the tier that matches your configured target. The default target is **60 FPS**, so on a stock config the loop sits at **Full** when everything is fine. The first sign of a performance problem is the tier sitting *below your configured target*, not a hard crash.

This page walks you through reading the `diagnose` output, identifying which part of the pipeline is the bottleneck, and fixing the root cause.

{% <callout type="warning"> %}
Never lower the FPS cap, canvas resolution, or LED output rate as a "fix." Those are product baselines and performance contracts. The right approach is to identify the actual bottleneck using the metrics below and address it there.
{% </callout> %}

---

## Step 1: run the diagnostics

```bash
hypercolor diagnose
```

This calls `POST /api/v1/diagnose` against the running daemon. By default it runs eight check groups: `daemon`, `render`, `devices`, `conflicts`, `config`, `input`, `memory`, and `openrgb`. Use `--check` to focus on one:

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

The `render` category has seven checks for FPS and LED output problems.

### `render_loop`

```
render_loop   state=running, tier=60fps (full)
```

The `tier` field shows where the FPS controller has landed, as its rate and name. Compare it against your configured `daemon.target_fps`: on the default config, `full` (60 FPS) is the healthy steady state. A tier sitting below your configured target means the loop has downshifted under budget pressure. If `state` is anything other than `running`, the render loop is not rendering; see [common issues](@/troubleshooting/common-issues.md). A `paused` loop renders nothing on purpose, so the detail names what paused it. A manual output pause (from `PATCH /api/v1/output`, a pause control, or an integration such as Home Assistant) persists across daemon restarts until output power is set back to running.

**Tier reference:**

| Tier | FPS | Frame budget |
|------|-----|-------------|
| minimal | 10 | 100 ms |
| low | 20 | 50 ms |
| medium | 30 | 33.3 ms |
| high | 45 | 22.2 ms |
| full | 60 | 16.6 ms |

The controller downshifts after 2 consecutive budget misses and only upshifts after sustained headroom, so a tier stuck below your target is a real signal.

```
render_loop   state=running, tier=full, parked=no_output_consumers
```

`parked` means nothing consumes rendered frames: no connected device, no open preview or stream, no capture. The loop renders nothing until one appears, then resumes at its full tier, so `0 fps` here is expected rather than a stall. `frame_liveness` passes while parked, and `hypercolor status` shows the same state on its render line. `snapshot.render.output_demand` in the JSON report lists the consumer counts the loop last saw and what last woke it.

### `render_thread`

```
render_thread   phase=running, last_frame_age_ms=12, restarts=0, last_failure=none
```

The render thread supervises its own pipeline. When a frame panics or the pipeline loses its input feed, the daemon logs the failure with a backtrace, rebuilds the pipeline with fresh GPU and render state, and keeps going. This check fails while that restart is in progress, when the thread has gone without completing a frame for 3 seconds while the loop runs, and when the thread is gone while the loop still says it is running. A paused or parked loop passes as long as the thread keeps polling, since no frames are due, and the detail names what paused it.

`restarts` counts in-process restarts since the daemon started. After five restarts within five minutes, or when the GPU device is lost, the daemon stops recovering in-process and exits with an error so its service manager starts a fresh process. Under systemd, a render thread stuck inside one frame for 20 seconds also stops the service watchdog keepalive, so systemd restarts the daemon. Every failure is also written to `render-failure.json` in the daemon state directory (`~/.local/state/hypercolor/` on Linux), so it survives a log that was lost. The next run reports that record here as `(earlier daemon run)`.

### `gpu_errors`

```
gpu_errors   validation=0, out_of_memory=0, internal=0, device_lost=0
```

GPU validation, out-of-memory, and internal errors that nothing handled are counted and logged instead of crashing the thread that raised them. One in the last 10 minutes turns this check into a warning, with the device and the latest message in the detail. The counts since startup always stay in the detail. A lost GPU device fails the check.

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

- `gpu_sample_stale=true`: the GPU-side canvas sample was not ready in time and the compositor reused the previous frame's sample. Sustained stale GPU samples accumulate in the `recent_window` counters (step 4).
- `gpu_sample_wait_blocked=true`: the render loop blocked on the GPU sample queue, meaning the sampling path is a bottleneck.
- `gpu_sample_queue_saturated=true`: the GPU import slot pool is exhausted; multiple frames are queued behind a single GPU fence.
- `output_source`: `current_frame` is healthy. `published_frame` or `routed_reuse` means the loop is serving a cached frame because the current render is not keeping up.
- `sample_us` / `push_us`: time in microseconds for spatial sampling (canvas to LED positions) and writing to devices. Elevated `push_us` across many devices points to USB saturation (step 3).

### `multi_zone_sampling`

```
multi_zone_sampling   gpu_sampled_zones=2, cpu_sampled_zones=0, held_zones=0, gpu_sampling_passes=3, zone_composition_skips=1
```

On the GPU compositor, a scene with more than one zone samples each zone's LEDs from that zone's own texture. This check counts the latest frame's zones by how they sampled:

- `held_zones`: zones with no frame to sample, whose LEDs hold their last colors or go black. Any held zone turns the check into a warning. A zone still loading its first frame holds briefly; a zone that stays held has a layer that never produces a frame the sampler can read.
- `cpu_sampled_zones`: zones whose frames were already on the CPU and sampled there.
- `gpu_sampling_passes`: texture passes the GPU sampler encoded this frame, one per zone texture plus one when unassigned outputs fall back to a zone. Zero means the sample cache answered.
- `zone_composition_skips`: zones whose layers did not change, so they reused the previous frame's composition.

A single-zone scene or the CPU compositor samples no per-zone set, so the check passes as not applicable.

### `recent_output_sources`

```
recent_output_sources   frames=18432, current_frame=18201, published_frame=140, routed_reuse=91, reused_published_frame=12, gpu_sample_stale=3
```

This check always reports `pass`; it is a counter readout, not a threshold. It totals which
frame each output push actually served over the session, so it tells you how often the loop
fell back on a cached frame rather than the frame it had just rendered. A `current_frame`
count close to `frames` is healthy. A large `published_frame` or `routed_reuse` share is the
same signal `led_freshness` reports per frame, accumulated over time.

---

## Step 3: check device output queues

The `devices` category includes two checks relevant to USB-bus and output latency.

### `output_queues`

```
output_queues   queues=6, usb_queues=4, lagging=1, worker_finished=0, dropped_total=0, errors_total=0
```

- `lagging`: queues where completed `delivered_fps` is significantly behind the queued cadence. Use the coalescing split below to distinguish an expected device cap from transport pressure.
- `worker_finished`: a worker thread exited unexpectedly. Any non-zero value is a hard failure and appears as `fail` status.
- `dropped_total`: legacy sum of coalesced frames across all device queues.

For the detailed per-device breakdown, use `-j` and inspect `snapshot.device_output.items`. Compare `delivered_fps` with `accepted_fps` and `fps_target`. Rising `coalesced_target_cadence` is expected pacing; rising `coalesced_backend_overrun`, transport latency, or failures identifies real pressure.

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

- `gpu_sample_stale` / `gpu_sample_deferred`: count of frames in the recent window where Servo's frame was not ready. A high ratio to `frames` means Servo is your bottleneck.
- `gpu_sample_cpu_fallback`: Servo fell back from GPU import to a CPU readback path. This adds a full-frame pixel readback cost and is significantly slower.
- `push_p95_ms` / `publish_p95_ms`: 95th-percentile device write and event-bus publish latency. Healthy values sit well under the frame budget for your current tier.

For Servo-specific timings, the system response (`GET /api/v1/system`) carries `status.effect_health`, with fields including `servo_render_frame_max_ms`, `servo_render_evaluate_scripts_max_ms`, `servo_render_paint_max_ms`, and `servo_render_readback_max_ms`. These are milliseconds, not microseconds. A `servo_render_frame_max_ms` value near or above the tier budget is a clear Servo bottleneck.

### Servo circuit breaker

Servo uses a circuit breaker to protect the daemon when the effect renderer fails repeatedly. After 3 consecutive failures it opens, blocking new render attempts for a 30-second cooldown with exponential backoff up to 5 minutes. The breaker state is visible in the Servo telemetry fields:

- `servo_breaker_opens_total` non-zero: Servo has tripped the breaker at least once this session.
- `servo_soft_stalls_total`: frames that stalled without a fatal failure.

When the breaker is open, effects fall back to a black/idle canvas, meaning your LEDs stop updating or hold the last frame. Switching to a native (non-HTML) effect clears the dependency on Servo immediately.

**Fixes for Servo overhead:**

- Switch to one of the native built-in effects if frame timing is critical; they run in compiled Rust with no browser overhead. Browse what is available in the [effects section](@/effects/_index.md).
- If you want to keep an HTML effect, reduce the effect's JavaScript complexity. Heavy `requestAnimationFrame` work, large canvas operations, and expensive audio FFT processing are common culprits. See [debugging and diagnostics](@/contributing/debugging.md) for Servo profiling techniques.
- If `servo_gpu_import_fallback_reason` appears in the JSON output, the GPU zero-copy import path is unavailable and Servo is doing full CPU readbacks every frame. Verify that your GPU drivers support the required Vulkan or OpenGL extensions for your platform.

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

At Full tier the budget is 16 666 µs. If `total_us` consistently exceeds the budget for a tier, the loop cannot hold that tier regardless of what it is targeting.

| Field | What it measures | High value means |
|-------|-----------------|-----------------|
| `render_us` | Effect renderer time (Servo or native) | Servo effect is slow; switch or optimize |
| `producer_us` | Canvas producer pipeline (includes Servo wait) | Effect frame is arriving late |
| `sample_us` | Spatial sampling (canvas to LED positions) | Many LEDs, complex layout, or slow GPU sample |
| `push_us` | Writing colors to device queues | USB saturation or slow device firmware |
| `composition_us` | SparkleFlinger compositor | Usually negligible; a spike indicates many active layers |

---

## Common scenarios

### Tier stuck below your configured target, no device errors

The render loop is consistently overrunning its budget. Inspect `total_us` in `latest_frame`. If `render_us` or `producer_us` dominates, the bottleneck is the effect renderer, most likely Servo. If `push_us` is high, it is device output. Address each at its root; do not lower the tier ceiling.

### `gpu_sample_stale` climbing in `recent_window`

Servo is not delivering frames fast enough to keep up with the render loop. Check `render_frame_max_us` and `render_evaluate_scripts_max_us` in the Servo telemetry to isolate whether JavaScript evaluation or paint is the slow phase.

### Devices showing `lagging=N` in output queues

N device queues are receiving frames faster than they can send them. This is USB bandwidth exhaustion. Use `-j` and check `avg_write_ms` per device: values significantly above `1000 / fps_target` indicate the device itself is slow to acknowledge writes.

### `worker_finished=N` in output queues

N device worker threads have exited. This is a hard error: those devices are no longer receiving output. Check `last_error` in the JSON. Common causes are USB disconnect, permission errors, or device firmware lockup. Try replugging the device. If errors persist, see [USB devices](@/hardware/usb-devices.md) for udev and hot-plug guidance.

### Servo breaker open

Check daemon logs for Servo-related errors, and check `servo_session_create_failures_total` in the system response. If the breaker is cycling (opens, cools down, opens again), the effect itself is broken, likely from a JavaScript runtime error. Switch effects to confirm, then file a bug with the `--report` output attached.

---

## Getting more detail

For a deeper look at the render architecture that produced these metrics, see [render pipeline](@/architecture/render-pipeline.md) and [renderer internals](@/architecture/renderer-internals.md).

If device output looks healthy but you still see stuttering at the hardware level, check [USB devices](@/hardware/usb-devices.md) for controller topology and permission guidance.

To attach a full diagnostic report to a bug report:

```bash
hypercolor diagnose --system --report ~/hypercolor-bug-report.json
```
