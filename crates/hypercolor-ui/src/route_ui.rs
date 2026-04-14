#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NowPlayingCanvasMode {
    Preview,
    Palette,
    Disabled,
}

pub fn now_playing_canvas_mode(path: &str) -> NowPlayingCanvasMode {
    if path == "/" || path.starts_with("/effects") || path.starts_with("/layout") {
        NowPlayingCanvasMode::Palette
    } else if path.starts_with("/displays") {
        NowPlayingCanvasMode::Disabled
    } else {
        NowPlayingCanvasMode::Preview
    }
}
