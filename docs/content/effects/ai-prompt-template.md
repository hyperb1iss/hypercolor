+++
title = "AI prompt template"
description = "A drop-in prompt for generating Hypercolor effects with Claude, GPT, or any coding model, plus a review checklist."
weight = 170
template = "page.html"
+++

A coding model will happily write a canvas animation. Left to its defaults it writes the wrong one: bright on a monitor, washed out on LEDs, with controls that do nothing and presets that nudge a parameter by two percent. This page gives you a prompt that pins down everything the model would otherwise guess, plus a review pass to catch what slips through.

Every constraint below prevents a specific class of generic output. Keep them. The point is not a clever prompt, it is a prompt that fits the [SDK](@/effects/setup.md) and the [color science of real LEDs](@/effects/color-science.md) at the same time.

{% callout(type="tip") %}
Pair this with the live engine. If you have the [MCP server](@/api/mcp.md) connected, an agent can read the current state, build the effect, install it, and apply it to your rig in one loop. The prompt produces the code; MCP closes the feedback cycle on real hardware.
{% end %}

## The prompt template

Fill in the angle-bracket fields and paste this into Claude, GPT, or your model of choice.

```text
Write a Hypercolor effect for hypercolor.

Target:
- Effect name: <display name>
- Effect id: <kebab-case workspace id>
- Renderer: <canvas or shader>
- Audio reactive: <yes or no>

Output requirements:
- Return code for effects/<id>/main.ts
- If renderer=shader, also return effects/<id>/fragment.glsl
- Each effect module calls canvas(...) or effect(...) and `export default`s
  the result (the call registers the effect; the default export is void)
- Use only hypercolor helpers; no external dependencies
- The code must build clean with `bunx hypercolor build`

Creative direction:
- Mood: <ambient / aggressive / dreamy / cinematic / etc>
- Motion: <slow drift / pulse / strobe / orbit / turbulence / etc>
- Palette: <specific colors or a named palette from the registry>
- Hardware shape bias: <strip / matrix / ring / generic>

Controls:
- Include 3-6 meaningful controls
- Group related controls
- Add 2-3 presets that fully set every control

Constraints:
- Read ctx.canvas.width and ctx.canvas.height every frame (never hardcode)
- Animate off elapsed time, never frame counts (FPS is adaptive)
- Design for LEDs, not a bright monitor
- Keep min(R,G,B)/max(R,G,B) below ~0.3 for any vivid color
- No large white fills unless explicitly requested
- Keep motion readable on low-density hardware
- If audio reactive, set { audio: true } in the options (the build FAILS
  without it once you read audio) and reach for the harmonic stack
  (chromagram, harmonicHue, chordMood, onsetPulse), not only bass and beatPulse
- Give the effect an idle life so it reads in silence
- Include author and description metadata

Return format:
- First the file path
- Then a fenced code block
- No extra explanation unless requested
```

A few of those constraints are load-bearing because they map to build-time hard errors, not style preferences:

- `{ audio: true }` is **required** whenever the source reads audio. The build statically scans for `audio(`, `ctx.audio`, `getAudioData(`, or `engine.audio` and throws `Audio reactivity validation failed` if the flag is missing. It is not cosmetic metadata.
- For shader effects, every control needs a matching `uniform i<Key>` in the GLSL or the build throws `missing control uniforms`. The model has to keep the control declaration and the shader uniforms in sync. See [GLSL shader effects](@/effects/glsl-effects.md) for the naming rule.
- The module must actually **call** `canvas()` / `effect()`. If it only defines a function, metadata extraction throws `no effect definitions were registered`.

## Constraint add-ons

Pick a few of these when you want tighter output. Each targets a habit models fall into.

- "Favor saturated mids over clipped highlights."
- "Make it legible on a 60-LED strip."
- "Use a dark floor so idle states do not wash out a room."
- "Keep presets meaningfully different, not tiny parameter nudges."
- "Bias the composition toward the center because this is for a ring."
- "Treat bass as structure and treble as sparkle."
- "Use palette sampling with shorthand declaration, not manual HSL math."
- "Prefer `globalCompositeOperation = 'lighter'` for overlapping glow elements."
- "Include a trails toggle that uses semi-transparent `fillRect` instead of clearing the canvas each frame."

{% callout(type="info") %}
Naming a palette beats describing colors. The SDK ships a registry of named palettes (SilkCircuit, Aurora, Cyberpunk, Vaporwave, Fire, Ice, Viridis, and more) that interpolate in Oklab, so they hold their chroma on hardware where hand-mixed HSL gradients turn to mud. Tell the model `Palette: Aurora` or expose a `paletteControl` rather than asking it to invent hex values. The full list lives in the [palette reference](@/effects/palettes.md).
{% end %}

## Seed the prompt with an existing effect

The fastest way to get house-style output is to hand the model a working effect as a reference. The SDK ships dozens of them under `sdk/src/effects/`, and the [effect gallery](@/effects/_index.md) shows what they look like. Append something like this to the prompt:

```text
Match the structure and quality bar of this existing effect. Use it as a
reference for control grouping, palette usage, and idle behavior — do not
copy its visuals.

<paste the contents of sdk/src/effects/lava-lamp/main.ts>
```

Stateful canvas effects like `lava-lamp` and `fiberflies` are good seeds: they show the factory pattern (a zero-argument setup function that returns the per-frame draw), real palette sampling, and an idle life that holds up in silence.

<!-- effect gallery tile: lava-lamp -->
![Lava Lamp, a stateful canvas effect that makes a good prompt seed](/img/effects/lava-lamp.webp)

## A worked example

```text
Write a Hypercolor effect for hypercolor.

Target:
- Effect name: Ember Halo
- Effect id: ember-halo
- Renderer: canvas
- Audio reactive: yes

Output requirements:
- Return code for effects/ember-halo/main.ts
- Each effect module calls canvas(...) and `export default`s the result
- Use only hypercolor helpers
- The code must build clean with `bunx hypercolor build`

Creative direction:
- Mood: molten, ritual, cinematic
- Motion: slow orbit with bass-driven flares
- Palette: Ember, Lava, Sunset (pick via a paletteControl)
- Hardware shape bias: ring

Controls:
- 5 meaningful controls grouped by Color and Motion
- 3 presets that fully set every control

Constraints:
- Read ctx.canvas.width and ctx.canvas.height every frame
- Set { audio: true } in the options
- No large white fills
- Keep motion readable on a 24-LED ring
- Reach for bassEnv and onsetPulse for reactivity, gate beat energy by
  beatConfidence so non-rhythmic audio does not strobe
- Give the halo an idle breathing life so it reads in silence
- Include author and description metadata

Return format:
- First the file path
- Then a fenced code block
- No extra explanation unless requested
```

That prompt names a real palette set, forces the `audio: true` flag, points the model at the decaying-envelope audio fields instead of the binary beat, and demands an idle state. Those four moves are what separate an effect that ships from one that looks fine in the editor and dead on the rig.

## Review the output

When the model returns code, run the normal authoring pipeline. These are the real authoring-CLI subcommands; see the [SDK CLI reference](@/effects/sdk-cli-reference.md) for every flag.

```bash
bunx hypercolor build --all
bunx hypercolor validate dist/<id>.html
```

A clean build plus a passing validate means the artifact is structurally sound: the render surface is present, controls and presets are well-formed, and any shader uniforms line up. It does not mean the effect looks good. Install it and judge it on hardware or in the app:

```bash
bunx hypercolor install dist/<id>.html --daemon
```

The `--daemon` flag uploads the validated HTML to the running daemon at `http://127.0.0.1:9420`, so it appears in the effect catalog immediately. Without the flag, `install` copies the file into your local effects directory and the daemon picks it up on the next rescan. Both paths validate first and reject a broken artifact.

### The checklist

Before you ship, walk the effect through these. Models pass the build and fail half of these on the first try.

- Does every control actually change something visible? A control that does nothing is worse than no control.
- Do presets produce meaningfully different looks, or are they parameter nudges?
- In silence, does the effect still feel alive, or does it freeze?
- At very low bass, does it collapse to nothing, or hold a baseline?
- Under an aggressive beat, does it blow out to white, or stay chromatically interesting?
- Is any vivid color keeping at least one RGB channel near zero, so it reads as a color and not as bright white?
- Does it hold up at the canvas sizes and on the hardware shapes you actually care about?

{% callout(type="warning") %}
The most common model failure is mapping a binary beat straight to brightness, which strobes harshly on real LEDs. The fix is in the prompt: route beat energy into motion, drive brightness from the decaying `beatPulse` / `onsetPulse` envelopes, and gate by `beatConfidence` so non-rhythmic audio stays calm. The full audio surface is documented in the [audio API reference](@/effects/audio.md).
{% end %}

## Pair with the effect reviewer

The `effect-reviewer` agent in `.agents/agents/` sanity-checks generated effects against LED hardware best practices. It is tuned against the same rules that drive the constraints above, so the two compose: the prompt generates, the reviewer audits. After the model returns code, point the reviewer at the file.

```text
Review effects/ember-halo/main.ts against LED best practices.
Flag any washout risk, hardcoded dimensions, missing idle behavior, or
binary-beat-to-brightness mapping.
```

For deeper edits — fixing a flagged washout, restructuring controls, porting a shader — hand the same file to the model with the reviewer's notes and iterate. The build and validate gates stay your ground truth at every step; if the artifact stops compiling, the review does not matter yet.
