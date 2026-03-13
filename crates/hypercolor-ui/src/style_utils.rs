//! Shared category styling — accent colors and badge classes used across
//! effect cards, the sidebar, dashboard, and effects page.

/// Category -> (badge Tailwind classes, accent RGB triplet for inline styles).
pub fn category_style(category: &str) -> (&'static str, &'static str) {
    match category {
        "ambient" => ("bg-neon-cyan/10 text-neon-cyan", "128, 255, 234"),
        "audio" => ("bg-coral/10 text-coral", "255, 106, 193"),
        "gaming" => ("bg-electric-purple/10 text-electric-purple", "225, 53, 255"),
        "reactive" => (
            "bg-electric-yellow/10 text-electric-yellow",
            "241, 250, 140",
        ),
        "generative" => ("bg-success-green/10 text-success-green", "80, 250, 123"),
        "interactive" => ("bg-info-blue/10 text-info-blue", "130, 170, 255"),
        "productivity" => ("bg-pink-soft/10 text-pink-soft", "255, 153, 255"),
        "utility" => ("bg-fg-tertiary/10 text-fg-tertiary", "139, 133, 160"),
        _ => ("bg-surface-overlay/50 text-fg-tertiary", "139, 133, 160"),
    }
}

/// Category -> accent RGB string for inline styles.
pub fn category_accent_rgb(category: &str) -> &'static str {
    category_style(category).1
}
