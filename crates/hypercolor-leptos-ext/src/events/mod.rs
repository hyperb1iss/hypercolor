//! Browser-only event primitives.

use gloo_events::{EventListener, EventListenerOptions};

pub use gloo_events::EventListenerPhase;

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
