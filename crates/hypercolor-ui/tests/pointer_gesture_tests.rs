use hypercolor_ui::pointer_gesture::{
    ButtonEdge, GestureEnd, HeldButtons, PointerEnd, PointerGesture, Press, button_mask,
    cancels_press_defaults,
};
use hypercolor_ui::ws::input::InputEdgeButton;

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

// `PointerEvent.buttons` bits.
const PRIMARY_BIT: u16 = 1;
const SECONDARY_BIT: u16 = 2;
const AUXILIARY_BIT: u16 = 4;
const BACK_BIT: u16 = 8;
const ERASER_BIT: u16 = 32;
const PEN: i32 = 4;

fn reconcile(
    held: &mut HeldButtons<InputEdgeButton>,
    pointer_id: i32,
    mask: u16,
) -> Vec<ButtonEdge<InputEdgeButton>> {
    held.reconcile(pointer_id, InputEdgeButton::held_in(mask))
}

use ButtonEdge::{Pressed, Released};
use InputEdgeButton::{Left, Middle, Right};

#[test]
fn masks_decode_to_wire_buttons_in_wire_order() {
    let all: Vec<_> =
        InputEdgeButton::held_in(AUXILIARY_BIT | SECONDARY_BIT | PRIMARY_BIT).collect();
    assert_eq!(all, vec![Left, Right, Middle]);
    assert_eq!(InputEdgeButton::held_in(0).count(), 0);
    // Back, forward, and the pen eraser have no wire identity.
    assert_eq!(
        InputEdgeButton::held_in(BACK_BIT | 16 | ERASER_BIT).count(),
        0
    );
}

#[test]
fn one_pointer_presses_and_releases_a_button_once_each() {
    let mut held = HeldButtons::new();
    assert_eq!(
        reconcile(&mut held, MOUSE, PRIMARY_BIT),
        vec![Pressed(Left)]
    );
    assert!(held.is_down(Left));
    assert!(held.holds_any(MOUSE));
    // The same mask again (a plain move) changes nothing.
    assert_eq!(reconcile(&mut held, MOUSE, PRIMARY_BIT), vec![]);
    assert_eq!(reconcile(&mut held, MOUSE, 0), vec![Released(Left)]);
    assert!(!held.is_down(Left));
    assert!(!held.holds_any(MOUSE));
}

#[test]
fn a_chorded_press_arriving_as_a_move_presses_the_added_button() {
    let mut held = HeldButtons::new();
    assert_eq!(
        reconcile(&mut held, MOUSE, PRIMARY_BIT),
        vec![Pressed(Left)]
    );
    // Secondary added while primary stays down: pointermove, not pointerdown.
    assert_eq!(
        reconcile(&mut held, MOUSE, PRIMARY_BIT | SECONDARY_BIT),
        vec![Pressed(Right)]
    );
    assert!(held.is_down(Left) && held.is_down(Right));
}

#[test]
fn an_intermediate_release_arriving_as_a_move_releases_only_that_button() {
    let mut held = HeldButtons::new();
    reconcile(&mut held, MOUSE, PRIMARY_BIT);
    reconcile(&mut held, MOUSE, PRIMARY_BIT | SECONDARY_BIT);
    // Primary lifts while secondary stays held.
    assert_eq!(
        reconcile(&mut held, MOUSE, SECONDARY_BIT),
        vec![Released(Left)]
    );
    assert!(!held.is_down(Left));
    assert!(held.is_down(Right));
    // The final pointerup carries an empty mask and releases the rest.
    assert_eq!(reconcile(&mut held, MOUSE, 0), vec![Released(Right)]);
}

#[test]
fn releasing_every_button_at_once_emits_releases_in_button_order() {
    let mut held = HeldButtons::new();
    assert_eq!(
        reconcile(
            &mut held,
            MOUSE,
            PRIMARY_BIT | SECONDARY_BIT | AUXILIARY_BIT
        ),
        vec![Pressed(Left), Pressed(Right), Pressed(Middle)]
    );
    assert_eq!(
        reconcile(&mut held, MOUSE, 0),
        vec![Released(Left), Released(Right), Released(Middle)]
    );
    assert!(!held.holds_any(MOUSE));
}

#[test]
fn a_swap_in_one_event_releases_before_it_presses() {
    let mut held = HeldButtons::new();
    reconcile(&mut held, MOUSE, SECONDARY_BIT);
    assert_eq!(
        reconcile(&mut held, MOUSE, PRIMARY_BIT),
        vec![Released(Right), Pressed(Left)]
    );
}

#[test]
fn another_pointers_mask_never_touches_this_pointers_holds() {
    let mut held = HeldButtons::new();
    assert_eq!(
        reconcile(&mut held, MOUSE, SECONDARY_BIT),
        vec![Pressed(Right)]
    );
    // A finger reporting only primary must not release the mouse's
    // secondary, and its own empty mask later must not either.
    assert_eq!(
        reconcile(&mut held, FIRST_FINGER, PRIMARY_BIT),
        vec![Pressed(Left)]
    );
    assert_eq!(reconcile(&mut held, FIRST_FINGER, 0), vec![Released(Left)]);
    assert!(held.is_down(Right));
    assert_eq!(reconcile(&mut held, MOUSE, 0), vec![Released(Right)]);
}

#[test]
fn two_pointers_on_one_button_release_it_when_the_last_lets_go() {
    for (first_up, second_up) in [(FIRST_FINGER, SECOND_FINGER), (SECOND_FINGER, FIRST_FINGER)] {
        let mut held = HeldButtons::new();
        assert_eq!(
            reconcile(&mut held, FIRST_FINGER, PRIMARY_BIT),
            vec![Pressed(Left)]
        );
        // Already down: the second finger must not press it again.
        assert_eq!(reconcile(&mut held, SECOND_FINGER, PRIMARY_BIT), vec![]);
        assert_eq!(reconcile(&mut held, first_up, 0), vec![]);
        assert!(held.is_down(Left));
        assert_eq!(reconcile(&mut held, second_up, 0), vec![Released(Left)]);
    }
}

#[test]
fn a_pen_barrel_button_is_a_secondary_chord_on_the_tip() {
    let mut held = HeldButtons::new();
    // Tip contact, then the barrel button while touching.
    assert_eq!(reconcile(&mut held, PEN, PRIMARY_BIT), vec![Pressed(Left)]);
    assert_eq!(
        reconcile(&mut held, PEN, PRIMARY_BIT | SECONDARY_BIT),
        vec![Pressed(Right)]
    );
    // Barrel released first, then the tip lifts.
    assert_eq!(
        reconcile(&mut held, PEN, PRIMARY_BIT),
        vec![Released(Right)]
    );
    assert_eq!(reconcile(&mut held, PEN, 0), vec![Released(Left)]);
    // The eraser has no wire button and forwards nothing.
    assert_eq!(reconcile(&mut held, PEN, ERASER_BIT), vec![]);
    assert!(!held.holds_any(PEN));
}

#[test]
fn a_cancelled_pointer_drops_only_its_own_holds() {
    let mut held = HeldButtons::new();
    reconcile(&mut held, FIRST_FINGER, PRIMARY_BIT);
    reconcile(&mut held, SECOND_FINGER, PRIMARY_BIT);
    reconcile(&mut held, MOUSE, SECONDARY_BIT);
    assert_eq!(held.reconcile(SECOND_FINGER, []), vec![]);
    assert!(held.is_down(Left));
    assert_eq!(held.reconcile(FIRST_FINGER, []), vec![Released(Left)]);
    assert!(held.is_down(Right));
    // A repeat for a pointer that holds nothing is harmless.
    assert_eq!(held.reconcile(FIRST_FINGER, []), vec![]);
}

#[test]
fn release_all_drains_every_held_button_in_order() {
    let mut held = HeldButtons::new();
    reconcile(&mut held, FIRST_FINGER, PRIMARY_BIT);
    reconcile(&mut held, SECOND_FINGER, PRIMARY_BIT);
    reconcile(&mut held, MOUSE, AUXILIARY_BIT | SECONDARY_BIT);
    assert_eq!(held.release_all(), vec![Left, Right, Middle]);
    assert!(!held.is_down(Left));
    assert!(held.release_all().is_empty());
}

#[test]
fn a_pointer_that_never_pressed_here_moves_nothing() {
    // A drag that started outside the surface, or a touch that began while
    // the surface ignored presses, reports buttons down on its moves.
    let mut held = HeldButtons::new();
    assert_eq!(
        held.track(MOUSE, InputEdgeButton::held_in(PRIMARY_BIT)),
        vec![]
    );
    assert!(!held.is_engaged(MOUSE));
    assert!(!held.is_down(Left));
}

#[test]
fn a_press_engages_and_tracks_chords_until_it_lifts() {
    let mut held = HeldButtons::new();
    assert_eq!(
        held.press(MOUSE, InputEdgeButton::held_in(PRIMARY_BIT)),
        vec![Pressed(Left)]
    );
    assert!(held.is_engaged(MOUSE));
    assert_eq!(
        held.track(MOUSE, InputEdgeButton::held_in(PRIMARY_BIT | SECONDARY_BIT)),
        vec![Pressed(Right)]
    );
    assert_eq!(held.lift(MOUSE), vec![Released(Left), Released(Right)]);
    assert!(!held.is_engaged(MOUSE));
    // Moves after the lift change nothing, whatever their mask says.
    assert_eq!(
        held.track(MOUSE, InputEdgeButton::held_in(PRIMARY_BIT)),
        vec![]
    );
}

#[test]
fn release_all_ends_engagement_so_a_still_held_button_is_not_pressed_again() {
    // Escape blurs the canvas mid-press: blur releases everything, but the
    // button is physically still down and capture survives.
    let mut held = HeldButtons::new();
    held.press(MOUSE, InputEdgeButton::held_in(PRIMARY_BIT));
    assert_eq!(held.release_all(), vec![Left]);
    assert!(!held.is_engaged(MOUSE));
    assert_eq!(
        held.track(MOUSE, InputEdgeButton::held_in(PRIMARY_BIT)),
        vec![]
    );
    assert!(!held.is_down(Left));
}

#[test]
fn lifting_one_pointer_leaves_another_engaged() {
    let mut held = HeldButtons::new();
    held.press(FIRST_FINGER, InputEdgeButton::held_in(PRIMARY_BIT));
    held.press(MOUSE, InputEdgeButton::held_in(SECONDARY_BIT));
    assert_eq!(held.lift(FIRST_FINGER), vec![Released(Left)]);
    assert!(held.is_engaged(MOUSE));
    assert_eq!(
        held.track(
            MOUSE,
            InputEdgeButton::held_in(SECONDARY_BIT | AUXILIARY_BIT)
        ),
        vec![Pressed(Middle)]
    );
    // A repeated lift for the finger is harmless.
    assert_eq!(held.lift(FIRST_FINGER), vec![]);
}
