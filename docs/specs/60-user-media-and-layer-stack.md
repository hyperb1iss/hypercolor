# Spec 60 — User Media & the Layer-Stack Model

> First-class user-chosen animations on faces and LED groups, composed
> through a real layer stack with alpha, blend modes, transforms, and
> color adjustments. Generalizes the existing `DisplayFaceTarget`
> face-over-effect compositing into a per-group `Vec<SceneLayer>`, adds
> media sources (image, GIF, APNG, animated WebP, video, Lottie, HTTP
> stream) as a sibling of `EffectRenderer`, and closes the in-flight
> display-face composition refactor's alpha gap by routing both LED and
> display-face groups through one composition contract.

**Status:** Draft (v1)
**Author:** Nova
**Date:** 2026-05-15
**Crates:** `hypercolor-types`, `hypercolor-core`, `hypercolor-daemon`, `hypercolor-ui`
**SDK:** `@hypercolor/sdk` (additive)
**Depends on:** Display Faces (42), Face SDK (43), Web Viewport Effect (44),
Canonical Render Pipeline (48), Screen Capture (14)
**Related:** SparkleFlinger (design/30), Render Pipeline Modernization (design/28),
in-flight "display-face composition refactor slice" (Sibyl procedure)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Architecture Overview](#4-architecture-overview)
5. [Layer-Stack Type System](#5-layer-stack-type-system)
6. [Media Sources and Decoder Tiering](#6-media-sources-and-decoder-tiering)
7. [Per-Layer Transform and Color Adjust](#7-per-layer-transform-and-color-adjust)
8. [Display-Face Composition Unification](#8-display-face-composition-unification)
9. [Asset Library](#9-asset-library)
10. [Scene Schema Evolution and Migration](#10-scene-schema-evolution-and-migration)
11. [API Surface](#11-api-surface)
12. [UI Integration](#12-ui-integration)
13. [Standalone Media Player Effect](#13-standalone-media-player-effect)
14. [Audio, Sensor, and Time Bindings](#14-audio-sensor-and-time-bindings)
15. [Color, Security, and Performance Posture](#15-color-security-and-performance-posture)
16. [Delivery Waves](#16-delivery-waves)
17. [Known Constraints](#17-known-constraints)
18. [Verification Strategy](#18-verification-strategy)
19. [Recommendation](#19-recommendation)
20. [Appendix A — File Inventory](#appendix-a--file-inventory)
21. [Appendix B — Migration Examples](#appendix-b--migration-examples)
22. [Appendix C — Competitor Feature Matrix](#appendix-c--competitor-feature-matrix)

---

## 1. Overview

Hypercolor today renders a scene by activating a set of render groups, each
holding exactly one effect (`effect_id: Option<EffectId>`). LED groups feed
the canonical scene canvas via `SparkleFlinger`. Display-face groups
(`role: RenderGroupRole::Display` + `display_target: DisplayFaceTarget`)
publish their own canvas directly to a display worker. The
`DisplayFaceTarget` already carries a `blend_mode: DisplayFaceBlendMode` and
`opacity: f32` so a face can optionally blend over the scene canvas instead
of fully replacing it — but only as a single, special face-over-effect
overlay, and only for display-face groups.

This spec replaces that single-effect-per-group shape with a per-group
**layer stack**. Each render group carries `Vec<SceneLayer>`, where every
layer is one of: an effect, a media file, a screen region, a web viewport,
or a solid color. Each layer has a blend mode, opacity, transform, and color
adjustment. The compositor (`SparkleFlinger`) already supports per-layer
blend modes through `CompositionLayer`; this spec promotes that capability
from a transient compositor concept to a first-class authored construct
that both LED groups and display-face groups consume through the same
contract.

The first new layer-source primitive is **media** — user-chosen image, GIF,
APNG, animated WebP, video, Lottie, or HTTP stream files, decoded by a
dedicated `MediaProducer` and exposed to the compositor as a
`ProducerFrame`. The asset library, decoder tiering, sampling policy, and
UI plumbing are all introduced here.

The architectural through-line is that *content source* (effect, media,
screen, web) is independent of *consumer* (LED via spatial sampling, or
display-face via direct canvas routing). One layer-stack contract serves
both, and the in-flight composition refactor's alpha gap closes naturally
because LED and display-face composition share the same path.

---

## 2. Problem Statement

### 2.1 Single Effect Per Group Is Too Narrow

`RenderGroup.effect_id: Option<EffectId>` admits exactly one effect.
Anything richer — a media background under a sensor-overlay face, an
audio-reactive effect on top of a still image, a livestream layer mixed
with a fallback color fill — has to be hand-written as a bespoke effect,
not composed declaratively. The compositor already supports the
arithmetic; the type system does not let users author it.

### 2.2 Display-Face Compositing Is Half-Built

`DisplayFaceTarget { blend_mode, opacity }` was added so a face could blend
over the underlying scene canvas with one of eleven blend modes. The
in-flight "display-face composition refactor slice" (per the Sibyl
procedure of the same name) is moving display-face composition semantics
between `render_thread/render_groups` and `display_output/worker` encode
paths. The current state has two known defects:

1. **Alpha tests fail** for the Alpha effect-face composition because the
   effect-layer contribution arrives missing or black.
2. **Blended group faces wait for a matching global effect frame** instead
   of composing against black when no effect frame is available, so frames
   stall instead of degrading gracefully.

Both come from the same root cause: display-face composition runs through
its own encode-time path instead of the canonical
`CompositionPlanner` → `SparkleFlinger` pipeline. Unifying the two closes
the gap and gives display faces the full layer-stack capability for free.

### 2.3 User Media Has No Home

Hypercolor renders effects (Rust + WebGL/canvas via Servo HTML), screen
captures, and arbitrary web pages (spec 44). It does not render
user-supplied media files. A user who wants to put a GIF on their Kraken
display, play an animated logo across their LED strips, or layer a Lottie
sparkle over an audio-reactive effect has no path that isn't
"author an HTML effect that embeds a `<video>` tag." That path works for
Servo display faces, but it foreclosures the LED side, the asset library,
adjustable blend modes, transforms, and per-layer parameter bindings, and
is invisible to scene/preset tooling.

### 2.4 Competitive Gap

Corsair iCUE Murals is the only mainstream RGB tool that lets users drop
arbitrary media (image/GIF/video) into the lighting workspace, but their
model is one mural at a time, pixel-locked to a workspace grid, with no
blend-mode layer stack, no vector support, no per-layer parameter
bindings, no spatial topology awareness, and no compositing onto LCD
faces. SignalRGB matches Hypercolor's HTML effect model but has no
framework-level blend modes. Razer Chroma Studio has layer-priority
stacking but no media import. NZXT CAM ships GIF-on-LCD plus a single
infographic overlay, no real compositor.

The shape of a Hypercolor "crush them" answer is already implied by what
we have: a real GPU-capable compositor, spatial sampling, a face system
that already has eleven blend modes in the type system, and a producer
pipeline that already accepts CPU canvases, published surfaces, and GPU
textures. The missing piece is the type system and authoring surface that
lets users compose all of it.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **Per-group layer stack.** `RenderGroup.layers: Vec<SceneLayer>` replaces
  the implicit single-effect model. The existing `effect_id` plus controls
  collapse into a degenerate one-layer stack via migration. Both LED groups
  and display-face groups share the same shape.
- **`SceneLayer` as the authored construct.** Each layer carries source,
  blend mode, opacity, transform, color adjustment, and parameter bindings.
  Serializable, mergeable, persistable in scenes and presets.
- **Media as a layer source.** Image, GIF/APNG, animated WebP, video,
  Lottie, and HTTP stream files become first-class `LayerSource` variants
  via a new `MediaProducer` that publishes `ProducerFrame`s into the
  producer queue at the producer's own cadence.
- **Per-layer transform.** Scale, offset, rotation, and fit mode (reusing
  spec 44's `FitMode`) so media of arbitrary aspect ratios maps onto the
  group's canvas predictably.
- **Per-layer color adjust.** Brightness, saturation, hue shift, tint,
  tint strength. Cheap GPU shader path; CPU fallback works in linear-sRGB.
- **Display-face composition unification.** Display-face groups and LED
  groups go through one `CompositionPlanner` → `SparkleFlinger` pipeline.
  Display workers receive an already-composed `PublishedSurface` and own
  only transport-side encoding. Closes the in-flight refactor's alpha
  gaps.
- **Asset library.** Daemon-managed user assets at
  `~/.config/hypercolor/assets/`, content-hashed, with thumbnails, REST
  CRUD, and hot-reload.
- **Standalone "Media Player" effect.** Users who don't author scenes can
  apply a single media file to a group as a one-shot effect, no layer
  authoring required.
- **Audio, sensor, and time bindings on layer parameters.** Bind opacity,
  tint strength, hue shift, transform scale, or playback speed to any
  audio band, sensor reading, or time-based driver.
- **Backwards-compatible scene format.** Existing scenes load unchanged
  via additive deserialization plus a one-shot migration when first
  written by the new code.

### 3.2 Non-Goals

- **A timeline editor.** Layers are stacked, not timed. A future spec can
  add per-layer in/out points and crossfade automation; this one lands
  the substrate.
- **A node-graph compositor.** Linear layer stack, top-to-bottom, with
  blend modes. No node graphs, no shader-graph authoring, no Houdini.
- **Replacing the existing face HTML SDK.** Spec 43's `face()` API stays
  intact. HTML faces continue to be authored in the SDK. Media layers
  coexist with them, not replace them.
- **Authoring tools for the media itself.** Hypercolor does not edit
  videos, draw images, or compose Lottie. Users bring authored assets.
- **GPU-accelerated Lottie.** Tier-4 Lottie rasterizes on CPU at the
  group canvas size (typically ≤ 1280×720). GPU Lottie is future work.
- **Browser-side animation streams from arbitrary URLs.** Spec 44 covers
  arbitrary URLs through `WebViewport`. This spec consumes that primitive
  rather than re-implementing it.
- **DRM-protected media.** No CDM integration. If a video stream requires
  Widevine/PlayReady, it does not play. We do not pretend.

---

## 4. Architecture Overview

### 4.1 Layered Model

```
Scene
├── RenderGroup "Main LEDs"   (role: Primary)
│   layers:
│   - Effect "Aurora Wave"       blend=Replace  opacity=1.0
│   - Media  "logo.png"          blend=Screen   opacity=0.6
│   - Media  "sparkle.lottie"    blend=Add      opacity=1.0
│     bind: opacity -> audio.bass(0.0..1.0)
│   canvas: 640×480 (spatial sample → LED strips, fans, motherboard)
├── RenderGroup "AIO Display"  (role: Display)
│   layers:
│   - Media  "kitty.gif"         blend=Replace  opacity=1.0
│   - Effect "Sensor Overlay"    blend=Alpha    opacity=1.0
│   canvas: 480×480 (direct to Corsair LCD via display worker)
└── RenderGroup "Reservoir"   (role: Display)
    layers:
    - Effect "Minimal Temp"      blend=Replace  opacity=1.0
    canvas: 320×320
```

### 4.2 Data Flow

```
                     ┌──────────────────────────────┐
                     │      Active RenderGroups     │
                     │  (each owns Vec<SceneLayer>) │
                     └──────────────┬───────────────┘
                                    │
                          ┌─────────▼─────────┐
                          │  Per-layer        │
                          │  Producer pump    │
                          │  • EffectRenderer │
                          │  • MediaProducer  │
                          │  • ScreenLatch    │
                          │  • WebViewport    │
                          │  • ColorFill      │
                          └─────────┬─────────┘
                                    │ ProducerFrame per layer
                          ┌─────────▼─────────────┐
                          │  CompositionPlanner   │
                          │  builds CompositionPlan│
                          │  per render group      │
                          └─────────┬─────────────┘
                                    │
                          ┌─────────▼─────────┐
                          │   SparkleFlinger   │
                          │  CPU lane / GPU    │
                          │  lane / GPU import │
                          └─────────┬─────────┘
                                    │
                ┌───────────────────┼───────────────────┐
                │                                       │
        ┌───────▼──────────┐               ┌────────────▼───────┐
        │  Spatial Engine  │               │  Display Worker     │
        │  → ZoneColors    │               │  • JPEG / packet    │
        │  → LED devices   │               │  • USB transport    │
        └──────────────────┘               └────────────────────┘
```

### 4.3 Key Invariants

1. **One composition contract.** Every render group, regardless of role,
   produces its output by feeding a `CompositionPlan` through
   `SparkleFlinger`. Display workers do not run a parallel compositor.
2. **Producers do not know about consumers.** A `MediaProducer` produces
   the same `ProducerFrame` whether the consumer is an LED group's
   spatial sampler or a display worker's JPEG encoder.
3. **Per-layer cadence.** Each producer runs at its own rate. The
   compositor latches the newest surface per producer per frame. No
   producer can stall the render loop indefinitely; falling-behind
   producers freeze on their last frame and the loop logs once per route.
4. **Sampling stays normalized.** Spatial layouts continue to address
   their canvas in normalized `[0.0, 1.0]` coordinates. Layer transforms
   are expressed in canvas-pixel units inside the compositor but applied
   before the spatial sampler runs.
5. **No silent color-space mutation.** Producers publish non-premultiplied
   sRGB RGBA. The compositor blends in linear sRGB per spec 48 §4.5 and
   converts back to non-premultiplied sRGB on output.

---

## 5. Layer-Stack Type System

### 5.1 `SceneLayer`

New in `hypercolor-types/src/scene.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneLayer {
    /// Stable identifier for this layer (UUID v7).
    pub id: SceneLayerId,

    /// Display name. Defaults to the source's intrinsic name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Content source that feeds this layer.
    pub source: LayerSource,

    /// How this layer composes with the layer beneath it.
    #[serde(default)]
    pub blend: LayerBlendMode,

    /// Layer opacity, clamped to [0.0, 1.0] at deserialize and apply time.
    #[serde(default = "default_layer_opacity")]
    pub opacity: f32,

    /// Geometric placement of the source within the group's canvas.
    #[serde(default)]
    pub transform: LayerTransform,

    /// Color adjustments applied after the source produces a frame.
    #[serde(default)]
    pub adjust: LayerAdjust,

    /// Live bindings from runtime drivers (audio bands, sensors, time)
    /// onto scalar layer parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<LayerBinding>,

    /// Whether this layer is currently active. Disabled layers skip the
    /// producer pump entirely so they do not consume CPU/GPU.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneLayerId(pub Uuid);
```

### 5.2 `LayerSource`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerSource {
    /// A Hypercolor effect from the registry.
    Effect {
        effect_id: EffectId,
        #[serde(default)]
        controls: HashMap<String, ControlValue>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        control_bindings: HashMap<String, ControlBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preset_id: Option<PresetId>,
    },

    /// A media asset from the asset library.
    Media {
        asset_id: AssetId,
        #[serde(default)]
        playback: MediaPlayback,
    },

    /// Live screen capture (full screen or sub-region).
    ScreenRegion {
        #[serde(default)]
        viewport: ViewportRect,
    },

    /// Arbitrary URL via the Web Viewport effect's session (spec 44).
    WebViewport {
        url: String,
        #[serde(default)]
        viewport: ViewportRect,
        #[serde(default = "default_web_viewport_render")]
        render: WebViewportRender,
    },

    /// Constant color fill (useful as a base or for tinted overlays).
    ColorFill { rgba: [f32; 4] },
}
```

### 5.2.1 Effect-Layer Slot Lifecycle

`EffectPool` today allocates one renderer slot per `RenderGroupId`
(`crates/hypercolor-core/src/effect/pool.rs`). The render path in
`crates/hypercolor-daemon/src/render_thread/render_groups.rs` reads
`group.effect_id` and assumes one effect renderer per group. Both
assumptions must change.

The new slot key is `(RenderGroupId, SceneLayerId)`. Reconciliation walks
each active group's `layers` array; for every layer whose
`source == LayerSource::Effect`, the pool ensures a renderer slot exists
keyed by `(group.id, layer.id)`. Layers added since the last reconcile
trigger `init_with_canvas_size()`. Layers removed trigger `destroy()`.

**Duplicate `EffectId` policy.** Two effect layers in the same group with
the same `EffectId` are explicitly supported and each gets its own
renderer instance (and, for HTML effects, its own Servo session per
spec 42 §6). Renderers do not share state across layers even when the
underlying effect is identical, because per-layer controls and bindings
differ. The duplicate-session cost is the user's choice; the daemon
warns once per scene if a single group has more than four effect layers
to flag obvious mistakes.

**Per-layer canvas dimensions.** All layers in a group render at the
group's canvas dimensions (matching the group's spatial layout for LED
groups or the display's native resolution for display-face groups). The
existing `resolve_display_canvas_size()` plumbing in
`render_groups.rs` continues to drive group canvas size; layers do not
override it.

**Servo session pressure.** Per spec 42 §6.7, the practical Servo
sweet-spot is 2 concurrent HTML sessions. A group with two HTML-effect
layers immediately exhausts the budget. Wave 2 must wire FPS downshift
to also account for layer count, not just group count, so a multi-HTML
layer scene degrades gracefully instead of cliff-failing.

### 5.3 `LayerBlendMode`

Unify the two existing blend-mode enums (`canvas::BlendMode`,
`scene::DisplayFaceBlendMode`) under one `LayerBlendMode` that is a
superset of both. `DisplayFaceBlendMode` is retained as an alias with a
`From<DisplayFaceBlendMode> for LayerBlendMode` impl and a deprecation
note; new code uses `LayerBlendMode` everywhere.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerBlendMode {
    Replace,
    #[default]
    Alpha,
    Add,
    Screen,
    Multiply,
    Overlay,
    SoftLight,
    ColorDodge,
    Difference,
    /// Source RGB tints destination RGB by alpha mask.
    Tint,
    /// Destination is revealed only where source luma exceeds a threshold.
    LumaReveal,
}
```

`LumaReveal` and `Tint` are carried over from `DisplayFaceBlendMode` so
faces keep their existing visual modes.

#### 5.3.1 Compositor Support Per Wave

The compositor today (`sparkleflinger/mod.rs:24`) only implements
`CompositionMode::{Replace, Alpha, Add, Screen}`. The remaining
`LayerBlendMode` variants need new compositor math. Schedule:

| Mode                                            | Where it works today           | Wave that lands it in SparkleFlinger |
| ----------------------------------------------- | ------------------------------ | ------------------------------------ |
| Replace, Alpha, Add, Screen                     | `SparkleFlinger` CPU + GPU     | Wave 0 (already shipped)             |
| Multiply, Overlay, SoftLight, ColorDodge, Difference | `canvas::BlendMode::blend()` only (per-pixel utility, not in compositor lane) | Wave 4 (CPU + GPU compositor support) |
| Tint, LumaReveal                                | `display_output/encode.rs:583` (face-vs-scene boundary only) | Wave 6 (promote into SparkleFlinger as general layer ops) |

Scene activation validates that every layer's `blend` is supported by
the current build. Unsupported modes fall back to `Alpha` with a
one-time warning event on the bus, so saved scenes from a newer client
do not silently render wrong on an older daemon.

### 5.4 `LayerTransform`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerTransform {
    /// Where in the destination canvas the source's center lands,
    /// normalized [0.0, 1.0]. Default (0.5, 0.5) = centered.
    pub anchor: NormalizedPoint,

    /// Source scaling, multiplicative. (1.0, 1.0) = native.
    pub scale: [f32; 2],

    /// Rotation in radians, around the anchor.
    pub rotation: f32,

    /// How the source dimensions map to the destination region after
    /// scale/rotation: Contain / Cover / Stretch / Tile / Mirror.
    pub fit: FitMode,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            anchor: NormalizedPoint::new(0.5, 0.5),
            scale: [1.0, 1.0],
            rotation: 0.0,
            fit: FitMode::Cover,
        }
    }
}
```

`FitMode` is shared with spec 44; `Tile` and `Mirror` are new variants
added there.

### 5.5 `LayerAdjust`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerAdjust {
    /// Multiplier on linear RGB. 1.0 = identity, 2.0 = double.
    pub brightness: f32,
    /// HSL saturation multiplier in linear sRGB. 1.0 = identity.
    pub saturation: f32,
    /// Hue shift in radians, applied in HSL space.
    pub hue_shift: f32,
    /// Color used for the tint operation.
    pub tint: [f32; 4],
    /// 0.0 = no tint, 1.0 = full tint.
    pub tint_strength: f32,
    /// 0.0 = identity, positive = contrast up, negative = down.
    pub contrast: f32,
}

impl Default for LayerAdjust {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            saturation: 1.0,
            hue_shift: 0.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            tint_strength: 0.0,
            contrast: 0.0,
        }
    }
}
```

The CPU compositor implements adjustments via the existing
`blend_math::apply_layer_adjust()` helper (new), which works in linear
sRGB. The GPU lane implements them as a small fragment shader stage
before the blend op.

### 5.6 `LayerBinding`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerBinding {
    /// Target parameter on the layer (opacity, tint_strength, hue_shift, ...).
    pub target: LayerParameter,
    /// Driver that feeds the parameter every frame.
    pub source: BindingSource,
    /// Domain → range mapping.
    pub map: BindingMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerParameter {
    Opacity,
    Brightness,
    Saturation,
    HueShift,
    TintStrength,
    Contrast,
    ScaleX,
    ScaleY,
    Rotation,
    PlaybackSpeed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BindingSource {
    AudioBand { band: AudioBand },
    Sensor { name: String },
    Time { rate_hz: f32, wave: TimeWave },
    Constant { value: f32 },
}
```

`AudioBand`, `TimeWave`, and `BindingMap` reuse existing shapes from the
control-binding subsystem in `hypercolor-types/src/effect.rs`. The
binding evaluator at frame start fills a small `LayerRuntime` struct that
the compositor reads instead of the raw `SceneLayer` fields when an
override is present.

---

## 6. Media Sources and Decoder Tiering

### 6.1 `MediaProducer`

```rust
// crates/hypercolor-core/src/effect/media/producer.rs

pub struct MediaProducer {
    asset: Arc<MediaAsset>,
    decoder: Box<dyn MediaDecoder + Send>,
    playback: MediaPlaybackRuntime,
    last_frame: Option<ProducerFrame>,
    last_frame_time_us: u64,
    target_canvas_size: (u32, u32),
}

pub trait MediaDecoder: Send {
    fn intrinsic_size(&self) -> (u32, u32);
    fn intrinsic_fps(&self) -> Option<f32>;
    fn duration_us(&self) -> Option<u64>;
    fn seek(&mut self, time_us: u64) -> Result<()>;

    /// Produce the frame nearest the requested playback time.
    /// Returns the same frame if the decoder is at end-of-stream and
    /// `playback.r#loop = Loop::None`.
    fn frame_for(
        &mut self,
        time_us: u64,
        target_size: (u32, u32),
    ) -> Result<MediaFrame>;
}

pub enum MediaFrame {
    Cpu(Canvas),
    #[cfg(feature = "wgpu")]
    Gpu(ImportedEffectFrame),
}
```

`MediaProducer` is *not* an `EffectRenderer`. It plugs into the producer
queue alongside the effect engine and screen-capture latch, producing
`ProducerFrame`s on its own cadence. A scene with one media layer per
group runs zero effect-engine sessions.

### 6.2 `MediaPlayback`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaPlayback {
    #[serde(default = "default_playback_speed")]
    pub speed: f32,
    #[serde(default)]
    pub loop_mode: LoopMode,
    #[serde(default)]
    pub start_offset_secs: f32,
    #[serde(default = "default_true")]
    pub auto_play: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    None,
    #[default]
    Loop,
    PingPong,
}
```

The `playback.speed` field is bindable via `LayerBinding`, so a sensor
or audio band can drive playback rate at runtime.

### 6.3 Decoder Tiering

| Tier | Format               | Crate                                    | Path        | Risks                                         |
| ---- | -------------------- | ---------------------------------------- | ----------- | --------------------------------------------- |
| 1    | PNG/JPEG static      | `image`                                  | CPU         | Trivial.                                       |
| 1    | GIF                  | `image` + `gif`                           | CPU         | Memory: pre-decoded RGBA frames per asset.    |
| 1    | APNG                 | `image` + `png` (APNG support)            | CPU         | Same as GIF.                                  |
| 1    | PNG sequence (dir)   | `image`                                  | CPU         | Discovery + ordering policy needed.           |
| 2    | Animated WebP        | `image-webp` / `webp-animation`           | CPU         | WebP-VP8L vs VP8 path differences.            |
| 3    | MP4 / WebM video     | `gstreamer-rs` (Linux first)              | GPU upload  | LGPL surface; codec packages required.        |
| 4    | Lottie               | `rlottie`                                | CPU         | Native C++ dependency. Rasterizes per frame.   |
| 5    | HTTP / HLS livestream| `gstreamer-rs` `playbin`                  | GPU upload  | Network failure model; reconnect policy.       |

Tier-1 decoders ship first and cover ≥80% of the user-meme-on-LCD use
case. Tier-2 (WebP) and Tier-4 (Lottie) are pure Rust adds, no system
deps. Tier-3 / Tier-5 require gstreamer, which we already pull
transitively via Servo on Linux but must verify in `cargo tree` and
gate behind a feature flag (`media-video`) for platforms that lack it.

### 6.4 Frame Production Policy

- **Pre-decode caps.** Tier-1 animated formats decode all frames to RGBA
  on load. If `decoded_size > MEDIA_PREDECODE_BUDGET` (default 64 MB per
  asset), the decoder falls back to on-demand decode with a small ring
  buffer (8 frames lookahead). Configurable via daemon config.
- **GPU upload.** Tier-3/5 video frames decode to NV12 in CPU memory and
  upload to a `wgpu::Texture` per frame. The GPU compositor lane consumes
  these as `ProducerFrame::Gpu`. CPU fallback path re-reads back to a
  `Canvas`.
- **Target size resampling.** Decoders pre-scale to the layer's target
  canvas dimensions to avoid re-blits in the compositor. The compositor
  also enforces a hard cap of 1920×1080 per producer frame.
- **End-of-stream handling.** With `LoopMode::Loop`, the decoder resets
  to start. With `LoopMode::PingPong`, direction flips. With
  `LoopMode::None`, the producer freezes on the last frame and stops
  consuming CPU.

### 6.5 Layer Runtime State Machine

Every layer carries a runtime state distinct from its serialized form.
The state lives in the render thread's per-group layer-runtime map and
is published on the bus as `LayerHealthEvent` so the UI can surface it.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerHealth {
    /// Decoder/renderer is initializing (e.g., page load, first decode).
    Loading,
    /// Producing fresh frames on schedule.
    Active,
    /// Frames are arriving but older than `stale_frame_max_age_ms` (default 1000).
    Stalled,
    /// Producer has failed; layer composites as black.
    Failed { reason: String },
    /// Source asset is missing from the library.
    AssetMissing,
}
```

Transitions:

| From      | Event                                           | To                  |
| --------- | ----------------------------------------------- | ------------------- |
| Loading   | First frame produced                            | Active              |
| Loading   | Decoder error or 10 s load timeout              | Failed              |
| Active    | No fresh frame within `stale_frame_max_age_ms`  | Stalled             |
| Active    | Producer pump returns Err                       | Failed              |
| Stalled   | Fresh frame produced                            | Active              |
| Stalled   | Stalled for `stalled_max_secs` (default 5)      | Failed              |
| Failed    | Reconcile reapplies layer (manual or auto)      | Loading             |
| any       | Referenced asset removed from library           | AssetMissing        |
| AssetMissing | Asset re-uploaded with matching hash         | Loading             |

`Failed` and `AssetMissing` layers compose as transparent black so the
rest of the stack still renders. The UI shows a per-layer health pill
(green / yellow / red) and the failure reason for `Failed`.

### 6.6 Producer-Specific Failure Policies

| Source           | Stall behavior                                            | Failure recovery                                      |
| ---------------- | --------------------------------------------------------- | ----------------------------------------------------- |
| `Media` tier 1-2 | Freeze on last decoded frame                              | Reconcile on file change; manual via UI               |
| `Media` tier 3   | Freeze on last decoded frame; reconnect demuxer on EOS    | Backoff 1 s, 2 s, 5 s, 10 s, 30 s; cap at 30 s        |
| `Media` tier 5   | Freeze on last frame; full pipeline reconnect on network error | Same backoff schedule; surface state in UI          |
| `Effect`         | Freeze on last canvas (existing `effect_retained` semantics) | Existing circuit breaker; no change                |
| `ScreenRegion`   | Existing screen-capture latch semantics                   | No new policy                                         |
| `WebViewport`    | Per spec 44 §8.5 (circuit breaker, 3 fails → 30 s open)   | Per spec 44                                           |

### 6.7 Asset Deletion Mid-Reference

When a scene references an `AssetId` and the asset is removed from the
library (via API or filesystem delete), the watcher publishes
`AssetEvent::Removed`. Every active layer holding that ID transitions
to `AssetMissing` and renders black. The scene loads and activates
successfully — missing-asset layers do not block scene activation —
but the UI surfaces the broken state. Re-uploading content with the
same hash promotes the layer back to `Loading`.

---

## 7. Per-Layer Transform and Color Adjust

### 7.1 Compositor Integration

`CompositionLayer` (in `sparkleflinger/mod.rs`) gains two optional
sub-structs:

```rust
pub struct CompositionLayer {
    frame: ProducerFrame,
    mode: CompositionMode,
    opacity: f32,
    opaque_hint: bool,
    transform: Option<CompositionTransform>, // NEW
    adjust: Option<CompositionAdjust>,        // NEW
}
```

When both are `None`, the existing fast paths (single-replace bypass,
shared-multilayer cache, GPU single-blit) continue to apply unchanged.
When either is `Some`, the compositor takes a slower path that first
resamples the source frame through the transform and adjustment shader,
then runs the blend op as today.

### 7.2 Sampling Math

`LayerTransform.fit` is applied first to determine the source-to-canvas
mapping (reusing `sample_viewport()` from spec 44 §4.3). `scale`,
`rotation`, and `anchor` apply on top, in canvas-space pixels. The CPU
path lives in a new `compositor/transform.rs` module. The GPU path
extends the existing single-blit shader to include a sampling matrix
uniform plus the adjust uniform block.

### 7.3 Adjustments in Linear sRGB

All `LayerAdjust` operations are performed on linear-sRGB values. This
matters most for hue shift, saturation, and contrast, which look badly
wrong in non-linear space at the brightness levels common in RGB
hardware. The CPU path uses the existing `RgbaF32` linearization helpers
in `hypercolor-types/src/canvas.rs`. The GPU path uses
`textureLoad` + manual gamma conversion (Servo already imports textures
as non-premultiplied sRGB; we encode that as the texture format).

---

## 8. Display-Face Composition Unification

### 8.1 The Bug We're Closing

The Sibyl procedure "Map display-face composition refactor slice" notes:

- Display-face composition semantics are split between
  `render_thread/render_groups` and `display_output/worker` encode paths.
- Alpha tests fail because the effect-layer contribution arrives missing
  or black at the face-vs-scene blend site
  (`display_output/encode.rs:583`).
- Blended group faces *intentionally* wait for a matching global effect
  frame today (`display_output/mod.rs:492`) instead of composing against
  black. The wait was a workaround; it produces correct visuals but
  stalls under demand changes and obscures the alpha-blend bug.

### 8.2 The Invariant We Are Preserving

Per spec 42 §1 and spec 48 §4.4, display-face groups are **siblings of
the canonical scene canvas, not layers inside it**. This spec must not
collapse the two. The blended-face mode that this spec carries forward
is a *second-stage composite at the display output* in which the face
canvas is overlaid onto the scene canvas before the device-specific
worker transforms run. The LED scene composition never observes a
display-face group.

### 8.3 Two-Stage Composition Model

After this spec, display-face content flows through two distinct compose
stages, both reusing the canonical `CompositionPlanner` →
`SparkleFlinger` pipeline:

**Stage A — Intra-group face composition.** The render thread treats a
display-face group like any other render group: it walks the group's
`layers` array, runs the producer pump per layer, compiles a
`CompositionPlan`, and asks `SparkleFlinger` to compose. The result is a
`PublishedSurface` at the display's native resolution (the *face
canvas*). This is what is new — display-face groups now have a layer
stack of their own.

**Stage B — Optional face-over-scene composite at the display.** If the
group's `DisplayFaceTarget.blend_mode == Replace`, the face canvas is
the final display output. Otherwise, the display worker compiles a
two-layer `CompositionPlan` (scene canvas as the base, face canvas as
the overlay with `blend_mode + opacity` from `DisplayFaceTarget`) and
runs it through the *same* `SparkleFlinger` lane. The output of stage B
is what feeds the device-specific worker transforms.

Both stages use one composition contract. The face-vs-scene blend math
that today lives in `display_output/encode.rs` moves into the
`SparkleFlinger` lane. The display worker stops compositing.

### 8.4 What the Display Worker Still Owns

The display worker is **not** reduced to a pure JPEG encoder. Per
spec 42 §3.1.1 and spec 48 §4.5, the worker continues to own every
device-specific transform that depends on the worker's identity:

- Viewport sampling and crop (worker-local staging)
- Circular masking (for circular AIO LCDs)
- Display-output brightness (LCD policy, distinct from LED brightness)
- JPEG/transport encoding and USB packetization
- Pacing, retry, transport-conflict yield to overdue LED frames

What the worker stops owning:

- The face-vs-scene blend math (moves into `SparkleFlinger` via stage B)
- The decision to stall on missing effect frames (replaced by
  compose-against-black, see §8.5)
- Any per-encode pixel mixing that depends on knowing which face is
  composited with which scene canvas

### 8.5 Semantic Break: Compose Against Black

Today, `display_output/mod.rs:492` keeps a blended-face worker idle
until a matching scene frame arrives. This spec replaces that behavior:
when the face canvas is ready and the scene canvas is not, stage B uses
the most-recent scene canvas if available, else `PublishedSurface::black(...)`
at the display's resolution. The face still renders on schedule.

This is a deliberate behavior change. Users on a slow first-paint may
now see "face on black" briefly during scene activation instead of the
display staying dark until the scene catches up. The trade is worth it:
it fixes the alpha-blend bug, removes the cross-thread coupling that
made the wait fragile, and matches the canvas-degrades-gracefully
posture of every other lane.

### 8.6 Wave 1 Scope (the bug-fix wave)

Wave 1 lands stage B only, and only the minimum surface needed to fix
the alpha bug without introducing layer stacks. Concretely:

1. Lift the face-vs-scene blend math from `display_output/encode.rs:583`
   into a new `SparkleFlinger::compose_face_overlay(scene, face,
   blend_mode, opacity) -> PublishedSurface` entry point. The math
   itself does **not** change — every `DisplayFaceBlendMode` variant
   that today renders correctly through the encode path (including
   `Tint`, `LumaReveal`, `Multiply`, `Overlay`, `SoftLight`,
   `ColorDodge`, `Difference`) must produce bit-identical output after
   the lift. Where this overlaps modes not yet in
   `CompositionMode::{Replace, Alpha, Add, Screen}`, `compose_face_overlay()`
   calls into the existing per-pixel `canvas::BlendMode::blend()`
   helpers (or the encode-path code factored into a shared module),
   not into the generic `CompositionPlan` lane. Wave 4 / Wave 6 promote
   those modes into the general compositor lane per §5.3.1.
2. Update `display_output/worker.rs` to call `compose_face_overlay()`
   before the existing viewport/mask/brightness/encode steps.
3. Replace the wait-for-scene gate at `display_output/mod.rs:492` with
   the black-fallback policy from §8.5.
4. Rewrite the wait-for-scene tests at `display_output/mod.rs:1061` to
   assert the new degradation behavior. List them in the Wave 1 PR.
5. Land the existing alpha-blend test cases as passing
   (`display_face_composition_tests.rs`).
6. Add regression coverage for **every** `DisplayFaceBlendMode` variant
   so the lift cannot silently change visual output. The tests compare
   the new `compose_face_overlay()` output against frozen golden frames
   produced by the existing encode-time math.

Stage A (intra-group face layer stack) ships in Wave 2 on top of the
producer-pump rework. Wave 1 deliberately does **not** require the
`Vec<SceneLayer>` schema change — it only touches the compositor entry
point and the display worker.

### 8.7 Preview Topology

Per-group canvases (spec 42 §8.2) keep their semantics. The "global
scene canvas" used for the UI preview remains the LED-role groups'
composed output. Display-face groups stay on their per-group channels
and are surfaced to the UI as separate preview streams; the UI may
optionally overlay them for visualization but the data is not commingled
with LED hardware composition.

---

## 9. Asset Library

### 9.1 Storage Layout

```
~/.config/hypercolor/assets/
    objects/
        <hash[0:2]>/<hash[2:64]>          — content-addressed blob
    thumbnails/
        <asset_id>.webp                    — 256×256 thumbnail
    index.json                             — id ↔ hash, metadata, mtime
```

Content addressing dedupes identical uploads. The index is the source of
truth for `(AssetId, name, hash, mime_type, intrinsic_size, duration_us)`.
The directory is owned by the daemon process; mode 0700.

### 9.2 `AssetId`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub Uuid);
```

UUID v7. Persisted in scenes/layers. Stable across rebuilds.

### 9.3 Hot Reload

A `notify`-based watcher emits `AssetEvent::Added/Modified/Removed`. The
UI subscribes to these via WebSocket. Active scenes referencing a
modified asset rebuild their `MediaProducer`s on the next render tick.

### 9.4 Size Policy

- **Per-asset soft cap:** 256 MB. Files over the cap upload but log a
  warning and are flagged in the UI.
- **Library soft cap:** 4 GB total. Over-cap library emits a warning;
  upload still succeeds.
- **Hard cap:** 2 GB per file (avoids surprising memory blow-ups).
- Both soft caps are configurable in daemon config (`assets.per_asset_mb`,
  `assets.library_gb`).

### 9.5 Mime Inference and Validation

On upload, the daemon sniffs the file (`infer` crate) to assign a
canonical MIME type. Unsupported types reject with HTTP 415. Supported:
`image/png`, `image/jpeg`, `image/webp`, `image/gif`, `image/apng`,
`video/mp4`, `video/webm`, `application/json` (Lottie), plus
`application/octet-stream` rejected unless an explicit `?type=lottie`
hint is provided.

---

## 10. Scene Schema Evolution and Migration

### 10.1 Backwards-Compatible Reads

`RenderGroup` gains `layers: Vec<SceneLayer>` with
`#[serde(default = "default_layers_from_effect")]`. The deserializer
runs a small migration when `layers` is absent: if `effect_id.is_some()`,
synthesize a one-layer stack containing an `Effect` source with the
existing controls, control bindings, and preset. If `effect_id.is_none()`,
synthesize an empty stack.

The legacy `effect_id`, `controls`, `control_bindings`, and `preset_id`
fields stay on `RenderGroup` for one release with
`#[serde(default, skip_serializing_if = "*::is_default")]`. They mirror
the first effect layer's values when reading, and the writer reflects the
first effect layer into them for forward compatibility with old clients.
A future spec removes them entirely.

### 10.2 Backwards-Compatible Writes

On every save, the daemon writes both the new `layers` field and the
legacy fields. Old clients see only the legacy fields and load the scene
as a single-effect group (losing media layers). New clients prefer
`layers`. This is the same compatibility contract used in spec 44's
screencast migration.

### 10.3 Migration Tool

A one-shot `hypercolor migrate` CLI subcommand walks
`~/.config/hypercolor/scenes/*.json` and rewrites them with explicit
`layers` arrays. Idempotent; safe to run repeatedly.

### 10.4 Concurrency: `layers_version` and Precedence

`RenderGroup` already carries `controls_version: u64` for optimistic
concurrency on `PATCH /api/v1/effects/current/controls`
(`hypercolor-types/src/scene.rs:131`, `api/effects.rs:1022`). The
existing wire contract uses an `If-Match` request header against the
version, returns the current version as the `ETag` response header on
reads and on successful mutations, and answers a mismatch with
HTTP 412 Precondition Failed. The layer-stack endpoints must use the
**same** wire contract — header-based preconditions only, no body
preconditions, 412 on mismatch — to avoid two competing conventions in
the same API.

```rust
pub struct RenderGroup {
    // ... existing fields ...

    /// Monotonic counter bumped on every layer add/remove/reorder
    /// or per-layer mutation (source, blend, opacity, transform,
    /// adjust, bindings, enabled). Defaults to 0 for old scenes.
    #[serde(default)]
    pub layers_version: u64,
}
```

Reads of any layer-stack resource set `ETag: "<layers_version>"`.
Mutations require `If-Match: "<layers_version>"`. Mismatch returns
HTTP 412 with a body containing the current `layers_version` so the
caller can refresh and retry. The version bumps once per request, not
per layer touched, so callers can batch.

**Per-layer controls.** Each layer's effect controls bump
`layers_version` (not a per-layer `controls_version`); we deliberately
keep one concurrency token per group to avoid combinatorial collisions.
A `PATCH .../layers/{id}/controls` body shape mirrors the existing
controls PATCH, just scoped to one layer, and uses the same `If-Match`
header.

**Reorder atomicity.** `PATCH .../layers/order` takes:

```json
{ "layer_ids": ["<uuid>", "<uuid>", ...] }
```

with `If-Match` carrying the precondition. No `if_match` body field —
header only. The server validates that `layer_ids` is a permutation of
the current group's layer IDs exactly. Add and remove are not
permitted through this endpoint. Mismatched membership returns HTTP 422
(client supplied an ill-formed permutation); mismatched version returns
HTTP 412 (concurrent edit). The two failures are distinguishable.

**Legacy / new field precedence.** When a scene's serialized form
contains both `layers` and legacy `effect_id`/`controls`, the daemon
treats `layers` as authoritative and overwrites the legacy mirrors on
the next save. If the two diverge on read (e.g., an old client edited
`effect_id` after a new client wrote `layers`), the daemon logs a
warning and keeps `layers`. A future spec removes legacy fields
entirely; this version of the contract is a one-release transition.

**Live PATCH at top-level.** The existing
`PATCH /api/v1/effects/current/controls` continues to work for groups
with exactly one effect layer (the common case) and continues to use
`controls_version` for its precondition. For multi-layer groups, the
daemon returns HTTP 422 with a body pointing the caller at the
per-layer endpoint, so old clients fail loud instead of silently
patching the wrong layer.

---

## 11. API Surface

### 11.1 Asset Endpoints (New)

```
GET    /api/v1/assets                        — List assets (paginated)
POST   /api/v1/assets                        — Upload (multipart/form-data)
GET    /api/v1/assets/{id}                   — Asset metadata
GET    /api/v1/assets/{id}/blob              — Raw asset bytes
GET    /api/v1/assets/{id}/thumbnail         — 256×256 WebP thumbnail
DELETE /api/v1/assets/{id}                   — Remove asset
PUT    /api/v1/assets/{id}                   — Rename / tag
```

Upload responses include `AssetId`, intrinsic dimensions, duration (for
animated formats), and the inferred MIME type. Duplicate uploads (same
hash) return the existing `AssetId` and HTTP 200.

### 11.2 Layer Endpoints (New)

```
GET    /api/v1/scenes/{id}/groups/{group_id}/layers
POST   /api/v1/scenes/{id}/groups/{group_id}/layers
PUT    /api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}
DELETE /api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}
PATCH  /api/v1/scenes/{id}/groups/{group_id}/layers/order
```

The PATCH `order` endpoint accepts `{ "layer_ids": [...] }` and reorders
the stack atomically. Concurrent edits use the existing
`controls_version`-style optimistic concurrency token, but at the
`RenderGroup.layers_version` granularity (new field).

### 11.3 Live Control Patching

The existing `PATCH /api/v1/effects/current/controls` continues to work
for the *active* layer's effect controls when there is exactly one effect
layer in the active group. For multi-layer scenes, callers must use the
layer-scoped endpoint:

```
PATCH /api/v1/scenes/{id}/groups/{group_id}/layers/{layer_id}/controls
```

### 11.4 MCP Tools

Add three tools to the MCP server:

- `list_assets` — returns `[{id, name, mime, intrinsic_size, duration}]`
- `assign_media_layer` — adds a media layer to a render group with
  `{ scene_id, group_id, asset_id, blend, opacity }`
- `set_layer_blend` — changes blend mode + opacity for an existing layer

---

## 12. UI Integration

### 12.1 Asset Browser

A new top-level page `/assets` shows the library as a grid of thumbnails
with drag-drop upload, search, filter by MIME, and per-asset detail
(intrinsic size, duration, hash, scenes referencing it).

### 12.2 Layers Panel

Under the active render group in the scene editor, a Layers panel shows
the stack top-to-bottom (top = composited last = on top visually). Each
layer row has:

- Drag handle for reorder.
- Source thumbnail (effect icon, asset thumbnail, screen-region preview).
- Blend mode dropdown.
- Opacity slider.
- Expand chevron to reveal transform + adjust + bindings sub-panels.
- Enable/disable toggle.
- Delete affordance.

An "Add layer" button opens a picker with five tabs: Effect, Media,
Screen, Web, Color.

### 12.3 Live Preview

The existing canvas preview shows the composed layer stack in real time.
Per-layer thumbnails update via a low-frequency (~10 fps) thumbnail
stream from the daemon, scoped to the layer panel's viewport.

### 12.4 SilkCircuit Tokens

The Layers panel uses the existing SilkCircuit palette and spacing
tokens. The blend-mode dropdown groups modes into three sections
(*Basic*: Replace, Alpha, Add, Screen; *Photoshop*: Multiply, Overlay,
SoftLight, ColorDodge, Difference; *Face*: Tint, LumaReveal) to mirror
how users think about them.

---

## 13. Standalone Media Player Effect

### 13.1 Built-in Effect

A new native effect `media_player` registers in the builtin effect
registry. Its controls:

| Control            | Type     | Default        | Notes                                    |
| ------------------ | -------- | -------------- | ---------------------------------------- |
| `asset`            | Asset    | (none)         | New `ControlType::Asset(MediaKind)`      |
| `fit`              | Combo    | Cover          | Contain / Cover / Stretch / Tile / Mirror |
| `loop_mode`        | Combo    | Loop           | None / Loop / PingPong                   |
| `speed`            | Slider   | 1.0            | [0.0, 4.0], step 0.05                    |
| `brightness`       | Slider   | 1.0            | [0.0, 2.0], step 0.01                    |
| `tint`             | Color    | white          | Tint color                                |
| `tint_strength`    | Slider   | 0.0            | [0.0, 1.0]                                |
| `hue_shift`        | Slider   | 0.0            | [-π, π]                                   |

Internally, the effect is a thin wrapper that constructs a one-layer
`SceneLayer { source: Media, transform, adjust, ... }` from its controls
and asks the engine to render it through the same path as authored
layers. This gives users who don't author scenes the simplest possible
path to "play a GIF on my devices" without exposing the layer stack.

### 13.2 SDK Asset Control

Add `asset(label, mediaKind)` to the TypeScript SDK as a new control
factory. The build pipeline emits `<meta property="..." type="asset"
media-kind="image" />`. The daemon's meta parser learns the new type.
Effects in the SDK can now declare media assets as inputs.

---

## 14. Audio, Sensor, and Time Bindings

### 14.1 Binding Evaluation

At frame start, after sampling shared inputs (audio, sensors, time), the
binding evaluator walks every active layer's `bindings` array and
produces a `LayerRuntime` with overridden scalar fields. The compositor
reads `LayerRuntime` if present, else the `SceneLayer` defaults.

This means binding evaluation costs O(layers × bindings) per frame,
typically ≤ 20 operations. Negligible.

### 14.2 Example Bindings

```rust
// Pulse opacity on bass.
LayerBinding {
    target: LayerParameter::Opacity,
    source: BindingSource::AudioBand { band: AudioBand::Bass },
    map: BindingMap::linear(0.0..=1.0, 0.3..=1.0),
}

// Hue shift over time.
LayerBinding {
    target: LayerParameter::HueShift,
    source: BindingSource::Time { rate_hz: 0.05, wave: TimeWave::Sine },
    map: BindingMap::linear(-1.0..=1.0, -3.14..=3.14),
}

// Tint strength on GPU temperature.
LayerBinding {
    target: LayerParameter::TintStrength,
    source: BindingSource::Sensor { name: "gpu_temp".into() },
    map: BindingMap::linear(40.0..=85.0, 0.0..=0.8),
}
```

### 14.3 Reuse of Existing Plumbing

`AudioBand`, `BindingMap`, and the sensor-snapshot watch channel already
exist for `ControlBinding`. `LayerBinding` is structurally similar but
targets a fixed set of scalar layer parameters rather than arbitrary
control IDs. The runtime evaluator is a small new function alongside the
existing control-binding evaluator.

---

## 15. Color, Security, and Performance Posture

### 15.1 Color Pipeline

All decoders publish non-premultiplied sRGB RGBA, matching the canonical
contract in spec 48 §4.5. Video decoders that produce YUV (NV12/I420)
convert to sRGB in the GPU shader (or in `image::imageops::convert_yuv()`
on the CPU path). ICC profiles are ignored in v1; we assume sRGB inputs.
Document this in the asset upload response so users with HDR/wide-gamut
content know it will be flattened.

### 15.2 Trust Model

Hypercolor is a self-hosted personal tool. The daemon already has a
two-tier auth posture (`api/security.rs:293`,
`daemon.rs:284`): loopback-only by default, API key required for
non-loopback binding, control-tier endpoints distinguished from
read-only ones. The asset and layer endpoints introduced here must slot
into that posture explicitly.

**Endpoint posture (mandatory):**

| Endpoint                                                          | Tier         | Bind   |
| ----------------------------------------------------------------- | ------------ | ------ |
| `GET    /api/v1/assets`, `/assets/{id}`, `/assets/{id}/thumbnail` | Read         | Any    |
| `GET    /api/v1/assets/{id}/blob`                                 | Read         | Any    |
| `POST   /api/v1/assets` (upload)                                  | Control      | Loopback or authenticated |
| `PUT    /api/v1/assets/{id}` (rename)                             | Control      | Loopback or authenticated |
| `DELETE /api/v1/assets/{id}`                                      | Control      | Loopback or authenticated |
| `POST/PUT/DELETE/PATCH .../layers*`                                | Control      | Loopback or authenticated |

The existing CSRF posture (origin-checked + API-key header) applies to
all control-tier endpoints. Spec 60 adds no new auth mechanism.

**Specific risks and mitigations:**

- **Path traversal on upload.** Filenames are sanitized server-side
  (alphanumeric + `-_. ` only; reject `..`, absolute paths, control
  bytes). The content-addressed store sidesteps filename collisions.
- **Decoder fuzz surface.** The `image` crate is well-fuzzed but not
  immune. Decode in a dedicated `std::thread` with a 10 s wall-clock
  timeout. Rust cannot safely kill a running native thread; on timeout
  the daemon **detaches** the decode thread, marks the upload as
  `Failed { reason: "decoder_timeout" }`, and quarantines the thread
  identifier so its eventual completion (success or panic) is dropped
  rather than acted on. The thread continues to consume CPU and memory
  until it returns or the process exits; this is the cost of not
  fork-isolating decoders. Hard caps on file size (2 GB) and decoded
  pixel count (`width * height ≤ 128 Mpx`) gate the decoder *before* it
  is asked, so the timeout path is rare. Promoting decoder isolation to
  a subprocess sandbox is tracked as future hardening but is out of
  scope here.
- **gstreamer plugin surface.** Configure gstreamer with a positive
  allowlist of demuxers and decoders (`mp4mux`, `qtdemux`, `matroskademux`,
  `vp8dec`, `vp9dec`, `avdec_h264`, `avdec_h265`, `audioconvert` only),
  not a denylist. Disable all sinks that touch the filesystem outside
  the asset store. The allowlist is documented in daemon config.
- **HTTP livestream URLs (SSRF).** Validate URLs against a default-deny
  allowlist for private IP ranges (RFC 1918, IPv6 ULA, loopback,
  link-local). The default rejects every private range; users opt in to
  specific ranges via config. Schemes are restricted to `http`, `https`,
  `rtmp`, `rtsp`. No `file://`, no arbitrary plugins.
- **Asset blob endpoint.** `GET /api/v1/assets/{id}/blob` serves raw
  user content. Set `Content-Disposition: attachment` and an
  `X-Content-Type-Options: nosniff` header so a browser preview never
  executes a malicious payload as HTML/JS.
- **Authentication off.** Per `api/security.rs:293`, auth can be fully
  disabled. When disabled *and* the daemon binds to a non-loopback
  address, the daemon refuses to start with control-tier asset/layer
  endpoints enabled. The user must explicitly accept this combination
  via config (`security.allow_unauthenticated_asset_write = true`).

### 15.3 Performance Budget

In the current implementation, most producers run inline on the render
thread tick (see `render_groups.rs` and `frame_composer.rs`); the
producer queue itself is a latest-frame latch
(`producer_queue.rs`), not a task runtime. The Servo worker is the
notable exception — it lives on its own thread. The decoder budgets
below assume the *target* posture, in which heavy decoders (video,
livestream, Lottie) run on dedicated tasks and publish into the same
latest-frame latch. Wave 3 ships that refactor for media producers
specifically; until then, decode cost is paid inline on the render
tick and the layer ceiling is correspondingly tighter. Spec 48 is the
canonical reference for the target pipeline.

For static and cached animated assets (image, GIF, APNG, WebP), the
decoder cost is amortized over the producer's own cadence: a 24 fps GIF
contributes one frame-cached lookup per 41 ms regardless of render FPS.

**Per-frame compositor cost (CPU lane, 30 fps target, 33 ms budget):**

| Stage                                      | Cost / layer        | Notes                                       |
| ------------------------------------------ | ------------------- | ------------------------------------------- |
| Binding evaluation                         | ≤ 0.01 ms           | O(layers × bindings); ~20 ops total          |
| Transform + adjust (identity, fast path)   | 0 ms                | Bypassed when both are `None`                |
| Transform + adjust (slow path)             | ~2 ms @ 640×480     | CPU resample + adjust shader                  |
| Blend op (Replace/Alpha/Add/Screen)        | ~0.5 ms @ 640×480   | Existing SparkleFlinger CPU lane             |
| Display worker encode (after stage B)      | ~4 ms @ 480×480     | TurboJPEG; unchanged                          |

Practical CPU-lane envelope: **4 active layers per group with identity
transform/adjust**, or **3 with full slow-path transform/adjust**.
Beyond that, FPS downshifts per the existing 2-frame-miss policy.

**Per-producer decoder cost (paid on producer cadence, not render
cadence):**

| Decoder            | Per-frame cost            | Source-size cap |
| ------------------ | ------------------------- | --------------- |
| Image (static)     | 0 (one-shot decode)       | 128 Mpx          |
| GIF/APNG (cached)  | ~0.05 ms (lookup)         | 64 MB decoded    |
| Animated WebP      | ~0.4 ms                   | 64 MB decoded    |
| Lottie (rasterize) | 2–8 ms @ 480×480          | n/a (vector)     |
| Video (CPU lane)   | 10–20 ms @ 1080p          | 1080p hard cap   |
| Video (GPU lane)   | 4–8 ms @ 1080p            | 1080p hard cap   |
| HTTP livestream    | 10–25 ms @ 1080p          | 1080p hard cap   |

Heavy-decoder layers (Lottie, video, livestream) advertise their
per-frame cost in `MediaProducer::estimated_cost_us()`. The render
thread sums advertised costs across all active layers and applies
admission control before activating a scene:

- **Hard cap: 2 concurrent video producers** across all groups, default.
  Configurable via `media.max_video_producers` (1..=4).
- **Hard cap: 1 livestream producer** across all groups, default.
  Configurable via `media.max_livestream_producers` (0..=2).
- **Soft cap: producer cost sum ≤ 60 ms.** Above this, the daemon logs
  a warning and the FPS controller pre-emptively downshifts the affected
  groups by one tier before the first miss.

Scenes that exceed hard caps fail activation with HTTP 422 and a body
listing which layers exceed the cap; users edit the scene to comply
rather than discovering the failure at runtime.

**GPU lane numbers:** Transform + adjust ~0.3 ms / layer, blend
~0.2 ms / layer. The GPU lane raises the practical layer ceiling but
adds upload bandwidth pressure for video producers (8 MB / frame @ 1080p
RGBA). See §17.4 for the GPU memory pressure constraint.

---

## 16. Delivery Waves

Seven landable waves. Each leaves the tree green.

### Wave 0 — Layer-Stack Types

Add `SceneLayer`, `LayerSource`, `LayerBlendMode`, `LayerTransform`,
`LayerAdjust`, `LayerBinding`, `AssetId`, `SceneLayerId`. Migration on
read/write for `RenderGroup`. Tests for round-trip, default synthesis,
and ordering invariants. No behavior change yet.

### Wave 1 — Display-Face Composition Unification (stage B only)

The narrow, bug-fix wave. Does **not** require the `Vec<SceneLayer>`
schema change.

Scope (matches §8.6):

1. Add `SparkleFlinger::compose_face_overlay()` that takes a scene
   `PublishedSurface` and a face `PublishedSurface` plus the existing
   `DisplayFaceTarget.blend_mode + opacity` and runs them through the
   canonical compositor lane.
2. Update `display_output/worker.rs` to call `compose_face_overlay()`
   before its existing viewport / mask / brightness / encode pipeline.
   The worker keeps every device-specific transform (§8.4).
3. Replace the wait-for-scene gate in `display_output/mod.rs:492` with
   the compose-against-black policy from §8.5. Update the affected
   tests at `display_output/mod.rs:1061` and the alpha-blend tests at
   `display_output/encode.rs:583` so the new degradation is asserted.
4. Add `display_face_composition_tests.rs` covering the alpha + slow-
   scene + Replace + Tint + LumaReveal cases.

**Lands during launch hardening.** It closes the in-flight refactor's
known alpha bug, removes the cross-thread coupling that made it
fragile, and adds no user-visible feature surface beyond what already
exists in `DisplayFaceBlendMode`. Tint and LumaReveal continue to use
their current encode-time math in Wave 1 (they only run between scene
and face, where the math is unchanged); compositor-side Tint/LumaReveal
arrive in Wave 6 per §5.3.1.

### Wave 2 — Producer Pump for Layer Stacks

Render thread consumes `Vec<SceneLayer>` instead of a single effect.
Effect-layer source is the only `LayerSource` variant initially. Plumbs
the layer stack through `CompositionPlanner` end to end. Multi-layer
LED groups now render correctly.

### Wave 3 — Asset Library + Tier-1 Media

Asset library (storage, REST, hot reload). `MediaProducer` with tier-1
decoders (PNG/JPEG/GIF/APNG/PNG-sequence). `MediaPlayback` runtime.
Standalone `media_player` builtin effect. New `ControlType::Asset`.

### Wave 4 — Transform + Adjust

`LayerTransform` and `LayerAdjust` in compositor and producer. CPU
sampling helpers and GPU shader uniforms. `FitMode::Tile` and `Mirror`
in spec 44's shared `FitMode`. Visual regression tests for each mode.

### Wave 5 — UI: Layers Panel + Asset Browser

Leptos pages: `/assets`, scene editor Layers panel, asset picker dialog,
blend/opacity/transform/adjust inspectors, drag-drop upload. Live preview
of composed stack.

### Wave 6 — Tier 2/3 Decoders + Bindings

Animated WebP (tier 2). Video via gstreamer behind `media-video` feature
flag (tier 3). `LayerBinding` evaluator and SDK exposure.

### Wave 7 — Tier 4/5 Decoders + Multi-Face Routing

Lottie (tier 4). HTTP livestream (tier 5). "Scene-wide" layer authoring
that broadcasts one media asset across multiple render groups with
per-group transforms.

---

## 17. Known Constraints

### 17.1 gstreamer License Surface

gstreamer is LGPL. We already pull it transitively via Servo on Linux,
but we need to confirm:

```bash
cargo tree -p hypercolor-daemon | grep gstreamer
```

If present, video and livestream support land at no licensing cost. If
absent, video tier moves behind a `media-video` feature flag, off by
default, with a documented build-time dep. Lottie (rlottie, LGPL) has
the same flag posture under `media-lottie`.

### 17.2 Layer Count Ceiling

`CompositionPlanner` today is exercised mostly with 1–2 layer plans
(single effect + optional scene-transition base). 4–6 layer plans will
stress the cache-reuse path in `sparkleflinger/cpu.rs`. Wave 2 must add
benchmarks to keep this honest.

### 17.3 Producer Queue Pressure

Each media layer is a producer. A scene with 3 render groups × 3 media
layers each adds 9 producers to the queue. The queue uses watch-style
"latest only" semantics so this does not allocate per-frame, but it does
multiply texture upload bandwidth on the GPU lane. Budget assumes ≤ 6
active media layers across all groups.

### 17.4 GPU Memory Pressure

Tier-3 video paths upload one wgpu texture per producer per frame. At
1080p RGBA, that's 8 MB. With three video layers, GPU memory pressure
adds ~24 MB / frame plus the GPU compositor's working set. Acceptable on
modern hardware; we will document the cost and add a runtime cap (max
two concurrent video producers by default) to avoid sudden OOMs.

### 17.5 Schema Forward-Compatibility Cost

Carrying legacy `effect_id`/`controls`/`control_bindings` on
`RenderGroup` for one release adds noise to scene JSON. The migration
tool plus a release-note bullet are sufficient; the legacy fields drop
in the next breaking release (tracked as a follow-up spec).

### 17.6 In-Flight Composition Refactor

This spec consumes the in-flight refactor's destination shape. If the
refactor lands a different shape before this spec ships Wave 1, Wave 1's
scope contracts to "extend the new contract to support multi-layer".
Coordinate with whoever owns the refactor before Wave 1 starts.

### 17.7 No HDR

The `image` crate decodes high-bit-depth PNG to RGBA8 today. Wide-gamut
or HDR sources are tone-mapped to sRGB on load and lose precision. Out
of scope for this spec; future work can carry through an HDR-aware
canvas type.

### 17.8 GPU Compose Fallback for GPU Producer Frames

`SparkleFlinger.compose_for_outputs()` in
`sparkleflinger/mod.rs:304-316` currently skips the CPU fallback when
the plan contains `ProducerFrame::Gpu` and the GPU lane fails or
declines to support the plan. In that case the function returns
`gpu_frame_without_cpu_fallback()` — a composed-frame set with no
sampling canvas and no preview surface.

Video and livestream producers feed `ProducerFrame::Gpu`, so Wave 6+
will exercise this path constantly. The current "no fallback" behavior
means a single GPU compose failure produces a blank frame for an entire
render cycle.

**Required policy (Wave 6):**

1. On GPU compose failure with GPU frames present, `SparkleFlinger`
   performs an explicit read-back of every `ProducerFrame::Gpu` in the
   plan into a `Canvas`, then runs the CPU lane with the read-back
   surfaces. The read-back path is heavier (one GPU → CPU copy per
   frame) but produces correct pixels.
2. Read-back failures (rare; OOM or device loss) escalate to a hard
   error event on the bus and the frame composes as black. The
   `LayerHealth` of affected layers transitions to `Failed` with reason
   `gpu_readback_failed`.
3. Two consecutive read-back fallbacks downshift the compositor
   acceleration mode for the rest of the session (returning to GPU on
   the next session start). Surface this as a one-time toast in the UI
   so users know they have left the fast path.

Wave 6 must also add a synthetic-failure test
(`gpu_compose_fallback_tests.rs`) that injects a GPU compose failure
on a video plan and asserts the read-back path produces the expected
pixels.

---

## 18. Verification Strategy

### 18.1 Unit Tests

- `SceneLayer` serde round-trip, migration synthesis, layer ordering,
  binding evaluation, opacity clamping.
- `LayerBlendMode` ↔ `BlendMode` ↔ `DisplayFaceBlendMode` conversion
  tables.
- `MediaPlayback` time computation: loop, ping-pong, speed, offset, EOS.
- `AssetId` collision: identical content uploads dedup.
- `CompositionLayer` transform + adjust: fast-path bypass when both are
  `None`.

### 18.2 Integration Tests

- **Display-face alpha (Wave 1):** Reproduce the Sibyl-noted alpha
  failures, verify they pass with the unified pipeline.
- **Multi-layer LED group:** 3-layer scene (effect base + media middle +
  effect overlay) renders correctly at 30 fps; spatial sample matches
  expected pixel values at known LED positions.
- **GIF on Kraken simulator:** Upload a GIF asset, assign as media layer
  to a display-face render group with a virtual display device,
  preview JPEG matches expected frames.
- **Audio-reactive binding:** Bass-bound opacity binding produces
  opacity changes matching synthetic audio inputs.
- **Hot-reload:** Modify an asset file on disk, scene rebuilds the
  decoder, next frame uses new content.

### 18.3 Manual Verification

- Upload a square GIF; assign to a Corsair LCD face group; verify it
  renders at native resolution.
- Stack a Lottie sparkle overlay on top of an audio-reactive effect on a
  desktop LED layout; verify both layers compose visually.
- Play a 1080p MP4 on the canonical canvas with `FitMode::Cover`; verify
  no OOM after 5 minutes of playback.
- Drag a 32-band scope effect under an APNG mask with `LumaReveal`; verify
  the mask reveals the effect through the bright portions.

---

## 19. Recommendation

**Land Wave 1 inside the launch hardening window.** It is the in-flight
composition refactor's destination, closes real bugs (the Sibyl-noted
alpha tests), and adds no user-facing surface area beyond what already
exists.

**Hold Waves 0, 2, 3 for v0.2.** Schema changes plus the producer-pump
refactor are too disruptive for late-cycle hardening, but they unlock
everything that follows and slot in cleanly post-launch.

**Treat Wave 6 (video) and Wave 7 (Lottie + livestream) as marketing
ammunition.** They are the differentiators against iCUE Murals and the
SignalRGB / Razer / NZXT incumbents. Each ships independently once the
substrate is in place.

The architectural payoff is one composition contract instead of two,
one layer-source abstraction across LED and display-face groups, and a
producer model that admits arbitrary content without touching the
compositor. The user-facing payoff is a layer stack that competitors do
not have, a vector media format (Lottie) that nobody else supports, and
a livestream source that nobody else has even tried.

Start with Wave 1 to retire the bug. The rest follows post-launch.

---

## Appendix A — File Inventory

### New files

```
crates/hypercolor-types/src/layer.rs
crates/hypercolor-types/src/asset.rs
crates/hypercolor-types/tests/layer_tests.rs
crates/hypercolor-types/tests/asset_tests.rs

crates/hypercolor-core/src/effect/media/mod.rs
crates/hypercolor-core/src/effect/media/producer.rs
crates/hypercolor-core/src/effect/media/decoder.rs
crates/hypercolor-core/src/effect/media/cpu_image.rs       (tier 1)
crates/hypercolor-core/src/effect/media/cpu_webp.rs        (tier 2)
crates/hypercolor-core/src/effect/media/gst_video.rs       (tier 3, gated)
crates/hypercolor-core/src/effect/media/lottie.rs          (tier 4, gated)
crates/hypercolor-core/src/effect/media/playback.rs
crates/hypercolor-core/src/effect/builtin/media_player.rs
crates/hypercolor-core/src/asset/mod.rs
crates/hypercolor-core/src/asset/library.rs
crates/hypercolor-core/src/asset/index.rs
crates/hypercolor-core/src/asset/watcher.rs
crates/hypercolor-core/src/blend_math/adjust.rs
crates/hypercolor-core/tests/media_producer_tests.rs
crates/hypercolor-core/tests/asset_library_tests.rs

crates/hypercolor-daemon/src/render_thread/layer_runtime.rs
crates/hypercolor-daemon/src/render_thread/binding_eval.rs
crates/hypercolor-daemon/src/render_thread/sparkleflinger/transform.rs
crates/hypercolor-daemon/src/api/assets.rs
crates/hypercolor-daemon/src/api/layers.rs
crates/hypercolor-daemon/src/mcp/tools/media.rs
crates/hypercolor-daemon/tests/display_face_composition_tests.rs
crates/hypercolor-daemon/tests/multi_layer_render_tests.rs
crates/hypercolor-daemon/tests/asset_api_tests.rs

crates/hypercolor-ui/src/pages/assets.rs
crates/hypercolor-ui/src/components/layers_panel/mod.rs
crates/hypercolor-ui/src/components/layers_panel/layer_row.rs
crates/hypercolor-ui/src/components/layers_panel/source_picker.rs
crates/hypercolor-ui/src/components/asset_browser/mod.rs
crates/hypercolor-ui/src/components/asset_browser/uploader.rs
crates/hypercolor-ui/src/api/assets.rs

sdk/packages/core/src/controls/asset.ts
```

### Modified files

```
crates/hypercolor-types/src/scene.rs              (layers field, migration)
crates/hypercolor-types/src/effect.rs             (ControlType::Asset)
crates/hypercolor-types/src/canvas.rs             (BlendMode parity)
crates/hypercolor-types/src/viewport.rs           (FitMode::Tile, Mirror)

crates/hypercolor-core/src/effect/registry.rs     (builtin media_player)
crates/hypercolor-core/src/effect/meta_parser.rs  (asset meta tag)
crates/hypercolor-core/src/spatial/viewport.rs    (Tile, Mirror)
crates/hypercolor-core/src/blend_math.rs          (LayerAdjust ops)

crates/hypercolor-daemon/src/render_thread/composition_planner.rs
crates/hypercolor-daemon/src/render_thread/frame_composer.rs
crates/hypercolor-daemon/src/render_thread/render_groups.rs
crates/hypercolor-daemon/src/render_thread/sparkleflinger/mod.rs
crates/hypercolor-daemon/src/render_thread/sparkleflinger/cpu.rs
crates/hypercolor-daemon/src/render_thread/sparkleflinger/gpu.rs
crates/hypercolor-daemon/src/display_output/mod.rs       (remove composition)
crates/hypercolor-daemon/src/display_output/worker.rs    (encoder only)
crates/hypercolor-daemon/src/display_output/encode.rs
crates/hypercolor-daemon/src/api/mod.rs
crates/hypercolor-daemon/src/api/scenes.rs
crates/hypercolor-daemon/src/api/control_values.rs       (asset value)
crates/hypercolor-daemon/src/mcp/server.rs

crates/hypercolor-ui/src/pages/scenes.rs
crates/hypercolor-ui/src/components/control_panel/mod.rs

sdk/packages/core/src/controls/specs.ts
```

---

## Appendix B — Migration Examples

### B.1 Single-Effect Group (Before)

```json
{
  "id": "...",
  "name": "Main LEDs",
  "effect_id": "aurora-wave-uuid",
  "controls": { "speed": 1.5 },
  "preset_id": null,
  "role": "primary"
}
```

### B.2 Same Group (After, Auto-Migrated)

```json
{
  "id": "...",
  "name": "Main LEDs",
  "role": "primary",
  "effect_id": "aurora-wave-uuid",        // legacy mirror
  "controls": { "speed": 1.5 },           // legacy mirror
  "layers": [
    {
      "id": "...",
      "source": {
        "type": "effect",
        "effect_id": "aurora-wave-uuid",
        "controls": { "speed": 1.5 }
      },
      "blend": "replace",
      "opacity": 1.0
    }
  ]
}
```

### B.3 Authored Multi-Layer Group

```json
{
  "id": "...",
  "name": "AIO Display",
  "role": "display",
  "display_target": {
    "device_id": "corsair-icue-link-lcd-1",
    "blend_mode": "replace",
    "opacity": 1.0
  },
  "layers": [
    {
      "id": "...",
      "name": "Loop Cat",
      "source": {
        "type": "media",
        "asset_id": "...",
        "playback": { "speed": 1.0, "loop_mode": "loop" }
      },
      "blend": "replace",
      "opacity": 1.0,
      "transform": { "fit": "cover" }
    },
    {
      "id": "...",
      "name": "Sensor Overlay",
      "source": {
        "type": "effect",
        "effect_id": "sensor-overlay-uuid",
        "controls": { "sensor": "cpu_temp" }
      },
      "blend": "alpha",
      "opacity": 1.0
    }
  ]
}
```

---

## Appendix C — Competitor Feature Matrix

| Capability                            | Hypercolor (this spec) | iCUE Murals | SignalRGB | Razer Chroma Studio | NZXT CAM |
| ------------------------------------- | ---------------------- | ----------- | --------- | ------------------- | -------- |
| User media: image                     | yes                    | yes         | indirect¹  | no                  | yes      |
| User media: GIF                       | yes                    | yes         | indirect¹  | no                  | yes      |
| User media: video (MP4/WebM)          | yes (tier 3)           | yes         | indirect¹  | no                  | no       |
| User media: vector (Lottie)           | **yes (tier 4)**       | no          | no        | no                  | no       |
| User media: HTTP livestream           | **yes (tier 5)**       | no          | no        | no                  | no       |
| Framework-level blend modes           | **yes (11 modes)**     | no          | no        | priority only       | no       |
| Per-layer opacity                     | **yes**                | no          | no        | no                  | no       |
| Per-layer transform                   | **yes**                | no          | no        | no                  | no       |
| Per-layer color adjust                | **yes**                | no          | no        | no                  | no       |
| Audio-reactive layer bindings         | **yes**                | no          | no        | no                  | no       |
| Sensor-reactive layer bindings        | **yes**                | no          | no        | no                  | no       |
| LCD face compositing with alpha       | **yes**                | n/a         | n/a       | n/a                 | single overlay |
| Spatial topology sampling             | **yes**                | grid only   | grid only | grid only           | n/a      |
| Multi-face routing of one asset       | **yes (wave 7)**       | yes         | no        | no                  | no       |
| Asset library (deduped, hashed)       | **yes**                | implicit    | implicit  | n/a                 | implicit |
| Hot-reload on asset change            | **yes**                | no          | no        | no                  | no       |
| Open source / self-hosted             | **yes**                | no          | no        | no                  | no       |

¹ SignalRGB users can write an HTML effect that embeds `<img>` / `<video>`,
but it is not a framework feature; users hand-author the markup and the
compositor has no concept of "media".

---
