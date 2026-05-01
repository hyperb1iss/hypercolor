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
recipe. Use `--out target/graphics-soak/<scenario>.json` when capturing several
scenarios.

## Acceptance Scenarios

Run each scenario against an already-configured daemon:

- Servo LED effect plus two display faces.
- Screen-reactive scene plus display output.
- Multi-group transition scene.
- Mixed LCD plus LED device output on a shared USB transport.
- WebSocket preview subscribers at varied FPS and formats.

For each scenario, let the daemon warm up, then run:

```bash
just graphics-soak-30 --out target/graphics-soak/<scenario>.json
```

## Passing Bar

A passing soak shows:

- Median actual FPS remains above the target FPS ratio.
- WebSocket backpressure uses latest-value drops, not unbounded buffering.
- Display write failures, retries, and output-error frames do not grow.
- Full-frame copy counters stay at zero after warmup.
- Render-surface pool saturation does not grow after warmup.
- Servo stalls, breaker opens, and lifecycle failures do not grow.
- Display-lane priority wait stays within one LED frame interval.
