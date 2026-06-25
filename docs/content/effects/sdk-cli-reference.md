+++
title = "SDK CLI reference"
description = "The bunx hypercolor authoring CLI: build, validate, install, add. Flags, exit codes, and env vars for the effect workspace toolchain."
weight = 140
template = "page.html"
+++

The authoring CLI ships inside `@hypercolor/sdk` and runs from a scaffolded effect workspace. It compiles your TypeScript and GLSL into self-contained HTML artifacts, validates them, and installs them so the daemon can pick them up. This page is the exhaustive reference for every command, flag, exit code, and environment variable.

{% callout(type="warning") %}
This is the **authoring** CLI — `hypercolor` resolved from your effect workspace, used to build and ship effects. It is a different binary from the **system** CLI (`hypercolor`, installed alongside the daemon) that talks to the running daemon to list devices, apply effects, and manage scenes. The system CLI lives in [its own reference](@/api/cli.md). When this page says `bunx hypercolor build`, it means the workspace tool, never the daemon client.
{% end %}

## Invocation

Inside a scaffolded workspace the CLI is wired three ways, all equivalent:

```bash
# Through the package script (the scaffold defines these)
bun run build

# Through bunx, resolving the @hypercolor/sdk bin
bunx hypercolor build

# Directly against the source entrypoint (what build:effects uses in-repo)
bun run packages/core/src/cli.ts build
```

The bin name is `hypercolor` and resolves through the workspace's `@hypercolor/sdk` dependency. The companion scaffolder is a separate package, `@hypercolor/create-effect`, invoked as `create-hypercolor-effect` — covered in its own section below.

Run with no command, `--help`, or `help` to print usage:

```bash
bunx hypercolor --help
```

```
hypercolor <command>

Commands:
  dev        Deprecated. Use build/install against the real daemon preview
  build      Build effect entrypoints into HTML artifacts
  validate   Validate built HTML artifacts
  install    Install HTML artifacts into the user effects directory
  add        Scaffold a new effect inside the workspace
```

## Command map

{% mermaid() %}
graph TD
    ADD["add — scaffold an effect"] --> BUILD["build — compile to HTML"]
    BUILD --> VALIDATE["validate — check artifacts"]
    VALIDATE --> INSTALL["install — ship to the daemon or user dir"]
{% end %}

The everyday loop is `add` to create an effect, `build` to compile it, `validate` to confirm the artifact is well-formed, then `install` to deliver it. The deprecated `dev` command is documented at the end so you know why it no longer works.

## `build`

Compile effect entrypoints into self-contained HTML artifacts. Each `effects/<id>/main.ts` (or `faces/<id>/main.ts`) becomes one HTML file in the output directory, with the JS bundle, inlined shader source, palette tables, and `<meta>` control metadata all baked in. No runtime or CDN loading.

```bash
# Build everything the workspace discovers
bunx hypercolor build --all

# Build one effect by entrypoint path
bunx hypercolor build effects/my-effect/main.ts
```

With no positional path and no `--all`, the CLI builds all discovered entrypoints anyway — an empty positional list implies `--all`.

### Flags

| Flag | Argument | Default | Behavior |
|---|---|---|---|
| `--all` | — | implied when no path given | Discover and build every entrypoint under each entry root. |
| `--out` | `<dir>` | `dist` | Output directory for HTML artifacts, resolved against the cwd. |
| `--entry-root` | `<dir>` | `effects` | Root to scan for `<id>/main.ts`. Repeatable — pass once per root. |
| `--workspace-root` | `<dir>` | `.` | Root that `--all` discovery walks from. |
| `--sdk-alias-path` | `<file>` | (none) | Alias the `@hypercolor/sdk` import to a source file. Used in-repo to point at `packages/core/src/index.ts`. |
| `--minify` | — | off | Minify the bundled JS. |
| `--watch` | — | off | Rebuild on `.ts` / `.glsl` changes. `Ctrl-C` (SIGINT) stops the watchers. |

`--entry-root` is repeatable, which is how the in-repo `build:effects` script builds effects and faces in one pass:

```bash
bun run hypercolor build --all \
  --workspace-root . \
  --entry-root src/effects \
  --entry-root src/faces \
  --out ../effects/hypercolor \
  --sdk-alias-path packages/core/src/index.ts
```

### Output

On success each artifact prints a line: a `✓` for canvas and shader effects, a `💎` for display faces, followed by the effect id, the output path, and the artifact size in KB.

```
✓ my-effect → dist/my-effect.html (42.3 KB)
💎 now-playing → dist/now-playing.html (58.1 KB)
```

{% callout(type="danger") %}
**The build enforces correctness — these fail the build, they do not warn.** If your source reads audio (`audio(`, `ctx.audio`, `getAudioData(`, or `engine.audio`) but the effect didn't set `audio: true` in its options, the build throws an audio-validation error. Every shader control except `asset` must have a matching `uniform i<Key>` in the GLSL, or the build reports missing control uniforms. And a module that never calls `canvas()`, `effect()`, or `face()` fails metadata extraction with "no effect definitions were registered." Treat a clean build as a real gate, not a formality.
{% end %}

## `validate`

Check one or more built HTML artifacts for the required render surface, title, and script. Validation runs automatically before every `install`, but you can run it standalone — for example, in CI against `dist/*.html`.

```bash
bunx hypercolor validate dist/*.html
bunx hypercolor validate dist/my-effect.html --strict
bunx hypercolor validate dist/my-effect.html --json
```

At least one file is required. With no files the CLI prints the usage line and exits non-zero.

### Flags

| Flag | Behavior |
|---|---|
| `--strict` | Treat warnings as failures. Exit non-zero if any artifact has a warning, even when no hard errors are present. |
| `--json` | Emit the raw validation result as JSON instead of the human-readable PASS/FAIL report. A single file emits one object; multiple files emit an array. |

### Human output

Each file reports a PASS/FAIL header, then one `WARN` line per warning and one `FAIL` line per error, then a final result with the warning count:

```
dist/my-effect.html

PASS  Render surface + title + script

Result: PASS
```

A failing artifact lists its errors:

```
dist/broken.html

FAIL  Validation errors present
FAIL  Missing required render surface

Result: FAIL
```

## `install`

Validate and deliver built artifacts. By default the CLI copies artifacts into your local user effects directory; with `--daemon` it uploads them to a running daemon over HTTP. Both paths validate first and reject invalid artifacts.

```bash
# Local install into the user effects directory
bunx hypercolor install dist/*.html

# Upload to a running daemon
bunx hypercolor install dist/*.html --daemon
```

With no positional patterns, install defaults to `dist/*.html`. Patterns may be globs or plain paths; non-matching plain paths are silently skipped.

### Flags

| Flag | Argument | Default | Behavior |
|---|---|---|---|
| `--daemon` | — | off | Upload to the daemon's install endpoint instead of copying locally. |
| `--daemon-url` | `<url>` | `$HYPERCOLOR_DAEMON_URL` or `http://127.0.0.1:9420` | Base URL of the daemon for `--daemon` installs. |

### Local install path

Local artifacts land in the user effects directory, derived from XDG:

```
$XDG_DATA_HOME/hypercolor/effects/user/
# falls back to ~/.local/share/hypercolor/effects/user/ when XDG_DATA_HOME is unset
```

On a name collision the installer appends a numeric suffix (`my-effect.html`, `my-effect-2.html`) rather than overwriting. The daemon picks up new local artifacts on startup or when you ask it to rescan from the system CLI:

```bash
hypercolor effects rescan
```

### Daemon install path

The `--daemon` path POSTs each validated artifact as a multipart form (field `file`) to `/api/v1/effects/install` on the daemon base URL:

{% api_endpoint(method="POST", path="/api/v1/effects/install") %}
Upload a built HTML effect to the running daemon. Multipart form, field `file`, content type `text/html`. Returns the standard `{ data, meta }` envelope where `data` carries `{ name, path, controls, presets }` — the installed effect name, its on-disk path, and the count of controls and presets the daemon extracted.
{% end %}

On a successful daemon install the CLI prints the installed name and control count:

```
✓ dist/my-effect.html → /home/you/.local/share/hypercolor/effects/user/my-effect.html (my-effect, 4 controls)
```

If the daemon rejects the artifact, the CLI surfaces the daemon's error details when present, otherwise the HTTP status.

{% callout(type="tip") %}
The scaffold defines `ship` and `ship:daemon` package scripts so you rarely type the install flags by hand. `bun run ship` is the local install, `bun run ship:daemon` is the daemon upload. Use the daemon path while iterating against a live app, and the local path when you want the effect to survive a daemon restart.
{% end %}

## `add`

Scaffold a new effect inside an existing workspace. Runs interactively when name or template is missing, prompting for the parts you didn't pass.

```bash
bunx hypercolor add aurora --template canvas
bunx hypercolor add aurora --template shader --audio
bunx hypercolor add            # fully interactive
```

### Flags

| Flag | Argument | Behavior |
|---|---|---|
| `--template` | `canvas \| shader \| face \| html` | Starter template for the new effect. |
| `--audio` | — | Seed audio-reactive boilerplate and set `audio: true`. |

When both a name and a valid `--template` are supplied, `add` runs non-interactively. Otherwise it prompts for the missing pieces. On success it prints the entry path of the new effect:

```
Entry: effects/aurora/main.ts
```

If `$VISUAL` or `$EDITOR` is set, `add` opens the new entrypoint in that editor.

## `dev` (removed)

The old `bunx hypercolor dev` preview server is gone. It now prints an error and exits non-zero:

```
hypercolor dev has been removed. Use build, validate, and install
against the real daemon/app preview instead.
Try: bun run build && bun run ship:daemon
```

The real iteration loop is build, ship, then preview inside the actual daemon or desktop app. Any guide that mentions a standalone effect preview server is stale.

## Exit codes

Every command returns a process exit code you can rely on in scripts and CI.

| Command | `0` (success) | Non-zero (`1`) |
|---|---|---|
| `build` | At least one artifact built. | No entrypoints found, or a build error (missing `audio: true`, shader uniform mismatch, no registered effect). |
| `validate` | All artifacts valid; under `--strict`, also no warnings. | Any artifact invalid, any warning under `--strict`, or no files given. |
| `install` | At least one artifact installed and none failed. | Any artifact failed, or zero artifacts installed. |
| `add` | Effect scaffolded. | Missing name or template after prompting. |
| `dev` | — | Always `1` (removed). |
| unknown command | — | `1`, after printing help. |

## Environment variables

| Variable | Used by | Default | Effect |
|---|---|---|---|
| `HYPERCOLOR_DAEMON_URL` | `install --daemon` | `http://127.0.0.1:9420` | Daemon base URL for daemon installs. `--daemon-url` overrides it. |
| `HYPERCOLOR_SDK_PACKAGE_SPEC` | `create-hypercolor-effect` | (none) | The `@hypercolor/sdk` dependency spec for new workspaces while the SDK is pre-release. `--sdk-spec` overrides it. |
| `VISUAL` / `EDITOR` | `add` | (none) | Editor opened on the new entrypoint. `VISUAL` wins when both are set. |

## The scaffolder: `create-hypercolor-effect`

Bootstrapping a brand-new workspace is a separate package, `@hypercolor/create-effect`, exposed as the `create-hypercolor-effect` bin. The authoring `hypercolor add` command reuses it internally to add effects to an existing workspace.

```bash
bunx create-hypercolor-effect my-effects --template canvas \
  --sdk-spec file:../hypercolor/sdk/packages/core
```

```
create-hypercolor-effect [name] [options]

Options:
  --template <type>       Starter template: canvas, shader, face, html
  --first <effect-name>   Name of the first effect (default: my-effect)
  --audio                 Include audio-reactive starter boilerplate
  --no-git                Skip git init
  --no-install            Skip bun install
  --sdk-spec <spec>       Required while @hypercolor/sdk is pre-release.
```

{% callout(type="info") %}
**The SDK is pre-release and not on npm.** Every new workspace must point its `@hypercolor/sdk` dependency at a local checkout, either through `--sdk-spec file:../hypercolor/sdk/packages/core` or the `HYPERCOLOR_SDK_PACKAGE_SPEC` environment variable. Without one of those, the scaffolder refuses to run. Bun's `link:` is not a drop-in for a relative path here — use `file:`. Once the SDK publishes to a registry this requirement goes away and a plain version spec will work.
{% end %}

The scaffolder runs interactively when the workspace name or template is missing, otherwise it builds the workspace directly. It initializes git and runs `bun install` by default; `--no-git` and `--no-install` opt out. When finished it prints the next command — `bun run build` for code templates, `bun run validate` for the raw `html` template.

## Scaffolded package scripts

A fresh workspace defines the everyday commands as package scripts, so the typical loop is short `bun run` invocations rather than long flag strings:

| Script | Runs | Purpose |
|---|---|---|
| `bun run build` | `hypercolor build --all` | Build every effect to `dist/`. |
| `bun run validate` | `hypercolor validate dist/*.html` | Validate built artifacts. |
| `bun run ship` | `hypercolor install dist/*.html` | Local install into the user effects directory. |
| `bun run ship:daemon` | `hypercolor install dist/*.html --daemon` | Upload to the running daemon. |
| `bun run add` | `hypercolor add` | Scaffold another effect interactively. |

## Where to go next

The [dev workflow](@/effects/dev-workflow.md) page walks the full build, validate, ship loop with screenshots and the live-preview story. The [setup](@/effects/setup.md) page covers installing Bun and wiring the `file:` SDK spec from scratch. For the runtime API the CLI compiles against, see the SDK API reference page in this section. And for the daemon-facing client this CLI is deliberately *not*, see the [system CLI reference](@/api/cli.md).
