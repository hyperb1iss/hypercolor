use leptos::prelude::use_context;

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

/// Select the sidebar canvas using an application-relative pathname.
#[must_use]
pub fn mounted_canvas_mode(pathname: &str) -> NowPlayingCanvasMode {
    let mount = use_context::<UiMount>().unwrap_or_default();
    now_playing_canvas_mode(mount.relative_path(pathname).unwrap_or_default())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NowPlayingCanvasMode {
    Preview,
    Palette,
}

pub fn now_playing_canvas_mode(path: &str) -> NowPlayingCanvasMode {
    if path == "/" || path.starts_with("/effects") || path.starts_with("/studio") {
        NowPlayingCanvasMode::Palette
    } else {
        NowPlayingCanvasMode::Preview
    }
}
