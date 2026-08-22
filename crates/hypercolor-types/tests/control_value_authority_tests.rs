use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CANONICAL_DEFINITION: &str = "hypercolor-types/src/control/mod.rs";
const FENCE_SOURCE: &str = "hypercolor-types/tests/control_value_authority_tests.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("types crate lives under the workspace crates directory")
        .to_path_buf()
}

fn sources_under(root: &Path, extension: &str) -> Vec<(String, String)> {
    let mut pending = vec![root.to_path_buf()];
    let mut sources = Vec::new();

    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("source directory reads") {
            let path = entry.expect("source entry reads").path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    pending.push(path);
                }
                continue;
            }
            if path
                .extension()
                .is_none_or(|candidate| candidate != extension)
            {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("source remains under root")
                .to_string_lossy()
                .replace('\\', "/");
            let source = std::fs::read_to_string(&path).expect("source file reads");
            sources.push((relative, source));
        }
    }

    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

fn rust_sources() -> Vec<(String, String)> {
    sources_under(&workspace_root().join("crates"), "rs")
        .into_iter()
        .filter(|(path, _)| path != FENCE_SOURCE)
        .collect()
}

fn imports_legacy_control_value(source: &str) -> bool {
    const LEGACY_MODULES: [&str; 4] = [
        "hypercolor_types::effect",
        "hypercolor_types::controls",
        "crate::effect",
        "crate::controls",
    ];

    source.split(';').any(|statement| {
        let compact = compact(statement);
        LEGACY_MODULES.iter().any(|module| {
            let direct = format!("use{module}::ControlValue");
            if compact == direct || compact.starts_with(&format!("{direct}as")) {
                return true;
            }

            let group = format!("use{module}::{{");
            compact.strip_prefix(&group).is_some_and(|items| {
                items.strip_suffix('}').is_some_and(|items| {
                    items
                        .split(',')
                        .any(|item| item == "ControlValue" || item.starts_with("ControlValueas"))
                })
            })
        })
    })
}

fn compact(source: &str) -> String {
    source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn declarations_after<'a>(source: &'a str, keyword: &str) -> Vec<(String, &'a str)> {
    let mut declarations = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = source[cursor..].find(keyword) {
        let start = cursor + relative_start;
        let before = source[..start].chars().next_back();
        let after_keyword = start + keyword.len();
        let after = source[after_keyword..].chars().next();
        if before.is_some_and(is_identifier_character) || after.is_some_and(is_identifier_character)
        {
            cursor = after_keyword;
            continue;
        }

        let name_start = source[after_keyword..]
            .find(|character: char| !character.is_whitespace())
            .map_or(after_keyword, |offset| after_keyword + offset);
        let name_end = source[name_start..]
            .find(|character: char| !is_identifier_character(character))
            .map_or(source.len(), |offset| name_start + offset);
        if name_start == name_end {
            cursor = after_keyword;
            continue;
        }

        let Some(relative_open) = source[name_end..].find('{') else {
            break;
        };
        let open = name_end + relative_open;
        if source[name_end..open].contains(';') {
            cursor = open + 1;
            continue;
        }
        let Some(close) = matching_brace(source, open) else {
            break;
        };
        declarations.push((
            source[name_start..name_end].to_owned(),
            &source[start..=close],
        ));
        cursor = close + 1;
    }

    declarations
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn matching_brace(source: &str, open: usize) -> Option<usize> {
    let mut depth = 0_u32;
    for (offset, character) in source[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn disallowed_control_value_enums(path: &str, source: &str) -> Vec<String> {
    declarations_after(source, "enum")
        .into_iter()
        .filter_map(|(name, declaration)| {
            if allowed_control_value_enum(path, &name) {
                return None;
            }
            let semantic_variants = identifier_tokens(declaration)
                .intersection(&control_value_variant_names())
                .count();
            (name.contains("ControlValue") || (name.ends_with("Value") && semantic_variants >= 6))
                .then_some(name)
        })
        .collect()
}

fn allowed_control_value_enum(path: &str, name: &str) -> bool {
    matches!(
        (path, name),
        (
            "hypercolor-types/src/control/mod.rs",
            "ControlValue" | "ControlValueRef" | "ControlValueWire" | "ControlValueInvalid"
        ) | (
            "hypercolor-types/src/controls.rs",
            "ControlValueType" | "ControlValueKind" | "ControlValueValidationError"
        )
    )
}

fn identifier_tokens(source: &str) -> BTreeSet<&str> {
    source
        .split(|character: char| !is_identifier_character(character))
        .filter(|token| !token.is_empty())
        .collect()
}

fn control_value_variant_names() -> BTreeSet<&'static str> {
    [
        "Null",
        "Bool",
        "Boolean",
        "Int",
        "Integer",
        "Float",
        "Text",
        "String",
        "Enum",
        "Color",
        "ColorRgb",
        "ColorRgba",
        "Gradient",
        "Rect",
        "List",
        "Map",
        "Object",
        "Duration",
        "SecretRef",
        "Ip",
        "Mac",
        "Unknown",
    ]
    .into_iter()
    .collect()
}

fn manual_json_authorities(path: &str, source: &str) -> Vec<String> {
    if path.contains("/tests/") {
        return Vec::new();
    }

    declarations_after(source, "fn")
        .into_iter()
        .filter_map(|(name, declaration)| {
            let normalized = name.to_ascii_lowercase();
            if !normalized.contains("control_value") || !normalized.contains("json") {
                return None;
            }

            let compact_declaration = compact(declaration);
            let delegates_to_canonical = [
                "ControlValue::try_from_effect_json",
                ".try_to_effect_json()",
                "serde_json::from_str::<ControlValue>",
                "serde_json::from_value::<ControlValue>",
            ]
            .iter()
            .any(|marker| compact_declaration.contains(&compact(marker)));
            let schema_aware_adapter = compact_declaration.contains("ControlValueType");
            let driver_settings_adapter = path == "hypercolor-driver-api/src/config.rs"
                && normalized.ends_with("to_settings_json");

            (!delegates_to_canonical && !schema_aware_adapter && !driver_settings_adapter)
                .then_some(name)
        })
        .collect()
}

fn legacy_generated_models() -> Vec<String> {
    let root = workspace_root().join("python/src/hypercolor/_generated");
    sources_under(&root, "py")
        .into_iter()
        .filter_map(|(path, source)| {
            let normalized_path = path.to_ascii_lowercase();
            (normalized_path.contains("effect_control_value")
                || normalized_path.contains("driver_control_value")
                || source.contains("EffectControlValue")
                || source.contains("DriverControlValue"))
            .then_some(path)
        })
        .collect()
}

#[test]
fn control_value_has_one_definition_and_no_legacy_projection_paths() {
    let sources = rust_sources();
    let mut definitions = Vec::new();
    let mut legacy_paths = Vec::new();
    let mut mirror_enums = Vec::new();
    let mut manual_authorities = Vec::new();

    for (path, source) in &sources {
        if declarations_after(source, "enum")
            .iter()
            .any(|(name, _)| name == "ControlValue")
        {
            definitions.push(path.clone());
        }

        let compact_source = compact(source);
        if imports_legacy_control_value(source)
            || compact_source.contains("asCanonicalControlValue")
            || compact_source.contains("asDynamicControlValue")
            || compact_source.contains("EventControlValue")
            || compact_source.contains("to_effect_wire(")
            || compact_source.contains("to_driver_wire(")
            || compact_source.contains("ControlValueProjection")
            || compact_source.contains("ControlValueConversion")
        {
            legacy_paths.push(path.clone());
        }

        mirror_enums.extend(
            disallowed_control_value_enums(path, source)
                .into_iter()
                .map(|name| format!("{path}: {name}")),
        );
        manual_authorities.extend(
            manual_json_authorities(path, source)
                .into_iter()
                .map(|name| format!("{path}: {name}")),
        );
    }

    assert_eq!(definitions, [CANONICAL_DEFINITION]);
    assert!(
        legacy_paths.is_empty(),
        "legacy control-value paths remain in {legacy_paths:#?}"
    );
    assert!(
        mirror_enums.is_empty(),
        "control-value mirror enums remain in {mirror_enums:#?}"
    );
    assert!(
        manual_authorities.is_empty(),
        "manual JSON control-value authorities remain in {manual_authorities:#?}"
    );
    assert!(
        legacy_generated_models().is_empty(),
        "generated clients still contain retired control-value models"
    );
}

#[test]
fn authority_fence_detects_renamed_mirrors_and_manual_parsers() {
    let renamed_mirror = r"
        enum LightScriptValue {
            Float(f64), Integer(i64), Boolean(bool), String(String),
            Color(String), Gradient(Vec<String>), Rect([f64; 4]),
        }
    ";
    assert_eq!(
        disallowed_control_value_enums("fixture.rs", renamed_mirror),
        ["LightScriptValue"]
    );

    let manual_parser = r"
        fn json_to_control_value(value: serde_json::Value) -> ControlValue {
            match value { _ => ControlValue::Unknown }
        }
    ";
    assert_eq!(
        manual_json_authorities("fixture.rs", manual_parser),
        ["json_to_control_value"]
    );

    let canonical_adapter = r#"
        fn json_to_control_value(value: &serde_json::Value) -> ControlValue {
            ControlValue::try_from_effect_json(value).expect("fixture")
        }
    "#;
    assert!(manual_json_authorities("fixture.rs", canonical_adapter).is_empty());

    let schema_adapter = r"
        fn json_to_surface_control_value(
            value_type: &ControlValueType,
            value: serde_json::Value,
        ) -> ControlValue { value_type.admit(value) }
    ";
    assert!(manual_json_authorities("fixture.rs", schema_adapter).is_empty());
}
