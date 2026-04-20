+++
title = "Setup"
description = "Install Bun, scaffold a workspace, and connect @hypercolor/sdk"
weight = 1
template = "page.html"
+++

Every Hypercolor effect lives in a Bun workspace that depends on `@hypercolor/sdk`. The scaffolder generates a ready-to-run workspace in seconds. This page covers the one-time install, the first workspace, and the two shapes of project you're likely to build inside of.

## Install Bun

The SDK, CLI, and build tools all run on Bun. Install it once:

```bash
curl -fsSL https://bun.sh/install | bash
```

Confirm you have Bun 1.2 or newer:

```bash
bun --version
```

If you already have Node installed, that's fine. The scaffolder and CLI run on Bun directly; Node is only required if you want to invoke the `create-hypercolor-effect` bin from a shell that shims `node`.

## Scaffold a workspace

From the directory you want your new workspace to sit in:

```bash
bunx create-hypercolor-effect my-effects
```

The scaffolder prompts you for a template and a first effect. If you want it non-interactive, pass the options explicitly:

```bash
bunx create-hypercolor-effect my-effects \
    --template canvas \
    --first aurora \
    --audio
```

Available templates:

| Template | What you get |
|---|---|
| `canvas` | TypeScript effect with a Canvas2D draw function |
| `shader` | TypeScript effect with a GLSL fragment shader |
| `face` | A device-face layout for sensor dashboards |
| `html` | A raw LightScript HTML effect, no TypeScript |

The scaffolder runs `git init` and `bun install` by default. Pass `--no-git` or `--no-install` to skip either.

## Workspace layout

A freshly scaffolded TypeScript workspace looks like this:

```text
my-effects/
  effects/
    aurora/
      main.ts          # one file per effect in its own folder
  dist/                # built HTML artifacts land here (gitignored)
  package.json
  bunfig.toml          # declares .glsl as a text import
  biome.jsonc          # formatter + linter config
  tsconfig.json
```

For the HTML template, the layout is flatter because there's no TypeScript to bundle:

```text
my-effects/
  effects/
    aurora.html
  package.json
  README.md
```

The `dist/` directory is build output. Never hand-edit it and never commit it. Regenerate from source with `bun run build`.

## Pointing at `@hypercolor/sdk`

The SDK is still pre-release. While it's unpublished, scaffolded workspaces need to point at a local checkout.

### Standalone workspace alongside the monorepo

If your workspace is a sibling of a `hypercolor/` clone (for example `~/dev/my-effects` and `~/dev/hypercolor`), use a relative `file:` spec:

```bash
bunx create-hypercolor-effect my-effects \
    --template canvas \
    --sdk-spec "file:../hypercolor/sdk/packages/core"
```

You can also set it globally via the environment:

```bash
export HYPERCOLOR_SDK_PACKAGE_SPEC="file:../hypercolor/sdk/packages/core"
bunx create-hypercolor-effect my-effects --template canvas
```

{% callout(type="warning", title="Don't use link:") %}
Bun's `link:` spec requires a prior `bun link` registration and is not a drop-in relative path like yarn's `link:`. Use `file:` until the SDK publishes to npm.
{% end %}

### Inside the Hypercolor monorepo

Effects that ship with Hypercolor live under `sdk/src/effects/` in the main repo. Those workspaces resolve the SDK through Bun's workspace protocol automatically; you don't configure anything. Use the top-level `just` recipes to drive them:

```bash
just sdk-dev
just effects-build
just effect-build prism-choir
```

All three run the same Bun authoring core that standalone workspaces use.

## Once it's published

Once `@hypercolor/sdk` lands on npm, the scaffolder's default `^0.1.0` spec will just work and you can omit `--sdk-spec` entirely. Your existing workspaces can migrate by editing `package.json`:

```diff
- "@hypercolor/sdk": "file:../hypercolor/sdk/packages/core"
+ "@hypercolor/sdk": "^0.1.0"
```

## What next

Once `bun install` has finished, head to [Dev Workflow](@/effects/dev-workflow.md) to build your first artifact and ship it to the daemon.
