//! Draggable vertical resize handle for resizable panel layouts.

use leptos::prelude::*;

use crate::pointer_gesture::{
    GestureEnd, PointerEnd, PointerGesture, Press, capture_pointer, holds_capture,
};

/// Vertical resize handle for drag-to-resize between adjacent panels.
///
/// Reports the pixel delta from the press position on each pointer move.
/// The parent component uses this delta to compute new panel widths. The
/// press captures its pointer on the handle, so the drag keeps tracking
/// anywhere on screen; a cancelled drag reports a zero delta (back to the
/// starting width) before ending.
#[component]
pub fn ResizeHandle(
    #[prop(into)] on_drag_start: Callback<()>,
    #[prop(into)] on_drag: Callback<f64>,
    #[prop(into)] on_drag_end: Callback<()>,
) -> impl IntoView {
    let handle_ref = NodeRef::<leptos::html::Div>::new();
    let (dragging, set_dragging) = signal(false);
    // The gesture state is the press position's client x.
    let gesture = StoredValue::new(PointerGesture::<f64>::new());

    let finish = move |outcome: GestureEnd| {
        set_dragging.set(false);
        if outcome == GestureEnd::Cancel {
            on_drag.run(0.0);
        }
        on_drag_end.run(());
    };

    let on_pointer_end = move |ev: web_sys::PointerEvent, how: PointerEnd| {
        let ended = gesture
            .try_update_value(|g| g.end(ev.pointer_id(), how))
            .flatten();
        if let Some((outcome, _)) = ended {
            finish(outcome);
        }
    };

    view! {
        <div
            node_ref=handle_ref
            class="resize-handle-zone touch-grab"
            class:resize-handle-active=move || dragging.get()
            on:pointerdown=move |ev: web_sys::PointerEvent| {
                ev.prevent_default();
                let Some(handle) = handle_ref.get_untracked() else {
                    return;
                };
                let handle: web_sys::Element = handle.into();
                let pointer_id = ev.pointer_id();
                let press = gesture
                    .try_update_value(|g| {
                        g.press(pointer_id, |owner, _| holds_capture(Some(&handle), owner))
                    })
                    .unwrap_or(Press::Busy);
                match press {
                    Press::Busy => return,
                    Press::Stale(_) => finish(GestureEnd::Cancel),
                    Press::Ready => {}
                }
                capture_pointer(&handle, &ev);
                gesture.update_value(|g| g.start(pointer_id, f64::from(ev.client_x())));
                set_dragging.set(true);
                on_drag_start.run(());
            }
            on:pointermove=move |ev: web_sys::PointerEvent| {
                let Some(start_x) = gesture.with_value(|g| g.state(ev.pointer_id()).copied())
                else {
                    return;
                };
                ev.prevent_default();
                on_drag.run(f64::from(ev.client_x()) - start_x);
            }
            on:pointerup=move |ev| on_pointer_end(ev, PointerEnd::Up)
            on:pointercancel=move |ev| on_pointer_end(ev, PointerEnd::Cancel)
            on:lostpointercapture=move |ev| on_pointer_end(ev, PointerEnd::LostCapture)
        >
            <div class="resize-handle-line" />
        </div>
    }
}
