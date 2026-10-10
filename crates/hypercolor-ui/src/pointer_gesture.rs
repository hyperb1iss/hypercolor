//! Single-pointer ownership for drag, resize, and press-hold surfaces.
//!
//! Every drag surface in the UI follows one model, so mouse, pen, and touch
//! behave the same way:
//!
//! - A press claims the gesture for its pointer and captures that pointer on
//!   the pressed element. Moves and the release keep arriving there after
//!   the pointer leaves it, with no window-level listeners.
//! - Only the owning pointer moves or ends the gesture. A second finger that
//!   lands mid-drag is ignored, so every surface stays single-finger.
//! - A release commits. Releasing the pressing button while another stays
//!   held also commits: pointer events report that as a `pointermove`, and
//!   the old `mouseup` handlers ended the drag there. A `pointercancel`, or a
//!   capture lost before the release, cancels: the surface puts back what it
//!   showed before the press.
//! - Drag surfaces set `touch-action: none` so the browser never turns the
//!   drag into a page scroll or zoom (which would cancel it).
//! - A mouse or pen press cancels its default actions (text selection, focus
//!   moves), as the old `mousedown` handlers did. A touch press keeps them:
//!   `touch-action` already stops panning, and Chromium gives a tap whose
//!   `pointerdown` was cancelled a click `detail` of 0 and never follows it
//!   with `dblclick`, which would break double-tap.
//!
//! Capture goes on the element that received the press, never on an
//! ancestor: browsers retarget `click` and `dblclick` to the capture
//! target, so capturing on an ancestor would swallow clicks meant for the
//! pressed element.
//!
//! [`PointerGesture`] is the pure ownership state machine. The free
//! functions are the thin DOM half.

use wasm_bindgen::JsCast;

/// How a gesture ended, from the surface's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureEnd {
    /// The pointer was released: keep the result.
    Commit,
    /// The browser took the pointer away or capture was lost before the
    /// release: restore the state from before the press.
    Cancel,
}

/// The pointer event that ends a gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEnd {
    /// `pointerup`.
    Up,
    /// `pointercancel`.
    Cancel,
    /// `lostpointercapture`. Browsers fire it after every `pointerup` and
    /// `pointercancel` too, so it only cancels a gesture that is still live.
    LostCapture,
}

impl PointerEnd {
    const fn outcome(self) -> GestureEnd {
        match self {
            Self::Up => GestureEnd::Commit,
            Self::Cancel | Self::LostCapture => GestureEnd::Cancel,
        }
    }
}

/// What a new press means for the gesture.
#[derive(Debug, PartialEq, Eq)]
pub enum Press<S> {
    /// No gesture is live. The surface may start one.
    Ready,
    /// A gesture was live but its end never arrived (the same pointer
    /// pressed again, or the owner no longer holds capture). The stale
    /// state comes back so the surface can roll it back before starting.
    Stale(S),
    /// Another pointer owns a live gesture. Ignore this press.
    Busy,
}

/// The `buttons` bit for a press's `button` value: primary, auxiliary,
/// secondary, back, forward, and pen eraser. Any other value maps to 0,
/// which no move can report released.
#[must_use]
pub const fn button_mask(button: i16) -> u16 {
    match button {
        0 => 1,
        1 => 4,
        2 => 2,
        3 => 8,
        4 => 16,
        5 => 32,
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner<S> {
    pointer_id: i32,
    /// `buttons` bit of the press that started the gesture.
    button: u16,
    state: S,
}

/// Which pointer owns the in-flight gesture, plus that gesture's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerGesture<S> {
    active: Option<Owner<S>>,
}

impl<S> Default for PointerGesture<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> PointerGesture<S> {
    /// An idle gesture.
    #[must_use]
    pub const fn new() -> Self {
        Self { active: None }
    }

    /// True while some pointer owns a gesture.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Classify a press from `pointer_id`. `owner_live` reports whether the
    /// current owner still holds pointer capture; it is only consulted when
    /// a different pointer owns the gesture.
    pub fn press(&mut self, pointer_id: i32, owner_live: impl FnOnce(i32, &S) -> bool) -> Press<S> {
        let Some(owner) = &self.active else {
            return Press::Ready;
        };
        // A pointer cannot press twice without releasing, so a press from
        // the owner means its release was lost.
        if owner.pointer_id != pointer_id && owner_live(owner.pointer_id, &owner.state) {
            return Press::Busy;
        }
        self.active
            .take()
            .map_or(Press::Ready, |owner| Press::Stale(owner.state))
    }

    /// Hand the gesture to `pointer_id`, pressed with `button` (the
    /// event's `button` value). Call after [`Self::press`] returned
    /// [`Press::Ready`] or [`Press::Stale`]; any gesture still live is
    /// replaced.
    pub fn start(&mut self, pointer_id: i32, button: i16, state: S) {
        self.active = Some(Owner {
            pointer_id,
            button: button_mask(button),
            state,
        });
    }

    /// True when a move from the owning pointer reports, through its
    /// `buttons`, that the button which started the gesture is up. End the
    /// gesture with [`PointerEnd::Up`] when it is.
    #[must_use]
    pub fn press_released(&self, pointer_id: i32, buttons: u16) -> bool {
        self.active.as_ref().is_some_and(|owner| {
            owner.pointer_id == pointer_id && owner.button != 0 && buttons & owner.button == 0
        })
    }

    /// The live gesture's state, if `pointer_id` owns it.
    #[must_use]
    pub fn state(&self, pointer_id: i32) -> Option<&S> {
        self.active
            .as_ref()
            .filter(|owner| owner.pointer_id == pointer_id)
            .map(|owner| &owner.state)
    }

    /// Mutable access to the live gesture's state, if `pointer_id` owns it.
    pub fn state_mut(&mut self, pointer_id: i32) -> Option<&mut S> {
        self.active
            .as_mut()
            .filter(|owner| owner.pointer_id == pointer_id)
            .map(|owner| &mut owner.state)
    }

    /// The live gesture's state, whichever pointer owns it. For work that
    /// runs outside a pointer event, like an animation frame.
    #[must_use]
    pub fn current(&self) -> Option<&S> {
        self.active.as_ref().map(|owner| &owner.state)
    }

    /// Mutable access to the live gesture's state, whichever pointer owns
    /// it.
    pub fn current_mut(&mut self) -> Option<&mut S> {
        self.active.as_mut().map(|owner| &mut owner.state)
    }

    /// End the gesture if `pointer_id` owns it, returning how it ended and
    /// its state. Events from any other pointer, and a lost capture that
    /// trails a release, return `None`.
    pub fn end(&mut self, pointer_id: i32, how: PointerEnd) -> Option<(GestureEnd, S)> {
        if !matches!(&self.active, Some(owner) if owner.pointer_id == pointer_id) {
            return None;
        }
        self.active.take().map(|owner| (how.outcome(), owner.state))
    }

    /// Drop the live gesture without an event (the surface is going away).
    pub fn abandon(&mut self) -> Option<S> {
        self.active.take().map(|owner| owner.state)
    }
}

/// Whether a press from this pointer type should cancel its default
/// actions. Only touch keeps them; see the module docs for why.
#[must_use]
pub fn cancels_press_defaults(pointer_type: &str) -> bool {
    pointer_type != "touch"
}

/// Cancel a press's default actions for mouse and pen. Call from every drag
/// surface's `pointerdown` instead of `prevent_default` directly.
pub fn suppress_press_defaults(ev: &web_sys::PointerEvent) {
    if cancels_press_defaults(&ev.pointer_type()) {
        ev.prevent_default();
    }
}

/// The element whose listener is handling `ev`. Leptos attaches `on:`
/// handlers directly to their element (the crate does not enable event
/// delegation), so this is the element the handler was declared on.
#[must_use]
pub fn listener_element(ev: &web_sys::Event) -> Option<web_sys::Element> {
    ev.current_target()?.dyn_into::<web_sys::Element>().ok()
}

/// Capture `ev`'s pointer on `element`, so moves and the release target it
/// until the pointer lifts, wherever the pointer goes.
pub fn capture_pointer(element: &web_sys::Element, ev: &web_sys::PointerEvent) {
    // Capture fails only for a pointer that is no longer active; the
    // gesture then ends on the next event from that pointer anyway.
    let _ = element.set_pointer_capture(ev.pointer_id());
}

/// True while `element` holds capture for `pointer_id`. A gesture whose
/// element lost capture without delivering its end (the element left the
/// document, so `lostpointercapture` went to the document) is stale.
#[must_use]
pub fn holds_capture(element: Option<&web_sys::Element>, pointer_id: i32) -> bool {
    element.is_some_and(|element| element.has_pointer_capture(pointer_id))
}
