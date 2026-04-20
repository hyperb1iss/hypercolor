# @hypercolor/create-effect

Scaffolder for [Hypercolor](https://github.com/hyperb1iss/hypercolor) effect workspaces.

One command gives you a ready-to-run Bun workspace with `@hypercolor/sdk` wired up, a starter effect, and the full authoring loop for build, validate, and install.

## Quick start

```bash
bunx create-hypercolor-effect my-effects
```

The scaffolder prompts you for a template and a first effect. For non-interactive use:

```bash
bunx create-hypercolor-effect my-effects \
    --template canvas \
    --first aurora \
    --audio
```

Then:

```bash
cd my-effects
bun run build
```

## Templates

| Template | What you get |
|---|---|
| `canvas` | TypeScript effect with a Canvas2D draw function |
| `shader` | TypeScript effect with a GLSL fragment shader |
| `face` | A device-face layout for sensor dashboards |
| `html` | A raw LightScript HTML effect, no TypeScript |

## Options

```
create-hypercolor-effect [name] [options]

  --template <type>       Starter template: canvas, shader, face, html
  --first <effect-name>   Name of the first effect (default: my-effect)
  --audio                 Include audio-reactive starter boilerplate
  --no-git                Skip git init
  --no-install            Skip bun install
  --sdk-spec <spec>       Override the generated @hypercolor/sdk dependency.
                          While the SDK is pre-release, point at a local
                          checkout: file:../hypercolor/sdk/packages/core
                          (HYPERCOLOR_SDK_PACKAGE_SPEC env var also works).
```

## Adding more effects

Once a workspace exists, scaffold additional effects with the workspace CLI:

```bash
bunx hypercolor add ember --template canvas
bunx hypercolor add skyline --template shader --audio
bunx hypercolor add flicker --template html
```

## Prerequisites

- Bun 1.2 or newer

## Documentation

See [Setup](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/setup.md) and [Dev Workflow](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/dev-workflow.md) for the full guide.

## License

Apache-2.0
