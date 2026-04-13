//! Integration tests for persisted named-scene storage.

use hypercolor_core::scene::{SceneManager, make_scene};
use hypercolor_daemon::scene_store::SceneStore;
use hypercolor_types::scene::SceneId;
use tempfile::TempDir;

#[test]
fn scene_store_round_trips_named_scenes() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut store = SceneStore::new(path.clone());
    store.replace_named_scenes([make_scene("Movie Night"), make_scene("Focus")]);
    store.save().expect("scene store should save");

    let loaded = SceneStore::load(&path).expect("scene store should load");
    let names = loaded
        .list()
        .map(|scene| scene.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(loaded.len(), 2);
    assert!(names.contains(&"Movie Night"));
    assert!(names.contains(&"Focus"));
}

#[test]
fn scene_store_sync_from_manager_filters_default_scene() {
    let tempdir = TempDir::new().expect("tempdir");
    let path = tempdir.path().join("scenes.json");

    let mut manager = SceneManager::with_default();
    let named_scene = make_scene("Relax");
    let named_scene_id = named_scene.id;
    manager.create(named_scene).expect("scene should create");

    let mut store = SceneStore::new(path);
    store.sync_from_manager(&manager);

    assert_eq!(store.len(), 1);
    assert_eq!(
        store.list().next().map(|scene| scene.id),
        Some(named_scene_id)
    );
    assert!(
        store.list().all(|scene| scene.id != SceneId::DEFAULT),
        "the synthesized default scene should never be persisted"
    );
}
