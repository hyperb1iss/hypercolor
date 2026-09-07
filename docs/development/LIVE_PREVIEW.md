# Live Preview Workflow

The live preview surface is the web UI. Run the daemon and the UI dev server
together and open the UI:

```bash
just dev
```

That serves the UI on `http://127.0.0.1:9430` and proxies its API calls to the
daemon on `:9420`. The dashboard's canvas preview shows the composed render
canvas; `/preview?display=<device-id>` opens one display's full-screen face.

The daemon used to serve its own standalone preview page at
`http://127.0.0.1:9420/preview`. Spec 76 wave 3.2c deleted it: the UI renders
the same surfaces through the real preview pipeline, and the daemon's route
shadowed the UI's own `/preview` route whenever the daemon served the UI.

## HTML Effects (Servo)

LightScript-compatible HTML effects require the Servo renderer feature:

```bash
./scripts/run-preview-servo.sh
```

This uses `./scripts/servo-cache-build.sh` under the hood so Servo/mozjs
artifacts are reused across runs. Use this script when you want a Servo daemon
with its hard-coded test environment; use `just daemon-servo` when you need the
full general-purpose Servo daemon (the recipe is also what
`SERVO_BUILD_CACHING.md` documents for CI and service-mode use).

If you need a custom bind address:

```bash
./scripts/run-preview-servo.sh --bind 0.0.0.0:9420
```

## Notes

- The effect library defaults to runnable effects only; the filter in the UI
  reveals the rest.
- Without the Servo feature the daemon logs that HTML rendering is unavailable
  and HTML effects stay non-runnable.
