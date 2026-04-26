# 43. Spec: `hypercolor-leptos-ext`

**Status:** Implementation spec. Revised for buildability after full Cinder doc review. Targets Year 1 Phase 0 per RFC 40.
**Date:** 2026-04-23.
**Author:** Bliss (with Nova).
**Depends on:** [35](35-next-gen-web-framework.md), [36](36-cinder-web-rfc.md), [37](37-cinder-stream-rfc.md), [41](41-cinder-boundary-contract.md).
**Supersedes:** `cinder-leptos-ext` prototype mentioned in RFC 41 Section 12.

## Summary

`hypercolor-leptos-ext` is an internal Hypercolor crate that prototypes the `cinder-web` and `cinder-stream` primitives in-tree before extraction to standalone published crates. It ships inside `crates/hypercolor-leptos-ext/` as a path dependency of `hypercolor-ui` (and `hypercolor-daemon` for the server-side codec pieces). API shapes match the intended public surface of RFCs 36 and 37 so extraction is mechanical renaming, not rewriting.

This crate is the **Year 1 canary** mandated by RFC 41's scope cut. It validates:

1. Typed DOM event parsing eliminates the 32 `HtmlInputElement` dyn_into sites.
2. A narrow raw-WebSocket `BinaryChannel<T, P>` layer reduces `hypercolor-ui/src/ws/` by 60-70%, while borrowing proven stream semantics from RSocket where useful.
3. A RAII RAF scheduler eliminates the 13 `Closure::*` construction sites and the `Rc<RefCell<Option<Closure>>>` pattern.
4. A typed `Canvas` + `WebGl` wrapper collapses the 284-line `preview_runtime/webgl.rs` to roughly 120 lines.
5. Schema-evolved binary frames via `#[derive(BinaryFrame)]` eliminate the 19-call `Uint8Array::get_index` decoder.
6. A preview-media spike determines whether visual frames should remain WebSocket messages or move to browser-native media machinery (`VideoDecoder` / WebRTC).

Success metrics are measured against `docs/archive/2026-03-cinder-audit-snapshot.txt` before and after each migration PR lands.

## Non-goals

- **Not a published crate.** Lives in `crates/hypercolor-leptos-ext/` as a workspace path dependency. Not released to crates.io until Year 2 extraction.
- **Not framework-agnostic.** Tightly coupled to Leptos 0.8 signals, context, and resources. When we extract to `cinder-stream` and `cinder-web`, the framework-agnostic layers come with us; the Leptos-specific wrappers stay behind as `cinder-stream/leptos` and `cinder-web/leptos` feature code.
- **Not a replacement for `hypercolor-ui`.** It is a dependency of `hypercolor-ui` that exposes reusable primitives. UI components still live in the UI crate.
- **Not complete coverage of `web_sys`.** Targets the ~40 most-used interfaces in Hypercolor. Anything else reaches `.raw()` and uses `web_sys` directly.
- **Not a perfect prototype.** API shapes may evolve as we migrate. The spec is a starting point, not a frozen contract.
- **Not a cargo workspace split.** One crate, one directory, one `Cargo.toml`. Feature flags for optional pieces.

## Buildability Constraints

The crate is intentionally split by feature so it can participate in both the native Hypercolor workspace and the excluded WASM UI crate without breaking either build.

- **Default features are empty.** `cargo check --workspace` must not accidentally compile browser-only modules on the native host target.
- **Browser modules are `wasm32`-gated.** `events`, `canvas`, `raf`, `prelude`, and `ws-client-wasm` compile only for `wasm32-unknown-unknown`.
- **Daemon imports only portable/server features.** `hypercolor-daemon` uses `features = ["ws-core", "axum"]`.
- **UI imports browser features explicitly.** `hypercolor-ui` uses `features = ["events", "canvas", "raf", "prelude", "ws-client-wasm", "leptos"]`.
- **No unsafe code.** Hypercolor forbids `unsafe_code`; event wrappers may use safe `JsCast` APIs, but the public API must not expose `unsafe fn` or require undocumented unsafe blocks.
- **WebGPU is not in the first migration.** `wgpu` is current at `29.x` and is expensive for compile time and bundle size. The prototype keeps WebGL first and leaves `webgpu` behind an explicit opt-in feature.
- **No dependency on stale stream stacks.** RFC 37 treats RSocket as protocol inspiration, not the default dependency path. PR 5 starts with a short RSocket health check; only a live maintained implementation earns a deeper adoption spike.
- **Do not confuse preview media with app state.** The control/state stream is still in scope. The visual preview path is explicitly allowed to use WebCodecs or WebRTC if a spike proves that browser-native video beats custom frame messages.

## Dependencies

- **Workspace parent:** `hypercolor` top-level `Cargo.toml` gains `hypercolor-leptos-ext` as a workspace member with default features disabled. The macro subcrate lives at `crates/hypercolor-leptos-ext-macros/` so it can be checked independently.
- **Direct dependencies of `hypercolor-leptos-ext`:**
  - `leptos = "0.8"` (optional behind `leptos`; match `hypercolor-ui`'s version)
  - `leptos-use = "0.18"` (optional behind `leptos`; matches current `hypercolor-ui`)
  - `wasm-bindgen = "0.2"` (optional behind browser-side features)
  - `wasm-bindgen-futures = "0.4"` (optional behind `ws-client-wasm`)
  - `web-sys = "0.3"` with features enumerated (optional behind browser-side features)
  - `js-sys = "0.3"` (optional behind browser-side features)
  - `gloo-events = "0.3"` (optional behind `events`; RAII event listener lifecycle)
  - `bytes = "1"`
  - `zerocopy = "0.8"` (header layer of binary codec)
  - `musli = "0.0"` (body layer, upgrade-stable mode)
  - `serde = "1"` + `serde_json = "1"` (metadata, fixtures, and session records)
  - `async-trait = "0.1"` (transport traits)
  - `async-compression = { version = "0.4", features = ["futures-io", "zstd"] }` (session compression)
  - `thiserror = "2"` (library errors)
  - `tracing = "0.1"` (logging)
  - `futures = "0.3"`
  - `pin-project-lite = "0.2"`
  - `send_wrapper = "0.6"` (for `!Send` values in `Send` contexts)
  - `wgpu = "29"` (optional, only behind `webgpu`)
- **Candidate RSocket dependencies** (only added if the RFC 37 health check finds a maintained implementation and the deeper spike passes):
  - `rsocket_rust = "0.7"`
  - `rsocket_rust_transport_websocket = "0.7"` (daemon/server-side WebSocket transport)
  - `rsocket_rust_transport_wasm = "0.7"` (browser WASM WebSocket transport)
- **Dev-dependencies:**
  - `wasm-bindgen-test = "0.3"`
  - `proptest = "1"`
  - `insta = "1"` (snapshot testing)
  - `criterion = "0.7"` (codec benchmark)

- **Daemon-side dependencies** (when the daemon imports the crate for codec/channel types):
  - `axum = "0.8"` (behind `axum` feature)
  - `tokio-tungstenite = "0.24"` (behind `axum` feature)
  - `tower = "0.5"` (behind `axum` feature)
  - `tower-http = "0.6"` (behind `axum` feature)

## Feature flags

Matches the intended `cinder-stream` feature matrix from RFC 37 so extraction is mechanical:

```toml
[features]
default = []

# Core browser-side primitives
ws-core = []                 # binary frame traits, codec, channels, reconnect
ws-client-wasm = ["ws-core"] # browser WebSocket transport, wasm32 only
events = ["dep:gloo-events"] # typed DOM events, wasm32 only
canvas = []                  # canvas + WebGL helpers, wasm32 only
webgpu = ["canvas", "dep:wgpu"] # optional WebGPU helper path
raf = []                     # request-animation-frame scheduler, wasm32 only
prelude = []                 # timers, page lifecycle, user preferences, wasm32 only

# Server-side integration (daemon imports with this)
axum = ["ws-core", "dep:axum", "dep:tower", "dep:tower-http", "dep:tokio-tungstenite"]

# Leptos integration (on by default in this crate since it's Leptos-specific)
leptos = []

# Opt-in for devtools hooks (Year 2)
devtools = []
```

Hypercolor UI imports with `default-features = false` and `features = ["events", "canvas", "raf", "prelude", "ws-client-wasm", "leptos"]`. Daemon imports with `default-features = false` and `features = ["ws-core", "axum"]`.

## Crate layout

```
crates/hypercolor-leptos-ext/
├── Cargo.toml
├── README.md                       # Points to docs/design/43 and audit snapshot
├── src/
│   ├── lib.rs                      # Re-exports, crate docs, feature gates
│   │
│   ├── events/
│   │   ├── mod.rs                  # StaticEvent trait, attach functions
│   │   ├── attach.rs               # on(), on_with_options(), EventHandle
│   │   ├── options.rs              # EventOptions builder
│   │   ├── types/
│   │   │   ├── mod.rs              # Re-exports
│   │   │   ├── mouse.rs            # Click, DblClick, MouseDown/Up/Move, ContextMenu
│   │   │   ├── keyboard.rs         # KeyDown, KeyUp, KeyPress
│   │   │   ├── pointer.rs          # PointerDown/Up/Move/Enter/Leave/Cancel
│   │   │   ├── touch.rs            # TouchStart/End/Move/Cancel
│   │   │   ├── wheel.rs            # Wheel
│   │   │   ├── input.rs            # Input, Change (with typed value::<T>())
│   │   │   ├── focus.rs            # Focus, Blur
│   │   │   ├── drag.rs             # DragStart/End/Enter/Over/Leave/Drop
│   │   │   ├── animation.rs        # AnimationStart/End/Iteration
│   │   │   ├── transition.rs       # TransitionRun/Start/End/Cancel
│   │   │   └── custom.rs           # Custom event wrapper, for webglcontextlost etc
│   │   └── macros.rs               # make_event! macro (dominator-style)
│   │
│   ├── canvas/
│   │   ├── mod.rs                  # Canvas newtype + context acquisition
│   │   ├── canvas.rs               # Canvas::from_element/by_id + size/rect
│   │   ├── ctx2d.rs                # Ctx2d wrapper: put_image_rgba, draw_pixels, Style
│   │   ├── webgl.rs                # WebGl wrapper: compile_vertex, link_program, etc
│   │   ├── texture.rs              # Texture wrapper: upload_2d, update_2d
│   │   ├── format.rs               # PixelFormat enum with stride/GL constant
│   │   ├── error.rs                # GlError, CanvasError
│   │   ├── style.rs                # Style enum, Gradient, Pattern
│   │   └── geometry.rs             # Point, Size, Rect, DomRect wrapper
│   │
│   ├── raf/
│   │   ├── mod.rs                  # Re-exports
│   │   ├── scheduler.rs            # Scheduler + FrameInfo + RAII
│   │   └── animation_frame.rs      # Lower-level AnimationFrameHandle
│   │
│   ├── ws/                         # [ws-core + transport feature gates]
│   │   ├── mod.rs                  # Re-exports
│   │   ├── frame.rs                # BinaryFrame trait, BinaryFrameEncode/Decode
│   │   ├── macros.rs               # Re-export #[derive(BinaryFrame)] from macro crate
│   │   ├── schema.rs               # SchemaRange, negotiation helpers
│   │   ├── channel.rs              # BinaryChannel<T, P>, BackpressurePolicy variants
│   │   ├── transport/
│   │   │   ├── mod.rs              # CinderTransport trait + MaybeSend alias
│   │   │   ├── websocket_wasm.rs   # WebSocketTransport (browser, wraps web_sys::WebSocket)
│   │   │   ├── websocket_native.rs # WebSocketTransport (server, tokio-tungstenite) [axum feature]
│   │   │   └── in_memory.rs        # InMemoryTransport for tests
│   │   ├── reconnect.rs            # Connector, Reconnecting<C, P>, ReconnectPolicy, ExponentialBackoff
│   │   ├── rpc.rs                  # RpcRequest/RpcResponse, RpcClient
│   │   └── websocket_raw.rs        # Raw web_sys::WebSocket wrapper per RFC 41 Seam 1
│   │
│   ├── axum/                       # [axum feature]
│   │   ├── mod.rs                  # Server-side integration
│   │   ├── middleware.rs           # Bearer auth, origin validation, host validation, rate limit
│   │   ├── upgrade.rs              # WebSocket upgrade handler wrapping ws::Transport
│   │   └── csp.rs                  # Recommended CSP helper (doc-only, no enforcement)
│   │
│   ├── leptos/                     # [leptos feature]
│   │   ├── mod.rs                  # Re-exports
│   │   ├── signal.rs               # UnsyncBroadcast<T> -> Signal<T> adapters
│   │   ├── ws_hook.rs              # use_binary_channel(), use_rpc()
│   │   ├── canvas_hook.rs          # use_canvas(), use_webgl(), use_webgpu()
│   │   ├── raf_hook.rs             # use_animation_frame()
│   │   └── prefs_hook.rs           # use_user_preferences()
│   │
│   ├── prelude/
│   │   ├── mod.rs                  # Re-exports
│   │   ├── timers.rs               # set_timeout, set_interval (RAII)
│   │   ├── lifecycle.rs            # page_visible_signal, online_signal
│   │   ├── preferences.rs          # user_preferences() + ColorScheme/Direction
│   │   ├── focus.rs                # FocusScope, focus_next/previous/first
│   │   └── performance.rs          # now_ms, wall_clock_ms
│   │
│   └── utils/
│       ├── mod.rs                  # MaybeSend alias, UnsyncBroadcast<T>
│       ├── maybe_send.rs           # cfg alias
│       └── raw.rs                  # escape-hatch convention helpers
│
├── tests/
│   ├── events_tests.rs             # wasm-bindgen-test; typed events
│   ├── canvas_tests.rs             # wasm-bindgen-test; Ctx2d, WebGl
│   ├── raf_tests.rs                # wasm-bindgen-test; scheduler RAII
│   ├── ws_codec_tests.rs           # Native; BinaryFrame round-trip
│   ├── ws_channel_tests.rs         # Native; InMemoryTransport + policies
│   ├── ws_reconnect_tests.rs       # Native; Connector + Reconnecting
│   ├── ws_schema_tests.rs          # Native; schema evolution proptest
│   └── fixtures/
│       ├── canvas_frame_v1.bin     # Real Hypercolor canvas frame, captured from daemon
│       ├── canvas_frame_v2.bin     # Forward-compat fixture
│       └── spectrum_frame_v1.bin
│
├── benches/
│   └── codec.rs                    # Criterion bench; decode vs current hand-rolled
│
└── ui-tests/                       # trybuild macro diagnostic tests
    └── binary_frame_errors.rs
```

The proc-macro crate is a sibling workspace member at `crates/hypercolor-leptos-ext-macros/`, not a nested crate. Keeping it as a sibling makes `cargo check --workspace` and later extraction to `cinder-stream-macros` straightforward.

## Subsystem specs

### 1. `events/` — typed DOM events

**Goal:** eliminate the 32 `HtmlInputElement` dyn_into sites in `hypercolor-ui/src/components/control_panel/*.rs` and `layout_builder.rs`.

**Core trait:**

```rust
pub trait StaticEvent: Sized {
    /// DOM event name as listened for (e.g. "click", "input").
    const EVENT_TYPE: &'static str;

    /// Construct from the raw event. Called only by listeners registered for `EVENT_TYPE`.
    fn from_event_unchecked(event: web_sys::Event) -> Self;

    /// Escape hatch to the raw event.
    fn raw(&self) -> &web_sys::Event;
}
```

**Macro for types:**

```rust
macro_rules! make_event {
    ($name:ident, $event_type:expr, $raw:ty) => {
        pub struct $name {
            event: $raw,
        }

        impl StaticEvent for $name {
            const EVENT_TYPE: &'static str = $event_type;
            fn from_event_unchecked(event: web_sys::Event) -> Self {
                Self { event: event.unchecked_into() }
            }
            fn raw(&self) -> &web_sys::Event { self.event.as_ref() }
        }

        impl $name {
            pub fn raw_as_inner(&self) -> &$raw { &self.event }
            pub fn stop_propagation(&self) { self.event.stop_propagation() }
            pub fn prevent_default(&self) { self.event.prevent_default() }
            pub fn target<T: JsCast>(&self) -> Option<T> {
                self.event.target().and_then(|t| t.dyn_into().ok())
            }
            pub fn current_target<T: JsCast>(&self) -> Option<T> {
                self.event.current_target().and_then(|t| t.dyn_into().ok())
            }
        }
    };
}
```

**Input event with typed value:**

```rust
make_event!(Input, "input", web_sys::Event);

impl Input {
    /// Parse the current event target as an `<input>` and read its value.
    pub fn value<T: FromStr>(&self) -> Option<T> {
        self.target::<web_sys::HtmlInputElement>()
            .and_then(|el| el.value().parse::<T>().ok())
    }

    pub fn checked(&self) -> Option<bool> {
        self.target::<web_sys::HtmlInputElement>().map(|el| el.checked())
    }

    pub fn files(&self) -> Option<web_sys::FileList> {
        self.target::<web_sys::HtmlInputElement>().and_then(|el| el.files())
    }
}
```

**Attachment:**

```rust
pub struct EventHandle {
    _listener: gloo_events::EventListener,  // internal; gloo handles the Closure lifecycle
}

impl EventHandle {
    /// Leak the listener so it lives for the app's lifetime.
    pub fn forget(self) {
        // gloo handles this internally
    }
}

pub fn on<E, F>(target: &impl AsRef<web_sys::EventTarget>, mut handler: F) -> EventHandle
where
    E: StaticEvent + 'static,
    F: FnMut(E) + 'static,
{
    let listener = gloo_events::EventListener::new(
        target.as_ref(),
        E::EVENT_TYPE,
        move |event| {
            let typed = E::from_event_unchecked(event.clone());
            handler(typed);
        },
    );
    EventHandle { _listener: listener }
}

pub fn on_with_options<E, F>(
    target: &impl AsRef<web_sys::EventTarget>,
    options: EventOptions,
    mut handler: F,
) -> EventHandle
where
    E: StaticEvent + 'static,
    F: FnMut(E) + 'static,
{
    let listener = gloo_events::EventListener::new_with_options(
        target.as_ref(),
        E::EVENT_TYPE,
        options.into_gloo(),
        move |event| {
            let typed = E::from_event_unchecked(event.clone());
            handler(typed);
        },
    );
    EventHandle { _listener: listener }
}

pub struct EventOptions {
    pub capture: bool,
    pub passive: Option<bool>,
    pub once: bool,
}
```

**Event types to ship (v0.1):**

Mouse: `Click, DblClick, MouseDown, MouseUp, MouseMove, MouseEnter, MouseLeave, MouseOver, MouseOut, ContextMenu`
Keyboard: `KeyDown, KeyUp, KeyPress`
Pointer: `PointerDown, PointerUp, PointerMove, PointerEnter, PointerLeave, PointerOver, PointerOut, PointerCancel`
Touch: `TouchStart, TouchEnd, TouchMove, TouchCancel`
Wheel: `Wheel`
Input: `Input, Change, Submit, Reset, Invalid`
Focus: `Focus, Blur, FocusIn, FocusOut`
Drag: `DragStart, Drag, DragEnd, DragEnter, DragOver, DragLeave, Drop`
Animation: `AnimationStart, AnimationEnd, AnimationIteration, AnimationCancel`
Transition: `TransitionRun, TransitionStart, TransitionEnd, TransitionCancel`
Custom: `Custom` plus `on_custom(...)` for arbitrary event names

**Custom event pattern:**

```rust
pub struct Custom {
    name: &'static str,
    event: web_sys::Event,
}

pub fn on_custom<F: FnMut(web_sys::Event) + 'static>(
    target: &impl AsRef<web_sys::EventTarget>,
    name: &'static str,
    mut handler: F,
) -> EventHandle {
    // Uses gloo_events directly
}
```

Used for `webglcontextlost` / `webglcontextrestored` which are not standard enough to get their own struct.

**Migration target (`hypercolor-ui/src/components/control_panel/number.rs`):**

Before:

```rust
on:input=move |ev| {
    if let Some(el) = ev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
        if let Ok(v) = el.value().parse::<f64>() {
            on_change.run(v);
        }
    }
}
```

After:

```rust
use hypercolor_leptos_ext::events::Input;
on:input=move |ev: Input| {
    if let Some(v) = ev.value::<f64>() {
        on_change.run(v);
    }
}
```

### 2. `canvas/` — typed Canvas and WebGL

**Canvas newtype:**

```rust
pub struct Canvas {
    el: web_sys::HtmlCanvasElement,
}

impl Canvas {
    pub fn from_element(el: web_sys::HtmlCanvasElement) -> Self;
    pub fn by_id(id: &str) -> Result<Self, CanvasError>;

    pub fn width(&self) -> u32;
    pub fn height(&self) -> u32;
    pub fn set_size(&self, width: u32, height: u32);
    pub fn bounding_rect(&self) -> Rect;

    pub fn context_2d(&self) -> Result<Ctx2d, CanvasError>;
    pub fn context_webgl(&self) -> Result<WebGl, CanvasError>;
    pub fn context_webgl2(&self) -> Result<WebGl2, CanvasError>;

    #[cfg(feature = "wgpu")]
    pub async fn context_webgpu(&self, instance: &wgpu::Instance) -> Result<wgpu::Surface<'static>, CanvasError>;

    pub fn raw(&self) -> &web_sys::HtmlCanvasElement;
    pub fn into_raw(self) -> web_sys::HtmlCanvasElement;
}

impl AsRef<web_sys::HtmlCanvasElement> for Canvas {
    fn as_ref(&self) -> &web_sys::HtmlCanvasElement { &self.el }
}

#[derive(thiserror::Error, Debug)]
pub enum CanvasError {
    #[error("canvas element not found: {0}")]
    NotFound(String),
    #[error("cast failed: expected HtmlCanvasElement")]
    WrongType,
    #[error("2d context unavailable")]
    Ctx2dUnavailable,
    #[error("webgl context unavailable")]
    WebGlUnavailable,
    #[error("webgl2 context unavailable")]
    WebGl2Unavailable,
    #[error("webgpu context unavailable: {0}")]
    WebGpu(String),
}
```

**`context_webgl` with experimental-webgl fallback:**

```rust
pub fn context_webgl(&self) -> Result<WebGl, CanvasError> {
    let ctx = self.el.get_context("webgl")
        .map_err(|_| CanvasError::WebGlUnavailable)?
        .or_else(|| {
            self.el.get_context("experimental-webgl").ok().flatten()
        })
        .ok_or(CanvasError::WebGlUnavailable)?;

    let ctx: web_sys::WebGlRenderingContext = ctx.dyn_into()
        .map_err(|_| CanvasError::WebGlUnavailable)?;

    Ok(WebGl { ctx })
}
```

**`Ctx2d`:**

```rust
pub struct Ctx2d {
    ctx: web_sys::CanvasRenderingContext2d,
}

#[derive(Clone, Debug)]
pub enum Style {
    Hex(String),
    Rgb(u8, u8, u8),
    Rgba(u8, u8, u8, f32),
}

impl Ctx2d {
    pub fn fill_style(&self, style: Style) {
        self.ctx.set_fill_style_str(&style.to_css());
    }

    pub fn stroke_style(&self, style: Style) { ... }
    pub fn line_width(&self, width: f64) { self.ctx.set_line_width(width); }
    pub fn clear_rect(&self, rect: Rect) { ... }
    pub fn fill_rect(&self, rect: Rect) { ... }
    pub fn stroke_rect(&self, rect: Rect) { ... }

    /// Upload raw RGBA pixels directly.
    pub fn put_image_rgba(&self, pixels: &[u8], size: Size, at: Point) -> Result<(), Ctx2dError>;

    /// Convenience for the Hypercolor color-wheel pattern.
    pub fn draw_pixels(&self, rect: Rect, rgba: &[u8]) -> Result<(), Ctx2dError>;

    pub fn arc(&self, center: Point, radius: f64, angles: Range<f64>) -> Result<(), Ctx2dError>;
    pub fn begin_path(&self) { self.ctx.begin_path(); }
    pub fn close_path(&self) { self.ctx.close_path(); }
    pub fn fill(&self) { self.ctx.fill(); }
    pub fn stroke(&self) { self.ctx.stroke(); }

    pub fn with_path<F: FnOnce(&Ctx2d)>(&self, build: F) {
        self.begin_path();
        build(self);
        self.close_path();
    }

    pub fn raw(&self) -> &web_sys::CanvasRenderingContext2d { &self.ctx }
}
```

**`WebGl` and `Texture`:**

```rust
pub struct WebGl {
    ctx: web_sys::WebGlRenderingContext,
}

pub struct Shader {
    shader: web_sys::WebGlShader,
}

pub struct Program {
    gl: web_sys::WebGlRenderingContext,
    program: web_sys::WebGlProgram,
}

pub struct Texture {
    gl: web_sys::WebGlRenderingContext,
    tex: web_sys::WebGlTexture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    R8,
    Rg8,
    Rgb8,
    Rgba8,
    Luminance8,
    // WebGL2-only formats hidden behind WebGl2 variant
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize { ... }
    pub fn gl_format(self) -> u32 { ... }  // GL_RGB, GL_RGBA, etc
    pub fn gl_type(self) -> u32 { ... }    // GL_UNSIGNED_BYTE, etc
}

impl WebGl {
    pub fn compile_vertex(&self, src: &str) -> Result<Shader, CompileError>;
    pub fn compile_fragment(&self, src: &str) -> Result<Shader, CompileError>;
    pub fn link_program(&self, vs: &Shader, fs: &Shader) -> Result<Program, LinkError>;

    pub fn create_texture(&self) -> Texture;
    pub fn clear_errors(&self);
    pub fn check_error(&self) -> Option<GlError>;

    pub fn raw(&self) -> &web_sys::WebGlRenderingContext;
}

impl Texture {
    pub fn bind(&self, unit: u32);

    /// Upload fresh pixels. Internally dispatches to the right tex_image_2d overload.
    pub fn upload_2d(&self, format: PixelFormat, size: Size, pixels: &[u8]) -> Result<(), GlError>;

    /// Update a sub-region. Internally dispatches to tex_sub_image_2d.
    pub fn update_2d(&self, region: Rect, pixels: &[u8]) -> Result<(), GlError>;

    pub fn set_filter(&self, min: Filter, mag: Filter);
    pub fn set_wrap(&self, s: Wrap, t: Wrap);
}

#[derive(thiserror::Error, Debug)]
pub enum GlError {
    #[error("out of memory")]
    OutOfMemory,
    #[error("invalid operation: {0}")]
    InvalidOperation(String),
    #[error("other GL error: 0x{0:x}")]
    Other(u32),
}

#[derive(thiserror::Error, Debug)]
pub enum CompileError {
    #[error("shader compile failed:\n{0}")]
    Compile(String),
    #[error("shader object creation failed")]
    Create,
}

#[derive(thiserror::Error, Debug)]
pub enum LinkError {
    #[error("program link failed:\n{0}")]
    Link(String),
    #[error("program object creation failed")]
    Create,
}
```

**Migration target (`hypercolor-ui/src/components/preview_runtime/webgl.rs`):**

The current 284-line file becomes roughly 120 lines. The context acquisition dance (5 levels of `.ok().flatten()`) becomes one line. The `tex_image_2d_with_*` overload selection becomes `texture.upload_2d(PixelFormat::Rgba8, size, &pixels)`.

### 3. `raf/` — request-animation-frame scheduler

```rust
pub struct Scheduler {
    inner: Rc<SchedulerInner>,
}

struct SchedulerInner {
    window: web_sys::Window,
    callback: RefCell<Option<Closure<dyn FnMut(f64)>>>,
    pending_frame: Cell<Option<i32>>,
    active: Cell<bool>,
    user: RefCell<Box<dyn FnMut(FrameInfo)>>,
    last_time: Cell<Option<f64>>,
    dropped_frames: Cell<u32>,
    mode: Cell<Mode>,
}

#[derive(Clone, Copy)]
enum Mode {
    Idle,
    OneShot,
    Continuous,
}

pub struct FrameInfo {
    pub time_ms: f64,          // requestAnimationFrame timestamp
    pub monotonic_ms: f64,     // performance.now()
    pub delta_ms: f64,         // time since last RAF callback
    pub dropped_frames: u32,   // estimated
}

impl Scheduler {
    pub fn new<F: FnMut(FrameInfo) + 'static>(callback: F) -> Self {
        let window = web_sys::window().expect("window unavailable");
        let inner = Rc::new(SchedulerInner {
            window,
            callback: RefCell::new(None),
            pending_frame: Cell::new(None),
            active: Cell::new(false),
            user: RefCell::new(Box::new(callback)),
            last_time: Cell::new(None),
            dropped_frames: Cell::new(0),
            mode: Cell::new(Mode::Idle),
        });
        Self { inner }
    }

    /// Request one frame. Idempotent: calling twice while a frame is pending is a no-op.
    pub fn schedule(&self) {
        if self.inner.pending_frame.get().is_some() { return; }
        self.inner.mode.set(Mode::OneShot);
        self.inner.active.set(true);
        self.request_next_frame();
    }

    /// Request continuous frames until `pause()` is called.
    pub fn schedule_continuous(&self) {
        self.inner.mode.set(Mode::Continuous);
        self.inner.active.set(true);
        self.request_next_frame();
    }

    pub fn pause(&self) {
        self.inner.active.set(false);
        if let Some(id) = self.inner.pending_frame.take() {
            let _ = self.inner.window.cancel_animation_frame(id);
        }
    }

    pub fn is_pending(&self) -> bool {
        self.inner.pending_frame.get().is_some()
    }

    fn request_next_frame(&self) { ... }  // Wraps requestAnimationFrame
}

impl Drop for SchedulerInner {
    fn drop(&mut self) {
        if let Some(id) = self.pending_frame.take() {
            let _ = self.window.cancel_animation_frame(id);
        }
        // callback is dropped, which drops the Closure
    }
}
```

**Migration target (`hypercolor-ui/src/components/canvas_preview.rs`):**

The 400+ line RAF scheduling logic with `Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>` latching pattern becomes:

```rust
let scheduler = Scheduler::new({
    let present = present.clone();
    move |frame: FrameInfo| {
        present.render(frame.time_ms);
    }
});
scheduler.schedule();  // on first frame arrival
```

The `schedule` Rc is shared across the effect closures that want to request a repaint.

### 4. `ws/` — binary channels, codec, reconnect

This is the core of `cinder-stream` 0.1. Everything in this module is what Year 1 ships to crates.io (after extraction).

#### 4.1 `BinaryFrame` derive macro

Subcrate `hypercolor-leptos-ext-macros/` because proc-macros live in their own crate.

```rust
#[derive(hypercolor_leptos_ext::BinaryFrame)]
#[frame(tag = 0x03, schema = 1)]
pub struct CanvasFrameV1 {
    #[header(le)] pub frame_number: u32,
    #[header(le)] pub timestamp_ms: u32,
    #[header(le)] pub width: u16,
    #[header(le)] pub height: u16,
    #[header]     pub format: u8,
    #[body(rest)] pub pixels: Bytes,
}
```

**Generated items:**

1. `#[repr(C, packed)]` zerocopy header struct
2. `impl BinaryFrameEncode for CanvasFrameV1`
3. `impl BinaryFrameDecode for CanvasFrameV1`
4. `impl BinaryFrameSchema for CanvasFrameV1` (const `TAG: u8`, `SCHEMA: u8`, `NAME: &'static str`)

Implementation approach: parse fields by attribute, emit zerocopy structure for `#[header]` fields contiguously, emit musli-encoded body for `#[body]` fields, and a `#[body(rest)]` field gets the remaining bytes. Error path uses a dedicated `DecodeError` type.

For v0.1, we implement the parser-generator for:

- `#[header]` with `le` and `be`
- `#[body(rest)]` (must be last)
- `#[frame(tag, schema)]`
- `#[since(N)]` and `#[default = "fn_path"]` for schema evolution

**Not in v0.1 macro** (deferred to v0.2):

- `#[until(N)]` for field removal
- `#[convert_from(N, "fn_path")]` for migrations
- `#[body]` non-rest fields (single musli-body frames only for now)

#### 4.2 `CinderTransport` trait

```rust
// utils/maybe_send.rs
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

// ws/transport/mod.rs
#[async_trait(?Send)]
pub trait CinderTransport: MaybeSend + 'static {
    type SendError: std::error::Error + MaybeSend + Sync + 'static;
    type RecvError: std::error::Error + MaybeSend + Sync + 'static;

    async fn send(&mut self, frame: Bytes) -> Result<(), Self::SendError>;
    async fn recv(&mut self) -> Result<Option<Bytes>, Self::RecvError>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::SendError>>;
    async fn close(&mut self) -> Result<(), Self::SendError>;
}

#[async_trait(?Send)]
pub trait CinderDatagramTransport: MaybeSend + 'static {
    type DatagramError: std::error::Error + MaybeSend + Sync + 'static;
    async fn send_datagram(&mut self, payload: Bytes) -> Result<(), Self::DatagramError>;
    async fn recv_datagram(&mut self) -> Result<Option<Bytes>, Self::DatagramError>;
}
```

#### 4.3 `WebSocketTransport` (WASM)

Protocol adoption note: sections 4.3 through 4.8 describe the expected Hypercolor channel facade. RSocket remains a semantic reference unless the RFC 37 health check finds a maintained Rust/WASM implementation worth shipping.

Year 1 ships the raw variant per RFC 41 phasing. Wraps `web_sys::WebSocket` directly via `wasm-bindgen-futures` and an internal mpsc.

```rust
pub struct WebSocketTransport {
    ws: web_sys::WebSocket,
    recv_rx: futures::channel::mpsc::UnboundedReceiver<Bytes>,
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _on_close: Closure<dyn FnMut(web_sys::CloseEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::Event)>,
    state: WsState,
}

enum WsState { Connecting, Open, Closed }

impl WebSocketTransport {
    pub async fn connect(url: impl Into<String>) -> Result<Self, WsConnectError>;
    pub async fn connect_with_protocols(url: impl Into<String>, protocols: &[&str]) -> Result<Self, WsConnectError>;
}

#[async_trait(?Send)]
impl CinderTransport for WebSocketTransport {
    type SendError = WsSendError;
    type RecvError = WsRecvError;
    // ...
}
```

Year 2 replaces the raw `web_sys::WebSocket` plumbing with a dependency on a (future) `cinder-web::net::WebSocket`. Internal detail; public API stays.

#### 4.4 `WebSocketTransport` (native)

Behind the `axum` feature. Wraps `tokio-tungstenite` for client and `axum::extract::ws::WebSocket` for server upgrade handlers.

#### 4.5 `InMemoryTransport`

For tests and replay:

```rust
pub struct InMemoryTransport {
    tx: tokio::sync::mpsc::UnboundedSender<Bytes>,
    rx: tokio::sync::mpsc::UnboundedReceiver<Bytes>,
}

impl InMemoryTransport {
    pub fn pair() -> (Self, Self) {
        let (tx_a, rx_b) = tokio::sync::mpsc::unbounded_channel();
        let (tx_b, rx_a) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx: tx_a, rx: rx_a }, Self { tx: tx_b, rx: rx_b })
    }
}
```

#### 4.6 `BinaryChannel<T, P>`

```rust
pub trait BackpressurePolicy: MaybeSend + 'static {
    fn on_full(queue: &mut VecDeque<Bytes>, frame: Bytes) -> OverflowAction;
}

pub enum OverflowAction {
    Accepted,
    Dropped { dropped_frames: u32 },
    Block,
}

pub struct DropOldest;
pub struct DropNewest;
pub struct Latest;
pub struct Queue<const N: usize>;
pub struct BlockOnFull;

pub struct BinaryChannel<T: BinaryFrame, P: BackpressurePolicy, Tr: CinderTransport = DynTransport> {
    transport: Tr,
    queue: VecDeque<Bytes>,
    metrics: Arc<ChannelMetrics>,
    _marker: PhantomData<(T, P)>,
}

impl<T: BinaryFrame, P: BackpressurePolicy, Tr: CinderTransport> BinaryChannel<T, P, Tr> {
    pub fn builder(transport: Tr) -> BinaryChannelBuilder<T, P, Tr>;

    pub async fn send(&mut self, frame: T) -> Result<(), SendError<T>>;
    pub fn try_send(&mut self, frame: T) -> Result<(), TrySendError<T>>;
    pub async fn recv(&mut self) -> Result<Option<T>, RecvError>;

    pub fn metrics(&self) -> ChannelMetrics;
}

pub type DynTransport = Box<dyn CinderTransport<SendError = CinderError, RecvError = CinderError>>;
```

#### 4.7 `Connector` + `Reconnecting<C, P>`

Per codex pass-2 finding: `Reconnecting` needs a factory to reopen a dropped transport.

```rust
#[async_trait(?Send)]
pub trait Connector: MaybeSend + 'static {
    type Transport: CinderTransport;
    type Error: std::error::Error + MaybeSend + Sync + 'static;
    async fn connect(&mut self) -> Result<Self::Transport, Self::Error>;
}

// Blanket impl for closures
impl<F, Fut, T, E> Connector for F where
    F: FnMut() -> Fut + MaybeSend + 'static,
    Fut: Future<Output = Result<T, E>> + MaybeSend,
    T: CinderTransport,
    E: std::error::Error + MaybeSend + Sync + 'static,
{ ... }

pub struct Reconnecting<C: Connector, P: ReconnectPolicy> {
    connector: C,
    policy: P,
    state: ConnectionState<C::Transport>,
    subscription_replay: Vec<SubscribeCatalogEntry>,
}

impl<C: Connector, P: ReconnectPolicy> Reconnecting<C, P> {
    pub fn new(connector: C, policy: P) -> Self;
    pub async fn connect(&mut self) -> Result<(), ReconnectError<C::Error>>;
    pub fn add_subscription(&mut self, entry: SubscribeCatalogEntry);
}

impl<C: Connector, P: ReconnectPolicy> CinderTransport for Reconnecting<C, P> {
    // On failure in send/recv/close, invokes policy.next_delay(), awaits, calls connector.connect(), replays subscriptions
}

pub trait ReconnectPolicy: MaybeSend + 'static {
    fn next_delay(&mut self, attempt: u32, outcome: ReconnectOutcome) -> Option<Duration>;
    fn reset(&mut self);
}

pub struct ExponentialBackoff {
    pub base: Duration,
    pub max: Duration,
    pub multiplier: f64,
    pub jitter: Jitter,
}

impl ExponentialBackoff {
    pub const HYPERCOLOR_DEFAULT: Self = Self {
        base: Duration::from_millis(500),
        max: Duration::from_secs(15),
        multiplier: 2.0,
        jitter: Jitter::Equal(0.25),
    };
}
```

#### 4.8 `SessionRecorder` + `SessionPlayer`

Per RFC 37 wire format. Implement the core types to validate the format; file output is optional for v0.1.

```rust
pub enum SessionRecord {
    TransportFrame { channel_id: u16, direction: Direction, bytes: Bytes },
    Metadata { channel_id: u16, key: String, value: serde_json::Value },
    External { source: &'static str, body: Bytes },
}

pub struct SessionRecorder<W: AsyncWrite + Unpin> {
    writer: async_compression::futures::write::ZstdEncoder<W>,
    started_at_ns: u64,
}

impl<W: AsyncWrite + Unpin> SessionRecorder<W> {
    pub async fn new(writer: W, channels: &[ChannelDescriptor]) -> Result<Self, ReplayError>;
    pub fn tap<T, P, Tr>(&self, channel_id: u16, chan: &BinaryChannel<T, P, Tr>) -> TappedChannel<T, P>;
    pub async fn record_external(&mut self, source: &'static str, body: Bytes) -> Result<(), ReplayError>;
    pub async fn finish(self) -> Result<(), ReplayError>;
}

pub struct SessionPlayer<R: AsyncRead + Unpin> {
    reader: async_compression::futures::bufread::ZstdDecoder<futures::io::BufReader<R>>,
    channels: Vec<ChannelDescriptor>,
}

impl<R: AsyncRead + Unpin> SessionPlayer<R> {
    pub async fn open(reader: R) -> Result<Self, ReplayError>;
    pub fn channels(&self) -> &[ChannelDescriptor];
    pub async fn play(self, pace: Pace) -> InMemoryTransport;
    pub async fn external_records(&mut self, source: &str) -> impl Stream<Item = (Duration, Bytes)>;
}
```

### 5. `axum/` — server-side integration (feature-gated)

```rust
#[cfg(feature = "axum")]
pub mod axum {
    use ::axum::{extract::WebSocketUpgrade, response::Response};
    use tower::ServiceBuilder;

    pub struct CinderSecurityLayer { /* internal */ }

    impl CinderSecurityLayer {
        pub fn builder() -> CinderSecurityLayerBuilder;
    }

    pub struct CinderSecurityLayerBuilder {
        origin_allow_list: Vec<String>,
        host_allow_list: Vec<String>,
        bearer_tokens: TokenConfig,
        rate_limits: RateLimits,
        cors: Cors,
    }

    impl CinderSecurityLayerBuilder {
        pub fn origin_allow_list(mut self, origins: &[&str]) -> Self;
        pub fn host_allow_list(mut self, hosts: &[&str]) -> Self;
        pub fn require_bearer_token(mut self, yes: bool) -> Self;
        pub fn bearer_tokens(mut self, config: TokenConfig) -> Self;
        pub fn rate_limits(mut self, limits: RateLimits) -> Self;
        pub fn cors(mut self, cors: Cors) -> Self;
        pub fn build(self) -> CinderSecurityLayer;
    }

    pub async fn upgrade_handler<F, Fut>(
        ws: WebSocketUpgrade,
        on_connect: F,
    ) -> Response
    where
        F: FnOnce(axum::extract::ws::WebSocket) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        ws.on_upgrade(on_connect)
    }
}
```

This mirrors Hypercolor's existing `security.rs` middleware stack. The goal is that `hypercolor-daemon` imports this crate with `features = ["ws", "axum"]` and replaces the hand-rolled middleware with `CinderSecurityLayer`.

### 6. `leptos/` — Leptos integration (feature-gated)

```rust
#[cfg(feature = "leptos")]
pub mod leptos {
    use ::leptos::prelude::*;

    /// Wrap a BinaryChannel in a Leptos Signal of the latest decoded frame.
    pub fn use_binary_channel<T: BinaryFrame + Clone + 'static>(
        url: &str,
        policy: impl BackpressurePolicy,
    ) -> (Signal<ConnectionState>, Signal<Option<T>>);

    /// RAF scheduler as a Leptos effect.
    pub fn use_animation_frame<F: FnMut(FrameInfo) + 'static>(callback: F) -> AnimationFrameHandle;

    /// User preferences as a Leptos signal bundle.
    pub fn use_user_preferences() -> UserPreferencesSignals;

    pub struct UserPreferencesSignals {
        pub reduced_motion: Signal<bool>,
        pub high_contrast: Signal<bool>,
        pub color_scheme: Signal<ColorScheme>,
    }
}
```

### 7. `prelude/` — small utilities

```rust
// Timers with RAII
pub struct TimeoutHandle { /* internal */ }
impl Drop for TimeoutHandle { /* clearTimeout */ }

pub fn set_timeout<F: FnOnce() + 'static>(delay: Duration, callback: F) -> TimeoutHandle;
pub fn set_interval<F: FnMut() + 'static>(interval: Duration, callback: F) -> IntervalHandle;

// Page lifecycle
pub fn page_visible_signal() -> UnsyncBroadcast<bool>;
pub fn online_signal() -> UnsyncBroadcast<bool>;
pub fn before_unload<F: FnMut() + 'static>(handler: F) -> EventHandle;

// User preferences
pub struct UserPreferences { /* fields */ }
pub fn user_preferences() -> UserPreferences;

// Focus management
pub struct FocusScope { /* internal */ }
impl FocusScope {
    pub fn new() -> Self;
    pub fn contain(self) -> Self;
    pub fn restore_on_exit(self) -> Self;
    pub fn autofocus_first(self) -> Self;
    pub fn enter(&self);
    pub fn exit(&self);
}

// Clipboard
pub async fn clipboard_read_text() -> Result<String, ClipboardError>;
pub async fn clipboard_write_text(text: &str) -> Result<(), ClipboardError>;

// Performance
pub fn now_ms() -> f64;
pub fn wall_clock_ms() -> f64;

// Shared signal primitive
pub struct UnsyncBroadcast<T> { /* thread-local watch-like channel */ }
```

## Migration PR plan

Six PRs, grouped by phase. Each PR is its own review, its own CI run, its own measurement against `cinder-audit-snapshot.txt`.

**Phase A is the RFC 41 Year 1 canary:** PR 1, PR 5, and PR 6. These must land first if the goal is extracting `cinder-stream 0.1`.

**Phase B is the browser-ergonomics prototype:** PR 2, PR 3, and PR 4. These can land earlier only if they do not delay the stream canary; otherwise they move behind PR 6.

### PR 1: `hypercolor-leptos-ext` scaffolding

- Workspace member added.
- Crate skeleton compiles.
- Empty `lib.rs` with feature flags matching the spec.
- `hypercolor-leptos-ext-macros/` subcrate with skeleton `BinaryFrame` derive.
- `scripts/cinder-audit.sh` run in CI.
- No Hypercolor code changes yet.

**Size:** small PR, ~2-3 days.

### PR 2: `events/` module complete + control-panel migration

- All 40 event types shipping.
- `on()`, `on_with_options()`, `EventOptions`, `EventHandle`.
- `Input::value::<T>()`, `.checked()`, `.files()`.
- Migrate `hypercolor-ui/src/components/control_panel/*.rs` (5 files, 25+ sites).
- Migrate `hypercolor-ui/src/components/layout_builder.rs`.
- Migrate `hypercolor-ui/src/components/layout_zone_properties.rs`.
- Tests: `events_tests.rs` with `wasm-bindgen-test` mounting a button/input and asserting typed event fires.

**Expected LOC reduction:** -200 LOC in hypercolor-ui, +600 LOC in ext crate. Net +400 but reusable.

**Validation:** `cinder-audit-snapshot.txt` shows `dyn_into` count dropping from 50 to ~17. `HtmlInputElement` casts dropping from 32 to 0.

### PR 3: `raf/` scheduler + `canvas_preview.rs` migration

- `Scheduler::new`, `schedule`, `schedule_continuous`, `pause`, `is_pending`.
- RAII cleanup via `Drop`.
- Migrate `hypercolor-ui/src/components/canvas_preview.rs`: RAF loop collapses from 400 lines to ~80.
- Tests: `raf_tests.rs` with `wasm-bindgen-test` asserting scheduler fires exactly once per `schedule()` call and cancels on drop.

**Expected LOC reduction:** canvas_preview.rs 594 → ~200 LOC.

**Validation:** `Closure::*` count drops from 13 to ~5. `Rc<RefCell<Option<Closure>>>` count drops to 0.

### PR 4: `canvas/` module + `preview_runtime/webgl.rs` migration

- `Canvas`, `Ctx2d`, `WebGl`, `Texture`, `PixelFormat` fully implemented.
- Migrate `hypercolor-ui/src/components/preview_runtime/webgl.rs`: 284 → ~120 LOC.
- Migrate `hypercolor-ui/src/components/color_wheel.rs`: `Ctx2d::draw_pixels` replaces `ImageData::new_with_u8_clamped_array_and_sh`. 355 → ~280 LOC.
- Migrate `hypercolor-ui/src/components/preview_runtime/canvas2d.rs`.
- Tests: `canvas_tests.rs` asserting context acquisition works and texture upload roundtrips.

**Validation:** `tex_image_2d_with_*` count drops from 2 to 0. `new_with_u8_clamped_array` drops from 3 to 0.

### PR 5: `ws/` module + `BinaryFrame` derive + `hypercolor-v2` codec migration

- Mandatory first task: run the RFC 37 RSocket health check. If no maintained Rust/WASM implementation exists, document that and proceed with the Hypercolor channel path.
- Mandatory second task: run the RFC 37 preview-media spike. Compare current raw/JPEG WebSocket preview against WebCodecs and WebRTC for latency, CPU, daemon dependency cost, browser support, color fidelity, and implementation size. Do not migrate canvas pixels to `CanvasFrameV2` until this decision is made.
- `BinaryFrame` derive implemented for tag + schema + header + body layout.
- `WebSocketTransport` (WASM raw variant).
- `BinaryChannel<T, P>` with all five policies.
- `Connector` + `Reconnecting<C, P>`.
- `ExponentialBackoff::HYPERCOLOR_DEFAULT`.
- Migrate `hypercolor-ui/src/ws/messages.rs`: `decode_preview_frame` becomes `CanvasFrameV1::decode(bytes)`. 652 → ~150 LOC.
- Wire protocol: introduce `hypercolor-v2` for control/state unless the health check finds a maintained RSocket implementation and a deeper spike passes. During migration, the daemon advertises both the old and new paths and the UI prefers the new path.
- Daemon-side: `hypercolor-daemon/src/api/ws/` incrementally adopts the chosen channel abstraction for command/reply, subscriptions, spectrum, metrics, and display-preview control. Canvas/display pixels stay on the current path until the preview-media spike chooses their final transport.
- Tests: `ws_codec_tests.rs` with proptest round-tripping `CanvasFrameV1` and `CanvasFrameV2` (forward-compat).
- Tests: `ws_channel_tests.rs` with `InMemoryTransport` exercising latest-value and backpressure behavior. If RSocket unexpectedly passes the gate, add equivalent adapter tests.
- Tests: `ws_reconnect_tests.rs` simulating connection drop, asserting reconnect recovers with the configured policy.
- Fixture files: real frames captured from a running Hypercolor daemon committed at `tests/fixtures/`.

**Expected LOC reduction:** ws/messages.rs 652 → ~150, ws/connection.rs 644 → ~250. Total UI ws layer: 1,745 → ~500-600.

**Validation:** `Uint8Array::get_index` count drops from 19 to 0.

### PR 6: `axum/` middleware + daemon security migration

- `CinderSecurityLayer` with bearer auth, origin/host validation, rate limits, CORS.
- Migrate `hypercolor-daemon/src/api/security.rs` to layer on `CinderSecurityLayer`. Preserve existing token-file format for backward compatibility.
- Add Host header allow-list (closes Hypercolor security gap #1 from RFC 39).
- Add CSP header emission on UI responses (closes gap #2).
- Explicit WebSocket Origin validation on upgrade (closes gap #3).
- Tests: `ws_security_tests.rs` asserting 401/421/403 responses per the threat model.

**Expected LOC impact:** daemon api/ws/ 5,201 → ~3,800.

## Current in-tree canary snapshot

As of the first implementation pass, the in-tree canary has shipped a narrower version of the plan above. The implementation intentionally proves the browser and stream seams before attempting the full framework extraction.

Completed:

- Workspace crates `hypercolor-leptos-ext` and `hypercolor-leptos-ext-macros`.
- Feature-gated modules for `events`, `canvas`, `raf`, `prelude`, `ws-core`, `ws-client-wasm`, `axum`, and `leptos`.
- Binary frame derive, schema negotiation, replay tapes, RPC frames, backpressure queues, reconnect policy, WASM WebSocket transport, and Axum WebSocket transport.
- Browser helpers for typed document/window access, worker message handlers, timers, sleep, monotonic time, random sampling, page location, viewport metrics, localStorage, and console logging.
- Canvas helpers for canvas creation, context acquisition, image data construction, Blob URLs, worker script URLs, WebGL texture upload, WebGL buffer upload, and bitmap worker probes.
- Hypercolor UI migrations for direct `window`, `localStorage`, console, WebSocket construction, WebSocket JSON sends, Blob URL creation, worker script URL creation, worker frame posting, and WebGL buffer upload.

Still deliberately deferred:

- Full `Canvas`, `Ctx2d`, `WebGl`, `Texture`, and `PixelFormat` wrapper types. The current crate exposes focused helpers because that preserved UI behavior while proving the boundary.
- CI execution for the `wasm-bindgen-test` browser suites. The suites exist and compile for the wasm test target, but the runner is not wired into CI yet.
- Full control-panel, color-wheel, and canvas-preview component rewrites. Remaining UI `web_sys` references are mostly event type signatures, file upload/FormData, canvas handles, and rendering contexts rather than repeated raw browser plumbing.
- Daemon-side `hypercolor-v2` control/state channel migration. The stream codec exists, but old daemon JSON message paths remain while the preview-media decision stays open.
- README extraction plan for `cinder-stream`.

Current audit result: no direct `web_sys::window()`, `localStorage`, console calls, WebSocket constructors, WebSocket text sends, Blob URL creation/revocation, `Float32Array`, `Date::now`, or `Math::random` remain in `hypercolor-ui/src/`. The remaining `js_sys::ArrayBuffer` use is the typed input for binary preview frame decode.

## Validation metrics

Every PR runs `scripts/cinder-audit.sh` before and after. Success criteria:

| Metric                          | Before (current) | Target after PR 6 | Source                  |
| ------------------------------- | ---------------- | ----------------- | ----------------------- |
| `dyn_into` total                | 50               | ≤ 5               | audit snapshot          |
| `HtmlInputElement` casts        | 32               | 0                 | audit snapshot          |
| `Closure::*`                    | 13               | ≤ 3               | audit snapshot          |
| `Rc<RefCell<Option<Closure>>>`  | 0                | 0                 | snapshot                |
| `tex_image_2d_with_*`           | 2                | 0                 | snapshot                |
| `new_with_u8_clamped_array`     | 3                | 0                 | snapshot                |
| `Uint8Array::get_index`         | 19               | 0                 | snapshot                |
| UI `src/ws/` LOC                | 1,745            | ≤ 600             | snapshot                |
| Daemon `api/ws/` LOC (no tests) | 5,201            | ≤ 4,000           | snapshot                |
| UI wasm gzipped                 | 7,378,595 bytes  | no regression     | snapshot                |
| `just verify` time              | current baseline | ±10%              | `hyperfine just verify` |
| Incremental build time          | current baseline | ±10%              | `time cargo build`      |

CI fails a PR if any regression exceeds 10% from the pre-PR baseline.

## Testing

### Native unit tests

`cargo test --lib` covers codec internals, type conversions, `ExponentialBackoff` math, `Style` enum, `PixelFormat` byte-stride, `BackpressurePolicy` dispatch.

### Proptest round-trips

`tests/ws_codec_tests.rs`:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn canvas_frame_v1_roundtrip(
        frame_number: u32,
        timestamp_ms: u32,
        width: u16,
        height: u16,
        format in 0u8..=2,
        pixels in prop::collection::vec(any::<u8>(), 0..=1024),
    ) {
        let frame = CanvasFrameV1 { frame_number, timestamp_ms, width, height, format, pixels: pixels.into() };
        let encoded = frame.encode();
        let decoded = CanvasFrameV1::decode(&encoded).expect("encoded frame decodes");
        prop_assert_eq!(frame, decoded);
    }

    #[test]
    fn v1_decoder_tolerates_v2_bytes(/* v2-shaped arbitrary input */) {
        // v1 decoder reads up to its expected size, ignores tail
    }
}
```

### Wasm-bindgen-test

`tests/events_tests.rs`:

```rust
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn click_event_delivers_typed_target() {
    let button = document().create_element("button").expect("button element created");
    let button: HtmlButtonElement = button.dyn_into().expect("button element type");
    document()
        .body()
        .expect("document body exists")
        .append_child(&button)
        .expect("button appended");

    let clicks = Rc::new(RefCell::new(Vec::new()));
    let clicks_inner = clicks.clone();
    let handle = on(&button, move |ev: Click| {
        clicks_inner.borrow_mut().push(ev.client_x());
    });

    let mut init = MouseEventInit::new();
    init.client_x(42).client_y(7);
    let event = MouseEvent::new_with_mouse_event_init_dict("click", &init).expect("click event created");
    button.dispatch_event(&event).expect("click dispatched");

    yield_microtask().await;
    assert_eq!(*clicks.borrow(), vec![42]);

    drop(handle);
    button.dispatch_event(&event).expect("click dispatched after drop");
    yield_microtask().await;
    assert_eq!(*clicks.borrow(), vec![42]); // drop removed listener
}
```

### Fixture-driven fuzzing

`cargo fuzz` targets:

- `fuzz_targets/canvas_frame_decode.rs`: random bytes fed to `CanvasFrameV1::decode()`. Assert no panic, no unbounded alloc.
- `fuzz_targets/spectrum_frame_decode.rs`: same for `SpectrumFrameV1`.

### Benchmark

`benches/codec.rs` with criterion:

```rust
fn bench_decode_canvas_frame(c: &mut Criterion) {
    let bytes = include_bytes!("../tests/fixtures/canvas_frame_v1.bin");
    c.bench_function("canvas_frame_v1_decode", |b| {
        b.iter(|| CanvasFrameV1::decode(black_box(bytes)).expect("fixture decodes"));
    });
    c.bench_function("canvas_frame_v1_decode_hand_rolled", |b| {
        b.iter(|| decode_preview_frame_hand_rolled(black_box(bytes)));
    });
}
```

Expected: `BinaryFrame` decode is within 10% of the hand-rolled loop, ideally faster due to `zerocopy`'s single-pass read.

### Macro diagnostics

`ui-tests/binary_frame_errors.rs` using `trybuild`. Negative test corpus:

```rust
#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("ui-tests/missing_tag.rs");
    t.compile_fail("ui-tests/header_after_body_rest.rs");
    t.compile_fail("ui-tests/unknown_attribute.rs");
}
```

Each fixture has a `.stderr` file with the expected diagnostic.

## Extraction to public crates (Year 1 Q4 or Year 2 Q1)

Once the Hypercolor migration validates the primitives:

1. **Extract `ws/`, `axum`, and the codec macro** to `cinder-stream` standalone repo / crate. Rename imports: `hypercolor_leptos_ext::ws::BinaryChannel` becomes `cinder_stream::BinaryChannel`, and rename the macro crate to `cinder-stream-macros`. If a maintained RSocket implementation unexpectedly passes the gate, extract the Hypercolor-tested adapter facade instead of publishing a competing wire protocol.
2. **Publish `cinder-stream 0.1` to crates.io.** Version pinned.
3. **`hypercolor-leptos-ext` depends on `cinder-stream`** going forward. Its `ws/` module becomes a thin re-export.
4. **Year 2: extract `events/`, `canvas/`, `raf/`, `prelude/`** to `cinder-web` similarly.
5. **Year 2: extract `leptos/`** to `cinder-web/leptos` and `cinder-stream/leptos` feature modules.
6. **Retire `hypercolor-leptos-ext`** once both extractions are complete. The crate becomes a compatibility shim or disappears.

API shapes match the RFC-specified public surfaces, so renaming is mechanical. No design churn expected.

## Risks and open questions

1. **RSocket Rust staleness.** RSocket has the right protocol vocabulary, but the current Rust/WASM crates look too stale for a foundational dependency. Mitigation: make PR 5 start with a health check, borrow semantics, and proceed with `hypercolor-v2` unless a live implementation exists.

2. **`BinaryFrame` derive complexity.** The proc-macro must handle header-then-body layout, schema evolution attributes, and endianness correctly. First implementation likely has bugs. Mitigation: extensive `trybuild` + proptest coverage; fuzz on day one.

3. **`Connector` abstraction ergonomics.** Users may find the async-closure bound awkward. If so, provide helper macros:

   ```rust
   let connector = connect_ws!("ws://127.0.0.1:9420");
   ```

4. **`!Send` vs `Send` confusion.** The `MaybeSend` alias works but may surprise users porting native tokio code that assumes `Send`. Document clearly and provide escape hatches.

5. **Axum version churn.** Axum 0.8 vs 0.9 breaks middleware layer shapes. Pin tightly and document upgrade cadence.

6. **`gloo_events` dependency.** The `EventHandle` internals delegate to `gloo_events`. If gloo moves slowly, we may inline the event-listener RAII. For v0.1, gloo is fine.

7. **`wasm-bindgen-test` flakiness.** Browser-hosted tests occasionally flake. Mitigation: keep test count modest, rely on native tests for most logic.

8. **Hypercolor UI bundle size.** Each PR adds some code, some removes. Monitor gzipped bundle closely; don't let it balloon.

9. **Migration order.** The ordering I've proposed (events → raf → canvas → ws → axum) is defensible but not the only way. An alternative is to start with `ws/` because it's the Year 1 canary; this would deliver the cinder-stream extraction sooner but would surface integration bugs later when the `events/` migration also touches component code.

10. **Daemon-side migration churn.** The daemon currently has 1,828 lines of `api/ws/tests.rs`. Preserving those while migrating the underlying codec is real work. Budget accordingly.

11. **What if gold-path migration reveals an API defect?** The RFC specifies shapes, but implementations can find problems the spec didn't anticipate. The ext crate is the place to discover and fix them before crates.io publication. Budget for at least one API revision cycle per module.

## Timeline

Rough estimate for one principal maintainer at 25-40% time:

- PR 1 (scaffolding): 1 week
- PR 2 (events + control panel): 2 weeks
- PR 3 (raf + canvas_preview): 2 weeks
- PR 4 (canvas + webgl): 3 weeks
- PR 5 (RSocket health check + ws/codec + hypercolor-v2): 6 weeks (largest)
- PR 6 (axum + daemon security): 3 weeks

**Total: ~17 calendar weeks, roughly 4 months.** Matches the RFC 40 Year 1 timeline with slack for review cycles and unexpected friction.

## Acceptance criteria

This spec is considered ratified when:

- [ ] `Cargo.toml` for `hypercolor-leptos-ext` and `hypercolor-leptos-ext-macros` committed with the feature matrix and dependencies above.
- [ ] `cargo check --workspace` still passes with `hypercolor-leptos-ext` default features empty.
- [ ] `cd crates/hypercolor-ui && cargo check --target wasm32-unknown-unknown` passes with the browser feature set enabled.
- [ ] PR 1 (scaffolding) landed.
- [ ] `scripts/cinder-audit.sh` runs in CI on every PR.
- [ ] API shapes reviewed against RFCs 36 and 37 for rename-compatibility.
- [ ] Extraction plan to `cinder-stream` documented in the crate's README.

---

_"Prove the wedge in-tree. Extract when it hurts to stay."_
