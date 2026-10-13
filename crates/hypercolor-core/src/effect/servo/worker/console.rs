use std::any::Any;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use reqwest::Url;

use super::super::delegate::ConsoleMessage;
use crate::effect::console::EffectConsoleSource;

const CONSOLE_SNIPPET_RADIUS: usize = 1;
const CONSOLE_SNIPPET_LINE_MAX_CHARS: usize = 180;

/// Render the list of console messages as human-readable summaries.
pub(super) fn summarize_console_messages(
    messages: &[ConsoleMessage],
    fallback_source: Option<&Path>,
) -> Vec<String> {
    messages
        .iter()
        .map(|entry| format_console_message(entry, fallback_source))
        .collect()
}

/// Format a single console message, annotating it with source context when
/// the message mentions a `file://` URL we can locate on disk.
pub(super) fn format_console_message(
    entry: &ConsoleMessage,
    fallback_source: Option<&Path>,
) -> String {
    let mut formatted = format!("{}: {}", entry.level, entry.message);
    if let Some(location) = parse_console_source_location(&entry.message, fallback_source) {
        let _ = write!(
            formatted,
            " [{}:{}]",
            location.path.display(),
            location.line_number
        );
        if let Some(snippet) =
            load_source_snippet(&location.path, location.line_number, CONSOLE_SNIPPET_RADIUS)
        {
            let _ = write!(formatted, " {snippet}");
        }
    }
    formatted
}

/// Point a console message from a runtime copy at the author's file.
///
/// Servo names a location either as `file:///page.html:12:5`, as the page's
/// base directory with a `:12:5` suffix, or, for inline script failures, as
/// the base directory followed by `, 12`. Every location in the message that
/// names the effect's own page (a stack trace has several) becomes the
/// author's file with the line mapped past the injected preamble. Returns the
/// first author line the message names. Locations in other files, and lines
/// inside the preamble, pass through unchanged.
pub(in crate::effect::servo) fn remap_console_message(
    message: &str,
    source: &EffectConsoleSource,
) -> (String, Option<usize>) {
    let Ok(source_url) = Url::from_file_path(&source.source_path) else {
        return (message.to_owned(), None);
    };
    let mut remapped = String::with_capacity(message.len());
    let mut first_line = None;
    let mut rest = message;
    while let Some(start) = rest.find("file://") {
        remapped.push_str(&rest[..start]);
        let tail = &rest[start..];
        let token = &tail[..file_url_token_len(tail)];
        let after = &tail[token.len()..];
        if let Some(location) = remap_location(token, after, source, &source_url) {
            remapped.push_str(&location.text);
            if first_line.is_none() {
                first_line = location.line;
            }
            rest = &after[location.consumed_after..];
        } else {
            remapped.push_str(token);
            rest = after;
        }
    }
    remapped.push_str(rest);
    (remapped, first_line)
}

/// One rewritten location: the text that replaces the token (and any
/// `, line` that followed it), how much of the following text it consumed,
/// and the author line it names.
struct RemappedLocation {
    text: String,
    consumed_after: usize,
    line: Option<usize>,
}

fn remap_location(
    token: &str,
    after: &str,
    source: &EffectConsoleSource,
    source_url: &Url,
) -> Option<RemappedLocation> {
    let token_path = file_url_token_to_path(token)?;
    let is_directory = file_url_token_points_to_directory(token);
    let names_page = if is_directory {
        source.source_path.parent() == Some(token_path.as_path())
    } else {
        token_path == source.runtime_path || token_path == source.source_path
    };
    if !names_page {
        return None;
    }

    let suffix = trailing_colon_numbers(token);
    if let [.., line, column] = suffix.as_slice() {
        let author_line = source.original_line(*line)?;
        return Some(RemappedLocation {
            text: format!("{source_url}:{author_line}:{column}"),
            consumed_after: 0,
            line: Some(author_line),
        });
    }
    if let [line] = suffix.as_slice() {
        let author_line = source.original_line(*line)?;
        return Some(RemappedLocation {
            text: format!("{source_url}:{author_line}"),
            consumed_after: 0,
            line: Some(author_line),
        });
    }

    if let Some((separator, runtime_line, digits_len)) = comma_line_after(after) {
        let author_line = source.original_line(runtime_line)?;
        return Some(RemappedLocation {
            text: format!("{source_url}{separator}{author_line}"),
            consumed_after: separator.len() + digits_len,
            line: Some(author_line),
        });
    }

    // A bare mention of the runtime copy still names the author's file; a
    // bare mention of the base directory is not a location at all.
    (!is_directory).then(|| RemappedLocation {
        text: source_url.to_string(),
        consumed_after: 0,
        line: None,
    })
}

/// Byte length of the `file://` token at the start of `tail`.
fn file_url_token_len(tail: &str) -> usize {
    tail.find(|ch: char| ['"', '\'', ')', '(', ',', ' ', '\t', '\n', '\r'].contains(&ch))
        .unwrap_or(tail.len())
}

/// Parse the `", 12)` that follows a location token in Servo's
/// `Error("message", "file:///dir/", 12)` shape.
///
/// Returns the separator text before the digits, the line, and the digit
/// count. Only an optional closing quote, a comma, and spaces may sit between
/// the token and the digits, and the digits must end the message or close a
/// bracket, so ordinary prose such as `, took 40 ms` is never read as a line.
fn comma_line_after(after: &str) -> Option<(&str, usize, usize)> {
    let mut index = 0;
    let bytes = after.as_bytes();
    if matches!(bytes.first(), Some(b'"' | b'\'')) {
        index += 1;
    }
    while bytes.get(index) == Some(&b' ') {
        index += 1;
    }
    if bytes.get(index) != Some(&b',') {
        return None;
    }
    index += 1;
    while bytes.get(index) == Some(&b' ') {
        index += 1;
    }
    let digits_len = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits_len == 0 {
        return None;
    }
    if !matches!(bytes.get(index + digits_len), None | Some(b')' | b']')) {
        return None;
    }
    let line = after[index..index + digits_len].parse().ok()?;
    Some((&after[..index], line, digits_len))
}

/// Line and column numbers trailing a `file://` token, in reading order:
/// `file:///x.html:12:5` yields `[12, 5]`.
fn trailing_colon_numbers(token: &str) -> Vec<usize> {
    let mut remaining = token;
    let mut numbers = Vec::new();
    while let Some((prefix, value)) = split_trailing_colon_number(remaining) {
        numbers.push(value);
        remaining = prefix;
    }
    numbers.reverse();
    numbers
}

/// Scan a batch of recent console messages for an initialization-failure
/// marker emitted by the effect runtime.
pub(super) fn find_initialization_failure_message(
    messages: &[ConsoleMessage],
) -> Option<&ConsoleMessage> {
    messages.iter().rev().find(|entry| {
        let is_error_level = matches!(entry.level.as_str(), "error" | "warn");
        let lower = entry.message.to_ascii_lowercase();
        is_error_level
            && (lower.contains("initialization failed") || lower.contains("failed to initialize"))
    })
}

pub(super) fn panic_payload_message(payload: &(dyn Any + Send + 'static)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        return (*message).to_owned();
    }
    "unknown panic payload".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConsoleSourceLocation {
    path: PathBuf,
    line_number: usize,
}

fn parse_console_source_location(
    message: &str,
    fallback_source: Option<&Path>,
) -> Option<ConsoleSourceLocation> {
    let url_token = extract_file_url_token(message);
    let line_number = url_token
        .as_deref()
        .and_then(extract_line_from_file_url_token)
        .or_else(|| extract_line_after_file_url_token(message, url_token.as_deref()))
        .or_else(|| extract_line_word_pattern(message))?;
    if line_number == 0 {
        return None;
    }

    let candidate_path = url_token.as_deref().and_then(|token| {
        let path = file_url_token_to_path(token)?;
        if file_url_token_points_to_directory(token) || path.is_dir() {
            return None;
        }
        Some(path)
    });
    let path = candidate_path.or_else(|| fallback_source.map(Path::to_path_buf))?;
    Some(ConsoleSourceLocation { path, line_number })
}

fn extract_file_url_token(message: &str) -> Option<String> {
    let start = message.find("file://")?;
    let tail = &message[start..];
    Some(tail[..file_url_token_len(tail)].to_owned())
}

fn extract_line_from_file_url_token(token: &str) -> Option<usize> {
    let mut remaining = token;
    let mut trailing_numbers: Vec<usize> = Vec::new();

    while let Some((prefix, value)) = split_trailing_colon_number(remaining) {
        trailing_numbers.push(value);
        remaining = prefix;
    }

    if trailing_numbers.is_empty() {
        return None;
    }

    // file:///path.js:123:45 -> trailing_numbers == [45, 123]
    if trailing_numbers.len() >= 2 {
        return Some(trailing_numbers[1]);
    }
    Some(trailing_numbers[0])
}

fn split_trailing_colon_number(input: &str) -> Option<(&str, usize)> {
    let (prefix, suffix) = input.rsplit_once(':')?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = suffix.parse::<usize>().ok()?;
    Some((prefix, value))
}

fn extract_line_after_file_url_token(message: &str, token: Option<&str>) -> Option<usize> {
    let token = token?;
    let start = message.find(token)?;
    let rest = &message[start + token.len()..];
    let comma = rest.find(',')?;
    parse_first_number(&rest[comma + 1..])
}

fn parse_first_number(input: &str) -> Option<usize> {
    let start = input.find(|ch: char| ch.is_ascii_digit())?;
    let digits = input[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse::<usize>().ok()
}

fn extract_line_word_pattern(message: &str) -> Option<usize> {
    let lower = message.to_ascii_lowercase();
    let marker = "line ";
    let start = lower.find(marker)?;
    parse_first_number(&lower[start + marker.len()..])
}

fn file_url_token_to_path(token: &str) -> Option<PathBuf> {
    let trimmed = strip_file_url_line_suffix(token);
    let parsed = Url::parse(trimmed).ok()?;
    if parsed.scheme() != "file" {
        return None;
    }
    parsed.to_file_path().ok()
}

fn strip_file_url_line_suffix(token: &str) -> &str {
    let mut trimmed = token;
    while let Some((prefix, _)) = split_trailing_colon_number(trimmed) {
        trimmed = prefix;
    }
    trimmed
}

fn file_url_token_points_to_directory(token: &str) -> bool {
    strip_file_url_line_suffix(token).ends_with('/')
}

fn load_source_snippet(path: &Path, line_number: usize, radius: usize) -> Option<String> {
    if line_number == 0 {
        return None;
    }

    let contents = std::fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = contents.lines().collect();
    if line_number > lines.len() {
        return None;
    }

    let start = line_number.saturating_sub(radius).max(1);
    let end = line_number.saturating_add(radius).min(lines.len());
    let mut window = Vec::with_capacity(end.saturating_sub(start) + 1);
    for idx in start..=end {
        let marker = if idx == line_number { ">" } else { "-" };
        let content = truncate_for_log(lines[idx - 1].trim(), CONSOLE_SNIPPET_LINE_MAX_CHARS);
        window.push(format!("{marker}{idx}: {content}"));
    }
    Some(window.join(" || "))
}

pub(super) fn truncate_for_log(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        return format!("{truncated}...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::effect::console::RuntimeLineShift;

    use super::*;

    fn shifted_source() -> EffectConsoleSource {
        EffectConsoleSource {
            effect_name: "Ember".to_owned(),
            source_path: PathBuf::from("/fx/ember.html"),
            runtime_path: PathBuf::from("/cache/effect-0001.html"),
            // Ten preamble lines after author line 3.
            line_shift: Some(RuntimeLineShift {
                insert_line: 3,
                inserted_lines: 10,
            }),
        }
    }

    #[test]
    fn remap_points_runtime_locations_at_the_author_file() {
        let (message, line) = remap_console_message(
            "TypeError: ctx is null at file:///cache/effect-0001.html:52:9",
            &shifted_source(),
        );
        assert_eq!(line, Some(42));
        assert_eq!(
            message,
            "TypeError: ctx is null at file:///fx/ember.html:42:9"
        );
    }

    #[test]
    fn remap_maps_base_directory_locations_for_inline_scripts() {
        let (message, line) = remap_console_message(
            r#"Initialization failed (new TypeError("Ge is not a function", "file:///fx/", 25))"#,
            &shifted_source(),
        );
        assert_eq!(line, Some(15));
        assert_eq!(
            message,
            r#"Initialization failed (new TypeError("Ge is not a function", "file:///fx/ember.html", 15))"#
        );
    }

    #[test]
    fn remap_leaves_other_files_and_preamble_lines_alone() {
        let source = shifted_source();
        let other = "ReferenceError at file:///fx/lib/noise.js:7:1";
        assert_eq!(
            remap_console_message(other, &source),
            (other.to_owned(), None)
        );

        let preamble = "SyntaxError at file:///cache/effect-0001.html:6:2";
        assert_eq!(
            remap_console_message(preamble, &source),
            (preamble.to_owned(), None)
        );

        let plain = "speed changed to 0.4";
        assert_eq!(
            remap_console_message(plain, &source),
            (plain.to_owned(), None)
        );
    }

    #[test]
    fn remap_rewrites_every_page_location_in_a_stack_trace() {
        let (message, line) = remap_console_message(
            "inner@file:///cache/effect-0001.html:27:27\nouter@file:///fx/:28:27\nloop@file:///cache/effect-0001.html:60:3\nnoise@file:///fx/lib/noise.js:9:1",
            &shifted_source(),
        );
        assert_eq!(line, Some(17), "the first page location names the line");
        assert_eq!(
            message,
            "inner@file:///fx/ember.html:17:27\nouter@file:///fx/ember.html:18:27\nloop@file:///fx/ember.html:50:3\nnoise@file:///fx/lib/noise.js:9:1"
        );
    }

    #[test]
    fn remap_never_reads_prose_numbers_as_lines() {
        let source = shifted_source();
        for message in [
            "href file:///fx/ember.html, fps 60",
            "loaded file:///fx/ember.html, took 40 ms",
            "canvas file:///fx/ember.html 640, 480",
            "dir file:///fx/, 480 px",
        ] {
            assert_eq!(
                remap_console_message(message, &source),
                (message.to_owned(), None),
                "numbers in prose must survive untouched"
            );
        }
        let (bare, _) = remap_console_message("loaded file:///cache/effect-0001.html", &source);
        assert_eq!(
            bare, "loaded file:///fx/ember.html",
            "the runtime copy is never named"
        );
    }

    #[test]
    fn remap_keeps_verbatim_pages_on_their_own_lines() {
        let source = EffectConsoleSource::verbatim("Page", "/fx/page.html");
        let (message, line) = remap_console_message("Error at file:///fx/page.html:12:3", &source);
        assert_eq!(line, Some(12));
        assert_eq!(message, "Error at file:///fx/page.html:12:3");
    }

    #[test]
    fn extracts_line_number_from_quoted_file_url_tuple_pattern() {
        let message = r#"Initialization failed (new TypeError("Ge is not a function", "file:///tmp/effects/custom/", 10585))"#;
        let token = extract_file_url_token(message).expect("file URL token");
        assert_eq!(extract_line_from_file_url_token(&token), None);
        assert_eq!(
            extract_line_after_file_url_token(message, Some(&token)),
            Some(10585)
        );
    }

    #[test]
    fn parses_location_with_fallback_when_console_url_is_directory() {
        let fallback = Path::new("/tmp/runtime-effect.html");
        let location = parse_console_source_location(
            r#"Failed to initialize: TypeError("boom", "file:///tmp/effects/custom/", 42)"#,
            Some(fallback),
        )
        .expect("source location");

        assert_eq!(location.path, fallback);
        assert_eq!(location.line_number, 42);
    }

    #[test]
    fn source_snippet_formats_context_window() {
        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.js");
        std::fs::write(&source_path, "line1\nline2\nline3\nline4\nline5\n")
            .expect("write source file");

        let snippet = load_source_snippet(&source_path, 3, 1).expect("snippet");
        assert!(snippet.contains("-2: line2"));
        assert!(snippet.contains(">3: line3"));
        assert!(snippet.contains("-4: line4"));
    }

    #[test]
    fn format_console_message_includes_source_context() {
        let temp_dir = tempfile::tempdir().expect("temporary directory");
        let source_path = temp_dir.path().join("effect.js");
        std::fs::write(&source_path, "alpha\nbeta\ngamma\ndelta\n").expect("write source");

        let entry = ConsoleMessage {
            level: "error".to_owned(),
            message: r#"TypeError("boom", "file:///tmp/effects/custom/", 3)"#.to_owned(),
        };
        let formatted = format_console_message(&entry, Some(&source_path));
        assert!(formatted.contains("error: TypeError(\"boom\""));
        assert!(formatted.contains("effect.js:3"));
        assert!(formatted.contains(">3: gamma"));
    }
}
