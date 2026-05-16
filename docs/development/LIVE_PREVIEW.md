# Live Preview Workflow

## Quick Start (Native Effects)

```bash
cargo run -p hypercolor-daemon -- --bind 127.0.0.1:9420
```

Open:

- `http://127.0.0.1:9420/preview`

This path is fast to compile and includes native effects (`color_wave`,
`rainbow`, `gradient`, etc.).

## HTML Effects (Servo)

LightScript-compatible HTML effects require the Servo renderer feature:

```bash
./scripts/run-preview-servo.sh
```

This uses `./scripts/servo-cache-build.sh` under the hood so Servo/mozjs
artifacts are reused across runs. Use this script when you want the preview
surface with its hard-coded test environment variables; use `just daemon-servo`
when you need the full general-purpose Servo daemon (the recipe is also what
`SERVO_BUILD_CACHING.md` documents for CI and service-mode use).

If you need a custom bind address:

```bash
./scripts/run-preview-servo.sh --bind 0.0.0.0:9420
```

## Preview Page Notes

- The effect list defaults to runnable effects only.
- If Servo is not enabled, the page shows a warning and the command needed to
  enable HTML rendering.
- Toggle `show unavailable` to inspect non-runnable effects in the dropdown.
