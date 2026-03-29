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

**Key methods**: `get_value()` reads, `set_value(v)` writes. There is no `update_value` — use `get_value()` + `set_value()` instead.

**When to use**: Accumulator state inside event callbacks (FPS smoothing, reconnect counters, debounce timers). These don't need to trigger re-renders — they're internal bookkeeping.

## Untracked Access for Snapshots

`get_untracked()` reads a signal's value without creating a reactive dependency:

```rust
fn capture_active_effect_state(ctx: &EffectsContext) -> ActiveEffectSnapshot {
    ActiveEffectSnapshot {
        id: ctx.active_effect_id.get_untracked(),
        name: ctx.active_effect_name.get_untracked(),
        category: ctx.active_effect_category.get_untracked(),
        controls: ctx.active_controls.get_untracked(),
        control_values: ctx.active_control_values.get_untracked(),
        preset_id: ctx.active_preset_id.get_untracked(),
    }
}
```

**When to use**: Capturing state for undo/rollback, logging, or one-time reads inside `Effect::new()` where you don't want the effect to re-run when that particular signal changes.

## Resource + Memo Composition

Device list uses `LocalResource` with reactive refetch triggered by WebSocket events:

```rust
// Resource fetches data — pass the async fn directly, no closure wrapper needed
let devices_resource = LocalResource::new(api::fetch_devices);

// Memo derives indexed view
let devices_index = Memo::new(move |_| {
    devices_resource.get()
        .and_then(Result::ok)
        .unwrap_or_default()
});

// Effect watches for refetch triggers
Effect::new(move |_| {
    if should_refetch_based_on_ws_event() {
        devices_resource.refetch();
    }
});
```

Pattern: Resource (async data) → Memo (derived view) → Effect (trigger refetch). Each layer has a single responsibility.

## Signal-Based Props Convention

```rust
#[component]
pub fn EffectCard(
    effect: EffectSummary,
    #[prop(into)] is_active: Signal<bool>,
    #[prop(into)] is_favorite: Signal<bool>,
    #[prop(into)] on_apply: Callback<String>,
) -> impl IntoView
```

- `Signal<T>` for reactive inputs (fine-grained — only re-renders when value changes)
- `Callback<T>` for event handlers (Rc-wrapped, zero-copy)
- `#[prop(into)]` for ergonomic conversion at call sites
- Plain types (`EffectSummary`) for static data that doesn't change

## Click-Outside Handler Pattern

Used by color picker expansion in control panel:

```rust
fn install_click_outside_handler(set_expanded_picker_id: WriteSignal<Option<String>>) {
    let options = UseEventListenerOptions::default().capture(true);
    use_event_listener_with_options(
        ev::click,
        move |ev: ev::PointerEvent| {
            let target = ev.target().and_then(|t| t.dyn_into::<HtmlElement>().ok());
            if let Some(el) = target {
                if el.closest("[data-picker]").ok().flatten().is_none() {
                    set_expanded_picker_id.set(None);
                }
            }
        },
        options,
    );
}
```

**Capture phase** (not bubble) ensures the handler runs before any child click handlers that might stop propagation.

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

Shell component extracts dominant hue from canvas frames for ambient UI glow:

```rust
let ambient_hue = Memo::new(move |_| {
    let frame = ws_ctx.canvas_frame.get()?;
    // Sample every Nth pixel, skip low-chroma (< threshold)
    // Circular mean using sin/cos for hue wraparound (0° and 360° are adjacent)
    let (sin_sum, cos_sum) = samples.iter().fold((0.0, 0.0), |(s, c), hue| {
        (s + hue.to_radians().sin(), c + hue.to_radians().cos())
    });
    let mean_hue = sin_sum.atan2(cos_sum).to_degrees();
    Some(mean_hue)
});
```

This drives `--ambient-hue` CSS custom property for reactive glow on sidebar/header.
