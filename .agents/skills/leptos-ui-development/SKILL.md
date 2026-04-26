---
name: leptos-ui-development
version: 1.0.0
description: >-
  This skill should be used when working on the Hypercolor web UI in
  crates/hypercolor-ui/. Triggers on "UI component", "Leptos signal", "WASM
  build", "Trunk build", "WebSocket frame", "canvas preview", "effect card",
  "control panel", "device page", "SilkCircuit token", "theme switching",
  "sidebar", "layout builder", "Leptos context", "web-sys binding", "UI
  state", "optimistic update", "WebGL texture", "toast notification",
  "command palette", "color wheel", "device pairing", "leptoaster", or any
  work in crates/hypercolor-ui/.
---

# Hypercolor UI Development

The UI is a **Leptos 0.8 CSR** app compiled to WASM via **Trunk**, excluded from the Cargo workspace. `cargo check --workspace` does NOT cover it — always build/check separately.

## Build Pipeline

```bash
just ui-dev          # Dev server on :9430, proxies API to daemon on :9420
cd crates/hypercolor-ui && trunk build   # One-shot build
```

Trunk pre-build hook runs Tailwind CSS compilation. Config in `Trunk.toml` — proxies `/api` to `127.0.0.1:9420`.

## Global Context Architecture

Five context structs provided at app root, accessed via `expect_context::<T>()`:

| Context          | Provides                      | Key Signals                                                                                                                                                                                                                                      |
| ---------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `WsContext`      | WebSocket state               | `canvas_frame`, `connection_state`, `preview_fps`, `preview_target_fps`, `set_preview_cap`, `set_preview_consumers`, `metrics`, `backpressure_notice`, `active_effect`, `last_device_event`, `audio_level`, `audio_enabled`, `set_audio_enabled` |
| `EffectsContext` | Effect library + active state | `effects_index` (Memo), `active_effect_id`, `active_effect_name`, `active_effect_category`, `active_controls`, `active_control_values`, `active_preset_id`, `favorite_ids` (each with read/write pair)                                           |
| `DevicesContext` | Device + layout resources     | `devices_resource`, `layouts_resource` (both LocalResource with refetch)                                                                                                                                                                         |
| `ThemeContext`   | Theme state                   | `is_dark` (Memo<bool>), `toggle` (Callback<()>)                                                                                                                                                                                                  |
| `PaletteContext` | Command palette trigger       | `open` (Callback<()>)                                                                                                                                                                                                                            |

## WebSocket Binary Protocol

WsManager (`src/ws.rs`) handles the daemon connection:

- **Binary frames**: Header byte `0x03` = canvas frame data
- **JSON messages**: Events, metrics, audio state
- **Subscribe on connect**: `{ "type": "subscribe", "channels": ["events", "metrics"] }`
- **Reconnection**: Exponential backoff 500ms → 15s

### CanvasFrame Structure

```rust
pub struct CanvasFrame {
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    format: CanvasPixelFormat,  // Rgb | Rgba
    pixels: js_sys::Uint8Array, // Direct WASM heap view
}
```

`rgba_at(pixel_index)` samples zero-copy. Used by shell for dominant hue extraction (circular mean with sin/cos for hue wraparound), sidebar for vibrant palette, and canvas_preview for WebGL texture.

## Critical Pattern: Event-Based Refetch

**Never use timer-based polling for device lists** — it causes flickering. Instead, watch WebSocket device events:

```rust
Effect::new(move |_| {
    let Some(event) = ws_ctx.last_device_event.get() else { return; };
    let should_refetch = match event.event_type.as_str() {
        "device_connected" | "device_discovered" => !already_known,
        "device_disconnected" => is_known,
        _ => false,
    };
    if should_refetch { devices_resource.refetch(); }
});
```

Only refetch when state _actually changed_ — not on every event.

## Critical Pattern: Optimistic Update with Rollback

Effect switching and favorites use capture/restore for error recovery:

```rust
let previous = capture_active_effect_state(&ctx); // snapshot
// ... apply effect via API ...
if api_call.is_err() {
    restore_active_effect_state(&ctx, previous); // rollback
}
```

## Critical Pattern: Control Panel Memo

The control panel groups controls for rendering. **Memoize structure, not values:**

```rust
let grouped = Memo::new(move |_| {
    let defs = control_definitions.get(); // reads definitions
    // Group by definition structure — NOT by control_values
    // Reading values here would teardown/rebuild entire widget tree on every slider move
});
```

## Serde Gotcha

`#[serde(default)]` does NOT handle unknown enum variants. When the daemon adds a new variant the UI doesn't know about, deserialization fails. Use `#[serde(other)]` on a fallback variant:

```rust
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EffectCategory {
    Ambient,
    Audio,
    Generative,
    #[serde(other)]
    Unknown, // catches future additions without breaking
}
```

## SilkCircuit Token System

Two-tier CSS custom properties in `tokens/`:

- **`primitives.css`**: Raw OKLCH values via `@theme` — void scale (dark surfaces), cloud scale (light), SilkCircuit palette colors
- **`semantic.css`**: Intent-mapped tokens (`--surface-base`, `--text-primary`, `--border-focus`) that swap between `:root` (dark) and `[data-theme="light"]`

**Dynamic ambient glow**: Components set `--ambient-hue` from live canvas frame pixel data. CSS uses it for reactive glow effects.

**Theme switching**: Stored in localStorage as `hc-theme`. Restored _before first paint_ (in `index.html` script) to prevent flash.

## API Client Pattern

All API calls use `gloo_net::http::Request` with a standard envelope:

```rust
let ApiEnvelope { data } = resp.json::<ApiEnvelope<T>>().await?;
```

Async operations run via `leptos::task::spawn_local()` — no threading in WASM.

## Canvas Preview (WebGL)

`src/components/canvas_preview.rs` uploads canvas frames as WebGL textures:

- Texture reused across frames; reinit only if dimensions change
- No pixel buffer copy — uses `Uint8Array` view of WASM heap
- Demand-driven streaming: components register/unregister via `set_preview_consumers` (WsContext). Canvas subscription is active only when consumer count > 0
- Default FPS cap is 30 (`DEFAULT_PREVIEW_FPS_CAP`). Hidden tab auto-reduces to 6 (`HIDDEN_TAB_PREVIEW_FPS_CAP`) via `document.visibilitychange`
- `set_preview_cap` (WsContext) lets pages override the client-side FPS ceiling; actual target = min(engine target, client cap, transport cap)
- Listens to `backpressure_notice` to auto-reduce transport FPS cap
- Does NOT use `Portal` — Portal is used by `control_panel.rs` (color picker popovers) and `component_picker.rs`, not canvas preview

## Visibility-Aware FPS

Tab hidden → reduce preview FPS to 6 via `document.visibilitychange` listener. Resets smoothed FPS counters on reconnect to avoid glitch display.

## leptos_icons Gotcha

`Icon`'s `style` prop is `MaybeProp<String>` — accepts `&str` or `String`, **not closures**. Use conditional rendering (`if`/`Show`) to vary icon styles reactively, not a closure-based style prop.

## Key File Locations

| Purpose             | Path                                                         |
| ------------------- | ------------------------------------------------------------ |
| App root + contexts | `src/app.rs`                                                 |
| WebSocket manager   | `src/ws.rs`                                                  |
| API modules         | `src/api/{effects,devices,layouts,library,config,system}.rs` |
| Canvas preview      | `src/components/canvas_preview.rs`                           |
| Effect controls     | `src/components/control_panel.rs`                            |
| Layout builder      | `src/components/layout_builder.rs`                           |
| Style utilities     | `src/style_utils.rs`                                         |
| Design tokens       | `tokens/{primitives,semantic}.css`                           |
| Trunk config        | `Trunk.toml`                                                 |
| Tests (unit only)   | `tests/`                                                     |

## Detailed References

- **`references/signal-patterns.md`** — Leptos 0.8 reactivity patterns specific to this codebase: StoredValue for closures, untracked access for snapshots, Resource + Memo composition
- **`references/websocket-protocol.md`** — Full binary frame parsing, channel subscription, reconnection state machine, backpressure handling
