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
    /// Render only in the mobile bottom bar, not the desktop sidebar.
    pub mobile_only: bool,
}

impl NavEntry {
    const fn core(path: &'static str, label: &'static str, icon: icondata_core::Icon) -> Self {
        Self {
            path,
            label,
            icon,
            divider_before: false,
            shortcut: true,
            mobile_only: false,
        }
    }

    fn from_extension(item: UiNavItem) -> Self {
        Self {
            path: item.path,
            label: item.label,
            icon: item.icon,
            divider_before: item.divider_before,
            shortcut: item.shortcut,
            mobile_only: item.mobile_only,
        }
    }
}

/// Extension nav items provided through Leptos context. Plain `Send + Sync`
/// data, cloned into the model when the sidebar/shell ask for it. Absent
/// context (or an empty list) yields the core nav set unchanged.
#[derive(Clone, Default)]
pub struct NavExtensionItems(pub Vec<UiNavItem>);

/// The core nav set (Spec 65 §5.1). `Settings` always sits last.
fn core_nav() -> Vec<NavEntry> {
    use crate::icons::*;

    vec![
        NavEntry::core("/", "Dashboard", LuLayoutDashboard),
        NavEntry::core("/effects", "Effects", LuLayers),
        NavEntry::core("/studio", "Studio", LuLayoutTemplate),
        NavEntry::core("/media", "Media", LuImages),
        NavEntry::core("/devices", "Devices", LuCpu),
        NavEntry::core("/settings", "Settings", LuSettings),
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

/// The phone bottom bar's split of the nav set.
#[derive(Clone)]
pub struct PhoneNav {
    /// The core tabs, which always hold the bar in sidebar order.
    pub tabs: Vec<NavEntry>,
    /// Extension items, which live in the bar's More sheet so they never
    /// shrink the core tabs below a finger's width. Empty means no More tab.
    pub more: Vec<NavEntry>,
}

/// Split the nav set for the phone bottom bar: the core tabs, then every
/// extension item (including `mobile_only` ones) behind a More tab.
#[must_use]
pub fn phone_nav(extra: &[UiNavItem]) -> PhoneNav {
    PhoneNav {
        tabs: core_nav(),
        more: extra
            .iter()
            .cloned()
            .map(NavEntry::from_extension)
            .collect(),
    }
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
    use super::{nav_model, nav_shortcut_path, phone_nav};
    use crate::extensions::UiNavItem;
    use crate::icons::LuKeyRound;

    #[test]
    fn mobile_only_items_carry_the_flag_and_no_shortcut() {
        let extra = [UiNavItem::new("/account", "Account", LuKeyRound).mobile_only()];
        let entries = nav_model(&extra);
        let account = entries.last().unwrap();
        assert!(account.mobile_only);
        assert!(!account.shortcut);
        assert!(nav_shortcut_path(&extra, "7").is_none());
    }

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

    #[test]
    fn phone_nav_keeps_core_tabs_and_has_no_more_without_extensions() {
        let nav = phone_nav(&[]);
        let tabs: Vec<&str> = nav.tabs.iter().map(|entry| entry.path).collect();
        assert_eq!(
            tabs,
            [
                "/",
                "/effects",
                "/studio",
                "/media",
                "/devices",
                "/settings"
            ]
        );
        assert!(nav.more.is_empty());
    }

    #[test]
    fn phone_nav_moves_every_extension_item_into_more() {
        let extra = [
            UiNavItem::new("/account", "Account", LuKeyRound),
            UiNavItem::new("/remote", "Remote", LuKeyRound).mobile_only(),
        ];
        let nav = phone_nav(&extra);
        assert_eq!(nav.tabs.len(), 6);
        let more: Vec<&str> = nav.more.iter().map(|entry| entry.path).collect();
        assert_eq!(more, ["/account", "/remote"]);
        assert!(nav.more[1].mobile_only);
    }
}
