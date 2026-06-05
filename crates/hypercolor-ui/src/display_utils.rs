//! Pure helpers used by the displays page that are worth unit-testing in
//! isolation. Keeping them out of `pages/displays.rs` avoids dragging the
//! full Leptos component tree into test builds.

use crate::api;

/// Returns `true` when a display summary came from the virtual simulator
/// backend (distinguished by its `family` field). Used by the displays
/// page to show the "Simulator" badge and to gate the edit/delete UI.
#[must_use]
pub fn is_simulator_display(display: &api::DisplaySummary) -> bool {
    display.family.eq_ignore_ascii_case("simulator")
}

/// Parse a user-supplied simulator dimension string into a positive u32.
///
/// Trims whitespace, rejects zero and non-numeric input, and formats a
/// friendly error message citing the field label so validation feedback
/// can flow directly into the modal form.
pub fn parse_simulator_dimension(raw: &str, label: &str) -> Result<u32, String> {
    raw.trim()
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{label} must be a positive number."))
}

/// Build the URL of the full-screen preview shell for a display. Opened
/// in a new tab via "Open preview" so users can cast the live face to a
/// secondary monitor or project it alongside the control column.
#[must_use]
pub fn display_preview_shell_url(display_id: &str) -> String {
    format!("/preview?display={display_id}")
}
