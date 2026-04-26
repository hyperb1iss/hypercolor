+++
title = "GLSL Effects"
description = "Fragment shader effects with @hypercolor/sdk. Full control over every pixel"
weight = 4
template = "page.html"
+++

Shader effects run a WebGL2 fragment shader for every canvas pixel on every frame. The SDK sets up the full-screen quad, the uniform plumbing, the audio pipeline, and the HTML bundle; you write the pixel math.

An effect is a TypeScript module that imports a GLSL string and hands it to `effect()`:

```typescript
import { effect, num } from "@hypercolor/sdk";
import shader from "./fragment.glsl";

export default effect(
  "Borealis",
  shader,
  {
    intensity: num("Intensity", [0, 100], 82),
    palette: ["Aurora", "SilkCircuit", "Frost"],
    speed: num("Speed", [1, 10], 5),
  },
  { description: "Aurora curtains with layered shader motion" },
);
```

Everything else is GLSL.

## The `effect()` signature

```typescript
effect(name, shader, controls?, options?)
```

| Parameter  | Type              | Purpose                                                              |
| ---------- | ----------------- | -------------------------------------------------------------------- |
| `name`     | `string`          | Display name                                                         |
| `shader`   | `string`          | GLSL fragment shader source, usually imported from `.glsl`           |
| `controls` | `ControlMap`      | Controls that become GLSL uniforms                                   |
| `options`  | `EffectFnOptions` | Metadata + `audio`, `preserveDrawingBuffer`, `setup`, `vertexShader` |

Shader source is imported as a string. Scaffolded workspaces declare `.glsl` as a text import in `bunfig.toml`, so `import shader from './fragment.glsl'` just works in `bun run build`.

## Built-in uniforms

The SDK injects these uniforms automatically; you don't declare them in controls and you don't have to register them:

```glsl
uniform float iTime;       // elapsed seconds
uniform vec2  iResolution; // canvas size in pixels
uniform vec2  iMouse;      // unused on LED hardware, kept for compatibility
```

Always UV-normalize against `iResolution` so the shader is resolution-independent:

```glsl
vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution.xy) / iResolution.y;
```

The daemon canvas can change mid-session, so shaders should never assume a fixed aspect ratio or pixel size.

## Controls become uniforms

Every control you declare gets a GLSL uniform named with an `i` prefix and the original key PascalCased:

| Control key   | Uniform                                       |
| ------------- | --------------------------------------------- |
| `speed`       | `uniform float iSpeed;`                       |
| `trailLength` | `uniform float iTrailLength;`                 |
| `palette`     | `uniform int iPalette;`                       |
| `mirror`      | `uniform int iMirror;` (booleans become ints) |
| `tintColor`   | `uniform vec3 iTintColor;` (color picker)     |

Write the uniform declaration at the top of your shader; the SDK writes the value before every draw.

```glsl
uniform float iSpeed;
uniform float iIntensity;
uniform int   iPalette;

void main() {
    vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution.xy) / iResolution.y;
    float t = iTime * (0.25 + iSpeed * 0.08);
    // ...
}
```

You can override the uniform name per control with the `uniform` option on any factory:

```typescript
speed: num("Speed", [1, 10], 5, { uniform: "uSpeed" });
```

but the convention is to let the defaults stand.

### Magic controls

Two control keys have special behavior in shaders:

- `speed`. A number control named `speed` is normalized from its slider range into a usable scalar before the uniform is written. You still declare `uniform float iSpeed;` and multiply it into time.
- `palette`. A combobox control named `palette` (not `combo('Palette', ...)` but the shorthand `palette: ['A', 'B', 'C']`) becomes an integer uniform whose value is the selected index. Branch on `iPalette` inside a `palette()` function to pick the palette's color math.

If you want the palette as a named string inside TypeScript, use `combo('Palette', ...)` instead and sample the palette registry yourself. See [Palettes](@/effects/palettes.md) for that pattern.

## Audio uniforms

Set `audio: true` on the options and the SDK registers a full set of audio uniforms for you:

```glsl
uniform float iAudioLevel;            // overall RMS (0-1)
uniform float iAudioBass;             // bass band (0-1)
uniform float iAudioMid;              // mid band (0-1)
uniform float iAudioTreble;           // treble band (0-1)
uniform float iAudioBeat;             // 1 on detected beat frame, 0 otherwise
uniform float iAudioBeatPulse;        // decaying pulse (1 → 0)
uniform float iAudioBeatPhase;        // phase inside current beat (0-1)
uniform float iAudioBeatConfidence;   // confidence (0-1)
uniform float iAudioOnset;            // 1 on transient onset frame
uniform float iAudioOnsetPulse;       // decaying onset (1 → 0)
uniform float iAudioSpectralFlux;     // overall spectrum change rate
uniform float iAudioHarmonicHue;      // Circle of Fifths hue (0-360)
uniform float iAudioChordMood;        // -1 minor ... +1 major
uniform float iAudioBrightness;       // spectral centroid (0-1)
uniform float iAudioMomentum;         // level derivative (-1..1)
uniform float iAudioSwell;            // positive momentum (0-1)
uniform float iAudioTempo;            // estimated BPM
uniform vec3  iAudioFluxBands;        // band-specific flux [bass, mid, treble]
```

The SDK pushes new values every frame. Shader effects do not get `chromagram`, `melBands`, or `dominantPitch` as uniforms; those live in the canvas API only. If you need pitch-class energy in a shader, drop to canvas instead.

Typical shader uses:

```glsl
// a subtle breathing that tightens on beats
float pulse = max(iAudioBeatPulse, iAudioBass * 0.8 + iAudioLevel * 0.4);

// harmonic tinting across the screen
vec3 hueCol = vec3(
    0.5 + 0.5 * cos(radians(iAudioHarmonicHue) + vec3(0.0, 2.094, 4.189))
);

// minor chords cool, major chords warm
float mood = iAudioChordMood;
vec3 warm = mix(vec3(0.2, 0.4, 0.8), vec3(1.0, 0.6, 0.2), clamp(mood * 0.5 + 0.5, 0.0, 1.0));
```

## A minimal shader effect

```glsl
#version 300 es
precision highp float;

out vec4 fragColor;

uniform float iTime;
uniform vec2  iResolution;
uniform float iSpeed;
uniform float iIntensity;
uniform int   iPalette;
uniform float iAudioBeatPulse;
uniform float iAudioBass;

vec3 paletteColor(float t, int mode) {
    if (mode == 1) return mix(vec3(0.95, 0.35, 0.18), vec3(1.00, 0.80, 0.25), t);
    if (mode == 2) return mix(vec3(0.05, 0.18, 0.42), vec3(0.25, 0.85, 0.95), t);
    return mix(vec3(0.12, 0.58, 0.95), vec3(0.68, 0.22, 1.00), t);
}

void main() {
    vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution.xy) / iResolution.y;
    float t = iTime * (0.25 + iSpeed * 0.08);
    float pulse = max(iAudioBeatPulse, iAudioBass * 0.8);
    float swirl = sin(uv.x * (3.0 + pulse * 2.0) + t) + cos(uv.y * 4.5 - t * 1.4);
    float bloom = smoothstep(1.15, 0.12, length(uv) - swirl * (0.08 + pulse * 0.12));
    float intensity = clamp(iIntensity / 100.0, 0.0, 1.0);

    vec3 col = paletteColor(0.5 + 0.5 * sin(t + swirl + pulse * 3.0), iPalette);
    col *= mix(0.28, 1.2, bloom) * mix(0.35, 1.1, intensity + pulse * 0.2);

    fragColor = vec4(col, 1.0);
}
```

Paired with the TypeScript stub:

```typescript
import { effect } from "@hypercolor/sdk";
import shader from "./fragment.glsl";

export default effect(
  "Swirl",
  shader,
  {
    intensity: [0, 100, 70],
    palette: ["Aurora", "Fire", "Ocean"],
    speed: [1, 10, 5],
  },
  { audio: true, description: "A layered audio-reactive swirl" },
);
```

## GLSL patterns for LED work

Fragment shaders on LED hardware are not the same as fragment shaders on a monitor. The daemon samples your canvas at LED positions, so every "pixel you draw" is a color being emitted from a real diode. Rules that actually help:

**Normalize UV once and reuse it.**

```glsl
vec2 uv = (gl_FragCoord.xy - 0.5 * iResolution.xy) / iResolution.y;
```

This gives you `[-aspect/2, aspect/2]` horizontally and `[-0.5, 0.5]` vertically. Centered, aspect-corrected, resolution-independent.

**Clamp final output.** Let the shader blow out internally if it wants, but saturate before the fragment color. Otherwise the daemon sees out-of-range values and the LED output becomes unpredictable.

```glsl
col = clamp(col, 0.0, 1.0);
fragColor = vec4(col, 1.0);
```

**Suppress white blowout.** When all three channels converge on 1.0, the LED looks flat white and loses all hue. A hue-preserving soft clamp keeps saturation:

```glsl
float peak = max(col.r, max(col.g, col.b));
if (peak > 1.0) col /= peak;
```

For an even stronger version, cap the floor-to-peak ratio so the darkest channel never climbs too close to the brightest:

```glsl
float peak = max(col.r, max(col.g, col.b));
float floor = min(col.r, min(col.g, col.b));
if (peak > 1e-5 && floor / peak > 0.34) {
    float target = peak * 0.34;
    col = max((col - vec3(floor)) * (peak - target) / (peak - floor) + vec3(target), 0.0);
}
```

**Prefer cosine palettes for generative gradients.** They're cheap, smooth, and don't need a texture upload. Inigo Quilez's four-vec3 form is the ergonomic classic:

```glsl
vec3 cosPal(float t, vec3 a, vec3 b, vec3 c, vec3 d) {
    return a + b * cos(6.28318 * (c * t + d));
}
```

**Gamma-adjust before returning.** LEDs are close to linear PWM; eyes are logarithmic. A gentle mid-lift keeps colors readable at low brightness without blowing out highlights:

```glsl
vec3 liftMids(vec3 color, float amount) {
    vec3 lifted = pow(max(color, vec3(0.0)), vec3(0.86));
    return mix(color, lifted, clamp(amount, 0.0, 1.0));
}
```

See [Color Science for RGB LEDs](@/effects/color-science.md) for the theory; the `sdk/src/effects/breakthrough/fragment.glsl` shader is a working reference with the full toolkit.

## Debugging shaders

`bun run build` catches shader compile and bundling errors before install. Useful patterns:

- Drop intermediate values straight into `fragColor.rgb` to eyeball them: `fragColor = vec4(pulse, pulse, pulse, 1.0);`
- Add a temporary toggle control (`debug: toggle('Debug', false)`) and branch on `iDebug > 0` to render a diagnostic pass
- Check `iResolution.x / iResolution.y` by color-coding aspect ratio against the real daemon canvas sizes you care about

When you ship, make sure `bun run validate` passes. The validator won't catch logic bugs, but it will catch a missing render surface, missing metadata, or unparseable controls before the daemon refuses the artifact.

## When to choose canvas instead

Shaders are the right tool for noise fields, warped coordinate systems, kaleidoscopes, raymarched scenes, and "every pixel is a function of its position" effects. They are the wrong tool for:

- particle systems with state that needs to persist across frames
- physics or interactive simulation
- effects that depend on the chromagram, mel bands, or dominant pitch
- logic that's simpler as imperative code than as math

For anything in that list, [TypeScript canvas effects](@/effects/typescript-effects.md) are cleaner, faster to iterate on, and get more of the SDK's audio surface.
