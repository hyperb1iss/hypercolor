use hypercolor_types::config::{EffectErrorFallbackPolicy, HypercolorConfig};
use hypercolor_types::portable_setup::PortablePreferences;

#[test]
fn portable_preferences_copy_taste_and_preserve_machine_configuration() {
    let mut source = HypercolorConfig::default();
    source.audio.device = "source private endpoint".into();
    source.audio.smoothing = 0.42;
    source.capture.restore_token = Some("private token".into());
    source.capture.saturation = 1.7;
    source.effect_engine.effect_error_fallback = EffectErrorFallbackPolicy::ClearZones;
    let preferences = PortablePreferences::from_config(&source);
    let serialized =
        serde_json::to_string(&preferences).expect("portable backup fixture should be valid");
    assert!(!serialized.contains("private"));
    assert!(!serialized.contains("enabled"));
    assert!(serialized.contains("clear_zones"));
    let mut target = HypercolorConfig::default();
    target.audio.device = "destination device".into();
    target.capture.restore_token = Some("destination token".into());
    let original = serde_json::to_value(&target).expect("portable backup fixture should be valid");
    preferences
        .apply_to_config(&mut target)
        .expect("portable backup fixture should be valid");
    let after = serde_json::to_value(&target).expect("portable backup fixture should be valid");
    assert_eq!(after["audio"]["device"], original["audio"]["device"]);
    assert_eq!(
        after["capture"]["restore_token"],
        original["capture"]["restore_token"]
    );
    assert_eq!(after["drivers"], original["drivers"]);
    assert_eq!(after["network"], original["network"]);
    assert_eq!(after["extensions"], original["extensions"]);
    assert_eq!(
        after["audio"]["smoothing"],
        serde_json::json!(source.audio.smoothing)
    );
    assert_eq!(
        after["capture"]["saturation"],
        serde_json::json!(source.capture.saturation)
    );
}

#[test]
fn preference_validation_rejects_unknown_private_fields_and_invalid_values_before_apply() {
    let mut preferences = PortablePreferences::from_config(&HypercolorConfig::default());
    let mut value =
        serde_json::to_value(&preferences).expect("portable backup fixture should be valid");
    value["audio"]["device"] = "private endpoint".into();
    assert!(serde_json::from_value::<PortablePreferences>(value).is_err());
    preferences
        .capture
        .as_mut()
        .expect("portable backup fixture should be valid")
        .gamma = f32::NAN;
    let mut target = HypercolorConfig::default();
    let original = serde_json::to_value(&target).expect("portable backup fixture should be valid");
    assert!(preferences.apply_to_config(&mut target).is_err());
    assert_eq!(
        serde_json::to_value(&target).expect("portable backup fixture should be valid"),
        original
    );
    preferences
        .capture
        .as_mut()
        .expect("portable backup fixture should be valid")
        .gamma = 1.0;
    preferences
        .shutdown
        .as_mut()
        .expect("portable backup fixture should be valid")
        .color = "#oops".into();
    assert!(preferences.validate().is_err());
}

mod manifest_schema {
    use std::path::Path;

    use hypercolor_types::portable_setup::{
        OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION, PORTABLE_BACKUP_SCHEMA_VERSION,
        PortableBackupDecodeError, PortableBackupV1,
    };

    /// The manifest of a backup exported by v0.9.0, the first release that
    /// wrote backups. Frozen: never edit it to make a test pass.
    const SHIPPED_V1: &str = include_str!("fixtures/portable_backup_v1.json");

    /// The manifest of a backup exported by the first build that wrote
    /// schema 2: the v1 manifest's configuration, upgraded, with one zone
    /// added whose top layer is scoped to one of its two outputs. Frozen:
    /// never edit it to make a test pass.
    const SHIPPED_V2: &str = include_str!("fixtures/portable_backup_v2.json");

    /// The whole acceptance rule of a build that reads only schema 1, as
    /// v0.9.0 does: it decodes the manifest, tolerating layer fields it
    /// does not know, then validates that the version is 1.
    fn a_schema_1_only_build_accepts(manifest: &serde_json::Value) -> bool {
        manifest["schema_version"] == 1
    }

    fn shipped_v1() -> serde_json::Value {
        serde_json::from_str(SHIPPED_V1).expect("the shipped v1 manifest is valid JSON")
    }

    fn decode(document: &serde_json::Value) -> Result<PortableBackupV1, PortableBackupDecodeError> {
        PortableBackupV1::from_manifest_json(
            &serde_json::to_vec(document).expect("test manifests serialize"),
        )
    }

    fn schema_refusal(document: &serde_json::Value) -> String {
        match decode(document) {
            Err(PortableBackupDecodeError::Schema(error)) => {
                assert_eq!(error.path, "schema_version");
                error.message
            }
            other => panic!("expected a schema refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_shipped_v1_manifest_decodes_and_validates() {
        let backup = PortableBackupV1::from_manifest_json(SHIPPED_V1.as_bytes())
            .expect("a v0.9.0 backup manifest decodes");
        backup
            .validate()
            .expect("a v0.9.0 backup manifest validates");
        assert_eq!(backup.schema_version, PORTABLE_BACKUP_SCHEMA_VERSION);
        assert_eq!(backup.setup.name, "Studio");
        assert_eq!(backup.favorites.len(), 1);
        assert!(backup.preferences.is_some());
        assert!(backup.tui_preferences.is_some());
    }

    #[test]
    fn the_shipped_v1_manifest_upgrades_with_nothing_changed_but_its_version() {
        let backup = PortableBackupV1::from_manifest_json(SHIPPED_V1.as_bytes())
            .expect("a v0.9.0 backup manifest decodes");

        let mut renumbered = shipped_v1();
        assert!(a_schema_1_only_build_accepts(&renumbered));
        renumbered["schema_version"] = PORTABLE_BACKUP_SCHEMA_VERSION.into();
        assert_eq!(
            backup,
            serde_json::from_value(renumbered).expect("the renumbered manifest decodes"),
        );
        assert!(
            backup
                .setup
                .scenes
                .values()
                .flat_map(|scene| &scene.zones)
                .flat_map(|zone| &zone.layers)
                .all(|layer| layer.scope.is_none()),
            "no schema 1 layer has a scope"
        );
    }

    #[test]
    fn the_shipped_v2_manifest_decodes_validates_and_carries_a_layer_scope() {
        let backup = PortableBackupV1::from_manifest_json(SHIPPED_V2.as_bytes())
            .expect("a schema 2 backup manifest decodes");
        backup
            .validate()
            .expect("a schema 2 backup manifest validates");
        assert_eq!(backup.schema_version, 2);

        let scoped = backup
            .setup
            .scenes
            .values()
            .flat_map(|scene| &scene.zones)
            .flat_map(|zone| zone.layers.iter().map(move |layer| (zone, layer)))
            .filter_map(|(zone, layer)| Some((zone, layer.scope.as_ref()?)))
            .collect::<Vec<_>>();
        assert_eq!(scoped.len(), 1);
        let (zone, scope) = scoped[0];
        assert_eq!(scope.outputs, ["front-left"]);
        assert!(
            zone.layout
                .zones
                .iter()
                .any(|output| output.id == "front-left"),
            "the scope names an output of its own zone"
        );
        let mut rewritten = serde_json::to_string_pretty(&backup).expect("manifest serializes");
        rewritten.push('\n');
        assert_eq!(
            rewritten, SHIPPED_V2,
            "this build writes the manifest back exactly as it was exported"
        );
    }

    #[test]
    fn a_build_that_reads_only_schema_1_refuses_the_v2_manifest() {
        let manifest: serde_json::Value = serde_json::from_str(SHIPPED_V2).expect("valid JSON");

        assert!(
            !a_schema_1_only_build_accepts(&manifest),
            "a build that would restore the scoped layer onto its whole zone must refuse it"
        );
    }

    #[test]
    fn every_readable_schema_has_a_shipped_fixture() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for schema in OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION..=PORTABLE_BACKUP_SCHEMA_VERSION {
            let path = fixtures.join(format!("portable_backup_v{schema}.json"));
            let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                panic!(
                    "backup schema {schema} is readable but {} is missing ({error}); \
                     add a manifest exported by the release that introduced it",
                    path.display()
                )
            });
            let backup = PortableBackupV1::from_manifest_json(&bytes)
                .unwrap_or_else(|error| panic!("backup schema {schema} decodes: {error}"));
            backup
                .validate()
                .unwrap_or_else(|error| panic!("backup schema {schema} validates: {error}"));
        }
    }

    #[test]
    fn a_newer_backup_is_refused_with_an_upgrade_message() {
        let newer = PORTABLE_BACKUP_SCHEMA_VERSION + 1;
        let mut document = shipped_v1();
        document["schema_version"] = newer.into();
        // A newer schema may add fields this build does not know; the refusal
        // must come from the version, not from an unknown-field error.
        document["field_from_the_future"] = true.into();
        let message = schema_refusal(&document);
        assert!(message.contains(&format!("schema {newer}")), "{message}");
        assert!(message.contains("newer Hypercolor"), "{message}");
        assert!(message.contains("Upgrade Hypercolor"), "{message}");
        assert!(
            message.contains(&format!(
                "{OLDEST_PORTABLE_BACKUP_SCHEMA_VERSION} through {PORTABLE_BACKUP_SCHEMA_VERSION}"
            )),
            "{message}"
        );
    }

    #[test]
    fn a_schema_below_the_floor_is_refused_by_name() {
        let mut document = shipped_v1();
        document["schema_version"] = 0.into();
        let message = schema_refusal(&document);
        assert!(message.contains("schema 0"), "{message}");
        assert!(
            message.contains("older than any this build reads"),
            "{message}"
        );
    }

    #[test]
    fn a_missing_or_malformed_schema_is_refused() {
        let mut missing = shipped_v1();
        missing
            .as_object_mut()
            .expect("manifest is an object")
            .remove("schema_version");
        let malformed = [
            missing,
            {
                let mut document = shipped_v1();
                document["schema_version"] = "1".into();
                document
            },
            {
                let mut document = shipped_v1();
                document["schema_version"] = (-1).into();
                document
            },
            {
                let mut document = shipped_v1();
                document["schema_version"] = 1.5.into();
                document
            },
        ];
        for document in &malformed {
            let message = schema_refusal(document);
            assert!(message.contains("non-negative integer"), "{message}");
        }
    }

    #[test]
    fn a_current_manifest_with_an_unknown_field_is_still_refused() {
        let mut document = shipped_v1();
        document["field_from_nowhere"] = true.into();
        assert!(matches!(
            decode(&document),
            Err(PortableBackupDecodeError::Json(_))
        ));
    }

    #[test]
    fn a_non_object_manifest_is_invalid_json_for_the_schema() {
        for document in [serde_json::json!([]), serde_json::json!("backup")] {
            assert!(matches!(
                decode(&document),
                Err(PortableBackupDecodeError::Json(_))
            ));
        }
    }
}
