# @hypercolor/create-effect

Scaffolder for [Hypercolor](https://github.com/hyperb1iss/hypercolor) effect workspaces.

One command gives you a ready-to-run Bun workspace with `@hypercolor/sdk` wired up, a starter effect, and the full authoring loop for build, validate, and install.

## Quick start

The package is pre-release and not on npm yet. From a directory that contains a
`hypercolor/` checkout:

```bash
cd hypercolor/sdk
bun install
cd ../..
bun ./hypercolor/sdk/packages/create-effect/bin/create-hypercolor-effect.js my-effects \
    --template canvas \
    --sdk-spec "file:../hypercolor/sdk/packages/core"
```

The scaffolder prompts you for any omitted template or first-effect option. For
audio-reactive starter boilerplate:

```bash
bun ./hypercolor/sdk/packages/create-effect/bin/create-hypercolor-effect.js my-effects \
    --template canvas \
    --first aurora \
    --audio \
    --sdk-spec "file:../hypercolor/sdk/packages/core"
```

Then:

```bash
cd my-effects
bun run build
```

## Templates

| Template | What you get                                    |
| -------- | ----------------------------------------------- |
| `canvas` | TypeScript effect with a Canvas2D draw function |
| `shader` | TypeScript effect with a GLSL fragment shader   |
| `face`   | A device-face layout for sensor dashboards      |
| `html`   | A raw LightScript HTML effect, no TypeScript    |

## Options

```
create-hypercolor-effect [name] [options]

  --template <type>       Starter template: canvas, shader, face, html
  --first <effect-name>   Name of the first effect (default: my-effect)
  --audio                 Include audio-reactive starter boilerplate
  --no-git                Skip git init
  --no-install            Skip bun install
  --sdk-spec <spec>       Required while @hypercolor/sdk is pre-release.
                          Point at a local checkout:
                          file:../hypercolor/sdk/packages/core
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
