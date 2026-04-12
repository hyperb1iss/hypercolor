/// Canvas resolution presets exposed by the settings UI.
pub const CANVAS_PRESETS: &[(&str, u32, u32)] = &[
    ("320x200", 320, 200),
    ("480x320", 480, 320),
    ("640x400", 640, 400),
    ("640x480", 640, 480),
    ("800x600", 800, 600),
    ("1024x768", 1024, 768),
    ("1280x720", 1280, 720),
    ("1280x800", 1280, 800),
    ("1280x960", 1280, 960),
    ("1280x1024", 1280, 1024),
    ("1600x900", 1600, 900),
    ("1600x1200", 1600, 1200),
    ("1920x1080", 1920, 1080),
    ("1920x1200", 1920, 1200),
    ("2560x1440", 2560, 1440),
    ("2560x1600", 2560, 1600),
    ("3440x1440", 3440, 1440),
    ("3840x2160", 3840, 2160),
];

/// Upper bounds for manual canvas entry in the settings UI.
pub const MAX_CUSTOM_CANVAS_WIDTH: f64 = 3840.0;
pub const MAX_CUSTOM_CANVAS_HEIGHT: f64 = 2160.0;

pub fn canvas_preset_key(width: u32, height: u32) -> String {
    for (label, w, h) in CANVAS_PRESETS {
        if *w == width && *h == height {
            return (*label).to_string();
        }
    }
    "custom".to_string()
}
