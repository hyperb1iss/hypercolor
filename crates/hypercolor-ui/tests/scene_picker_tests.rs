//! The scene picker's rows and actions.

use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::api::scenes::SceneSummary;
use hypercolor_types::scene::SceneMutationMode;
use hypercolor_ui::components::scene_picker::{
    SceneAction, SceneRow, active_saved_scene_id, active_scene_label, active_scene_locked,
    scene_actions, scene_rows,
};

const MOVIE: &str = "33333333-3333-4333-8333-333333333333";
const GAMING: &str = "44444444-4444-4444-8444-444444444444";

fn summary(id: &str, name: &str, mode: SceneMutationMode) -> SceneSummary {
    SceneSummary {
        id: id.to_owned(),
        name: name.to_owned(),
        description: None,
        enabled: true,
        priority: 0,
        mutation_mode: mode,
        layout_id: None,
        activation_brightness: None,
    }
}

fn document(id: &str, name: &str, kind: &str, mode: &str) -> SceneDocument {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": name, "kind": kind, "is_default": kind == "ephemeral",
        "mutation_mode": mode, "revision": 1, "zones": [],
    }))
    .expect("valid scene document")
}

fn default_scene() -> SceneDocument {
    document(
        "00000000-0000-4000-8000-000000000000",
        "Default",
        "ephemeral",
        "live",
    )
}

fn library() -> Vec<SceneSummary> {
    vec![
        summary(MOVIE, "Movie Night", SceneMutationMode::Live),
        summary(GAMING, "Gaming", SceneMutationMode::Snapshot),
    ]
}

#[test]
fn default_leads_the_list_and_is_active_while_the_default_runs() {
    let rows = scene_rows(&library(), Some(&default_scene()));
    assert_eq!(
        rows[0],
        SceneRow {
            id: None,
            label: "Default".to_owned(),
            locked: false,
            active: true,
        }
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["Default", "Movie Night", "Gaming"]
    );
    assert_eq!(rows.iter().filter(|row| row.active).count(), 1);
}

#[test]
fn default_stays_first_while_a_saved_scene_is_active() {
    // The old Studio picker dropped Default once a saved scene ran, so
    // Studio could never return to it.
    let active = document(MOVIE, "Movie Night", "named", "live");
    let rows = scene_rows(&library(), Some(&active));
    assert_eq!(rows[0].id, None);
    assert!(!rows[0].active);
    let checked: Vec<_> = rows.iter().filter(|row| row.active).collect();
    assert_eq!(checked.len(), 1);
    assert_eq!(checked[0].id.as_deref(), Some(MOVIE));
}

#[test]
fn snapshot_locked_scenes_carry_the_lock_marker() {
    let rows = scene_rows(&library(), None);
    let locked: Vec<_> = rows
        .iter()
        .filter(|row| row.locked)
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(locked, ["Gaming"]);

    let gaming = document(GAMING, "Gaming", "named", "snapshot");
    assert!(active_scene_locked(Some(&gaming)));
    assert!(!active_scene_locked(Some(&default_scene())));
}

#[test]
fn an_active_scene_missing_from_the_list_is_folded_in_after_default() {
    // A fresh activation can land before the list refetch does.
    let fresh = document(
        "55555555-5555-4555-8555-555555555555",
        "Fresh",
        "named",
        "live",
    );
    let rows = scene_rows(&library(), Some(&fresh));
    assert_eq!(rows[1].label, "Fresh");
    assert!(rows[1].active);
    assert_eq!(rows.iter().filter(|row| row.active).count(), 1);
}

#[test]
fn a_rig_with_only_default_still_offers_new_scene() {
    let rows = scene_rows(&[], Some(&default_scene()));
    assert_eq!(rows.len(), 1);
    assert_eq!(scene_actions(Some(&default_scene())), [SceneAction::New]);
    assert_eq!(scene_actions(None), [SceneAction::New]);
}

#[test]
fn rename_and_delete_act_only_on_a_saved_scene() {
    let active = document(MOVIE, "Movie Night", "named", "live");
    assert_eq!(
        scene_actions(Some(&active)),
        [SceneAction::New, SceneAction::Rename, SceneAction::Delete]
    );
}

#[test]
fn the_trigger_names_the_saved_scene_or_default() {
    let active = document(MOVIE, "Movie Night", "named", "live");
    assert_eq!(active_scene_label(Some(&active)), "Movie Night");
    assert_eq!(active_scene_label(Some(&default_scene())), "Default");
    assert_eq!(active_scene_label(None), "Default");
    assert_eq!(active_saved_scene_id(Some(&active)).as_deref(), Some(MOVIE));
    assert_eq!(active_saved_scene_id(Some(&default_scene())), None);
}
