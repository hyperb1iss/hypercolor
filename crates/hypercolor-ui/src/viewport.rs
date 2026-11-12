//! Phone layout detection.
//!
//! Spec 75 makes `md` (768px) the single phone/desktop boundary: below it
//! the bottom tab bar replaces the sidebar. Most phone styling is plain
//! `max-md:` CSS, but some surfaces must not *mount* on the wrong side of
//! the boundary: a hidden live preview still streams frames, and the Studio
//! editor is heavy enough that phones land on a summary and mount it on
//! request.
//! Those read [`is_phone_layout`].
//!
//! The signal is evaluated synchronously when the app mounts, so a phone
//! never builds the desktop tree first and tears it down a frame later.

use leptos::prelude::*;

/// The media query matching phone widths. Mirrors Tailwind's `max-md:`
/// variant (`width < 48rem`); the root font size is 112.5%, but media
/// queries resolve `rem` against the initial 16px, so 48rem is 768px.
pub const PHONE_MEDIA_QUERY: &str = "(width < 48rem)";

/// App-wide phone layout flag, provided once at the app root.
#[derive(Clone, Copy)]
pub struct PhoneLayout(pub Signal<bool>);

/// Create the phone layout signal and provide it as context. Call once,
/// at the app root, inside the reactive owner that outlives every page.
pub fn provide_phone_layout() {
    provide_context(PhoneLayout(phone_layout_signal()));
}

/// Whether the viewport is below the `md` breakpoint. Without the root
/// context (unit tests, isolated mounts) the desktop layout is assumed.
#[must_use]
pub fn is_phone_layout() -> Signal<bool> {
    use_context::<PhoneLayout>().map_or_else(|| Signal::stored(false), |layout| layout.0)
}

#[cfg(target_arch = "wasm32")]
fn phone_layout_signal() -> Signal<bool> {
    use leptos_use_event_listener::{UseEventListenerOptions, use_event_listener_with_options};

    let Some(query) =
        web_sys::window().and_then(|window| window.match_media(PHONE_MEDIA_QUERY).ok().flatten())
    else {
        return Signal::stored(false);
    };
    let (matches, set_matches) = signal(query.matches());
    let source = query.clone();
    // The listener lives as long as the owner that called this, and
    // leptos-use removes it on cleanup.
    let _ = use_event_listener_with_options(
        query,
        leptos::ev::change,
        move |_| set_matches.set(source.matches()),
        UseEventListenerOptions::default().passive(true),
    );
    matches.into()
}

#[cfg(not(target_arch = "wasm32"))]
fn phone_layout_signal() -> Signal<bool> {
    Signal::stored(false)
}
