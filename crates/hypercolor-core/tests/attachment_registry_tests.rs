use tempfile::tempdir;

use hypercolor_core::attachment::{AttachmentRegistry, AttachmentRegistryError, TemplateFilter};
use hypercolor_types::attachment::{
    AttachmentCanvasSize, AttachmentCategory, AttachmentCompatibility, AttachmentOrigin,
    AttachmentTemplate, AttachmentTemplateManifest,
};
use hypercolor_types::spatial::{LedTopology, NormalizedPosition};

fn sample_template(id: &str, origin: AttachmentOrigin) -> AttachmentTemplate {
    AttachmentTemplate {
        id: id.to_owned(),
        name: "Sample Template".to_owned(),
        category: AttachmentCategory::Fan,
        origin,
        description: "Sample".to_owned(),
        vendor: "Hypercolor".to_owned(),
        default_size: AttachmentCanvasSize::default(),
        topology: LedTopology::Ring {
            count: 16,
            start_angle: 0.0,
            direction: hypercolor_types::spatial::Winding::Clockwise,
        },
        compatible_slots: vec![AttachmentCompatibility {
            families: vec!["prismrgb".to_owned()],
            models: vec!["prism_8".to_owned()],
            slots: vec!["channel-1".to_owned()],
        }],
        tags: vec!["sample".to_owned()],
        led_names: None,
        led_mapping: None,
        image_url: None,
        physical_size_mm: None,
    }
}

#[test]
fn load_builtins_embeds_generated_catalog() {
    let mut registry = AttachmentRegistry::new();
    let loaded = registry.load_builtins().expect("load built-ins");

    assert_eq!(loaded, 209);
    assert_eq!(registry.builtin_count(), 209);
    assert!(registry.get("lian-li-sl-infinity-fan").is_some());
    assert!(registry.get("corsair-ql-fan").is_some());
}

#[test]
fn builtins_drop_external_source_metadata() {
    let mut registry = AttachmentRegistry::new();
    registry.load_builtins().expect("load built-ins");

    let templates = registry.list(&TemplateFilter::default());
    assert!(!templates.is_empty());
    assert!(templates.iter().all(|template| {
        !template
            .description
            .to_ascii_lowercase()
            .contains("imported from")
    }));
}

#[test]
fn list_filters_by_vendor_category_and_query() {
    let mut registry = AttachmentRegistry::new();
    registry.load_builtins().expect("load built-ins");

    let templates = registry.list(&TemplateFilter {
        vendor: Some("Lian Li".to_owned()),
        category: Some(AttachmentCategory::Fan),
        query: Some("infinity".to_owned()),
        ..TemplateFilter::default()
    });

    assert!(!templates.is_empty());
    assert!(
        templates
            .iter()
            .all(|template| template.vendor == "Lian Li")
    );
    assert!(
        templates
            .iter()
            .all(|template| template.category == AttachmentCategory::Fan)
    );
    assert!(
        templates
            .iter()
            .any(|template| template.id == "lian-li-sl-infinity-fan")
    );
}

#[test]
fn compatible_with_uses_slot_family_model_and_led_budget() {
    let mut registry = AttachmentRegistry::new();
    registry
        .register(sample_template("sample-ring", AttachmentOrigin::BuiltIn))
        .expect("register built-in");
    registry
        .register(AttachmentTemplate {
            id: "sample-too-large".to_owned(),
            topology: LedTopology::Ring {
                count: 64,
                start_angle: 0.0,
                direction: hypercolor_types::spatial::Winding::Clockwise,
            },
            ..sample_template("sample-too-large", AttachmentOrigin::BuiltIn)
        })
        .expect("register large built-in");

    let compatible = registry.compatible_with("prismrgb", Some("prism_8"), "channel-1", 20);
    assert_eq!(compatible.len(), 1);
    assert_eq!(compatible[0].id, "sample-ring");
}

#[test]
fn register_allows_user_overwrite_but_rejects_builtin_conflicts() {
    let mut registry = AttachmentRegistry::new();
    registry
        .register(sample_template("sample", AttachmentOrigin::BuiltIn))
        .expect("register built-in");

    let duplicate_builtin = registry.register(sample_template("sample", AttachmentOrigin::BuiltIn));
    assert!(matches!(
        duplicate_builtin,
        Err(AttachmentRegistryError::DuplicateTemplateId(id)) if id == "sample"
    ));

    registry
        .register(sample_template("custom", AttachmentOrigin::User))
        .expect("register user template");
    registry
        .register(AttachmentTemplate {
            description: "Updated".to_owned(),
            ..sample_template("custom", AttachmentOrigin::User)
        })
        .expect("overwrite user template");

    assert_eq!(
        registry.get("custom").expect("user template").description,
        "Updated"
    );
}

#[test]
fn load_user_dir_reads_nested_template_tree() {
    let dir = tempdir().expect("tempdir");
    let nested = dir.path().join("nested");
    std::fs::create_dir_all(&nested).expect("create nested");

    let manifest = AttachmentTemplateManifest {
        schema_version: 1,
        template: AttachmentTemplate {
            id: "user-panel".to_owned(),
            name: "User Panel".to_owned(),
            category: AttachmentCategory::Other("panel".to_owned()),
            origin: AttachmentOrigin::User,
            description: "Custom panel".to_owned(),
            vendor: "User".to_owned(),
            default_size: AttachmentCanvasSize::default(),
            topology: LedTopology::Custom {
                positions: vec![NormalizedPosition::new(0.0, 0.0)],
            },
            compatible_slots: Vec::new(),
            tags: vec!["custom".to_owned()],
            led_names: None,
            led_mapping: None,
            image_url: None,
            physical_size_mm: None,
        },
    };
    let payload = toml::to_string_pretty(&manifest).expect("serialize manifest");
    std::fs::write(nested.join("user-panel.toml"), payload).expect("write manifest");

    let mut registry = AttachmentRegistry::new();
    let loaded = registry.load_user_dir(dir.path()).expect("load user dir");

    assert_eq!(loaded, 1);
    assert_eq!(registry.user_count(), 1);
    assert!(registry.get("user-panel").is_some());
}
