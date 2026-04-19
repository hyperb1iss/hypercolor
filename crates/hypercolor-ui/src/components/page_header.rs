//! Unified page header — title row + toolbar row, fixed 104px on every page.
//!
//! Every top-level route uses `<PageHeader>`. The shape is identical across
//! pages so content below never shifts Y when the user navigates: 60px title
//! row + 44px toolbar row + 1px bottom border. The title row holds the icon,
//! title, tagline, and an optional trailing slot for page-level actions. The
//! toolbar row is always rendered; callers fill it with search, tabs, or a
//! context strip.
//!
//! Accents are chosen from a fixed palette of five SilkCircuit tokens plus a
//! spectrum gradient for the Dashboard; each page gets a distinct identity.

use icondata_core::Icon as IconData;
use leptos::prelude::*;
use leptos_icons::Icon;

/// Per-page identity. Drives the icon color and title gradient.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageAccent {
    /// Electric purple — brand/creative (Effects).
    Purple,
    /// Neon cyan — hardware/live state (Devices).
    Cyan,
    /// Coral — spatial/design (Layout).
    Coral,
    /// Success green — live rendering output (Displays).
    Green,
    /// Electric yellow — configuration/caution (Settings).
    Yellow,
    /// Cyan→purple→green rainbow — the home/overview (Dashboard).
    Spectrum,
}

impl PageAccent {
    fn icon_rgb(self) -> &'static str {
        match self {
            PageAccent::Purple => "225, 53, 255",
            PageAccent::Cyan | PageAccent::Spectrum => "128, 255, 234",
            PageAccent::Coral => "255, 106, 193",
            PageAccent::Green => "80, 250, 123",
            PageAccent::Yellow => "241, 250, 140",
        }
    }

    fn title_gradient(self) -> &'static str {
        match self {
            PageAccent::Purple => {
                "linear-gradient(105deg,#80ffea 0%,#c8d4ff 48%,#e135ff 100%)"
            }
            PageAccent::Cyan => {
                "linear-gradient(105deg,#80ffea 0%,#e8f4ff 55%,#80ffea 100%)"
            }
            PageAccent::Coral => {
                "linear-gradient(105deg,#80ffea 0%,#e8d4ff 50%,#ff6ac1 100%)"
            }
            PageAccent::Green => {
                "linear-gradient(105deg,#80ffea 0%,#d4eaff 50%,#50fa7b 100%)"
            }
            PageAccent::Yellow => {
                "linear-gradient(105deg,#80ffea 0%,#e8f0ff 50%,#f1fa8c 100%)"
            }
            PageAccent::Spectrum => {
                "linear-gradient(105deg,#80ffea 0%,#e135ff 52%,#50fa7b 100%)"
            }
        }
    }
}

/// Slot for right-aligned content in the title row (status pills, action
/// buttons, counts).
#[slot]
pub struct HeaderTrailing {
    children: Children,
}

/// Slot for the toolbar row (search, tabs, context strip). When absent the
/// row still renders at 44px so every page's content area starts at the same
/// Y coordinate.
#[slot]
pub struct HeaderToolbar {
    children: Children,
}

#[component]
pub fn PageHeader(
    icon: IconData,
    #[prop(into)] title: String,
    #[prop(into)] tagline: String,
    accent: PageAccent,
    #[prop(optional)] header_trailing: Option<HeaderTrailing>,
    #[prop(optional)] header_toolbar: Option<HeaderToolbar>,
) -> impl IntoView {
    let icon_rgb = accent.icon_rgb();
    let gradient = accent.title_gradient();
    let icon_style = format!(
        "color: rgb({icon_rgb}); filter: drop-shadow(0 0 10px rgba({icon_rgb}, 0.55))"
    );
    let title_style = format!(
        "font-family:'Orbitron',sans-serif; font-weight:900; font-size:20px; \
         letter-spacing:-0.01em; line-height:1; background-image:{gradient}"
    );

    view! {
        <header class="page-header sticky top-0 z-30 shrink-0 glass-subtle border-b border-edge-default">
            <div class="h-[60px] px-6 flex items-center justify-between gap-4">
                <div class="min-w-0 flex items-center gap-3">
                    <span class="shrink-0" style=icon_style>
                        <Icon icon=icon width="20px" height="20px" />
                    </span>
                    <div class="min-w-0 flex flex-col gap-1.5">
                        <h1 class="logo-gradient-text" style=title_style>
                            {title}
                        </h1>
                        <p class="text-[11.5px] leading-none text-fg-tertiary/72 truncate max-w-2xl">
                            {tagline}
                        </p>
                    </div>
                </div>
                <div class="flex items-center gap-3 shrink-0">
                    {header_trailing.map(|t| (t.children)())}
                </div>
            </div>

            <div class="h-[44px] px-6 flex items-center gap-3 border-t border-edge-subtle/40">
                {header_toolbar.map(|t| (t.children)())}
            </div>
        </header>
    }
}
