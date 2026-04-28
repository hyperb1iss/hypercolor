use hypercolor_tui::chrome::{StatusBar, StatusBarHit};
use hypercolor_tui::screen::ScreenId;
use ratatui::layout::Rect;

fn hit_at(col: u16, show_donate: bool) -> Option<StatusBarHit> {
    StatusBar::hit_test(
        Rect::new(0, 0, 120, 1),
        col,
        0,
        ScreenId::Dashboard,
        &[ScreenId::Dashboard, ScreenId::EffectBrowser],
        show_donate,
    )
}

#[test]
fn status_bar_sponsor_region_is_clickable_when_visible() {
    let sponsor_col = (0..120)
        .find(|col| hit_at(*col, true) == Some(StatusBarHit::Sponsor))
        .expect("sponsor hit region should exist");

    assert_eq!(hit_at(sponsor_col, true), Some(StatusBarHit::Sponsor));
    assert_ne!(hit_at(sponsor_col, false), Some(StatusBarHit::Sponsor));
}

#[test]
fn status_bar_screen_and_help_regions_are_clickable() {
    assert!(
        (0..120).any(|col| {
            hit_at(col, true) == Some(StatusBarHit::Screen(ScreenId::EffectBrowser))
        })
    );
    assert!((0..120).any(|col| hit_at(col, true) == Some(StatusBarHit::Help)));
}
