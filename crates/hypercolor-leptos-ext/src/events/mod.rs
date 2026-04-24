//! Browser-only event primitives.

use std::str::FromStr;

use gloo_events::{EventListener, EventListenerOptions};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

pub use gloo_events::EventListenerPhase;

pub trait StaticEvent: Sized {
    const EVENT_TYPE: &'static str;

    fn from_event_unchecked(event: web_sys::Event) -> Self;

    fn raw(&self) -> &web_sys::Event;
}

macro_rules! make_event {
    ($name:ident, $event_type:literal, $raw:ty) => {
        pub struct $name {
            event: $raw,
        }

        impl StaticEvent for $name {
            const EVENT_TYPE: &'static str = $event_type;

            fn from_event_unchecked(event: web_sys::Event) -> Self {
                Self {
                    event: event.unchecked_into(),
                }
            }

            fn raw(&self) -> &web_sys::Event {
                self.event.unchecked_ref::<web_sys::Event>()
            }
        }

        impl $name {
            #[must_use]
            pub fn from_event(event: web_sys::Event) -> Self {
                <Self as StaticEvent>::from_event_unchecked(event)
            }

            #[must_use]
            pub const fn raw_as_inner(&self) -> &$raw {
                &self.event
            }

            pub fn stop_propagation(&self) {
                self.event.stop_propagation();
            }

            pub fn prevent_default(&self) {
                self.event.prevent_default();
            }

            pub fn target<T>(&self) -> Option<T>
            where
                T: JsCast,
            {
                self.event
                    .target()
                    .and_then(|target| target.dyn_into().ok())
            }

            pub fn current_target<T>(&self) -> Option<T>
            where
                T: JsCast,
            {
                self.event
                    .current_target()
                    .and_then(|target| target.dyn_into().ok())
            }
        }
    };
}

make_event!(Input, "input", web_sys::Event);
make_event!(Change, "change", web_sys::Event);

macro_rules! impl_input_target_helpers {
    ($name:ident) => {
        impl $name {
            pub fn value<T>(&self) -> Option<T>
            where
                T: FromStr,
            {
                self.input_element()
                    .and_then(|element| element.value().parse::<T>().ok())
            }

            pub fn value_string(&self) -> Option<String> {
                self.input_element().map(|element| element.value())
            }

            pub fn checked(&self) -> Option<bool> {
                self.input_element().map(|element| element.checked())
            }

            pub fn files(&self) -> Option<web_sys::FileList> {
                self.input_element().and_then(|element| element.files())
            }

            fn input_element(&self) -> Option<web_sys::HtmlInputElement> {
                self.target()
            }
        }
    };
}

impl_input_target_helpers!(Input);
impl_input_target_helpers!(Change);

pub fn target_element(target: Option<web_sys::EventTarget>) -> Option<web_sys::Element> {
    target.and_then(|target| target.dyn_into().ok())
}

pub fn target_closest(target: Option<web_sys::EventTarget>, selector: &str) -> bool {
    target_element(target).is_some_and(|element| element.closest(selector).ok().flatten().is_some())
}

pub fn target_is_text_entry(target: Option<web_sys::EventTarget>) -> bool {
    target_element(target).is_some_and(|element| {
        let tag = element.tag_name();
        tag.eq_ignore_ascii_case("input")
            || tag.eq_ignore_ascii_case("textarea")
            || tag.eq_ignore_ascii_case("select")
            || element.has_attribute("contenteditable")
            || element
                .closest("[contenteditable='true']")
                .ok()
                .flatten()
                .is_some()
    })
}

pub fn document() -> Option<web_sys::Document> {
    web_sys::window().and_then(|window| window.document())
}

pub fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

pub fn document_event_target(document: &web_sys::Document) -> &web_sys::EventTarget {
    document.unchecked_ref()
}

pub fn scroll_into_view_start(element: &web_sys::Element) {
    let options = web_sys::ScrollIntoViewOptions::new();
    options.set_behavior(web_sys::ScrollBehavior::Smooth);
    options.set_block(web_sys::ScrollLogicalPosition::Start);
    element.scroll_into_view_with_scroll_into_view_options(&options);
}

/// RAII browser event listener handle.
pub struct EventHandle {
    listener: Option<EventListener>,
}

impl EventHandle {
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.listener.is_some()
    }

    pub fn cancel(&mut self) {
        self.listener.take();
    }
}

/// Register an event listener and remove it when the returned handle is dropped.
#[must_use]
pub fn on<S, F>(target: &web_sys::EventTarget, event_type: S, callback: F) -> EventHandle
where
    S: Into<std::borrow::Cow<'static, str>>,
    F: FnMut(&web_sys::Event) + 'static,
{
    EventHandle {
        listener: Some(EventListener::new(target, event_type, callback)),
    }
}

/// Register an event listener with capture/passive options.
#[must_use]
pub fn on_with_options<S, F>(
    target: &web_sys::EventTarget,
    event_type: S,
    options: EventListenerOptions,
    callback: F,
) -> EventHandle
where
    S: Into<std::borrow::Cow<'static, str>>,
    F: FnMut(&web_sys::Event) + 'static,
{
    EventHandle {
        listener: Some(EventListener::new_with_options(
            target, event_type, options, callback,
        )),
    }
}

pub struct WorkerMessageHandler {
    _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl WorkerMessageHandler {
    #[must_use]
    pub fn attach<F>(worker: &web_sys::Worker, on_message: F) -> Self
    where
        F: FnMut(web_sys::MessageEvent) + 'static,
    {
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(on_message);
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        Self {
            _on_message: on_message,
        }
    }

    pub fn detach_from(&self, worker: &web_sys::Worker) {
        worker.set_onmessage(None);
    }
}
