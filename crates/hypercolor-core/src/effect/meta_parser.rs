//! Lenient HTML metadata extraction for LightScript-style effects.
//!
//! This parser intentionally avoids a full DOM/HTML dependency. It extracts
//! enough metadata from `<title>` and `<meta ...>` tags to populate the effect
//! registry and provide discovery/filtering in the API.

use std::collections::{BTreeSet, HashMap};

use hypercolor_types::effect::EffectCategory;

/// Parsed control type from HTML `<meta property=... type=...>` declarations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlControlKind {
    Number,
    Boolean,
    Color,
    Combobox,
    Sensor,
    Hue,
    Area,
    Text,
    Other(String),
}

impl HtmlControlKind {
    fn from_raw(raw: &str) -> Self {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "color" => Self::Color,
            "combobox" | "dropdown" => Self::Combobox,
            "sensor" => Self::Sensor,
            "hue" => Self::Hue,
            "area" => Self::Area,
            "textfield" | "text" | "input" => Self::Text,
            _ => Self::Other(normalized),
        }
    }
}

/// Parsed control metadata from a property `<meta>` tag.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlControlMetadata {
    pub property: String,
    pub label: String,
    pub kind: HtmlControlKind,
    pub default: Option<String>,
    pub min: Option<f32>,
    pub max: Option<f32>,
    pub step: Option<f32>,
    pub values: Vec<String>,
    pub tooltip: Option<String>,
}

/// Parsed preset from a `<meta preset="..." ...>` tag.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlPresetMetadata {
    pub name: String,
    pub description: Option<String>,
    pub controls: HashMap<String, String>,
}

/// Parsed metadata summary for a single HTML effect file.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedHtmlEffectMetadata {
    pub title: String,
    pub description: String,
    pub publisher: String,
    pub controls: Vec<HtmlControlMetadata>,
    pub presets: Vec<HtmlPresetMetadata>,
    pub category: EffectCategory,
    pub audio_reactive: bool,
    pub uses_canvas2d: bool,
    pub uses_three_js: bool,
    pub tags: Vec<String>,
}

/// Parse HTML effect metadata from a raw file string.
#[must_use]
pub fn parse_html_effect_metadata(html: &str) -> ParsedHtmlEffectMetadata {
    let sanitized = strip_html_comments(html);
    let lower = sanitized.to_ascii_lowercase();

    let mut description = String::new();
    let mut publisher = String::new();
    let mut title_from_meta: Option<String> = None;
    let mut controls = Vec::new();
    let mut presets = Vec::new();

    for meta_tag in extract_start_tags(&sanitized, "meta") {
        let attrs = parse_tag_attributes(&meta_tag);
        if attrs.is_empty() {
            continue;
        }

        if description.is_empty() {
            if let Some(value) = attr_value(&attrs, "description") {
                description = normalize_whitespace(value);
            } else if attr_name_is(&attrs, "description")
                && let Some(content) = attr_value(&attrs, "content")
            {
                description = normalize_whitespace(content);
            }
        }

        if publisher.is_empty() {
            if let Some(value) = attr_value(&attrs, "publisher") {
                publisher = normalize_whitespace(value);
            } else if attr_name_is_any(&attrs, &["publisher", "author"])
                && let Some(content) = attr_value(&attrs, "content")
            {
                publisher = normalize_whitespace(content);
            }
        }

        if title_from_meta.is_none()
            && attr_name_is_any(&attrs, &["name", "title"])
            && let Some(content) = attr_value(&attrs, "content")
        {
            let normalized = normalize_whitespace(content);
            if !normalized.is_empty() {
                title_from_meta = Some(normalized);
            }
        }

        if let Some(preset) = parse_preset_metadata(&attrs) {
            presets.push(preset);
        } else if let Some(control) = parse_control_metadata(&attrs) {
            controls.push(control);
        }
    }

    let title = extract_title(&sanitized)
        .filter(|value| !value.is_empty())
        .or(title_from_meta)
        .unwrap_or_else(|| "Unnamed Effect".to_owned());

    if description.is_empty() {
        description.push_str("No description provided.");
    }
    if publisher.is_empty() {
        publisher.push_str("unknown");
    }

    let audio_reactive = detect_audio_meta_tag(&sanitized)
        .unwrap_or_else(|| detect_audio_reactivity_heuristic(&lower));
    let uses_three_js = detect_three_js(&lower);
    let uses_canvas2d = detect_canvas2d(&lower);
    let category = infer_category(&lower, &controls, audio_reactive);
    let tags = build_tags(
        &controls,
        category,
        audio_reactive,
        uses_canvas2d,
        uses_three_js,
    );

    ParsedHtmlEffectMetadata {
        title,
        description,
        publisher,
        controls,
        presets,
        category,
        audio_reactive,
        uses_canvas2d,
        uses_three_js,
        tags,
    }
}

fn strip_html_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(start_rel) = input[cursor..].find("<!--") {
        let start = cursor + start_rel;
        out.push_str(&input[cursor..start]);

        let body_start = start + 4;
        if let Some(end_rel) = input[body_start..].find("-->") {
            cursor = body_start + end_rel + 3;
        } else {
            cursor = input.len();
            break;
        }
    }

    out.push_str(&input[cursor..]);
    out
}

fn extract_start_tags(input: &str, tag_name: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let bytes = input.as_bytes();
    let tag_bytes = tag_name.as_bytes();

    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'<' {
            idx += 1;
            continue;
        }

        let mut cursor = idx + 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }

        if cursor >= bytes.len() || matches!(bytes[cursor], b'/' | b'!' | b'?') {
            idx += 1;
            continue;
        }

        let name_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'-')
        {
            cursor += 1;
        }

        if !eq_ignore_ascii_case_bytes(&bytes[name_start..cursor], tag_bytes) {
            idx += 1;
            continue;
        }

        let mut end = cursor;
        let mut in_single = false;
        let mut in_double = false;

        while end < bytes.len() {
            match bytes[end] {
                b'\'' if !in_double => in_single = !in_single,
                b'"' if !in_single => in_double = !in_double,
                b'>' if !in_single && !in_double => {
                    end += 1;
                    break;
                }
                _ => {}
            }
            end += 1;
        }

        let clamped_end = end.min(input.len());
        tags.push(input[idx..clamped_end].to_owned());
        idx = clamped_end;
    }

    tags
}

fn parse_tag_attributes(tag: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();

    let trimmed = tag
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_end_matches('/')
        .trim();
    let body = trimmed
        .strip_prefix("meta")
        .or_else(|| trimmed.strip_prefix("META"))
        .unwrap_or(trimmed)
        .trim();

    let bytes = body.as_bytes();
    let mut idx = 0usize;

    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        let key_start = idx;
        while idx < bytes.len() {
            let byte = bytes[idx];
            if byte.is_ascii_whitespace() || byte == b'=' || byte == b'/' {
                break;
            }
            idx += 1;
        }

        if idx == key_start {
            idx += 1;
            continue;
        }

        let key = body[key_start..idx].to_ascii_lowercase();

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        let mut value = String::new();

        if idx < bytes.len() && bytes[idx] == b'=' {
            idx += 1;
            while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
                idx += 1;
            }

            if idx < bytes.len() {
                if matches!(bytes[idx], b'"' | b'\'') {
                    let quote = bytes[idx];
                    idx += 1;
                    let value_start = idx;
                    while idx < bytes.len() && bytes[idx] != quote {
                        idx += 1;
                    }
                    value.clear();
                    value.push_str(&body[value_start..idx]);
                    if idx < bytes.len() {
                        idx += 1;
                    }
                } else {
                    let value_start = idx;
                    while idx < bytes.len() {
                        let byte = bytes[idx];
                        if byte.is_ascii_whitespace() || byte == b'/' {
                            break;
                        }
                        idx += 1;
                    }
                    value.clear();
                    value.push_str(&body[value_start..idx]);
                }
            }
        }

        attrs.insert(key, value);
    }

    attrs
}

fn parse_control_metadata(attrs: &HashMap<String, String>) -> Option<HtmlControlMetadata> {
    let property = attr_value(attrs, "property")?;
    if property.is_empty() {
        return None;
    }

    let raw_type = attr_value(attrs, "type").unwrap_or("number");
    let kind = HtmlControlKind::from_raw(raw_type);

    let label = attr_value(attrs, "label").map_or_else(|| property.to_owned(), ToOwned::to_owned);

    Some(HtmlControlMetadata {
        property: property.to_owned(),
        label,
        kind,
        default: attr_value(attrs, "default").map(ToOwned::to_owned),
        min: parse_f32_attr(attrs, "min"),
        max: parse_f32_attr(attrs, "max"),
        step: parse_f32_attr(attrs, "step"),
        values: parse_csv_attr(attrs, "values"),
        tooltip: attr_value(attrs, "tooltip").map(ToOwned::to_owned),
    })
}

/// Parse a `<meta preset="Name" preset-description="..." preset-controls='{"k":"v"}'>` tag.
fn parse_preset_metadata(attrs: &HashMap<String, String>) -> Option<HtmlPresetMetadata> {
    let name = attr_value(attrs, "preset")?;
    if name.is_empty() {
        return None;
    }

    let description = attr_value(attrs, "preset-description").map(normalize_whitespace);

    // Controls are stored as a JSON object in the preset-controls attribute.
    let controls = attr_value(attrs, "preset-controls")
        .and_then(|json_str| {
            serde_json::from_str::<serde_json::Value>(json_str)
                .ok()
                .and_then(|v| v.as_object().cloned())
        })
        .map(|obj| {
            obj.into_iter()
                .map(|(k, v)| {
                    let s = match &v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    (k, s)
                })
                .collect()
        })
        .unwrap_or_default();

    Some(HtmlPresetMetadata {
        name: normalize_whitespace(name),
        description,
        controls,
    })
}

fn parse_f32_attr(attrs: &HashMap<String, String>, key: &str) -> Option<f32> {
    attr_value(attrs, key)?.parse::<f32>().ok()
}

fn parse_csv_attr(attrs: &HashMap<String, String>, key: &str) -> Vec<String> {
    attr_value(attrs, key)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}

/// Check for an explicit `<meta audio-reactive="true"/>` tag (emitted by the SDK build script).
/// Returns `Some(true/false)` if found, `None` if absent (fall back to heuristic).
fn detect_audio_meta_tag(html: &str) -> Option<bool> {
    for meta_tag in extract_start_tags(html, "meta") {
        let attrs = parse_tag_attributes(&meta_tag);
        if let Some(value) = attr_value(&attrs, "audio-reactive") {
            return Some(value.eq_ignore_ascii_case("true") || value == "1");
        }
    }
    None
}

/// Heuristic fallback for legacy/custom effects that lack an explicit audio meta tag.
fn detect_audio_reactivity_heuristic(lower: &str) -> bool {
    const AUDIO_MARKERS: &[&str] = &[
        "engine.audio",
        "iaudio",
        "audio.freq",
        "audio.level",
        "audio.density",
    ];

    AUDIO_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn detect_three_js(lower: &str) -> bool {
    const WEBGL_MARKERS: &[&str] = &["three.", "webglrenderer", "webglrendertarget"];
    WEBGL_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn detect_canvas2d(lower: &str) -> bool {
    const CANVAS_MARKERS: &[&str] = &[
        "getcontext(\"2d\"",
        "getcontext('2d'",
        "getcontext(\"2d'",
        "getcontext('2d\"",
    ];
    CANVAS_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn infer_category(
    lower: &str,
    _controls: &[HtmlControlMetadata],
    audio_reactive: bool,
) -> EffectCategory {
    if audio_reactive {
        return EffectCategory::Audio;
    }

    if contains_any(
        lower,
        &["mouse", "keyboard", "tap", "click", "touch", "cursor"],
    ) {
        return EffectCategory::Interactive;
    }

    if contains_any(
        lower,
        &[
            "firework",
            "meteor",
            "bubble",
            "fire",
            "ember",
            "particle",
            "spark",
            "trail",
            "confetti",
            "explosion",
        ],
    ) {
        return EffectCategory::Particle;
    }

    if contains_any(lower, &["city", "landscape", "underwater", "scenic", "sky"]) {
        return EffectCategory::Scenic;
    }

    if contains_any(
        lower,
        &["game", "gaming", "fun", "holiday", "snow", "tetris"],
    ) {
        return EffectCategory::Fun;
    }

    if contains_any(
        lower,
        &[
            "fractal",
            "voronoi",
            "plasma",
            "noise",
            "kaleido",
            "quantum",
            "neural",
            "cellular",
            "automaton",
        ],
    ) {
        return EffectCategory::Generative;
    }

    if contains_any(
        lower,
        &["status", "monitor", "clock", "temperature", "battery"],
    ) {
        return EffectCategory::Utility;
    }

    EffectCategory::Ambient
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn build_tags(
    controls: &[HtmlControlMetadata],
    category: EffectCategory,
    audio_reactive: bool,
    uses_canvas2d: bool,
    uses_three_js: bool,
) -> Vec<String> {
    let mut tags = BTreeSet::new();
    tags.insert("html".to_owned());
    tags.insert(format!("{category}"));

    if uses_canvas2d {
        tags.insert("canvas2d".to_owned());
    }
    if uses_three_js {
        tags.insert("webgl".to_owned());
        tags.insert("threejs".to_owned());
    }
    if audio_reactive {
        tags.insert("audio-reactive".to_owned());
    }

    if controls
        .iter()
        .any(|control| matches!(control.kind, HtmlControlKind::Color))
    {
        tags.insert("color-control".to_owned());
    }
    if controls
        .iter()
        .any(|control| matches!(control.kind, HtmlControlKind::Combobox))
    {
        tags.insert("combobox-control".to_owned());
    }
    if controls
        .iter()
        .any(|control| matches!(control.kind, HtmlControlKind::Sensor))
    {
        tags.insert("sensor-control".to_owned());
    }

    tags.into_iter().collect()
}

fn extract_title(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let start = find_ascii_case_insensitive(bytes, b"<title", 0)?;

    let mut open_end = start;
    while open_end < bytes.len() && bytes[open_end] != b'>' {
        open_end += 1;
    }
    if open_end >= bytes.len() {
        return None;
    }
    open_end += 1;

    let close_start = find_ascii_case_insensitive(bytes, b"</title>", open_end)?;
    let raw = &input[open_end..close_start];
    let normalized = normalize_whitespace(raw);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn attr_value<'a>(attrs: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    attrs
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn attr_name_is(attrs: &HashMap<String, String>, expected: &str) -> bool {
    attrs
        .get("name")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
}

fn attr_name_is_any(attrs: &HashMap<String, String>, expected: &[&str]) -> bool {
    attrs.get("name").is_some_and(|value| {
        let normalized = value.trim();
        expected
            .iter()
            .any(|candidate| normalized.eq_ignore_ascii_case(candidate))
    })
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }

    let max_start = haystack.len().checked_sub(needle.len())?;
    let mut idx = from;
    while idx <= max_start {
        if eq_ignore_ascii_case_bytes(&haystack[idx..idx + needle.len()], needle) {
            return Some(idx);
        }
        idx += 1;
    }
    None
}

fn eq_ignore_ascii_case_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.iter()
        .zip(b.iter())
        .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lightscript_style_metadata() {
        let html = r#"
<head>
  <title>Aurora</title>
  <meta description="Northern lights simulation" />
  <meta publisher="Hypercolor" />
  <meta property="speed" label="Speed" type="number" default="40" min="0" max="100" />
  <meta property="cycle" label="Cycle" type="boolean" default="1" />
</head>
<script>
  const freqs = new Uint8Array(engine.audio.freq);
</script>
"#;

        let parsed = parse_html_effect_metadata(html);
        assert_eq!(parsed.title, "Aurora");
        assert_eq!(parsed.description, "Northern lights simulation");
        assert_eq!(parsed.publisher, "Hypercolor");
        assert_eq!(parsed.controls.len(), 2);
        assert!(parsed.audio_reactive);
        assert_eq!(parsed.category, EffectCategory::Audio);
        assert!(parsed.tags.contains(&"html".to_owned()));
        assert!(parsed.tags.contains(&"audio-reactive".to_owned()));
    }

    #[test]
    fn parses_name_content_metadata() {
        let html = r#"
<head>
  <meta name="name" content="Broken Cube Visualizer" />
  <meta name="description" content="3D cube visualizer" />
  <meta name="author" content="Nova" />
  <meta property="mode" label="Mode" type="combobox" values="A,B,C" default="A" />
</head>
<script>
  console.log("THREE.WebGLRenderer");
</script>
"#;

        let parsed = parse_html_effect_metadata(html);
        assert_eq!(parsed.title, "Broken Cube Visualizer");
        assert_eq!(parsed.description, "3D cube visualizer");
        assert_eq!(parsed.publisher, "Nova");
        assert_eq!(parsed.controls.len(), 1);
        assert!(parsed.uses_three_js);
        assert!(parsed.tags.contains(&"webgl".to_owned()));
    }

    #[test]
    fn ignores_meta_tags_inside_comments() {
        let html = r#"
<head>
  <title>Comment Test</title>
  <!-- <meta property="ghost" label="Ghost" type="number" default="10" /> -->
  <meta property="real" label="Real" type="number" default="1" />
</head>
"#;

        let parsed = parse_html_effect_metadata(html);
        assert_eq!(parsed.controls.len(), 1);
        assert_eq!(parsed.controls[0].property, "real");
    }

    #[test]
    fn supports_sensor_control_kind() {
        let html = r#"
<head>
  <title>Sensor Test</title>
  <meta property="userSensor1" label="Sensor" type="sensor" default="CPU Load" />
</head>
"#;

        let parsed = parse_html_effect_metadata(html);
        assert_eq!(parsed.controls.len(), 1);
        assert!(matches!(parsed.controls[0].kind, HtmlControlKind::Sensor));
        assert!(parsed.tags.contains(&"sensor-control".to_owned()));
    }

    #[test]
    fn explicit_audio_meta_tag_overrides_heuristic() {
        // Simulates a bundled SDK effect: engine.audio appears in the runtime code
        // but the effect itself is NOT audio-reactive (no meta tag = would trigger heuristic).
        // The explicit meta tag should take precedence.
        let html = r#"
<head>
  <title>Borealis</title>
  <meta description="Aurora borealis effect" />
  <meta publisher="Hypercolor"/>
</head>
<script>
  // Bundled SDK runtime includes audio infrastructure:
  engine.audio.freq; audio.level; audio.density;
</script>
"#;
        let parsed = parse_html_effect_metadata(html);
        // Without explicit meta tag, heuristic fires — this is the legacy fallback
        assert!(parsed.audio_reactive);

        // Now with explicit audio-reactive="true" meta tag
        let html_audio = r#"
<head>
  <title>Audio Pulse</title>
  <meta description="Audio visualizer" />
  <meta audio-reactive="true"/>
</head>
<script>engine.audio.freq;</script>
"#;
        let parsed = parse_html_effect_metadata(html_audio);
        assert!(parsed.audio_reactive);
        assert_eq!(parsed.category, EffectCategory::Audio);

        // Explicit audio-reactive absent — but NO audio markers in body = not audio
        let html_clean = r#"
<head>
  <title>Clean Effect</title>
  <meta description="No audio" />
</head>
<script>console.log("hello");</script>
"#;
        let parsed = parse_html_effect_metadata(html_clean);
        assert!(!parsed.audio_reactive);
    }
}
