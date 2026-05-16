# Spec 44 — Web Viewport Effect & Unified Viewport Control

> Arbitrary webpages as effect sources, with a shared rectangular
> viewport control that replaces screencast's four-slider crop and
> extends cleanly to future viewport-style effects. Reuses the existing
> Servo rendering path and the existing region-picker UI patterns.

**Status:** Implemented — `WebViewportRenderer` fully implemented in `hypercolor-core`
**Author:** Nova
**Date:** 2026-04-12
**Packages:** `hypercolor-types`, `hypercolor-core`, `hypercolor-ui`, `@hypercolor/sdk`
**Depends on:** Servo HTML Effects (WEB_ENGINE_DECISION), Screen Capture (14)
**Related:** Render Pipeline Modernization (design/28), Spatial Engine (06)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Problem Statement](#2-problem-statement)
3. [Goals and Non-Goals](#3-goals-and-non-goals)
4. [Unified Viewport Primitive](#4-unified-viewport-primitive)
5. [Rect Control Type](#5-rect-control-type)
6. [ViewportPicker UI Widget](#6-viewportpicker-ui-widget)
7. [Web Viewport Effect](#7-web-viewport-effect)
8. [Servo Arbitrary URL Loading](#8-servo-arbitrary-url-loading)
9. [Preview Stream](#9-preview-stream)
10. [Screencast Migration](#10-screencast-migration)
11. [Security Posture](#11-security-posture)
12. [Delivery Waves](#12-delivery-waves)
13. [Known Constraints](#13-known-constraints)
14. [Verification Strategy](#14-verification-strategy)

---

## 1. Overview

Hypercolor already renders arbitrary HTML with Servo, and already samples
a normalized rectangular sub-region for the screencast effect. The two
capabilities were built independently: Servo only loads local HTML files
from allowed directories, and screencast exposes its crop rect as four
separate slider controls picked up by a bespoke composite widget.

This spec unifies both halves. It introduces a canonical `Rect` control
type, extracts the screencast picker into a generic viewport widget, and
adds a native Web Viewport effect that points Servo at an arbitrary URL
and samples a user-chosen region of the rendered page into the effect
canvas. Screencast migrates to the new control type, the picker widget
serves both effects, and future viewport-style effects (camera feeds,
media player output, etc.) plug into the same primitive.

The philosophy stays intact. Effects are web pages. With this spec, _any_
web page becomes an effect, and the user's desk becomes the canvas.

---

## 2. Problem Statement

### 2.1 No Arbitrary URL Support

`hypercolor-core/src/effect/paths.rs:45-75` resolves every effect path
to a local file before handing it to Servo. The worker thread at
`hypercolor-core/src/effect/servo/worker.rs:896-944` converts that path
to a `file://` URL via `file_url_for_path(path)` and calls
`webview.load()`. The underlying Servo session handles any URL scheme,
but the load path is hardcoded to the bundled/user/cwd file resolver.
There is no configuration surface for "load this URL," even though
the runtime already supports it.

### 2.2 Four-Slider Crop Is A Widget Hack

`hypercolor-core/src/effect/builtin/screen_cast.rs:81-166` declares
`frame_x`, `frame_y`, `frame_width`, `frame_height` as four independent
`Slider` controls. The UI at
`hypercolor-ui/src/components/control_panel/mod.rs:207-289` detects the
specific combination of control IDs and substitutes a composite widget
from `screen_cast.rs` in the same directory. The composite owns a live
preview canvas, a draggable rect overlay, and four corner handles.
Every interaction emits four separate `ControlValue::Float` updates that
the effect reassembles.

The widget works, but it is fundamentally a special case keyed on
string IDs. A second effect that wants the same UI would have to either
repeat the four-slider convention or duplicate the widget. Neither
scales.

### 2.3 Viewport Primitives Are Fragmented

The codebase already has three unrelated rectangular types:

- `NormalizedRect` in `hypercolor-types/src/spatial.rs:127-143`. Used
  in zone canvas regions. Serializable. Not used as a control value.
- `DisplayViewport` in `hypercolor-daemon/src/display_output/mod.rs:108-152`.
  Used for LCD output. Adds rotation, scale, edge behavior. Not
  serialized.
- `FrameRect` in `hypercolor-ui/src/control_geometry.rs:1-108`. UI only.
  Has the clamp/drag/resize math for the screencast widget.

These were built at different times for different callers. A single
canonical viewport type would let the picker widget, screencast, and
web viewport share code paths, and would give future effects a clear
primitive to reach for.

### 2.4 Render Resolution Is Canvas-Sized

The current Servo integration renders the webpage at the effect canvas
size (640×480 default). For a viewport effect, cropping a sub-region
from a 640-wide render produces tiny, blurry pixels. Web pages look
right at 1280×720 or larger, and the viewport sampler should pull from
that higher-resolution buffer, downsampling into the canvas only at the
final stage.

---

## 3. Goals and Non-Goals

### 3.1 Goals

- **`Rect` control type** added to `hypercolor-types`, the SDK, and the
  daemon JSON conversion layer
- **Canonical normalized viewport** reused by both screencast and the
  new effect, with clear semantics for bounds, clamping, and
  serialization
- **`ViewportPickerWidget`** in `hypercolor-ui`, parameterized over the
  preview frame source and optional URL input, replacing the bespoke
  screencast composite
- **Web Viewport native effect** that exposes URL, viewport rect, fit
  mode, brightness, and refresh-interval controls
- **Arbitrary URL loading in Servo** via a new load path that skips
  metadata extraction and preamble injection
- **Dedicated render resolution** for the web viewport effect, decoupled
  from the effect canvas size, sampled through the viewport rect
- **Web viewport preview stream** surfaced through the WebSocket bus so
  the picker can draw its rect overlay on the live page render
- **Screencast migrated** to the new `Rect` control, bespoke composite
  widget removed
- **SDK `rect()` factory** so future SDK-authored effects can declare
  rect controls without resorting to four sliders

### 3.2 Non-Goals

- **Arbitrary URL support for SDK HTML effects.** SDK effects remain
  file-based. The web viewport effect is a native Rust effect that
  drives a Servo session, not an SDK-authored HTML effect.
- **Pushing viewport upstream of screen capture.** Screencast continues
  to crop after the 640×480 downscale. A future spec can move the crop
  into the capture source for detail preservation; out of scope here.
- **Allowlist or CSP enforcement.** Hypercolor is a self-hosted personal
  tool. The effect description calls out the trust model. Future specs
  can layer policy on top if the threat model changes.
- **Unifying `DisplayViewport` with the new type.** Display viewports
  carry rotation, scale, and edge behavior that effects do not need.
  They stay distinct for now. A later pass can align names once the
  usage patterns are clear.
- **Interactive webpages.** The web viewport effect is a one-way sampler.
  It does not forward mouse, keyboard, audio, or sensor data into the
  loaded page. Pages render as if no one is looking at them.

---

## 4. Unified Viewport Primitive

### 4.1 Type Definition

A new `ViewportRect` lives in `hypercolor-types/src/viewport.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewportRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
```

All fields are normalized `[0.0, 1.0]`. `x` and `y` are the top-left
corner. `width` and `height` are fractions of the source dimensions.

`ViewportRect::full()` returns `{ 0.0, 0.0, 1.0, 1.0 }`. `clamp()`
pins values into `[0.0, 1.0]` and enforces a minimum edge length of
0.02 to avoid zero-area crops that divide by zero in samplers.

`to_pixel_rect(source_width, source_height)` returns a pixel-space
`PixelRect { x: u32, y: u32, width: u32, height: u32 }` for samplers.
Rounding uses floor for origin and ceil for extent so the sampled
region always contains the normalized area.

`NormalizedRect` in `hypercolor-types/src/spatial.rs` is kept as a
distinct type, used by spatial zones. A `From<ViewportRect> for
NormalizedRect` impl handles conversion. We do not merge them because
zones may grow shape information (polygons, masks) that effect
viewports will not.

### 4.2 Fit Modes

The existing `FitMode` enum in screencast moves to
`hypercolor-types/src/viewport.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Contain,
    Cover,
    Stretch,
}
```

`Contain` preserves aspect with letterboxing. `Cover` center-crops to
match the target aspect. `Stretch` scales without preserving aspect.
Both screencast and web viewport use this enum as a dropdown control.

### 4.3 Sampling Helper

A free function `sample_viewport` in `hypercolor-core/src/spatial/viewport.rs`
takes a source buffer, a `ViewportRect`, a `FitMode`, and a target
canvas. It handles the full "crop then blit" path, using the existing
`blit_stretch`/`blit_contain`/`blit_cover` functions from screencast,
generalized over any source buffer. Screencast's existing helpers move
here.

---

## 5. Rect Control Type

### 5.1 Types Crate

`hypercolor-types/src/effect.rs` gains three variants.

`ControlType`:

```rust
pub enum ControlType {
    // existing variants unchanged
    Rect,
}
```

`ControlValue`:

```rust
pub enum ControlValue {
    // existing variants unchanged
    Rect(ViewportRect),
}
```

`ControlKind`:

```rust
pub enum ControlKind {
    // existing variants unchanged
    Rect,
}
```

`ControlDefinition` gains no new fields. Rect controls have no min/max
(they are implicit) and no labels or step. The `default_value` carries
the initial `ViewportRect`. The UI infers aspect-lock behavior from a
new `aspect_lock: Option<f32>` field on `ControlDefinition`. `None`
means free aspect. `Some(ratio)` means width/height is pinned at that
ratio. Other control types ignore the field.

### 5.2 Daemon JSON Conversion

`hypercolor-daemon/src/api/control_values.rs` gets a new branch in
`json_to_control_value()` that accepts:

```json
{ "x": 0.1, "y": 0.05, "width": 0.8, "height": 0.6 }
```

and produces `ControlValue::Rect(ViewportRect { ... })`. The serializer
round-trips as the same object shape. Validation rejects non-finite
values and out-of-range components.

### 5.3 SDK Factory

`sdk/packages/core/src/controls/specs.ts` gets a `rect()` factory
mirroring `num()`:

```typescript
export function rect(
  label: string,
  defaultValue: { x: number; y: number; width: number; height: number },
  opts?: RectOptions,
): ControlSpec<"rect">;

export interface RectOptions {
  tooltip?: string;
  group?: string;
  aspectLock?: number;
}
```

The build pipeline emits:

```html
<meta
  property="viewport"
  label="Viewport"
  type="rect"
  default="0,0,1,1"
  aspectLock="1.7777"
  group="Source"
/>
```

The daemon's meta parser at
`hypercolor-core/src/effect/meta_parser.rs` learns to parse the
`type="rect"` tag, default as four comma-separated floats, and
`aspectLock` as an optional f32.

---

## 6. ViewportPicker UI Widget

### 6.1 Extracted From Screencast

The composite widget at
`hypercolor-ui/src/components/control_panel/screen_cast.rs:1-379`
becomes a generic component at
`hypercolor-ui/src/components/control_panel/viewport_picker.rs`.

Signature:

```rust
#[component]
pub fn ViewportPicker(
    control_id: String,
    value: Signal<ViewportRect>,
    on_change: Callback<(String, serde_json::Value)>,
    preview_source: Signal<Option<CanvasFrame>>,
    accent_rgb: String,
    aspect_lock: Option<f32>,
    url_input: Option<UrlInputBinding>,
    aspect_ratio: String,
) -> impl IntoView
```

`UrlInputBinding` carries a `Signal<String>`, a commit callback, and a
placeholder. Passing `None` hides the URL field (screencast case).
Passing `Some(...)` renders a URL input above the preview (web viewport
case).

The geometry helpers in `hypercolor-ui/src/control_geometry.rs` stay
put and are reused. `FrameRect` becomes an alias for `ViewportRect`
via a re-export so existing tests continue to pass unchanged.

### 6.2 Control Panel Dispatch

`hypercolor-ui/src/components/control_panel/mod.rs:207-289` gains a new
arm for `ControlType::Rect` that renders `ViewportPicker` with
`preview_source` chosen from the effect's declared preview binding
(see section 6.3). The four-slider detection block
(`INTERACTIVE_RECT_CONTROL_IDS` and its companion function) is
deleted after screencast migrates in section 10.

### 6.3 Preview Binding

A rect control needs to know where to pull its preview frame from.
`ControlDefinition` gains a new optional field:

```rust
pub preview_source: Option<PreviewSource>,
```

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewSource {
    ScreenCapture,
    WebViewport,
    EffectCanvas,
}
```

The UI maps each variant to the correct `WsContext` signal.
`ScreenCapture` reads `ws.screen_canvas_frame`. `WebViewport` reads a
new `ws.web_viewport_canvas_frame`. `EffectCanvas` reads
`ws.canvas_frame`. Non-rect controls ignore the field. Future preview
sources (camera, media player) add variants without touching the UI
plumbing.

The SDK's `rect()` factory accepts a `preview?: 'screen' | 'web' | 'canvas'`
option that maps to this enum.

---

## 7. Web Viewport Effect

### 7.1 Registration

A new native effect at
`hypercolor-core/src/effect/builtin/web_viewport.rs` registers as
`EffectMetadata { id: "web_viewport", name: "Web Viewport", category:
EffectCategory::Source, ... }`. It ships in the builtin registry and
appears in `GET /api/v1/effects` alongside screencast.

### 7.2 Controls

| ID                 | Label         | Type     | Default               | Notes                         |
| ------------------ | ------------- | -------- | --------------------- | ----------------------------- |
| `url`              | URL           | Text     | `https://example.com` | HTTP, HTTPS, or file URLs     |
| `viewport`         | Viewport      | Rect     | full                  | `preview_source: WebViewport` |
| `fit_mode`         | Fit           | Dropdown | Cover                 | Contain / Cover / Stretch     |
| `brightness`       | Brightness    | Slider   | 1.0                   | `[0.0, 2.0]`, step 0.01       |
| `refresh_interval` | Refresh       | Slider   | 0                     | Seconds, 0 = never reload     |
| `render_width`     | Render Width  | Slider   | 1280                  | `[640, 1920]`, step 160       |
| `render_height`    | Render Height | Slider   | 720                   | `[360, 1080]`, step 90        |

Render width and height control the Servo render size, not the effect
canvas size. A 1280×720 render sampled through a viewport rect of
`{ 0.25, 0.25, 0.5, 0.5 }` produces a 640×360 crop that gets blitted
into the effect canvas via the chosen fit mode.

### 7.3 Renderer Lifecycle

`WebViewportRenderer` implements `EffectRenderer` from
`hypercolor-core/src/effect/traits.rs:77-142`.

On `init_with_canvas_size()`: acquire a Servo worker, create a new
session via the shared `ServoSessionHandle` primitive (see section 8),
record the initial URL, and issue the first load.

On `render_into()`: poll the session for the latest completed
framebuffer. If absent, return the last good canvas or a black
placeholder. If present, apply `sample_viewport()` with the current
viewport rect and fit mode, apply brightness, and write the result
into the target canvas.

On `set_control()`:

- `url`: if changed, debounce 250ms then issue a new load command to
  the session. Pending renders drain into the previous page's
  framebuffer.
- `viewport`: update the stored rect, no session interaction needed.
- `fit_mode`, `brightness`: store directly.
- `refresh_interval`: update the timer driver.
- `render_width`, `render_height`: resize the session's rendering
  context via a new `SessionCommand::Resize` (section 8).

On `destroy()`: send `SessionCommand::Close`, drain pending renders,
release worker slot.

### 7.4 Refresh Timer

A tokio interval runs at `refresh_interval` seconds (when non-zero) and
issues a reload command. Useful for pages that don't auto-update (news
sites, status dashboards). Zero disables the timer entirely.

### 7.5 No Input Injection

Unlike HTML effects, the web viewport does not inject `window.engine.*`
audio, sensor, keyboard, or mouse data. The preamble HTML wrapping
step at `hypercolor-core/src/effect/servo/worker.rs:204-248` is
skipped entirely. Pages render as passive viewers. If a page happens
to use `window.engine`, it sees `undefined` and degrades.

---

## 8. Servo Arbitrary URL Loading

### 8.1 ServoSessionHandle Primitive

The current `ServoRenderer` owns its session lifecycle directly and
couples it to HTML-effect concerns (metadata parsing, preamble
injection, temp file writing). The web viewport needs the session
without any of that.

A new `ServoSessionHandle` at
`hypercolor-core/src/effect/servo/session.rs` wraps the existing
`ServoWorkerClient` with a narrow command interface:

```rust
impl ServoSessionHandle {
    pub fn new(worker: Arc<ServoWorker>, config: SessionConfig) -> Result<Self>;
    pub fn load_url(&self, url: &str) -> Result<()>;
    pub fn load_html_file(&self, path: &Path) -> Result<()>;
    pub fn request_render(&self, input: FrameInput<'_>) -> Result<()>;
    pub fn poll_frame(&mut self) -> Option<Canvas>;
    pub fn resize(&self, width: u32, height: u32) -> Result<()>;
    pub fn close(self) -> Result<()>;
}

pub struct SessionConfig {
    pub render_width: u32,
    pub render_height: u32,
    pub inject_engine_globals: bool,
}
```

Both `ServoRenderer` and `WebViewportRenderer` consume this. The HTML
case sets `inject_engine_globals: true`. The web viewport case sets it
to `false`.

### 8.2 Worker Command

`hypercolor-core/src/effect/servo/worker.rs` gains a new command
variant:

```rust
enum WorkerCommand {
    // existing variants
    LoadUrl { session: SessionId, url: String, respond_to: Sender<LoadResult> },
}
```

The handler calls `webview.load(url)` directly with no file reading,
no preamble preparation, no temp file writing. It uses the same
load-complete detection and `LOAD_TIMEOUT` (5s) as the HTML path. On
failure it surfaces the error to the caller; on success it records
the load in session state.

### 8.3 Render Resolution

`SessionConfig.render_width` and `render_height` control the dimensions
passed to `SoftwareRenderingContext`. The existing context creation at
`hypercolor-core/src/effect/servo/worker.rs:818-822` moves behind a
resize-capable wrapper. On resize, the wrapper rebuilds the
`SoftwareRenderingContext` and notifies the webview via
`webview.resize()`. The next render targets the new dimensions.

### 8.4 Frame Readback and Sampling

The worker reads back the full render at `render_width × render_height`
via the existing
`rendering_context.read_to_image(DeviceIntRect::new_full(...))` at
`worker.rs:1000-1009`. It returns the full `ImageBuffer<Rgba<u8>>` to
the session handle, which returns it as a `Canvas`.

The web viewport renderer then calls `sample_viewport()` with its
current `ViewportRect` to crop and blit into the effect canvas. The
crop is applied in the effect, not the worker, so the worker stays
agnostic to effect-specific sampling policy.

### 8.5 Error Handling

Load failures (404, DNS error, timeout, network error) surface as soft
failures that the existing circuit breaker at
`hypercolor-core/src/effect/servo/circuit_breaker.rs:50-184` can catch.
Three consecutive failures open the breaker for 30 seconds. The
effect displays a black canvas with a log warning while the breaker
is open.

JS errors in the loaded page are logged at `debug` level without
failing the render. Console output is captured as usual.

---

## 9. Preview Stream

### 9.1 Daemon Side

The daemon needs to broadcast the full web viewport render so the UI
picker can overlay its rect. The existing `HypercolorBus` at
`hypercolor-core/src/bus.rs` already publishes `canvas_frame` and
`screen_canvas_frame` as watch channels. A new
`web_viewport_canvas_frame` watch channel joins them.

The renderer publishes each full render (pre-crop, `render_width ×
render_height`) into the channel. The WebSocket layer at
`hypercolor-daemon/src/ws/` forwards to subscribed clients using the
same mechanism as the other canvas frame channels.

Publication is gated on subscriber demand, matching the recent changes
at commits `99a2d82` and `74788cf`. With no UI subscribers, the
renderer skips the publication step. This matters because the full
render can be 1920×1080 RGBA — roughly 8.3 MB per frame before any
extra copies — and publishing that unconditionally burns bandwidth.

### 9.2 UI Side

`hypercolor-ui/src/context.rs` adds a `web_viewport_canvas_frame`
signal alongside the existing `canvas_frame` and `screen_canvas_frame`
signals. The WebSocket handler at
`hypercolor-tui/src/client/ws.rs` and the UI's own WebSocket client
learn the new channel name.

`ViewportPicker` with `preview_source: WebViewport` subscribes to this
signal and renders it through `CanvasPreview`. The rect overlay uses
the same drag/resize math regardless of source.

---

## 10. Screencast Migration

### 10.1 Control Schema Change

`hypercolor-core/src/effect/builtin/screen_cast.rs:81-166` replaces
its four slider controls with one rect control:

```rust
ControlDefinition {
    id: "viewport".into(),
    name: "Frame".into(),
    kind: ControlKind::Rect,
    control_type: ControlType::Rect,
    default_value: ControlValue::Rect(ViewportRect::full()),
    preview_source: Some(PreviewSource::ScreenCapture),
    aspect_lock: None,
    ..Default::default()
}
```

The `SourceRect` private struct at `screen_cast.rs:138-144` and the
`normalized_crop()` helper are removed. Their logic moves into
`sample_viewport()` in `hypercolor-core/src/spatial/viewport.rs`.
`blit_stretch`, `blit_contain`, `blit_cover` also move there, reused
by both effects.

### 10.2 Control Panel Cleanup

`hypercolor-ui/src/components/control_panel/screen_cast.rs` is
deleted entirely after `ViewportPicker` ships. The detection block
at `control_panel/mod.rs:207-289` (`INTERACTIVE_RECT_CONTROL_IDS` and
its companion function) is removed. The `ControlWidget` dispatch
handles `ControlType::Rect` through `ViewportPicker`.

### 10.3 Backwards Compatibility

User configs that reference screencast's old `frame_x`, `frame_y`,
`frame_width`, `frame_height` control values need a one-shot migration
in the profile loader. When loading a saved effect state with those
four float controls, combine them into a single `viewport` rect
control and log a one-time info message. Unknown control IDs already
get dropped by the daemon's control reconciliation, so the migration
is additive.

Favorites and scenes that pin screencast with the old controls get
rewritten on first load. The migration lives at
`hypercolor-daemon/src/library/migration.rs` as a new module.

---

## 11. Security Posture

Hypercolor is a self-hosted personal tool. Users load URLs they choose
on their own machines against their own networks. The web viewport
effect does not attempt to sandbox, restrict, or validate URLs beyond
basic URL parsing.

The effect description states this plainly:

> Loads the URL in Servo and samples a region of the rendered page.
> The page runs with full JavaScript, network access, and local storage.
> Do not point this at untrusted URLs.

Servo's existing preference set at
`hypercolor-core/src/effect/servo/worker.rs:504-544` already disables
ServiceWorkers, WebRTC, WebXR, GamePad, and IndexedDB. JIT is
disabled. Network HTTP cache is disabled. These defaults stay in
place. The web viewport effect does not relax them.

Future work can layer policy on top: an allowlist in daemon config, a
per-profile trust tier, a "render-only no-JS" mode. None are in scope
here. The design keeps the door open by threading `SessionConfig`
through the session handle, so adding policy fields later does not
require rewiring the render path.

---

## 12. Delivery Waves

The spec ships in six waves. Each wave is landable independently and
leaves the tree green.

### Wave 1 — Types and Schema

- `ViewportRect` and `FitMode` in `hypercolor-types/src/viewport.rs`
- `ControlType::Rect`, `ControlValue::Rect`, `ControlKind::Rect` in
  `hypercolor-types/src/effect.rs`
- `ControlDefinition.preview_source` and `aspect_lock` fields
- `PreviewSource` enum
- JSON conversion in `hypercolor-daemon/src/api/control_values.rs`
- Unit tests for clamp, rect serialization, JSON round-trip

### Wave 2 — SDK Integration

- `rect()` factory in `sdk/packages/core/src/controls/specs.ts`
- Build pipeline emits `type="rect"` meta tags
- Meta parser learns the rect tag format
- SDK tests for factory output and parsing round-trip

### Wave 3 — ViewportPicker Widget

- Extract `screen_cast.rs` widget into `viewport_picker.rs`
- Parameterize over preview source, aspect lock, optional URL input
- Dispatch `ControlType::Rect` through `ViewportPicker` in
  `control_panel/mod.rs`
- Geometry reuses `control_geometry.rs` unchanged
- Keep the four-slider detection alive in parallel for screencast

### Wave 4 — Servo Session Primitive

- Extract `ServoSessionHandle` at
  `hypercolor-core/src/effect/servo/session.rs`
- Add `WorkerCommand::LoadUrl` and the direct URL load path
- Make rendering context resizable
- Refactor `ServoRenderer` to consume the handle
- Tests for load-url, resize, and inject-globals toggle

### Wave 5 — Web Viewport Effect

- `WebViewportRenderer` at
  `hypercolor-core/src/effect/builtin/web_viewport.rs`
- Register in builtin effects
- Wire `sample_viewport()` from the shared helper
- Add `web_viewport_canvas_frame` watch channel with demand gating
- WebSocket forwarding for the new channel
- UI context signal and WebSocket binding
- End-to-end test: load a known URL, verify a frame publishes and
  cropping works

### Wave 6 — Screencast Migration

- Replace screencast's four sliders with a single `viewport` rect
- Move sampling helpers into
  `hypercolor-core/src/spatial/viewport.rs`
- Delete `hypercolor-ui/src/components/control_panel/screen_cast.rs`
- Remove four-slider detection from `control_panel/mod.rs`
- Profile migration at
  `hypercolor-daemon/src/library/migration.rs`
- Update screencast tests to use the rect control

---

## 13. Known Constraints

### 13.1 Servo Load Time Dominates Throughput

A full page load takes hundreds of milliseconds for HTTPS sites. The
effect freezes on the previous frame during load. For the refresh
timer case, this is expected behavior. For URL-change debouncing,
250ms keeps typing responsive without triggering loads per keystroke.

### 13.2 Page-Paint Pacing Is Unpredictable

A static blog renders once and never repaints. A live dashboard
repaints on data updates, maybe once per second. A WebGL demo
repaints at 60fps. The render thread calls `webview.paint()` every
tick regardless. On static pages this is cheap; on animated pages it
honors their internal rate limits. Nothing to tune here; the Servo
integration already handles this.

### 13.3 Render Resolution Costs Memory

A 1920×1080 RGBA buffer is 8 MB. Two (double-buffered readback) is 16
MB. The preview stream broadcasts another copy. Total memory per
active web viewport effect is roughly 30 MB plus Servo's own working
set. Acceptable, but users who run multiple web viewports at 1080p
should expect the daemon RSS to climb.

### 13.4 Render Pipeline Modernization Overlap

Spec 28 (render pipeline modernization) is reshaping surface
ownership in `hypercolor-daemon/src/render_thread/`. Wave 5 of this
spec depends on surface ownership being stable. Waves 1 through 3 are
independent and can land in parallel. Wave 4 touches `servo/` only,
so it is also independent. Wave 5 should wait until spec 28 has
landed its surface-ownership changes.

### 13.5 No GPU Path

Servo currently uses `SoftwareRenderingContext`. A future GPU path
(discussed in design/28) would apply here unchanged — the session
handle abstracts the context choice. No work here to prepare for that;
the interface already fits.

---

## 14. Verification Strategy

### 14.1 Unit Tests

- `ViewportRect::clamp` enforces bounds and minimum edge length
- `ViewportRect::to_pixel_rect` produces correct pixel bounds for
  representative source sizes
- `sample_viewport` with each fit mode matches golden crops at small
  source sizes
- JSON round-trip for `ControlValue::Rect` preserves all fields
- `ServoSessionHandle::load_url` rejects empty and malformed URLs
- Profile migration combines four-slider screencast configs correctly

### 14.2 Integration Tests

- `tests/web_viewport_tests.rs` in `hypercolor-core`: load a local
  `file://` URL pointing at a fixture page, request a render, verify a
  frame comes back and the expected region is sampled. Use a static
  fixture to avoid network flakiness.
- `tests/screencast_migration_tests.rs` in `hypercolor-daemon`: load
  a saved profile with the four-slider shape, verify it loads under
  the new rect control and the viewport values match.

### 14.3 UI Tests

- `tests/viewport_picker_tests.rs` in `hypercolor-ui`: mount the
  widget with a fixture preview frame, simulate pointer drag on the
  rect, verify emitted `on_change` carries the expected rect.
- `tests/control_geometry_tests.rs` (existing): still passes after
  `FrameRect` becomes an alias for `ViewportRect`.

### 14.4 Manual Verification

- Load `https://example.com` in the web viewport effect. Verify the
  page renders and the viewport rect crops correctly.
- Load `https://www.google.com/maps` and pan the viewport over the
  map. Verify smooth interactive preview.
- Load a local dev server (`http://localhost:3000`) with hot reload.
  Verify updates propagate.
- Load an invalid URL. Verify the effect falls back to black and the
  log records the failure.
- Open the control panel in the UI. Drag the viewport rect. Verify
  the crop updates live on connected devices.
- Migrate a saved scene that uses screencast. Verify it loads
  correctly under the new control schema.
- Enable and disable the refresh timer on a page that does not
  auto-update. Verify reloads happen at the expected interval.

---

## Appendix A — File Inventory

New files:

- `crates/hypercolor-types/src/viewport.rs`
- `crates/hypercolor-core/src/spatial/viewport.rs`
- `crates/hypercolor-core/src/effect/servo/session.rs`
- `crates/hypercolor-core/src/effect/builtin/web_viewport.rs`
- `crates/hypercolor-ui/src/components/control_panel/viewport_picker.rs`
- `crates/hypercolor-daemon/src/library/migration.rs`
- `crates/hypercolor-core/tests/viewport_tests.rs`
- `crates/hypercolor-core/tests/web_viewport_tests.rs`
- `crates/hypercolor-daemon/tests/screencast_migration_tests.rs`
- `crates/hypercolor-ui/tests/viewport_picker_tests.rs`
- `sdk/packages/core/src/controls/rect.ts`

Modified files:

- `crates/hypercolor-types/src/effect.rs`
- `crates/hypercolor-core/src/effect/builtin/screen_cast.rs`
- `crates/hypercolor-core/src/effect/servo/renderer.rs`
- `crates/hypercolor-core/src/effect/servo/worker.rs`
- `crates/hypercolor-core/src/effect/meta_parser.rs`
- `crates/hypercolor-core/src/bus.rs`
- `crates/hypercolor-daemon/src/api/control_values.rs`
- `crates/hypercolor-daemon/src/ws/` (channel wiring)
- `crates/hypercolor-ui/src/context.rs`
- `crates/hypercolor-ui/src/components/control_panel/mod.rs`
- `crates/hypercolor-tui/src/client/ws.rs`
- `sdk/packages/core/src/controls/specs.ts`

Removed files:

- `crates/hypercolor-ui/src/components/control_panel/screen_cast.rs`

---

## Appendix B — Control Flow Diagrams

### B.1 Web Viewport Render Path

```
EffectEngine::tick()
  ↓ render_into(FrameInput, target_canvas)
WebViewportRenderer::render_into()
  ↓ ServoSessionHandle::poll_frame()
  ↓ (returns full RENDER_WIDTH × RENDER_HEIGHT Canvas)
sample_viewport(source, viewport_rect, fit_mode, target_canvas)
  ↓ crop then blit
target_canvas (effect canvas dimensions)
  ↓ spatial sampler
ZoneColors → device backends
```

### B.2 Preview Publication Path

```
WebViewportRenderer::render_into()
  ↓ ServoSessionHandle::poll_frame() → full render
HypercolorBus::publish_web_viewport_canvas(frame)
  ↓ watch channel (gated on subscriber demand)
WebSocket forwarder
  ↓ binary canvas frame
UI WsContext.web_viewport_canvas_frame signal
  ↓
ViewportPicker → CanvasPreview (live page)
  ↓ rect overlay drawn on top
User drags rect → on_change → PATCH /api/v1/effects/current/controls
  ↓
WebViewportRenderer::set_control("viewport", rect)
  ↓ sample_viewport uses the new rect next frame
```

---
