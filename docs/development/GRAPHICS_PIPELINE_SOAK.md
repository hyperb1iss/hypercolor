# Graphics Pipeline Soak

Use this runbook after graphics-pipeline changes that touch Servo rendering,
composition, display faces, display output, LED output, preview streaming, or
surface handoff behavior.

The soak observer watches an already-running daemon. It does not start,
restart, or mutate services.

## Commands

```bash
just graphics-soak --duration 60s
just graphics-soak-30
just graphics-soak-30 --daemon http://127.0.0.1:9420
```

Reports are written to `target/graphics-soak/latest.json` by the 30-minute
recipe unless `CARGO_TARGET_DIR` is set. Use
`--out ${CARGO_TARGET_DIR:-target}/graphics-soak/<scenario>.json` when
capturing several scenarios.

## Acceptance Scenarios

Run each scenario against an already-configured daemon:

- Servo LED effect plus two display faces.
- Screen-reactive scene plus display output.
- Multi-group transition scene.
- Mixed LCD plus LED device output on a shared USB transport.
- WebSocket preview subscribers at varied FPS and formats.

For each scenario, let the daemon warm up, then run:

```bash
just graphics-soak-30 --out "${CARGO_TARGET_DIR:-target}/graphics-soak/<scenario>.json"
```

## Passing Bar

A passing soak shows:

- Median actual FPS remains above the target FPS ratio.
- WebSocket backpressure uses latest-value drops, not unbounded buffering.
- Display write failures, retries, and output-error frames do not grow.
- Full-frame copy counters stay at zero after warmup.
- Render-surface pool saturation does not grow after warmup.
- Servo stalls, breaker opens, lifecycle failures, and pending render age do not
  grow after warmup.
- Display-lane priority wait stays within one LED frame interval.

## Report Fields

The JSON report and terminal summary call out these pressure lanes:

- `copy pressure`: producer, publication, and total full-frame copies.
- `surface pressure`: preview, forced scene-canvas, LED readback, and pool
  saturation counters.
- `servo qos`: render queue wait, pending render age, queue depth, and
  superseded render deltas.
- `servo lifecycle`: renderer load wait/failures and destroy wait.
- `display_output`: write failures, retry attempts, last failure age, and
  display-lane LED-priority wait.

Investigate any nonzero steady-state growth before calling a soak clean. Some
startup-time movement is expected during warmup; the acceptance window starts
after `--warmup`.
