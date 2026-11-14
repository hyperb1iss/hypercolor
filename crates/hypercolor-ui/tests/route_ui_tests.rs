use hypercolor_ui::pages::effects::phone_view::PhoneView;
use hypercolor_ui::route_ui::{
    NowPlayingCanvasMode, now_playing_canvas_mode, page_has_own_preview,
};

/// An application address split into its path and `view` query value.
fn path_and_view(href: &str) -> (&str, Option<&str>) {
    match href.split_once("?view=") {
        Some((path, view)) => (path, Some(view)),
        None => (href, None),
    }
}

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
fn relocated_routes_keep_the_application_asset_base() {
    let daemon_routes =
        hypercolor_ui::UiMount::new("/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1", "")
            .expect("valid route mount");
    let deployed =
        hypercolor_ui::UiMount::new("", "/remote-app/versions/0.6.0-beta.7").expect("valid");

    let bridged = deployed.with_routes_from(&daemon_routes);
    assert_eq!(
        bridged.route_base(),
        "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1"
    );
    assert_eq!(
        bridged.route_href("/devices"),
        "/remote/018f4c36-4a44-7cc9-9f57-0d2e9224d2f1/devices"
    );
    assert_eq!(
        bridged.asset_href("/assets/brand/mark-color.png"),
        "/remote-app/versions/0.6.0-beta.7/assets/brand/mark-color.png"
    );

    let standalone = hypercolor_ui::UiMount::default().with_routes_from(&daemon_routes);
    assert_eq!(
        standalone.asset_href("/assets/brand/mark-color.png"),
        "/assets/brand/mark-color.png"
    );
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
            mounted_canvas_mode("/embedded/device/studio", None, false),
            NowPlayingCanvasMode::Palette
        );
        assert_eq!(
            mounted_canvas_mode("/embedded/device/devices", None, false),
            NowPlayingCanvasMode::Preview
        );
        assert_eq!(
            mounted_canvas_mode("/embedded/device/effects", None, true),
            NowPlayingCanvasMode::Preview
        );
        assert_eq!(
            mounted_canvas_mode("/embedded/device/effects", Some("controls"), true),
            NowPlayingCanvasMode::Palette
        );
        assert_eq!(
            mounted_canvas_mode("/embedded/device/effects", None, false),
            NowPlayingCanvasMode::Palette
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
    assert_eq!(
        now_playing_canvas_mode("/", None, false),
        NowPlayingCanvasMode::Palette
    );
    assert_eq!(
        now_playing_canvas_mode("/effects", None, false),
        NowPlayingCanvasMode::Palette
    );
    assert_eq!(
        now_playing_canvas_mode("/effects/pulse-temp", None, false),
        NowPlayingCanvasMode::Palette
    );
}

#[test]
fn studio_routes_use_live_palette_mode() {
    // Studio mounts its own Stage preview, so the sidebar must drop its
    // duplicate live canvas.
    assert_eq!(
        now_playing_canvas_mode("/studio", None, false),
        NowPlayingCanvasMode::Palette
    );
    assert_eq!(
        now_playing_canvas_mode("/studio/output", None, false),
        NowPlayingCanvasMode::Palette
    );
}

#[test]
fn remaining_shell_routes_keep_sidebar_preview_mode() {
    assert_eq!(
        now_playing_canvas_mode("/devices", None, false),
        NowPlayingCanvasMode::Preview
    );
    assert_eq!(
        now_playing_canvas_mode("/media", None, false),
        NowPlayingCanvasMode::Preview
    );
    assert_eq!(
        now_playing_canvas_mode("/settings", None, false),
        NowPlayingCanvasMode::Preview
    );
}

#[test]
fn pages_with_their_own_preview_carry_a_zone_picker() {
    // The dashboard and effects cabinets carry zone tabs, and Studio its
    // zone list, so their previews follow the focused zone.
    for path in [
        "/",
        "/effects",
        "/effects/pulse-temp",
        "/studio",
        "/studio/output",
    ] {
        assert!(page_has_own_preview(path, None, false), "{path}");
    }
    for path in ["/devices", "/media", "/settings", "/layouts"] {
        assert!(!page_has_own_preview(path, None, false), "{path}");
    }
}

#[test]
fn phone_effects_browse_leaves_the_live_canvas_to_the_player() {
    // A phone mounts the effects cabinet only in Controls, so Browse
    // shows no preview of its own and the strip shows the live canvas.
    for path in ["/effects", "/effects/pulse-temp"] {
        for view in [None, Some(""), Some("grid")] {
            assert!(!page_has_own_preview(path, view, true), "{path} {view:?}");
            assert_eq!(
                now_playing_canvas_mode(path, view, true),
                NowPlayingCanvasMode::Preview,
                "{path} {view:?}"
            );
        }
        assert!(page_has_own_preview(path, Some("controls"), true), "{path}");
        assert_eq!(
            now_playing_canvas_mode(path, Some("controls"), true),
            NowPlayingCanvasMode::Palette,
            "{path}"
        );
    }
}

#[test]
fn each_phone_effects_address_picks_its_player_thumbnail() {
    // The addresses the view switch and the player sheet navigate to.
    for (view, expected) in [
        (PhoneView::Browse, NowPlayingCanvasMode::Preview),
        (PhoneView::Controls, NowPlayingCanvasMode::Palette),
    ] {
        let (path, value) = path_and_view(view.href());
        assert_eq!(
            now_playing_canvas_mode(path, value, true),
            expected,
            "{view:?}"
        );
    }
}

#[test]
fn desktop_effects_keeps_its_own_preview_in_every_view() {
    // Wider layouts show the grid and the cabinet side by side and ignore
    // the view, so the sidebar keeps its palette swatch.
    for path in ["/effects", "/effects/pulse-temp"] {
        for view in [None, Some("controls"), Some("grid")] {
            assert!(page_has_own_preview(path, view, false), "{path} {view:?}");
            assert_eq!(
                now_playing_canvas_mode(path, view, false),
                NowPlayingCanvasMode::Palette,
                "{path} {view:?}"
            );
        }
    }
}

#[test]
fn the_phone_layout_moves_only_effects_browse() {
    // Studio's own phone views share the `view` parameter; only Effects
    // reads it.
    for path in ["/", "/studio", "/studio/output"] {
        for view in [None, Some("controls"), Some("editor")] {
            assert!(page_has_own_preview(path, view, true), "{path} {view:?}");
        }
    }
    for path in ["/devices", "/media", "/settings", "/layouts"] {
        for view in [None, Some("controls")] {
            assert!(!page_has_own_preview(path, view, true), "{path} {view:?}");
        }
    }
}
