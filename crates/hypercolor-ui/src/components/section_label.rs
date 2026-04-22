//! Unified small-caps label system.
//!
//! Replaces ~11 bespoke treatments (`text-[8px]` through `text-xs` with
//! various trackings, weights, and colors) with three deliberate sizes and
//! three tones. Every uppercase label in the UI should reach for one of these
//! size/tone combinations rather than handcrafting its own tracking/opacity.

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
