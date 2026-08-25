# Signal Patterns in Hypercolor UI

Leptos 0.8 reactivity patterns specific to this codebase.

## StoredValue for Closure State

WebSocket manager uses `StoredValue<T>` to hold mutable state across callback invocations without triggering reactivity:

```rust
let last_frame_number: StoredValue<Option<u32>> = StoredValue::new(None);
let smoothed_fps: StoredValue<f64> = StoredValue::new(0.0);

// Inside WS message handler (closure) — read with get_value, write with set_value:
if let Some(prev_number) = last_frame_number.get_value() {
    // ... compute FPS from frame delta ...
    smoothed_fps.set_value(next);
}
last_frame_number.set_value(Some(current_frame_number));
```

**Key methods**: `get_value()` reads, `set_value(v)` writes, and
`update_value(|v| ...)` mutates in place. Reach for `update_value` when the new
value is derived from the old one and the type is expensive to clone, which is
why the WS layer uses it for the interactive-preview lifecycle tracker and the
output-power reconciler (`ws/connection.rs:302,307`).

**When to use**: Accumulator state inside event callbacks (FPS smoothing, reconnect counters, debounce timers). These don't need to trigger re-renders — they're internal bookkeeping.

## Untracked Access for Snapshots

`get_untracked()` reads a signal's value without creating a reactive dependency:

```rust
fn capture_active_effect_state(ctx: &EffectsContext) -> ActiveEffectSnapshot {
    ActiveEffectSnapshot {
        id: ctx.active_effect_id.get_untracked(),
        target: ctx.active_effect_target.get_untracked(),
        name: ctx.active_effect_name.get_untracked(),
        category: ctx.active_effect_category.get_untracked(),
        controls: ctx.active_controls.get_untracked(),
        control_values: ctx.active_control_values.get_untracked(),
        preset_id: ctx.active_preset_id.get_untracked(),
    }
}
```

`target` is an `Option<api::EffectLayerTarget>` carrying `effect_id`,
`zone_id`, and `layer_id`. Layer ids are real server ids, so a snapshot that
omits it restores the effect into the wrong zone.

**When to use**: Capturing state for undo/rollback, logging, or one-time reads inside `Effect::new()` where you don't want the effect to re-run when that particular signal changes.

## Resource + Memo Composition

The device list is a `LocalResource` refetched from WebSocket events and connection epochs, never a timer:

```rust
// Resource fetches data. Use api::daemon_resource, not LocalResource::new:
// it folds WsContext::connection_generation into the fetcher so the resource
// refetches after a reconnect gap, which plain events never cover.
let devices_resource = api::daemon_resource(api::fetch_devices);

// Memo derives indexed view
let devices_index = Memo::new(move |_| {
    devices_resource.get()
        .and_then(Result::ok)
        .unwrap_or_default()
});

// Effect watches for refetch triggers
Effect::new(move |_| {
    let Some(event) = ws_ctx.last_device_event.get() else { return; };
    let current_device_ids = devices_resource
        .get_untracked()
        .and_then(|result| result.ok())
        .map(|devices| devices.into_iter().map(|d| d.id).collect::<Vec<_>>())
        .unwrap_or_default();

    if should_refetch_devices_for_event(
        &event.event_type,
        event.device_id.as_deref(),
        event.found_count,
        &current_device_ids,
    ) {
        devices_resource.refetch();
    }
});
```

Pattern: Resource (async data) → Memo (derived view) → Effect (trigger refetch). Each layer has a single responsibility.

`should_refetch_devices_for_event` is a real, tested helper in
`src/device_event_logic.rs`. Call it rather than reimplementing the match: it
already covers `device_state_changed` and the `device_discovery_completed`
case where the current list is empty but the scan found devices.

## Signal-Based Props Convention

```rust
#[component]
pub fn EffectCard(
    effect: EffectSummary,
    #[prop(into)] is_active: Signal<bool>,
    #[prop(into)] is_favorite: Signal<bool>,
    /// LED zone names this effect is running in; renders a badge when non-empty.
    #[prop(optional, into)]
    active_zone_names: Signal<Vec<String>>,
    #[prop(into)] on_apply: Callback<String>,
    #[prop(into)] on_toggle_favorite: Callback<String>,
    /// Index for stagger animation (clamped to 12).
    #[prop(default = 0)]
    index: usize,
) -> impl IntoView
```

- `Signal<T>` for reactive inputs (fine-grained — only re-renders when value changes)
- `Callback<T>` for event handlers (Rc-wrapped, zero-copy)
- `#[prop(into)]` for ergonomic conversion at call sites
- Plain types (`EffectSummary`) for static data that doesn't change

## Click-Outside Handler Pattern

Used by color picker expansion in the control panel
(`components/control_panel/mod.rs:432`):

```rust
fn install_click_outside_handler(
    expanded_picker_id: ReadSignal<Option<String>>,
    set_expanded: WriteSignal<Option<String>>,
) {
    let Some(win) = window() else { return; };

    let _ = use_event_listener_with_options(
        win,
        ev::mousedown,
        move |ev: leptos::ev::MouseEvent| {
            if expanded_picker_id.get_untracked().is_none() {
                return;
            }
            let inside = ev.target().is_some_and(|target| {
                target_closest(Some(target.clone()), ".color-picker-popover")
                    || target_closest(Some(target), ".swatch-glow")
            });
            if !inside {
                set_expanded.set(None);
            }
        },
        UseEventListenerOptions::default().capture(true),
    );
}
```

Four details that are easy to get wrong:

- `use_event_listener_with_options` takes the **target first**, then the event,
  then the handler, then the options. A three-argument call does not compile.
- The event is `mousedown`, not `click`. Closing on mousedown fires before the
  browser moves focus, so the popover is already gone when the click lands.
- The handler parameter is `MouseEvent`, not `PointerEvent`.
- The hit test is against the real class names, `.color-picker-popover` and
  `.swatch-glow`. There is no `[data-picker]` attribute in the tree.

**Capture phase** (not bubble) ensures the handler runs before any child click handlers that might stop propagation. The early return on a closed picker keeps the listener nearly free while nothing is open.

## FPS Calculation with Smoothed Average

From WsManager — calculates FPS from frame metadata, not wall-clock timing:

```rust
// Using metadata timestamps avoids measuring WebSocket delivery jitter
// FPS is computed from frame_number delta / timestamp delta (not 1/elapsed)
let instant_fps = frame_delta as f64 * 1000.0 / elapsed_ms as f64;

// Exponential moving average (0.82/0.18 weighting)
let next = if previous <= 0.0 {
    instant_fps
} else {
    previous * 0.82 + instant_fps * 0.18
};
smoothed_fps.set_value(next);
```

**Reset on reconnect** — stale timestamps from previous connection would produce bogus FPS spikes.

## Derived Signals for Dynamic Styling

The dominant hue is computed once by the shared frame-analysis pass, which
publishes `FrameAnalysisContext::live_canvas` as a `CanvasFrameAnalysis
{ palette, dominant_hue }`. The shell consumes it with an `Effect`, not a
`Memo`, because the job is a side effect on the DOM rather than a derived value
(`components/shell.rs:55`):

```rust
let frame_analysis = use_context::<FrameAnalysisContext>();
let last_ambient_hue = StoredValue::new(None::<i16>);

if let Some(frame_analysis) = frame_analysis {
    Effect::new(move |_| {
        let Some(analysis) = frame_analysis.live_canvas.get() else { return; };
        let hue = analysis.dominant_hue.round() as i16;
        if last_ambient_hue.get_value() == Some(hue) { return; }

        if let Some(doc) = browser_document()
            && let Some(root) = doc.document_element()
            && let Some(html_el) = root.dyn_ref::<web_sys::HtmlElement>()
        {
            let _ = html_el.style().set_property("--ambient-hue", &hue.to_string());
            last_ambient_hue.set_value(Some(hue));
        }
    });
}
```

Two things are load-bearing. The property goes on the **document element**, not
the shell div: custom-property `var()` substitution resolves where a property is
declared, and every `--ambient-*` token is declared in a `:root` block of
`tokens/semantic.css`, so a hue set lower in the tree would never reach them.
And the rounded-hue guard in `StoredValue` skips the DOM write when the hue has
not moved a whole degree, which is most frames.
