//! Every config schema a release ever wrote still loads.
//!
//! Each fixture under `fixtures/config_releases/` is the file a tagged
//! release daemon writes on its first save after starting with no config
//! file: the live config `ConfigManager` holds, serialized by the same
//! `toml::to_string_pretty` its save path uses. That live config is the
//! normalized `HypercolorConfig::default()` plus the entries the builtin
//! driver bundle seeds at startup (the network drivers and every HAL family,
//! with the OpenRGB bridge disabled), as a default-feature Linux release
//! build installs them. The files were produced at each tag by building
//! `hypercolor-driver-builtin` with its default features and running
//! `ConfigManager::new` over a missing file followed by `update` with the
//! seeded default, which is the sequence daemon startup performs. For v0.9.0
//! that output is byte-identical to serializing the daemon's own
//! `startup::default_config()`. Releases that wrote byte-identical files
//! share one fixture, named after the first of them:
//!
//! | Fixture                | Schema | Written by            |
//! | ---------------------- | ------ | --------------------- |
//! | `schema4-v0.2.1.toml`  | 4      | v0.2.1                |
//! | `schema4-v0.3.1.toml`  | 4      | v0.3.1, v0.3.2        |
//! | `schema5-v0.4.0.toml`  | 5      | v0.4.0                |
//! | `schema5-v0.5.0.toml`  | 5      | v0.5.0, v0.5.1        |
//! | `schema5-v0.6.0.toml`  | 5      | v0.6.0 through v0.6.3 |
//! | `schema5-v0.7.0.toml`  | 5      | v0.7.0-rc.1, v0.7.0   |
//! | `schema6-v0.7.1.toml`  | 6      | v0.7.1                |
//! | `schema7-v0.7.2.toml`  | 7      | v0.7.2 through v0.9.0 |
//!
//! Fixtures are frozen history. Never edit one to make a test pass; fix the
//! migration instead. When a release changes the default file, add its file
//! alongside the others.

use std::path::{Path, PathBuf};

use hypercolor_core::config::ConfigManager;
use hypercolor_types::config::{
    CURRENT_SCHEMA_VERSION, DEFAULT_BEAT_SENSITIVITY, DEFAULT_TARGET_FPS,
    OLDEST_MIGRATED_SCHEMA_VERSION,
};

struct ReleaseFixture {
    name: String,
    declared_schema: u32,
    contents: String,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config_releases")
}

fn release_fixtures() -> Vec<ReleaseFixture> {
    let mut fixtures: Vec<ReleaseFixture> = std::fs::read_dir(fixture_dir())
        .expect("release fixture directory should exist")
        .map(|entry| {
            entry
                .expect("release fixture entry should be readable")
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("release fixture names are UTF-8")
                .to_owned();
            let declared_schema = name
                .strip_prefix("schema")
                .and_then(|rest| rest.split_once('-'))
                .and_then(|(schema, _)| schema.parse().ok())
                .unwrap_or_else(|| panic!("{name} should be named schema<N>-<release>.toml"));
            let contents = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{name} should be readable: {error}"));
            ReleaseFixture {
                name,
                declared_schema,
                contents,
            }
        })
        .collect();
    fixtures.sort_by(|left, right| left.name.cmp(&right.name));
    fixtures
}

#[test]
fn every_supported_schema_has_a_release_fixture() {
    let fixtures = release_fixtures();
    for schema in OLDEST_MIGRATED_SCHEMA_VERSION..=CURRENT_SCHEMA_VERSION {
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.declared_schema == schema),
            "schema {schema} is supported but no release fixture covers it; \
             add the default file the release that introduced it wrote"
        );
    }
}

#[test]
fn release_fixtures_declare_the_schema_their_name_claims() {
    for fixture in release_fixtures() {
        let document: toml::Value =
            toml::from_str(&fixture.contents).expect("release fixtures are valid TOML");
        assert_eq!(
            document
                .get("schema_version")
                .and_then(toml::Value::as_integer),
            Some(i64::from(fixture.declared_schema)),
            "{} declares a different schema than its name",
            fixture.name
        );
    }
}

#[test]
fn every_release_default_file_loads_at_the_current_schema() {
    for fixture in release_fixtures() {
        let config = ConfigManager::parse_toml(&fixture.contents)
            .unwrap_or_else(|error| panic!("{} should load: {error:#}", fixture.name));
        assert_eq!(
            config.schema_version, CURRENT_SCHEMA_VERSION,
            "{} should migrate to the current schema",
            fixture.name
        );
        assert_eq!(
            config.daemon.start_scene, "last",
            "{} keeps its startup scene through the start_profile rename",
            fixture.name
        );
        assert_eq!(
            config.daemon.target_fps, DEFAULT_TARGET_FPS,
            "{} should land on the current render target, since every \
             release wrote its own default",
            fixture.name
        );
        assert!(
            (config.audio.beat_sensitivity - DEFAULT_BEAT_SENSITIVITY).abs() < 1e-6,
            "{} should land on the current beat threshold, found {}",
            fixture.name,
            config.audio.beat_sensitivity
        );
        assert!(
            config
                .drivers
                .get("wled")
                .is_some_and(|entry| entry.enabled),
            "{} keeps its seeded network driver entries",
            fixture.name
        );
        assert!(
            config
                .drivers
                .get("openrgb")
                .is_some_and(|entry| !entry.enabled),
            "{} keeps the OpenRGB bridge disabled",
            fixture.name
        );
    }
}

#[test]
fn a_migrated_release_file_survives_the_next_save_and_reload() {
    for fixture in release_fixtures() {
        let loaded = ConfigManager::parse_toml(&fixture.contents)
            .unwrap_or_else(|error| panic!("{} should load: {error:#}", fixture.name));
        let saved = toml::to_string_pretty(&loaded)
            .unwrap_or_else(|error| panic!("{} should serialize: {error}", fixture.name));
        let reloaded = ConfigManager::parse_toml(&saved)
            .unwrap_or_else(|error| panic!("{} should reload: {error:#}", fixture.name));
        let resaved = toml::to_string_pretty(&reloaded)
            .unwrap_or_else(|error| panic!("{} should serialize again: {error}", fixture.name));
        assert_eq!(
            saved, resaved,
            "{} should reach a fixed point after one save",
            fixture.name
        );
    }
}
