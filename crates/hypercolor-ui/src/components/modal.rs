//! Shared modal behavior wrapper.
//!
//! Owns the *behavior* of every modal surface — backdrop dismissal,
//! `role="dialog"` + `aria-modal` semantics, Escape-to-close, initial focus,
//! Tab focus trapping, and focus restoration on close — while leaving the
//! visuals entirely to the caller. Callers pass their existing container and
//! backdrop classes through untouched and render their panel as children, so
//! adopting the wrapper never changes how a modal looks.

use hypercolor_leptos_ext::events::document as browser_document;
use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Selector matching everything keyboard focus can land on inside a dialog.
const FOCUSABLE_SELECTOR: &str = "a[href], button:not([disabled]), input:not([disabled]), \
     select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])";

fn focusable_elements(dialog: &web_sys::HtmlElement) -> Vec<web_sys::HtmlElement> {
    let Ok(list) = dialog.query_selector_all(FOCUSABLE_SELECTOR) else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|index| list.get(index))
        .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
        .collect()
}

fn active_element() -> Option<web_sys::Element> {
    browser_document().and_then(|doc| doc.active_element())
}

fn contains_element(container: &web_sys::HtmlElement, element: &web_sys::Element) -> bool {
    let node: &web_sys::Node = element;
    container.contains(Some(node))
}

fn is_same_element(a: &web_sys::Element, b: &web_sys::HtmlElement) -> bool {
    let a_value: &wasm_bindgen::JsValue = a.as_ref();
    let b_value: &wasm_bindgen::JsValue = b.as_ref();
    a_value == b_value
}

/// Behavior-only modal wrapper: backdrop + dialog semantics + keyboard
/// handling. Children render the visual panel exactly as before adoption.
#[component]
pub fn Modal(
    /// Fires when the user asks to dismiss (Escape or a backdrop click).
    #[prop(into)]
    on_close: Callback<()>,
    /// Classes for the outer fixed-position container (centering, z-index,
    /// entrance animation). Owned by the caller for visual parity.
    #[prop(into)]
    container_class: String,
    /// Classes for the backdrop layer (dim + blur), rendered behind children.
    #[prop(into)]
    backdrop_class: String,
    /// Accessible name announced for the dialog.
    #[prop(into, optional)]
    label: MaybeProp<String>,
    /// When false, Escape and backdrop clicks are ignored (busy overlays).
    #[prop(into, optional)]
    dismissible: MaybeProp<bool>,
    /// When false, clicking the backdrop does not dismiss (Escape still can).
    #[prop(default = true)]
    close_on_backdrop: bool,
    children: Children,
) -> impl IntoView {
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    let can_dismiss = move || dismissible.get_untracked().unwrap_or(true);

    // Remember what had focus before the modal opened and give it back on
    // close. `_local` because DOM handles are not `Send`.
    let previous_focus = StoredValue::new_local(
        active_element().and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok()),
    );
    on_cleanup(move || {
        if let Some(el) = previous_focus.get_value() {
            let _ = el.focus();
        }
    });

    // Initial focus: the container itself (tabindex=-1) unless a child —
    // e.g. an autofocused input — already claimed focus during mount.
    Effect::new(move |_| {
        let Some(container) = dialog_ref.get() else {
            return;
        };
        let inside = active_element().is_some_and(|el| contains_element(&container, &el));
        if !inside {
            let _ = container.focus();
        }
    });

    // Escape closes; Tab cycles within the dialog instead of escaping it.
    // `default_prevented` lets inner widgets (dropdowns) consume Escape for
    // themselves without also dismissing the modal.
    let _keydown = window_event_listener(ev::keydown, move |event| match event.key().as_str() {
        "Escape" if !event.default_prevented() && can_dismiss() => {
            event.prevent_default();
            on_close.run(());
        }
        "Tab" => {
            let Some(container) = dialog_ref.get_untracked() else {
                return;
            };
            let focusables = focusable_elements(&container);
            let Some((first, last)) = focusables.first().zip(focusables.last()) else {
                event.prevent_default();
                let _ = container.focus();
                return;
            };
            let active = active_element();
            let inside = active
                .as_ref()
                .is_some_and(|el| contains_element(&container, el));
            if !inside {
                event.prevent_default();
                let _ = first.focus();
            } else if event.shift_key()
                && active.as_ref().is_some_and(|el| is_same_element(el, first))
            {
                event.prevent_default();
                let _ = last.focus();
            } else if !event.shift_key()
                && active.as_ref().is_some_and(|el| is_same_element(el, last))
            {
                event.prevent_default();
                let _ = first.focus();
            }
        }
        _ => {}
    });

    view! {
        <div
            node_ref=dialog_ref
            class=container_class
            role="dialog"
            aria-modal="true"
            aria-label=move || label.get()
            tabindex="-1"
        >
            <div
                class=backdrop_class
                on:click=move |_| {
                    if close_on_backdrop && can_dismiss() {
                        on_close.run(());
                    }
                }
            />
            {children()}
        </div>
    }
}
