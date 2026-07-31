use std::collections::HashSet;
use std::fs;

#[test]
fn process_allocator_targets_are_feature_gated() {
    let manifest = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("core manifest should be readable");
    let manifest: toml::Value = toml::from_str(&manifest).expect("core manifest should parse");
    let targets = manifest
        .get("test")
        .and_then(toml::Value::as_array)
        .expect("core manifest should declare isolated test targets");

    let mut isolated_paths = HashSet::new();
    for target in targets {
        let required_features = target
            .get("required-features")
            .and_then(toml::Value::as_array);
        let is_allocation_target = required_features.is_some_and(|features| {
            features
                .iter()
                .any(|feature| feature.as_str() == Some("allocation-contract-tests"))
        });
        if is_allocation_target {
            let path = target
                .get("path")
                .and_then(toml::Value::as_str)
                .expect("isolated test target should declare its path");
            isolated_paths.insert(path.to_owned());
        }
    }

    let tests_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    let allocator_marker = ["global", "allocator"].join("_");
    for entry in fs::read_dir(tests_dir).expect("core tests directory should be readable") {
        let entry = entry.expect("core test entry should be readable");
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("core test source should be readable");
        if !source.contains(&allocator_marker) {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("core test path should have a UTF-8 file name");
        let manifest_path = format!("tests/{file_name}");
        assert!(
            isolated_paths.contains(&manifest_path),
            "{manifest_path} owns the process allocator without the allocation-contract-tests gate"
        );
    }
}
