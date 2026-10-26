//! Contract tests for live layout commits: what a commit writes, and how
//! edits wait while a write is in flight.

use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};
use hypercolor_ui::components::layout_builder::commit::{
    CommitQueue, PendingCommit, commit_placements,
};

fn output(id: &str, device: &str, x: f32) -> Output {
    Output {
        id: id.to_owned(),
        name: id.to_owned(),
        device_id: device.to_owned(),
        zone_name: Some("Main".to_owned()),
        position: NormalizedPosition::new(x, 0.5),
        size: NormalizedPosition::new(0.1, 0.1),
        rotation: 0.0,
        scale: 1.0,
        display_order: 0,
        orientation: None,
        topology: LedTopology::Strip {
            count: 10,
            direction: StripDirection::LeftToRight,
        },
        led_positions: Vec::new(),
        led_mapping: None,
        sampling_mode: None,
        edge_behavior: None,
        shape: None,
        shape_preset: None,
        attachment: None,
        brightness: None,
    }
}

#[test]
fn a_commit_writes_the_edited_geometry_in_member_order() {
    let members = [output("a", "one", 0.1), output("b", "two", 0.2)];
    let mut moved = output("b", "two", 0.8);
    moved.rotation = 1.0;
    let edited = [moved, output("a", "one", 0.1)];
    let written = commit_placements(&members, &members, &edited).expect("b moved");
    assert_eq!(written.len(), 2);
    assert_eq!(written[0].id, "a");
    assert_eq!(written[1].id, "b");
    assert_eq!(written[1].position.x, 0.8);
    assert_eq!(written[1].rotation, 1.0);
}

#[test]
fn a_commit_never_adds_drops_or_rebinds_a_member() {
    let members = [output("a", "one", 0.1), output("b", "two", 0.2)];
    // The editor lost "a", holds an output the zone does not, and holds
    // "b" with a different binding than the zone has.
    let edited = [output("stray", "three", 0.9), output("b", "other", 0.7)];
    let baseline = [output("a", "one", 0.1), output("b", "other", 0.2)];
    let written = commit_placements(&members, &baseline, &edited).expect("b moved");
    assert_eq!(
        written
            .iter()
            .map(|output| output.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(written[0], members[0]);
    assert_eq!(written[1].position.x, 0.7);
    assert_eq!(written[1].device_id, "two", "the binding is the zone's");
}

#[test]
fn an_output_whose_editor_copy_was_renamed_still_commits_its_moves() {
    let members = [output("a", "one", 0.1)];
    // The editor copy carries a different channel and name than the zone.
    let mut loaded = members[0].clone();
    loaded.zone_name = Some("Channel 2".to_owned());
    loaded.name = "Desk".to_owned();
    let mut moved = loaded.clone();
    moved.position.x = 0.6;
    let written = commit_placements(&members, &[loaded], &[moved]).expect("a moved");
    assert_eq!(written[0].position.x, 0.6);
    assert_eq!(written[0].zone_name.as_deref(), Some("Main"));
    assert_eq!(written[0].name, "a");
}

#[test]
fn an_edit_queued_behind_a_landing_write_keeps_its_return_to_the_old_value() {
    let saved = vec![output("a", "one", 0.2)];
    let mut queue = CommitQueue::default();
    queue.push(
        "main".to_owned(),
        saved.clone(),
        vec![output("a", "one", 0.4)],
    );
    let written = queue.pop().expect("first write");
    // Moved back to 0.2 while the first write was in flight, against the
    // same saved baseline.
    queue.push("main".to_owned(), saved, vec![output("a", "one", 0.2)]);
    queue.landed(&written);
    let waiting = queue.pop().expect("second write");
    let live = [output("a", "one", 0.4)];
    let rewritten = commit_placements(&live, &waiting.baseline, &waiting.edited)
        .expect("the return to 0.2 is a change from what landed");
    assert_eq!(rewritten[0].position.x, 0.2);
}

#[test]
fn a_landed_write_leaves_other_zones_and_untouched_fields_waiting_as_they_were() {
    let mut queue = CommitQueue::default();
    let saved = vec![output("a", "one", 0.2), output("b", "two", 0.5)];
    queue.push(
        "main".to_owned(),
        saved.clone(),
        vec![output("a", "one", 0.4), output("b", "two", 0.5)],
    );
    let written = queue.pop().expect("first write");
    queue.push("keyboard".to_owned(), saved.clone(), saved.clone());
    let mut resized = saved.clone();
    resized[1].size.x = 0.3;
    queue.push("main".to_owned(), saved.clone(), resized);
    queue.landed(&written);
    let keyboard = queue.pop().expect("keyboard waits first");
    assert_eq!(keyboard.baseline, saved, "another zone keeps its baseline");
    let main = queue.pop().expect("main");
    assert_eq!(main.baseline[0].position.x, 0.4);
    assert_eq!(main.baseline[1], saved[1], "b was not written, so it stays");
}

#[test]
fn a_commit_keeps_what_changed_elsewhere_since_the_editor_loaded() {
    let baseline = [output("a", "one", 0.1), output("b", "two", 0.2)];
    // Another client resized "a" and moved "b" since.
    let mut live = baseline.clone();
    live[0].size.x = 0.4;
    live[1].position.x = 0.6;
    // The user only moved "a".
    let mut edited = baseline.clone();
    edited[0].position.x = 0.9;
    let written = commit_placements(&live, &baseline, &edited).expect("a moved");
    assert_eq!(written[0].position.x, 0.9);
    assert_eq!(written[0].size.x, 0.4, "the remote resize survives");
    assert_eq!(written[1].position.x, 0.6, "the remote move survives");
}

#[test]
fn an_edit_that_changes_no_placement_writes_nothing() {
    let members = [output("a", "one", 0.1)];
    let mut renamed = members[0].clone();
    renamed.name = "Desk".to_owned();
    renamed.brightness = Some(0.5);
    assert_eq!(commit_placements(&members, &members, &[renamed]), None);
    assert_eq!(commit_placements(&[], &[], &[]), None);
}

#[test]
fn queued_edits_keep_the_newest_state_and_the_oldest_baseline() {
    let mut queue = CommitQueue::default();
    assert!(queue.is_empty());
    let loaded = vec![output("a", "one", 0.1)];
    queue.push(
        "main".to_owned(),
        loaded.clone(),
        vec![output("a", "one", 0.5)],
    );
    queue.push(
        "keyboard".to_owned(),
        Vec::new(),
        vec![output("k", "kb", 0.5)],
    );
    queue.push(
        "main".to_owned(),
        vec![output("a", "one", 0.5)],
        vec![output("a", "one", 0.9)],
    );
    assert!(queue.holds("main"));
    assert!(!queue.holds("roli"));

    let PendingCommit {
        zone_id,
        baseline,
        edited,
    } = queue.pop().expect("main first");
    assert_eq!(zone_id, "main");
    assert_eq!(baseline, loaded);
    assert_eq!(edited[0].position.x, 0.9);
    assert!(!queue.holds("main"));
    assert_eq!(
        queue.pop().map(|pending| pending.zone_id).as_deref(),
        Some("keyboard")
    );
    assert_eq!(queue.pop(), None);
    assert!(queue.is_empty());
}
