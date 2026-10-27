use super::*;
use tempfile::TempDir;

fn template(id: &str, name: &str) -> ComponentTemplate {
    ComponentTemplate {
        id: id.into(),
        name: name.into(),
        vendor: "Custom".into(),
        origin: ComponentOrigin::User,
        ..ComponentTemplateManifest::default().template
    }
}

fn save(path: &Path, template: ComponentTemplate) {
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(
        path,
        toml::to_string_pretty(&ComponentTemplateManifest {
            template,
            ..ComponentTemplateManifest::default()
        })
        .expect("TOML"),
    )
    .expect("template");
}

#[tokio::test]
async fn replaces_old_paths_and_deletes_only_authored_template_ids() {
    let temp = TempDir::new().expect("temp");
    let directory = temp.path().join("attachments");
    let old = template("edited", "Old name");
    let removed = template("deleted", "Removed");
    let unrelated = template("unrelated", "Keep me");
    save(&directory.join("nested/old-name.toml"), old.clone());
    save(&directory.join("delete-me.toml"), removed.clone());
    save(&directory.join("unrelated.toml"), unrelated.clone());
    let mut registry = ComponentRegistry::new();
    registry.load_user_dir(&directory).expect("load");
    let registry = Arc::new(RwLock::new(registry));
    let before = BTreeMap::from([
        (old.id.clone(), old.clone()),
        (removed.id.clone(), removed),
        (unrelated.id.clone(), unrelated.clone()),
    ]);
    let updated = template("edited", "Updated");
    let desired = BTreeMap::from([
        (updated.id.clone(), updated.clone()),
        (unrelated.id.clone(), unrelated),
    ]);
    let mut prepared = prepare_templates(
        registry.clone(),
        &before,
        &desired,
        &directory,
        Uuid::new_v4(),
        temp.path(),
    )
    .await
    .expect("prepare");
    assert!(directory.join("nested/old-name.toml").exists());
    assert!(registry.try_write().is_err());
    assert_eq!(
        prepared
            .candidate_registry()
            .get("edited")
            .expect("candidate")
            .name,
        "Updated"
    );
    assert_eq!(
        prepared
            .persist(&PersistenceShutdown::default())
            .expect("persist"),
        AtomicWriteWaitOutcome::Written
    );
    prepared.publish();
    drop(prepared);
    let mut restarted = ComponentRegistry::new();
    restarted
        .load_user_dir(&directory)
        .expect("restart has no duplicate IDs");
    assert_eq!(restarted.user_count(), 2);
    assert!(restarted.get("deleted").is_none());
    assert_eq!(restarted.get("edited").expect("edited"), &updated);
    assert!(restarted.get("unrelated").is_some());
}

#[tokio::test]
async fn stale_registry_or_repurposed_disk_file_stops_before_new_writes() {
    let temp = TempDir::new().expect("temp");
    let directory = temp.path().join("attachments");
    let old = template("old", "Old");
    save(&directory.join("old.toml"), old.clone());
    let mut registry = ComponentRegistry::new();
    registry.register(old.clone()).expect("register");
    let registry = Arc::new(RwLock::new(registry));
    assert!(
        prepare_templates(
            registry.clone(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &directory,
            Uuid::new_v4(),
            temp.path()
        )
        .await
        .is_err()
    );
    let before = BTreeMap::from([(old.id.clone(), old)]);
    let mut prepared = prepare_templates(
        registry,
        &before,
        &BTreeMap::new(),
        &directory,
        Uuid::new_v4(),
        temp.path(),
    )
    .await
    .expect("prepare");
    save(
        &directory.join("old.toml"),
        template("someone-else", "Another identity"),
    );
    assert!(prepared.persist(&PersistenceShutdown::default()).is_err());
    assert!(directory.join("old.toml").exists());
}

#[tokio::test]
async fn fresh_templates_support_arbitrary_ids_and_shutdown_keeps_disk_unchanged() {
    let temp = TempDir::new().expect("temp");
    let directory = temp.path().join("attachments");
    let item = template("../../outside", "Safe identity");
    let desired = BTreeMap::from([(item.id.clone(), item)]);
    let registry = Arc::new(RwLock::new(ComponentRegistry::new()));
    let mut prepared = prepare_templates(
        registry.clone(),
        &BTreeMap::new(),
        &desired,
        &directory,
        Uuid::new_v4(),
        temp.path(),
    )
    .await
    .expect("prepare");
    let shutdown = PersistenceShutdown::default();
    shutdown.writers_stopped();
    assert_eq!(
        prepared.persist(&shutdown).expect("shutdown"),
        AtomicWriteWaitOutcome::ShutdownPending
    );
    assert!(template_files(&directory).expect("files").is_empty());
    drop(prepared);
    let mut prepared = prepare_templates(
        registry,
        &BTreeMap::new(),
        &desired,
        &directory,
        Uuid::new_v4(),
        temp.path(),
    )
    .await
    .expect("reprepare");
    assert_eq!(
        prepared
            .persist(&PersistenceShutdown::default())
            .expect("persist"),
        AtomicWriteWaitOutcome::Written
    );
    prepared.publish();
    let files = template_files(&directory).expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0.parent(), Some(directory.as_path()));
}

#[test]
fn repeating_retirement_reestablishes_durability_without_reappearing_toml() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("old.toml");
    let retired = temp.path().join("old.retired");
    save(&source, template("old", "Old"));
    retire_template(&source, &retired, "old").expect("first");
    retire_template(&source, &retired, "old").expect("repeat");
    assert!(!source.exists());
    assert!(retired.exists());
    assert!(template_files(temp.path()).expect("files").is_empty());
}
