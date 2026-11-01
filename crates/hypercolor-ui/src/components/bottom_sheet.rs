//! Phone bottom sheet: a dialog that rises from the bottom edge.
//!
//! Behavior comes from [`Modal`]: dialog semantics, Escape, a backdrop tap,
//! focus trapping and restoration. On top of that the sheet adds what
//! phones expect: a grab area that drags the sheet down and dismisses it
//! past a distance or on a downward flick, a body that scrolls without
//! chaining into the page beneath, and padding for the home indicator.
//!
//! The drag follows the shared pointer gesture model
//! ([`crate::pointer_gesture`]): the press captures its pointer on the grab
//! area, one pointer owns the drag, a release decides, and a cancel or a
//! lost capture snaps the sheet back.

use std::time::Duration;

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::modal::Modal;
use crate::icons::LuX;
use crate::pointer_gesture::{
    GestureEnd, PointerEnd, PointerGesture, Press, capture_pointer, holds_capture,
    listener_element, suppress_press_defaults,
};

/// A drag this far down always dismisses, on any sheet taller than four
/// times it.
pub const DISMISS_DISTANCE_PX: f64 = 96.0;
/// A drag past this fraction of the sheet's height dismisses a short sheet.
pub const DISMISS_FRACTION: f64 = 0.25;
/// A release moving down at least this fast (px per ms) is a flick.
pub const FLICK_VELOCITY: f64 = 0.5;
/// A flick only dismisses once the sheet has moved this far, so a tap
/// with a little jitter never closes it.
pub const FLICK_MIN_PX: f64 = 24.0;
/// How long the exit slide runs before the sheet unmounts. Matches the
/// `.sheet-panel` transition in `input.css`.
pub const EXIT_MS: u64 = 200;

/// Whether releasing a drag that moved the sheet `distance` px down, on a
/// sheet `height` px tall, at `velocity` px per ms (positive is down),
/// dismisses it.
#[must_use]
pub fn drag_dismisses(distance: f64, height: f64, velocity: f64) -> bool {
    if distance <= 0.0 {
        return false;
    }
    let threshold = DISMISS_DISTANCE_PX.min(height * DISMISS_FRACTION);
    distance >= threshold || (velocity >= FLICK_VELOCITY && distance >= FLICK_MIN_PX)
}

/// Blend a new velocity sample into the running estimate, so one noisy
/// move event cannot fake or hide a flick.
#[must_use]
pub fn smoothed_velocity(previous: f64, dy: f64, dt_ms: f64) -> f64 {
    if dt_ms <= 0.0 {
        return previous;
    }
    previous.mul_add(0.4, (dy / dt_ms) * 0.6)
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    start_y: f64,
    last_y: f64,
    last_t: f64,
    velocity: f64,
}

fn prefers_reduced_motion() -> bool {
    web_sys::window()
        .and_then(|window| {
            window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|query| query.matches())
}

/// A bottom sheet. `children` render inside the scrolling body.
#[component]
pub fn BottomSheet(
    /// Fires once the sheet has finished closing.
    #[prop(into)]
    on_close: Callback<()>,
    /// Accessible name for the dialog, also shown as its heading.
    #[prop(into)]
    title: String,
    /// Hide the heading text visually (the content names itself).
    #[prop(default = false)]
    hide_title: bool,
    children: ChildrenFn,
) -> impl IntoView {
    let offset = RwSignal::new(0.0_f64);
    let dragging = RwSignal::new(false);
    let closing = RwSignal::new(false);
    let panel_ref = NodeRef::<leptos::html::Div>::new();
    let gesture = StoredValue::new(PointerGesture::<Drag>::new());

    let close = move || {
        if closing.get_untracked() {
            return;
        }
        closing.set(true);
        if prefers_reduced_motion() {
            on_close.run(());
            return;
        }
        // The sheet may unmount another way before the slide finishes, so
        // the deferred close must tolerate a disposed callback.
        set_timeout(
            move || {
                let _ = on_close.try_run(());
            },
            Duration::from_millis(EXIT_MS),
        );
    };

    // A phone rotated or resized past `md` drops the sheet; every surface
    // that opens one is phone-only.
    let phone = crate::viewport::is_phone_layout();
    Effect::new(move |_| {
        if !phone.get() {
            close();
        }
    });

    let finish = move |ev: &web_sys::PointerEvent, how: PointerEnd| {
        let Some((end, drag)) = gesture
            .try_update_value(|gesture| gesture.end(ev.pointer_id(), how))
            .flatten()
        else {
            return;
        };
        dragging.set(false);
        let height = panel_ref
            .get_untracked()
            .map_or(0.0, |panel| f64::from(panel.offset_height()));
        let distance = offset.get_untracked();
        if end == GestureEnd::Commit && drag_dismisses(distance, height, drag.velocity) {
            close();
        } else {
            offset.set(0.0);
        }
    };

    let on_pointerdown = move |ev: web_sys::PointerEvent| {
        if ev.button() != 0 || closing.get_untracked() {
            return;
        }
        let element = listener_element(&ev);
        let press = gesture
            .try_update_value(|gesture| {
                gesture.press(ev.pointer_id(), |owner, _| {
                    holds_capture(element.as_ref(), owner)
                })
            })
            .unwrap_or(Press::Busy);
        if matches!(press, Press::Busy) {
            return;
        }
        suppress_press_defaults(&ev);
        if let Some(element) = element.as_ref() {
            capture_pointer(element, &ev);
        }
        let y = f64::from(ev.client_y());
        gesture.update_value(|gesture| {
            gesture.start(
                ev.pointer_id(),
                ev.button(),
                Drag {
                    start_y: y,
                    last_y: y,
                    last_t: ev.time_stamp(),
                    velocity: 0.0,
                },
            );
        });
        offset.set(0.0);
        dragging.set(true);
    };

    let on_pointermove = move |ev: web_sys::PointerEvent| {
        let released =
            gesture.with_value(|gesture| gesture.press_released(ev.pointer_id(), ev.buttons()));
        if released {
            finish(&ev, PointerEnd::Up);
            return;
        }
        let y = f64::from(ev.client_y());
        let t = ev.time_stamp();
        let moved = gesture
            .try_update_value(|gesture| {
                gesture.state_mut(ev.pointer_id()).map(|drag| {
                    drag.velocity =
                        smoothed_velocity(drag.velocity, y - drag.last_y, t - drag.last_t);
                    drag.last_y = y;
                    drag.last_t = t;
                    (y - drag.start_y).max(0.0)
                })
            })
            .flatten();
        if let Some(distance) = moved {
            offset.set(distance);
        }
    };

    let panel_style = move || {
        if closing.get() {
            "transform: translateY(100%)".to_owned()
        } else {
            format!("transform: translateY({}px)", offset.get())
        }
    };

    view! {
        <Modal
            on_close=Callback::new(move |()| close())
            container_class="bottom-sheet fixed inset-0 z-[60] flex flex-col justify-end"
            backdrop_class="sheet-backdrop absolute inset-0 bg-black/60 animate-enter-fade"
            label=title.clone()
        >
            <div
                node_ref=panel_ref
                class="sheet-panel relative flex max-h-[85dvh] w-full flex-col rounded-t-xl \
                       border-x border-t border-edge-subtle glass-dense modal-glow \
                       animate-enter-sheet"
                class:sheet-dragging=move || dragging.get()
                attr:data-closing=move || closing.get().then_some("")
                style=panel_style
            >
                // Grab area: the handle and heading. Dragging it pulls the
                // sheet down; the browser never turns that into a scroll.
                <div
                    class="sheet-grab relative min-h-11 shrink-0 select-none px-4 pt-2"
                    on:pointerdown=on_pointerdown
                    on:pointermove=on_pointermove
                    on:pointerup=move |ev| finish(&ev, PointerEnd::Up)
                    on:pointercancel=move |ev| finish(&ev, PointerEnd::Cancel)
                    on:lostpointercapture=move |ev| finish(&ev, PointerEnd::LostCapture)
                >
                    <div class="mx-auto h-1 w-10 rounded-full bg-fg-tertiary/40" aria-hidden="true" />
                    <h2
                        class="truncate pb-2 pr-12 pt-3 text-sm font-semibold text-fg-primary"
                        class:sr-only=hide_title
                    >
                        {title.clone()}
                    </h2>
                    <button
                        type="button"
                        class="absolute right-2 top-1 flex h-11 w-11 items-center justify-center \
                               rounded-lg text-fg-tertiary btn-press"
                        aria-label="Close"
                        on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
                        on:click=move |_| close()
                    >
                        <Icon icon=LuX width="18px" height="18px" />
                    </button>
                </div>
                <div class="sheet-body min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 \
                            pb-[calc(1rem+env(safe-area-inset-bottom))]">
                    {children()}
                </div>
            </div>
        </Modal>
    }
}
