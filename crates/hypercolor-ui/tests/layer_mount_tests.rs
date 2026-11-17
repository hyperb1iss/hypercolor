//! A layer row stays mounted across a replace of its layer.

use hypercolor_ui::components::layer_panel::mount::{MountedRow, RowKey, RowSlot, inherit_mounts};

const SCENE: &str = "scene";
const ZONE: &str = "zone";
const FACE: &str = "effect:now-playing";
const WASH: &str = "effect:wash";

fn slot(layer_id: &str, source: Option<&str>) -> RowSlot {
    RowSlot {
        scene_id: SCENE.to_owned(),
        zone_id: ZONE.to_owned(),
        layer_id: layer_id.to_owned(),
        source: source.map(str::to_owned),
        on_screen: false,
    }
}

fn mounts(rows: &[MountedRow]) -> Vec<(&str, &str)> {
    rows.iter()
        .map(|row| (row.key.mount.0.as_str(), row.layer_id.as_str()))
        .collect()
}

fn keys(rows: &[MountedRow]) -> Vec<RowKey> {
    rows.iter().map(|row| row.key.clone()).collect()
}

#[test]
fn a_layer_mounts_under_its_own_id_the_first_time_it_is_listed() {
    let rows = inherit_mounts(&[], &[slot("a", Some(FACE)), slot("b", Some(WASH))]);

    assert_eq!(mounts(&rows), [("a", "a"), ("b", "b")]);
    assert_eq!(rows[0].key.mount.1, FACE);
}

#[test]
fn a_replace_hands_the_row_to_the_layer_it_minted() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE)), slot("b", Some(WASH))]);
    // Writing a color value replaced `a`: same place, same source, new id.
    let second = inherit_mounts(&first, &[slot("a2", Some(FACE)), slot("b", Some(WASH))]);

    assert_eq!(keys(&second), keys(&first), "no row is rebuilt");
    assert_eq!(mounts(&second), [("a", "a2"), ("b", "b")]);

    // And again: the row follows every later replace.
    let third = inherit_mounts(&second, &[slot("a3", Some(FACE)), slot("b", Some(WASH))]);
    assert_eq!(keys(&third), keys(&first));
    assert_eq!(mounts(&third), [("a", "a3"), ("b", "b")]);
}

#[test]
fn a_different_source_at_the_same_place_is_a_new_row() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE))]);
    let second = inherit_mounts(&first, &[slot("c", Some(WASH))]);

    assert_ne!(keys(&second), keys(&first));
    assert_eq!(mounts(&second), [("c", "c")]);
}

#[test]
fn reordering_moves_rows_and_rebuilds_none() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE)), slot("b", Some(FACE))]);
    let second = inherit_mounts(&first, &[slot("b", Some(FACE)), slot("a", Some(FACE))]);

    assert_eq!(mounts(&second), [("b", "b"), ("a", "a")]);
}

#[test]
fn an_added_layer_never_takes_a_row_whose_layer_is_still_listed() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE))]);
    // A second layer of the same effect lands on top.
    let second = inherit_mounts(&first, &[slot("n", Some(FACE)), slot("a", Some(FACE))]);

    assert_eq!(mounts(&second), [("n", "n"), ("a", "a")]);
}

#[test]
fn a_removed_layer_takes_its_row_with_it() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE)), slot("b", Some(FACE))]);
    let second = inherit_mounts(&first, &[slot("b", Some(FACE))]);

    assert_eq!(mounts(&second), [("b", "b")]);
}

#[test]
fn a_row_holds_while_the_stack_and_the_list_disagree() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE))]);
    // The list names the minted id before the stack that holds it is read.
    let between = inherit_mounts(&first, &[slot("a2", None)]);
    assert_eq!(keys(&between), keys(&first));
    assert_eq!(mounts(&between), [("a", "a2")]);
    // Or the stack moved first and the list still names the old id.
    let behind = inherit_mounts(&first, &[slot("a", None)]);
    assert_eq!(keys(&behind), keys(&first));

    let settled = inherit_mounts(&between, &[slot("a2", Some(FACE))]);
    assert_eq!(keys(&settled), keys(&first));
    assert_eq!(mounts(&settled), [("a", "a2")]);
}

#[test]
fn a_layer_that_cannot_be_read_and_has_no_row_is_left_out() {
    assert!(inherit_mounts(&[], &[slot("a", None)]).is_empty());
}

#[test]
fn rows_do_not_cross_zones_scenes_or_the_screen_list() {
    let first = inherit_mounts(&[], &[slot("a", Some(FACE))]);

    let mut other_zone = slot("x", Some(FACE));
    other_zone.zone_id = "other".to_owned();
    assert_eq!(mounts(&inherit_mounts(&first, &[other_zone])), [("x", "x")]);

    let mut other_scene = slot("y", Some(FACE));
    other_scene.scene_id = "other".to_owned();
    assert_eq!(
        mounts(&inherit_mounts(&first, &[other_scene])),
        [("y", "y")]
    );

    let mut on_screen = slot("z", Some(FACE));
    on_screen.on_screen = true;
    assert_eq!(mounts(&inherit_mounts(&first, &[on_screen])), [("z", "z")]);
}
