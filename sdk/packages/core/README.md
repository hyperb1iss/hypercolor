# @hypercolor/sdk

TypeScript SDK for authoring [Hypercolor](https://github.com/hyperb1iss/hypercolor) RGB lighting effects.

One declarative function call turns your idea into a shippable artifact. The SDK handles the render loop, control UI generation, audio pipeline, palette sampling, preview studio, and HTML bundling so you stay focused on the pixels.

## Quick start

Scaffold a fresh workspace with the companion package:

```bash
bunx create-hypercolor-effect my-effects
cd my-effects
bun run dev
```

That gives you a live preview studio at `http://localhost:4200`, an effect to edit, and the full build/validate/install loop.

Write an effect:

```typescript
import { audio, canvas, num } from '@hypercolor/sdk'

export default canvas(
    'Pulse',
    {
        palette: ['SilkCircuit', 'Aurora', 'Synthwave'],
        speed: num('Speed', [1, 10], 5),
    },
    (ctx, time, controls) => {
        const { width, height } = ctx.canvas
        const pal = controls.palette as (t: number, alpha?: number) => string
        const a = audio()

        ctx.fillStyle = 'rgba(4, 2, 14, 0.22)'
        ctx.fillRect(0, 0, width, height)

        const radius = Math.min(width, height) * (0.2 + a.beatPulse * 0.3)
        ctx.fillStyle = pal(0.7, 0.6 + a.bass * 0.4)
        ctx.beginPath()
        ctx.arc(width * 0.5, height * 0.5, radius, 0, Math.PI * 2)
        ctx.fill()
    },
    { audio: true },
)
```

Build, validate, and install:

```bash
bun run build
bun run validate
bun run ship:daemon
```

## Three authoring paths

- **TypeScript canvas effects** via `canvas()`: declarative draw functions with full audio and palette access.
- **GLSL shader effects** via `effect()`: fragment shaders with auto-mapped uniforms, including all audio bands.
- **Raw LightScript HTML**: standalone HTML files with meta-tag metadata, for porting and one-offs.

## Authoring CLI

Inside any scaffolded workspace, the `hypercolor` CLI drives the full loop:

```bash
bunx hypercolor dev            # preview studio on :4200
bunx hypercolor build --all    # compile every effect into dist/
bunx hypercolor validate dist/*.html
bunx hypercolor install dist/my-effect.html           # local filesystem copy
bunx hypercolor install dist/my-effect.html --daemon  # upload via daemon API
bunx hypercolor add ember --template canvas           # scaffold another effect
```

Scaffolded workspaces expose the same flow through `bun run dev`, `bun run build`, `bun run validate`, `bun run ship`, and `bun run ship:daemon`.

## Documentation

Full docs live at [the Hypercolor documentation site](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects). Highlights:

- [Effects overview](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/_index.md): pick your authoring path
- [Setup](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/setup.md): Bun, scaffolding, SDK spec recipes
- [Dev workflow](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/dev-workflow.md): studio, build, validate, install
- [TypeScript effects](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/typescript-effects.md)
- [GLSL effects](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/glsl-effects.md)
- [Raw HTML effects](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/raw-html.md)
- [Controls](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/controls.md)
- [Audio](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/audio.md)
- [Palettes](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/palettes.md)
- [Color science for RGB LEDs](https://github.com/hyperb1iss/hypercolor/tree/main/docs/content/effects/color-science.md)

## Prerequisites

- Bun 1.2 or newer
- Node 24 or newer if invoking the scaffolder's `create-hypercolor-effect` bin from a Node shell

## Status

Pre-release. The SDK is not yet published to npm. Standalone workspaces should point at a local checkout via a `file:` spec:

```json
{
    "devDependencies": {
        "@hypercolor/sdk": "file:../hypercolor/sdk/packages/core"
    }
}
```

The scaffolder's `--sdk-spec` flag and the `HYPERCOLOR_SDK_PACKAGE_SPEC` environment variable both accept this form.

## License

Apache-2.0
