//! Shared styling utilities — color conversions, accent generation, badge
//! classes, and reusable UI primitives used across the app.

use leptos::prelude::*;

use hypercolor_color::Hsl;
use hypercolor_leptos_ext::prelude::random_unit;

/// Category -> (badge Tailwind classes, accent RGB triplet for inline styles).
pub fn category_style(category: &str) -> (&'static str, &'static str) {
    // One arm per EffectCategory variant. The daemon serializes the enum
    // in snake_case, so these are the only strings that ever arrive.
    match category {
        "ambient" => ("bg-cyan/10 text-cyan", "128, 255, 234"),
        "audio" => ("bg-coral/10 text-coral", "255, 106, 193"),
        "generative" => ("bg-status-success/10 text-status-success", "80, 250, 123"),
        "particle" => ("bg-accent/10 text-accent", "225, 53, 255"),
        "scenic" => ("bg-accent-hover/10 text-accent-hover", "255, 153, 255"),
        "interactive" => ("bg-status-info/10 text-status-info", "130, 170, 255"),
        "fun" => ("bg-purple-light/10 text-purple-light", "189, 0, 221"),
        "source" => (
            "bg-status-warning/10 text-status-warning",
            "241, 250, 140",
        ),
        "utility" => ("bg-fg-tertiary/10 text-fg-tertiary", "139, 133, 160"),
        "display" => ("bg-coral/10 text-coral", "255, 106, 193"),
        _ => ("bg-surface-overlay/50 text-fg-tertiary", "139, 133, 160"),
    }
}

/// Category -> accent RGB string for inline styles.
pub fn category_accent_rgb(category: &str) -> &'static str {
    category_style(category).1
}

/// Generate a short pseudo-random hex ID (suitable for zone IDs in the editor).
pub fn uuid_v4_hex() -> String {
    let r = random_unit();
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let n = (r * 4_294_967_295.0) as u32;
    format!("{n:08x}")
}

/// Generate unique primary + secondary accent colors for a device based on its ID.
///
/// Uses FNV-1a hash to pick a hue, then derives a complementary secondary
/// hue shifted 40° for a rich gradient effect.
pub fn device_accent_colors(device_id: &str) -> (String, String) {
    let mut hash: u32 = 2_166_136_261;
    for byte in device_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }

    #[allow(clippy::cast_possible_truncation)]
    let hue = (hash % 360) as f32;
    let secondary_hue = (hue + 40.0) % 360.0;

    let sat = 75.0 + (((hash >> 8) % 20) as f32);
    let lit = 62.0 + (((hash >> 16) % 12) as f32);

    let primary = hsl_to_rgb_string(hue, sat, lit);
    let secondary = hsl_to_rgb_string(secondary_hue, sat.min(90.0), lit + 4.0);
    (primary, secondary)
}

/// Convert HSL (h: 0–360, s: 0–100, l: 0–100) to an "r, g, b" string.
///
/// Percentages are this call site's units, so they are divided out here
/// and the conversion itself is the kernel's. The kernel wraps hue,
/// which fixes a latent bug: the old sector chain sent every hue at or
/// above 360 into the magenta arm.
fn hsl_to_rgb_string(h: f32, s: f32, l: f32) -> String {
    let rgb = Hsl::new(h, s / 100.0, l / 100.0).to_rgb();
    format!("{}, {}, {}", rgb.r, rgb.g, rgb.b)
}

// ── Shared UI primitives ────────────────────────────────────────────────────

/// Render a row of filter chips with active/inactive/hover states.
///
/// Each chip is a `(label, rgb)` pair. The `current` signal holds the active
/// label; clicking a chip updates it via `set_current`. The RGB triplet rides
/// `--glow-rgb` into the `.filter-chip-*` classes (input.css), which also own
/// the inactive hover treatment.
pub fn filter_chips(
    chips: &'static [(&'static str, &'static str)],
    current: ReadSignal<String>,
    set_current: WriteSignal<String>,
) -> impl IntoView {
    chips
        .iter()
        .map(|&(label, rgb)| {
            let is_active = Memo::new(move |_| current.get() == label);
            view! {
                <button
                    class="px-2 py-0.5 rounded-full text-[10px] font-medium capitalize border transition-all"
                    class=("filter-chip-active", is_active)
                    class=("filter-chip-inactive", move || !is_active.get())
                    style=("--glow-rgb", rgb)
                    on:click=move |_| set_current.set(label.to_string())
                >
                    {label}
                </button>
            }
        })
        .collect_view()
}

#[cfg(test)]
mod category_style_tests {
    use hypercolor_types::effect::EffectCategory;
    use strum::VariantNames;

    use super::category_style;

    /// Every real category gets its own identity, not the fallback.
    #[test]
    fn every_effect_category_has_a_styled_arm() {
        let fallback = category_style("definitely-not-a-category");

        for variant in EffectCategory::VARIANTS {
            assert_ne!(
                category_style(variant),
                fallback,
                "{variant} falls through to the unknown-category style"
            );
        }
    }
}
