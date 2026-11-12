//! The device card's folded output list: when it folds, what the toggle
//! says, and which selections open it.

use std::collections::HashSet;

use hypercolor_ui::pages::studio::pane_model::{
    FOLD_OUTPUTS_ABOVE, fold_label, output_list_folds, selects_part_of,
};

fn ids(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn selection(names: &[&str]) -> HashSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn a_placed_card_lists_a_handful_of_outputs_and_folds_many() {
    assert!(!output_list_folds(true, 2));
    assert!(!output_list_folds(true, FOLD_OUTPUTS_ABOVE));
    assert!(output_list_folds(true, FOLD_OUTPUTS_ABOVE + 1));
}

#[test]
fn an_unassigned_card_always_folds() {
    assert!(output_list_folds(false, 2));
    assert!(output_list_folds(false, 40));
}

#[test]
fn the_toggle_names_outputs_once_placed_and_channels_before() {
    assert_eq!(fold_label(true, 21), "21 outputs");
    assert_eq!(fold_label(false, 4), "4 channels");
}

#[test]
fn picking_one_output_selects_part_of_its_device() {
    let outputs = ids(&["a", "b", "c"]);
    assert!(selects_part_of(&outputs, &selection(&["b"])));
    assert!(selects_part_of(
        &outputs,
        &selection(&["a", "c", "elsewhere"])
    ));
}

#[test]
fn selecting_the_whole_device_or_none_of_it_is_not_a_part() {
    let outputs = ids(&["a", "b", "c"]);
    assert!(!selects_part_of(&outputs, &selection(&["a", "b", "c"])));
    assert!(!selects_part_of(&outputs, &selection(&["elsewhere"])));
    assert!(!selects_part_of(&outputs, &selection(&[])));
    assert!(!selects_part_of(&[], &selection(&["a"])));
}
