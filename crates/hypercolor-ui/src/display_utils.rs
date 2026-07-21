//! Pure display helpers that are worth unit-testing in isolation, kept out
//! of the component tree so test builds stay Leptos-free.

/// Build the URL of the full-screen preview shell for a display. Opened
/// in a new tab via "Open preview" so users can cast the live face to a
/// secondary monitor or project it alongside the control column.
#[must_use]
pub fn display_preview_shell_url(display_id: &str) -> String {
    format!("/preview?display={display_id}")
}
