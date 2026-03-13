//! Draggable vertical resize handle for resizable panel layouts.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Vertical resize handle for drag-to-resize between adjacent panels.
///
/// Reports the pixel delta from the drag start position on each mouse move.
/// The parent component uses this delta to compute new panel widths.
///
/// Document-level listeners are leaked via `forget()` (same pattern as
/// `ControlPanel`'s click-outside handler) — acceptable since each instance
/// only creates two small closures and the component is long-lived.
#[component]
pub fn ResizeHandle(
    #[prop(into)] on_drag_start: Callback<()>,
    #[prop(into)] on_drag: Callback<f64>,
    #[prop(into)] on_drag_end: Callback<()>,
) -> impl IntoView {
    let (dragging, set_dragging) = signal(false);
    let start_x = StoredValue::new(0.0_f64);

    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let move_handler =
            Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
                if !dragging.get_untracked() {
                    return;
                }
                ev.prevent_default();
                let delta = f64::from(ev.client_x()) - start_x.get_value();
                on_drag.run(delta);
            });

        let up_handler =
            Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |_ev: web_sys::MouseEvent| {
                if !dragging.get_untracked() {
                    return;
                }
                set_dragging.set(false);
                on_drag_end.run(());
            });

        let _ = doc.add_event_listener_with_callback(
            "mousemove",
            move_handler.as_ref().unchecked_ref(),
        );
        let _ = doc
            .add_event_listener_with_callback("mouseup", up_handler.as_ref().unchecked_ref());

        move_handler.forget();
        up_handler.forget();
    }

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
