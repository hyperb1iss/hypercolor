+++
title = "Debugging & diagnostics"
description = "RUST_LOG targets, trace spans, diagnose command, Servo troubleshooting, and display-face simulator debugging."
weight = 10
template = "page.html"
+++

When things go wrong — devices not responding, effects rendering incorrectly, audio not reacting,
Servo hanging — here is how to find and fix the problem.

## Daemon logs

The daemon uses `tracing` with a themed console formatter. Control verbosity with `RUST_LOG` or
the `--log-level` flag. The default is `info`, which logs device connections, effect changes, and
errors without flooding the terminal.

```bash
# Debug logging (recommended starting point)
RUST_LOG=debug just daemon

# Maximum verbosity: every frame, every USB packet
RUST_LOG=trace just daemon

# Specific module, rest at info
RUST_LOG=hypercolor_hal=debug,hypercolor_core::engine=trace just daemon

# Warnings and errors only
RUST_LOG=warn just daemon
```

The app shell (`hypercolor-app`) writes a separate rolling log file to the data directory.
Its default filter is `hypercolor_app=debug,tauri=info,wry=warn`. Override it the same way
with `RUST_LOG`.

### Key log targets

| Target | What it shows |
|---|---|
| `hypercolor_daemon` | API requests, WebSocket connections, startup sequence |
| `hypercolor_core::engine` | Render loop timing, FPS tier shifts, frame drops |
| `hypercolor_core::effect` | Effect loading, control updates, renderer state |
| `hypercolor_core::effect::servo` | Servo worker lifecycle, session create/destroy, circuit breaker |
| `hypercolor_core::input` | Audio capture, FFT processing, beat detection |
| `hypercolor_hal` | Device discovery, USB communication, protocol encoding |
| `hypercolor_hal::drivers::razer` | Razer-specific USB packet detail |
| `hypercolor_hal::drivers::prismrgb` | PrismRGB chunked protocol detail |

## Built-in diagnostics

`hypercolor diagnose` posts to `POST /api/v1/diagnose` and runs a set of named health checks
against the live daemon. The default check set is `daemon`, `render`, `devices`, `config`.

```bash
# Tabular output (the default, easiest to read)
hypercolor diagnose

# Run specific checks only (repeatable)
hypercolor diagnose --check render --check devices

# Add verbose system information (GPU, kernel, audio version, uptime)
hypercolor diagnose --system

# Write a full report file for bug reports
hypercolor diagnose --report /tmp/hypercolor-report.json

# Raw JSON via REST
curl -s -X POST http://localhost:9420/api/v1/diagnose | jq
```

Each check produces a `pass`, `warning`, or `fail` status with a detail string. The summary
line at the end counts totals across all checks.

### Diagnose checks reference

| Check | Category | What it tests |
|---|---|---|
| `daemon` | system | Daemon is running and returns its version |
| `render` | render | Render loop state, frame liveness (stale > 2 s → warning, > 10 s → fail), LED freshness |
| `devices` | devices | Registry count, output queue health, USB actor display lane, display output encoder |
| `config` | config | Config manager availability |

The response also includes a `snapshot` object with detailed render timing, USB actor metrics,
display output encoder counters, and per-device queue state. Inspect it with:

```bash
curl -s -X POST http://localhost:9420/api/v1/diagnose | jq '.data.snapshot'
```

The per-device items under `snapshot.device_output.items` include accepted and delivered FPS,
target-cadence versus backend-overrun coalescing, actual transport latency, terminal counters,
and `last_error`. That split distinguishes healthy latest-wins pacing from a lagging device.

### Servo memory diagnostics

On non-Windows builds with the `servo` feature enabled, a separate endpoint captures Servo's
internal memory profiler output:

```bash
curl -s -X POST http://localhost:9420/api/v1/diagnose/memory | jq
```

This returns a `ServoMemoryReportSnapshot` with per-process explicit heap, system heap, non-heap,
and non-explicit bytes. Useful for tracking down long-session Servo memory growth.

{% callout(type="warning") %}
Servo memory diagnostics are disabled on Windows because the embedded memory reporter can abort
the daemon process. The endpoint returns `404` on that platform. On Linux/macOS builds without
the `servo` feature the endpoint also returns `404`.
{% end %}

## REST and WebSocket inspection

```bash
# Full system status (includes audio_available and effect_health)
curl -s http://localhost:9420/api/v1/status | jq

# Connected devices and their current state
curl -s http://localhost:9420/api/v1/devices | jq

# Active effect
curl -s http://localhost:9420/api/v1/effects/active | jq

# Real-time event stream (install websocat if needed: cargo install websocat)
websocat ws://localhost:9420/api/v1/ws | jq
```

The WebSocket stream shows effect changes, device events, control updates, and frame metrics as
they happen. It is the fastest way to understand timing and sequencing. See the
[WebSocket reference](@/api/websocket.md) for channel details and binary frame layout.

## USB debugging

### Permission issues

If devices are discovered but fail to connect, it is almost always a permissions problem:

```bash
# Check if udev rules are installed
ls -la /etc/udev/rules.d/99-hypercolor.rules

# Install them (requires sudo)
just udev-install

# Verify device file permissions
ls -la /dev/hidraw*

# See recent kernel messages for HID/USB events
sudo dmesg | grep -i "hid\|usb" | tail -20
```

The rules grant access to the physically logged-in user via systemd-logind (`TAG+="uaccess"`),
with `GROUP="users"` as a fallback. After installing them, re-plug the device or reboot so
existing device nodes pick up the new ACLs.

### Device not discovered

```bash
# Check if the device is visible to the system
lsusb

# Get detailed USB descriptor info for a specific device
lsusb -v -d VENDOR:PRODUCT 2>/dev/null

# List HID device files
ls /dev/hidraw*

# Watch udev events in real time, then plug in the device
sudo udevadm monitor --property
```

If `lsusb` shows the device but it does not appear in Hypercolor, run the daemon at `debug`
level and look for discovery output from `hypercolor_hal`. The vendor/product ID may not be in
the device database yet.

## Audio debugging

### Effects not reacting to audio

1. Check that audio capture is working at the system level:

   ```bash
   # List PulseAudio/PipeWire sources
   pactl list sources short

   # Confirm a monitor source exists (what you hear through speakers)
   pactl list sources short | grep monitor
   ```

2. Check the daemon audio state:

   ```bash
   curl -s http://localhost:9420/api/v1/status | jq '.data.audio_available'
   ```

3. Enable audio input tracing to see what the daemon receives:

   ```bash
   RUST_LOG=hypercolor_core::input=debug just daemon
   ```

{% callout(type="tip") %}
For audio-reactive effects, you want a "monitor" source that captures your system audio output.
On PulseAudio and PipeWire these are named like `alsa_output.*.monitor`.
{% end %}

## Effect debugging

### Effect not loading

```bash
# Check daemon logs for loading errors
RUST_LOG=hypercolor_core::effect=debug just daemon

# List loaded effects after startup
curl -s http://localhost:9420/api/v1/effects | jq '.data.items[].name'
```

### Effect rendering issues

Run with effect engine and render loop tracing:

```bash
RUST_LOG=hypercolor_core::effect=trace,hypercolor_core::engine=debug just daemon
```

This surfaces which renderer is active, frame timing, control value injection, and audio
delivery to the effect.

## Servo troubleshooting

HTML effects — TypeScript SDK canvas effects, GLSL shaders (via WebGL2), raw HTML, and display
faces — all render through the Servo worker. Servo has its own failure modes.

### Start the Servo-enabled daemon

The standard `just daemon` command does not include Servo. To test HTML effects you need:

```bash
just daemon-servo
```

### Servo-specific log targets

```bash
RUST_LOG=hypercolor_core::effect::servo=debug just daemon-servo
```

Key sub-targets:

| Target | What it shows |
|---|---|
| `hypercolor_core::effect::servo` | Session lifecycle, page loads, render requests, circuit breaker |
| `hypercolor_core::effect::servo::worker` | Worker thread spawn/teardown, command channel health |
| `hypercolor_core::effect::servo::renderer` | `EffectRenderer` facade, render submission timing |

### Circuit breaker

The Servo worker has a circuit breaker that opens after 3 consecutive soft failures and applies
exponential backoff starting at 30 seconds, capped at 5 minutes. If effects suddenly stop
rendering after a crash or hang, the breaker may be open.

Signs the breaker has opened:

- Effect applies without error but LEDs stop updating.
- `hypercolor_core::effect::servo` logs show repeated worker acquisition failures.
- `servo_breaker_opens_total` increments in the `effect_health` field of the status endpoint.

The breaker resets automatically after the cooldown. Restarting the daemon also clears it.
Servo telemetry lives under `effect_health` on the status endpoint — not in the diagnose
snapshot:

```bash
curl -s http://localhost:9420/api/v1/status | jq '.data.effect_health'
```

### Servo session failures

The Servo worker manages sessions in an `Idle → Loading → Running` state machine. Session
creation and page load failures are counted under `effect_health`:

```bash
curl -s http://localhost:9420/api/v1/status | jq '
  .data.effect_health |
  {
    session_creates: .servo_session_creates_total,
    session_create_failures: .servo_session_create_failures_total,
    page_loads: .servo_page_loads_total,
    page_load_failures: .servo_page_load_failures_total,
    breaker_opens: .servo_breaker_opens_total
  }
'
```

For full Servo telemetry (render queue waits, GPU import stats, per-frame timing), inspect every
`servo_*` field under `effect_health`:

```bash
curl -s http://localhost:9420/api/v1/status | jq '.data.effect_health | with_entries(select(.key | startswith("servo_")))'
```

### Servo CSS and layout constraints

{% callout(type="warning") %}
CSS grid does not work in Servo: children render stacked full-width instead of in a grid. Use
flexbox for display-face layouts. This is a known Servo limitation, not a Hypercolor bug.
{% end %}

WebGL2 works via the Servo canvas but not all extensions available in Chrome are present. If a
GLSL effect works in a browser but not in Hypercolor, check for extension dependencies.

{% callout(type="info") %}
There is no native GPU/wgpu shader lane. `EffectSource::Shader` is reserved for future work and
is not currently executed. GLSL effects run as WebGL2 inside Servo. Do not expect a native
compiled shader path to be available.
{% end %}

### Servo OOM or crash

If the daemon process exits or becomes unresponsive while running HTML effects:

1. Run `just daemon-servo` with `RUST_LOG=hypercolor_core::effect::servo=trace` to capture the
   last log lines before exit.

2. Check Servo memory before and after loading the problematic effect:

   ```bash
   curl -s -X POST http://localhost:9420/api/v1/diagnose/memory | jq '.data.totals'
   ```

3. If explicit heap grows unboundedly across effect restarts, the effect may be holding DOM
   references across render frames. Move all allocations inside the draw callback.

## Display-face simulator debugging

Display faces render to virtual LCD displays. When debugging face effects, use the virtual
simulator instead of requiring physical hardware.

### Create a simulator via REST

```bash
# Create a rectangular simulator (e.g., Corsair LCD module size)
curl -s -X POST http://localhost:9420/api/v1/simulators/displays \
  -H 'Content-Type: application/json' \
  -d '{"name":"test-lcd","width":480,"height":270,"circular":false}' | jq

# Create a circular simulator (e.g., AIO cooler LCD)
curl -s -X POST http://localhost:9420/api/v1/simulators/displays \
  -H 'Content-Type: application/json' \
  -d '{"name":"round-lcd","width":240,"height":240,"circular":true}' | jq
```

The SDK's `just face-dev NAME` command builds and installs the face, creates a round and a strip
simulator display, assigns the face to both, and rebuilds on save — it is the recommended
development loop for display faces.

### Inspect simulator output

The daemon serves a JPEG frame per simulated display. List active simulators first:

```bash
curl -s http://localhost:9420/api/v1/simulators/displays | jq
```

Then fetch the current frame:

```bash
curl -s http://localhost:9420/api/v1/simulators/displays/{id}/frame -o /tmp/face-preview.jpg
```

The display preview WebSocket channel (binary tag `0x07`) streams JPEG frames to subscribers.
Those payloads are binary JPEG, not JSON, so you subscribe with a config message rather than
piping through `jq`. See the [WebSocket reference](@/api/websocket.md) for the `display_preview`
channel configuration and frame layout.

### Face dev log targets

```bash
RUST_LOG=hypercolor_core::effect::servo=debug,hypercolor_daemon::render_thread::display_lane=debug just daemon-servo
```

The `ServoProducerRole::DisplayFaceHtml` variant is tracked separately from scene HTML rendering
in telemetry. Look for `servo_render_display_requests_total` versus
`servo_render_scene_requests_total` under `effect_health` on the status endpoint to confirm
the face is actually submitting render requests.

### Two-geometry face dev

`just face-dev NAME` spins up two simulators at once — a 480x480 round display and a 960x160
strip — so you can watch a face render on both a circular and a rectangular surface side by
side. Validate the built artifact before installing with `bun run validate`, which checks
metadata and render surfaces.

If a face looks wrong on the round display:

1. Check `ctx.display.circular` in your draw function. Round displays clip to a circle, so
   honor the `safeArea` inset (339x339 centered on a 480x480 round LCD) for content that must
   stay visible.
2. Use `clip-path: circle()` or the SDK's container mask rather than CSS grid, which Servo does
   not support.

## Common issues quick reference

**"Permission denied" on USB devices** — install udev rules with `just udev-install`, then
re-plug the device or reboot so existing nodes pick up the new ACLs. The rules use `uaccess`
for the logged-in user with `GROUP="users"` as a fallback.

**Daemon starts but no devices connect** — run `lsusb` to confirm the device is visible. If
visible, the VID/PID may not be in the device database. Run at `debug` level and look for
discovery output from `hypercolor_hal`.

**Effects apply but LEDs show wrong colors** — likely a spatial layout mismatch. Verify device
zones are positioned correctly. Use `hypercolor devices identify <id>` to confirm the device is
receiving data.

**Low FPS in the render loop** — enable `RUST_LOG=hypercolor_core::engine=trace` and look for
frame time spikes. Common causes: slow USB writes (check `avg_write_ms` per device in the
diagnose snapshot), Servo rendering overhead (check `servo_render_frame_max_ms` under
`effect_health` on the status endpoint), or too many devices on one USB controller.

**WebSocket connection drops** — the daemon pings clients periodically and drops non-responding
ones. Verify your client handles WebSocket ping/pong. Also check for reverse proxy or firewall
idle-connection timeouts.

**Servo circuit breaker open, effects frozen** — wait for the cooldown to expire (30 s minimum,
up to 5 min with repeated failures) or restart the daemon. If it reopens immediately, set
`RUST_LOG=hypercolor_core::effect::servo=trace` to capture the failure reason before the
breaker trips again.

**Servo memory diagnostics return 404** — this endpoint is disabled on Windows. On Linux/macOS
with the `servo` feature it should always be available; without the feature it also returns
`404`.

## Related pages

- [Architecture: render pipeline](@/architecture/render-pipeline.md)
- [Effects: display faces](@/effects/display-faces.md)
- [Troubleshooting: common issues](@/troubleshooting/common-issues.md)
- [API: REST reference](@/api/rest.md)
- [API: WebSocket reference](@/api/websocket.md)
