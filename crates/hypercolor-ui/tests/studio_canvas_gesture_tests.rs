//! Contract tests for how a canvas gesture lands in the editor: only what
//! the gesture changed, over the editor as it stands, with its press-time
//! snapshot taking in what arrived meanwhile.

use hypercolor_types::spatial::{LedTopology, NormalizedPosition, Output, StripDirection};
use hypercolor_ui::components::layout_builder::gesture::{merge_gesture, rebase_unedited};

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
fn a_release_lands_only_the_gesture_over_a_move_that_arrived_mid_drag() {
    let pressed = [output("a", "one", 0.2), output("b", "two", 0.5)];
    // The gesture dragged "a"; its runtime still holds "b" from the press.
    let mut gestured = pressed.clone();
    gestured[0].position.x = 0.7;
    gestured[0].position.y = 0.3;
    // Another client moved "b" while the drag ran.
    let mut current = pressed.clone();
    current[1].position.x = 0.9;
    let merged = merge_gesture(&current, &pressed, &gestured);
    assert_eq!(merged[0].position.x, 0.7);
    assert_eq!(merged[0].position.y, 0.3);
    assert_eq!(
        merged[1].position.x, 0.9,
        "the remote move survives the release"
    );
}

#[test]
fn a_press_snapshot_takes_in_a_move_that_arrived_mid_drag() {
    let loaded = [output("a", "one", 0.2), output("b", "two", 0.5)];
    let mut pressed = loaded.clone();
    let mut landed = loaded.clone();
    landed[1].position.x = 0.9;
    rebase_unedited(&mut pressed, &loaded, &landed);
    assert_eq!(pressed[0], loaded[0]);
    assert_eq!(
        pressed[1].position.x, 0.9,
        "undoing the drag leaves b where it went"
    );
}

#[test]
fn a_press_snapshot_keeps_its_own_edits_when_an_earlier_write_lands() {
    let saved = [output("a", "one", 0.2), output("b", "two", 0.5)];
    // The press saw "a" at 0.6, an edit still waiting to be written.
    let mut pressed = saved.clone();
    pressed[0].position.x = 0.6;
    // An earlier write of ours landed "a" at 0.4; another client moved "b".
    let mut landed = saved.clone();
    landed[0].position.x = 0.4;
    landed[1].position.x = 0.9;
    rebase_unedited(&mut pressed, &saved, &landed);
    assert_eq!(pressed[0].position.x, 0.6, "the snapshot's own edit stays");
    assert_eq!(pressed[1].position.x, 0.9, "the remote move joins it");
}
