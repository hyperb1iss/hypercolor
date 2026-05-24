# Dynamic logo tinting

The masks in this directory let you render the Hypercolor logo in any color
at runtime, so the brand can breathe with the active scene's actual lighting.

## Files

| File | What it is | Best for |
|---|---|---|
| `mark-mask.png` | Petals only, white-on-black grayscale | tinting the whole trinity one color |
| `wordmark-mask.png` | Wordmark only, white-on-black | tinting just the wordmark |
| `lockup-vertical-mask.png` | Full mark + wordmark | tinting the whole vertical lockup |
| `lockup-horizontal-mask.png` | Full horizontal lockup | tinting the nav-bar lockup |
| `petal-top-mask.png` | Single petal (top), white-on-black | rotation-based per-petal animations |
| `petal-{top,left,right}-segmented-mask.png` | One petal each, from angular wedge segmentation | per-petal CSS layering with independent colors |
| `petal-mask-tri.png` | All three petals packed into R/G/B channels | shader sampling — one texture lookup, three masks |

## The three flavors of dynamic tint

### Flavor 1 — single color wash (simplest)

> "The trinity tints to the dominant color of whatever's playing."

Use `mark-mask.png` (or any of the lockup masks) as an alpha matte and fill
underneath with a solid color sampled from the current effect.

**CSS:**

```css
.hypercolor-mark {
  width: 256px;
  height: 256px;
  background-color: var(--scene-color, #e135ff); /* dynamic */
  mask-image: url("/brand/mask/mark-mask.png");
  mask-size: contain;
  mask-position: center;
  mask-repeat: no-repeat;
  -webkit-mask-image: url("/brand/mask/mark-mask.png"); /* Safari */
}
```

Animate `--scene-color` from the daemon's reported average frame color and the
logo follows the room. The mask's grayscale gradient becomes the alpha falloff
automatically, so the chrome highlights still read.

### Flavor 2 — per-petal independent colors (the killer feature)

> "Three primaries, three independent lights. Each petal carries one zone."

Layer three masked divs, one per petal. Each gets its own dynamic color.

**CSS:**

```css
.hypercolor-mark-tri {
  position: relative;
  width: 256px;
  height: 256px;
}

.hypercolor-mark-tri > div {
  position: absolute;
  inset: 0;
  mask-size: contain;
  mask-position: center;
  mask-repeat: no-repeat;
  mix-blend-mode: screen; /* primaries add together at the trinity center */
}

.hypercolor-mark-tri .petal-top    { background: var(--petal-top, #e135ff);
  mask-image: url("/brand/mask/petal-top-segmented-mask.png"); }
.hypercolor-mark-tri .petal-left   { background: var(--petal-left, #80ffea);
  mask-image: url("/brand/mask/petal-left-segmented-mask.png"); }
.hypercolor-mark-tri .petal-right  { background: var(--petal-right, #ff6ac1);
  mask-image: url("/brand/mask/petal-right-segmented-mask.png"); }
```

```html
<div class="hypercolor-mark-tri">
  <div class="petal-top"></div>
  <div class="petal-left"></div>
  <div class="petal-right"></div>
</div>
```

Update the three CSS custom properties from the daemon: petal-top reflects the
"effects" zone, petal-left reflects "devices", petal-right reflects "scenes" —
or any three-zone mapping you want.

### Flavor 3 — chrome preserve + hue tint (prettiest)

> "Original chrome detail intact; just shifts hue with the scene."

Use the full-color master logo as the base; layer a tinted version on top with
`mix-blend-mode: color` or `hue`. The chrome highlights stay, the hue shifts.

```css
.hypercolor-mark-tinted {
  position: relative;
  width: 256px;
  height: 256px;
}

.hypercolor-mark-tinted .base {
  position: absolute;
  inset: 0;
  background: url("/brand/master/mark-color.png") no-repeat center / contain;
}

.hypercolor-mark-tinted .tint {
  position: absolute;
  inset: 0;
  background: var(--scene-color, #e135ff);
  mask-image: url("/brand/mask/mark-mask.png");
  mask-size: contain;
  mask-position: center;
  mask-repeat: no-repeat;
  mix-blend-mode: color;       /* "hue" if you want even less saturation pull */
  opacity: 0.55;
}
```

The base image carries the chrome rim lighting + 3D shading; the tint layer
shifts the dominant hue. Lower opacity for subtle shifts, higher for full
re-skin.

## Leptos component

For `crates/hypercolor-ui`, here's a reusable component covering all three
flavors:

```rust
use leptos::*;

#[derive(Clone, Copy, PartialEq)]
pub enum HypercolorMarkVariant {
    /// Single color wash. Pass one color via `--scene-color`.
    Wash,
    /// Three independent petal colors via `--petal-top|left|right`.
    Tri,
    /// Original chrome + hue-shift overlay.
    ChromeTint,
}

#[component]
pub fn HypercolorMark(
    #[prop(default = HypercolorMarkVariant::ChromeTint)]
    variant: HypercolorMarkVariant,
    /// Size in pixels.
    #[prop(default = 64)]
    size: u32,
    /// Wash + ChromeTint: the scene color. Tri: top petal color.
    #[prop(default = Signal::derive(|| "#e135ff".to_string()))]
    primary: Signal<String>,
    /// Tri only: left petal color.
    #[prop(default = Signal::derive(|| "#80ffea".to_string()))]
    secondary: Signal<String>,
    /// Tri only: right petal color.
    #[prop(default = Signal::derive(|| "#ff6ac1".to_string()))]
    tertiary: Signal<String>,
) -> impl IntoView {
    let style = move || format!(
        "width: {size}px; height: {size}px; \
         --scene-color: {p}; \
         --petal-top: {p}; --petal-left: {s}; --petal-right: {t};",
        size = size,
        p = primary.get(),
        s = secondary.get(),
        t = tertiary.get(),
    );

    match variant {
        HypercolorMarkVariant::Wash => view! {
            <div class="hypercolor-mark" style=style />
        }.into_view(),
        HypercolorMarkVariant::Tri => view! {
            <div class="hypercolor-mark-tri" style=style>
                <div class="petal-top" />
                <div class="petal-left" />
                <div class="petal-right" />
            </div>
        }.into_view(),
        HypercolorMarkVariant::ChromeTint => view! {
            <div class="hypercolor-mark-tinted" style=style>
                <div class="base" />
                <div class="tint" />
            </div>
        }.into_view(),
    }
}
```

Wire `primary`/`secondary`/`tertiary` to signals from the daemon's
`average_frame_color` event and the logo becomes a live indicator of system
state — without you ever having to touch it after wiring.

## Shader sampling (for the eventual Servo/WebGPU rendered hero)

When rendering the logo in a shader (Servo canvas, WebGPU widget), use the
packed `petal-mask-tri.png`:

```wgsl
@group(0) @binding(0) var tri_mask: texture_2d<f32>;
@group(0) @binding(1) var tri_sampler: sampler;

struct Uniforms {
  petal_top: vec4<f32>,
  petal_left: vec4<f32>,
  petal_right: vec4<f32>,
}
@group(0) @binding(2) var<uniform> u: Uniforms;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let mask = textureSample(tri_mask, tri_sampler, uv);
  let color = u.petal_top.rgb * mask.r
            + u.petal_left.rgb * mask.g
            + u.petal_right.rgb * mask.b;
  let alpha = max(mask.r, max(mask.g, mask.b));
  return vec4<f32>(color, alpha);
}
```

One texture lookup, three independent tints, additive at the trinity center
where petals meet. Set the uniforms from the same daemon event stream.

## React / Next.js (hypercolor.lighting site)

Same CSS classes work in React. For the marketing site, the killer move is
syncing the hero logo's tri-color to a slow ambient cycle:

```tsx
function HypercolorMarkHero() {
  const [phase, setPhase] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setPhase((p) => (p + 1) % 360), 80);
    return () => clearInterval(id);
  }, []);

  const top = `hsl(${phase}, 100%, 65%)`;
  const left = `hsl(${(phase + 120) % 360}, 100%, 65%)`;
  const right = `hsl(${(phase + 240) % 360}, 100%, 65%)`;

  return (
    <div
      className="hypercolor-mark-tri"
      style={{
        width: 320,
        height: 320,
        ['--petal-top' as never]: top,
        ['--petal-left' as never]: left,
        ['--petal-right' as never]: right,
      }}
    >
      <div className="petal-top" />
      <div className="petal-left" />
      <div className="petal-right" />
    </div>
  );
}
```

The trinity slowly rainbow-cycles, each petal 120° apart on the hue wheel —
which is exactly what the brand metaphor promises.

## Mask file authoring notes

- CSS masks are RGBA PNGs: white RGB with the grayscale mask in alpha.
- `petal-mask-tri.png` is 8-bit RGB. R = top, G = left, B = right.
- The segmented per-petal masks were derived by angular-wedge segmentation
  from `mark-mask.png`; boundaries sit at ±60° from each petal's center
  direction, so each wedge fully contains exactly one petal.
- Per-petal masks use the same `1145x1032` canvas as `mark-mask.png`, so
  layered CSS masks align without compensating offsets.
- Edges are not anti-aliased at wedge boundaries. The partition is exact:
  the three petal masks sum back to `mark-mask.png` with no overlap.
- All masks are regenerated from `mark-color.png` by `../build.py`. Don't
  hand-edit them; they'll be overwritten on next build.
