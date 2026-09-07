use hypercolor_ui::route_ui::{NowPlayingCanvasMode, now_playing_canvas_mode};

#[test]
fn mounts_keep_navigation_assets_and_relative_state_separate() {
    let mount =
        hypercolor_ui::UiMount::new("/embedded/device/", "/bundles/").expect("valid mounts");
    assert_eq!(mount.route_base(), "/embedded/device");
    assert_eq!(mount.route_href("/"), "/embedded/device/");
    assert_eq!(
        mount.route_href("/devices?tab=all#selected"),
        "/embedded/device/devices?tab=all#selected"
    );
    assert_eq!(
        mount.asset_href("/assets/brand/mark-color.png"),
        "/bundles/assets/brand/mark-color.png"
    );
    for root in ["/embedded/device", "/embedded/device/"] {
        assert_eq!(mount.relative_path(root), Some("/"));
        assert!(mount.route_is_active(root, "/"));
    }
    assert_eq!(
        mount.relative_path("/embedded/device/studio"),
        Some("/studio")
    );
    assert_eq!(mount.relative_path("/embedded/device-other/studio"), None);
    assert!(!mount.route_is_active("/embedded/device-other/devices", "/devices"));
    assert!(mount.route_is_active("/embedded/device/devices/one", "/devices"));
    assert!(!mount.route_is_active("/embedded/device/devices-other", "/devices"));
}

#[test]
fn root_mount_preserves_existing_urls() {
    for mount in [
        hypercolor_ui::UiMount::default(),
        hypercolor_ui::UiMount::new("/", "/").expect("root"),
    ] {
        assert_eq!(mount.route_base(), "");
        assert_eq!(mount.route_href("/devices"), "/devices");
        assert_eq!(
            mount.asset_href("/assets/vendors/wled.png"),
            "/assets/vendors/wled.png"
        );
        assert_eq!(mount.relative_path("/studio"), Some("/studio"));
    }
}

#[test]
fn mounted_display_preview_link_keeps_the_selected_display_query() {
    use hypercolor_ui::display_utils::display_preview_shell_url;
    use hypercolor_ui::route_ui::route_href;
    use leptos::prelude::{Owner, provide_context};

    let path = display_preview_shell_url("display-123");
    assert_eq!(path, "/preview?display=display-123");
    let owner = Owner::new();
    owner.with(|| {
        assert_eq!(route_href(&path), "/preview?display=display-123");
        provide_context(
            hypercolor_ui::UiMount::new("/embedded/device", "/bundles").expect("valid mounts"),
        );
        assert_eq!(
            route_href(&path),
            "/embedded/device/preview?display=display-123"
        );
        assert_eq!(path, "/preview?display=display-123");
    });
}

#[test]
fn invalid_mounts_cannot_escape_the_origin_or_change_url_meaning() {
    for invalid in [
        "relative",
        "//other.test",
        "/a//b",
        "/a/../b",
        "/a/./b",
        "/a?query",
        "/a#fragment",
        "/%2fother",
        "/a\\b",
        "https://other.test",
    ] {
        assert!(
            hypercolor_ui::UiMount::new(invalid, "").is_err(),
            "{invalid}"
        );
        assert!(
            hypercolor_ui::UiMount::new("", invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn contextual_helpers_use_the_mount_without_changing_imperative_route_inputs() {
    use hypercolor_ui::route_ui::{asset_href, mounted_canvas_mode, route_href, route_is_active};
    use leptos::prelude::{Owner, provide_context};
    let owner = Owner::new();
    owner.with(|| {
        assert_eq!(route_href("/devices"), "/devices");
        assert_eq!(asset_href("/assets/mark.png"), "/assets/mark.png");
        provide_context(
            hypercolor_ui::UiMount::new("/embedded/device", "/bundles").expect("valid mounts"),
        );
        assert_eq!(route_href("/devices"), "/embedded/device/devices");
        assert_eq!(asset_href("/assets/mark.png"), "/bundles/assets/mark.png");
        assert!(route_is_active("/embedded/device/devices", "/devices"));
        assert_eq!(
            mounted_canvas_mode("/embedded/device/studio"),
            NowPlayingCanvasMode::Palette
        );
        assert_eq!(
            mounted_canvas_mode("/embedded/device/devices"),
            NowPlayingCanvasMode::Preview
        );
        let core = hypercolor_ui::nav_model(&[]);
        assert!(core.iter().all(|item| !item.path.starts_with("/embedded")));
        assert_eq!(
            hypercolor_ui::UiExtensions::default().mount,
            hypercolor_ui::UiMount::default()
        );
    });
}

#[test]
fn home_and_effect_routes_use_live_palette_mode() {
    assert_eq!(now_playing_canvas_mode("/"), NowPlayingCanvasMode::Palette);
    assert_eq!(
        now_playing_canvas_mode("/effects"),
        NowPlayingCanvasMode::Palette
    );
    assert_eq!(
        now_playing_canvas_mode("/effects/pulse-temp"),
        NowPlayingCanvasMode::Palette
    );
}

#[test]
fn studio_routes_use_live_palette_mode() {
    // Studio mounts its own Stage preview, so the sidebar must drop its
    // duplicate live canvas.
    assert_eq!(
        now_playing_canvas_mode("/studio"),
        NowPlayingCanvasMode::Palette
    );
    assert_eq!(
        now_playing_canvas_mode("/studio/output"),
        NowPlayingCanvasMode::Palette
    );
}

#[test]
fn remaining_shell_routes_keep_sidebar_preview_mode() {
    assert_eq!(
        now_playing_canvas_mode("/devices"),
        NowPlayingCanvasMode::Preview
    );
    assert_eq!(
        now_playing_canvas_mode("/media"),
        NowPlayingCanvasMode::Preview
    );
    assert_eq!(
        now_playing_canvas_mode("/settings"),
        NowPlayingCanvasMode::Preview
    );
}
