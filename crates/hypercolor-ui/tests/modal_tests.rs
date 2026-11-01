//! Which elements the modal's focus trap counts as Tab stops.

use hypercolor_ui::components::modal::is_tab_stop;

#[test]
fn a_roving_group_member_parked_at_minus_one_is_not_a_tab_stop() {
    // The player sheet's scene radios: only the checked one has tabindex 0.
    // Counting the others let Tab walk off the end of the dialog.
    assert!(!is_tab_stop(-1, false, true));
    assert!(is_tab_stop(0, false, true));
}

#[test]
fn positive_tab_indexes_stay_in_the_sequence() {
    assert!(is_tab_stop(3, false, true));
}

#[test]
fn inert_or_unrendered_elements_cannot_take_focus() {
    assert!(!is_tab_stop(0, true, true));
    assert!(!is_tab_stop(0, false, false));
}
