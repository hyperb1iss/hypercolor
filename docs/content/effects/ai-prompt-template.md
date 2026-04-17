+++
title = "AI Prompt Template"
description = "A copy-paste prompt for generating Hypercolor effects with the SDK"
weight = 4
template = "page.html"
+++

Use this when you want Claude, GPT, or another coding model to generate a Hypercolor effect that actually fits the SDK, the preview studio, and LED hardware.

## Prompt Template

```text
Write a Hypercolor effect for @hypercolor/sdk.

Target:
- Effect name: <display name>
- Effect id: <kebab-case workspace id>
- Renderer: <canvas or shader>
- Audio reactive: <yes or no>

Output requirements:
- Return code for effects/<id>/main.ts
- If renderer=shader, also return effects/<id>/fragment.glsl
- Export a single default effect
- Use @hypercolor/sdk helpers only
- The code must build with bunx hypercolor build

Creative direction:
- Mood: <ambient / aggressive / dreamy / cinematic / etc>
- Motion: <slow drift / pulse / strobe / orbit / turbulence / etc>
- Palette: <specific colors or named direction>
- Hardware shape bias: <strip / matrix / ring / generic>

Controls:
- Include 3-6 meaningful controls
- Group related controls
- Add 2-3 presets

Constraints:
- Read canvas width and height every frame
- Design for LEDs, not a bright monitor
- Avoid large white fills unless explicitly requested
- Keep motion readable on low-density hardware
- If audio reactive, use engine audio data tastefully instead of maxing every band
- Include author and description metadata

Return format:
- First the file path
- Then a fenced code block
- No extra explanation unless requested
```

## Good Constraint Add-Ons

Add a few of these when you want tighter results:

- "Favor saturated mids over clipped highlights."
- "Make it legible on a 60 LED strip."
- "Use a dark floor so idle states do not wash out a room."
- "Keep presets meaningfully different, not tiny parameter nudges."
- "Bias the composition toward the center because this is for a ring."
- "Treat bass as structure and treble as sparkle."

## Example Prompt

```text
Write a Hypercolor effect for @hypercolor/sdk.

Target:
- Effect name: Ember Halo
- Effect id: ember-halo
- Renderer: canvas
- Audio reactive: yes

Output requirements:
- Return code for effects/ember-halo/main.ts
- Export a single default effect
- Use @hypercolor/sdk helpers only
- The code must build with bunx hypercolor build

Creative direction:
- Mood: molten, ritual, cinematic
- Motion: slow orbit with occasional bass-driven flares
- Palette: ember orange, toxic magenta, near-black background
- Hardware shape bias: ring

Controls:
- Include 5 meaningful controls
- Group related controls
- Add 3 presets

Constraints:
- Read canvas width and height every frame
- Design for LEDs, not a bright monitor
- Avoid large white fills
- Keep motion readable on a 24 LED ring
- Use engine audio data tastefully
- Include author and description metadata

Return format:
- First the file path
- Then a fenced code block
- No extra explanation unless requested
```

## Review Checklist

After generating code, run the normal pipeline:

```bash
bunx hypercolor build --all
bunx hypercolor validate dist/<id>.html
bunx hypercolor dev
```

If the effect is worth keeping, finish with:

```bash
bunx hypercolor install dist/<id>.html --daemon
```
