//! TUI views — top-level screen components.

mod dashboard;
mod effect_browser;
mod effect_control;

pub use dashboard::DashboardView;
pub use effect_browser::EffectBrowserView;
pub use effect_control::EffectControlView;

use crate::component::Component;
use crate::screen::ScreenId;

/// Instantiate all built-in screen views.
///
/// Returns a vec of `(ScreenId, Component)` pairs that the app shell mounts
/// into its screen map.
#[must_use]
pub fn create_screens() -> Vec<(ScreenId, Box<dyn Component>)> {
    let screens: Vec<(ScreenId, Box<dyn Component>)> = vec![
        (ScreenId::Dashboard, Box::new(DashboardView::new())),
        (ScreenId::EffectBrowser, Box::new(EffectBrowserView::new())),
        (ScreenId::EffectControl, Box::new(EffectControlView::new())),
    ];
    screens
}
