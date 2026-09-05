//! Unified page header — title row + toolbar row, fixed 104px on every page.
//!
//! Every top-level route uses `<PageHeader>`. The shape is identical across
//! pages so content below never shifts Y when the user navigates: 60px title
//! row + 44px toolbar row. Elevation comes from a soft downward drop shadow
//! rather than a hairline border. The title row holds the icon, title, and an
//! optional trailing slot for page-level actions. The toolbar row is always
//! rendered; callers fill it with search, tabs, or a context strip.
//!
//! Accents are chosen from a fixed palette of six SilkCircuit tokens plus a
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
    /// Soft pink — the media library (Media).
    Pink,
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
            PageAccent::Pink => "255, 153, 255",
            PageAccent::Green => "80, 250, 123",
            PageAccent::Yellow => "241, 250, 140",
        }
    }

    /// CSS class carrying the accent's title gradient + glow. The gradient
    /// definitions live in `input.css` (`.page-title-*`) so the light theme
    /// can re-mix them without component logic.
    fn title_class(self) -> &'static str {
        match self {
            PageAccent::Purple => "page-title-purple",
            PageAccent::Cyan => "page-title-cyan",
            PageAccent::Coral => "page-title-coral",
            PageAccent::Pink => "page-title-pink",
            PageAccent::Green => "page-title-green",
            PageAccent::Yellow => "page-title-yellow",
            PageAccent::Spectrum => "page-title-spectrum",
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
    accent: PageAccent,
    #[prop(optional)] header_trailing: Option<HeaderTrailing>,
    #[prop(optional)] header_toolbar: Option<HeaderToolbar>,
) -> impl IntoView {
    let icon_rgb = accent.icon_rgb();
    let icon_style =
        format!("color: rgb({icon_rgb}); filter: drop-shadow(0 0 10px rgba({icon_rgb}, 0.55))");
    let title_class = format!("page-title {}", accent.title_class());

    view! {
        <header class="page-header sticky top-0 z-30 shrink-0 glass-subtle page-header-elevation">
            // Phone rows are 48px + 40px (88px total) versus the desktop
            // 60px + 44px; both are fixed so content never shifts Y on
            // navigation at either size. The title is shrink-proof on
            // phones; trailing content shrinks and pages diet it instead.
            <div class="h-12 md:h-[60px] px-4 md:px-6 flex items-center justify-between gap-3 md:gap-4">
                <div class="min-w-0 max-md:shrink-0 flex items-center gap-3 max-md:gap-2.5">
                    // The sidebar carries the brand on desktop; with it
                    // hidden below md, the mark anchors every page header.
                    <img
                        src=crate::route_ui::asset_href("/assets/brand/mark-color.png")
                        alt=""
                        class="md:hidden w-6 h-6 select-none shrink-0"
                        draggable="false"
                    />
                    <span class="shrink-0" style=icon_style>
                        <Icon icon=icon width="20px" height="20px" />
                    </span>
                    <div class="min-w-0 flex flex-col">
                        <h1 class=title_class>
                            {title}
                        </h1>
                    </div>
                </div>
                <div class="flex items-center gap-3 shrink-0 max-md:gap-2 max-md:min-w-0 max-md:shrink">
                    {header_trailing.map(|t| (t.children)())}
                </div>
            </div>

            // Overflow stays visible at every width: several pages hang
            // non-portaling dropdown panels off toolbar children, and any
            // overflow value here turns the row into a 44px clip box that
            // shreds them (spec 75 wave 2 covers the phone-width strategy).
            <div class="h-10 md:h-[44px] px-4 md:px-6 flex items-center gap-3">
                {header_toolbar.map(|t| (t.children)())}
            </div>
        </header>
    }
}
