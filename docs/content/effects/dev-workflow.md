+++
title = "Dev Workflow"
description = "Studio, build, validate, install. The loop from code to LEDs"
weight = 2
template = "page.html"
+++

Every scaffolded workspace ships with a full authoring loop: a live preview studio, a build step that produces standalone HTML artifacts, a validator that catches metadata problems before they reach the daemon, and two install paths for getting the artifact onto running hardware.

## The loop at a glance

```bash
bun run dev              # start the studio at localhost:4200
bun run build            # compile every effect into dist/*.html
bun run validate         # check metadata and render surfaces
bun run ship             # install locally (filesystem copy)
bun run ship:daemon      # install through the daemon API
```

Each `bun run` script is a thin alias over the authoring CLI. The underlying commands work anywhere inside the workspace:

```bash
bunx hypercolor dev
bunx hypercolor build --all
bunx hypercolor validate dist/*.html
bunx hypercolor install dist/*.html
bunx hypercolor install dist/*.html --daemon
```

## The preview studio

```bash
bun run dev
```

The studio boots on `http://localhost:4200`. It gives you:

- an effect switcher for every effect in the workspace
- generated UI for every declared control, grouped as you grouped them
- preset buttons that snap all controls in one click
- audio simulation with bass, mid, treble sliders and a manual beat trigger
- canvas size presets for daemon defaults, strip, matrix, and ring layouts
- LED sampling preview overlaid on the canvas so you can see what the hardware will see

Every save triggers a rebuild. The studio swaps the running iframe and carries your current control values through the reload so you don't lose your place.

Override the port with `--port`:

```bash
bunx hypercolor dev --port 4321
```

Pass `--open` to auto-launch the studio in your default browser.

## Building artifacts

```bash
bun run build
```

This compiles every effect in `effects/` into a standalone HTML file under `dist/`. Each artifact has all JavaScript, shader source, palette tables, and metadata inlined into one file; no runtime loading, no CDN dependency.

Build a single effect:

```bash
bunx hypercolor build effects/aurora/main.ts
```

Rebuild on file changes:

```bash
bunx hypercolor build --all --watch
```

Minify for distribution:

```bash
bunx hypercolor build --all --minify
```

## Validating artifacts

```bash
bun run validate
```

The validator parses the built HTML and confirms:

- `<meta name="hypercolor-version">` is present
- the render surface exists (`<canvas id="exCanvas">` for canvas/shader, `<div id="faceContainer">` for face effects)
- there is at least one script tag
- metadata is well-formed (controls parse, presets have valid JSON)

Validation runs automatically during `ship:daemon` installs, but running it directly is useful when you're iterating on raw HTML or porting a foreign effect.

## Installing to the daemon

There are two install paths, and they have different tradeoffs.

### Local install

```bash
bun run ship
```

This copies validated artifacts into the user effects directory at `$XDG_DATA_HOME/hypercolor/effects/user/` (falls back to `~/.local/share/hypercolor/effects/user/`). The daemon picks them up on startup, or live via:

```bash
hypercolor effects rescan
```

Use this when the daemon isn't running yet, when you're iterating fast, or when you want the artifact on disk without a network round trip.

### Daemon install

```bash
bun run ship:daemon
```

This uploads the artifact through `POST /api/v1/effects/install` on a running daemon. The daemon validates, deduplicates against existing user effects, and registers the new effect in the catalog immediately. No rescan needed.

Override the daemon URL if your daemon isn't on `localhost:9420`:

```bash
bunx hypercolor install dist/aurora.html --daemon --daemon-url http://some-host:9420
```

Use this when the daemon is already running and you want the effect live without restarting anything.

## Adding more effects to a workspace

One workspace can hold as many effects as you like. Add another one without starting over:

```bash
bunx hypercolor add ember --template canvas
bunx hypercolor add skyline --template shader --audio
bunx hypercolor add flicker --template html
```

Each new effect lands in `effects/<id>/` (or `effects/<id>.html` for HTML) without touching your existing effects. The studio picks up the new entry on its next rebuild.

## The `hypercolor` daemon CLI

Don't confuse the authoring CLI (`bunx hypercolor`, inside a workspace) with the system CLI (`hypercolor`, installed alongside the daemon). The system CLI talks to the running daemon to list effects, activate them, patch live controls, manage scenes, and rescan.

Typical end-to-end loop once your daemon is running:

```bash
cd my-effects
bun run build
bun run ship:daemon
hypercolor effects list --search aurora
hypercolor effects activate "Aurora"
hypercolor effects patch --param speed=8
```

See the [CLI reference](@/api/cli.md) for the full system CLI surface.

## Inside the Hypercolor monorepo

If you're working inside a `hypercolor/` clone, the top-level `just` recipes wrap the same authoring core:

```bash
just sdk-dev                 # live-rebuild the SDK itself
just effects-build           # build every built-in effect into effects/hypercolor/
just effect-build aurora     # build one by id
```

Everything else in this guide works identically inside and outside the monorepo.
