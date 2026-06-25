+++
title = "Setup & workspace"
description = "Bun, the scaffold, and the file: SDK spec. The SDK is pre-release, so workspaces point at a local checkout, not npm."
weight = 10
template = "page.html"
+++

Every Hypercolor effect lives in a Bun workspace that depends on `@hypercolor/sdk`. This page gets that workspace running: install Bun, get the SDK source, scaffold a project, and wire the `file:` spec that connects the two. Once `bun install` finishes, you're ready to author.

![The Hypercolor effects browser, the surface your built effects land in](/img/ui/effects.webp)
<!-- No setup-specific UI shot exists; the effects browser shows what authored effects become once shipped. -->

The endpoint of this page is a workspace where `bun install` has succeeded and `@hypercolor/sdk` resolves from a local checkout. If that is true, you can author. The next page, [Creating effects](@/effects/creating-effects.md), writes the first one.

{% callout(type="warning", title="The SDK is pre-release and not on npm") %}
`@hypercolor/sdk` has not been published yet. Workspaces depend on it through a local `file:` spec that points at a Hypercolor checkout, **not** through npm and **not** through Bun's `link:`. The scaffolder hard-fails if you don't supply that spec, and a `package.json` copied from a tutorial that lists `"@hypercolor/sdk": "^0.1.0"` will fail `bun install`. Always pass `--sdk-spec file:../hypercolor/sdk/packages/core` (or set `HYPERCOLOR_SDK_PACKAGE_SPEC`).
{% end %}

## Install Bun

The SDK, the authoring CLI, and the build tooling all run on Bun. Install it once:

```bash
curl -fsSL https://bun.sh/install | bash
```

Confirm you have Bun 1.2 or newer (the workspace `engines` field requires it):

```bash
bun --version
```

Node is not required to author effects. The scaffolder, the build, and the CLI all run on Bun directly. The workspace declares `node >=24.0.0` only so the same `package.json` stays valid if you ever run the published bin through a Node shim.

## Get the SDK source

Because the SDK is unpublished, clone Hypercolor next to wherever you want your workspace to live, then install its own dependencies once:

```bash
mkdir -p ~/dev
cd ~/dev
git clone https://github.com/hyperb1iss/hypercolor.git
cd hypercolor/sdk
bun install
cd ../..
```

After this you have `~/dev/hypercolor/sdk/packages/core` on disk. That directory is the target your workspace's `file:` spec resolves to.

## Scaffold a workspace

From the directory that contains your `hypercolor/` clone, run the scaffolder:

```bash
bun ./hypercolor/sdk/packages/create-effect/bin/create-hypercolor-effect.js my-effects \
    --template canvas \
    --first aurora \
    --sdk-spec "file:../hypercolor/sdk/packages/core"
```

That creates `my-effects/`, drops in one starter effect named `aurora`, runs `git init`, and runs `bun install`. The CLI prints the next command to run when it finishes.

Omit the workspace name or `--template` and the scaffolder switches to interactive prompts for the workspace name, the template, the first effect name, and an "Audio reactive?" toggle. Keep `--sdk-spec` regardless. Without it the scaffolder exits with code 1 and the message `@hypercolor/sdk is not published yet. Pass --sdk-spec file:../hypercolor/sdk/packages/core or set HYPERCOLOR_SDK_PACKAGE_SPEC.`.

### Flags

| Flag | Effect |
| --- | --- |
| `--template <type>` | Starter template: `canvas`, `shader`, `face`, or `html`. |
| `--first <name>` | Name of the first effect. Defaults to `my-effect`. |
| `--audio` | Include audio-reactive starter boilerplate. |
| `--sdk-spec <spec>` | The SDK dependency spec. Required while pre-release. |
| `--no-git` | Skip `git init`. |
| `--no-install` | Skip `bun install`. |

`--sdk-spec` falls back to the `HYPERCOLOR_SDK_PACKAGE_SPEC` environment variable, so you can export it once instead of repeating the flag:

```bash
export HYPERCOLOR_SDK_PACKAGE_SPEC="file:../hypercolor/sdk/packages/core"
bun ./hypercolor/sdk/packages/create-effect/bin/create-hypercolor-effect.js my-effects --template canvas
```

## The four templates

The scaffolder ships four starter shapes. The first three are TypeScript projects bundled to HTML at build time; `html` is raw markup with no TypeScript step.

| Template | What you get | Authoring guide |
| --- | --- | --- |
| `canvas` | A TypeScript effect with a Canvas2D draw function. The default starting point for most effects. | [TypeScript effects](@/effects/typescript-effects.md) |
| `shader` | A TypeScript effect backed by a GLSL fragment shader, rendered as WebGL2 in the runtime. | [GLSL effects](@/effects/glsl-effects.md) |
| `face` | A device-face layout for sensor and status dashboards. | [Display faces](@/effects/display-faces.md) |
| `html` | A raw LightScript HTML effect with no TypeScript or bundle step. | [Raw HTML](@/effects/raw-html.md) |

Add `--audio` to any of them to seed the audio-reactive boilerplate from the start. You can always grow into audio later; see [Audio](@/effects/audio.md).

## Workspace layout

A freshly scaffolded TypeScript workspace (`canvas`, `shader`, or `face`) looks like this:

```text
my-effects/
  effects/
    aurora/
      main.ts          # one folder per effect, main.ts is the entry
  package.json         # @hypercolor/sdk via the file: spec
  bunfig.toml          # loads .glsl as text, hardlinks installs
  biome.jsonc          # formatter + linter config
  tsconfig.json
  README.md
  .gitignore           # node_modules, dist, .DS_Store
```

The `dist/` directory appears the first time you build. It holds the self-contained HTML artifacts the daemon loads. It is gitignored, it is build output, and you should never hand-edit or commit it. Regenerate it from source with `bun run build`.

The `html` template is flatter because there's nothing to bundle. Effects are authored directly as the shipped artifact:

```text
my-effects/
  effects/
    aurora.html
  package.json
  README.md
```

## The package scripts

The scaffolded `package.json` wraps the authoring CLI in named scripts so you rarely call the binary directly. For a TypeScript workspace:

```json
{
  "scripts": {
    "build": "hypercolor build --all",
    "build:one": "hypercolor build",
    "validate": "hypercolor validate dist/*.html",
    "ship": "hypercolor install dist/*.html",
    "ship:daemon": "hypercolor install dist/*.html --daemon",
    "add": "hypercolor add",
    "check": "biome check .",
    "check:fix": "biome check --write ."
  }
}
```

The `html` workspace skips `build` and points `validate` and `ship` straight at `effects/*.html`. The full meaning of each command, both install paths, and the difference between this authoring CLI and the system `hypercolor` CLI all live in [Dev workflow](@/effects/dev-workflow.md).

## How the file: spec resolves

The scaffolder substitutes whatever you pass to `--sdk-spec` into the workspace `package.json`:

```json
{
  "devDependencies": {
    "@hypercolor/sdk": "file:../hypercolor/sdk/packages/core"
  }
}
```

That relative path is resolved from the workspace root, so it assumes your workspace is a sibling of the `hypercolor/` clone (for example `~/dev/my-effects` next to `~/dev/hypercolor`). If your layout differs, adjust the relative path or use an absolute one.

{% callout(type="warning", title="Use file:, not link:") %}
Bun's `link:` spec is not a drop-in relative path the way yarn's is. It requires a prior `bun link` registration of the package and resolves through Bun's global link store, so a copied `link:` spec breaks `bun install` with no obvious cause. Stick with `file:` until the SDK ships to npm.
{% end %}

### Inside the Hypercolor monorepo

Effects that ship with Hypercolor live under `sdk/src/effects/` in the main repo. Those resolve the SDK through Bun's workspace protocol automatically, so there's nothing to configure. Drive them with the top-level `just` recipes instead of the standalone scaffolder:

```bash
just sdk-dev          # authoring dev server with HMR
just effects-build    # build every bundled effect
just effect-build NAME
```

All of these run the same Bun authoring core a standalone workspace uses.

### After the SDK publishes

Once `@hypercolor/sdk` lands on npm, the `--sdk-spec` flag becomes optional and the scaffolder can default to a version range. Existing workspaces migrate with a one-line edit:

```diff
- "@hypercolor/sdk": "file:../hypercolor/sdk/packages/core"
+ "@hypercolor/sdk": "^0.1.0"
```

## What next

Your workspace is ready. Head to [Creating effects](@/effects/creating-effects.md) to write your first effect, or jump straight to [Dev workflow](@/effects/dev-workflow.md) to build an artifact and ship it to the daemon. The SDK already ships roughly four dozen HTML effects under `sdk/src/effects/`, alongside the engine's native built-in renderers, so there is plenty of reference material to mine for idioms once you start authoring.
