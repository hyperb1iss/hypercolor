use hypercolor_ui::pointer_gesture::{
    GestureEnd, HeldButtons, PointerEnd, PointerGesture, Press, button_mask, cancels_press_defaults,
};

const MOUSE: i32 = 1;
const FIRST_FINGER: i32 = 2;
const SECOND_FINGER: i32 = 3;
const PRIMARY: i16 = 0;
const SECONDARY: i16 = 2;

fn live(_: i32, _: &&str) -> bool {
    true
}

fn gone(_: i32, _: &&str) -> bool {
    false
}

fn started(pointer_id: i32, state: &'static str) -> PointerGesture<&'static str> {
    let mut gesture = PointerGesture::new();
    assert_eq!(gesture.press(pointer_id, live), Press::Ready);
    gesture.start(pointer_id, PRIMARY, state);
    gesture
}

#[test]
fn idle_gesture_accepts_a_press_without_asking_about_an_owner() {
    let mut gesture = PointerGesture::<&str>::new();
    assert!(!gesture.is_active());
    let press = gesture.press(MOUSE, |_, _| panic!("no owner to check"));
    assert_eq!(press, Press::Ready);
    assert!(!gesture.is_active());
}

#[test]
fn release_from_the_owner_commits_and_returns_the_state() {
    let mut gesture = started(MOUSE, "drag");
    assert!(gesture.is_active());
    assert_eq!(gesture.state(MOUSE), Some(&"drag"));
    assert_eq!(
        gesture.end(MOUSE, PointerEnd::Up),
        Some((GestureEnd::Commit, "drag"))
    );
    assert!(!gesture.is_active());
}

#[test]
fn pointercancel_from_the_owner_cancels() {
    let mut gesture = started(FIRST_FINGER, "drag");
    assert_eq!(
        gesture.end(FIRST_FINGER, PointerEnd::Cancel),
        Some((GestureEnd::Cancel, "drag"))
    );
    assert!(!gesture.is_active());
}

#[test]
fn capture_lost_before_the_release_cancels() {
    let mut gesture = started(MOUSE, "drag");
    assert_eq!(
        gesture.end(MOUSE, PointerEnd::LostCapture),
        Some((GestureEnd::Cancel, "drag"))
    );
}

#[test]
fn lost_capture_trailing_a_release_or_cancel_is_a_no_op() {
    let mut gesture = started(MOUSE, "drag");
    assert!(gesture.end(MOUSE, PointerEnd::Up).is_some());
    assert_eq!(gesture.end(MOUSE, PointerEnd::LostCapture), None);

    let mut gesture = started(FIRST_FINGER, "drag");
    assert!(gesture.end(FIRST_FINGER, PointerEnd::Cancel).is_some());
    assert_eq!(gesture.end(FIRST_FINGER, PointerEnd::LostCapture), None);
}

#[test]
fn second_finger_cannot_press_move_or_end_a_live_gesture() {
    let mut gesture = started(FIRST_FINGER, "drag");

    assert_eq!(gesture.press(SECOND_FINGER, live), Press::Busy);
    assert_eq!(gesture.state(SECOND_FINGER), None);
    assert_eq!(gesture.state_mut(SECOND_FINGER), None);
    assert_eq!(gesture.end(SECOND_FINGER, PointerEnd::Up), None);
    assert_eq!(gesture.end(SECOND_FINGER, PointerEnd::Cancel), None);
    assert_eq!(gesture.end(SECOND_FINGER, PointerEnd::LostCapture), None);

    // The first finger still owns the drag after all of that.
    assert_eq!(gesture.state(FIRST_FINGER), Some(&"drag"));
    assert_eq!(
        gesture.end(FIRST_FINGER, PointerEnd::Up),
        Some((GestureEnd::Commit, "drag"))
    );
}

#[test]
fn busy_check_asks_about_the_owner_not_the_new_pointer() {
    let mut gesture = started(FIRST_FINGER, "drag");
    let mut asked = None;
    let press = gesture.press(SECOND_FINGER, |owner, state| {
        asked = Some((owner, *state));
        true
    });
    assert_eq!(press, Press::Busy);
    assert_eq!(asked, Some((FIRST_FINGER, "drag")));
}

#[test]
fn press_from_the_owner_means_its_release_was_lost() {
    let mut gesture = started(MOUSE, "stale");
    let press = gesture.press(MOUSE, |_, _| panic!("same pointer is stale outright"));
    assert_eq!(press, Press::Stale("stale"));
    assert!(!gesture.is_active());

    gesture.start(MOUSE, PRIMARY, "fresh");
    assert_eq!(gesture.state(MOUSE), Some(&"fresh"));
}

#[test]
fn owner_without_capture_is_stale_and_yields_to_the_new_pointer() {
    let mut gesture = started(FIRST_FINGER, "stale");
    assert_eq!(gesture.press(SECOND_FINGER, gone), Press::Stale("stale"));
    gesture.start(SECOND_FINGER, PRIMARY, "fresh");
    assert_eq!(gesture.state(FIRST_FINGER), None);
    assert_eq!(gesture.state(SECOND_FINGER), Some(&"fresh"));
}

#[test]
fn state_mut_updates_the_live_gesture() {
    let mut gesture = PointerGesture::new();
    assert_eq!(gesture.press(MOUSE, |_, _: &u32| true), Press::Ready);
    gesture.start(MOUSE, PRIMARY, 1_u32);
    if let Some(state) = gesture.state_mut(MOUSE) {
        *state += 1;
    }
    assert_eq!(
        gesture.end(MOUSE, PointerEnd::Up),
        Some((GestureEnd::Commit, 2))
    );
}

#[test]
fn abandon_drops_the_live_gesture_without_an_event() {
    let mut gesture = started(MOUSE, "drag");
    assert_eq!(gesture.abandon(), Some("drag"));
    assert!(!gesture.is_active());
    assert_eq!(gesture.abandon(), None);
    assert_eq!(gesture.end(MOUSE, PointerEnd::Up), None);
}

#[test]
fn current_reads_the_live_gesture_for_any_caller() {
    let mut gesture = PointerGesture::<u32>::new();
    assert_eq!(gesture.current(), None);
    assert_eq!(gesture.current_mut(), None);

    gesture.start(FIRST_FINGER, PRIMARY, 7);
    assert_eq!(gesture.current(), Some(&7));
    if let Some(state) = gesture.current_mut() {
        *state = 8;
    }
    assert_eq!(gesture.state(FIRST_FINGER), Some(&8));
    assert!(gesture.end(FIRST_FINGER, PointerEnd::Up).is_some());
    assert_eq!(gesture.current(), None);
}

#[test]
fn only_touch_presses_keep_their_default_actions() {
    assert!(cancels_press_defaults("mouse"));
    assert!(cancels_press_defaults("pen"));
    // Unknown pointer types behave like a mouse.
    assert!(cancels_press_defaults(""));
    assert!(!cancels_press_defaults("touch"));
}

#[test]
fn button_masks_follow_the_pointer_events_buttons_bits() {
    assert_eq!(button_mask(0), 1);
    assert_eq!(button_mask(1), 4);
    assert_eq!(button_mask(2), 2);
    assert_eq!(button_mask(3), 8);
    assert_eq!(button_mask(4), 16);
    assert_eq!(button_mask(5), 32);
    assert_eq!(button_mask(-1), 0);
    assert_eq!(button_mask(9), 0);
}

#[test]
fn releasing_the_pressing_button_mid_chord_ends_the_gesture() {
    let mut gesture = PointerGesture::new();
    assert_eq!(gesture.press(MOUSE, live), Press::Ready);
    gesture.start(MOUSE, PRIMARY, "drag");
    // Primary held, then secondary added: still dragging.
    assert!(!gesture.press_released(MOUSE, 0b01));
    assert!(!gesture.press_released(MOUSE, 0b11));
    // Primary released while secondary stays down arrives as a move.
    assert!(gesture.press_released(MOUSE, 0b10));
    // Another pointer's moves never end this gesture.
    assert!(!gesture.press_released(FIRST_FINGER, 0));
}

#[test]
fn a_secondary_button_drag_ends_when_that_button_lifts() {
    let mut gesture = PointerGesture::new();
    gesture.start(MOUSE, SECONDARY, "drag");
    assert!(!gesture.press_released(MOUSE, 0b10));
    assert!(gesture.press_released(MOUSE, 0b01));
    assert!(gesture.press_released(MOUSE, 0));
}

#[test]
fn a_touch_contact_stays_pressed_until_it_lifts() {
    let mut gesture = PointerGesture::new();
    gesture.start(FIRST_FINGER, PRIMARY, "drag");
    assert!(!gesture.press_released(FIRST_FINGER, 1));
    assert!(gesture.press_released(FIRST_FINGER, 0));
}

#[test]
fn an_unknown_button_is_never_released_by_a_move() {
    let mut gesture = PointerGesture::new();
    gesture.start(MOUSE, -1, "drag");
    assert!(!gesture.press_released(MOUSE, 0));
}

#[test]
fn an_idle_gesture_has_nothing_to_release() {
    let gesture = PointerGesture::<&str>::new();
    assert!(!gesture.press_released(MOUSE, 0));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Button {
    Left,
    Right,
}

#[test]
fn one_pointer_presses_and_releases_a_button_once_each() {
    let mut held = HeldButtons::new();
    assert!(held.press(Button::Left, MOUSE));
    assert!(held.is_down(Button::Left));
    assert!(held.release(Button::Left, MOUSE));
    assert!(!held.is_down(Button::Left));
}

#[test]
fn two_pointers_on_one_button_release_it_when_the_last_lets_go() {
    for (first_up, second_up) in [(FIRST_FINGER, SECOND_FINGER), (SECOND_FINGER, FIRST_FINGER)] {
        let mut held = HeldButtons::new();
        assert!(held.press(Button::Left, FIRST_FINGER));
        // Already down: the second finger must not press it again.
        assert!(!held.press(Button::Left, SECOND_FINGER));
        assert!(!held.release(Button::Left, first_up));
        assert!(held.is_down(Button::Left));
        assert!(held.release(Button::Left, second_up));
        assert!(!held.is_down(Button::Left));
    }
}

#[test]
fn cancelling_one_pointer_keeps_buttons_another_pointer_holds() {
    let mut held = HeldButtons::new();
    held.press(Button::Left, FIRST_FINGER);
    held.press(Button::Left, SECOND_FINGER);
    held.press(Button::Right, MOUSE);
    assert_eq!(held.release_pointer(SECOND_FINGER), Vec::<Button>::new());
    assert!(held.is_down(Button::Left));
    assert_eq!(held.release_pointer(FIRST_FINGER), vec![Button::Left]);
    assert!(held.is_down(Button::Right));
    assert_eq!(held.release_pointer(MOUSE), vec![Button::Right]);
    // Nothing left to release, and a repeat is harmless.
    assert_eq!(held.release_pointer(MOUSE), Vec::<Button>::new());
}

#[test]
fn a_release_for_an_unseen_press_is_forwarded_unless_someone_holds_it() {
    let mut held = HeldButtons::new();
    assert!(held.release(Button::Left, MOUSE));
    held.press(Button::Left, FIRST_FINGER);
    assert!(!held.release(Button::Left, MOUSE));
    assert!(held.is_down(Button::Left));
}

#[test]
fn release_all_drains_every_held_button() {
    let mut held = HeldButtons::new();
    held.press(Button::Left, FIRST_FINGER);
    held.press(Button::Left, SECOND_FINGER);
    held.press(Button::Right, MOUSE);
    let mut released = held.release_all();
    released.sort_by_key(|button| *button as u8);
    assert_eq!(released, vec![Button::Left, Button::Right]);
    assert!(!held.is_down(Button::Left));
    assert!(held.release_all().is_empty());
}
