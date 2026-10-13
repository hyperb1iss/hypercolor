use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hypercolor_types::control::ControlValue;
use hypercolor_types::display::DisplayDescriptor;
use hypercolor_types::effect::{EffectCategory, EffectMetadata};
use reqwest::Url;
use tracing::debug;

use crate::effect::console::RuntimeLineShift;
use crate::effect::lightscript::control_js_literal;

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

/// An effect page ready for Servo to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::effect::servo) struct RuntimeHtmlSource {
    /// File Servo should load.
    pub(in crate::effect::servo) path: PathBuf,
    /// Temporary runtime copy to delete once the page is done with, if one
    /// was written.
    pub(in crate::effect::servo) cleanup_path: Option<PathBuf>,
    /// How the runtime copy shifts the author's lines, when it differs.
    pub(in crate::effect::servo) line_shift: Option<RuntimeLineShift>,
}

/// Prepare an HTML file with a runtime control preamble injected into `<head>`.
///
/// A session that did not ask for engine globals loads its page verbatim, so
/// no preamble is built and no temporary file is written.
pub(in crate::effect::servo) fn prepare_runtime_html_source(
    original_path: &Path,
    controls: &HashMap<String, ControlValue>,
    host_driven_animation: bool,
    display_descriptor: Option<&DisplayDescriptor>,
    inject_engine_globals: bool,
) -> Result<RuntimeHtmlSource> {
    if !inject_engine_globals {
        return Ok(RuntimeHtmlSource {
            path: original_path.to_path_buf(),
            cleanup_path: None,
            line_shift: None,
        });
    }

    let html = std::fs::read_to_string(original_path).with_context(|| {
        format!(
            "failed to read HTML effect file while preparing runtime source: {}",
            original_path.display()
        )
    })?;

    let preamble =
        build_control_preamble_script(controls, host_driven_animation, display_descriptor)?;
    let base_tag = original_path
        .parent()
        .and_then(|parent| Url::from_directory_path(parent).ok())
        .map_or_else(String::new, |url| format!("<base href=\"{url}\">\n"));
    let injected_block = format!("{base_tag}<script>\n{preamble}\n</script>\n");
    let (runtime_html, line_shift) = inject_runtime_head_block(&html, &injected_block);

    let cache_root = crate::config::paths::servo_runtime_cache_dir();
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

    Ok(RuntimeHtmlSource {
        path: runtime_path.clone(),
        cleanup_path: Some(runtime_path),
        line_shift: Some(line_shift),
    })
}

/// Remove a temporary runtime HTML file once its page is done with.
pub(in crate::effect::servo) fn cleanup_runtime_html_path(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        debug!(
            path = %path.display(),
            %error,
            "Failed to remove temporary runtime HTML source"
        );
    }
}

fn build_control_preamble_script(
    controls: &HashMap<String, ControlValue>,
    host_driven_animation: bool,
    display_descriptor: Option<&DisplayDescriptor>,
) -> Result<String> {
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
        let value_literal = control_js_literal(value)
            .with_context(|| format!("control '{name}' cannot enter the effect runtime"))?;
        let _ = writeln!(
            script,
            "  if (typeof globalThis[{key_literal}] === 'undefined') globalThis[{key_literal}] = {};",
            value_literal
        );
    }
    script.push_str("})();");
    Ok(script)
}

/// Splice `block` into `html` after the `<head>` tag, before the first
/// `<script>` when there is no head, or ahead of everything otherwise.
///
/// Also reports where the block landed so console locations in the result
/// can be mapped back to `html`.
fn inject_runtime_head_block(html: &str, block: &str) -> (String, RuntimeLineShift) {
    let lowered = html.to_ascii_lowercase();
    let block_lines = block.matches('\n').count();

    let insert_at = lowered
        .find("<head")
        .and_then(|head_start| {
            lowered[head_start..]
                .find('>')
                .map(|head_close_offset| head_start + head_close_offset + 1)
        })
        .or_else(|| lowered.find("<script"));

    if let Some(insert_at) = insert_at {
        let (before, after) = html.split_at(insert_at);
        let shift = RuntimeLineShift {
            insert_line: before.matches('\n').count() + 1,
            // The leading newline ends the author's line before the block.
            inserted_lines: block_lines + 1,
        };
        return (format!("{before}\n{block}{after}"), shift);
    }

    let shift = RuntimeLineShift {
        insert_line: 0,
        inserted_lines: block_lines,
    };
    (format!("{block}{html}"), shift)
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
        controls.insert("enabled".to_owned(), ControlValue::Bool(true));
        controls.insert("color".to_owned(), ControlValue::Text("#00ffaa".to_owned()));
        controls.insert(
            "overlay".to_owned(),
            ControlValue::ColorLinear(
                hypercolor_color::LinearRgba::from_hex_srgb("#4080bf")
                    .expect("test color should parse"),
            ),
        );

        let script = build_control_preamble_script(&controls, false, None)
            .expect("valid controls should build a preamble");

        assert!(script.contains("globalThis[\"speed\"] = 42"));
        assert!(script.contains("globalThis[\"enabled\"] = true"));
        assert!(script.contains("globalThis[\"color\"] = \"#00ffaa\""));
        assert!(script.contains("globalThis[\"overlay\"] = \"#4080bf\""));
        assert!(script.contains("window.__hypercolorCaptureMode = true"));
        assert!(script.contains("window.__hypercolorPreserveDrawingBuffer = false"));
        assert!(script.contains("globalThis.__hypercolorCaptureMode = true"));
        assert!(script.contains("globalThis.__hypercolorPreserveDrawingBuffer = false"));
        assert!(!script.contains("__hypercolorHostDrivenAnimation"));
    }

    #[test]
    fn control_preamble_rejects_effect_integer_overflow() {
        let controls = HashMap::from([(
            "count".to_owned(),
            ControlValue::Int(i64::from(i32::MAX) + 1),
        )]);

        let error = build_control_preamble_script(&controls, false, None)
            .expect_err("out-of-range controls must not enter JavaScript");
        assert!(error.to_string().contains("control 'count'"));
    }

    #[test]
    fn control_preamble_marks_host_driven_animation_before_effect_script_runs() {
        let controls = HashMap::new();

        let script = build_control_preamble_script(&controls, true, None)
            .expect("valid controls should build a preamble");

        assert!(script.contains("window.__hypercolorHostDrivenAnimation = true"));
        assert!(script.contains("globalThis.__hypercolorHostDrivenAnimation = true"));
    }

    #[test]
    fn inject_runtime_block_prefers_head_tag() {
        let html = "<html><head><title>x</title></head><body><script>run()</script></body></html>";
        let block = "<script>bootstrap()</script>\n";

        let (injected, _) = inject_runtime_head_block(html, block);
        let expected = "<html><head>\n<script>bootstrap()</script>\n<title>x</title></head>";
        assert!(injected.contains(expected));
    }

    #[test]
    fn inject_runtime_block_falls_back_to_first_script() {
        let html = "<body><script>run()</script></body>";
        let block = "<script>bootstrap()</script>\n";

        let (injected, _) = inject_runtime_head_block(html, block);
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
        let prepared = prepare_runtime_html_source(&html_path, &controls, true, None, true)
            .expect("runtime html should build");
        let runtime_path = prepared.path;

        assert_ne!(runtime_path, html_path);
        assert_eq!(
            prepared.cleanup_path.as_deref(),
            Some(runtime_path.as_path())
        );
        assert!(prepared.line_shift.is_some());

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
        let runtime_path =
            prepare_runtime_html_source(&html_path, &controls, true, Some(&descriptor), true)
                .expect("runtime html should build")
                .path;

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
        let runtime_path = prepare_runtime_html_source(&html_path, &controls, false, None, true)
            .expect("runtime html should build")
            .path;

        let runtime_html =
            std::fs::read_to_string(&runtime_path).expect("runtime html should be readable");
        assert!(!runtime_html.contains("window.hypercolor.display"));
    }

    #[test]
    fn prepare_runtime_html_source_loads_verbatim_without_engine_globals() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let html_path = temp.path().join("page.html");
        let source = "<html><head></head><body></body></html>";
        std::fs::write(&html_path, source).expect("html write should work");

        let mut controls = HashMap::new();
        controls.insert("speed".to_owned(), ControlValue::Float(0.5));
        let prepared = prepare_runtime_html_source(&html_path, &controls, true, None, false)
            .expect("verbatim load should succeed");
        let runtime_path = prepared.path;

        assert_eq!(runtime_path, html_path);
        assert!(
            prepared.cleanup_path.is_none(),
            "a verbatim load writes no temporary runtime file"
        );
        assert!(
            prepared.line_shift.is_none(),
            "verbatim pages keep their lines"
        );
        assert_eq!(
            std::fs::read_to_string(&runtime_path).expect("source should be readable"),
            source
        );
    }

    /// Every author line the runtime copy still contains must map back to
    /// itself, and every injected line must map to nothing.
    fn assert_line_shift_round_trips(html: &str, block: &str) {
        let (runtime, shift) = inject_runtime_head_block(html, block);
        let author_lines: Vec<&str> = html.lines().collect();
        let runtime_lines: Vec<&str> = runtime.lines().collect();
        let mut mapped = 0;
        for (index, runtime_line) in runtime_lines.iter().enumerate() {
            let runtime_number = index + 1;
            match shift.original_line(runtime_number) {
                Some(author_number) if author_number == shift.insert_line => {
                    // The author's insertion line is split around the block.
                    assert!(author_lines[author_number - 1].contains(runtime_line.trim()));
                }
                Some(author_number) => {
                    assert_eq!(
                        *runtime_line,
                        author_lines[author_number - 1],
                        "runtime line {runtime_number} should be author line {author_number}"
                    );
                    mapped += 1;
                }
                None => assert!(
                    block.lines().any(|line| line == *runtime_line) || runtime_line.is_empty(),
                    "unmapped runtime line {runtime_number} should come from the block: {runtime_line:?}"
                ),
            }
        }
        assert!(mapped > 0, "some author lines should survive unchanged");
    }

    #[test]
    fn line_shift_maps_runtime_lines_after_head_back_to_the_author() {
        let html = "<!doctype html>\n<html>\n<head><title>x</title>\n</head>\n<body>\n<script>\nthrow new Error('line 7');\n</script>\n</body>\n</html>\n";
        let block = "<base href=\"file:///fx/\">\n<script>\n(function(){\n  globalThis.speed = 1;\n})();\n</script>\n";
        assert_line_shift_round_trips(html, block);

        let (_, shift) = inject_runtime_head_block(html, block);
        assert_eq!(shift.insert_line, 3);
        assert_eq!(shift.original_line(1), Some(1));
        assert_eq!(shift.original_line(4), None, "first injected line");
        let throw_runtime_line = 7 + shift.inserted_lines;
        assert_eq!(shift.original_line(throw_runtime_line), Some(7));
    }

    #[test]
    fn line_shift_handles_script_and_prefix_insertions() {
        let block = "<script>\nboot();\n</script>\n";
        assert_line_shift_round_trips(
            "<body>\n<p>hi</p>\n<script>run()</script>\n</body>\n",
            block,
        );

        let (_, prefix) = inject_runtime_head_block("plain\ntext\n", block);
        assert_eq!(prefix.insert_line, 0);
        assert_eq!(prefix.original_line(3), None, "the block's last line");
        assert_eq!(prefix.original_line(4), Some(1));
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
            input_reactive: false,
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
            input_reactive: false,
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
            input_reactive: false,
            source: EffectSource::Html {
                path: PathBuf::from("effects/electric-colors.html"),
            },
            license: None,
        };

        assert!(!effect_is_audio_reactive(&metadata));
    }
}
