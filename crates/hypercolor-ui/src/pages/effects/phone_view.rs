//! Which half of the Effects page a phone shows, and how it lives in the URL.
//!
//! Phones split the page into Browse (the grid) and Controls (the live
//! preview and controls). The view is part of the address: Controls is
//! `?view=controls` and Browse is the bare page. Every switch writes the URL
//! and the page reads its view back from it, so a link to Controls always
//! lands, even from Browse after an earlier visit through the same link. A
//! view kept beside the URL would go stale there: the query is a memo, and
//! navigating to the address it already holds never notifies.

/// The query parameter that carries the view.
pub const VIEW_PARAM: &str = "view";

/// One half of the phone Effects page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneView {
    /// The effect grid.
    Browse,
    /// The live preview and controls of what is playing.
    Controls,
}

impl PhoneView {
    /// The view a `view` query value names. Anything but `controls`,
    /// including no value, is Browse.
    #[must_use]
    pub fn from_query(value: Option<&str>) -> Self {
        if value == Some("controls") {
            Self::Controls
        } else {
            Self::Browse
        }
    }

    /// The application path that shows this view.
    #[must_use]
    pub const fn href(self) -> &'static str {
        match self {
            Self::Browse => "/effects",
            Self::Controls => "/effects?view=controls",
        }
    }
}
