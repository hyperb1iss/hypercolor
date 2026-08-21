use std::path::Path;

const CANONICAL_DEFINITION: &str = "hypercolor-types/src/control/mod.rs";
const FENCE_SOURCE: &str = "hypercolor-types/tests/control_value_authority_tests.rs";

fn rust_sources() -> Vec<(String, String)> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("types crate lives under the workspace crates directory");
    let crates = workspace.join("crates");
    let mut pending = vec![crates.clone()];
    let mut sources = Vec::new();

    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("crate source directory reads") {
            let path = entry.expect("crate source entry reads").path();
            if path.is_dir() {
                if path.file_name().is_none_or(|name| name != "target") {
                    pending.push(path);
                }
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let relative = path
                .strip_prefix(&crates)
                .expect("source remains under crates")
                .to_string_lossy()
                .replace('\\', "/");
            if relative == FENCE_SOURCE {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("Rust source reads");
            sources.push((relative, source));
        }
    }

    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

fn imports_legacy_control_value(source: &str) -> bool {
    const LEGACY_MODULES: [&str; 4] = [
        "hypercolor_types::effect",
        "hypercolor_types::controls",
        "crate::effect",
        "crate::controls",
    ];

    source.split(';').any(|statement| {
        let compact = statement
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
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

#[test]
fn control_value_has_one_definition_and_no_legacy_projection_paths() {
    let mut definitions = Vec::new();
    let mut legacy_paths = Vec::new();

    for (path, source) in rust_sources() {
        let compact = source
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        if compact.contains("enumControlValue{") {
            definitions.push(path.clone());
        }
        if imports_legacy_control_value(&source)
            || compact.contains("asCanonicalControlValue")
            || compact.contains("asDynamicControlValue")
            || compact.contains("EventControlValue")
            || compact.contains("to_effect_wire(")
            || compact.contains("to_driver_wire(")
            || compact.contains("ControlValueProjection")
            || compact.contains("ControlValueConversion")
        {
            legacy_paths.push(path);
        }
    }

    assert_eq!(definitions, [CANONICAL_DEFINITION]);
    assert!(
        legacy_paths.is_empty(),
        "legacy control-value paths remain in {legacy_paths:#?}"
    );
}
