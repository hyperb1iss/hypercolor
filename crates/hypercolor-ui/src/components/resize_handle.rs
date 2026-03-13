//! Draggable vertical resize handle for resizable panel layouts.

use leptos::ev;
use leptos::prelude::*;

/// Vertical resize handle for drag-to-resize between adjacent panels.
///
/// Reports the pixel delta from the drag start position on each mouse move.
/// The parent component uses this delta to compute new panel widths.
#[component]
pub fn ResizeHandle(
    #[prop(into)] on_drag_start: Callback<()>,
    #[prop(into)] on_drag: Callback<f64>,
    #[prop(into)] on_drag_end: Callback<()>,
) -> impl IntoView {
    let (dragging, set_dragging) = signal(false);
    let start_x = StoredValue::new(0.0_f64);

    let _drag_move = window_event_listener(ev::mousemove, move |ev| {
        if !dragging.try_get_untracked().unwrap_or(false) {
            return;
        }
        let Some(start) = start_x.try_get_value() else {
            return;
        };
        ev.prevent_default();
        let delta = f64::from(ev.client_x()) - start;
        on_drag.run(delta);
    });

    let _drag_end = window_event_listener(ev::mouseup, move |_| {
        if !dragging.try_get_untracked().unwrap_or(false) {
            return;
        }
        set_dragging.set(false);
        on_drag_end.run(());
    });

    view! {
        <div
            class="resize-handle-zone"
            class:resize-handle-active=move || dragging.get()
            on:mousedown=move |ev: web_sys::MouseEvent| {
                ev.prevent_default();
                start_x.set_value(f64::from(ev.client_x()));
                set_dragging.set(true);
                on_drag_start.run(());
            }
        >
            <div class="resize-handle-line" />
        </div>
    }
}
