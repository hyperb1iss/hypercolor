+++
title = "AI Prompt Template"
description = "A drop-in prompt for generating Hypercolor effects with Claude, GPT, or another model"
weight = 10
template = "page.html"
+++

Use this when you want a coding model to generate a Hypercolor effect that actually fits the SDK and LED hardware. The constraints are specific because each one prevents a class of generic output the models default to.

## Prompt template

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
- Use only @hypercolor/sdk helpers; no external dependencies
- The code must build with `bunx hypercolor build`

Creative direction:
- Mood: <ambient / aggressive / dreamy / cinematic / etc>
- Motion: <slow drift / pulse / strobe / orbit / turbulence / etc>
- Palette: <specific colors or named palette from the registry>
- Hardware shape bias: <strip / matrix / ring / generic>

Controls:
- Include 3-6 meaningful controls
- Group related controls
- Add 2-3 presets that fully set every control

Constraints:
- Read ctx.canvas.width and ctx.canvas.height every frame (never hardcode)
- Design for LEDs, not a bright monitor
- Keep at least one RGB channel near zero for any vivid color
- No large white fills unless explicitly requested
- Keep motion readable on low-density hardware
- If audio reactive, reach for the harmonic stack (chromagram, harmonicHue,
  chordMood, onsetPulse), not only bass and beatPulse
- Give the effect an idle life so it reads in silence
- Include author and description metadata

Return format:
- First the file path
- Then a fenced code block
- No extra explanation unless requested
```

## Constraint add-ons

Pick a few of these when you want tighter output:

- "Favor saturated mids over clipped highlights."
- "Make it legible on a 60 LED strip."
- "Use a dark floor so idle states do not wash out a room."
- "Keep presets meaningfully different, not tiny parameter nudges."
- "Bias the composition toward the center because this is for a ring."
- "Treat bass as structure and treble as sparkle."
- "Use palette sampling with shorthand declaration, not manual HSL math."
- "Prefer `globalCompositeOperation = 'lighter'` for overlapping glow elements."
- "Include a trails toggle that uses semi-transparent fillRect instead of clear."

## Example prompt

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
- Use only @hypercolor/sdk helpers
- The code must build with `bunx hypercolor build`

Creative direction:
- Mood: molten, ritual, cinematic
- Motion: slow orbit with bass-driven flares
- Palette: Ember, Lava, Sunset (pick via combo control)
- Hardware shape bias: ring

Controls:
- 5 meaningful controls grouped by Color and Motion
- 3 presets that fully set every control

Constraints:
- Read ctx.canvas.width and ctx.canvas.height every frame
- No large white fills
- Keep motion readable on a 24 LED ring
- Reach for bassEnv and onsetPulse for reactivity
- Give the halo an idle breathing life so it reads in silence
- Include author and description metadata

Return format:
- First the file path
- Then a fenced code block
- No extra explanation unless requested
```

## Review checklist

After the model returns code, run the normal pipeline:

```bash
bunx hypercolor build --all
bunx hypercolor validate dist/<id>.html
bunx hypercolor install dist/<id>.html --daemon
```

In the real app or on hardware, check each of these before shipping:

- Does every control actually change something visible?
- Do presets produce meaningfully different looks, or are they parameter nudges?
- In silence, does the effect still feel alive?
- At very low bass, does the effect collapse to nothing or hold a baseline?
- Under an aggressive beat, does it blow out to white or stay chromatically interesting?
- Does it hold up on the canvas sizes and hardware shapes you actually care about?

If it passes, ship:

```bash
bunx hypercolor install dist/<id>.html --daemon
```

## Pairing with effect-reviewer

The `.agents/agents/effect-reviewer` subagent will sanity-check generated effects against LED hardware best practices. After the model generates the code, point the reviewer at the file:

```text
Review effects/ember-halo/main.ts against LED best practices.
Flag any washout risk, hardcoded dimensions, or missing idle behavior.
```

The reviewer is tuned against the same rules that drive the constraints in this prompt, so the two work well in sequence.
