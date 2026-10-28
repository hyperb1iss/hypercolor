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
    let original = fs::read(&source).expect("original template bytes");
    let proof = read_template_file(&source).expect("read").expect("proof");
    retire_template(&source, &retired, &proof).expect("first");
    for _ in 0..3 {
        retire_template(&source, &retired, &proof).expect("repeat");
        assert!(!source.exists());
        assert_eq!(fs::read(&retired).expect("retired bytes"), original);
        assert!(template_files(temp.path()).expect("files").is_empty());
    }
}

#[tokio::test]
async fn recovery_proves_retired_templates_before_replacing_missing_ids() {
    let temp = TempDir::new().expect("temp");
    let directory = temp.path().join("attachments");
    let old = template("edited", "Old");
    let removed = template("deleted", "Deleted");
    save(&directory.join("old.toml"), old.clone());
    save(&directory.join("deleted.toml"), removed.clone());
    let mut registry = ComponentRegistry::new();
    registry.load_user_dir(&directory).expect("load");
    let before = BTreeMap::from([(old.id.clone(), old), (removed.id.clone(), removed)]);
    let updated = template("edited", "New");
    let desired = BTreeMap::from([(updated.id.clone(), updated.clone())]);
    let operation_id = Uuid::new_v4();
    let mut prepared = prepare_templates(
        Arc::new(RwLock::new(registry)),
        &before,
        &desired,
        &directory,
        operation_id,
        temp.path(),
    )
    .await
    .expect("prepare");
    assert_eq!(
        prepared
            .persist_retirements(&PersistenceShutdown::default())
            .expect("retire"),
        AtomicWriteWaitOutcome::Written
    );
    drop(prepared);
    let mut restarted = ComponentRegistry::new();
    restarted
        .load_user_dir(&directory)
        .expect("partial startup");
    assert!(restarted.is_empty());
    let restarted = Arc::new(RwLock::new(restarted));
    assert!(
        prepare_templates(
            restarted.clone(),
            &before,
            &desired,
            &directory,
            operation_id,
            temp.path()
        )
        .await
        .is_err(),
        "normal CAS never accepts missing entries"
    );
    assert!(
        prepare_templates_recovery(
            restarted.clone(),
            &before,
            &desired,
            &directory,
            Uuid::new_v4(),
            temp.path()
        )
        .await
        .is_err(),
        "another operation cannot use retirement evidence"
    );
    let mut resumed = prepare_templates_recovery(
        restarted,
        &before,
        &desired,
        &directory,
        operation_id,
        temp.path(),
    )
    .await
    .expect("recover exact operation");
    resumed
        .persist(&PersistenceShutdown::default())
        .expect("persist");
    resumed.publish();
    let mut final_registry = ComponentRegistry::new();
    final_registry
        .load_user_dir(&directory)
        .expect("final startup");
    assert_eq!(final_registry.get("edited"), Some(&updated));
    assert!(final_registry.get("deleted").is_none());
}

#[tokio::test]
async fn recovery_rejects_changed_retirement_content_and_unrelated_new_templates() {
    let temp = TempDir::new().expect("temp");
    let directory = temp.path().join("attachments");
    let old = template("old", "Old");
    let before = BTreeMap::from([(old.id.clone(), old)]);
    let operation_id = Uuid::new_v4();
    save(
        &directory.join(format!("old.retired-{operation_id}")),
        template("old", "Tampered"),
    );
    let registry = Arc::new(RwLock::new(ComponentRegistry::new()));
    assert!(
        prepare_templates_recovery(
            registry.clone(),
            &before,
            &BTreeMap::new(),
            &directory,
            operation_id,
            temp.path()
        )
        .await
        .is_err()
    );
    registry
        .write()
        .await
        .register(template("unrelated", "New concurrent template"))
        .expect("concurrent register");
    assert!(
        prepare_templates_recovery(
            registry,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &directory,
            operation_id,
            temp.path()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn same_id_external_edits_are_preserved_before_prepare_and_before_persist() {
    let temp = TempDir::new().expect("temp");
    let directory = temp.path().join("attachments");
    let path = directory.join("old.toml");
    let old = template("old", "Old");
    let before = BTreeMap::from([(old.id.clone(), old.clone())]);
    let mut registry = ComponentRegistry::new();
    registry.register(old.clone()).expect("register");
    let registry = Arc::new(RwLock::new(registry));
    save(&path, template("old", "External edit before prepare"));
    assert!(
        prepare_templates(
            registry.clone(),
            &before,
            &BTreeMap::new(),
            &directory,
            Uuid::new_v4(),
            temp.path()
        )
        .await
        .is_err()
    );
    save(&path, old);
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
    let mut bytes = fs::read(&path).expect("bytes");
    bytes.extend_from_slice(b"\n# External comment after preparation\n");
    fs::write(&path, &bytes).expect("external edit");
    assert!(prepared.persist(&PersistenceShutdown::default()).is_err());
    assert_eq!(fs::read(path).expect("preserved"), bytes);
}
