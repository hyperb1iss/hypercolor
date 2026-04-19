//! Unified small-caps label system.
//!
//! Replaces ~11 bespoke treatments (`text-[8px]` through `text-xs` with
//! various trackings, weights, and colors) with three deliberate sizes and
//! three tones. Every uppercase label in the UI should reach for one of these
//! size/tone combinations rather than handcrafting its own tracking/opacity.
//!
//! Use the `<SectionLabel>` component when you want icon + text laid out
//! together. For raw inline use inside a button/chip/badge, call
//! [`label_class`] and splat the returned string onto the existing element.

use icondata_core::Icon as IconData;
use leptos::prelude::*;
use leptos_icons::Icon;

/// Type-scale tier for an uppercase label. Pick based on the density of
/// surrounding content, not the importance of the label itself (tone picks
/// that).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelSize {
    /// 9px mono — chart axes, status-pill sublabels, dense datum rows.
    Micro,
    /// 11px mono — card metadata, filter-dropdown section titles, panel
    /// sub-titles. The everyday default.
    Small,
    /// 12px mono — canonical panel section headers (settings "Audio",
    /// "Network", etc). Use sparingly — one per major section.
    Section,
}

/// Opacity/weight variant for an uppercase label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelTone {
    /// fg-tertiary/50 — dropdown section titles, deeply subdued chrome.
    Subtle,
    /// fg-tertiary/80 — default section headers, metadata field names.
    Default,
    /// fg-secondary (semibold) — sub-panel titles, emphasis within a card.
    Strong,
}

/// Tailwind class string for a given size + tone. Every label in the UI
/// should compose through this function so tracking/opacity/weight stay
/// consistent even for inline uses that can't render the component.
#[must_use]
pub const fn label_class(size: LabelSize, tone: LabelTone) -> &'static str {
    match (size, tone) {
        // Micro (9px)
        (LabelSize::Micro, LabelTone::Subtle) => {
            "text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary/50"
        }
        (LabelSize::Micro, LabelTone::Default) => {
            "text-[9px] font-mono uppercase tracking-[0.14em] text-fg-tertiary"
        }
        (LabelSize::Micro, LabelTone::Strong) => {
            "text-[9px] font-mono uppercase tracking-[0.14em] font-semibold text-fg-secondary"
        }

        // Small (11px)
        (LabelSize::Small, LabelTone::Subtle) => {
            "text-[11px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/50"
        }
        (LabelSize::Small, LabelTone::Default) => {
            "text-[11px] font-mono uppercase tracking-[0.12em] text-fg-tertiary/80"
        }
        (LabelSize::Small, LabelTone::Strong) => {
            "text-[11px] font-mono uppercase tracking-[0.14em] font-semibold text-fg-secondary"
        }

        // Section (12px)
        (LabelSize::Section, LabelTone::Subtle) => {
            "text-xs font-mono uppercase tracking-[0.12em] text-fg-tertiary/60"
        }
        (LabelSize::Section, LabelTone::Default) => {
            "text-xs font-mono uppercase tracking-[0.12em] text-fg-tertiary/80"
        }
        (LabelSize::Section, LabelTone::Strong) => {
            "text-xs font-mono uppercase tracking-[0.14em] font-semibold text-fg-secondary"
        }
    }
}

/// Uppercase label with an optional leading icon. The icon inherits the
/// label color unless `icon_color` is supplied.
#[component]
pub fn SectionLabel(
    #[prop(into)] text: String,
    #[prop(default = LabelSize::Small)] size: LabelSize,
    #[prop(default = LabelTone::Default)] tone: LabelTone,
    #[prop(optional)] icon: Option<IconData>,
    /// Override the icon color with a custom CSS color string. When absent,
    /// the icon inherits the label's tone.
    #[prop(optional, into)]
    icon_color: Option<String>,
    /// Margin class applied to the wrapper (default `mb-2`). Pass `""` to
    /// suppress when the label sits inside a flex row that handles spacing.
    #[prop(default = "mb-2")]
    margin: &'static str,
) -> impl IntoView {
    let text_class = label_class(size, tone);
    let wrapper_class = if margin.is_empty() {
        "flex items-center gap-1.5".to_string()
    } else {
        format!("flex items-center gap-1.5 {margin}")
    };
    let icon_size = match size {
        LabelSize::Micro => "11px",
        LabelSize::Small => "12px",
        LabelSize::Section => "14px",
    };

    view! {
        <div class=wrapper_class>
            {icon.map(|icon_data| {
                let style = icon_color
                    .map(|color| format!("color: {color}"))
                    .unwrap_or_default();
                view! {
                    <span class="shrink-0" style=style>
                        <Icon icon=icon_data width=icon_size height=icon_size />
                    </span>
                }
            })}
            <span class=text_class>{text}</span>
        </div>
    }
}
