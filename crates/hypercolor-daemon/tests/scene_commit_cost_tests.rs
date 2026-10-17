//! Which durable files a scene commit replaces.
//!
//! A control patch is the hottest scene mutation, so these pin its write
//! set: the one file its change lives in, and nothing that would only
//! rewrite bytes the disk already holds.

#![cfg(feature = "persistence-test-hooks")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use hypercolor_core::effect::EffectEntry;
use hypercolor_daemon::app_state::AppState;
use hypercolor_daemon::domain::MutationContext;
use hypercolor_daemon::domain::scene::commit_scene;
use hypercolor_daemon::domain::scene_tree::{PatchLayerControls, patch_layer_controls};
use hypercolor_daemon::persistence::AtomicFileWriter;
use hypercolor_types::control::ControlValue;
use hypercolor_types::effect::{
    ControlDefinition, ControlKind, ControlType, EffectCategory, EffectId, EffectMetadata,
    EffectSource, EffectState,
};
use hypercolor_types::event::SceneChangeReason;
use hypercolor_types::layer::{SceneLayer, SceneLayerId};
use hypercolor_types::scene::{SceneId, SceneKind, SceneMutationMode, ZoneId};
use uuid::Uuid;

struct Fixture {
    state: Arc<AppState>,
    _root: tempfile::TempDir,
    zone_id: ZoneId,
    layer_id: SceneLayerId,
}

/// One writer per durable file a control patch could touch. Each store
/// keeps its destination alive, so these share its replacement counter.
struct Files {
    scenes: AtomicFileWriter,
    runtime_state: AtomicFileWriter,
    driver_inventory: AtomicFileWriter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Replacements {
    scenes: usize,
    runtime_state: usize,
    driver_inventory: usize,
}

impl Files {
    fn of(state: &AppState) -> Self {
        let writer = |path: PathBuf| AtomicFileWriter::new(&path).expect("store writer");
        Self {
            scenes: writer(state.data_dir.join("scenes.json")),
            runtime_state: writer(state.runtime_state_path.clone()),
            driver_inventory: writer(state.driver_host().driver_inventory().path().to_path_buf()),
        }
    }

    fn replacements(&self) -> Replacements {
        Replacements {
            scenes: self.scenes.replacement_count(),
            runtime_state: self.runtime_state.replacement_count(),
            driver_inventory: self.driver_inventory.replacement_count(),
        }
    }
}

fn speed_effect() -> EffectMetadata {
    EffectMetadata {
        id: EffectId::new(Uuid::now_v7()),
        name: "commit-cost".to_owned(),
        author: "test".to_owned(),
        version: "0.1.0".to_owned(),
        description: "commit cost probe".to_owned(),
        category: EffectCategory::Ambient,
        tags: Vec::new(),
        controls: vec![ControlDefinition {
            id: "speed".to_owned(),
            name: "Speed".to_owned(),
            kind: ControlKind::Number,
            control_type: ControlType::Slider,
            default_value: ControlValue::Float(0.25),
            min: Some(0.0),
            max: Some(1.0),
            step: None,
            labels: Vec::new(),
            group: None,
            tooltip: None,
            aspect_lock: None,
            preview_source: None,
            binding: None,
        }],
        presets: Vec::new(),
        audio_reactive: false,
        screen_reactive: false,
        input_reactive: false,
        source: EffectSource::Native {
            path: "builtin/commit-cost".into(),
        },
        license: None,
    }
}

/// An active scene whose primary zone runs one effect layer with a
/// `speed` control. `kind` picks whether the scene store or the runtime
/// projection owns the scene's durable content.
async fn fixture(kind: SceneKind) -> Fixture {
    let root = tempfile::tempdir().expect("fixture directory");
    let data_dir = root.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("data directory");
    let state = Arc::new(AppState::new_with_data_dir(data_dir));

    let effect = speed_effect();
    let _ = state
        .domains
        .effects
        .register(EffectEntry {
            metadata: effect.clone(),
            source_path: "builtin/commit-cost".into(),
            modified: SystemTime::now(),
            state: EffectState::Loading,
        })
        .await;

    let mut scene = state
        .scene_manager
        .snapshot()
        .await
        .active_scene()
        .expect("default scene")
        .clone();
    let layer_id = SceneLayerId::new();
    scene.zones[0].layers = vec![SceneLayer::from_effect(
        layer_id,
        effect.id,
        HashMap::new(),
        HashMap::new(),
        None,
    )];
    let zone_id = scene.zones[0].id;

    let mut mutation = state.scene_manager.begin_mutation().await;
    if kind == SceneKind::Named {
        scene.id = SceneId::new();
        "Commit cost".clone_into(&mut scene.name);
        scene.kind = SceneKind::Named;
        scene.mutation_mode = SceneMutationMode::Live;
        let scene_id = scene.id;
        mutation.create_scene(scene).expect("seed named scene");
        mutation
            .activate(scene_id, None, SceneChangeReason::UserActivate)
            .expect("activate named scene");
    } else {
        mutation.update_scene(scene).expect("seed default scene");
    }
    commit_scene(&state.domains.scene, mutation)
        .await
        .expect("seed commit");

    Fixture {
        state,
        _root: root,
        zone_id,
        layer_id,
    }
}

async fn patch_speed(fixture: &Fixture, speed: f64) {
    patch_layer_controls(
        &fixture.state.domains.scene_tree,
        PatchLayerControls {
            zone_id: fixture.zone_id,
            layer_id: fixture.layer_id,
            values: HashMap::from([("speed".to_owned(), ControlValue::Float(speed))]),
            clear_bindings: Vec::new(),
            expected_revision: None,
        },
        MutationContext::mcp(),
    )
    .await
    .expect("control patch commits");
}

#[tokio::test]
async fn a_named_scene_control_patch_replaces_only_the_scene_store() {
    let fixture = fixture(SceneKind::Named).await;
    let files = Files::of(&fixture.state);
    // The first patch writes the runtime projection that names the scene.
    patch_speed(&fixture, 0.5).await;
    let before = files.replacements();
    let barriers_before = files.scenes.directory_sync_count();
    let syncs_before = files.scenes.file_sync_count();
    let reads_before = files.scenes.verification_read_count();
    let runtime_syncs_before = files.runtime_state.file_sync_count();
    let runtime_barriers_before = files.runtime_state.directory_sync_count();
    let runtime_reads_before = files.runtime_state.verification_read_count();

    patch_speed(&fixture, 0.75).await;

    assert_eq!(
        files.replacements(),
        Replacements {
            scenes: before.scenes + 1,
            ..before
        },
        "one scene-store write; the unchanged runtime projection and the \
         driver inventory stay untouched"
    );
    if cfg!(unix) {
        assert_eq!(
            files.scenes.directory_sync_count(),
            barriers_before + 1,
            "the scene-store write runs one directory barrier"
        );
    }
    assert_eq!(
        files.scenes.file_sync_count(),
        syncs_before + 1,
        "no fsync of scenes.json beyond the write's own"
    );
    assert_eq!(
        files.scenes.verification_read_count(),
        reads_before,
        "the runtime save trusts the commit's receipt instead of the file"
    );
    // The unchanged runtime projection is a verified skip: one read, one
    // file sync, and one directory sync, and no replacement.
    assert_eq!(
        files.runtime_state.verification_read_count(),
        runtime_reads_before + 1
    );
    assert_eq!(
        files.runtime_state.file_sync_count(),
        runtime_syncs_before + 1
    );
    if cfg!(unix) {
        assert_eq!(
            files.runtime_state.directory_sync_count(),
            runtime_barriers_before + 1
        );
    }
}

#[tokio::test]
async fn a_default_scene_control_patch_replaces_only_the_runtime_projection() {
    let fixture = fixture(SceneKind::Ephemeral).await;
    let files = Files::of(&fixture.state);
    patch_speed(&fixture, 0.5).await;
    let before = files.replacements();
    let scene_syncs_before = files.scenes.file_sync_count();
    let scene_reads_before = files.scenes.verification_read_count();

    patch_speed(&fixture, 0.75).await;

    assert_eq!(
        files.replacements(),
        Replacements {
            runtime_state: before.runtime_state + 1,
            ..before
        },
        "the default scene lives in the runtime projection, so the named \
         scene store has nothing new to write"
    );
    assert_eq!(
        files.scenes.verification_read_count(),
        scene_reads_before,
        "an unchanged store the receipts cover is never read"
    );
    assert_eq!(files.scenes.file_sync_count(), scene_syncs_before);
}

#[tokio::test]
async fn a_whole_session_save_rewrites_a_store_edited_from_outside() {
    let fixture = fixture(SceneKind::Named).await;
    let files = Files::of(&fixture.state);
    patch_speed(&fixture, 0.5).await;
    let path = fixture.state.data_dir.join("scenes.json");
    let durable = std::fs::read(&path).expect("read scene store");
    let modified = std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .expect("store mtime");

    // A same-length edit in place with the mtime put back, behind a stamp
    // that misses it.
    let mut edited = durable.clone();
    let digit = edited
        .iter()
        .position(u8::is_ascii_digit)
        .expect("the store holds a digit");
    edited[digit] = if edited[digit] == b'9' {
        b'0'
    } else {
        edited[digit] + 1
    };
    std::fs::write(&path, &edited).expect("edit the store in place");
    std::fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(modified))
        .expect("restore the mtime");
    files.scenes.set_injected_stale_stamps(true);
    let before = files.replacements();

    // A per-commit save trusts the receipts and leaves the edit alone.
    fixture.state.domains.scene.save_runtime_session().await;
    assert_eq!(std::fs::read(&path).expect("read scene store"), edited);

    // A whole-session save verifies the file and puts memory back.
    fixture.state.domains.runtime_session.save().await;
    assert_eq!(std::fs::read(&path).expect("read scene store"), durable);
    assert_eq!(files.replacements().scenes, before.scenes + 1);
    files.scenes.set_injected_stale_stamps(false);
}

#[tokio::test]
async fn scene_commits_leave_the_driver_inventory_to_discovery() {
    let fixture = fixture(SceneKind::Named).await;
    let files = Files::of(&fixture.state);

    for speed in [0.1, 0.2, 0.3] {
        patch_speed(&fixture, speed).await;
    }
    assert_eq!(files.replacements().driver_inventory, 0);

    // A whole-session save still refreshes the inventory, so the patches
    // above would have written it had they refreshed it too.
    fixture.state.domains.runtime_session.save().await;
    if cfg!(feature = "builtin-drivers") {
        assert!(files.replacements().driver_inventory > 0);
    }
}
