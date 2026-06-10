use hypercolor_ui::components::shell::nav_shortcut_path;

#[test]
fn base_nav_set_maps_digits_in_sidebar_order() {
    assert_eq!(nav_shortcut_path(false, "1"), Some("/"));
    assert_eq!(nav_shortcut_path(false, "2"), Some("/effects"));
    assert_eq!(nav_shortcut_path(false, "3"), Some("/assets"));
    assert_eq!(nav_shortcut_path(false, "4"), Some("/layout"));
    assert_eq!(nav_shortcut_path(false, "5"), Some("/devices"));
    assert_eq!(nav_shortcut_path(false, "6"), Some("/displays"));
    assert_eq!(nav_shortcut_path(false, "7"), Some("/settings"));
}

#[test]
fn studio_nav_set_swaps_studio_and_media_in() {
    assert_eq!(nav_shortcut_path(true, "1"), Some("/"));
    assert_eq!(nav_shortcut_path(true, "2"), Some("/effects"));
    assert_eq!(nav_shortcut_path(true, "3"), Some("/studio"));
    assert_eq!(nav_shortcut_path(true, "4"), Some("/media"));
    assert_eq!(nav_shortcut_path(true, "5"), Some("/devices"));
    assert_eq!(nav_shortcut_path(true, "6"), Some("/settings"));
}

#[test]
fn out_of_range_digits_are_ignored() {
    assert_eq!(nav_shortcut_path(false, "0"), None);
    assert_eq!(nav_shortcut_path(false, "8"), None);
    assert_eq!(nav_shortcut_path(true, "7"), None);
    assert_eq!(nav_shortcut_path(true, "9"), None);
}

#[test]
fn non_digit_keys_are_ignored() {
    assert_eq!(nav_shortcut_path(false, "k"), None);
    assert_eq!(nav_shortcut_path(false, "Escape"), None);
    assert_eq!(nav_shortcut_path(false, ""), None);
    assert_eq!(nav_shortcut_path(false, "11"), None);
    assert_eq!(nav_shortcut_path(false, "-1"), None);
}
