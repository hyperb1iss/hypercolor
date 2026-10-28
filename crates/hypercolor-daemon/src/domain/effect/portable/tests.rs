//! Restore evidence covers restart ownership, partial writes, and publication.

use super::*;
use tokio::sync::RwLock;

struct Fixture {
    temp: tempfile::TempDir,
    managed: PathBuf,
    staging: PathBuf,
    blobs: SetupBlobStore,
    registry: Arc<RwLock<EffectRegistry>>,
    gate: Arc<RwLock<()>>,
    operation: Uuid,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("valid isolated effect fixture");
        let managed =
            files::directory(&temp.path().join("effects")).expect("valid isolated effect fixture");
        let staging = temp.path().join("state/operation");
        let blobs =
            SetupBlobStore::open(temp.path().join("blobs")).expect("valid isolated effect fixture");
        Self {
            registry: Arc::new(RwLock::new(EffectRegistry::new(vec![managed.clone()]))),
            gate: Arc::new(RwLock::new(())),
            temp,
            managed,
            staging,
            blobs,
            operation: Uuid::now_v7(),
        }
    }

    fn effect(&self, title: &str) -> PortableEffect {
        let content = format!("<title>{title}</title>");
        let resource = b"<title>Supporting resource</title>";
        let mut files = BTreeMap::new();
        for (path, bytes) in [
            ("content/main.html", content.as_bytes()),
            ("resources/iframe.html", resource.as_slice()),
        ] {
            let descriptor = PortableBlob {
                sha256: files::digest(bytes),
                byte_len: bytes.len() as u64,
            };
            self.blobs
                .stage(&descriptor, bytes)
                .expect("valid isolated effect fixture");
            files.insert(path.to_owned(), descriptor);
        }
        PortableEffect {
            name: title.to_owned(),
            entrypoint: "content/main.html".into(),
            files,
        }
    }

    async fn prepare(
        &self,
        before: &BTreeMap<Uuid, PortableEffect>,
        desired: &BTreeMap<Uuid, PortableEffect>,
    ) -> Result<PreparedPortableEffects, DomainError> {
        prepare(
            Arc::clone(&self.gate).write_owned().await,
            Arc::clone(&self.registry).write_owned().await,
            PortableEffectsRestore {
                before,
                desired,
                blobs: &self.blobs,
                managed_root: &self.managed,
                staging_root: &self.staging,
                operation_id: self.operation,
            },
        )
    }

    async fn install(&self, id: Uuid, effect: &PortableEffect) -> PathBuf {
        let root = files::nested_directory(&self.managed, Path::new("original"))
            .expect("valid isolated effect fixture");
        for (path, blob) in &effect.files {
            files::copy_blob(&self.blobs, blob, &root, Path::new(path))
                .expect("valid isolated effect fixture");
        }
        files::write_bytes(
            &root.join(HTML_EFFECT_BUNDLE_MANIFEST),
            &serde_json::to_vec(&manifest(id, effect)).expect("valid isolated effect fixture"),
        )
        .expect("valid isolated effect fixture");
        let entry = load_html_effect_file(&root.join(&effect.entrypoint))
            .expect("valid isolated effect fixture")
            .expect("valid isolated effect fixture");
        self.registry.write().await.register(entry);
        root
    }
}

#[tokio::test]
async fn portable_effect_prepare_is_private_and_publication_preserves_identity() {
    let fixture = Fixture::new();
    let id = Uuid::now_v7();
    let effect = fixture.effect("Aurora");
    let desired = BTreeMap::from([(id, effect.clone())]);
    let mut prepared = fixture
        .prepare(&BTreeMap::new(), &desired)
        .await
        .expect("valid isolated effect fixture");
    assert_eq!(
        fs::read_dir(&fixture.managed)
            .expect("valid isolated effect fixture")
            .count(),
        0
    );
    assert!(fixture.registry.try_write().is_err());
    assert!(fixture.gate.try_read().is_err());
    assert_eq!(prepared.candidate().len(), 1);
    assert_eq!(
        prepared
            .persist(&PersistenceShutdown::default())
            .expect("valid isolated effect fixture"),
        AtomicWriteWaitOutcome::Written
    );
    prepared.publish();
    drop(prepared);
    assert!(
        fixture
            .registry
            .read()
            .await
            .get(&EffectId::new(id))
            .is_some()
    );
    let mut restart = EffectRegistry::new(vec![fixture.managed.clone()]);
    let report = hypercolor_core::effect::register_html_effects(
        &mut restart,
        std::slice::from_ref(&fixture.managed),
    );
    assert_eq!(report.failed_files(), 0);
    assert_eq!(restart.len(), 1);
    assert_eq!(
        restart
            .get(&EffectId::new(id))
            .expect("valid isolated effect fixture")
            .metadata
            .name,
        "Aurora"
    );
}

#[tokio::test]
async fn portable_effect_restart_resumes_partial_retirement_without_resource_effects() {
    let fixture = Fixture::new();
    #[cfg(unix)]
    let fixture = {
        let mut fixture = fixture;
        let state = fixture.temp.path().join("state");
        fs::create_dir(&state).expect("create staging parent");
        let alias = fixture.temp.path().join("state-alias");
        std::os::unix::fs::symlink(&state, &alias).expect("alias staging parent");
        fixture.staging = alias.join("operation");
        fixture
    };
    let id = Uuid::now_v7();
    let old = fixture.effect("Before");
    let original = fixture.install(id, &old).await;
    let before = BTreeMap::from([(id, old.clone())]);
    let desired = BTreeMap::from([(id, fixture.effect("After"))]);
    let prepared = fixture
        .prepare(&before, &desired)
        .await
        .expect("valid isolated effect fixture");
    prepared
        .persist_bundle(id, &desired[&id], &PersistenceShutdown::default())
        .expect("valid isolated effect fixture");
    assert_eq!(
        prepared.staging_root,
        fixture
            .staging
            .canonicalize()
            .expect("canonical staging root")
    );
    // Fault injection uses the same normalized authority as production retirement.
    let archive =
        files::nested_directory(&prepared.staging_root, Path::new(&format!("retired/{id}")))
            .expect("valid isolated effect fixture");
    files::copy_blob(
        &fixture.blobs,
        &old.files[&old.entrypoint],
        &archive,
        Path::new(&old.entrypoint),
    )
    .expect("valid isolated effect fixture");
    files::remove_verified(&original.join(&old.entrypoint), &old.files[&old.entrypoint])
        .expect("valid isolated effect fixture");
    drop(prepared);
    let mut restarted = EffectRegistry::new(vec![fixture.managed.clone()]);
    let _ = hypercolor_core::effect::register_html_effects(
        &mut restarted,
        std::slice::from_ref(&fixture.managed),
    );
    assert!(
        restarted.is_empty(),
        "incomplete old bundle and pending new bundle stay hidden"
    );
    *fixture.registry.write().await = restarted;
    let mut resumed = fixture
        .prepare(&before, &desired)
        .await
        .expect("valid isolated effect fixture");
    assert_eq!(
        resumed
            .persist(&PersistenceShutdown::default())
            .expect("valid isolated effect fixture"),
        AtomicWriteWaitOutcome::Written
    );
    resumed.publish();
    drop(resumed);
    assert_eq!(
        fs::read_to_string(archive.join(&old.entrypoint)).expect("valid isolated effect fixture"),
        "<title>Before</title>"
    );
    assert!(!original.join(HTML_EFFECT_BUNDLE_MANIFEST).exists());
    let mut restarted = EffectRegistry::new(vec![fixture.managed.clone()]);
    assert_eq!(
        hypercolor_core::effect::register_html_effects(
            &mut restarted,
            std::slice::from_ref(&fixture.managed)
        )
        .failed_files(),
        0
    );
    assert_eq!(restarted.len(), 1);
    assert_eq!(
        restarted
            .get(&EffectId::new(id))
            .expect("valid isolated effect fixture")
            .metadata
            .name,
        "After"
    );
}

#[tokio::test]
async fn portable_effect_retirement_rejects_external_edits_and_keeps_manifest() {
    let fixture = Fixture::new();
    let id = Uuid::now_v7();
    let old = fixture.effect("Original");
    let root = fixture.install(id, &old).await;
    let before = BTreeMap::from([(id, old)]);
    let mut prepared = fixture
        .prepare(&before, &BTreeMap::new())
        .await
        .expect("valid isolated effect fixture");
    fs::write(root.join("new.html"), "<title>External edit</title>")
        .expect("valid isolated effect fixture");
    assert!(prepared.persist(&PersistenceShutdown::default()).is_err());
    assert!(root.join(HTML_EFFECT_BUNDLE_MANIFEST).exists());
    assert!(root.join("content/main.html").exists());
    fs::remove_file(root.join("new.html")).expect("valid isolated effect fixture");
    assert_eq!(
        prepared
            .persist(&PersistenceShutdown::default())
            .expect("valid isolated effect fixture"),
        AtomicWriteWaitOutcome::Written
    );
    prepared.publish();
    drop(prepared);
    assert!(fixture.registry.read().await.is_empty());
}

#[tokio::test]
async fn portable_effect_recovery_rejects_incomplete_or_other_operation_plan() {
    let fixture = Fixture::new();
    let id = Uuid::now_v7();
    let old = fixture.effect("Original");
    fixture.install(id, &old).await;
    let before = BTreeMap::from([(id, old)]);
    drop(
        fixture
            .prepare(&before, &BTreeMap::new())
            .await
            .expect("valid isolated effect fixture"),
    );
    let path = fixture.staging.join("effects-plan.json");
    let mut plan: EffectPlan =
        serde_json::from_reader(fs::File::open(&path).expect("valid isolated effect fixture"))
            .expect("valid isolated effect fixture");
    plan.retired.clear();
    fs::write(
        &path,
        serde_json::to_vec(&plan).expect("valid isolated effect fixture"),
    )
    .expect("valid isolated effect fixture");
    assert!(fixture.prepare(&before, &BTreeMap::new()).await.is_err());
    plan.operation_id = Uuid::now_v7();
    fs::write(
        path,
        serde_json::to_vec(&plan).expect("valid isolated effect fixture"),
    )
    .expect("valid isolated effect fixture");
    assert!(fixture.prepare(&before, &BTreeMap::new()).await.is_err());
}

#[tokio::test]
async fn portable_effect_shutdown_does_not_write_live_files() {
    let fixture = Fixture::new();
    let id = Uuid::now_v7();
    let desired = BTreeMap::from([(id, fixture.effect("New"))]);
    let mut prepared = fixture
        .prepare(&BTreeMap::new(), &desired)
        .await
        .expect("valid isolated effect fixture");
    let shutdown = PersistenceShutdown::default();
    shutdown.writers_stopped();
    assert_eq!(
        prepared
            .persist(&shutdown)
            .expect("valid isolated effect fixture"),
        AtomicWriteWaitOutcome::ShutdownPending
    );
    assert_eq!(
        fs::read_dir(&fixture.managed)
            .expect("valid isolated effect fixture")
            .count(),
        0
    );
}

#[tokio::test]
async fn portable_effect_loose_replacement_preserves_shared_resources() {
    let fixture = Fixture::new();
    let root = &fixture.managed;
    fs::write(root.join("loose.html"), "<title>Loose</title>")
        .expect("valid isolated effect fixture");
    fs::write(root.join("shared.js"), "console.log('shared')")
        .expect("valid isolated effect fixture");
    let entry = load_html_effect_file(&root.join("loose.html"))
        .expect("valid isolated effect fixture")
        .expect("valid isolated effect fixture");
    let id = entry.metadata.id.0;
    fixture.registry.write().await.register(entry);
    let old = PortableEffect {
        name: "Loose".into(),
        entrypoint: "loose.html".into(),
        files: BTreeMap::from([
            (
                "loose.html".into(),
                fixture
                    .blobs
                    .capture(&root.join("loose.html"))
                    .expect("valid isolated effect fixture"),
            ),
            (
                "shared.js".into(),
                fixture
                    .blobs
                    .capture(&root.join("shared.js"))
                    .expect("valid isolated effect fixture"),
            ),
        ]),
    };
    let before = BTreeMap::from([(id, old)]);
    let desired = BTreeMap::from([(id, fixture.effect("Restored"))]);
    let mut prepared = fixture
        .prepare(&before, &desired)
        .await
        .expect("valid isolated effect fixture");
    prepared
        .persist(&PersistenceShutdown::default())
        .expect("valid isolated effect fixture");
    prepared.publish();
    drop(prepared);
    assert!(!root.join("loose.html").exists());
    assert_eq!(
        fs::read_to_string(root.join("shared.js")).expect("valid isolated effect fixture"),
        "console.log('shared')"
    );
    assert_eq!(
        fixture
            .registry
            .read()
            .await
            .get(&EffectId::new(id))
            .expect("valid isolated effect fixture")
            .metadata
            .name,
        "Restored"
    );
}

#[tokio::test]
async fn portable_effect_destination_conflict_is_not_overwritten() {
    let fixture = Fixture::new();
    let id = Uuid::now_v7();
    let desired = BTreeMap::from([(id, fixture.effect("New"))]);
    let mut prepared = fixture
        .prepare(&BTreeMap::new(), &desired)
        .await
        .expect("valid isolated effect fixture");
    let destination = destination(&fixture.managed, fixture.operation, id);
    fs::create_dir(&destination).expect("valid isolated effect fixture");
    fs::write(destination.join("unknown.html"), "keep me").expect("valid isolated effect fixture");
    assert!(prepared.persist(&PersistenceShutdown::default()).is_err());
    assert_eq!(
        fs::read_to_string(destination.join("unknown.html"))
            .expect("valid isolated effect fixture"),
        "keep me"
    );
}

#[tokio::test]
async fn portable_effect_external_content_edit_survives_failed_retirement() {
    let fixture = Fixture::new();
    let id = Uuid::now_v7();
    let old = fixture.effect("Original");
    let root = fixture.install(id, &old).await;
    let before = BTreeMap::from([(id, old.clone())]);
    let mut prepared = fixture
        .prepare(&before, &BTreeMap::new())
        .await
        .expect("valid isolated effect fixture");
    fs::write(
        root.join(&old.entrypoint),
        "<title>Edited externally</title>",
    )
    .expect("valid isolated effect fixture");
    assert!(prepared.persist(&PersistenceShutdown::default()).is_err());
    assert_eq!(
        fs::read_to_string(root.join(&old.entrypoint)).expect("valid isolated effect fixture"),
        "<title>Edited externally</title>"
    );
    assert!(root.join(HTML_EFFECT_BUNDLE_MANIFEST).exists());
}

#[tokio::test]
async fn portable_effect_unowned_source_and_reserved_identity_are_rejected() {
    let fixture = Fixture::new();
    let outside = fixture.temp.path().join("outside.html");
    fs::write(&outside, "<title>Outside</title>").expect("valid isolated effect fixture");
    let entry = load_html_effect_file(&outside)
        .expect("valid isolated effect fixture")
        .expect("valid isolated effect fixture");
    let id = entry.metadata.id.0;
    fixture.registry.write().await.register(entry);
    let before = BTreeMap::from([(
        id,
        PortableEffect {
            name: "Outside".into(),
            entrypoint: "outside.html".into(),
            files: BTreeMap::from([(
                "outside.html".into(),
                fixture
                    .blobs
                    .capture(&outside)
                    .expect("valid isolated effect fixture"),
            )]),
        },
    )]);
    assert!(fixture.prepare(&before, &BTreeMap::new()).await.is_err());
    assert!(outside.exists());
    hypercolor_core::effect::builtin::register_builtin_effects(
        &mut *fixture.registry.write().await,
    );
    let native = fixture.registry.read().await.search("Breathing")[0]
        .metadata
        .id
        .0;
    assert!(
        fixture
            .prepare(
                &BTreeMap::new(),
                &BTreeMap::from([(native, fixture.effect("Imposter"))])
            )
            .await
            .is_err()
    );
}
