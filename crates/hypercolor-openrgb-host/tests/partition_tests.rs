use std::path::Path;

use hypercolor_openrgb_host::{
    DETECTORS_MAP, DETECTORS_SECTION, HostError, MANAGED_DIR_NAME, detector_families,
    detector_prefixes_for_drivers, managed_config_dir, matches_prefix, parse_detector_table,
    partition_detectors, write_detector_partition,
};
use serde_json::{Map, Value, json};

#[test]
fn embedded_detector_table_covers_native_families() {
    let families = detector_families();
    let ids: Vec<&str> = families
        .iter()
        .map(|family| family.driver_id.as_str())
        .collect();
    for expected in ["razer", "lianli", "corsair", "dygma", "nollie", "asus"] {
        assert!(ids.contains(&expected), "missing family {expected}");
    }
    for family in families {
        assert!(
            !family.prefixes.is_empty(),
            "{} has no prefixes",
            family.driver_id
        );
        for detector in &family.detectors {
            assert!(
                matches_prefix(detector, &family.prefixes),
                "{detector} does not match its own family prefixes {:?}",
                family.prefixes
            );
        }
    }
}

#[test]
fn parse_detector_table_rejects_garbage() {
    let error = parse_detector_table("[[family]\ndriver_id = 1").expect_err("should fail");
    assert!(matches!(error, HostError::DetectorTable(_)));
}

#[test]
fn prefixes_for_drivers_are_deduplicated_sorted_and_case_insensitive() {
    let prefixes = detector_prefixes_for_drivers(["ASUS", "razer", "razer", "nope"]);
    assert_eq!(prefixes, vec!["ASUS Aura", "ENE SMBus DRAM", "Razer "]);
    assert!(detector_prefixes_for_drivers(Vec::<String>::new()).is_empty());
}

#[test]
fn prefix_matching_ignores_ascii_case_and_respects_boundaries() {
    let prefixes = ["Razer ", "ASUS Aura"];
    assert!(matches_prefix("Razer Blackwidow Chroma", &prefixes));
    assert!(matches_prefix("RAZER huntsman", &prefixes));
    assert!(matches_prefix("asus aura addressable", &prefixes));
    assert!(!matches_prefix("Razerblade", &prefixes));
    assert!(!matches_prefix("Raz", &prefixes));
    assert!(!matches_prefix("Corsair Lighting Node Pro", &prefixes));
    assert!(!matches_prefix("anything", &Vec::<String>::new()));
}

#[test]
fn managed_config_dir_lives_under_the_data_dir() {
    let dir = managed_config_dir(Path::new("/var/lib/hypercolor"));
    assert_eq!(
        dir.root,
        Path::new("/var/lib/hypercolor").join(MANAGED_DIR_NAME)
    );
    assert_eq!(dir.config_path(), dir.root.join("OpenRGB.json"));
    assert_eq!(AsRef::<Path>::as_ref(&dir), dir.root.as_path());
}

#[test]
fn partition_disables_prefix_matches_and_preserves_unrelated_toggles() {
    let mut existing = Map::new();
    existing.insert("Razer Huntsman".to_owned(), Value::Bool(true));
    existing.insert("Gigabyte RGB Fusion 2 SMBus".to_owned(), Value::Bool(false));
    existing.insert("MSI Mystic Light".to_owned(), Value::Bool(true));
    existing.insert("Corsair Commander Pro".to_owned(), Value::Bool(false));

    let map = partition_detectors(
        &existing,
        &["Razer "],
        Some(&["Nollie 32CH".to_owned(), "Wooting Two".to_owned()]),
    );

    assert_eq!(map["Razer Huntsman"], false, "disabled prefix wins");
    assert_eq!(
        map["Razer Blackwidow Chroma"], false,
        "embedded seed names under a disabled prefix are written false"
    );
    assert_eq!(
        map["Gigabyte RGB Fusion 2 SMBus"], false,
        "unmanaged user toggles are preserved"
    );
    assert_eq!(map["MSI Mystic Light"], true);
    assert_eq!(
        map["Corsair Commander Pro"], true,
        "a managed family that is not disabled is handed back to OpenRGB"
    );
    assert_eq!(map["Nollie 32CH"], true, "seeded managed names are enabled");
    assert_eq!(
        map["Wooting Two"], true,
        "unknown seeded names default to enabled"
    );
}

#[test]
fn write_partition_creates_dir_and_fresh_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());
    assert!(!dir.root.exists());

    let report = write_detector_partition(&dir, &["Lian Li ".to_owned()], None)
        .expect("partition should write");
    assert!(
        report
            .disabled
            .iter()
            .all(|name| name.starts_with("Lian Li "))
    );
    assert!(!report.disabled.is_empty());
    assert!(report.enabled.iter().any(|name| name.starts_with("Razer ")));

    let text = std::fs::read_to_string(dir.config_path()).expect("config written");
    let value: Value = serde_json::from_str(&text).expect("valid JSON");
    let detectors = &value[DETECTORS_SECTION][DETECTORS_MAP];
    assert_eq!(detectors["Lian Li Uni Hub"], false);
    assert_eq!(detectors["Razer Huntsman"], true);
}

#[test]
fn write_partition_preserves_unrelated_keys_and_replaces_in_place() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());
    std::fs::create_dir_all(&dir.root).expect("mkdir");
    let fixture = json!({
        "Detectors": {
            "detectors": {
                "Razer Huntsman": true,
                "Gigabyte RGB Fusion 2 SMBus": false
            },
            "some_future_key": [1, 2, 3]
        },
        "Server": { "port": 6742 },
        "SMBusPlugins": { "enable": true },
        "UserInterface": { "minimize_on_close": true }
    });
    std::fs::write(
        dir.config_path(),
        serde_json::to_string_pretty(&fixture).expect("fixture"),
    )
    .expect("write fixture");

    let report = write_detector_partition(&dir, &["razer ".to_owned()], None)
        .expect("partition should write");
    assert!(report.disabled.contains(&"Razer Huntsman".to_owned()));

    let text = std::fs::read_to_string(dir.config_path()).expect("config rewritten");
    let value: Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(value["Server"]["port"], 6742);
    assert_eq!(value["SMBusPlugins"]["enable"], true);
    assert_eq!(value["UserInterface"]["minimize_on_close"], true);
    assert_eq!(value["Detectors"]["some_future_key"], json!([1, 2, 3]));
    let detectors = &value["Detectors"]["detectors"];
    assert_eq!(detectors["Razer Huntsman"], false);
    assert_eq!(detectors["Gigabyte RGB Fusion 2 SMBus"], false);

    let entries: Vec<_> = std::fs::read_dir(&dir.root)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(entries, vec!["OpenRGB.json"], "no temp files left behind");
}

#[test]
fn write_partition_is_idempotent_when_handing_a_family_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());
    write_detector_partition(&dir, &["Corsair ".to_owned()], None).expect("first write");
    let first: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.config_path()).expect("read"))
            .expect("json");
    assert_eq!(
        first["Detectors"]["detectors"]["Corsair Commander Pro"],
        false
    );

    write_detector_partition(&dir, &Vec::<String>::new(), None).expect("second write");
    let second: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.config_path()).expect("read"))
            .expect("json");
    assert_eq!(
        second["Detectors"]["detectors"]["Corsair Commander Pro"],
        true
    );
}

#[test]
fn write_partition_rejects_invalid_json_and_wrong_shapes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());
    std::fs::create_dir_all(&dir.root).expect("mkdir");

    std::fs::write(dir.config_path(), "{ not json").expect("write");
    let error = write_detector_partition(&dir, &["Razer ".to_owned()], None)
        .expect_err("invalid JSON must not be clobbered");
    assert!(matches!(error, HostError::InvalidExistingConfig { .. }));

    std::fs::write(dir.config_path(), r#"{"Detectors": []}"#).expect("write");
    let error = write_detector_partition(&dir, &["Razer ".to_owned()], None)
        .expect_err("non-object Detectors must not be clobbered");
    assert!(matches!(
        error,
        HostError::UnexpectedConfigShape {
            section: "Detectors",
            ..
        }
    ));

    std::fs::write(dir.config_path(), r#"{"Detectors": {"detectors": 5}}"#).expect("write");
    let error = write_detector_partition(&dir, &["Razer ".to_owned()], None)
        .expect_err("non-object detectors must not be clobbered");
    assert!(matches!(
        error,
        HostError::UnexpectedConfigShape {
            section: "detectors",
            ..
        }
    ));

    std::fs::write(dir.config_path(), "[]").expect("write");
    let error = write_detector_partition(&dir, &["Razer ".to_owned()], None)
        .expect_err("array root must not be clobbered");
    assert!(matches!(
        error,
        HostError::UnexpectedConfigShape {
            section: "root",
            ..
        }
    ));
}

#[test]
fn write_partition_treats_empty_file_as_fresh() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = managed_config_dir(temp.path());
    std::fs::create_dir_all(&dir.root).expect("mkdir");
    std::fs::write(dir.config_path(), "  \n").expect("write");
    write_detector_partition(&dir, &["Nollie ".to_owned()], None).expect("write");
    let value: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.config_path()).expect("read"))
            .expect("json");
    assert_eq!(value["Detectors"]["detectors"]["Nollie 32CH"], false);
}
