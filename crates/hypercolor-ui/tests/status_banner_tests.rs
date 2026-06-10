use hypercolor_ui::components::status_banner::StatusBannerTone;

#[test]
fn warning_tone_rides_status_warning_tokens() {
    let container = StatusBannerTone::Warning.container_class();
    assert!(container.contains("border-status-warning"));
    assert!(container.contains("bg-status-warning"));
    assert!(container.contains("accent-yellow"));
    assert!(
        StatusBannerTone::Warning
            .icon_class()
            .contains("text-status-warning")
    );
    assert!(
        StatusBannerTone::Warning
            .title_class()
            .contains("text-status-warning")
    );
}

#[test]
fn error_tone_rides_status_error_tokens() {
    let container = StatusBannerTone::Error.container_class();
    assert!(container.contains("border-status-error"));
    assert!(container.contains("bg-status-error"));
    assert!(container.contains("accent-red"));
    assert!(
        StatusBannerTone::Error
            .icon_class()
            .contains("text-status-error")
    );
    assert!(
        StatusBannerTone::Error
            .title_class()
            .contains("text-status-error")
    );
}

#[test]
fn tones_never_hand_paint_raw_color() {
    for tone in [StatusBannerTone::Warning, StatusBannerTone::Error] {
        for class in [
            tone.container_class(),
            tone.icon_class(),
            tone.title_class(),
        ] {
            assert!(!class.contains("rgba(241"), "raw yellow literal in {class}");
            assert!(!class.contains("rgba(255"), "raw red literal in {class}");
            assert!(!class.contains('#'), "raw hex literal in {class}");
        }
    }
}
