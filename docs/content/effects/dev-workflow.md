+++
title = "Dev workflow"
description = "The authoring loop from code to LEDs: build standalone HTML, validate it, ship it to the running daemon, preview on real hardware."
weight = 120
template = "page.html"
+++

Edit, build, ship, see light. That is the whole loop. Every scaffolded workspace turns your effect source into a self-contained HTML artifact, validates it before it reaches the engine, installs it into the real runtime, and lets you preview the result on actual LEDs. There is no fake browser shell in the middle, so what you preview is what ships.

## The loop at a glance

```bash
bun run build            # compile every effect → dist/*.html
bun run validate         # check render surface, metadata, scripts
bun run ship             # install locally (filesystem copy)
bun run ship:daemon      # install through the running daemon API
```

Each `bun run` script is a thin alias over the authoring CLI that ships inside `hypercolor`. The underlying commands work anywhere in the workspace:

```bash
bunx hypercolor build --all
bunx hypercolor validate dist/*.html
bunx hypercolor install dist/*.html
bunx hypercolor install dist/*.html --daemon
```

{% callout(type="info") %}
This is the **authoring** CLI: `bunx hypercolor`, run inside a workspace, resolved through the `hypercolor` dependency. It builds and installs artifacts. Do not confuse it with the **system** CLI (`hypercolor`, installed alongside the daemon) that talks to the running daemon to list, activate, and patch effects. Both are covered below.
{% end %}

## The real iteration loop

The old synthetic `hypercolor dev` preview server is gone. Run it now and it prints an error and exits non-zero, pointing you at `build` + `ship:daemon`. The source of truth is the daemon and the desktop app runtime, so the recommended loop is:

1. Edit the effect source.
2. `bun run build`.
3. `bun run ship:daemon` if a daemon is already running, or `bun run ship` plus `hypercolor effects rescan` for a plain filesystem install.
4. Preview the result in the app and on real hardware.

That loop is a little slower than a browser shell, but it exercises the behavior that actually matters: audio wiring, metadata parsing, HTML loading, Servo rendering, and effect registration. A fake preview hides exactly the parts that break.

## Building artifacts

```bash
bun run build
```

This compiles every effect under your workspace `effects/` directory into a standalone HTML file in `dist/`. Each artifact inlines all of its JavaScript (bundled as an IIFE), shader source (loaded as text via the `.glsl` import), palette tables, and metadata into a single file. Display-face font controls can load selected Google Fonts at runtime unless capture mode disables remote fonts.

Build a single effect by passing its entry path:

```bash
bunx hypercolor build effects/aurora/main.ts
```

Rebuild on every save:

```bash
bunx hypercolor build --all --watch
```

Minify for distribution:

```bash
bunx hypercolor build --all --minify
```

The full flag set on `build`: `--all` (auto-discover `effects/<id>/main.ts`), `--watch`, `--minify`, `--out <dir>` (default `dist`), `--entry-root <dir>` (repeatable, default `effects`), `--workspace-root <dir>`, and `--sdk-alias-path <path>`.

{% callout(type="tip") %}
A successful build prints one line per artifact, for example `✓ aurora → dist/aurora.html (38.2 KB)`. Faces print a 💎 instead of a checkmark. If nothing prints, the build found no entrypoints — check that your effect lives at `effects/<id>/main.ts` or that you passed an explicit path.
{% end %}

## Validating artifacts

```bash
bun run validate
```

The validator parses the built HTML and confirms the artifact will actually load in the engine. It checks that a render surface exists (`<canvas id="exCanvas">` for canvas and shader effects, or a `<div id="faceContainer">` for faces), that the document carries a title and at least one `<script>`, and that the metadata is well-formed.

A passing run reports per file:

```text
dist/aurora.html

PASS  Render surface + title + script

Result: PASS
```

A failure swaps in `FAIL` lines with the specific problem, and any non-fatal issues surface as `WARN`. Two flags shape the exit code: `--strict` treats warnings as failures, and `--json` emits the structured result instead of the human report. Validation also runs automatically inside both install paths, so a broken artifact never lands in the catalog. Running it directly is most useful when you are iterating on raw HTML or porting a foreign effect.

For the catalog of build-time errors the metadata extractor can throw (missing `audio: true`, shader uniform mismatch, "no effect definitions were registered"), see the effect-build troubleshooting notes in this section.

## Installing to the daemon

There are two install paths with different tradeoffs. Both validate the artifact first and reject anything that fails.

### Local install

```bash
bun run ship
```

This copies validated artifacts into the user effects directory at `$XDG_DATA_HOME/hypercolor/effects/user/`, falling back to `~/.local/share/hypercolor/effects/user/` when `XDG_DATA_HOME` is unset. If a file with the same name already exists, the installer dedupes by appending a numeric suffix rather than overwriting. The daemon picks the new artifact up on its next startup, or live with:

```bash
hypercolor effects rescan
```

Reach for the local path when the daemon is not running yet, when you are iterating fast, or when you want the artifact on disk without a network round trip.

### Daemon install

```bash
bun run ship:daemon
```

This uploads the artifact through `POST /api/v1/effects/install` (a multipart form upload) to a running daemon. The daemon validates, deduplicates against existing user effects, and registers the new effect in the catalog immediately. No rescan needed. The success line reports the installed name and control count, for example `✓ dist/aurora.html → .../aurora.html (Aurora, 4 controls)`.

The default daemon URL is `http://127.0.0.1:9420`. Override it with the `--daemon-url` flag or the `HYPERCOLOR_DAEMON_URL` environment variable when your daemon lives elsewhere:

```bash
bunx hypercolor install dist/aurora.html --daemon --daemon-url http://some-host:9420
```

Reach for the daemon path when the daemon is already running and you want the effect live without restarting anything.

{% mermaid() %}
graph LR
  A[edit source] --> B[bun run build]
  B --> C{daemon running?}
  C -->|yes| D[bun run ship:daemon]
  C -->|no| E[bun run ship]
  E --> F[hypercolor effects rescan]
  D --> G[preview on hardware]
  F --> G
{% end %}

## The face dev loop

Display faces (full-screen HTML for LCD pump caps, Push 2 strips, and similar) get their own zero-friction loop that handles simulators for you:

```bash
just face-dev system-pulse
```

This builds the named face, installs it into the running daemon, ensures the two canonical simulator displays exist (a 480×480 round panel and a 960×160 strip), assigns the face to both, opens the Displays page, then rebuilds and reinstalls on every save. The target is save-to-preview in under five seconds, with no physical display attached. It expects a daemon on `http://127.0.0.1:9420` (override with `HYPERCOLOR_URL`); start one with `just daemon` if nothing is reachable.

Because a face ships only when it is intentional on both a round panel and a wide strip, the dual-simulator setup is the quality gate, not just a convenience. See the display-faces authoring guide in this section for the `face()` contract, the Servo CSS matrix, and the data sources a face can read.

## Adding more effects to a workspace

One workspace holds as many effects as you like. Add another without starting over:

```bash
bunx hypercolor add ember --template canvas
bunx hypercolor add skyline --template shader --audio
bunx hypercolor add aurora-face --template face
bunx hypercolor add flicker --template html
```

The four templates are `canvas`, `shader`, `face`, and `html`. Pass `--audio` to scaffold an audio-reactive starter. Each new effect lands in `effects/<id>/` (or `effects/<id>.html` for the raw HTML template) without touching your existing effects. Run `add` with no arguments to get an interactive prompt instead.

## The system CLI: list, activate, patch

Once an effect is installed, the **system** CLI drives the running daemon to make it live. This is the `hypercolor` binary that ships with the daemon, not the `bunx hypercolor` authoring CLI.

A typical end-to-end loop once your daemon is running:

```bash
cd my-effects
bun run build
bun run ship:daemon
hypercolor effects list --search aurora
hypercolor effects activate aurora
hypercolor effects patch --param speed=7
```

{% callout(type="warning") %}
Live control values use the `--param name=value` form (for example `--param speed=7`), not bare `--speed` flags. The control name is the lowercased label you declared in the effect. Check the [CLI reference](@/api/cli.md) for the authoritative flag set.
{% end %}

The system CLI also splits three distinct top-level commands that are easy to confuse: `server` configures and talks to the daemon as an HTTP server, `servers` manages multiple known daemon connections, and `service` controls the OS-level background service (install, start, stop). They are not interchangeable. The [CLI reference](@/api/cli.md) covers the full surface, and the daemon's REST contract is enumerated in the [REST API reference](@/api/rest.md).

## Driving the loop from an agent

If you are building effects with an AI agent, the same loop composes over the daemon's control surfaces. The build-and-ship half lives in the authoring CLI above; the apply-and-verify half can run through either the system CLI (with JSON output for parsing) or the MCP server. The catalog discovery, effect application, and live-control patching an agent needs are all exposed there. See the agent CLI-scripting and MCP guides in the Agents section for the worked playbooks, including the cross from "build an effect" (authoring CLI) to "apply it" (MCP or system CLI).

## Inside the Hypercolor monorepo

If you are working inside a `hypercolor/` clone rather than a standalone workspace, the top-level `just` recipes wrap the same authoring core against the in-repo effect sources under `sdk/src/`:

```bash
just sdk-dev                 # live-rebuild the SDK package itself (HMR)
just effects-build           # build every SDK effect → effects/hypercolor/*.html
just effect-build borealis   # build one effect by id
just faces-build             # build every SDK face → effects/hypercolor/*.html
just face-build silkcircuit-hud   # build one face by id
just face-dev system-pulse   # the face authoring loop described above
```

{% callout(type="danger") %}
`effects/hypercolor/` is generated, gitignored build output. Never hand-edit it and never commit it. The source lives in `sdk/src/effects/` and `sdk/src/faces/`; regenerate with the recipes above.
{% end %}

Everything else in this guide works identically inside and outside the monorepo, because both routes call the same `build` / `validate` / `install` core.

{% callout(type="info") %}
The SDK is pre-release and not published to npm. Scaffolded workspaces point at a local checkout through a `file:` spec (set via `--sdk-spec` or `HYPERCOLOR_SDK_PACKAGE_SPEC`); Bun's `link:` is not a drop-in substitute. Those `file:` instructions will change once the package publishes. See the setup guide in this section for the current pinning rules.
{% end %}
