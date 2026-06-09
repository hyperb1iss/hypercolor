use hypercolor_ui::route_ui::{NowPlayingCanvasMode, now_playing_canvas_mode};

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
    assert_eq!(
        now_playing_canvas_mode("/assets"),
        NowPlayingCanvasMode::Palette
    );
    assert_eq!(
        now_playing_canvas_mode("/layout"),
        NowPlayingCanvasMode::Palette
    );
}

#[test]
fn studio_routes_use_live_palette_mode() {
    // Studio mounts its own Stage preview, so the sidebar must drop its
    // duplicate live canvas the way /layout does.
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
fn displays_routes_disable_main_canvas_sidebar_features() {
    assert_eq!(
        now_playing_canvas_mode("/displays"),
        NowPlayingCanvasMode::Disabled
    );
    assert_eq!(
        now_playing_canvas_mode("/displays/preview-simulator"),
        NowPlayingCanvasMode::Disabled
    );
}

#[test]
fn remaining_shell_routes_keep_sidebar_preview_mode() {
    assert_eq!(
        now_playing_canvas_mode("/devices"),
        NowPlayingCanvasMode::Preview
    );
    assert_eq!(
        now_playing_canvas_mode("/settings"),
        NowPlayingCanvasMode::Preview
    );
}
