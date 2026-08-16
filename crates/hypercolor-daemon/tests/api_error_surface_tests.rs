//! Source scans fencing the single error rendering (Spec 76 §2.1).
//!
//! `DomainError`'s `IntoResponse` is the daemon's only error rendering.
//! Two idioms let a second one grow back, and both are cheap to see from
//! the outside: a function that hands a `Response` back through a
//! `Result`, and a route that builds an error body by hand. These scans
//! are the fence. They are deliberately textual, because the type system
//! cannot express "no other error factory exists", and they carry their
//! own tests so a hole in the scan fails loudly rather than silently.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `src`, as `(relative path, code)`.
///
/// The whole crate is scanned, not just `src/api`: a second error factory
/// is no more welcome in `src/domain` or `src/mcp` than in a route module.
/// Comments and string literals are stripped, so a doc comment that names
/// a banned pattern in order to forbid it does not read as a sighting of
/// one.
fn daemon_sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    let mut pending: Vec<PathBuf> = vec![src.clone()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory reads") {
            let path = entry.expect("source entry reads").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let relative = path
                .strip_prefix(&src)
                .expect("paths are under src")
                .to_string_lossy()
                .replace('\\', "/");
            let source = std::fs::read_to_string(&path).expect("source file reads");
            sources.push((relative, strip_comments_and_strings(&source)));
        }
    }
    sources.sort();
    sources
}

/// Blank out `//` comments, `/* */` blocks, and string literals, leaving
/// byte offsets and line structure intact.
fn strip_comments_and_strings(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < chars.len() {
        let rest_is =
            |needle: &str| chars[index..].starts_with(&needle.chars().collect::<Vec<_>>());
        if rest_is("//") {
            while index < chars.len() && chars[index] != '\n' {
                out.push(' ');
                index += 1;
            }
        } else if rest_is("/*") {
            while index < chars.len() && !chars[index..].starts_with(&['*', '/']) {
                out.push(if chars[index] == '\n' { '\n' } else { ' ' });
                index += 1;
            }
            for _ in 0..2 {
                if index < chars.len() {
                    out.push(' ');
                    index += 1;
                }
            }
        } else if chars[index] == '"' {
            out.push(' ');
            index += 1;
            while index < chars.len() && chars[index] != '"' {
                if chars[index] == '\\' {
                    out.push(' ');
                    index += 1;
                }
                out.push(if chars[index] == '\n' { '\n' } else { ' ' });
                index += 1;
            }
            if index < chars.len() {
                out.push(' ');
                index += 1;
            }
        } else {
            out.push(chars[index]);
            index += 1;
        }
    }
    out
}

/// Collapse a signature onto one line so a `Result<` and its `Response>`
/// are visible together however rustfmt wrapped them.
fn flattened(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The error half of every `Result<_, _>` in a source file.
///
/// Angle brackets are balanced so a nested generic in the ok position
/// (`Result<Response<Body>, E>`) cannot be mistaken for an error type,
/// and so the split lands on the argument comma rather than one inside
/// `Box<Response>`.
fn result_error_types(source: &str) -> Vec<String> {
    let flat = flattened(source);
    let bytes: Vec<char> = flat.chars().collect();
    let mut types = Vec::new();
    let mut index = 0;
    while let Some(offset) = flat[index..].find("Result<") {
        let open = index + offset + "Result<".len();
        let mut depth = 1_i32;
        let mut split = None;
        let mut cursor = open;
        while cursor < bytes.len() && depth > 0 {
            match bytes[cursor] {
                '<' => depth += 1,
                '>' => depth -= 1,
                ',' if depth == 1 && split.is_none() => split = Some(cursor),
                _ => {}
            }
            cursor += 1;
        }
        if depth == 0
            && let Some(split) = split
        {
            let close = cursor - 1;
            types.push(flat[split + 1..close].trim().to_owned());
        }
        index = open;
    }
    types
}

/// Whether an error type is an Axum `Response` in any spelling.
///
/// `axum::response::Response` IS `http::Response<Body>`, so the generic
/// form has to count too, and the type can arrive under any path prefix
/// or through a `Box`.
fn is_response_type(error_type: &str) -> bool {
    let stripped = error_type
        .strip_prefix("Box<")
        .and_then(|inner| inner.strip_suffix('>'))
        .unwrap_or(error_type)
        .trim();
    // Drop a generic argument list, so `Response<Body>` reads as `Response`.
    let bare = stripped.split_once('<').map_or(stripped, |(head, _)| head);
    // Drop any path prefix, so `axum::response::Response` reads the same.
    let last = bare.rsplit("::").next().unwrap_or(bare).trim();
    last == "Response"
}

/// `Result<T, Response>` is how a second error rendering hides: the
/// `Response` is already built, so whatever produced it is an error
/// factory that `DomainError` does not own. Helpers return
/// `Result<T, DomainError>` and let the route `?` them.
#[test]
fn no_api_helper_returns_a_response_through_a_result() {
    let mut offenders: Vec<String> = Vec::new();
    for (relative, source) in daemon_sources() {
        for error_type in result_error_types(&source) {
            if is_response_type(&error_type) {
                offenders.push(format!("{relative}: Result<_, {error_type}>"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a Result must not carry a built Response as its error; these should \
         return DomainError instead:\n{}",
        offenders.join("\n")
    );
}

/// Whether `needle` appears as a whole identifier rather than as the tail
/// of a longer one — `TrustedLocalApiError` is a live type and must not
/// read as a sighting of `ApiError`.
fn contains_identifier(source: &str, needle: &str) -> bool {
    source.match_indices(needle).any(|(offset, _)| {
        offset == 0
            || !source[..offset]
                .chars()
                .next_back()
                .is_some_and(|previous| previous.is_alphanumeric() || previous == '_')
    })
}

/// The deleted machinery, by name.
///
/// `ApiError` was the `Response` factory, `into_v1_response` and
/// `domain::legacy` were the transitional projections that reproduced the
/// pre-canonical shapes. All three are gone; a reference to any of them
/// means one grew back.
#[test]
fn the_deleted_error_machinery_stays_deleted() {
    const BANNED: [&str; 4] = [
        "ApiError::",
        "into_v1_response",
        "domain::legacy",
        "scene_family_error_response",
    ];

    let mut offenders: Vec<String> = Vec::new();
    for (relative, source) in daemon_sources() {
        for banned in BANNED {
            if contains_identifier(&source, banned) {
                offenders.push(format!("{relative}: {banned}"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these names were deleted in the error-surface flip:\n{}",
        offenders.join("\n")
    );
}

#[cfg(test)]
mod scan_self_tests {
    use super::{is_response_type, result_error_types, strip_comments_and_strings};

    #[test]
    fn every_spelling_of_an_axum_response_error_type_is_caught() {
        for spelling in [
            "Response",
            "Box<Response>",
            "Response<Body>",
            "axum::response::Response",
            "axum::http::Response<Body>",
            "response::Response",
            "crate::api::Response",
            "Box<axum::response::Response>",
        ] {
            assert!(
                is_response_type(spelling),
                "`{spelling}` names an Axum Response and must be caught"
            );
        }
    }

    #[test]
    fn types_that_merely_resemble_a_response_are_left_alone() {
        for spelling in [
            "DomainError",
            "TrustedLocalApiError",
            "ResponseError",
            "MyResponseKind",
            "String",
        ] {
            assert!(
                !is_response_type(spelling),
                "`{spelling}` is not an Axum Response"
            );
        }
    }

    #[test]
    fn the_error_half_is_read_past_nested_generics() {
        assert_eq!(
            result_error_types("fn f() -> Result<Response<Body>, TrustedLocalApiError> {"),
            vec!["TrustedLocalApiError".to_owned()],
            "a Response in the ok position is not an error type"
        );
        assert_eq!(
            result_error_types("fn f() -> Result<Vec<(String, u64)>, Box<Response>> {"),
            vec!["Box<Response>".to_owned()],
            "the split lands on the argument comma, not one inside a generic"
        );
    }

    #[test]
    fn prose_that_names_a_banned_pattern_is_not_a_sighting_of_one() {
        let source = r#"
            // Result<T, Response> is exactly what this forbids.
            /* ApiError::not_found was deleted. */
            let message = "Result<T, Response> and ApiError:: are gone";
            fn real() -> Result<u8, DomainError> { Ok(0) }
        "#;
        let code = strip_comments_and_strings(source);
        assert!(
            !code.contains("ApiError::"),
            "comments and strings are blanked"
        );
        assert_eq!(
            result_error_types(&code),
            vec!["DomainError".to_owned()],
            "only real signatures survive the strip"
        );
    }
}
