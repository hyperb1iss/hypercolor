use hypercolor_ui::pointer_gesture::{
    GestureEnd, PointerEnd, PointerGesture, Press, cancels_press_defaults,
};

const MOUSE: i32 = 1;
const FIRST_FINGER: i32 = 2;
const SECOND_FINGER: i32 = 3;

fn live(_: i32, _: &&str) -> bool {
    true
}

fn gone(_: i32, _: &&str) -> bool {
    false
}

fn started(pointer_id: i32, state: &'static str) -> PointerGesture<&'static str> {
    let mut gesture = PointerGesture::new();
    assert_eq!(gesture.press(pointer_id, live), Press::Ready);
    gesture.start(pointer_id, state);
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

    gesture.start(MOUSE, "fresh");
    assert_eq!(gesture.state(MOUSE), Some(&"fresh"));
}

#[test]
fn owner_without_capture_is_stale_and_yields_to_the_new_pointer() {
    let mut gesture = started(FIRST_FINGER, "stale");
    assert_eq!(gesture.press(SECOND_FINGER, gone), Press::Stale("stale"));
    gesture.start(SECOND_FINGER, "fresh");
    assert_eq!(gesture.state(FIRST_FINGER), None);
    assert_eq!(gesture.state(SECOND_FINGER), Some(&"fresh"));
}

#[test]
fn state_mut_updates_the_live_gesture() {
    let mut gesture = PointerGesture::new();
    assert_eq!(gesture.press(MOUSE, |_, _: &u32| true), Press::Ready);
    gesture.start(MOUSE, 1_u32);
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

    gesture.start(FIRST_FINGER, 7);
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
