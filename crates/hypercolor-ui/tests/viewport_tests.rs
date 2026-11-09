//! The phone layout signal's breakpoint and its no-context default.

use hypercolor_ui::viewport::{PHONE_MEDIA_QUERY, is_phone_layout};
use leptos::prelude::*;

#[test]
fn the_phone_query_matches_the_md_boundary() {
    // Tailwind's `max-md:` variant compiles to the same range, so CSS and
    // the mount-time signal agree on which side of 768px a viewport is.
    assert_eq!(PHONE_MEDIA_QUERY, "(width < 48rem)");
}

#[test]
fn without_the_root_context_the_desktop_layout_is_assumed() {
    let owner = Owner::new();
    owner.with(|| {
        assert!(!is_phone_layout().get_untracked());
    });
}
