use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hypercolor_types::controls::ControlValueKind;

const CANONICAL_DEFINITION: &str = "hypercolor-types/src/control/mod.rs";
const CANONICAL_CONTROL_SURFACE_LIST_QUERY: &str = "hypercolor-types/src/api/controls.rs";
const CANONICAL_CONTROL_SURFACE_LIST_RESPONSE: &str = "hypercolor-types/src/api/controls.rs";
const CANONICAL_WIRE: &str = "hypercolor-types/src/control/wire.rs";
const CANONICAL_KIND: &str = "hypercolor-types/src/controls.rs";
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
                if path.file_name().is_none_or(|name| {
                    !matches!(
                        name.to_string_lossy().as_ref(),
                        "target" | "node_modules" | ".venv" | "dist"
                    )
                }) {
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

fn python_sources() -> Vec<(String, String)> {
    sources_under(&workspace_root().join("python"), "py")
        .into_iter()
        .filter(|(path, _)| !path.starts_with("src/hypercolor/_generated/"))
        .map(|(path, source)| (format!("python/{path}"), source))
        .collect()
}

fn typescript_sources() -> Vec<(String, String)> {
    sources_under(&workspace_root().join("sdk"), "ts")
        .into_iter()
        .map(|(path, source)| (format!("sdk/{path}"), source))
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
            "ControlValue" | "ControlValueInvalid"
        ) | (
            "hypercolor-types/src/control/wire.rs",
            "ControlValueRef" | "ControlValueWire"
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
            if path == "hypercolor-types/src/control/wire.rs" && name == "control_value_json_schema"
            {
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

fn disallowed_control_surface_list_responses(path: &str, source: &str) -> Vec<String> {
    declarations_after(source, "struct")
        .into_iter()
        .filter(|(name, _)| {
            name == "ControlSurfaceListResponse" && path != CANONICAL_CONTROL_SURFACE_LIST_RESPONSE
        })
        .map(|(name, _)| name)
        .collect()
}

fn control_surface_query_fields(declaration: &str) -> BTreeSet<String> {
    declaration
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub "))
        .filter_map(|field| {
            field
                .split_once(':')
                .map(|(name, _)| name.trim().to_owned())
        })
        .collect()
}

fn disallowed_control_surface_queries(path: &str, source: &str) -> Vec<String> {
    let canonical_fields = BTreeSet::from([
        "device_id".to_owned(),
        "driver_id".to_owned(),
        "include_driver".to_owned(),
    ]);
    declarations_after(source, "struct")
        .into_iter()
        .filter_map(|(name, declaration)| {
            let fields = control_surface_query_fields(declaration);
            let mirrors_canonical_fields = fields == canonical_fields;
            ((name == "ControlSurfaceListQuery" && path != CANONICAL_CONTROL_SURFACE_LIST_QUERY)
                || (name != "ControlSurfaceListQuery" && mirrors_canonical_fields))
                .then_some(name)
        })
        .collect()
}

fn contains_retired_external_tag(source: &str) -> bool {
    let compact_source = compact(source);
    [
        "float", "integer", "boolean", "gradient", "enum", "text", "rect",
    ]
    .iter()
    .any(|tag| compact_source.contains(&format!(r#"{{"{tag}":"#)))
        || compact_source.contains(r#"{"color":["#)
}

fn retired_rust_external_tags(path: &str, source: &str) -> Vec<String> {
    declarations_after(source, "fn")
        .into_iter()
        .filter_map(|(name, declaration)| {
            if !contains_retired_external_tag(declaration) {
                return None;
            }
            let compact_declaration = compact(declaration);
            let explicit_rejection = compact_declaration.contains("retiredexternaltagshouldfail")
                && compact_declaration.contains("expect_err");
            (!explicit_rejection).then(|| format!("{path}: {name}"))
        })
        .collect()
}

fn retired_kind_tags(source: &str) -> Vec<&'static str> {
    let compact_source = compact(source);
    ["boolean", "duration_ms", "integer", "object", "string"]
        .into_iter()
        .filter(|kind| compact_source.contains(&format!(r#""kind":"{kind}""#)))
        .collect()
}

fn retired_value_block_tags(path: &str, source: &str) -> Vec<String> {
    let compact_source = compact(source);
    let mut findings = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = compact_source[cursor..].find(r#""values":{"#) {
        let open = cursor + relative + r#""values":"#.len();
        let Some(close) = matching_brace(&compact_source, open) else {
            break;
        };
        for kind in retired_kind_tags(&compact_source[open..=close]) {
            findings.push(format!("{path}: values uses {kind}"));
        }
        cursor = close + 1;
    }
    findings
}

fn python_mapper_violations(source: &str) -> Vec<&'static str> {
    let compact_source = compact(source);
    let mut violations = Vec::new();
    for (name, marker) in [
        (
            "exact tagged envelope keys",
            "_require_exact_keys(value,expected_keys,kind)",
        ),
        (
            "recursive explicit list admission",
            "[_canonical_control_value(item)foriteminpayload]",
        ),
        (
            "recursive explicit map admission",
            "{key:_canonical_control_value(item)forkey,iteminpayload.items()}",
        ),
        ("linear sRGB conversion", "_srgb_to_linear(int(color"),
    ] {
        if !compact_source.contains(marker) {
            violations.push(name);
        }
    }
    if compact_source.contains("result={str(key):itemforkey,iteminvalue.items()}") {
        violations.push("permissive tagged envelope copying");
    }
    if compact_source.contains("len(value)==4andall(isinstance(channel,int|float)") {
        violations.push("bare four-number color guessing");
    }
    violations
}

#[test]
fn control_value_has_one_definition_and_no_legacy_projection_paths() {
    let sources = rust_sources();
    let mut definitions = Vec::new();
    let mut legacy_paths = Vec::new();
    let mut mirror_enums = Vec::new();
    let mut manual_authorities = Vec::new();
    let mut response_mirrors = Vec::new();
    let mut query_mirrors = Vec::new();
    let mut retired_rust_tags = Vec::new();

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
        response_mirrors.extend(
            disallowed_control_surface_list_responses(path, source)
                .into_iter()
                .map(|name| format!("{path}: {name}")),
        );
        query_mirrors.extend(
            disallowed_control_surface_queries(path, source)
                .into_iter()
                .map(|name| format!("{path}: {name}")),
        );
        retired_rust_tags.extend(retired_rust_external_tags(path, source));
        retired_rust_tags.extend(retired_value_block_tags(path, source));
    }

    let retired_python_tags = python_sources()
        .into_iter()
        .flat_map(|(path, source)| {
            let mut findings = retired_kind_tags(&source)
                .into_iter()
                .map(|kind| format!("{path}: kind {kind}"))
                .collect::<Vec<_>>();
            if contains_retired_external_tag(&source) {
                findings.push(format!("{path}: externally tagged value"));
            }
            findings
        })
        .collect::<Vec<_>>();
    let python_client =
        std::fs::read_to_string(workspace_root().join("python/src/hypercolor/client.py"))
            .expect("Python client source reads");
    let retired_typescript_tags = typescript_sources()
        .into_iter()
        .filter_map(|(path, source)| {
            (contains_retired_external_tag(&source)
                || source.contains("unwrapControlValue")
                || source.contains("isControlValueTag"))
            .then_some(path)
        })
        .collect::<Vec<_>>();

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
    assert!(
        response_mirrors.is_empty(),
        "control-surface list response mirrors remain in {response_mirrors:#?}"
    );
    assert!(
        query_mirrors.is_empty(),
        "control-surface list query mirrors remain in {query_mirrors:#?}"
    );

    let canonical_query_source = sources
        .iter()
        .find(|(path, _)| path == CANONICAL_CONTROL_SURFACE_LIST_QUERY)
        .map(|(_, source)| source)
        .expect("canonical control-surface query source should exist");
    let canonical_query = declarations_after(canonical_query_source, "struct")
        .into_iter()
        .find(|(name, _)| name == "ControlSurfaceListQuery")
        .map(|(_, declaration)| declaration)
        .expect("canonical control-surface query should exist");
    assert_eq!(
        control_surface_query_fields(canonical_query),
        BTreeSet::from([
            "device_id".to_owned(),
            "driver_id".to_owned(),
            "include_driver".to_owned(),
        ]),
        "canonical control-surface query field set drifted"
    );
    assert!(
        retired_rust_tags.is_empty(),
        "retired Rust control-value tags remain in {retired_rust_tags:#?}"
    );
    assert!(
        retired_python_tags.is_empty(),
        "retired Python control-value tags remain in {retired_python_tags:#?}"
    );
    let python_mapper_violations = python_mapper_violations(&python_client);
    assert!(
        python_mapper_violations.is_empty(),
        "Python control mapper lost canonical authority fences: {python_mapper_violations:#?}"
    );
    assert!(
        retired_typescript_tags.is_empty(),
        "retired TypeScript control-value adapters remain in {retired_typescript_tags:#?}"
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

    let response_mirror = r"
        struct ControlSurfaceListResponse {
            surfaces: Vec<ControlSurfaceDocument>,
        }
    ";
    assert_eq!(
        disallowed_control_surface_list_responses("fixture.rs", response_mirror),
        ["ControlSurfaceListResponse"]
    );

    let query_mirror = r"
        struct BorrowedControlSurfaceQuery<'a> {
            pub device_id: Option<&'a str>,
            pub driver_id: Option<&'a str>,
            pub include_driver: bool,
        }
    ";
    assert_eq!(
        disallowed_control_surface_queries("fixture.rs", query_mirror),
        ["BorrowedControlSurfaceQuery"]
    );

    let retired_rust_fixture = r#"
        fn effect_payload() -> Value {
            json!({ "float": 0.5 })
        }
    "#;
    assert_eq!(
        retired_rust_external_tags("fixture.rs", retired_rust_fixture),
        ["fixture.rs: effect_payload"]
    );
    assert_eq!(
        retired_kind_tags(r#"{"kind": "integer", "value": 7}"#),
        ["integer"]
    );
    assert_eq!(
        retired_value_block_tags(
            "fixture.rs",
            r#"json!({ "values": { "count": { "kind": "string", "value": "7" } } })"#,
        ),
        ["fixture.rs: values uses string"]
    );

    let permissive_python_mapper = r#"
        result = {str(key): item for key, item in value.items()}
        if len(value) == 4 and all(isinstance(channel, int | float) for channel in value):
            return {"kind": "color_linear", "value": value}
    "#;
    let mapper_violations = python_mapper_violations(permissive_python_mapper);
    assert!(mapper_violations.contains(&"permissive tagged envelope copying"));
    assert!(mapper_violations.contains(&"bare four-number color guessing"));
}

// ── Variant-space parity ────────────────────────────────────────────────────

/// The four Rust enumerations that must enumerate the same variant space.
const PARITY_ENUMS: [(&str, &str); 4] = [
    (CANONICAL_DEFINITION, "ControlValue"),
    (CANONICAL_WIRE, "ControlValueRef"),
    (CANONICAL_WIRE, "ControlValueWire"),
    (CANONICAL_KIND, "ControlValueKind"),
];

fn enum_variant_names(source: &str, wanted: &str) -> Vec<String> {
    let declaration = declarations_after(source, "enum")
        .into_iter()
        .find_map(|(name, declaration)| (name == wanted).then_some(declaration))
        .unwrap_or_else(|| panic!("{wanted} is declared where the fence expects it"));

    let open = declaration.find('{').expect("an enum body opens");
    let body = &declaration[open + 1..declaration.len() - 1];

    let mut variants = Vec::new();
    let mut depth = 0_usize;
    let mut at_variant_start = true;
    let mut cursor = 0;
    let bytes = body.as_bytes();
    while cursor < bytes.len() {
        let rest = &body[cursor..];
        if depth == 0 && rest.starts_with("//") {
            cursor += rest.find('\n').map_or(rest.len(), |offset| offset + 1);
            continue;
        }
        let character = body[cursor..].chars().next().expect("cursor stays aligned");
        match character {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => at_variant_start = true,
            '#' if depth == 0 => {
                // Skip an attribute and whatever bracketed payload follows it.
                cursor += 1;
                continue;
            }
            _ if depth == 0 && at_variant_start && character.is_ascii_uppercase() => {
                let end = body[cursor..]
                    .find(|candidate: char| !is_identifier_character(candidate))
                    .map_or(body.len(), |offset| cursor + offset);
                variants.push(body[cursor..end].to_owned());
                at_variant_start = false;
                cursor = end;
                continue;
            }
            _ if depth == 0 && !character.is_whitespace() => at_variant_start = false,
            _ => {}
        }
        cursor += character.len_utf8();
    }

    variants
}

#[test]
fn every_control_value_enumeration_carries_the_same_variants() {
    let sources = rust_sources();
    let source_for = |wanted: &str| {
        sources
            .iter()
            .find_map(|(path, source)| (path == wanted).then_some(source.as_str()))
            .unwrap_or_else(|| panic!("{wanted} is readable"))
    };

    let canonical = enum_variant_names(source_for(CANONICAL_DEFINITION), "ControlValue");
    assert_eq!(
        canonical.len(),
        ControlValueKind::COUNT,
        "ControlValue declares {} variants but ControlValueKind::COUNT is {}",
        canonical.len(),
        ControlValueKind::COUNT
    );

    for (path, name) in PARITY_ENUMS {
        assert_eq!(
            enum_variant_names(source_for(path), name),
            canonical,
            "{name} in {path} drifted from the ControlValue variant space"
        );
    }

    let declared_tags: Vec<String> = canonical.iter().map(|name| snake_case(name)).collect();
    let wire_tags: Vec<String> = ControlValueKind::ALL
        .iter()
        .map(|kind| kind.wire_tag().to_owned())
        .collect();
    assert_eq!(
        wire_tags, declared_tags,
        "ControlValueKind::wire_tag disagrees with the serde snake_case tags"
    );
}

fn snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}
