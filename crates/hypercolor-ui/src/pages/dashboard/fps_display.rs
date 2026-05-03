const TARGET_LOCK_FRACTION: f64 = 0.05;
const TARGET_LOCK_MIN_FPS: f64 = 1.0;

pub(crate) fn stabilize_fps_for_display(raw_fps: f64, target_fps: u32) -> f64 {
    if !raw_fps.is_finite() || raw_fps <= 0.0 {
        return 0.0;
    }

    if target_fps == 0 {
        return raw_fps;
    }

    let target = f64::from(target_fps);
    let tolerance = (target * TARGET_LOCK_FRACTION).max(TARGET_LOCK_MIN_FPS);
    if (raw_fps - target).abs() <= tolerance {
        target
    } else {
        raw_fps
    }
}

pub(crate) fn stabilize_fps_for_display_f32(raw_fps: f32, target_fps: u32) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    {
        stabilize_fps_for_display(f64::from(raw_fps), target_fps) as f32
    }
}
