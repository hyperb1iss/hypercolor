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

/// The element id of the tab panel both view tabs control.
pub const PANEL_ID: &str = "effects-view-panel";

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

    /// The element id of this view's tab.
    #[must_use]
    pub const fn tab_id(self) -> &'static str {
        match self {
            Self::Browse => "effects-view-browse",
            Self::Controls => "effects-view-controls",
        }
    }
}

/// The view a key pressed on the view tabs selects. The two tabs sit in a
/// row: Left and Right step to the other one (wrapping, as tabs do), Home
/// selects the first and End the last. `None` for any other key.
#[must_use]
pub fn view_for_key(current: PhoneView, key: &str) -> Option<PhoneView> {
    let other = match current {
        PhoneView::Browse => PhoneView::Controls,
        PhoneView::Controls => PhoneView::Browse,
    };
    match key {
        "ArrowLeft" | "ArrowRight" => Some(other),
        "Home" => Some(PhoneView::Browse),
        "End" => Some(PhoneView::Controls),
        _ => None,
    }
}
