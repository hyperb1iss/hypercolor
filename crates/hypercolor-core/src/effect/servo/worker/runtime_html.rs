use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::effect::{ControlValue, EffectCategory, EffectMetadata};
use reqwest::Url;

/// Whether an effect should receive `engine.audio.*` updates each frame.
pub(in crate::effect::servo) fn effect_is_audio_reactive(metadata: &EffectMetadata) -> bool {
    if metadata.audio_reactive {
        return true;
    }

    if matches!(metadata.category, EffectCategory::Audio) {
        return true;
    }

    metadata
        .tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case("audio") || tag.eq_ignore_ascii_case("audio-reactive"))
}

/// Prepare an HTML file with a runtime control preamble injected into `<head>`.
pub(in crate::effect::servo) fn prepare_runtime_html_source(
    original_path: &Path,
    controls: &HashMap<String, ControlValue>,
    host_driven_animation: bool,
    display_descriptor: Option<&DisplayDescriptor>,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let html = std::fs::read_to_string(original_path).with_context(|| {
        format!(
            "failed to read HTML effect file while preparing runtime source: {}",
            original_path.display()
        )
    })?;

    let preamble =
        build_control_preamble_script(controls, host_driven_animation, display_descriptor);
    let base_tag = original_path
        .parent()
        .and_then(|parent| Url::from_directory_path(parent).ok())
        .map_or_else(String::new, |url| format!("<base href=\"{url}\">\n"));
    let injected_block = format!("{base_tag}<script>\n{preamble}\n</script>\n");
    let runtime_html = inject_runtime_head_block(&html, &injected_block);

    let cache_root = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hypercolor")
        .join("servo-runtime");
    std::fs::create_dir_all(&cache_root).with_context(|| {
        format!(
            "failed to create Servo runtime cache directory: {}",
            cache_root.display()
        )
    })?;

    let runtime_path = cache_root.join(format!("effect-{}.html", uuid::Uuid::now_v7()));
    std::fs::write(&runtime_path, runtime_html).with_context(|| {
        format!(
            "failed to write runtime HTML source '{}'",
            runtime_path.display()
        )
    })?;

    Ok((runtime_path.clone(), Some(runtime_path)))
}

fn build_control_preamble_script(
    controls: &HashMap<String, ControlValue>,
    host_driven_animation: bool,
    display_descriptor: Option<&DisplayDescriptor>,
) -> String {
    let mut sorted_controls: Vec<_> = controls.iter().collect();
    sorted_controls.sort_by_key(|(name, _)| *name);

    let mut script = String::from("(function(){\n");
    script.push_str("  window.__hypercolorCaptureMode = true;\n");
    script.push_str("  window.__hypercolorPreserveDrawingBuffer = false;\n");
    if host_driven_animation {
        script.push_str("  window.__hypercolorHostDrivenAnimation = true;\n");
    }
    if let Some(descriptor) = display_descriptor {
        let payload = descriptor.bootstrap_json().to_string();
        script.push_str("  window.hypercolor = window.hypercolor || {};\n");
        let _ = writeln!(script, "  window.hypercolor.display = {payload};");
    }
    script.push_str("  if (typeof globalThis === 'object' && globalThis !== null) {\n");
    script.push_str("    globalThis.__hypercolorCaptureMode = true;\n");
    script.push_str("    globalThis.__hypercolorPreserveDrawingBuffer = false;\n");
    if host_driven_animation {
        script.push_str("    globalThis.__hypercolorHostDrivenAnimation = true;\n");
    }
    if display_descriptor.is_some() {
        script.push_str("    globalThis.hypercolor = window.hypercolor;\n");
    }
    script.push_str("  }\n");
    for (name, value) in sorted_controls {
        let key_literal = serde_json::to_string(name).unwrap_or_else(|_| "\"invalid\"".to_owned());
        let _ = writeln!(
            script,
            "  if (typeof globalThis[{key_literal}] === 'undefined') globalThis[{key_literal}] = {};",
            value.to_js_literal()
        );
    }
    script.push_str("})();");
    script
}

fn inject_runtime_head_block(html: &str, block: &str) -> String {
    let lowered = html.to_ascii_lowercase();

    if let Some(head_start) = lowered.find("<head")
        && let Some(head_close_offset) = lowered[head_start..].find('>')
    {
        let insert_at = head_start + head_close_offset + 1;
        let (before, after) = html.split_at(insert_at);
        return format!("{before}\n{block}{after}");
    }

    if let Some(script_start) = lowered.find("<script") {
        let (before, after) = html.split_at(script_start);
        return format!("{before}\n{block}{after}");
    }

    format!("{block}{html}")
}

#[cfg(test)]
mod tests {
    use hypercolor_types::effect::{EffectId, EffectSource};
    use uuid::Uuid;

    use super::*;

    #[test]
    fn control_preamble_assigns_all_defaults() {
        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(42.0));
        controls.insert("enabled".to_owned(), ControlValue::Boolean(true));
        controls.insert("color".to_owned(), ControlValue::Text("#00ffaa".to_owned()));

        let script = build_control_preamble_script(&controls, false, None);

        assert!(script.contains("globalThis[\"speed\"] = 42"));
        assert!(script.contains("globalThis[\"enabled\"] = true"));
        assert!(script.contains("globalThis[\"color\"] = \"#00ffaa\""));
        assert!(script.contains("window.__hypercolorCaptureMode = true"));
        assert!(script.contains("window.__hypercolorPreserveDrawingBuffer = false"));
        assert!(script.contains("globalThis.__hypercolorCaptureMode = true"));
        assert!(script.contains("globalThis.__hypercolorPreserveDrawingBuffer = false"));
        assert!(!script.contains("__hypercolorHostDrivenAnimation"));
    }

    #[test]
    fn control_preamble_marks_host_driven_animation_before_effect_script_runs() {
        let controls = HashMap::new();

        let script = build_control_preamble_script(&controls, true, None);

        assert!(script.contains("window.__hypercolorHostDrivenAnimation = true"));
        assert!(script.contains("globalThis.__hypercolorHostDrivenAnimation = true"));
    }

    #[test]
    fn inject_runtime_block_prefers_head_tag() {
        let html = "<html><head><title>x</title></head><body><script>run()</script></body></html>";
        let block = "<script>bootstrap()</script>\n";

        let injected = inject_runtime_head_block(html, block);
        let expected = "<html><head>\n<script>bootstrap()</script>\n<title>x</title></head>";
        assert!(injected.contains(expected));
    }

    #[test]
    fn inject_runtime_block_falls_back_to_first_script() {
        let html = "<body><script>run()</script></body>";
        let block = "<script>bootstrap()</script>\n";

        let injected = inject_runtime_head_block(html, block);
        assert!(injected.starts_with("<body>\n<script>bootstrap()</script>"));
    }

    #[test]
    fn prepare_runtime_html_source_injects_capture_and_host_flags_before_script() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let html_path = temp.path().join("effect.html");
        std::fs::write(
            &html_path,
            "<html><head><title>x</title></head><body><script>run()</script></body></html>",
        )
        .expect("html write should work");

        let controls = HashMap::new();
        let (runtime_path, runtime_html_path) =
            prepare_runtime_html_source(&html_path, &controls, true, None)
                .expect("runtime html should build");

        assert_ne!(runtime_path, html_path);
        assert_eq!(runtime_html_path.as_deref(), Some(runtime_path.as_path()));

        let runtime_html =
            std::fs::read_to_string(&runtime_path).expect("runtime html should be readable");
        assert!(runtime_html.contains("window.__hypercolorCaptureMode = true"));
        assert!(runtime_html.contains("window.__hypercolorPreserveDrawingBuffer = false"));
        assert!(runtime_html.contains("window.__hypercolorHostDrivenAnimation = true"));
        assert!(
            runtime_html.find("window.__hypercolorHostDrivenAnimation = true")
                < runtime_html.find("<script>run()</script>")
        );
    }

    #[test]
    fn prepare_runtime_html_source_injects_display_descriptor_before_script() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let html_path = temp.path().join("face.html");
        std::fs::write(
            &html_path,
            "<html><head><title>x</title></head><body><script>run()</script></body></html>",
        )
        .expect("html write should work");

        let descriptor = hypercolor_types::display::DisplayDescriptor::derive(
            960,
            160,
            false,
            None,
            30,
            hypercolor_types::display::DisplayPixelFormat::Rgb,
        );
        let controls = HashMap::new();
        let (runtime_path, _) =
            prepare_runtime_html_source(&html_path, &controls, true, Some(&descriptor))
                .expect("runtime html should build");

        let runtime_html =
            std::fs::read_to_string(&runtime_path).expect("runtime html should be readable");
        assert!(runtime_html.contains("window.hypercolor = window.hypercolor || {}"));
        assert!(runtime_html.contains("\"apiVersion\":1"));
        assert!(runtime_html.contains("\"shape\":\"wide\""));
        assert!(runtime_html.contains("\"class\":\"strip\""));
        assert!(runtime_html.contains("\"safeArea\""));
        assert!(runtime_html.contains("\"targetFps\":30"));
        assert!(runtime_html.contains("globalThis.hypercolor = window.hypercolor"));
        assert!(
            runtime_html.find("window.hypercolor.display =")
                < runtime_html.find("<script>run()</script>")
        );
    }

    #[test]
    fn prepare_runtime_html_source_omits_descriptor_when_absent() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let html_path = temp.path().join("effect.html");
        std::fs::write(&html_path, "<html><head></head><body></body></html>")
            .expect("html write should work");

        let controls = HashMap::new();
        let (runtime_path, _) = prepare_runtime_html_source(&html_path, &controls, false, None)
            .expect("runtime html should build");

        let runtime_html =
            std::fs::read_to_string(&runtime_path).expect("runtime html should be readable");
        assert!(!runtime_html.contains("window.hypercolor.display"));
    }

    #[test]
    fn effect_is_audio_reactive_for_audio_category() {
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Audio".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Audio reactive".to_owned(),
            category: EffectCategory::Audio,
            tags: Vec::new(),
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: true,
            screen_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/audio.html"),
            },
            license: None,
        };

        assert!(effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn effect_is_audio_reactive_for_audio_tags() {
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Ambient Audio".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Ambient effect with audio response".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["visual".to_owned(), "audio-reactive".to_owned()],
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/ambient-audio.html"),
            },
            license: None,
        };

        assert!(effect_is_audio_reactive(&metadata));
    }

    #[test]
    fn effect_is_not_audio_reactive_without_audio_signals() {
        let metadata = EffectMetadata {
            id: EffectId::from(Uuid::nil()),
            name: "Electric Colors".to_owned(),
            author: "hypercolor".to_owned(),
            version: "0.1.0".to_owned(),
            description: "Ambient effect".to_owned(),
            category: EffectCategory::Ambient,
            tags: vec!["ambient".to_owned(), "canvas2d".to_owned()],
            controls: Vec::new(),
            presets: Vec::new(),
            audio_reactive: false,
            screen_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/electric-colors.html"),
            },
            license: None,
        };

        assert!(!effect_is_audio_reactive(&metadata));
    }
}
