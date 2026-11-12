//! What a Studio address asks for: which face of the page a phone shows,
//! and whether the visit is about the screens.
//!
//! Below `md` Studio opens on the zone summary, and the spatial editor is
//! one tap away at `?view=editor`. The view is part of the address, so the
//! browser's back button leaves the editor and a link to it always lands.
//! At `md` and up the editor is the page and the parameter is ignored.

/// The query parameter that carries the view.
pub const VIEW_PARAM: &str = "view";

/// One face of the phone Studio page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudioPhoneView {
    /// The per-zone summary with effect switching.
    Summary,
    /// The spatial editor, with its device pane in a drawer.
    Editor,
}

impl StudioPhoneView {
    /// The view a `view` query value names. Anything but `editor`,
    /// including no value, is the summary.
    #[must_use]
    pub fn from_query(value: Option<&str>) -> Self {
        if value == Some("editor") {
            Self::Editor
        } else {
            Self::Summary
        }
    }

    /// The application path that shows this view.
    #[must_use]
    pub const fn href(self) -> &'static str {
        match self {
            Self::Summary => "/studio",
            Self::Editor => "/studio?view=editor",
        }
    }
}

/// The query parameter that asks Studio to open on a kind of surface.
pub const SURFACE_PARAM: &str = "surface";

/// The address that opens Studio on its screens, at any width: a phone
/// needs the editor, and wider layouts ignore the view.
pub const SCREENS_HREF: &str = "/studio?view=editor&surface=screens";

/// Whether a `surface` query value asks for the screens.
#[must_use]
pub fn wants_screens(value: Option<&str>) -> bool {
    value == Some("screens")
}
