//! Generic UI extension seam.
//!
//! Lets a downstream entry crate inject extra top-level routes, sidebar nav
//! items, and (later) settings sections into the app **at startup**, without the
//! OSS tree knowing anything about what is being injected. This is the UI
//! analogue of the daemon's `DaemonExtensionInstaller`/`ApiExtension` seam: the
//! OSS app builds and runs identically with an empty [`UiExtensions`], and the
//! seam itself names nothing domain-specific.
//!
//! ## Why routes are passed by value, not through context
//!
//! Leptos Router 0.8 builds its `RouteDefs` once, when `<Routes>` is
//! constructed — routes cannot be added after the fact. So the seam is a
//! one-time composition at mount, not live mutation. Route defs are erased into
//! [`AnyNestedRoute`], which is `Send` but **not `Sync`**, so they cannot go
//! through `provide_context` (which requires `Send + Sync`). They are therefore
//! threaded **by value** from [`run_with_extensions`](crate::run_with_extensions)
//! into the mount closure. Nav items are plain `Send + Sync` data and may be
//! provided through context.

use leptos::prelude::IntoView;
use leptos_router::any_nested_route::{AnyNestedRoute, IntoAnyNestedRoute};
use leptos_router::{MatchNestedRoutes, NestedRoute, PossibleRouteMatch};

/// Build a type-erased leaf route from a matcher segment and a view closure.
///
/// `path` is normally produced by [`leptos_router::path!`]. The view closure
/// must be `Clone + Send` because the router clones route defs while matching;
/// it is the same bound Leptos Router places on every `view=` prop.
pub fn ui_route<S, F, V>(path: S, view: F) -> AnyNestedRoute
where
    S: PossibleRouteMatch + Clone + Send + 'static,
    F: Fn() -> V + Clone + Send + 'static,
    V: IntoView + 'static,
{
    NestedRoute::new(path, move || view().into_view()).into_any_nested_route()
}

/// Build a type-erased parent route that wraps `children` and renders `view`
/// (which should contain an [`leptos_router::components::Outlet`] for the
/// children to render into).
pub fn parent_route<S, F, V, C>(path: S, view: F, children: C) -> AnyNestedRoute
where
    S: PossibleRouteMatch + Clone + Send + 'static,
    F: Fn() -> V + Clone + Send + 'static,
    V: IntoView + 'static,
    C: MatchNestedRoutes + Clone + Send + 'static,
{
    NestedRoute::new(path, move || view().into_view())
        .child(children)
        .into_any_nested_route()
}

/// A sidebar navigation entry contributed by an extension.
///
/// Mirrors the OSS core nav model so injected items render with the same
/// affordances. Plain `Send + Sync` data — safe to thread through context.
#[derive(Clone)]
pub struct UiNavItem {
    /// Route path the item links to (e.g. `"/account"`).
    pub path: &'static str,
    /// Label shown beside the icon.
    pub label: &'static str,
    /// Icon glyph rendered in the rail.
    pub icon: icondata_core::Icon,
    /// Draw a hairline divider above this item.
    pub divider_before: bool,
    /// Whether this item should claim a `Ctrl/Cmd+<digit>` shortcut slot in
    /// sidebar order. Items that set this to `false` are reachable only by
    /// click, leaving the core shortcut numbering undisturbed.
    pub shortcut: bool,
}

impl UiNavItem {
    /// Construct a nav item that takes a `Ctrl/Cmd+<digit>` shortcut slot and
    /// no leading divider — the common case.
    #[must_use]
    pub fn new(path: &'static str, label: &'static str, icon: icondata_core::Icon) -> Self {
        Self {
            path,
            label,
            icon,
            divider_before: false,
            shortcut: true,
        }
    }

    /// Draw a hairline divider above this item.
    #[must_use]
    pub fn with_divider(mut self) -> Self {
        self.divider_before = true;
        self
    }

    /// Make this item click-only — it claims no keyboard shortcut slot.
    #[must_use]
    pub fn without_shortcut(mut self) -> Self {
        self.shortcut = false;
        self
    }
}

/// Placeholder for an extension-contributed settings section.
///
/// Settings-section injection is deferred past the current MVP; the type exists
/// so [`UiExtensions`] carries the field without a later breaking change.
#[derive(Clone)]
pub struct UiSettingsSection {
    /// Stable identifier for the section.
    pub id: &'static str,
}

/// Registry of everything an embedder injects into the app at startup.
///
/// Default is empty, and the OSS entry ([`run`](crate::run)) uses exactly that —
/// so the standalone OSS app is byte-for-byte unchanged.
#[derive(Default)]
pub struct UiExtensions {
    /// Extra top-level routes, composed into the router once, by value. Each is
    /// rendered inside the app shell (they are appended as children of the
    /// shell parent route).
    pub routes: Vec<AnyNestedRoute>,
    /// Extra sidebar nav items, appended after the core items. Threaded through
    /// context (plain `Send + Sync` data).
    pub nav_items: Vec<UiNavItem>,
    /// Extra settings sections (deferred; reserved for future use).
    pub settings_sections: Vec<UiSettingsSection>,
}
