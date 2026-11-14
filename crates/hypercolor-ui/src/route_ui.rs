use leptos::prelude::use_context;

use crate::pages::effects::phone_view::PhoneView;

/// Independent, application-root paths for routes and bundled static assets.
/// Empty paths and `/` select the origin root. Mounts are fixed at startup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UiMount {
    route_base: String,
    asset_base: String,
}

impl UiMount {
    /// Validate same-origin path prefixes, without query strings or dot segments.
    /// Trailing slashes are normalized away. URL-encoded prefixes are not accepted.
    pub fn new(route_base: &str, asset_base: &str) -> Result<Self, &'static str> {
        fn prefix(value: &str) -> Result<String, &'static str> {
            if value.is_empty() || value == "/" {
                return Ok(String::new());
            }
            if !value.starts_with('/')
                || value.contains("//")
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte))
                || value.split('/').any(|part| part == "." || part == "..")
            {
                return Err("UI mount must be an absolute path without empty or dot segments");
            }
            Ok(value.trim_end_matches('/').to_owned())
        }
        Ok(Self {
            route_base: prefix(route_base)?,
            asset_base: prefix(asset_base)?,
        })
    }

    /// Keep this mount's asset base and take the route base from `routes`.
    ///
    /// An embedder that relocates the application's routes, such as a
    /// bridged Remote page, must not move the static assets the application
    /// was built to load.
    #[must_use]
    pub fn with_routes_from(&self, routes: &Self) -> Self {
        Self {
            route_base: routes.route_base.clone(),
            asset_base: self.asset_base.clone(),
        }
    }

    /// Base passed to Leptos Router. Imperative navigation already applies it.
    #[must_use]
    pub fn route_base(&self) -> &str {
        &self.route_base
    }

    /// Resolve a rendered application-root link. Do not pass the result to `use_navigate`.
    #[must_use]
    pub fn route_href(&self, path: &str) -> String {
        format!("{}{path}", self.route_base)
    }

    /// Resolve an application-root static asset independently from the route mount.
    #[must_use]
    pub fn asset_href(&self, path: &str) -> String {
        format!("{}{path}", self.asset_base)
    }

    /// Strip the mount only at a path segment boundary; another app is not this app.
    #[must_use]
    pub fn relative_path<'a>(&self, pathname: &'a str) -> Option<&'a str> {
        let remaining = pathname.strip_prefix(&self.route_base)?;
        if remaining.is_empty() {
            Some("/")
        } else {
            remaining.starts_with('/').then_some(remaining)
        }
    }

    /// Match a navigation item against the browser's full pathname.
    #[must_use]
    pub fn route_is_active(&self, pathname: &str, route: &str) -> bool {
        self.relative_path(pathname).is_some_and(|current| {
            current == route
                || (route != "/"
                    && current
                        .strip_prefix(route)
                        .is_some_and(|rest| rest.starts_with('/')))
        })
    }
}

/// Resolve a rendered application-root link using the current startup mount.
#[must_use]
pub fn route_href(path: &str) -> String {
    use_context::<UiMount>()
        .unwrap_or_default()
        .route_href(path)
}

/// Resolve a bundled image using the current startup asset mount.
#[must_use]
pub fn asset_href(path: &str) -> String {
    use_context::<UiMount>()
        .unwrap_or_default()
        .asset_href(path)
}

/// Test navigation state using the current startup route mount.
#[must_use]
pub fn route_is_active(pathname: &str, route: &str) -> bool {
    use_context::<UiMount>()
        .unwrap_or_default()
        .route_is_active(pathname, route)
}

/// [`now_playing_canvas_mode`] for the browser's pathname under the current
/// UI mount.
#[must_use]
pub fn mounted_canvas_mode(
    pathname: &str,
    view: Option<&str>,
    phone: bool,
) -> NowPlayingCanvasMode {
    let mount = use_context::<UiMount>().unwrap_or_default();
    now_playing_canvas_mode(
        mount.relative_path(pathname).unwrap_or_default(),
        view,
        phone,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NowPlayingCanvasMode {
    Preview,
    Palette,
}

/// The player's thumbnail for an application-relative `path`: a palette
/// swatch where the page shows its own preview, the live canvas elsewhere.
/// `view` and `phone` are as in [`page_has_own_preview`].
#[must_use]
pub fn now_playing_canvas_mode(
    path: &str,
    view: Option<&str>,
    phone: bool,
) -> NowPlayingCanvasMode {
    if page_has_own_preview(path, view, phone) {
        NowPlayingCanvasMode::Palette
    } else {
        NowPlayingCanvasMode::Preview
    }
}

/// Whether a page renders its own main preview with a zone picker beside
/// it: the dashboard and effects cabinet's zone tabs, and Studio's zone
/// list. The player hides its live thumbnail on these pages. Every preview
/// follows the shared selected zone, including the player thumbnail.
///
/// `view` is the address's `view` query value and `phone` the phone
/// layout. A phone splits Effects into Browse and Controls, and only
/// Controls mounts the cabinet, so Effects counts as having its own
/// preview there only in Controls. Wider layouts ignore the view.
#[must_use]
pub fn page_has_own_preview(path: &str, view: Option<&str>, phone: bool) -> bool {
    if path.starts_with("/effects") {
        return !phone || PhoneView::from_query(view) == PhoneView::Controls;
    }
    path == "/" || path.starts_with("/studio")
}
