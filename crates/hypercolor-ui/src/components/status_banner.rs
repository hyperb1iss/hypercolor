//! Shared status banner — the tokenized warning/error strip shown under a
//! page header (named-scene warnings, degraded-effect notices).
//!
//! Replaces the hand-painted `rgba(241, 250, 140, …)` / `rgba(255, 99, 99, …)`
//! banners that Effects and Displays duplicated wholesale. Color rides the
//! `--status-warning` / `--status-error` semantic tokens plus the `.accent-*`
//! `--glow-rgb` mechanism for the soft outer glow, so both themes stay
//! correct without raw triplets in component source.

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::icons::LuTriangleAlert;

/// Visual tone of a [`StatusBanner`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusBannerTone {
    /// Yellow — cautionary state the user may want to undo (named scene
    /// active, snapshot lock).
    Warning,
    /// Red — something already went wrong (degraded effect/face).
    Error,
}

impl StatusBannerTone {
    /// Container classes: tokenized border/background plus the `--glow-rgb`
    /// accent class driving the outer glow.
    #[must_use]
    pub fn container_class(self) -> &'static str {
        match self {
            Self::Warning => {
                "accent-yellow rounded-xl border border-status-warning/24 bg-status-warning/8 \
                 px-4 py-3 shadow-[0_0_24px_rgba(var(--glow-rgb),0.08)]"
            }
            Self::Error => {
                "accent-red rounded-xl border border-status-error/28 bg-status-error/10 \
                 px-4 py-3 shadow-[0_0_24px_rgba(var(--glow-rgb),0.10)]"
            }
        }
    }

    /// Leading icon color class.
    #[must_use]
    pub fn icon_class(self) -> &'static str {
        match self {
            Self::Warning => "mt-0.5 shrink-0 text-status-warning/90",
            Self::Error => "mt-0.5 shrink-0 text-status-error/95",
        }
    }

    /// Uppercase kicker color class.
    #[must_use]
    pub fn title_class(self) -> &'static str {
        match self {
            Self::Warning => {
                "text-[11px] font-semibold uppercase tracking-[0.16em] text-status-warning/82"
            }
            Self::Error => {
                "text-[11px] font-semibold uppercase tracking-[0.16em] text-status-error/85"
            }
        }
    }
}

/// Tokenized warning/info banner: icon, uppercase kicker, emphasized subject
/// + detail body, and an optional trailing action passed as children.
#[component]
pub fn StatusBanner(
    /// Visual tone (warning yellow / error red).
    tone: StatusBannerTone,
    /// Short uppercase kicker line.
    #[prop(into)]
    title: String,
    /// Emphasized subject opening the body line.
    #[prop(into)]
    subject: String,
    /// Body text rendered immediately after the subject (include any
    /// leading space/punctuation).
    #[prop(into)]
    detail: String,
    /// Optional trailing action (e.g. a "Return to Default" button).
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class=tone.container_class()>
            <div class="flex items-start gap-3">
                <div class=tone.icon_class()>
                    <Icon icon=LuTriangleAlert width="14px" height="14px" />
                </div>
                <div class="min-w-0 flex-1">
                    <div class=tone.title_class()>{title}</div>
                    <div class="mt-1 text-sm leading-5 text-fg-secondary">
                        <span class="text-fg-primary">{subject}</span>
                        {detail}
                    </div>
                </div>
                {children.map(|render| render())}
            </div>
        </div>
    }
}
