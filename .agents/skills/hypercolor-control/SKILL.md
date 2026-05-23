---
name: hypercolor-control
version: 0.1.0
description: >-
  Use this skill when an agent needs to inspect or control a running
  Hypercolor daemon, browse or activate effects, patch live controls, adjust
  brightness, manage scenes or profiles, or install built HTML effects from an
  authoring workspace. Triggers on "hypercolor", "list effects", "apply
  effect", "patch controls", "install effect", "rescan effects", "brightness",
  "scene", "profile", or any request to control the daemon from Claude Code.
---

# Hypercolor Control

Hypercolor has two related CLIs:

- Bare `hypercolor` is the Rust system CLI for the daemon on `localhost:9420`.
- `bunx hypercolor` inside an effect workspace is the Bun authoring CLI for build, validate, dev, install, and scaffolding.

Always choose the right one before acting.

## Default workflow

Start with state discovery before changing anything:

```bash
hypercolor status
hypercolor effects list
hypercolor scenes active
```

When parsing output in automation, prefer JSON:

```bash
hypercolor status -j
hypercolor effects list -j
hypercolor library presets list -j
```

## Daemon control

Core runtime commands:

```bash
hypercolor status
hypercolor effects list --search aurora
hypercolor effects info "Aurora"
hypercolor effects activate "Aurora" --param speed=6 --param palette=\"Ocean\"
hypercolor effects patch --param speed=8
hypercolor effects reset
hypercolor effects stop
hypercolor effects rescan
hypercolor brightness get
hypercolor brightness set 45
hypercolor scenes list
hypercolor scenes activate "Movie Night"
hypercolor scenes deactivate
hypercolor profiles list
hypercolor profiles apply "Evening"
hypercolor diagnose --system
```

Use `hypercolor effects layout show|set|clear` when effect-to-layout links matter.

## Diagnostics workflow

When diagnosing a running daemon, query telemetry before asking for pasted logs:

```bash
just diagnose
just diagnose -- --json
hypercolor diagnose --system -j
curl -s http://127.0.0.1:9420/api/v1/devices/metrics
```

Use `just windows-diagnose` only when Windows service/PawnIO/SMBus environment state matters; the daemon render/output telemetry itself is cross-platform.

Read these fields first for LED jank:

- `snapshot.render.latest_frame.output_frame_source` — `current_frame`, `published_frame`, or `routed_reuse`
- `gpu_sample_stale`, `gpu_sample_deferred`, `gpu_sample_retry_hit`, `gpu_sample_queue_saturated`, `gpu_sample_wait_blocked`
- `sample_us`, `push_us`, `publish_us`, `devices_written`, `total_leds`
- `snapshot.device_output.items[]`: `backend_id`, `fps_sent`, `fps_queued`, `frames_dropped`, `avg_queue_wait_ms`, `avg_write_ms`, `last_error`
- `snapshot.usb`: USB actor display-lane wait counters

Interpretation:

- Smooth display previews with LED jank usually means LED sampling/output freshness, not effect rendering.
- `gpu_sample_stale=true` with `output_frame_source=published_frame` means LEDs reused older LED data while the visual path may still be smooth.
- `output_frame_source=current_frame` with `gpu_sample_retry_hit=true`, low sample/push times, and `wake_late` warnings usually means the app is ready but the OS woke the render thread late. On Windows, inspect active compiler/linker jobs before changing rendering or device code.
- `fps_queued` above `fps_sent`, rising `frames_dropped`, or high queue/write time points to device-output pressure.
- Drops on queues capped below render FPS are normal latest-frame replacement when `fps_sent` is near that queue's target and write latency/errors are clean.
- Multiple USB devices janking together points upstream or shared queue pressure; one device with errors points at that driver/transport.

## Effect authoring commands

Inside a Bun-authored workspace:

```bash
bunx hypercolor dev
bunx hypercolor build --all
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
bunx hypercolor add ember --template canvas
```

Common package scripts from scaffolded workspaces:

```bash
bun run dev
bun run build
bun run ship
bun run ship:daemon
```

`bun run ship` copies validated artifacts into the user effects directory.
`bun run ship:daemon` uploads through `POST /api/v1/effects/install`.

## Install workflow

Preferred install sequence for built HTML effects:

```bash
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
hypercolor effects rescan
hypercolor effects activate "Aurora"
```

If no daemon is running yet, local install still works:

```bash
bunx hypercolor install dist/aurora.html
```

That writes into `$XDG_DATA_HOME/hypercolor/effects/user/`, which the daemon
will pick up on boot or via `hypercolor effects rescan`.

## Behavioral guidance

- Inspect first. Do not guess the active effect, scene, or brightness.
- Prefer targeted actions over restarting the daemon.
- Validate HTML artifacts before installing them.
- Use JSON output when another tool or script needs to consume the result.
- After installing a new effect, confirm it appears in `hypercolor effects list` before applying it.
- If an effect patch fails, read the control definitions from `hypercolor effects info <name>` and retry with values that match the declared types.

## Example playbooks

### List and activate an effect

```bash
hypercolor effects list -j
hypercolor effects activate "Aurora" --param speed=7
hypercolor status
```

### Tweak a running effect

```bash
hypercolor effects info "Aurora"
hypercolor effects patch --param speed=4 --param brightness=80
hypercolor status
```

### Install a fresh artifact from an SDK workspace

```bash
cd /path/to/effect-workspace
bun run build
bunx hypercolor validate dist/aurora.html
bunx hypercolor install dist/aurora.html --daemon
hypercolor effects list --search aurora
```

### Recover after a manual file copy

```bash
cp dist/aurora.html ~/.local/share/hypercolor/effects/user/
hypercolor effects rescan
hypercolor effects info "Aurora"
```
