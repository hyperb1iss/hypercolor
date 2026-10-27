//! Portable HTML identity survives installation paths and resource documents.

use std::{fs, path::Path, time::SystemTime};

use hypercolor_core::effect::{
    EffectRegistry, HTML_EFFECT_BUNDLE_MANIFEST, HtmlEffectBundleManifest,
    builtin::register_builtin_effects, load_html_effect_file, register_html_effects,
};
use hypercolor_types::effect::EffectId;
use tempfile::TempDir;
use uuid::Uuid;

fn bundle(root: &Path, id: EffectId, entrypoint: &str) {
    let entry = root.join(entrypoint);
    fs::create_dir_all(entry.parent().expect("valid test fixture")).expect("valid test fixture");
    fs::write(entry, "<title>Portable aurora</title>").expect("valid test fixture");
    manifest(root, id, entrypoint);
}

fn manifest(root: &Path, id: EffectId, entrypoint: &str) {
    fs::write(
        root.join(HTML_EFFECT_BUNDLE_MANIFEST),
        serde_json::to_vec(&HtmlEffectBundleManifest {
            schema_version: 1,
            id,
            entrypoint: entrypoint.to_owned(),
        })
        .expect("valid test fixture"),
    )
    .expect("valid test fixture");
}

fn id() -> EffectId {
    EffectId::new(Uuid::now_v7())
}

#[test]
fn identity_survives_different_installation_paths() {
    let temp = TempDir::new().expect("valid test fixture");
    let expected = id();
    for name in ["linux", "windows"] {
        let root = temp.path().join(name);
        bundle(&root, expected, "content/main.html");
        let entry = load_html_effect_file(&root.join("content/main.html"))
            .expect("valid test fixture")
            .expect("valid test fixture");
        assert_eq!(entry.metadata.id, expected);
        assert_eq!(
            entry.source_path,
            fs::canonicalize(root.join("content/main.html")).expect("valid test fixture")
        );
    }
}

#[test]
fn discovery_and_reload_never_register_resource_html() {
    let temp = TempDir::new().expect("valid test fixture");
    let root = temp.path();
    let expected = id();
    bundle(root, expected, "main.html");
    bundle(&root.join("resources"), id(), "iframe.html");
    let mut registry = EffectRegistry::new(vec![root.to_owned()]);
    let report = register_html_effects(&mut registry, &[root.to_owned()]);
    assert_eq!(report.failed_files(), 0);
    assert_eq!(report.loaded_effects, 1);
    assert_eq!(report.replaced_effects, 0);
    assert_eq!(registry.len(), 1);
    let resource = root.join("resources/iframe.html");
    assert!(
        load_html_effect_file(&resource)
            .expect("valid test fixture")
            .is_none()
    );
    assert_eq!(registry.reload_single(&resource).added, 0);
    assert!(
        registry
            .reload_source(
                &resource,
                "<title>Still a resource</title>",
                SystemTime::now()
            )
            .expect("valid test fixture")
            .1
            .is_none()
    );
    assert!(registry.get(&expected).is_some());
    assert_eq!(registry.len(), 1);
}

#[test]
fn builtin_identity_and_builtin_html_markers_are_rejected() {
    let temp = TempDir::new().expect("valid test fixture");
    let mut registry = EffectRegistry::new(vec![temp.path().to_owned()]);
    register_builtin_effects(&mut registry);
    let native = registry.search("Breathing")[0].metadata.id;
    bundle(temp.path(), native, "main.html");
    assert!(load_html_effect_file(&temp.path().join("main.html")).is_err());
    let before = registry
        .get(&native)
        .expect("valid test fixture")
        .source_path
        .clone();
    registry.reload_single(&temp.path().join("main.html"));
    assert_eq!(
        registry
            .get(&native)
            .expect("valid test fixture")
            .source_path,
        before
    );
    manifest(temp.path(), id(), "main.html");
    let result = registry.reload_source(
        &temp.path().join("main.html"),
        r#"<meta builtin-id="breathing">"#,
        SystemTime::now(),
    );
    assert!(result.is_err());
}

#[test]
fn duplicate_identity_cannot_replace_another_source() {
    let temp = TempDir::new().expect("valid test fixture");
    let expected = id();
    bundle(&temp.path().join("a"), expected, "main.html");
    bundle(&temp.path().join("b"), expected, "main.html");
    let mut registry = EffectRegistry::new(vec![temp.path().to_owned()]);
    let report = register_html_effects(&mut registry, &[temp.path().to_owned()]);
    assert!(report.failed_files() > 0);
    assert_eq!(registry.len(), 1);
    let source = registry
        .get(&expected)
        .expect("valid test fixture")
        .source_path
        .clone();
    let other = temp.path().join("b/main.html");
    registry.reload_single(&other);
    assert_eq!(
        registry
            .get(&expected)
            .expect("valid test fixture")
            .source_path,
        source
    );
    assert!(
        registry
            .reload_source(&other, "<title>Imposter</title>", SystemTime::now())
            .is_err()
    );
}

#[test]
fn manifest_retarget_and_removal_replace_stale_identities() {
    let temp = TempDir::new().expect("valid test fixture");
    let expected = id();
    bundle(temp.path(), expected, "z.html");
    fs::write(temp.path().join("b.html"), "<title>Updated</title>").expect("valid test fixture");
    let mut registry = EffectRegistry::new(vec![temp.path().to_owned()]);
    assert_eq!(
        register_html_effects(&mut registry, &[temp.path().to_owned()]).failed_files(),
        0
    );
    manifest(temp.path(), expected, "b.html");
    registry.reload_single(&temp.path().join(HTML_EFFECT_BUNDLE_MANIFEST));
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry
            .get(&expected)
            .expect("valid test fixture")
            .source_path,
        fs::canonicalize(temp.path().join("b.html")).expect("valid test fixture")
    );
    fs::remove_file(temp.path().join(HTML_EFFECT_BUNDLE_MANIFEST)).expect("valid test fixture");
    registry.reload_single(&temp.path().join(HTML_EFFECT_BUNDLE_MANIFEST));
    assert!(registry.get(&expected).is_none());
    assert_eq!(registry.len(), 2);
}

#[test]
fn malformed_manifest_does_not_fall_back_to_path_identity() {
    let temp = TempDir::new().expect("valid test fixture");
    bundle(temp.path(), id(), "main.html");
    fs::write(temp.path().join(HTML_EFFECT_BUNDLE_MANIFEST), "{}").expect("valid test fixture");
    let mut registry = EffectRegistry::new(vec![temp.path().to_owned()]);
    let report = register_html_effects(&mut registry, &[temp.path().to_owned()]);
    assert!(report.failed_files() > 0);
    assert!(registry.is_empty());
}

#[test]
fn entrypoint_validation_rejects_missing_escaping_and_non_html_paths() {
    let temp = TempDir::new().expect("valid test fixture");
    bundle(temp.path(), id(), "main.html");
    for entrypoint in [
        "../main.html",
        "/main.html",
        "C:/main.html",
        "a\\main.html",
        "a/../main.html",
        "main.js",
        "missing.html",
    ] {
        let value = HtmlEffectBundleManifest {
            schema_version: 1,
            id: id(),
            entrypoint: entrypoint.to_owned(),
        };
        assert!(
            value.resolve_entrypoint(temp.path()).is_err(),
            "{entrypoint}"
        );
    }
    let mut value = HtmlEffectBundleManifest {
        schema_version: 1,
        id: id(),
        entrypoint: "main.html".to_owned(),
    };
    assert!(value.resolve_entrypoint(temp.path()).is_ok());
    value.schema_version = 2;
    assert!(value.resolve_entrypoint(temp.path()).is_err());
    value.schema_version = 1;
    value.id = EffectId::new(Uuid::nil());
    assert!(value.resolve_entrypoint(temp.path()).is_err());
}

#[cfg(unix)]
#[test]
fn entrypoint_symlink_cannot_escape_the_bundle() {
    let temp = TempDir::new().expect("valid test fixture");
    let root = temp.path().join("bundle");
    fs::create_dir(&root).expect("valid test fixture");
    fs::write(temp.path().join("outside.html"), "<title>Outside</title>")
        .expect("valid test fixture");
    std::os::unix::fs::symlink(temp.path().join("outside.html"), root.join("main.html"))
        .expect("valid test fixture");
    manifest(&root, id(), "main.html");
    assert!(load_html_effect_file(&root.join("main.html")).is_err());
}
