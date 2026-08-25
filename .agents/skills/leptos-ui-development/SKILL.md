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

## Verification

`cargo check -p hypercolor-ui --lib` is the check that covers the app.
Everything lives in the lib target: `src/lib.rs` declares 46 `pub mod`s, and
`src/main.rs` is a three-line shim that calls `hypercolor_ui::run()`. The bin
is the hollow half.

Do not run cargo in this crate while a Trunk dev server is serving. The two
fight over the target directory, and the usual symptom is a burst of phantom
"missing field" or "no such variant" errors from stale rmeta. Stop Trunk, or
wait for it.

That lib-first layout exists so integration tests under `tests/` reach every
module as `hypercolor_ui::...`. No test file uses a `#[path]` source include,
so write new tests against `hypercolor_ui::...` rather than reaching into
`../src/`. Leptos-free logic modules are exercised that way without a WASM
toolchain. Two `#[path]` attributes do remain in `src/layout_geometry.rs`, but
those are ordinary submodule layout, not test plumbing, and several module doc
comments still describe the older `#[path]`-include pattern that the lib-first
move retired. Trust `lib.rs` and `tests/`, not those comments. Visual verification goes through `agent-browser` against
`docs/DESIGN-SYSTEM.md`.

## Global Context Architecture

Contexts are provided at the app root (`src/app.rs`) and by `crate::zones`,
and read with `expect_context::<T>()`. The ones you will actually reach for:

| Context                 | Provides                                | Notable members                                                                                                                                            |
| ----------------------- | --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `WsContext`             | Everything streamed off the daemon socket | `canvas_frame`, `connection_generation`, the `last_*_event` hint family, `metrics`, `sensors`, `layer_health`, `output_paused`, `set_preview_consumers` |
| `ZonesContext`          | The active scene's zones                 | `active_scene`, `zones`, `led_zones`, `multi_zone`, `focused_zone`, `refresh`                                                                              |
| `ScenesContext`         | Saved scenes and activation              | `scenes`, `active`, `switching`, `activate`, `deactivate`, `refresh`                                                                                        |
| `EffectsContext`        | Effect library plus active-effect state  | `effects_index` (Memo, no setter), `zone_effects`, `focused_zone_effect`, `apply_target`, `scene_refresh`, and read/write pairs for the `active_*` signals |
| `CapabilitiesContext`   | Daemon-advertised capability names       | `has(name)`, `zone_crud_ready()`                                                                                                                            |
| `DevicesContext`        | Device and layout resources              | `devices_resource`, `layouts_resource`                                                                                                                      |
| `DisplaysContext`       | Display resources                        | `displays_resource`                                                                                                                                         |
| `ConfigContext`         | Daemon config mirror                     | `config`, `refresh`, `audio_enabled` (Memo)                                                                                                                 |
| `ConfigSchemaContext`   | The config key registry                  | `entries`                                                                                                                                                   |
| `FrameAnalysisContext`  | Shared canvas analysis pass              | `live_canvas` (dominant hue, palette)                                                                                                                       |
| `StudioContext`         | Studio page state                        | `selected_surface_id`, `composition_open`, `selected_output_ids`, `hidden_outputs`                                                                           |
| `PreviewTelemetryContext` | Preview delivery telemetry            | `presenter`, `set_presenter`                                                                                                                                |

Sub-tree contexts are provided lower down and are only visible inside their
own subtree: `LayoutEditorContext`, `LayoutZoneDisplayContext`, and
`ZoneCanvasActions` in the layout builder, plus the extension slots
(`NavExtensionItems`, `SettingsExtensionSections`, `SidebarExtensionWidgets`,
`UiChromeFlags`), which are empty in the standalone OSS app.

There is no `ThemeContext` and no `PaletteContext`. Theme has no runtime
toggle at all (see Design System below), and the command palette is local
state inside `components/shell.rs`.

## Capability Gating

Multi-zone Studio affordances gate on capability names the daemon advertises
through its authenticated `/api/v1/system` status. **There is no probe
fallback**: an absent advertisement means the affordance stays hidden, with no
error and no explanation anywhere in the UI. If a control you built never
appears, check the capability before you check your rendering.

```rust
let caps = expect_context::<CapabilitiesContext>();
if caps.zone_crud_ready() { /* + New zone, zone rows */ }
```

`zone_crud_ready()` is the conjunction of `zone-crud`, `multi-zone-sampling`,
and `zone-device-assignment`, because a user who can create a zone but cannot
render it or move outputs into it has an unusable zone. The fourth name in use
is `scene-unassigned-behavior-write`.

## Scene, Zone, and Layer Resource Model

Post-#216 this is most of what the UI does. One live scene tree hangs off
`/api/v1/scene`, and everything the user composes lives inside it.

```
SceneDocument
  └─ zones (LED zones and display Screens, in scene order)
       └─ layers (ordered stack; the top layer is the tile caption)
            └─ controls (patched per layer)
```

`crate::zones` owns the shared resources and publishes `ZonesContext` and
`ScenesContext`. Never fetch `/api/v1/scene` yourself from a component: read
`active_scene` off the context and call `refresh` after a mutation.

The UI's presentation type for a zone is `zones::Surface` (`src/zones/surface.rs`):
`id`, `name`, `kind` (`SurfaceKind::Light` for LED zones, `Screen` for display
faces), `enabled`, `role` (`Primary` is the permanent Default zone, `Custom` is
deletable), `color`, `display_device_id`, `layer_ids`, `top_layer`.
`ZoneEffectState` (`src/zones.rs:141`) pairs a `Surface` with what it is showing:
`effect_id`, `effect_name`, `effect_category`, `control_values`, `preset_id`,
and the scene `revision` those values were read at.

`EffectsContext::zone_effects` is the multi-zone now-playing source of truth,
in scene order. The singular `active_*` signals mirror the primary zone only,
so anything zone-aware reads `zone_effects` or `focused_zone_effect`.

The API modules (`src/api/`, sixteen alongside `mod.rs`) map onto the tree:

| Module      | Surface                                                                    |
| ----------- | -------------------------------------------------------------------------- |
| `scenes.rs` | `/api/v1/scene`, `/api/v1/scene/deactivate`, `/api/v1/scenes` CRUD + activate |
| `zones.rs`  | `/api/v1/scene/zones` create, update, delete, `/layout`, `/members`         |
| `layers.rs` | `/api/v1/scene/zones/{zone}/layers` CRUD, `/order`, `/controls`             |
| `effects.rs`| Effect catalog and `EffectLayerTarget { effect_id, zone_id, layer_id }`     |
| `controls.rs`, `output.rs`, `assets.rs`, `displays.rs`, `drivers.rs`, `device_metrics.rs`, `devices.rs`, `layouts.rs`, `library.rs`, `config.rs`, `system.rs` | the rest, all over `client.rs` |

Mutations carry `expected_revision` for optimistic concurrency. Control writes
go through `api::patch_layer_controls(zone_id, layer_id, &controls)`, which
sends `PatchControlsRequest { values, clear_bindings }`. Control values
serialize as the canonical adjacently-tagged form:

```json
{ "values": { "speed": { "kind": "float", "value": 0.5 } }, "clear_bindings": [] }
```

The deserializer denies unknown fields, so a bare `{"speed": 0.5}` or a
`{"speed": {"float": 0.5}}` shorthand is a hard parse failure on this route.
`patch_layer_controls` handles one retry itself: when the daemon rejects a
write because the control is bound, it resends the same patch with those keys
in `clear_bindings`, so removing the binding and applying the value land in one
atomic commit.

Studio lives in `src/pages/studio/` (10 modules: `mod`, `stage`, `zone_tree`,
`zone_controls`, `zone_add_device`, `device_card`, `device_assignment`,
`composition_panel`, `face_composition`, `scene_selector`).

## Never Poll: Events, Hints, and Epochs

A timer-driven fetch loop anywhere in the UI is a fatal defect. Freshness comes
from two mechanisms, and non-trivial resources need both.

**1. Event hints.** The daemon relays every `HypercolorBus` event to `events`
subscribers, and `WsContext` exposes the latest of each kind as a signal:
`last_device_event`, `last_scene_event`, `last_effect_error`,
`last_control_surface_event`, `last_extension_event`,
`last_input_source_status_event`, `last_service_identity_event`. Read one
inside a fetcher or an `Effect` to make a resource live.

**2. Connection epochs.** Events are not replayed across a socket gap, so a
resource that only watches hints goes stale after a reconnect.
`WsContext::connection_generation` increments on every socket open. Fold it
into the fetcher and the resource refetches after every reconnect.

`api::daemon_resource(fetcher)` (`src/api/mod.rs:31`) does the epoch folding
for you, and is the default way to create a resource:

```rust
let status_resource = api::daemon_resource(api::fetch_status);

// Reactive inputs are read in the sync part of the closure; the async block
// owns them. Reading a signal inside the future would not register it.
let layers_resource = api::daemon_resource(move || {
    let zone_id = selected_surface_id.get();
    async move {
        match zone_id {
            Some(zone_id) => api::list_layers(&zone_id).await,
            None => Ok(empty_layer_stack()),
        }
    }
});
```

Layer the hint on top when the data also changes mid-connection. The scene
resources in `src/zones.rs` are the reference implementation, and they show the
discipline that matters: refetch only when state *actually* changed.

```rust
Effect::new(move |previous: Option<Option<SceneEventHint>>| {
    let current = last_scene_event.get();
    if previous.as_ref() == Some(&current) { return current; }
    let Some(hint) = current.as_ref() else { return current; };

    // Control patches arrive at slider-drag rate and change no structure.
    let controls_only = hint.event_type == "zone_changed"
        && hint.zone_change_kind == Some(ZoneChangeKind::ControlsPatched);
    if !controls_only { active_scene_resource.refetch(); }
    if matches!(hint.event_type.as_str(),
        "active_scene_changed" | "scene_library_changed") {
        scenes_resource.refetch();
    }
    current
});
```

For devices, `should_refetch_devices_for_event(event_type, device_id, found_count, current_device_ids)`
in `src/device_event_logic.rs` is the tested predicate. Call it rather than
rewriting the match: it already handles `device_state_changed` and the
`device_discovery_completed` case where the list is empty but the scan found
something.

One-shot handshakes with server-driven pacing (device-authorization login) are
protocol, not polling.

## WebSocket Layer

`src/ws/` is a directory: `connection.rs` (the socket and `WsManager`, at
`connection.rs:120`), `messages.rs` (JSON and binary dispatch), `preview.rs`
(preview subscriptions and FPS caps), `input.rs`, `interactive_preview.rs`.

- **Subprotocol**: always open the socket with `HYPERCOLOR_WS_PROTOCOL`
  (`"hypercolor-v1"`). The daemon echoes it back through
  `ws.protocols([HYPERCOLOR_WS_PROTOCOL])`, and it is what the manifest
  advertises, but it is not an upgrade gate: the only rejection in
  `ws_handler` is the Origin check. A verified session token rides as a
  `?token=` query parameter.
- **Binary type**: `ArrayBuffer`, required for preview frames.
- **Initial subscribe**: three topics, one with a config.
  `{"type":"subscribe","topics":[{"topic":"events"},{"topic":"metrics","config":{"fps":2.0}},{"topic":"sensors"}]}`
  Preview topics are subscribed on demand, never at connect.
- **Binary frames**: preview transport v2 has several envelopes. Never parse
  them by hand; decode through `PreviewBinaryDecoder`. See
  `references/websocket-protocol.md`.
- **Reconnection**: exponential backoff 500ms to 15s.

### CanvasFrame Structure

`CanvasFrame` is an alias for `hypercolor_leptos_ext::ws::PreviewFrameView`
(`src/ws/messages.rs:19-21`), not a UI-local struct:

```rust
pub struct PreviewFrameView {
    pub channel: PreviewFrameChannel,
    pub frame_number: u32,
    pub timestamp_ms: u32,
    pub width: u32,
    pub height: u32,
    pub format: PreviewPixelFormat,  // Rgb | Rgba | Jpeg
    pub payload: js_sys::Uint8Array, // Direct WASM heap view
}
```

**`Jpeg` is load-bearing.** Display previews arrive JPEG-encoded, and
`rgba_at(pixel_index)` returns `None` for that format because there are no raw
pixels to read. Any consumer doing pixel math has to handle it. For `Rgb` and
`Rgba`, `rgba_at` samples zero-copy off the heap view; the shared frame-analysis
pass uses it for dominant hue, and the preview runtimes upload the payload as a
texture.

## Critical Pattern: Optimistic Update with Rollback

Effect switching and favorites use capture/restore for error recovery:

```rust
let previous = capture_active_effect_state(&ctx); // snapshot
// ... apply effect via API ...
if api_call.is_err() {
    restore_active_effect_state(&ctx, previous); // rollback
}
```

The snapshot carries `target: Option<EffectLayerTarget>` alongside id, name,
category, controls, control values, and preset id. Since #216 layer ids are
real server ids, so a snapshot that drops `target` restores the effect into the
wrong zone. Keep the field.

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

`#[serde(default)]` does NOT handle unknown enum variants: when a producer adds
a variant the UI doesn't know about, deserialization fails outright. Use
`#[serde(other)]` on a fallback variant:

```rust
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MacosOwnerRemedy {
    StopStandaloneOwner { pid: u32 },
    StartAppSidecar,
    #[serde(other)]
    Unknown,
}
```

The real uses are in `src/tauri_bridge.rs`, where the producer is the native
app shell rather than the daemon. **This does not apply to daemon types.**
Effect categories, control values, scene documents, and every other shared
contract come from `hypercolor-types`, compiled into both sides, so the UI
cannot skew from the daemon's definition. Never hand-mirror a daemon enum in
the UI just to add a fallback variant.

## Design System (Luminary)

**`docs/DESIGN-SYSTEM.md` is the canonical style guide.** Any UI design work (tokens, color, typography, surfaces, motion, glass, ambient reactivity, components) follows it. Consult it before styling a surface; its §14 ("Working in Luminary") is the rules checklist.

The design language is **Luminary**. SilkCircuit is the accent palette inside
it, not a synonym for it. The rules that get broken most often (§4):

- **Electric purple is the only accent in UI chrome.** Buttons, toggles, active
  states, slider thumbs, focus partners: all purple. Reaching for cyan or coral
  to decorate a button is a category or status signal leaking into chrome.
- **Category color is identity only.** `category_style()` in `src/style_utils.rs`
  is the single source of truth for the category-to-accent map. Add a category
  there and nowhere else.
- **Colored glows go through `--glow-rgb`**, set by an `.accent-*` class or
  inline from a `category_style()` triplet. Never spray raw `rgba()`.
- **Semantic tokens only.** No `oklch(...)` or hex in component CSS or Leptos
  `style` attributes.
- **Both themes, every time.** Verify `[data-theme="light"]` before a surface is
  done.

Token files in `tokens/` (DESIGN-SYSTEM.md §3 has the full architecture):

- **`primitives.css`**: Tier 1, raw OKLCH values via `@theme` (void scale for dark surfaces, cloud scale for light, SilkCircuit palette). Auto-generates Tailwind utilities.
- **`semantic.css`**: Tier 2, intent-mapped tokens (`--surface-base`, `--text-primary`, `--border-focus`) that swap between `:root` (dark) and `[data-theme="light"]`.

**Dynamic ambient glow**: `components/shell.rs` watches
`FrameAnalysisContext::live_canvas` and writes `--ambient-hue` onto the
**document element**, not the shell div. Custom-property `var()` substitution
resolves where the property is *declared*, and every `--ambient-*` token is
declared in a `:root` block of `semantic.css`, so a hue set lower in the tree
would never reach them.

**Theme**: stored in localStorage as `hc-theme` and applied by an inline
`index.html` script _before first paint_ to prevent flash. There is no runtime
toggle in Rust.

## API Client Pattern

`src/api/client.rs` owns transport. It unwraps the canonical `ApiResponse<T>`
envelope exactly once, in `parse_envelope` (`client.rs:401`), and exposes typed
helpers that domain modules call. There is no `ApiEnvelope` type, and domain
modules do not build `gloo_net` requests by hand:

```rust
let list: EffectListResponse = client::fetch_json("/api/v1/effects").await?;
let effects: Vec<EffectSummary> = list.items;
let layouts: Vec<LayoutSummary> = client::fetch_all_pages("/api/v1/layouts").await?;
client::patch_json_discard(&url, &request).await?;
```

`fetch_json` deserializes whatever the envelope's `data` actually is, so a list
route needs the `ListResponse<T>` alias, not a bare `Vec<T>`. `api/effects.rs`
fetches `EffectListResponse` (`= ListResponse<EffectSummary>`) and then takes
`.items`; `api/assets.rs` fetches `AssetListResponse`
(`= ListResponse<MediaAssetRecord>`) and returns the whole envelope to its
caller. `fetch_all_pages` is the other half of the
pair: it walks the `page` block and hands back a flat `Vec<T>`, which is how
`api/layouts.rs` and `api/devices.rs` read their collections.

The helper set is `fetch_json`, `fetch_json_optional`, `fetch_all_pages`,
`post_json`, `patch_json`, `put_json`, `delete_json`, the `_discard` and
`_empty` variants, and `send_json_versioned` for revision-checked mutations.
Errors come back as `ApiError`; `ApiResult<T>` is the alias.

Async operations run via `leptos::task::spawn_local()` — no threading in WASM.

## Canvas Preview

`src/components/canvas_preview.rs` drives a `PreviewRuntime`
(`src/components/preview_runtime/`) with three backends and automatic
degradation, not WebGL alone:

- **Worker**: an OffscreenCanvas bitmap worker, used for JPEG frames and as the
  first fallback when a WebGL context is unavailable
- **WebGl**: the default for `Rgb`/`Rgba` frames. Texture reused across frames,
  reinit only if dimensions change, no pixel buffer copy
- **Canvas2d**: last resort, gated behind `CANVAS2D_FALLBACK_THRESHOLD = 3`
  consecutive WebGL-unavailable attempts, so a transient context loss does not
  permanently demote the surface

The component listens for `webglcontextlost` and `webglcontextrestored` and
reinitializes the runtime on restore.

Streaming is demand-driven: components register and unregister through
`set_preview_consumers` (WsContext), and the canvas subscription is active only
while the consumer count is above zero. Parallel counters exist for the other
streams (`set_screen_preview_consumers`, `set_screen_zones_consumers`,
`set_web_viewport_preview_consumers`, `set_device_metrics_consumers`).

FPS caps (`src/ws/preview.rs:11-14`):

| Constant                        | Value | Applies to                         |
| ------------------------------- | ----- | ---------------------------------- |
| `DEFAULT_PREVIEW_FPS_CAP`       | 60    | The main canvas preview            |
| `HIDDEN_TAB_PREVIEW_FPS_CAP`    | 6     | Any preview while the tab is hidden |
| `SCREEN_PREVIEW_FPS_CAP`        | 15    | The screen-source preview          |
| `WEB_VIEWPORT_PREVIEW_FPS_CAP`  | 15    | The web-viewport preview           |

These are product ceilings. Do not lower one to make a metric look better; fix
the cost instead. `set_preview_cap` (WsContext) lets a page lower its own
client-side ceiling, and the actual target is min(engine target, client cap,
transport cap).

Backpressure-driven auto-reduction lives in `src/ws/messages.rs:1009-1027`, not
in the preview component. Note that the daemon only runs backpressure reporters
for `frames`, `spectrum`, `metrics`, and `device_metrics`, so the UI's
`topic == "canvas"` reduction branch does not fire in practice.

Canvas preview does **not** use `Portal`. Portal is used by `modal.rs`,
`silk_select.rs`, `component_picker.rs`, `viewport_designer.rs`, and the
`control_panel/{color,enum_select,sensor}.rs` popovers, which have to escape
overflow-hidden cards.

## Visibility-Aware FPS

Tab hidden → reduce preview FPS to 6 via `document.visibilitychange` listener. Resets smoothed FPS counters on reconnect to avoid glitch display.

## leptos_icons Gotcha

`Icon`'s `style` prop is `MaybeProp<String>` — accepts `&str` or `String`, **not closures**. Use conditional rendering (`if`/`Show`) to vary icon styles reactively, not a closure-based style prop.

## Key File Locations

| Purpose                | Path                                                             |
| ---------------------- | ---------------------------------------------------------------- |
| App root + contexts    | `src/app.rs`, `src/app/`                                         |
| Scene/zone/scene state | `src/zones.rs`, `src/zones/surface.rs`                           |
| Studio page            | `src/pages/studio/`                                              |
| WebSocket layer        | `src/ws/{connection,messages,preview,input,interactive_preview}.rs` |
| API transport          | `src/api/client.rs`, `src/api/mod.rs` (`daemon_resource`)        |
| API modules            | `src/api/` (sixteen domain modules)                              |
| Canvas preview         | `src/components/canvas_preview.rs`, `src/components/preview_runtime/` |
| Effect controls        | `src/components/control_panel/`                                  |
| Layer stack UI         | `src/components/layer_panel/`                                    |
| Layout builder         | `src/components/layout_builder.rs`, `src/components/layout_builder/` |
| Device refetch logic   | `src/device_event_logic.rs`                                      |
| Style utilities        | `src/style_utils.rs` (`category_style`)                          |
| Design tokens          | `tokens/{primitives,semantic}.css`                               |
| Design system guide    | `docs/DESIGN-SYSTEM.md` (canonical Luminary style guide)         |
| Trunk config           | `Trunk.toml`                                                     |
| Tests (unit only)      | `tests/`                                                         |

## Detailed References

- **`references/signal-patterns.md`** — Leptos 0.8 reactivity patterns specific to this codebase: StoredValue for closures, untracked access for snapshots, Resource + Memo composition
- **`references/websocket-protocol.md`** — Full binary frame parsing, channel subscription, reconnection state machine, backpressure handling
