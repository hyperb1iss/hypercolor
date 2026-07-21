//! Single source of truth for the sidebar nav set and the `Ctrl/Cmd+<digit>`
//! shortcut map.
//!
//! Both the sidebar render (`components/sidebar.rs`) and the shortcut handler
//! (`components/shell.rs`) derive from [`nav_model`], so an extension-contributed
//! item (appended via [`UiExtensions::nav_items`](crate::UiExtensions::nav_items))
//! lands in both places consistently instead of the two lists drifting apart.
//!
//! The extension nav items are read from context — they are plain `Send + Sync`
//! data, unlike the route defs, which are threaded by value (see
//! [`crate::extensions`]). With no extensions installed the model is exactly the
//! core nav set, so OSS behavior is unchanged.

use crate::extensions::UiNavItem;

/// One resolved nav entry. Mirrors the shape the sidebar renders and the shell
/// reads for shortcuts.
#[derive(Clone)]
pub struct NavEntry {
    /// Route path the entry links to.
    pub path: &'static str,
    /// Label shown beside the icon.
    pub label: &'static str,
    /// Icon glyph.
    pub icon: icondata_core::Icon,
    /// Draw a hairline divider above this entry.
    pub divider_before: bool,
    /// Whether the entry participates in `Ctrl/Cmd+<digit>` numbering.
    pub shortcut: bool,
}

impl NavEntry {
    const fn core(path: &'static str, label: &'static str, icon: icondata_core::Icon) -> Self {
        Self {
            path,
            label,
            icon,
            divider_before: false,
            shortcut: true,
        }
    }

    const fn with_divider(mut self) -> Self {
        self.divider_before = true;
        self
    }

    fn from_extension(item: UiNavItem) -> Self {
        Self {
            path: item.path,
            label: item.label,
            icon: item.icon,
            divider_before: item.divider_before,
            shortcut: item.shortcut,
        }
    }
}

/// Extension nav items provided through Leptos context. Plain `Send + Sync`
/// data, cloned into the model when the sidebar/shell ask for it. Absent
/// context (or an empty list) yields the core nav set unchanged.
#[derive(Clone, Default)]
pub struct NavExtensionItems(pub Vec<UiNavItem>);

/// The core nav set (Spec 65 §5.1). `Settings` always sits last with a
/// divider above it.
fn core_nav() -> Vec<NavEntry> {
    use crate::icons::*;

    vec![
        NavEntry::core("/", "Dashboard", LuLayoutDashboard),
        NavEntry::core("/effects", "Effects", LuLayers),
        NavEntry::core("/studio", "Studio", LuLayoutTemplate),
        NavEntry::core("/media", "Media", LuImages),
        NavEntry::core("/devices", "Devices", LuCpu),
        NavEntry::core("/settings", "Settings", LuSettings).with_divider(),
    ]
}

/// The full nav set: core items followed by any extension-contributed items.
///
/// `extra` is the extension nav list (from [`NavExtensionItems`]); pass an empty
/// slice for the OSS default to reproduce the core nav exactly.
#[must_use]
pub fn nav_model(extra: &[UiNavItem]) -> Vec<NavEntry> {
    let mut entries = core_nav();
    entries.extend(extra.iter().cloned().map(NavEntry::from_extension));
    entries
}

/// Resolve a `Ctrl/Cmd+<digit>` key to a nav path in sidebar order.
///
/// Shortcut slots are assigned by position over the entries that opt into a
/// shortcut (`shortcut == true`), so a click-only extension item never shifts
/// the core numbering. Returns `None` for a non-digit key or an out-of-range
/// slot.
#[must_use]
pub fn nav_shortcut_path(extra: &[UiNavItem], key: &str) -> Option<String> {
    let digit = key.parse::<usize>().ok()?;
    if digit == 0 {
        return None;
    }
    nav_model(extra)
        .into_iter()
        .filter(|entry| entry.shortcut)
        .nth(digit - 1)
        .map(|entry| entry.path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{nav_model, nav_shortcut_path};
    use crate::extensions::UiNavItem;
    use crate::icons::LuKeyRound;

    #[test]
    fn core_nav_is_unchanged_with_no_extensions() {
        let base: Vec<&str> = nav_model(&[]).into_iter().map(|entry| entry.path).collect();
        assert_eq!(
            base,
            [
                "/",
                "/effects",
                "/studio",
                "/media",
                "/devices",
                "/settings"
            ]
        );
    }

    #[test]
    fn shortcuts_match_sidebar_order() {
        assert_eq!(nav_shortcut_path(&[], "1").as_deref(), Some("/"));
        assert_eq!(nav_shortcut_path(&[], "3").as_deref(), Some("/studio"));
        assert_eq!(nav_shortcut_path(&[], "6").as_deref(), Some("/settings"));
        assert_eq!(nav_shortcut_path(&[], "7"), None);
        assert_eq!(nav_shortcut_path(&[], "0"), None);
        assert_eq!(nav_shortcut_path(&[], "x"), None);
    }

    #[test]
    fn extension_item_with_shortcut_gets_the_next_slot() {
        let extra = vec![UiNavItem::new("/account", "Account", LuKeyRound)];
        let model = nav_model(&extra);
        assert_eq!(model.last().map(|entry| entry.path), Some("/account"));
        assert_eq!(nav_shortcut_path(&extra, "7").as_deref(), Some("/account"));
    }

    #[test]
    fn click_only_extension_item_does_not_take_a_shortcut_slot() {
        let extra = vec![UiNavItem::new("/account", "Account", LuKeyRound).without_shortcut()];
        let model = nav_model(&extra);
        assert_eq!(model.last().map(|entry| entry.path), Some("/account"));
        // Core numbering is undisturbed; the extension item is click-only.
        assert_eq!(nav_shortcut_path(&extra, "7"), None);
        assert_eq!(nav_shortcut_path(&extra, "6").as_deref(), Some("/settings"));
    }
}
