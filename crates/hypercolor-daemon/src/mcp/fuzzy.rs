//! Natural language color parsing and matching.

use hypercolor_color::{Hsl, Rgb};

/// Minimum score threshold for a match to be considered viable.
const MATCH_THRESHOLD: f32 = 0.3;

/// Compute the Levenshtein edit distance between two strings.
///
/// Uses the classic dynamic programming approach with O(min(m,n)) space.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    // Use a single row to save memory — O(min(m,n)) space.
    let mut prev_row: Vec<usize> = (0..=n).collect();
    let mut curr_row = vec![0; n + 1];

    for i in 1..=m {
        curr_row[0] = i;
        for j in 1..=n {
            let cost = usize::from(a_chars[i - 1] != b_chars[j - 1]);
            curr_row[j] = (prev_row[j] + 1)
                .min(curr_row[j - 1] + 1)
                .min(prev_row[j - 1] + cost);
        }
        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[n]
}

// ── Color Matching ─────────────────────────────────────────────────────────

/// An RGB color with a human-readable name.
#[derive(Debug, Clone)]
pub struct NamedColor {
    /// Human-readable color name (lowercase).
    pub name: &'static str,
    /// sRGB red component (0–255).
    pub r: u8,
    /// sRGB green component (0–255).
    pub g: u8,
    /// sRGB blue component (0–255).
    pub b: u8,
}

/// A color match result with the resolved RGB value.
#[derive(Debug, Clone)]
pub struct ColorMatch {
    /// The matched or resolved color name.
    pub name: String,
    /// Hex representation (e.g., `"#ff6ac1"`).
    pub hex: String,
    /// sRGB components.
    pub r: u8,
    /// sRGB components.
    pub g: u8,
    /// sRGB components.
    pub b: u8,
    /// Match confidence (1.0 = exact parse, lower for fuzzy name matches).
    pub confidence: f32,
}

/// Built-in color name table — 40 common colors covering the full hue range.
static NAMED_COLORS: &[NamedColor] = &[
    NamedColor {
        name: "red",
        r: 255,
        g: 0,
        b: 0,
    },
    NamedColor {
        name: "green",
        r: 0,
        g: 128,
        b: 0,
    },
    NamedColor {
        name: "blue",
        r: 0,
        g: 0,
        b: 255,
    },
    NamedColor {
        name: "white",
        r: 255,
        g: 255,
        b: 255,
    },
    NamedColor {
        name: "black",
        r: 0,
        g: 0,
        b: 0,
    },
    NamedColor {
        name: "yellow",
        r: 255,
        g: 255,
        b: 0,
    },
    NamedColor {
        name: "cyan",
        r: 0,
        g: 255,
        b: 255,
    },
    NamedColor {
        name: "magenta",
        r: 255,
        g: 0,
        b: 255,
    },
    NamedColor {
        name: "orange",
        r: 255,
        g: 165,
        b: 0,
    },
    NamedColor {
        name: "purple",
        r: 128,
        g: 0,
        b: 128,
    },
    NamedColor {
        name: "pink",
        r: 255,
        g: 192,
        b: 203,
    },
    NamedColor {
        name: "coral",
        r: 255,
        g: 127,
        b: 80,
    },
    NamedColor {
        name: "salmon",
        r: 250,
        g: 128,
        b: 114,
    },
    NamedColor {
        name: "crimson",
        r: 220,
        g: 20,
        b: 60,
    },
    NamedColor {
        name: "gold",
        r: 255,
        g: 215,
        b: 0,
    },
    NamedColor {
        name: "lime",
        r: 0,
        g: 255,
        b: 0,
    },
    NamedColor {
        name: "teal",
        r: 0,
        g: 128,
        b: 128,
    },
    NamedColor {
        name: "navy",
        r: 0,
        g: 0,
        b: 128,
    },
    NamedColor {
        name: "indigo",
        r: 75,
        g: 0,
        b: 130,
    },
    NamedColor {
        name: "violet",
        r: 238,
        g: 130,
        b: 238,
    },
    NamedColor {
        name: "lavender",
        r: 230,
        g: 230,
        b: 250,
    },
    NamedColor {
        name: "turquoise",
        r: 64,
        g: 224,
        b: 208,
    },
    NamedColor {
        name: "aquamarine",
        r: 127,
        g: 255,
        b: 212,
    },
    NamedColor {
        name: "olive",
        r: 128,
        g: 128,
        b: 0,
    },
    NamedColor {
        name: "maroon",
        r: 128,
        g: 0,
        b: 0,
    },
    NamedColor {
        name: "tan",
        r: 210,
        g: 180,
        b: 140,
    },
    NamedColor {
        name: "beige",
        r: 245,
        g: 245,
        b: 220,
    },
    NamedColor {
        name: "ivory",
        r: 255,
        g: 255,
        b: 240,
    },
    NamedColor {
        name: "chartreuse",
        r: 127,
        g: 255,
        b: 0,
    },
    NamedColor {
        name: "amber",
        r: 255,
        g: 191,
        b: 0,
    },
    NamedColor {
        name: "peach",
        r: 255,
        g: 218,
        b: 185,
    },
    NamedColor {
        name: "rose",
        r: 255,
        g: 0,
        b: 127,
    },
    NamedColor {
        name: "sky blue",
        r: 135,
        g: 206,
        b: 235,
    },
    NamedColor {
        name: "ocean blue",
        r: 0,
        g: 105,
        b: 148,
    },
    NamedColor {
        name: "forest green",
        r: 34,
        g: 139,
        b: 34,
    },
    NamedColor {
        name: "warm white",
        r: 255,
        g: 244,
        b: 229,
    },
    NamedColor {
        name: "cool white",
        r: 240,
        g: 248,
        b: 255,
    },
    NamedColor {
        name: "sunset orange",
        r: 255,
        g: 83,
        b: 73,
    },
    NamedColor {
        name: "hot pink",
        r: 255,
        g: 105,
        b: 180,
    },
    NamedColor {
        name: "electric blue",
        r: 125,
        g: 249,
        b: 255,
    },
];

/// Resolve a color specification string to an RGB value.
///
/// Tries parsing strategies in order:
/// 1. Hex code (`#ff6ac1` or `ff6ac1`)
/// 2. `rgb(r, g, b)` function syntax
/// 3. `hsl(h, s%, l%)` function syntax
/// 4. Named color lookup (exact or fuzzy)
pub fn resolve_color(input: &str) -> Option<ColorMatch> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. Hex code
    if let Some(result) = parse_hex(trimmed) {
        return Some(result);
    }

    // 2. rgb() function
    if let Some(result) = parse_rgb_function(trimmed) {
        return Some(result);
    }

    // 3. hsl() function
    if let Some(result) = parse_hsl_function(trimmed) {
        return Some(result);
    }

    // 4. Named color (exact then fuzzy)
    match_color_name(trimmed)
}

/// Parse a hex color code like `#ff6ac1`, `ff6ac1`, or `#f6c`.
///
/// Shorthand is honored only behind a `#`. Without one, a three-letter
/// word of hex digits is a name query — `bed` asks the name matcher for a
/// color called "bed", not for `#bbeedd`.
fn parse_hex(input: &str) -> Option<ColorMatch> {
    let explicit = input.starts_with('#');
    if !explicit && input.len() != 6 {
        return None;
    }

    let Rgb {
        r: red,
        g: green,
        b: blue,
    } = Rgb::from_hex(input).ok()?;
    let closest_name = find_closest_color_name(red, green, blue);
    Some(ColorMatch {
        name: closest_name,
        hex: format!("#{red:02x}{green:02x}{blue:02x}"),
        r: red,
        g: green,
        b: blue,
        confidence: 1.0,
    })
}

/// Parse `rgb(255, 106, 193)` syntax.
fn parse_rgb_function(input: &str) -> Option<ColorMatch> {
    let lower = input.to_lowercase();
    let inner = lower.strip_prefix("rgb(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return None;
    }
    let red: u8 = parts[0].parse().ok()?;
    let green: u8 = parts[1].parse().ok()?;
    let blue: u8 = parts[2].parse().ok()?;
    let closest_name = find_closest_color_name(red, green, blue);
    Some(ColorMatch {
        name: closest_name,
        hex: format!("#{red:02x}{green:02x}{blue:02x}"),
        r: red,
        g: green,
        b: blue,
        confidence: 1.0,
    })
}

/// Parse `hsl(330, 100%, 71%)` syntax and convert to RGB.
fn parse_hsl_function(input: &str) -> Option<ColorMatch> {
    let lower = input.to_lowercase();
    let inner = lower.strip_prefix("hsl(")?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return None;
    }
    let hue: f32 = parts[0].parse().ok()?;
    let sat: f32 = parts[1].strip_suffix('%')?.parse::<f32>().ok()? / 100.0;
    let light: f32 = parts[2].strip_suffix('%')?.parse::<f32>().ok()? / 100.0;

    let Rgb {
        r: red,
        g: green,
        b: blue,
    } = Hsl::new(hue, sat, light).to_rgb();
    let closest_name = find_closest_color_name(red, green, blue);
    Some(ColorMatch {
        name: closest_name,
        hex: format!("#{red:02x}{green:02x}{blue:02x}"),
        r: red,
        g: green,
        b: blue,
        confidence: 1.0,
    })
}

/// Match a natural language color name against the built-in palette.
///
/// Tries exact match first, then Levenshtein fuzzy match, then keyword overlap.
fn match_color_name(input: &str) -> Option<ColorMatch> {
    let lower = input.to_lowercase();

    // Exact match
    for color in NAMED_COLORS {
        if color.name == lower {
            return Some(ColorMatch {
                name: color.name.to_owned(),
                hex: format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
                r: color.r,
                g: color.g,
                b: color.b,
                confidence: 1.0,
            });
        }
    }

    // Fuzzy match — find closest by Levenshtein distance
    let mut best_score: f32 = 0.0;
    let mut best_color: Option<&NamedColor> = None;

    for color in NAMED_COLORS {
        let distance = levenshtein(&lower, color.name);
        let max_len = lower.len().max(color.name.len());
        if max_len == 0 {
            continue;
        }
        #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
        let score = 1.0 - (distance as f32 / max_len as f32);
        if score > best_score {
            best_score = score;
            best_color = Some(color);
        }

        // Also check if any query word matches part of the color name
        let query_words: Vec<&str> = lower.split_whitespace().collect();
        for word in &query_words {
            if color.name.contains(word) {
                #[expect(clippy::cast_precision_loss, clippy::as_conversions)]
                let word_ratio =
                    word.len().min(color.name.len()) as f32 / color.name.len().max(1) as f32;
                let word_score = 0.6 + (0.3 * word_ratio);
                if word_score > best_score {
                    best_score = word_score;
                    best_color = Some(color);
                }
            }
        }
    }

    let color = best_color?;
    if best_score > MATCH_THRESHOLD {
        Some(ColorMatch {
            name: color.name.to_owned(),
            hex: format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
            r: color.r,
            g: color.g,
            b: color.b,
            confidence: best_score,
        })
    } else {
        None
    }
}

/// Find the closest named color to a given RGB value using Euclidean distance.
fn find_closest_color_name(r: u8, g: u8, b: u8) -> String {
    let mut best_name = "unknown";
    let mut best_distance = u32::MAX;

    for color in NAMED_COLORS {
        let dr = i32::from(r) - i32::from(color.r);
        let dg = i32::from(g) - i32::from(color.g);
        let db = i32::from(b) - i32::from(color.b);
        #[expect(clippy::cast_sign_loss, clippy::as_conversions)]
        let distance = (dr * dr + dg * dg + db * db) as u32;
        if distance < best_distance {
            best_distance = distance;
            best_name = color.name;
        }
    }

    best_name.to_owned()
}
