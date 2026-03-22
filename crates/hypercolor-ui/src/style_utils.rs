//! Shared styling utilities — color conversions, accent generation, badge
//! classes, and reusable UI primitives used across the app.

use leptos::prelude::*;

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

/// Generate a short pseudo-random hex ID (suitable for zone IDs in the editor).
pub fn uuid_v4_hex() -> String {
    let r = js_sys::Math::random();
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
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn hsl_to_rgb_string(h: f32, s: f32, l: f32) -> String {
    let s = s / 100.0;
    let l = l / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r1, g1, b1) = match h as u32 {
        0..60 => (c, x, 0.0),
        60..120 => (x, c, 0.0),
        120..180 => (0.0, c, x),
        180..240 => (0.0, x, c),
        240..300 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    let r = ((r1 + m) * 255.0).round() as u8;
    let g = ((g1 + m) * 255.0).round() as u8;
    let b = ((b1 + m) * 255.0).round() as u8;
    format!("{r}, {g}, {b}")
}

// ── Shared UI primitives ────────────────────────────────────────────────────

/// Render a row of filter chips with active/inactive states.
///
/// Each chip is a `(label, rgb)` pair. The `current` signal holds the active
/// label; clicking a chip updates it via `set_current`.
pub fn filter_chips(
    chips: &'static [(&'static str, &'static str)],
    current: ReadSignal<String>,
    set_current: WriteSignal<String>,
) -> impl IntoView {
    chips
        .iter()
        .map(|&(label, rgb)| {
            let is_active = Memo::new(move |_| current.get() == label);
            let active_style = format!(
                "background: rgba({rgb}, 0.15); color: rgb({rgb}); border-color: rgba({rgb}, 0.3); \
                 box-shadow: 0 0 8px rgba({rgb}, 0.15)"
            );
            let inactive_style = format!(
                "color: rgba({rgb}, 0.5); border-color: rgba({rgb}, 0.08); background: transparent"
            );
            view! {
                <button
                    class="px-2 py-0.5 rounded-full text-[10px] font-medium capitalize border transition-all"
                    style=move || if is_active.get() { active_style.clone() } else { inactive_style.clone() }
                    on:click=move |_| set_current.set(label.to_string())
                >
                    {label}
                </button>
            }
        })
        .collect_view()
}
