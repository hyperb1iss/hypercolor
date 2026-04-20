# 02 · What Makes a Plugin Effect API Feel Elegant

**Status:** research, opinionated
**Scope:** survey of effect/plugin authoring APIs across shader editors, audio
plugins, visual compositors, and LED engines — with a concrete recommendation
for Hypercolor's WASM-loaded native effect API.
**Consumers:** whoever builds the WASM plugin story (see `01-*` in this folder
for runtime selection; this doc is strictly about the guest-facing API shape).

> TL;DR — **Recommendation: "derive-the-contract" Rust API.**
> A `#[derive(Effect)]` macro on a struct holding typed parameter fields (the
> same field-driven schema NIH-plug popularized) plus a single
> `fn render(&mut self, ctx: &Frame, canvas: &mut Canvas)` method. No JSON
> alongside the binary. No per-pixel callback. No function-pointer table.
> Parameters *are* struct fields; the schema is derived; the host introspects
> the compiled component for controls, presets, and persistence. One file,
> ~30 lines for a striking effect, zero ceremony. Full defense in §12.

---

## 1 · Shadertoy and ISF: the per-pixel callback

Shadertoy's `void mainImage(out vec4 fragColor, in vec2 fragCoord)` is the
canonical elegant effect API. The appeal has three real sources. First, the
coordinate-in-color-out contract fits what most visual effects actually *are*
mathematically, so the surface area of the API equals the surface area of the
problem. Second, ergonomic uniforms (`iTime`, `iTimeDelta`, `iFrame`,
`iResolution`, `iMouse`, `iDate`, `iChannel0..3`, `iSampleRate`) are
pre-declared, removing every drop of binding boilerplate; authors just read
them as if they were locals. Third, it's accidentally reactive: the thing is
re-invoked at video rate, and because the function is stateless the author
never has to think about lifetime — just the equation for this pixel at this
time. (Source: [Shadertoy how-to](https://www.shadertoy.com/howto),
[WebGL Fundamentals: Shadertoy](https://webglfundamentals.org/webgl/lessons/webgl-shadertoy.html).)

ISF, the Interactive Shader Format, takes Shadertoy's ergonomic core and
wires host UI to it. An ISF shader is a standard GLSL fragment shader with a
**JSON blob prepended as a comment**; the JSON declares `INPUTS` (typed:
`float`, `bool`, `long` for pop-ups, `color`, `point2D`, `event`, `image`,
`audio`, `audioFFT`), plus optional `PASSES` with `PERSISTENT` and `FLOAT`
flags for multi-pass with feedback. The host reads that JSON, auto-declares
a `uniform` for each input, auto-generates controls, and wires FFT/waveform
inputs into sampler slots. The tight binding between schema and GPU uniform
is what makes ISF feel magical: you write one declaration, you get the slider,
the uniform, the automation path, and the preset-save pipe for free.
(Sources: [ISF JSON Reference](https://docs.isf.video/ref_json.html),
[ISF Spec README](https://github.com/mrRay/ISF_Spec/blob/master/README.md).)

```glsl
/*{
    "DESCRIPTION": "Audio-reactive rainbow wave",
    "INPUTS": [
        { "NAME": "speed",     "TYPE": "float", "DEFAULT": 1.0,  "MIN": 0.1, "MAX": 4.0 },
        { "NAME": "intensity", "TYPE": "float", "DEFAULT": 0.6,  "MIN": 0.0, "MAX": 1.0 },
        { "NAME": "audioFFT",  "TYPE": "audioFFT", "MAX": 64 }
    ]
}*/
void main() {
    vec2 uv = gl_FragCoord.xy / RENDERSIZE.xy;
    float bass = IMG_NORM_PIXEL(audioFFT, vec2(0.05, 0.5)).r;
    float hue  = fract(uv.x + TIME * speed * 0.1);
    gl_FragColor = vec4(hsv2rgb(vec3(hue, 1.0, intensity + bass)), 1.0);
}
```

**Call overhead in a CPU context.** This is the quiet catch. Shadertoy's
per-pixel model is cheap because the GPU dispatches millions of fragment
invocations in parallel with amortized setup. On CPU, calling a Rust
function once per pixel for a 640×480 canvas is 307,200 calls per frame; at
60 Hz that's 18.4 M calls/sec. An empty Rust fn call crosses into nanoseconds
easily, but a WASM host↔guest boundary crossing is ~10 ns on Wasmtime (per the
[Bytecode Alliance Cranelift 2023 update](https://bytecodealliance.org/articles/wasmtime-and-cranelift-in-2023)),
so calling a guest-exported per-pixel callback at 18.4 M/sec would burn
~184 ms/sec on the boundary alone — **three frames entirely lost just to call
overhead**, before any pixel math runs. This is why *every CPU LED engine that
thinks it wants a per-pixel API actually wants a per-frame API*. See §10.

The per-pixel *conceptual* model is still worth preserving. Rust's iterator
chain over `canvas.pixels_mut()` keeps the authorial feel of Shadertoy while
collapsing to one WASM-host roundtrip per frame.

---

## 2 · VST3 / CLAP: audio plugin process callbacks

**VST3** is C++-flavored COM. The plugin ships an `IAudioProcessor::process()`
which receives a `ProcessData` struct bundling `numInputs`, `numOutputs`,
`inputs`/`outputs` as nested buffer arrays, `inputParameterChanges`, and
`numSamples`. Parameter changes arrive as an `IParameterChanges` queue where
each `IParamValueQueue` holds a sequence of `(sampleOffset, normalizedValue)`
points — sample-accurate automation baked into the frame. The `IEditController`
is a *separate component* from the `IAudioProcessor`, with its own parameter
representation; the bridge between them is the `ParamID` integer. Parameter
flags include `kCanAutomate`, `kIsReadOnly`, `kIsList`, `kIsProgramChange`,
`kIsBypass`, and `kIsHidden`. (Sources:
[VST3 Parameters & Automation](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical+Documentation/Parameters+Automation/Index.html),
[VST3 parameter flow article](https://dev.classmethod.jp/en/articles/vst3-plugin-parameter-flow-again-processdata-inputparameterchanges/).)

This is elegant on a whiteboard and brutal in a codebase. Two components,
two parameter snapshots, two object hierarchies, a marshaling queue — the DX
reality is that plugin devs spend weeks understanding parameter ID/queue/
sampleOffset semantics before writing a useful synth.

**CLAP** is the reaction. Single C header, `clap_plugin_params` with
`get_info`/`get_value`/`value_to_text`/`text_to_value`/`flush`, and a single
event-queue-driven `process()` where parameter updates, MIDI, and notes
arrive in one time-sorted `clap_input_events` stream. The `clap_param_info`
struct carries `min_value`, `max_value`, `default_value`, a display name, a
module path for grouping, and a cookie for fast host/plugin lookups. One
header, one ABI, every feature is a named extension. (Sources:
[free-audio/clap](https://github.com/free-audio/clap),
[clap/ext/params.h](https://github.com/free-audio/clap/blob/main/include/clap/ext/params.h),
[u-he on CLAP](https://u-he.com/community/clap/),
[Sweetwater CLAP overview](https://www.sweetwater.com/insync/clap-the-new-clever-audio-plug-in-format/).)

**NIH-plug** (Rust) is where this story crystallizes into real elegance. Look
at a *gain plugin* top-to-bottom:

```rust
#[derive(Params)]
struct GainParams {
    #[id = "gain"]
    gain: FloatParam,
}

impl Default for GainParams {
    fn default() -> Self {
        Self {
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(30.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 30.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" dB"),
        }
    }
}

impl Plugin for MyGain {
    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        for channel_samples in buffer.iter_samples() {
            let g = self.params.gain.smoothed.next();
            for sample in channel_samples { *sample *= g; }
        }
        ProcessStatus::Normal
    }
}
```

(Source: [nih-plug Plugin trait](https://nih-plug.robbertvanderhelm.nl/nih_plug/plugin/trait.Plugin.html),
[nih-plug README](https://github.com/robbert-vdh/nih-plug).)

Things to steal:
- **Parameters are struct fields.** `#[derive(Params)]` reads the types and
  `#[id = "..."]` attrs and emits the entire CLAP/VST3 metadata story. The
  author never writes JSON or matches on control names by string.
- **Smoothers are per-parameter.** Declared at construction, pulled at use.
  `.with_smoother(SmoothingStyle::Logarithmic(50.0))` covers 90% of audio
  pop-click problems in one line.
- **Units, display, formatters are fluent.** `.with_unit(" dB")`,
  `.with_value_to_string(...)`, `.with_string_to_value(...)` are
  *behavior on the parameter*, not duplicated state in the GUI.
- **`process()` is one callback with one `ProcessStatus` return.** No
  separate "have params changed" check — smoothers pull the queue.

What's still painful: VST3/CLAP both inherit from audio where buffers are
always `&mut [f32]` and the "canvas" is 1D time. For a 2D RGBA canvas the
shape is different, but the *declarative parameter pattern* is directly
portable.

---

## 3 · Resolume FFGL / ISF plugins: visual-effects plugin model

FFGL is a C++ plugin API with two moving parts: the parameter system managed
by `CFreeFrameGLPlugin::AddParam(Param::Create("Opacity"))` (plus typed
variants `ParamColor::Create`, `ParamOption::Create` for enums, etc.), and
a `ProcessOpenGL(ProcessOpenGLStruct*)` render callback that receives bound
input textures and writes to a bound output FBO. Parameter names *must match*
fragment shader uniform names; `SendParams(shader)` copies the current values
to all declared uniforms in one call. Host-side features include parameter
groups via `SetParamGroup` and per-frame uniforms `resolution`, `time`,
`deltaTime`, `frame`, `bpm`, `phase`. (Sources:
[resolume/ffgl](https://github.com/resolume/ffgl),
[FFGL framework wiki](https://github.com/resolume/ffgl/wiki/3.-Get-to-know-the-framework-better),
[Add.cpp plugin example](https://github.com/resolume/ffgl/blob/master/source/plugins/Add/Add.cpp).)

```cpp
Add::Add() {
    SetMinInputs(2); SetMaxInputs(2);
    AddParam(Param::Create("Opacity"));
}
FFResult Add::ProcessOpenGL(ProcessOpenGLStruct* pGL) {
    ScopedShaderBinding sb(shader.GetGLID());
    /* bind textures */
    SendParams(shader);
    quad.Draw();
    return FF_SUCCESS;
}
```

The lesson here is subtle: **parameter-to-uniform naming by convention** is a
lot of ergonomic mileage. The author writes `Opacity` once as a parameter name
*and* once as a shader uniform name; the host reconciles. No manual wiring
code. This is the pattern ISF codifies declaratively.

The downside: FFGL is render-per-frame with no state accessor — persistent
buffers require a separate ping-pong-texture dance. Resolume's newer engine
also supports ISF natively, so ISF is effectively the preferred authoring
surface now.

---

## 4 · SignalRGB LightScripts: the webpage-as-effect model

SignalRGB effects are literally HTML pages. The `<head>` holds `<meta>` tags
that declare UI controls and effect metadata; the `<body>` has a
`<canvas width="320" height="200">`; the `<script>` runs a `render()` loop
(typically via `requestAnimationFrame`). The engine exposes globals:
`engine.audio.level` (−100..0 dB), `engine.audio.density` (0..1 tonal
roughness), and `engine.audio.freq[]` (200-element FFT). Per-LED sampling is
handled *by the engine*, not the effect: the engine reads pixel colors off
your `<canvas>` at the device's LED positions and forwards them to the
hardware. The effect author's mental model is "paint a 320×200 canvas; the
lighting happens to someone else." (Sources:
[SignalRGB LightScript intro](https://docs.signalrgb.com/developer/lightscripts/it-s-a-webpage/),
[Audio Visualizer tutorial](https://docs.signalrgb.com/developer/lightscripts/audio-visualizer/),
[device.color API](https://docs.signalrgb.com/developer/plugins/device-functions/),
[HTML5+JS overview](https://docs.signalrgb.com/developer/lightscripts/html5-js/).)

```html
<meta description="Audio Rainbow" publisher="Me" />
<meta property="slider" name="Speed" type="number"
      default="1.0" min="0.1" max="4.0" />
<canvas id="c" width="320" height="200"></canvas>
<script>
  const ctx = document.getElementById('c').getContext('2d');
  function render() {
    const bass = Math.abs(engine.audio.freq[5]) / 100;
    const hue  = (performance.now() * 0.05) % 360;
    ctx.fillStyle = `hsl(${hue}, 100%, ${30 + bass * 50}%)`;
    ctx.fillRect(0, 0, 320, 200);
    requestAnimationFrame(render);
  }
  render();
</script>
```

**Lightweight about this model:** the author uses the most known-in-the-world
canvas API on Earth. No SDK to install. No compilation. Hot reload is the
browser's DevTools. Audio is three named globals. If you can build a 200-line
canvas demo in a 2012 Chrome, you can build a SignalRGB effect.

**Clunky about this model:** metadata lives in `<meta>` tag attribute strings,
which means control types are stringly-typed and opaque to tooling. Per-LED
spatial awareness requires reading back from the canvas — an implicit
contract that the engine never actually tells the effect "here are your 144
LEDs at these positions." The effect paints and hopes. This is fine for
ambient stuff, terrible for anything wanting to know *where* a keyboard key
is in the layout. (Hypercolor already dodges this problem by generating a
logical 320×200 canvas and letting the spatial sampler map pixels to LEDs.
The SignalRGB model is, in essence, already how the daemon thinks.)

---

## 5 · OBS Studio video filters: effect-file + C-callback split

OBS filter plugins split into: a C source file implementing
`obs_source_info` (lifecycle, properties, render callback), and an `.effect`
file containing HLSL-syntax shaders. The `video_render` callback calls
`obs_source_process_filter_begin(...)`, sets uniforms on the loaded
`gs_effect_t*`, then calls `obs_source_process_filter_end(effect, w, h)`.
Uniform declarations live in the `.effect` file:
`uniform float4 color = {1,1,1,1};`. (Sources:
[OBS Rendering Graphics](https://docs.obsproject.com/graphics),
[obs_source_info reference](https://docs.obsproject.com/reference-sources),
[exeldro/obs-shaderfilter](https://github.com/exeldro/obs-shaderfilter).)

```c
static void filter_video_render(void *data, gs_effect_t *fx) {
    if (!obs_source_process_filter_begin(ctx->source, GS_RGBA,
                                          OBS_ALLOW_DIRECT_RENDERING))
        return;
    gs_effect_set_float(ctx->strength_param, ctx->strength);
    obs_source_process_filter_end(ctx->source, ctx->effect, 0, 0);
}
```

Bureaucratic. Three parallel metadata trees: `obs_properties_t` (for the UI),
the `.effect` file's `uniform` declarations, and C struct members for
runtime cache — plus a "look up each uniform by name once on load" ritual
(`gs_effect_get_param_by_name`). The shader-filter community plugin papers
over the worst of it by parsing the `.effect` file to auto-generate the
OBS properties list, which is exactly what ISF does natively. **Avoid the
three-place metadata problem.**

---

## 6 · TouchDesigner: dataflow operators

Every pixel, every channel, every geometry point in TouchDesigner is a node
in an operator graph. TOPs are 2D image ops, CHOPs are channel/time-series
ops, SOPs are geometry, DATs are tables/text. Custom operators are C++
plugins that implement `FillTOPPluginInfo(TOP_PluginInfo*)` and a per-frame
`execute(TOP_Context*, const TOP_Input*, ...)` that writes to an output
texture. (Sources:
[TouchDesigner custom ops](https://docs.derivative.ca/Custom_Operators),
[CPlusPlus TOP](https://docs.derivative.ca/CPlusPlus_TOP),
[Operator families](https://docs.derivative.ca/Operator).)

Each operator declares:
- a `setupParameters(OP_ParameterManager*)` that registers typed parameters
  (numeric, menu, pulse, toggle) — these auto-generate the inspector UI;
- `getInputInfo(...)` describing input channel layout;
- `execute(...)` where the actual work happens.

The brilliance is composition: any TOP output can be wired into any TOP
input, and the Python scripting layer can rebind parameters at runtime. The
clunkiness is cognitive load — a beginner meets the entire operator
taxonomy before producing a visible pixel.

For Hypercolor this is overkill. We have *one* output target (the canvas)
and we already have post-processing (spatial sampler, brightness, color
correction). A full node graph is a separate product. But the lesson — a
single op's internal API should look the same whether you're a novice or
composing twenty of them — matters. §11 Shape C explores a lightweight
composition model.

---

## 7 · OpenRGB plugins: lessons in what not to do

OpenRGB plugin API is Qt-based, C++-only, dynamically loaded `.so`/`.dll`s,
and requires the plugin to build against the exact OpenRGB source tree
(headers live in the main repo, there's no stable SDK crate). Effects are
implemented as Qt widgets that draw into per-device color buffers via the
`RGBController*` API. (Sources:
[OpenRGB plugin page](https://openrgb.org/plugins.html),
[OpenRGB Effects Plugin](https://openrgb.org/plugin_effects.html),
[OpenRGB on GitLab](https://gitlab.com/CalcProgrammer1/OpenRGB).)

What this produces in practice:
- **Plugin ABI breaks every minor release** because Qt symbols are not
  stable across C++ compiler/stdlib combinations.
- **No sandboxing.** Plugin crashes take the daemon with them.
- **The UI and the effect are the same code.** Want headless LED control?
  Rip out half the plugin. Want web-based UI? Re-implement.
- **Discovery is OS-native dynamic loading.** No integrity checks, no
  manifests, no sane deny-by-default permissions.

Hypercolor already solved most of these structurally: the UI is Leptos
over REST/WebSocket, effects are data-driven, and plugins *will* run in
WASM. The single biggest lesson from OpenRGB: **never make "compiled
against exact host version" the plugin distribution story.** WASM's
stable-by-construction ABI is a gift we should protect fiercely; the
plugin API should use only WIT types or a narrow C-ABI, never Rust types
that drift release-to-release.

---

## 8 · Bevy: a modern Rust render graph with derive macros

Bevy's material story converged on `AsBindGroup` derive. A custom material
is:

```rust
#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
struct CoolMaterial {
    #[uniform(0)] color: LinearRgba,
    #[texture(1)] #[sampler(2)] color_texture: Option<Handle<Image>>,
}

impl Material2d for CoolMaterial {
    fn fragment_shader() -> ShaderRef { "shaders/cool.wgsl".into() }
}
```

(Sources:
[PR #5053 AsBindGroup derive](https://github.com/bevyengine/bevy/pull/5053),
[Material2d docs](https://docs.rs/bevy/latest/bevy/sprite_render/trait.Material2d.html),
[Bevy Rendering cheat book](https://bevy-cheatbook.github.io/gpu/intro.html).)

The bind-group derive is the same pattern NIH-plug's `Params` derive uses:
the compiler reads field attrs, emits the schema and the wire code. The
author writes types, the framework emits infrastructure. Bevy's
`#[uniform(0)]`/`#[texture(1)]` is *more magical* than NIH-plug's
`#[id = "foo"]` because binding slot numbers are a shader concern, but the
underlying philosophy is identical: **the struct is the schema.**

Bevy also separates render-graph construction from per-material code.
Materials declare shaders and bindings; render-graph nodes stitch them
into phases. For Hypercolor, we don't need a full graph — a single "effect
renders into one canvas, daemon does the rest" node is enough — but
separating "what this effect computes" from "where it goes next" is a
design principle worth preserving for future compositing.

---

## 9 · WLED, Hyperion, AuroraRGB: our neighbors

**WLED** authoring is C++ in-tree. Effects are plain functions
`uint16_t mode_blink(void)` registered with
`strip.addEffect(id, &mode_blink, _data_FX_MODE_BLINK)`. The metadata string
is a cryptic single-line grammar: `!,!;;!;1;sx=24,pal=50` decodes to
"two standard sliders, no color slots, one-dimensional, palette 50". Effects
access a global `strip` object and use `SEGMENT.speed`, `SEGMENT.intensity`,
`SEGCOLOR(0)` helpers. Everything compiles into the firmware; "plugins"
mean rebuild and reflash. (Sources:
[WLED Custom Features](https://kno.wled.ge/advanced/custom-features/),
[WLED JSON API](https://kno.wled.ge/interfaces/json-api/),
[Usermod system](https://deepwiki.com/wled/WLED/6-usermod-system).)

**Hyperion** effects are Python scripts. A minimal one:

```python
import hyperion, time
color = bytearray(hyperion.args.get('color', [255, 0, 0]))
while not hyperion.abort():
    hyperion.imageLinearGradient(0, 0, hyperion.imageWidth(), 0,
                                  bytearray([0, 255, 0, 0,
                                             255, 0, 255, 0]), 1)
    hyperion.imageShow()
    time.sleep(0.05)
```

The `hyperion` module exposes `setColor`, `imageLinearGradient`,
`imageRadialGradient`, `imageConicalGradient`, `imageDrawLine`,
`imageDrawRect`, `imageDrawPie`, `imageSetPixel`, `imageShow`,
transforms (`imageCRotate`, `imageCOffset`, `imageCShear`), and an
`abort()` poll. Arguments arrive through `hyperion.args` dict; metadata
lives in a separate `.json` sidecar. (Source:
[Hyperion effect functions](https://docs.hyperion-project.org/effects/Functions.html),
[Our First Effect](https://docs.hyperion-project.org/effects/OurFirstEffect.html).)

What Hyperion gets right: **the effect is a loop, not a frame callback.**
The author writes imperative code with `while not abort(): ... sleep(dt)`,
which mirrors how beginners actually think about animation. The effect
*owns* its timing. What it gets wrong: sidecar JSON, no type safety, Python
runtime cost, and immediate-mode drawing API that mixes coordinate math
with color data (bytes interleaved `r,g,b,a`).

**AuroraRGB** (antonpup/Aurora) takes a different shape: effects are *layers*
in a stack, and each layer is either a built-in effect type or a "Scripted
Layer" that hosts C# or Python scripts implementing an `Update()` method
against a `Canvas` object. The layer system is an implicit compositor — the
*stack* composes. (Sources:
[Aurora GitHub](https://github.com/antonpup/Aurora),
[Aurora Script Layer docs](https://www.project-aurora.com/Docs/reference-layers/script/).)

The neighbors converge: **a frame callback that writes into a 2D buffer, with
typed controls declared declaratively, is the universal shape.** Hypercolor
already has this with `EffectRenderer::render_into(input, canvas)`. The
question for WASM is how to keep that shape while making the guest code
beautiful.

---

## 10 · Core axes for evaluating any effect API

| Axis | Options | Hypercolor's need |
|------|---------|-------------------|
| Granularity | per-pixel · per-canvas · per-LED | per-canvas (spatial sampler already maps canvas→LEDs) |
| State | stateless (Shadertoy) · stateful (VST) | stateful (framebuffer feedback, particles, beat smoothers) |
| Schema discovery | runtime JSON · compile-time derive · sidecar file | compile-time derive (no drift, no parsing) |
| Hot reload | file-watch rebuild · swap binary · in-place | swap WASM module on file change |
| Input injection | push (engine writes into input struct) · pull (effect asks) | push — keeps effect-side code branchless |
| Composition | single effect · effect chain · node graph | single effect now, chain later |
| Failure model | crash host · sandbox + restart · skip frame | sandbox + skip frame, then restart on repeat |
| Audio model | raw buffer · FFT bins · semantic (beat, bpm, onsets) | semantic, already provided by `AudioData` |
| Timing | host pushes `dt` · guest calls `now()` · sample-accurate | host pushes `dt` (WASM has no monotonic clock guarantee) |

**Key observation about granularity.** Per-pixel on CPU is a 10 ns×N=ms tax
per frame at WASM boundary rates — unworkable. Per-LED is what hardware
*cares* about but resolution-dependent — a 144-LED strip and a 54-key keyboard
need different code. **Per-canvas at normalized resolution is the only one
that scales.** The per-pixel *feel* is recoverable with iterator-style APIs
(`canvas.par_pixels_mut(|(x, y), px| { ... })`) that loop entirely inside the
guest.

**Key observation about schema discovery.** Runtime JSON (ISF, SignalRGB
`<meta>`, WLED fxdata strings) forces a parallel maintenance burden: shader
variables must match JSON names must match the UI. Compile-time derive
(NIH-plug, Bevy) collapses all three into struct fields. For Rust guests
with proc-macros, derive wins on every axis; the *only* reason to prefer
JSON is if you don't control the compiler, which for WASM we do.

**Key observation about composition.** Bevy, TouchDesigner, Resolume all
support effect chaining. Hypercolor does not today. The right move is to
ship v1 as single-effect and design the API so chaining is additive
(effects read a `previous: Option<&Canvas>` in the frame input, return
their own canvas as usual). Premature compositing bloats the core API.

---

## 11 · Three candidate API shapes

All three target a `rainbow wave with audio-reactive intensity` on a 320×200
RGBA canvas at ~60 fps. All three are real Rust code that would compile
against a WIT-generated guest binding assuming standard types
(`Canvas`, `Frame`, `AudioData`). Aliases shorten the noise.

### Shape A — "derive-the-contract" (Rust-first, recommended)

The effect *is* a Rust struct. A `#[derive(Effect)]` macro reads the struct
and generates the WIT-exported component entry points, the parameter schema,
the persistence shape, and a default `new()` built from parameter defaults.
The author writes a single `render` fn that iterates the canvas inline.

```rust
use hypercolor_effect_sdk::prelude::*;

#[derive(Effect)]
#[effect(
    name    = "Rainbow Wave",
    author  = "Bliss",
    version = "0.1.0",
    audio_reactive,
)]
struct RainbowWave {
    #[param(label = "Speed",     default = 1.0,  range = 0.1..=4.0)]
    speed: f32,
    #[param(label = "Intensity", default = 0.6,  range = 0.0..=1.0)]
    intensity: f32,
    #[param(label = "Hue Shift", default = 0.0,  range = 0.0..=360.0, unit = "deg")]
    hue_shift: f32,
    #[param(label = "Direction", default = Direction::Right)]
    direction: Direction,
}

#[derive(Param, Default, Copy, Clone)]
enum Direction { #[default] Right, Left, Up, Down }

impl Render for RainbowWave {
    fn render(&mut self, f: &Frame, canvas: &mut Canvas) {
        let bass   = f.audio.bass_energy();     // helper on AudioData
        let boost  = 1.0 + bass * self.intensity * 2.0;
        let t      = f.time_secs * self.speed;
        let shift  = self.hue_shift / 360.0;

        canvas.fill_with(|u, v| {
            let axis = match self.direction {
                Direction::Right => u, Direction::Left => 1.0 - u,
                Direction::Down  => v, Direction::Up   => 1.0 - v,
            };
            let hue = (axis + t * 0.1 + shift).fract();
            oklch(0.75 * boost.min(1.25), 0.18, hue * 360.0)
        });
    }
}

export_effect!(RainbowWave);
```

**Lines that actually do work: 18.** Parameter schema, metadata, WIT
exports, state persistence — all compiler-emitted from attrs. No JSON. No
separate registration table. Audio is a typed struct with semantic
helpers. Spatial indices are normalized `[0,1]` floats, resolution-free.

**Elegance.** Highest. The *only* things on the page are (a) what this
effect is (attrs), (b) what it exposes (param fields), (c) what it does
(`render`). Idiomatic Rust: derive macros everywhere, strong types, enum
params, `fill_with` closure maps to the Shadertoy feeling without the
boundary cost.

**Performance.** Best. Zero host/guest boundary crossings inside `render`;
the closure compiles to inline SIMD-friendly code in the guest. Parameters
are plain `f32`/enum fields, no string lookups. Audio data is handed in
once as a borrow; inline helpers (`bass_energy()`) are monomorphized in
the guest. Expected frame cost for this effect at 320×200: **<0.3 ms**.

**Flexibility.** Highest for Rust guests. Enum params auto-serialize.
Nested groups work (`#[param(group = "Motion")] speed`). Advanced users can
opt out with `#[derive(Effect)] #[effect(manual_params)]` and hand-roll the
param table. Cost: **Rust-only at first**. AssemblyScript/Zig guests would
need a parallel (and uglier) API — see the recommendation's AS/Zig
footnote.

**Trade-off.** Ties the guest toolchain to a Rust proc-macro. Mitigation:
the generated code targets a stable WIT world, so non-Rust guests can
still implement the same world by hand with a more verbose API. The
proc-macro is pure polish, not load-bearing for compatibility.

### Shape B — "Shadertoy on CPU" (per-canvas closure flavor)

A single `mainImage`-style function receives a pre-allocated canvas and a
`Frame` bundle with all inputs. No proc-macros: controls are a const array
exported at module init. Closest to ISF/Shadertoy feel.

```rust
use hypercolor_effect_sdk::prelude::*;

const CONTROLS: &[Control] = &[
    Control::float("speed",     "Speed",     1.0, 0.1..=4.0),
    Control::float("intensity", "Intensity", 0.6, 0.0..=1.0),
    Control::enm  ("direction", "Direction", "right",
                   &["right", "left", "up", "down"]),
];

#[no_mangle]
pub extern "C" fn metadata() -> Metadata {
    Metadata {
        name: "Rainbow Wave", version: "0.1.0",
        author: "Bliss", audio_reactive: true,
        controls: CONTROLS,
    }
}

#[no_mangle]
pub extern "C" fn render(f: &Frame, canvas: &mut Canvas) {
    let speed     = f.controls.float("speed");
    let intensity = f.controls.float("intensity");
    let boost     = 1.0 + f.audio.bass_energy() * intensity * 2.0;
    let t         = f.time_secs * speed;

    canvas.fill_with(|u, v| {
        let axis = match f.controls.enm("direction") {
            "left" => 1.0 - u, "up" => 1.0 - v, "down" => v, _ => u,
        };
        let hue = (axis + t * 0.1).fract();
        oklch(0.75 * boost.min(1.25), 0.18, hue * 360.0)
    });
}
```

**Elegance.** Good. Familiar to anyone who's written an ISF shader. No
macros to debug. Controls are a const array — grep-able, readable.

**Performance.** Same as Shape A for the render path. Slightly worse for
parameter reads because we look up by name string (`controls.float("speed")`),
though a monomorphizable `Controls` with interned indices can fix that.
Expected overhead: **a few hundred ns/frame**, lost in the noise.

**Flexibility.** Good. Language-agnostic — AssemblyScript/Zig implement the
same two functions with the same signatures. Controls array can be grown
without touching `render`. Enums are stringly-typed; the compiler doesn't
catch typos in `"left"/"right"`.

**Trade-off.** String keys for params is the wart. It's the cost of being
language-agnostic at the ABI. Also: state lives where? We'd need a third
exported `state_alloc()/state_free()` pair, or a WIT `resource` type. Doable
but noisier than Shape A.

### Shape C — "typed channels" (compositional flavor)

The effect is a pipeline. Each stage is a named function with typed inputs
and outputs; the framework runs them in declaration order and threads a
canvas through. Stages can be reordered or muted by the host. Parameters
attach to stages.

```rust
use hypercolor_effect_sdk::prelude::*;

#[effect(name = "Rainbow Wave", audio_reactive)]
mod rainbow_wave {
    #[param(default = 1.0, range = 0.1..=4.0)] static SPEED: f32;
    #[param(default = 0.6, range = 0.0..=1.0)] static INTENSITY: f32;

    #[stage]
    fn base(f: &Frame, out: &mut Canvas) {
        let t = f.time_secs * *SPEED;
        out.fill_with(|u, _| oklch(0.75, 0.18,
            ((u + t * 0.1).fract()) * 360.0));
    }

    #[stage(after = "base")]
    fn audio_boost(f: &Frame, io: &mut Canvas) {
        let boost = 1.0 + f.audio.bass_energy() * *INTENSITY * 2.0;
        io.map_rgb(|r, g, b| (r * boost, g * boost, b * boost));
    }
}
```

**Elegance.** Attractive for composition-heavy effects. Ugly for simple ones
— our two-stage rainbow is more code than Shape A's single closure.
`static SPEED: f32` feels wrong; the macro magics a thread-safe accessor,
but readers rightly wince.

**Performance.** Slight cost: each stage is a function, the framework calls
them sequentially, and internal canvas reads between stages are extra
memory traffic unless the compiler fuses. Expected: **~0.5 ms vs ~0.3 ms**
for Shape A.

**Flexibility.** Highest for composition. The host can reorder stages,
mute them, or hoist one to a later global pass. Maps naturally to a future
node-graph UI.

**Trade-off.** Too much framework for v1. Shape A plus a future
"post-process stack" daemon feature gives 80% of the composition benefit
without asking every effect author to think in stages.

### Shape D — "imperative loop" (Hyperion flavor)

The effect is a long-running entry point that polls `abort()` and drives
its own timing. Most "stateful and weird" of the shapes.

```rust
#[no_mangle]
pub extern "C" fn run(host: &mut Host) {
    let speed     = host.param_f32("speed",     1.0, 0.1, 4.0);
    let intensity = host.param_f32("intensity", 0.6, 0.0, 1.0);
    let mut canvas = host.canvas();
    let mut t = 0.0;
    while !host.abort() {
        let f = host.next_frame();
        t += f.delta_secs * *speed;
        let boost = 1.0 + f.audio.bass_energy() * *intensity * 2.0;
        canvas.fill_with(|u, _| {
            let hue = (u + t * 0.1).fract();
            oklch(0.75 * boost.min(1.25), 0.18, hue * 360.0)
        });
        host.present(&canvas);
    }
}
```

**Elegance.** Beginner-friendly. The effect reads top-to-bottom like a
program. No callback inversion — the author controls the loop.

**Performance.** Worst of the four. `host.next_frame()` crosses the
boundary per frame (not per pixel, so still cheap at ~10 ns), and
`host.present()` adds another crossing. Worse: the host must cooperatively
schedule the guest, meaning a stack switch or a dedicated WASM instance
per effect. This conflicts with Hypercolor's render-loop architecture.

**Flexibility.** Great for effects that want explicit multi-phase state
machines (sparkle bursts, keyboard ripples with manual delay loops).

**Trade-off.** Architectural impedance mismatch. Hypercolor's render loop
is host-driven (see `crates/hypercolor-core/src/effect/traits.rs`:
`render_into` is called *to* the effect, not *from* it). Supporting
`run()` would require a fiber/coroutine harness for every effect. Not
worth it for the 5% of effects that want it — those effects can implement
the same pattern inside Shape A with a state machine field.

---

## 12 · Recommendation

**Ship Shape A.** "Derive-the-contract" is the right shape for Hypercolor
specifically because every design pressure points at it:

- **Hypercolor is Rust-first.** Every authoring surface in this project —
  native effects, daemon routes, UI, SDK — is either Rust or TypeScript.
  WASM guests will overwhelmingly be Rust. A Rust-first API lets us
  co-evolve the proc-macro with the codebase without marshaling through
  JSON.
- **The existing `EffectRenderer` trait is already Shape A in spirit.**
  `render_into(input, canvas)` with `set_control(name, value)` is the
  primitive; we're just making it prettier for the WASM side. Built-in
  effects and WASM effects speak the same contract, which means the
  daemon's `EffectPool` and spatial sampler don't branch on "is this
  WASM?" — they just consume a `dyn EffectRenderer` wrapper over the
  component. (See `crates/hypercolor-core/src/effect/pool.rs`.)
- **Hot reload is trivial.** WASM components are stable-ABI artifacts.
  `notify`-watch the `.wasm` file, swap the instance atomically, done.
  Because parameters are serialized by ID (`#[param]` generates stable
  IDs from field names), state survives reload; if field names change,
  we drop the stale values and re-default — familiar and predictable.
- **Audio/sensor/screen data is already semantic.** `AudioData` has
  `beat_detected`, `beat_phase`, `bass_energy()` affordances. Shape A
  surfaces those as-is, with the SDK adding ergonomic helpers. No
  competitor's API ships this rich an audio model — ISF gives you raw
  FFT bins, SignalRGB gives you three numbers. Ours is closer to what
  authors *want*.
- **The author-facing line count target is met.** The Shape A example
  above is 30 lines including the enum, imports, and export macro. A
  sparser effect (`Solid Color` with one slider) is 10 lines.
- **Strong typing catches the common mistakes.** Wrong range on a slider
  is a compile error, not a silent runtime clamp. Parameter rename
  breaks the build in the host *and* the guest with useful spans. An
  enum control with a typo in a preset doesn't match any variant and
  the compiler yells.

### The costs, honestly

1. **Rust-only ergonomics.** AssemblyScript or Zig guests can implement
   the same WIT world, but they'd do it by hand and the code would look
   more like Shape B. Acceptable: the proc-macro is polish, not ABI.
   We should ship Shape B as the *stable underlying target* of Shape A's
   macro expansion, so non-Rust guests have a clean story.
2. **Proc-macro maintenance burden.** The macro has to understand param
   attrs, enum `Param` derives, and emit WIT-compatible exports. That's
   a ~500-line crate with a test matrix. Not cheap, but it's the kind of
   code that, once right, doesn't move.
3. **Discoverability inside the IDE.** Macro-expanded code confuses
   rust-analyzer's "go to definition" on `canvas.fill_with`. Mitigation:
   ship the SDK as a regular library crate — `Canvas`, `Frame`,
   `oklch()`, everything the author sees — so rust-analyzer navigates
   into real source. The macro only generates the component
   plumbing, not the user-visible surface.
4. **WIT world needs versioning.** If we add a field to `Frame`
   (say, `wifi_rssi: f32`), old WASM binaries should keep working. The
   component model supports this with optional record fields; we have
   to be disciplined about never removing or renaming.

### Concretely, what to build first

1. **`hypercolor-effect-sdk` crate.** Pure-library types: `Frame`,
   `Canvas` (WASM-friendly wrapper over a shared linear-memory buffer),
   `AudioData`, helper math (`oklch`, `hsl`, `smoothstep`), zero unsafe.
   Re-exports of `ControlValue` types.
2. **`hypercolor-effect-macros` crate.** The `#[derive(Effect)]`,
   `#[derive(Param)]`, and `export_effect!` macros. Expand into WIT
   exports targeting `hypercolor:effect/v0.1`.
3. **Host side: `WasmEffectRenderer`.** Implements `EffectRenderer` as a
   thin wrapper over a wasmtime `Component` instance. Lives in
   `crates/hypercolor-core/src/effect/wasm/`. Use the existing
   `EffectRenderer` trait; the render loop doesn't need to know the
   difference.
4. **`examples/` in the SDK.** Rainbow Wave, Audio Pulse, Solid Color,
   Breathing, Color Wave — ports of the current native built-ins.
   If we can't port `color_wave.rs` to Shape A in under 200 lines total,
   the API has failed; we iterate. (Current `color_wave.rs` is ~935
   lines; most of it is preset data and boilerplate that the macro
   eliminates. I expect a clean port to land near 120 lines.)
5. **A "hello world" rainbow in ~30 lines** as the SDK README's opening
   example. If a reader can't go from `cargo new` to a working WASM
   effect in ten minutes, the story isn't done.

The full plan for *how* to load, sandbox, and watch these WASM modules
belongs in sibling research docs; this doc is only about the shape of the
code an author writes. That shape is Shape A.

---

## Sources

### Shadertoy / ISF
- [Shadertoy how-to](https://www.shadertoy.com/howto) — fragCoord semantics, uniform conventions.
- [WebGL Fundamentals: Shadertoy](https://webglfundamentals.org/webgl/lessons/webgl-shadertoy.html) — why mainImage is elegant.
- [Book of Shaders: uniforms](https://thebookofshaders.com/03/) — uniform-first authoring.
- [ISF JSON Reference](https://docs.isf.video/ref_json.html) — INPUT types, PASSES, PERSISTENT flags.
- [ISF Spec README](https://github.com/mrRay/ISF_Spec/blob/master/README.md) — end-to-end format spec.

### CLAP / VST3 / NIH-plug
- [free-audio/clap repository](https://github.com/free-audio/clap) — CLAP headers and template.
- [clap/ext/params.h](https://github.com/free-audio/clap/blob/main/include/clap/ext/params.h) — `clap_param_info` struct.
- [u-he on CLAP](https://u-he.com/community/clap/) — KISS design philosophy.
- [Sweetwater CLAP overview](https://www.sweetwater.com/insync/clap-the-new-clever-audio-plug-in-format/).
- [VST3 Parameters & Automation](https://steinbergmedia.github.io/vst3_dev_portal/pages/Technical+Documentation/Parameters+Automation/Index.html).
- [VST3 parameter flow article](https://dev.classmethod.jp/en/articles/vst3-plugin-parameter-flow-again-processdata-inputparameterchanges/).
- [nih-plug repository](https://github.com/robbert-vdh/nih-plug) — Rust CLAP/VST3 framework.
- [nih-plug Plugin trait docs](https://nih-plug.robbertvanderhelm.nl/nih_plug/plugin/trait.Plugin.html).
- [nih-plug Params trait docs](https://nih-plug.robbertvanderhelm.nl/nih_plug/params/trait.Params.html).

### Resolume / FFGL / SignalRGB
- [resolume/ffgl repository](https://github.com/resolume/ffgl).
- [FFGL framework wiki](https://github.com/resolume/ffgl/wiki/3.-Get-to-know-the-framework-better).
- [FFGL Add.cpp example](https://github.com/resolume/ffgl/blob/master/source/plugins/Add/Add.cpp).
- [SignalRGB LightScript intro](https://docs.signalrgb.com/developer/lightscripts/it-s-a-webpage/).
- [SignalRGB Audio Visualizer tutorial](https://docs.signalrgb.com/developer/lightscripts/audio-visualizer/).
- [SignalRGB HTML5+JS overview](https://docs.signalrgb.com/developer/lightscripts/html5-js/).
- [SignalRGB device functions](https://docs.signalrgb.com/developer/plugins/device-functions/).

### OBS / TouchDesigner / Bevy / OpenRGB
- [OBS Rendering Graphics docs](https://docs.obsproject.com/graphics).
- [OBS source API reference](https://docs.obsproject.com/reference-sources).
- [exeldro/obs-shaderfilter](https://github.com/exeldro/obs-shaderfilter).
- [TouchDesigner Custom Operators](https://docs.derivative.ca/Custom_Operators).
- [CPlusPlus TOP reference](https://docs.derivative.ca/CPlusPlus_TOP).
- [Bevy Material2d docs](https://docs.rs/bevy/latest/bevy/sprite_render/trait.Material2d.html).
- [Bevy AsBindGroup PR #5053](https://github.com/bevyengine/bevy/pull/5053).
- [Bevy Rendering cheat book](https://bevy-cheatbook.github.io/gpu/intro.html).
- [OpenRGB plugin page](https://openrgb.org/plugins.html).
- [OpenRGB Effects Plugin](https://openrgb.org/plugin_effects.html).
- [OpenRGB on GitLab](https://gitlab.com/CalcProgrammer1/OpenRGB).

### WLED / Hyperion / AuroraRGB
- [WLED Custom Features](https://kno.wled.ge/advanced/custom-features/).
- [WLED JSON API](https://kno.wled.ge/interfaces/json-api/).
- [WLED usermod system](https://deepwiki.com/wled/WLED/6-usermod-system).
- [Hyperion Our First Effect](https://docs.hyperion-project.org/effects/OurFirstEffect.html).
- [Hyperion effect functions reference](https://docs.hyperion-project.org/effects/Functions.html).
- [Aurora GitHub](https://github.com/antonpup/Aurora).
- [Aurora Script Layer docs](https://www.project-aurora.com/Docs/reference-layers/script/).

### WASM runtime / Component Model
- [Wasmtime 1.0 performance](https://bytecodealliance.org/articles/wasmtime-10-performance).
- [Wasmtime and Cranelift in 2023](https://bytecodealliance.org/articles/wasmtime-and-cranelift-in-2023) — ~10 ns host-call overhead figure.
- [wasmtime::component Rust docs](https://docs.wasmtime.dev/api/wasmtime/component/index.html).
- [Component Model plugin walkthrough](https://tartanllama.xyz/posts/wasm-plugins/).
- [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen).
- [Component Model 2026 cheat sheet](https://techbytes.app/posts/wasm-component-model-cheat-sheet/).
- [Extism Rust PDK](https://github.com/extism/rust-pdk).
