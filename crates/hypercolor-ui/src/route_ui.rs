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
